//! One language server per project generation and language.
//!
//! Build documents share the process their language owns. Activating a tab
//! opens its document on the existing server and detaches the outgoing one;
//! the process itself lives until the project is switched or the app quits, so
//! opening five `.ts` tabs costs one `typescript-language-server`, not five.
//!
//! The registry owns the client, its event loop and the one-retry policy. A
//! session only keeps the handle and document URI it was attached with, which
//! keeps diagnostics and providers scoped to one file.

use super::*;
use crate::editor::{
    lsp::{ProviderSupport, SHUTDOWN_GRACE, release_after_shutdown},
    session::LspStatus,
};
use astrolith_lsp::{Client as LspClient, ClientHandle, Event as LspEvent, LanguageId};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    pin::Pin,
    time::Duration,
};

/// Delay before the single restart a failed server earns.
pub(super) const LSP_RESTART_DELAY_MS: u64 = 500;

/// A test seam for the process the registry would otherwise discover.
#[cfg(test)]
pub(super) type LspStartHook = dyn Fn(PathBuf, LanguageId) -> Pin<Box<dyn Future<Output = Result<LspClient, String>> + Send>>
    + Send
    + Sync;

/// One running server for one language of the open project.
pub(super) struct LspRuntime {
    /// Keeps the process and protocol loop alive for as long as the runtime.
    client: LspClient,
    handle: ClientHandle,
    /// What the handshake advertised; empty until `Ready` arrives.
    support: ProviderSupport,
    /// Whether the handshake already landed.
    ready: bool,
    /// Documents this server has open, by canonical document URI.
    open: BTreeMap<lsp_types::Uri, PathBuf>,
    /// The project generation the process was started for.
    generation: u64,
    /// Whether this run's failure was already surfaced, so the exit that
    /// follows a handshake error does not overwrite its cause.
    failure_seen: bool,
}

/// Every server of the open project, keyed by the language it serves.
#[derive(Default)]
pub(super) struct LspRegistry {
    runtimes: HashMap<LanguageId, LspRuntime>,
    /// Event loops, one per running server; dropping one stops its routing.
    events: HashMap<LanguageId, Task<()>>,
    /// In-flight discovery/start per language, so one language starts once.
    starts: HashMap<LanguageId, Task<()>>,
    /// Pending single restart per failed language.
    restarts: HashMap<LanguageId, Task<()>>,
    /// Language and document whose discovery failed, so one Build attachment
    /// searches once and a different attachment may try again.
    unavailable: HashMap<LanguageId, PathBuf>,
    /// Language and document that already spent their single restart.
    retried: HashMap<LanguageId, PathBuf>,
}

/// The active Build document a server should be attached to.
struct LspTarget {
    path: PathBuf,
    session: Entity<DocumentSession>,
    language: LanguageId,
    uri: lsp_types::Uri,
}

impl ProjectWorkspace {
    /// The active document's path, language and URI, when Build shows it.
    fn lsp_target(&self, cx: &App) -> Option<LspTarget> {
        if !self.source_view {
            return None;
        }
        let root = self.project_root()?;
        let path = self.active.clone()?;
        let session = self.sessions.get(&path)?.clone();
        let canonical = {
            let session = session.read(cx);
            if !session.is_source_view() {
                return None;
            }
            session.canonical_source().to_owned()
        };
        let document = root.join(canonical);
        let language = LanguageId::from_path(&document)?;
        language.server()?;
        let uri = crate::editor::lsp::uri_for(&document)?;
        Some(LspTarget {
            path,
            session,
            language,
            uri,
        })
    }

    /// Point the project's server at the active Build document.
    ///
    /// Idempotent: the mode switch, a tab switch and the completion of an
    /// asynchronous start all funnel through here, and only a document that is
    /// not open yet reaches the server.
    pub(super) fn sync_lsp(&mut self, cx: &mut Context<Self>) {
        let target = self.lsp_target(cx);

        // A failed discovery or a spent restart belongs to one Build
        // attachment of one document; a different attachment starts fresh.
        match target.as_ref() {
            Some(target) => {
                let stale: Vec<LanguageId> = self
                    .lsp
                    .unavailable
                    .keys()
                    .chain(self.lsp.retried.keys())
                    .copied()
                    .filter(|language| {
                        *language != target.language
                            || self
                                .lsp
                                .unavailable
                                .get(language)
                                .is_some_and(|document| *document != target.path)
                            || self
                                .lsp
                                .retried
                                .get(language)
                                .is_some_and(|document| *document != target.path)
                    })
                    .collect();
                for language in stale {
                    self.lsp.unavailable.remove(&language);
                    self.lsp.retried.remove(&language);
                }
            }
            None => {
                self.lsp.unavailable.clear();
                self.lsp.retried.clear();
            }
        }

        // Detach every document that is no longer the Build target. The
        // server stays alive: a tab switch re-opens on the same process.
        let attached: Vec<(PathBuf, Entity<DocumentSession>)> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.read(cx).lsp_attachment().is_some())
            .map(|(path, session)| (path.clone(), session.clone()))
            .collect();
        for (path, session) in attached {
            let keep = target.as_ref().is_some_and(|target| {
                target.path == path
                    && session
                        .read(cx)
                        .lsp_attachment()
                        .is_some_and(|attachment| attachment.uri.as_str() == target.uri.as_str())
            });
            if !keep {
                self.detach_lsp_document(session, cx);
            }
        }

        let Some(target) = target else {
            return;
        };

        // A runtime from another project generation is dead weight.
        if self
            .lsp
            .runtimes
            .get(&target.language)
            .is_some_and(|runtime| runtime.generation != self.generation)
        {
            self.release_lsp_language(target.language, cx);
            self.lsp.retried.remove(&target.language);
        }

        if self.lsp.runtimes.contains_key(&target.language) {
            self.open_lsp_document(&target, cx);
            return;
        }

        if self.lsp.starts.contains_key(&target.language) {
            target.session.update(cx, |session, cx| {
                session.note_lsp_status(LspStatus::Starting, cx)
            });
            return;
        }
        if self.lsp.unavailable.get(&target.language) == Some(&target.path) {
            return;
        }
        let Some(root) = self.project_root() else {
            return;
        };
        self.start_lsp_runtime(&target, root, cx);
    }

    /// Open one document on the language's running server.
    fn open_lsp_document(&mut self, target: &LspTarget, cx: &mut Context<Self>) {
        let Some(runtime) = self.lsp.runtimes.get_mut(&target.language) else {
            return;
        };
        let handle = runtime.handle.clone();
        let support = runtime.support;
        let ready = runtime.ready;
        if !runtime.open.contains_key(&target.uri) {
            runtime.open.insert(target.uri.clone(), target.path.clone());
            let text = target.session.read(cx).buffer.current_text().to_owned();
            handle.open(target.uri.clone(), target.language, text);
        }
        let uri = target.uri.clone();
        let language = target.language;
        target.session.update(cx, |session, cx| {
            session.attach_lsp(handle, uri, language, support, ready, cx)
        });
    }

    /// Detach one document and close it on the server it was open on.
    pub(super) fn detach_lsp_document(
        &mut self,
        session: Entity<DocumentSession>,
        cx: &mut Context<Self>,
    ) {
        let Some(attachment) = session.update(cx, |session, cx| session.detach_lsp(cx)) else {
            return;
        };
        let language = attachment.language;
        let closed = self
            .lsp
            .runtimes
            .get_mut(&language)
            .is_some_and(|runtime| runtime.open.remove(&attachment.uri).is_some());
        if closed {
            attachment.handle.close(attachment.uri);
        }
        let failed = self
            .lsp
            .runtimes
            .get(&language)
            .is_some_and(|runtime| runtime.open.is_empty() && runtime.failure_seen);
        if failed {
            // A dead server with no documents is replaced by the next
            // attachment's own start, which keeps the retry per attachment.
            self.release_lsp_language(language, cx);
        }
    }

    /// Start the language's server; one in-flight start per language.
    fn start_lsp_runtime(&mut self, target: &LspTarget, root: PathBuf, cx: &mut Context<Self>) {
        let language = target.language;
        let document = target.path.clone();
        let generation = self.generation;
        target.session.update(cx, |session, cx| {
            session.note_lsp_status(LspStatus::Starting, cx)
        });
        let starter = self.lsp_starter(root, language);
        let task = cx.spawn(async move |this, cx| {
            let started = runtime::spawn(cx, starter).await.unwrap_or_else(Err);
            let _ = this.update(cx, |project, cx| {
                project.finish_lsp_start(language, document, generation, started, cx)
            });
        });
        self.lsp.starts.insert(language, task);
    }

    /// The discovery/start future, or the test seam that replaces it.
    fn lsp_starter(
        &self,
        root: PathBuf,
        language: LanguageId,
    ) -> Pin<Box<dyn Future<Output = Result<LspClient, String>> + Send>> {
        #[cfg(test)]
        if let Some(hook) = self.lsp_start_hook.clone() {
            return hook(root, language);
        }
        Box::pin(async move {
            let spec =
                astrolith_lsp::discover(&root, language).map_err(|error| error.to_string())?;
            LspClient::spawn_process(spec.into())
                .await
                .map_err(|error| error.to_string())
        })
    }

    /// Adopt a freshly started client, or surface the start failure once.
    fn finish_lsp_start(
        &mut self,
        language: LanguageId,
        document: PathBuf,
        generation: u64,
        result: Result<LspClient, String>,
        cx: &mut Context<Self>,
    ) {
        self.lsp.starts.remove(&language);
        if generation != self.generation {
            return;
        }
        match result {
            Ok(mut client) => {
                let handle = client.handle();
                let mut events = client.take_events();
                self.lsp.runtimes.insert(
                    language,
                    LspRuntime {
                        client,
                        handle,
                        support: ProviderSupport::default(),
                        ready: false,
                        open: BTreeMap::new(),
                        generation,
                        failure_seen: false,
                    },
                );
                self.lsp.unavailable.remove(&language);
                let task = cx.spawn(async move |this, cx| {
                    while let Some(event) = events.recv().await {
                        let routed = this.update(cx, |project, cx| {
                            project.route_lsp_event(language, event, cx)
                        });
                        if routed.is_err() {
                            break;
                        }
                    }
                });
                self.lsp.events.insert(language, task);
            }
            Err(error) => {
                self.lsp.unavailable.insert(language, document.clone());
                tracing::info!(
                    language = language.as_str(),
                    document = %document.display(),
                    %error,
                    "language server unavailable for this document"
                );
                if let Some(session) = self.sessions.get(&document).cloned() {
                    session.update(cx, |session, cx| {
                        session.report_lsp_failure(LspStatus::Failed(error), false, cx)
                    });
                }
            }
        }
        self.sync_lsp(cx);
    }

    /// Route one event from the language's server to the document it names.
    fn route_lsp_event(&mut self, language: LanguageId, event: LspEvent, cx: &mut Context<Self>) {
        match event {
            LspEvent::Diagnostics {
                uri,
                version,
                diagnostics,
            } => {
                let path = self
                    .lsp
                    .runtimes
                    .get(&language)
                    .and_then(|runtime| runtime.open.get(&uri).cloned());
                let Some(path) = path else {
                    tracing::debug!(
                        document = %uri.as_str(),
                        "dropping diagnostics for a document this project does not show"
                    );
                    return;
                };
                let Some(session) = self.sessions.get(&path).cloned() else {
                    return;
                };
                session.update(cx, |session, cx| {
                    session.apply_lsp_diagnostics(uri, version, diagnostics, cx)
                });
            }
            LspEvent::Ready(capabilities) => {
                let support = ProviderSupport::from_capabilities(&capabilities);
                let open = {
                    let Some(runtime) = self.lsp.runtimes.get_mut(&language) else {
                        return;
                    };
                    runtime.support = support;
                    runtime.ready = true;
                    runtime
                        .open
                        .iter()
                        .map(|(uri, path)| (uri.clone(), path.clone()))
                        .collect::<Vec<_>>()
                };
                for (uri, path) in open {
                    let Some(session) = self.sessions.get(&path).cloned() else {
                        continue;
                    };
                    session.update(cx, |session, cx| {
                        if session
                            .lsp_attachment()
                            .is_some_and(|attachment| attachment.uri.as_str() == uri.as_str())
                        {
                            session.lsp_ready(support, cx);
                        }
                    });
                }
            }
            LspEvent::Log { level, message } => match level {
                astrolith_lsp::client::LogLevel::Error => {
                    tracing::error!(%message, "language server")
                }
                astrolith_lsp::client::LogLevel::Warning => {
                    tracing::warn!(%message, "language server")
                }
                _ => tracing::info!(%message, "language server"),
            },
            LspEvent::Failed(error) => {
                tracing::warn!(%error, language = language.as_str(), "language server failed");
                self.lsp_failed(language, LspStatus::Failed(error), cx);
            }
            LspEvent::Exited(code) => {
                tracing::info!(
                    ?code,
                    language = language.as_str(),
                    "language server exited"
                );
                self.lsp_failed(language, LspStatus::Exited(code), cx);
            }
        }
    }

    /// Surface a server failure and arm the one restart that run earned.
    fn lsp_failed(&mut self, language: LanguageId, status: LspStatus, cx: &mut Context<Self>) {
        let target = self.lsp_target(cx);
        let wants_server = target
            .as_ref()
            .is_some_and(|target| target.language == language);
        let (retry, documents) = {
            let Some(runtime) = self.lsp.runtimes.get_mut(&language) else {
                return;
            };
            if runtime.failure_seen {
                // `Failed` is followed by `Exited` for the same dead server; the
                // first event is the cause and the one that owns the retry.
                return;
            }
            runtime.failure_seen = true;
            runtime.ready = false;
            (
                wants_server,
                runtime.open.values().cloned().collect::<Vec<_>>(),
            )
        };
        // One restart per Build attachment of one document, even though the
        // replacement server is a new runtime.
        let retry = retry
            && target
                .as_ref()
                .is_some_and(|target| self.lsp.retried.get(&language) != Some(&target.path));
        if retry && let Some(target) = target.as_ref() {
            self.lsp.retried.insert(language, target.path.clone());
        }
        for path in documents {
            let Some(session) = self.sessions.get(&path).cloned() else {
                continue;
            };
            session.update(cx, |session, cx| {
                session.report_lsp_failure(status.clone(), retry, cx)
            });
        }
        if retry {
            self.arm_lsp_restart(language, cx);
        }
    }

    /// Replace a failed server once, after the existing short delay.
    fn arm_lsp_restart(&mut self, language: LanguageId, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LSP_RESTART_DELAY_MS))
                .await;
            let _ = this.update(cx, |project, cx| {
                project.lsp.restarts.remove(&language);
                project.release_lsp_language(language, cx);
                project.detach_lsp_language_sessions(language, cx);
                project.sync_lsp(cx);
                cx.notify();
            });
        });
        self.lsp.restarts.insert(language, task);
    }

    /// Drop every session attachment to a language whose server is gone.
    fn detach_lsp_language_sessions(&mut self, language: LanguageId, cx: &mut Context<Self>) {
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .read(cx)
                    .lsp_attachment()
                    .is_some_and(|attachment| attachment.language == language)
            })
            .map(|(path, session)| (path.clone(), session.clone()))
            .collect();
        for (_, session) in sessions {
            session.update(cx, |session, cx| session.detach_lsp(cx));
        }
    }

    /// Give one language's server a bounded, graceful exit.
    fn release_lsp_language(&mut self, language: LanguageId, cx: &mut Context<Self>) {
        self.lsp.starts.remove(&language);
        self.lsp.unavailable.remove(&language);
        self.lsp.events.remove(&language);
        if let Some(runtime) = self.lsp.runtimes.remove(&language) {
            release_after_shutdown(
                &runtime.handle,
                runtime.client,
                cx.background_executor().clone(),
                SHUTDOWN_GRACE,
            )
            .detach();
        }
    }

    /// Stop every project server once, gracefully, and clear the registry.
    ///
    /// Project switch and quit both land here; the second call is a no-op, so
    /// the bounded grace path runs exactly once per process.
    pub(crate) fn shutdown_lsp(&mut self, cx: &mut Context<Self>) {
        self.lsp.starts.clear();
        self.lsp.restarts.clear();
        self.lsp.events.clear();
        self.lsp.unavailable.clear();
        self.lsp.retried.clear();
        let runtimes = std::mem::take(&mut self.lsp.runtimes);
        let languages: HashSet<LanguageId> = runtimes.keys().copied().collect();
        for language in languages {
            self.detach_lsp_language_sessions(language, cx);
        }
        for (_, runtime) in runtimes {
            release_after_shutdown(
                &runtime.handle,
                runtime.client,
                cx.background_executor().clone(),
                SHUTDOWN_GRACE,
            )
            .detach();
        }
    }
}

#[cfg(test)]
impl ProjectWorkspace {
    /// Servers the registry currently owns.
    pub(super) fn lsp_runtime_count(&self) -> usize {
        self.lsp.runtimes.len()
    }

    /// Documents currently open on every server.
    pub(super) fn lsp_open_documents(&self) -> Vec<lsp_types::Uri> {
        self.lsp
            .runtimes
            .values()
            .flat_map(|runtime| runtime.open.keys().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, mount, open_project_and_wait, wait_for_session};
    use super::*;
    use crate::editor::session::LspStatus;
    use gpui_kit::base::Root;
    use gpui_kit::{TestAppContext, WindowHandle};
    use serde_json::{Value, json};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::AsyncWriteExt;

    /// A scripted in-memory language server.
    ///
    /// It answers `initialize` and `shutdown` and records every frame the
    /// client sends, so didOpen/didClose ordering and the spawn count are
    /// observable without a real server on `PATH`.
    #[derive(Default)]
    struct FakeServer {
        frames: Arc<Mutex<Vec<Value>>>,
        spawns: Arc<AtomicUsize>,
        writers: Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<Value>>>>,
    }

    impl FakeServer {
        /// Publish from the server side, as a real server would.
        fn publish(&self, uri: &str, version: i32, message: &str) {
            let writer = self
                .writers
                .lock()
                .expect("the writer list")
                .last()
                .expect("a connected server")
                .clone();
            let _ = writer.send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": uri,
                    "version": version,
                    "diagnostics": [{
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 1},
                        },
                        "severity": 1,
                        "message": message,
                    }],
                },
            }));
        }

        fn hook(self: &Arc<Self>) -> Arc<LspStartHook> {
            let fake = self.clone();
            Arc::new(move |root, _language| {
                let fake = fake.clone();
                Box::pin(async move {
                    fake.spawns.fetch_add(1, Ordering::SeqCst);
                    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
                    let (outgoing, incoming) = tokio::sync::mpsc::unbounded_channel();
                    fake.writers
                        .lock()
                        .expect("the writer list")
                        .push(outgoing.clone());
                    tokio::spawn(serve(
                        server_stream,
                        fake.frames.clone(),
                        incoming,
                        outgoing,
                    ));
                    let root_uri =
                        astrolith_lsp::uri_from_path(&root).map_err(|error| error.to_string())?;
                    astrolith_lsp::Client::connect(client_stream, root_uri)
                        .await
                        .map_err(|error| error.to_string())
                })
            })
        }

        fn methods(&self, method: &str) -> Vec<Value> {
            self.frames
                .lock()
                .expect("the recorded frames")
                .iter()
                .filter(|frame| frame.get("method").and_then(Value::as_str) == Some(method))
                .cloned()
                .collect()
        }

        fn documents(&self, method: &str) -> Vec<String> {
            self.methods(method)
                .iter()
                .filter_map(|frame| {
                    frame["params"]["textDocument"]["uri"]
                        .as_str()
                        .map(str::to_owned)
                })
                .collect()
        }
    }

    async fn serve(
        stream: tokio::io::DuplexStream,
        frames: Arc<Mutex<Vec<Value>>>,
        mut incoming: tokio::sync::mpsc::UnboundedReceiver<Value>,
        outgoing: tokio::sync::mpsc::UnboundedSender<Value>,
    ) {
        let (reader, mut writer) = tokio::io::split(stream);
        tokio::spawn(async move {
            while let Some(message) = incoming.recv().await {
                let Ok(bytes) = astrolith_lsp::framing::encode(&message) else {
                    break;
                };
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        });
        let mut frames_in = astrolith_lsp::framing::FrameReader::new(reader);
        while let Ok(Some(message)) = frames_in.read_message().await {
            let reply = match message.get("method").and_then(Value::as_str) {
                Some("initialize") => json!({
                    "jsonrpc": "2.0",
                    "id": message.get("id").cloned().unwrap_or(Value::Null),
                    "result": {"capabilities": {
                        "completionProvider": {},
                        "hoverProvider": true,
                        "definitionProvider": true,
                    }},
                }),
                Some("shutdown") => json!({
                    "jsonrpc": "2.0",
                    "id": message.get("id").cloned().unwrap_or(Value::Null),
                    "result": Value::Null,
                }),
                _ => Value::Null,
            };
            frames.lock().expect("the recorded frames").push(message);
            if !reply.is_null() {
                let _ = outgoing.send(reply);
            }
        }
    }

    fn document_uri(root: &Path, relative: &str) -> String {
        let path = root.join(relative).canonicalize().expect("the file exists");
        astrolith_lsp::uri_from_path(&path)
            .expect("a document URI")
            .as_str()
            .to_owned()
    }

    async fn open_source(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        relative: &str,
    ) -> (PathBuf, Entity<DocumentSession>) {
        let path = view.read_with(cx, |view, _| {
            view.project_root().expect("a project root").join(relative)
        });
        let path = path.canonicalize().expect("the fixture file exists");
        let opened = path.clone();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_project_source(opened, window, cx)
                    .expect("the source opens")
            })
        })
        .expect("the window is open");
        wait_for_session(cx, view, &path).await;
        let session = view.read_with(cx, |view, _| {
            view.sessions
                .get(&path)
                .cloned()
                .expect("the session is retained")
        });
        (path, session)
    }

    fn enter_build(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        source: bool,
    ) {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.set_source_view(source, window, cx))
        })
        .expect("the window is open");
    }

    fn activate(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        path: &Path,
    ) {
        let path = path.to_owned();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&path, window, cx))
        })
        .expect("the window is open");
    }

    async fn wait_for(cx: &mut TestAppContext, mut ready: impl FnMut() -> bool, what: &str) {
        for _ in 0..600 {
            cx.run_until_parked();
            if ready() {
                return;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(5))
            })
            .await
            .expect("the test sleeps on the blocking lane");
        }
        panic!("timed out waiting for {what}");
    }

    /// Wait until one session's editor shows exactly `expected` as its first
    /// diagnostic message, or report what it shows instead.
    async fn wait_for_diagnostics(
        cx: &mut TestAppContext,
        session: &Entity<DocumentSession>,
        expected: Option<&str>,
        what: &str,
    ) {
        for _ in 0..600 {
            cx.run_until_parked();
            if diagnostics_of(cx, session).as_deref() == expected {
                return;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(5))
            })
            .await
            .expect("the test sleeps on the blocking lane");
        }
        panic!(
            "timed out waiting for {what}: {:?}",
            diagnostics_of(cx, session)
        );
    }

    async fn wait_for_status(
        cx: &mut TestAppContext,
        session: &Entity<DocumentSession>,
        expected: LspStatus,
        what: &str,
    ) {
        for _ in 0..600 {
            cx.run_until_parked();
            if session_status(cx, session) == expected {
                return;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(5))
            })
            .await
            .expect("the test sleeps on the blocking lane");
        }
        panic!(
            "timed out waiting for {what}: {:?}",
            session_status(cx, session)
        );
    }

    /// Wait until one session shows a failure, without pinning the wording.
    async fn wait_for_failure(
        cx: &mut TestAppContext,
        session: &Entity<DocumentSession>,
        what: &str,
    ) {
        for _ in 0..600 {
            cx.run_until_parked();
            if matches!(session_status(cx, session), LspStatus::Failed(_)) {
                return;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(5))
            })
            .await
            .expect("the test sleeps on the blocking lane");
        }
        panic!(
            "timed out waiting for {what}: {:?}",
            session_status(cx, session)
        );
    }

    fn diagnostics_of(cx: &TestAppContext, session: &Entity<DocumentSession>) -> Option<String> {
        session.read_with(cx, |session, cx| {
            session
                .editor
                .read(cx)
                .diagnostics()
                .and_then(|set| set.iter().next().map(|entry| entry.message.to_string()))
        })
    }

    #[test]
    fn an_empty_registry_holds_no_server() {
        let registry = LspRegistry::default();
        assert!(registry.runtimes.is_empty());
        assert!(registry.starts.is_empty());
        assert!(registry.events.is_empty());
        assert!(registry.restarts.is_empty());
        assert!(registry.unavailable.is_empty());
        assert!(registry.retried.is_empty());
    }

    #[gpui_kit::test]
    async fn two_build_documents_share_one_server_process(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/one.ts"), "export const one = 1;\n").unwrap();
        std::fs::write(dir.path().join("src/two.ts"), "export const two = 2;\n").unwrap();
        let one_uri = document_uri(dir.path(), "src/one.ts");
        let two_uri = document_uri(dir.path(), "src/two.ts");

        let (handle, view) = mount(cx);
        let fake = Arc::new(FakeServer::default());
        let hook = fake.hook();
        view.update(cx, |view, _| view.lsp_start_hook = Some(hook));
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let (one, one_session) = open_source(cx, handle, &view, "src/one.ts").await;
        enter_build(cx, handle, &view, true);
        wait_for(
            cx,
            || {
                fake.spawns.load(Ordering::SeqCst) == 1
                    && fake.documents("textDocument/didOpen") == vec![one_uri.clone()]
            },
            "the first document to open on one server",
        )
        .await;
        assert_eq!(
            view.read_with(cx, |view, _| view.lsp_runtime_count()),
            1,
            "Build starts exactly one server"
        );
        assert_eq!(
            fake.writers.lock().expect("the writer list").len(),
            1,
            "one connection, not one per document"
        );

        // A second document of the same language joins the same process.
        let (_two, two_session) = open_source(cx, handle, &view, "src/two.ts").await;
        wait_for(
            cx,
            || {
                fake.documents("textDocument/didOpen") == vec![one_uri.clone(), two_uri.clone()]
                    && fake.documents("textDocument/didClose") == vec![one_uri.clone()]
            },
            "the tab switch to close one document and open the other",
        )
        .await;
        assert_eq!(
            fake.spawns.load(Ordering::SeqCst),
            1,
            "a tab switch must not start another process"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.lsp_runtime_count()),
            1,
            "one server serves both documents"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.lsp_open_documents()),
            vec![two_uri.parse().expect("a valid URI")],
            "only the active document is open on the server"
        );

        // Returning to the first tab reuses the very same connection.
        activate(cx, handle, &view, &one);
        wait_for(
            cx,
            || {
                fake.documents("textDocument/didClose") == vec![one_uri.clone(), two_uri.clone()]
                    && fake.documents("textDocument/didOpen")
                        == vec![one_uri.clone(), two_uri.clone(), one_uri.clone()]
            },
            "the switch back to the first document",
        )
        .await;
        assert_eq!(
            fake.spawns.load(Ordering::SeqCst),
            1,
            "returning to a tab reuses the process"
        );
        assert_eq!(
            fake.writers.lock().expect("the writer list").len(),
            1,
            "the same client served all three switches"
        );
        assert_eq!(
            session_status(cx, &one_session),
            LspStatus::Ready,
            "the session shows the shared server's state"
        );
        assert_eq!(
            session_status(cx, &two_session),
            LspStatus::Inactive,
            "the inactive tab is detached, not served by a second process"
        );
    }

    fn session_status(cx: &TestAppContext, session: &Entity<DocumentSession>) -> LspStatus {
        session.read_with(cx, |session, _| session.lsp_status().clone())
    }

    #[gpui_kit::test]
    async fn diagnostics_route_by_document_uri(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/one.ts"), "export const one = 1;\n").unwrap();
        std::fs::write(dir.path().join("src/two.ts"), "export const two = 2;\n").unwrap();
        let one_uri = document_uri(dir.path(), "src/one.ts");
        let two_uri = document_uri(dir.path(), "src/two.ts");

        let (handle, view) = mount(cx);
        let fake = Arc::new(FakeServer::default());
        let hook = fake.hook();
        view.update(cx, |view, _| view.lsp_start_hook = Some(hook));
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let (one, one_session) = open_source(cx, handle, &view, "src/one.ts").await;
        let (two, two_session) = open_source(cx, handle, &view, "src/two.ts").await;
        activate(cx, handle, &view, &one);
        enter_build(cx, handle, &view, true);
        wait_for(
            cx,
            || fake.documents("textDocument/didOpen") == vec![one_uri.clone()],
            "the first document to open",
        )
        .await;

        // A publish for the other document never paints the active editor.
        fake.publish(&one_uri, 1, "one");
        wait_for_diagnostics(
            cx,
            &one_session,
            Some("one"),
            "the active document's diagnostics",
        )
        .await;
        fake.publish(&two_uri, 1, "two-while-closed");
        fake.publish(&one_uri, 2, "one-again");
        wait_for_diagnostics(
            cx,
            &one_session,
            Some("one-again"),
            "the ordered follow-up publish",
        )
        .await;
        assert!(
            diagnostics_of(cx, &two_session).is_none(),
            "a publish for a closed document must not attach to another session"
        );

        // The other tab receives its own publishes once it is the open document.
        activate(cx, handle, &view, &two);
        wait_for(
            cx,
            || fake.documents("textDocument/didOpen").len() == 2,
            "the second document to open",
        )
        .await;
        fake.publish(&one_uri, 3, "one-while-closed");
        fake.publish(&two_uri, 1, "two");
        wait_for_diagnostics(
            cx,
            &two_session,
            Some("two"),
            "the second document's diagnostics",
        )
        .await;
        fake.publish(&two_uri, 2, "two-again");
        wait_for_diagnostics(
            cx,
            &two_session,
            Some("two-again"),
            "the ordered follow-up publish for the second document",
        )
        .await;
        assert!(
            diagnostics_of(cx, &one_session).is_none(),
            "the detached document keeps no diagnostics from another file"
        );
    }

    #[gpui_kit::test]
    async fn leaving_build_detaches_the_document_but_keeps_the_process(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/one.ts"), "export const one = 1;\n").unwrap();
        let one_uri = document_uri(dir.path(), "src/one.ts");

        let (handle, view) = mount(cx);
        let fake = Arc::new(FakeServer::default());
        let hook = fake.hook();
        view.update(cx, |view, _| view.lsp_start_hook = Some(hook));
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let (_one, one_session) = open_source(cx, handle, &view, "src/one.ts").await;
        enter_build(cx, handle, &view, true);
        wait_for(
            cx,
            || fake.documents("textDocument/didOpen") == vec![one_uri.clone()],
            "the document to open",
        )
        .await;

        enter_build(cx, handle, &view, false);
        wait_for(
            cx,
            || fake.documents("textDocument/didClose") == vec![one_uri.clone()],
            "the document to close",
        )
        .await;
        assert_eq!(
            fake.spawns.load(Ordering::SeqCst),
            1,
            "leaving Build never restarts the server"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.lsp_runtime_count()),
            1,
            "the process stays up for the next Build entry"
        );
        assert!(view.read_with(cx, |view, _| view.lsp_open_documents().is_empty()));
        assert!(
            one_session.read_with(cx, |session, _| session.lsp_attachment().is_none()),
            "the session drops its attachment"
        );
        assert_eq!(session_status(cx, &one_session), LspStatus::Inactive);

        // Re-entering Build reuses the process and reopens the document.
        enter_build(cx, handle, &view, true);
        wait_for(
            cx,
            || fake.documents("textDocument/didOpen") == vec![one_uri.clone(), one_uri.clone()],
            "the document to open again",
        )
        .await;
        assert_eq!(fake.spawns.load(Ordering::SeqCst), 1);
    }

    #[gpui_kit::test]
    async fn shutting_down_sends_one_graceful_shutdown(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/one.ts"), "export const one = 1;\n").unwrap();
        let one_uri = document_uri(dir.path(), "src/one.ts");

        let (handle, view) = mount(cx);
        let fake = Arc::new(FakeServer::default());
        let hook = fake.hook();
        view.update(cx, |view, _| view.lsp_start_hook = Some(hook));
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let (_one, one_session) = open_source(cx, handle, &view, "src/one.ts").await;
        enter_build(cx, handle, &view, true);
        wait_for(
            cx,
            || fake.documents("textDocument/didOpen") == vec![one_uri.clone()],
            "the document to open",
        )
        .await;

        view.update(cx, |view, cx| view.shutdown_lsp(cx));
        view.update(cx, |view, cx| view.shutdown_lsp(cx));
        wait_for(
            cx,
            || !fake.methods("shutdown").is_empty(),
            "the graceful shutdown request",
        )
        .await;
        assert_eq!(
            fake.methods("shutdown").len(),
            1,
            "the bounded grace path runs exactly once"
        );
        assert_eq!(view.read_with(cx, |view, _| view.lsp_runtime_count()), 0);
        assert!(
            one_session.read_with(cx, |session, _| session.lsp_attachment().is_none()),
            "shutdown detaches the document first"
        );
        wait_for(
            cx,
            || !fake.methods("exit").is_empty(),
            "the exit notification the client sends after shutdown",
        )
        .await;
    }

    #[gpui_kit::test]
    async fn a_project_switch_stops_the_outgoing_server(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/one.ts"), "export const one = 1;\n").unwrap();
        let one_uri = document_uri(dir.path(), "src/one.ts");
        let next = fixture();

        let (handle, view) = mount(cx);
        let fake = Arc::new(FakeServer::default());
        let hook = fake.hook();
        view.update(cx, |view, _| view.lsp_start_hook = Some(hook));
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        open_source(cx, handle, &view, "src/one.ts").await;
        enter_build(cx, handle, &view, true);
        wait_for(
            cx,
            || fake.documents("textDocument/didOpen") == vec![one_uri.clone()],
            "the document to open",
        )
        .await;

        let root = next.path().to_owned();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(root, window, cx))
        })
        .expect("the window is open");
        wait_for(
            cx,
            || !fake.methods("shutdown").is_empty(),
            "the outgoing project's shutdown",
        )
        .await;
        cx.condition(&view, |view, _| !view.is_loading()).await;
        assert_eq!(
            fake.methods("shutdown").len(),
            1,
            "a project switch shuts the outgoing server down once"
        );
        assert_eq!(view.read_with(cx, |view, _| view.lsp_runtime_count()), 0);
    }

    #[gpui_kit::test]
    async fn a_failed_start_is_attempted_once_per_attachment(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/one.ts"), "export const one = 1;\n").unwrap();
        std::fs::write(dir.path().join("src/two.ts"), "export const two = 2;\n").unwrap();

        let (handle, view) = mount(cx);
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        let hook: Arc<LspStartHook> = Arc::new(move |_root, _language| {
            counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Err("no server installed".to_owned()) })
        });
        view.update(cx, |view, _| view.lsp_start_hook = Some(hook));
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let (_one, one_session) = open_source(cx, handle, &view, "src/one.ts").await;
        enter_build(cx, handle, &view, true);
        wait_for(
            cx,
            || attempts.load(Ordering::SeqCst) == 1,
            "the first start attempt",
        )
        .await;
        wait_for_status(
            cx,
            &one_session,
            LspStatus::Failed("no server installed".into()),
            "the failed start to surface",
        )
        .await;
        assert_eq!(
            session_status(cx, &one_session),
            LspStatus::Failed("no server installed".into())
        );
        // Session notices are surfaced into the workspace status when the
        // session emits, so the user sees one copy of the failure.
        assert_eq!(
            view.read_with(cx, |view, _| view.status.clone()),
            "Language server failed: no server installed. Build continues without language support."
        );
        assert!(
            one_session.read_with(cx, |session, _| session.notice.is_none()),
            "the workspace consumed the session notice instead of duplicating it"
        );

        // Repeated synchronization for the same Build attachment never retries.
        enter_build(cx, handle, &view, true);
        view.update(cx, |view, cx| view.sync_lsp(cx));
        cx.run_until_parked();
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "one attempt per attachment"
        );

        // A different document is a different attachment: one more attempt.
        let (_two, two_session) = open_source(cx, handle, &view, "src/two.ts").await;
        wait_for(
            cx,
            || attempts.load(Ordering::SeqCst) == 2,
            "the second document's own attempt",
        )
        .await;
        wait_for_status(
            cx,
            &two_session,
            LspStatus::Failed("no server installed".into()),
            "the second document's failure",
        )
        .await;
        assert_eq!(view.read_with(cx, |view, _| view.lsp_runtime_count()), 0);
    }

    #[gpui_kit::test]
    async fn a_failed_server_is_replaced_once_per_attachment(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/one.ts"), "export const one = 1;\n").unwrap();

        let (handle, view) = mount(cx);
        let spawns = Arc::new(AtomicUsize::new(0));
        let counter = spawns.clone();
        // A server that dies before answering the handshake.
        let hook: Arc<LspStartHook> = Arc::new(move |root, _language| {
            counter.fetch_add(1, Ordering::SeqCst);
            let root = root.to_owned();
            Box::pin(async move {
                let root_uri =
                    astrolith_lsp::uri_from_path(&root).map_err(|error| error.to_string())?;
                let (client_stream, server_stream) = tokio::io::duplex(1024);
                drop(server_stream);
                astrolith_lsp::Client::connect(client_stream, root_uri)
                    .await
                    .map_err(|error| error.to_string())
            })
        });
        view.update(cx, |view, _| view.lsp_start_hook = Some(hook));
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let (_one, one_session) = open_source(cx, handle, &view, "src/one.ts").await;
        enter_build(cx, handle, &view, true);
        wait_for(cx, || spawns.load(Ordering::SeqCst) == 1, "the first start").await;
        wait_for_failure(cx, &one_session, "the first failure").await;
        let notice = view.read_with(cx, |view, _| view.status.clone());
        assert!(
            notice.starts_with("Language server failed: ") && notice.ends_with(" Retrying once."),
            "the first failure surfaces the retry: {notice}"
        );

        // The single restart fires on the existing short delay.
        for _ in 0..600 {
            cx.executor().advance_clock(Duration::from_millis(600));
            cx.run_until_parked();
            if spawns.load(Ordering::SeqCst) == 2 {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(5))
            })
            .await
            .expect("the test sleeps on the blocking lane");
        }
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            2,
            "a failed server is replaced once"
        );
        wait_for_failure(cx, &one_session, "the replacement's failure").await;
        for _ in 0..600 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.status
                    .contains("Build continues without language support.")
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(5))
            })
            .await
            .expect("the test sleeps on the blocking lane");
        }
        assert!(
            view.read_with(cx, |view, _| view
                .status
                .contains("Build continues without language support.")),
            "the spent retry is reported once: {}",
            view.read_with(cx, |view, _| view.status.clone())
        );

        // A replacement that also fails does not start a third server.
        for _ in 0..20 {
            cx.executor().advance_clock(Duration::from_millis(600));
            cx.run_until_parked();
        }
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            2,
            "one restart per attachment, never a restart loop"
        );
    }
}
