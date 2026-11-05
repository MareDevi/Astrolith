//! Editor-facing language-server providers and diagnostic projection.
//!
//! The protocol client lives in `astrolith-lsp`; this module adapts it to the
//! pinned editor's provider traits. Every provider awaits the server on the
//! background executor, so rendering and typing stay independent of it.

use std::{
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use astrolith_lsp::{
    Client, ClientHandle, LanguageId, code_action_params, completion_params, convert_positions,
    definition_params, hover_params, methods, uri_from_path, utf16_position_to_char,
};
use gpui_kit::{
    App, AppContext as _, BackgroundExecutor, Entity, SharedString, Task, Window,
    component::highlighter::{Diagnostic, DiagnosticSet},
    component::input::{
        CodeActionProvider, CompletionProvider, DefinitionProvider, EditorState, HoverProvider,
        Rope,
    },
};
use lsp_types::{
    CodeAction, CodeActionOrCommand, CompletionContext, CompletionResponse, GotoDefinitionResponse,
    Hover, LocationLink, WorkspaceEdit,
};

/// One running server bound to one open build-mode document.
pub(crate) struct LspState {
    /// Keeps the process and protocol loop alive for as long as the session.
    pub _client: Client,
    pub handle: ClientHandle,
    pub uri: lsp_types::Uri,
    pub language: LanguageId,
}

impl LspState {
    pub fn open(&self, text: String) {
        self.handle.open(self.uri.clone(), self.language, text);
    }

    pub fn close(&self) {
        self.handle.close(self.uri.clone());
    }
}

/// How long a server that acknowledged `shutdown` gets to exit on its own.
pub(crate) const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// The one handle operation the stop path needs; tests supply a counter.
pub(crate) trait ShutdownSignal {
    fn request_shutdown(&self);
}

impl ShutdownSignal for ClientHandle {
    fn request_shutdown(&self) {
        self.shutdown();
    }
}

/// Ask the server to exit, then drop its client after `grace`.
///
/// The returned task must be detached: the caller never waits for the server,
/// and the client (which kills the child on drop) stays alive long enough for
/// the actor to finish its own shutdown handshake.
pub(crate) fn release_after_shutdown<C>(
    handle: &impl ShutdownSignal,
    client: C,
    executor: BackgroundExecutor,
    grace: Duration,
) -> Task<()>
where
    C: Send + 'static,
{
    handle.request_shutdown();
    let timer = executor.clone();
    executor.spawn(async move {
        timer.timer(grace).await;
        drop(client);
    })
}

/// The providers a server advertised in its `initialize` result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProviderSupport {
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub code_actions: bool,
}

impl ProviderSupport {
    pub(crate) fn from_capabilities(capabilities: &lsp_types::ServerCapabilities) -> Self {
        Self {
            completion: capabilities.completion_provider.is_some(),
            hover: match capabilities.hover_provider {
                Some(lsp_types::HoverProviderCapability::Simple(advertised)) => advertised,
                Some(lsp_types::HoverProviderCapability::Options(_)) => true,
                None => false,
            },
            definition: match &capabilities.definition_provider {
                Some(lsp_types::OneOf::Left(advertised)) => *advertised,
                Some(lsp_types::OneOf::Right(_)) => true,
                None => false,
            },
            code_actions: match &capabilities.code_action_provider {
                Some(lsp_types::CodeActionProviderCapability::Simple(advertised)) => *advertised,
                Some(lsp_types::CodeActionProviderCapability::Options(_)) => true,
                None => false,
            },
        }
    }
}

/// Install the providers the server advertises on `editor`.
pub(crate) fn install(
    editor: &Entity<EditorState>,
    handle: ClientHandle,
    uri: lsp_types::Uri,
    capabilities: &lsp_types::ServerCapabilities,
    cx: &mut App,
) {
    let support = ProviderSupport::from_capabilities(capabilities);
    editor.update(cx, |editor, cx| {
        let provider = Rc::new(LspProvider {
            handle: handle.clone(),
            uri: uri.clone(),
            completions: CompletionGate::default(),
        });
        let actions = Rc::new(CodeActions { handle, uri });
        let lsp = editor.lsp_mut();
        lsp.completion_provider = if support.completion {
            Some(provider.clone())
        } else {
            None
        };
        lsp.hover_provider = if support.hover {
            Some(provider.clone())
        } else {
            None
        };
        lsp.definition_provider = if support.definition {
            Some(provider.clone())
        } else {
            None
        };
        lsp.code_action_providers = if support.code_actions {
            vec![actions]
        } else {
            Vec::new()
        };
        editor.refresh(cx);
    });
}

/// Remove every provider and the diagnostics the server contributed.
pub(crate) fn uninstall(editor: &Entity<EditorState>, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        let lsp = editor.lsp_mut();
        lsp.completion_provider = None;
        lsp.hover_provider = None;
        lsp.definition_provider = None;
        lsp.code_action_providers.clear();
        lsp.show_document = None;
        if let Some(diagnostics) = editor.diagnostics_mut() {
            diagnostics.clear();
        }
        editor.refresh(cx);
    });
}

/// Whether two document URIs name the same server-side document.
pub(crate) fn same_document(document: &lsp_types::Uri, published: &lsp_types::Uri) -> bool {
    document.as_str() == published.as_str()
}

/// Whether one `publishDiagnostics` may replace this document's set.
///
/// A publish for another project file never paints this editor. Versioned
/// publishes only move forward; an unversioned publish can be ordered against
/// nothing, so it is accepted only while no versioned publish has been.
pub(crate) fn accepts_diagnostics(
    document: &lsp_types::Uri,
    last_accepted: Option<i32>,
    published: &lsp_types::Uri,
    version: Option<i32>,
) -> bool {
    if !same_document(document, published) {
        return false;
    }
    match (last_accepted, version) {
        (Some(last), Some(version)) => version >= last,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// Replace the editor's diagnostics with one server publish.
///
/// The server sends UTF-16 positions; the editor counts characters, so every
/// range is converted against the text the editor currently shows (the Build
/// source view) before it is published.
pub(crate) fn apply_diagnostics(
    editor: &Entity<EditorState>,
    diagnostics: &[lsp_types::Diagnostic],
    cx: &mut App,
) {
    let text = editor.read(cx).text().clone();
    let plain = text.to_string();
    let mut set = DiagnosticSet::new(&text);
    for diagnostic in diagnostics {
        let mut converted = diagnostic.clone();
        converted.range.start = utf16_position_to_char(&plain, converted.range.start);
        converted.range.end = utf16_position_to_char(&plain, converted.range.end);
        set.push(Diagnostic::from(converted));
    }
    editor.update(cx, |editor, cx| {
        if let Some(current) = editor.diagnostics_mut() {
            *current = set;
        }
        cx.notify();
    });
}

pub(crate) fn uri_for(path: &std::path::Path) -> Option<lsp_types::Uri> {
    uri_from_path(path).ok()
}

/// Type-ahead completion waits this long so one request serves a burst.
pub(crate) const COMPLETION_DEBOUNCE: Duration = Duration::from_millis(70);

/// One completion provider's request generations.
///
/// Every invocation claims the newest generation; a debounced request only
/// reaches the server when its claim is still current, so a burst of typed
/// characters sends one request for the text the user stopped on.
#[derive(Clone, Default)]
pub(crate) struct CompletionGate {
    generation: Arc<AtomicU64>,
}

impl CompletionGate {
    /// Claim the newest generation; earlier claims are superseded.
    pub(crate) fn claim(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Whether `claim` is still the newest generation.
    pub(crate) fn is_current(&self, claim: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == claim
    }
}

/// Wait out the debounce; `false` means a newer claim superseded this one.
///
/// A zero debounce is the explicit invocation: it never waits and never
/// yields to a claim, because the user asked for this menu directly.
pub(crate) async fn complete_after_debounce(
    executor: &BackgroundExecutor,
    gate: &CompletionGate,
    claim: u64,
    debounce: Duration,
) -> bool {
    if debounce.is_zero() {
        return true;
    }
    executor.timer(debounce).await;
    gate.is_current(claim)
}

struct LspProvider {
    handle: ClientHandle,
    uri: lsp_types::Uri,
    completions: CompletionGate,
}

impl CompletionProvider for LspProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let claim = self.completions.claim();
        let gate = self.completions.clone();
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let text = text.clone();
        let executor = cx.background_executor().clone();
        let trigger_kind = match trigger.trigger_kind {
            lsp_types::CompletionTriggerKind::INVOKED => 1,
            lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER => 2,
            lsp_types::CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS => 3,
            _ => 1,
        };
        // The user-pressed chord is explicit intent; typed characters coalesce.
        let debounce = if trigger_kind == 1 {
            Duration::ZERO
        } else {
            COMPLETION_DEBOUNCE
        };
        let trigger_character = trigger.trigger_character.clone();
        cx.background_spawn(async move {
            if !complete_after_debounce(&executor, &gate, claim, debounce).await {
                return Ok(CompletionResponse::Array(Vec::new()));
            }
            let plain = text.to_string();
            let mut params = completion_params(&uri, &plain, offset, trigger_kind);
            if let Some(character) = trigger_character {
                params["context"]["triggerCharacter"] = serde_json::Value::String(character);
            }
            let mut value = handle
                .request(methods::COMPLETION, params)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            convert_positions(&mut value, &plain);
            serde_json::from_value(value)
                .map_err(|error| anyhow::anyhow!("invalid completion response: {error}"))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        new_text.chars().next_back().is_some_and(|character| {
            character.is_alphanumeric()
                || matches!(
                    character,
                    '_' | '.' | '$' | '@' | '/' | '-' | '"' | '\'' | '<'
                )
        })
    }
}

impl HoverProvider for LspProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<Hover>>> {
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let plain = text.to_string();
        cx.background_spawn(async move {
            let value = handle
                .request(methods::HOVER, hover_params(&uri, &plain, offset))
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if value.is_null() {
                return Ok(None);
            }
            let mut value = value;
            convert_positions(&mut value, &plain);
            serde_json::from_value(value)
                .map(Some)
                .map_err(|error| anyhow::anyhow!("invalid hover response: {error}"))
        })
    }
}

impl DefinitionProvider for LspProvider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<LocationLink>>> {
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let plain = text.to_string();
        cx.background_spawn(async move {
            let mut value = handle
                .request(methods::DEFINITION, definition_params(&uri, &plain, offset))
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            convert_positions(&mut value, &plain);
            let response: GotoDefinitionResponse = serde_json::from_value(value)
                .map_err(|error| anyhow::anyhow!("invalid definition response: {error}"))?;
            Ok(match response {
                GotoDefinitionResponse::Scalar(location) => vec![LocationLink {
                    origin_selection_range: None,
                    target_uri: location.uri,
                    target_range: location.range,
                    target_selection_range: location.range,
                }],
                GotoDefinitionResponse::Array(locations) => locations
                    .into_iter()
                    .map(|location| LocationLink {
                        origin_selection_range: None,
                        target_uri: location.uri,
                        target_range: location.range,
                        target_selection_range: location.range,
                    })
                    .collect(),
                GotoDefinitionResponse::Link(links) => links,
            })
        })
    }
}

struct CodeActions {
    handle: ClientHandle,
    uri: lsp_types::Uri,
}

impl CodeActionProvider for CodeActions {
    fn id(&self) -> SharedString {
        "astrolith.lsp".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<CodeAction>>> {
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let plain = state.read(cx).text().to_string();
        cx.background_spawn(async move {
            let mut value = handle
                .request(
                    methods::CODE_ACTION,
                    code_action_params(&uri, &plain, range),
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            convert_positions(&mut value, &plain);
            let actions: Vec<CodeActionOrCommand> = serde_json::from_value(value)
                .map_err(|error| anyhow::anyhow!("invalid code action response: {error}"))?;
            Ok(actions
                .into_iter()
                .filter_map(|action| match action {
                    CodeActionOrCommand::CodeAction(action) => Some(action),
                    // Commands are server-side work; this client has no
                    // workspace/executeCommand flow, so it does not offer them.
                    CodeActionOrCommand::Command(_) => None,
                })
                .collect())
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        let Some(edit) = action.edit else {
            return Task::ready(Ok(()));
        };
        apply_workspace_edit(&state, &self.uri, edit, window, cx);
        Task::ready(Ok(()))
    }
}

/// Apply the edits that target the open document.
///
/// Cross-file workspace edits are deliberately skipped: opening and editing a
/// different retained session is the workspace's responsibility, not a
/// provider's, and half-applying them would be worse than not offering them.
fn apply_workspace_edit(
    state: &Entity<EditorState>,
    uri: &lsp_types::Uri,
    edit: WorkspaceEdit,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(changes) = edit.changes {
        for (target, edits) in changes {
            if target.as_str() != uri.as_str() {
                tracing::info!(document = %target.as_str(), "skipping a cross-file code action edit");
                continue;
            }
            state.update(cx, |editor, cx| {
                editor.apply_lsp_edits(&edits, window, cx);
            });
        }
    }
    if edit.document_changes.is_some() {
        tracing::info!("skipping a documentChanges code action; only plain edits are applied");
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Instant,
    };

    use super::*;

    fn uri(text: &str) -> lsp_types::Uri {
        text.parse().expect("a valid document URI")
    }

    #[test]
    fn only_the_same_document_is_accepted() {
        let document = uri("file:///project/src/index.astro");
        let other = uri("file:///project/src/other.astro");

        assert!(same_document(
            &document,
            &uri("file:///project/src/index.astro")
        ));
        assert!(!same_document(&document, &other));
        assert!(!accepts_diagnostics(&document, None, &other, Some(1)));
        assert!(
            !accepts_diagnostics(&document, None, &other, None),
            "an unversioned publish for another file is still another document"
        );
    }

    #[test]
    fn an_older_publish_is_dropped() {
        let document = uri("file:///project/src/index.astro");

        assert!(!accepts_diagnostics(&document, Some(3), &document, Some(2)));
        assert!(!accepts_diagnostics(&document, Some(3), &document, Some(1)));
    }

    #[test]
    fn a_newer_or_repeated_publish_is_accepted() {
        let document = uri("file:///project/src/index.astro");

        assert!(accepts_diagnostics(&document, Some(3), &document, Some(4)));
        assert!(accepts_diagnostics(&document, Some(3), &document, Some(3)));
    }

    #[test]
    fn an_unversioned_publish_is_accepted_until_a_versioned_one() {
        let document = uri("file:///project/src/index.astro");

        assert!(accepts_diagnostics(&document, None, &document, None));
        assert!(
            !accepts_diagnostics(&document, Some(1), &document, None),
            "an unversioned publish cannot be ordered after a versioned one"
        );
    }

    #[test]
    fn capabilities_enable_exactly_the_advertised_providers() {
        let capabilities = lsp_types::ServerCapabilities {
            hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
            completion_provider: Some(lsp_types::CompletionOptions::default()),
            definition_provider: Some(lsp_types::OneOf::Left(false)),
            code_action_provider: Some(lsp_types::CodeActionProviderCapability::Simple(false)),
            ..Default::default()
        };

        assert_eq!(
            ProviderSupport::from_capabilities(&capabilities),
            ProviderSupport {
                completion: true,
                hover: true,
                definition: false,
                code_actions: false,
            }
        );
    }

    #[gpui_kit::test]
    fn only_advertised_providers_are_installed_on_the_editor(cx: &mut gpui_kit::TestAppContext) {
        struct Host;
        impl gpui_kit::Render for Host {
            fn render(
                &mut self,
                _: &mut gpui_kit::Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                gpui_kit::div()
            }
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime");
        let (client, _peer) = runtime.block_on(async {
            let (client_stream, peer) = tokio::io::duplex(1024);
            (
                Client::connect(client_stream, uri("file:///project"))
                    .await
                    .expect("an in-memory client"),
                peer,
            )
        });
        let mut editor_slot = None;
        let (window, _host) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                editor_slot = Some(cx.new(|cx| EditorState::new(window, cx)));
                cx.new(|_| Host)
            })
            .expect("a test window")
        });
        let editor = editor_slot.expect("the editor is built with the window");
        let handle = client.handle();
        let capabilities = lsp_types::ServerCapabilities {
            hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
            ..Default::default()
        };
        cx.update_window(window, |_, _, cx| {
            install(
                &editor,
                handle,
                uri("file:///project/file.ts"),
                &capabilities,
                cx,
            );
            let lsp = editor.read(cx).lsp();
            assert!(
                lsp.hover_provider.is_some(),
                "the advertised hover provider is installed"
            );
            assert!(lsp.completion_provider.is_none());
            assert!(lsp.definition_provider.is_none());
            assert!(lsp.code_action_providers.is_empty());
        })
        .expect("the window is open");
    }

    #[gpui_kit::test]
    async fn rapid_typed_completions_send_only_the_newest_request(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let gate = CompletionGate::default();
        let sent = Arc::new(AtomicUsize::new(0));
        let executor = cx.executor();
        let first = gate.claim();
        let second = gate.claim();
        let mut tasks = Vec::new();
        for claim in [first, second] {
            let gate = gate.clone();
            let sent = sent.clone();
            let executor = executor.clone();
            let timer = executor.clone();
            tasks.push(executor.spawn(async move {
                if complete_after_debounce(&timer, &gate, claim, COMPLETION_DEBOUNCE).await {
                    sent.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        cx.executor().advance_clock(COMPLETION_DEBOUNCE * 2);
        cx.run_until_parked();
        assert_eq!(
            sent.load(Ordering::SeqCst),
            1,
            "a keystroke burst must send one completion request"
        );
        drop(tasks);
    }

    #[gpui_kit::test]
    async fn an_explicit_completion_sends_without_waiting(cx: &mut gpui_kit::TestAppContext) {
        let gate = CompletionGate::default();
        let sent = Arc::new(AtomicUsize::new(0));
        let claim = gate.claim();
        let executor = cx.executor();
        let send = {
            let gate = gate.clone();
            let sent = sent.clone();
            let executor = executor.clone();
            let timer = executor.clone();
            executor.spawn(async move {
                if complete_after_debounce(&timer, &gate, claim, Duration::ZERO).await {
                    sent.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        send.await;
        assert_eq!(
            sent.load(Ordering::SeqCst),
            1,
            "an invoked completion must not wait for the debounce"
        );
    }

    struct FakeHandle(Arc<AtomicUsize>);

    impl ShutdownSignal for FakeHandle {
        fn request_shutdown(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakeClient(Arc<AtomicBool>);

    impl Drop for FakeClient {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[gpui_kit::test]
    async fn stopping_requests_shutdown_without_waiting_for_the_server(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let requests = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let task = release_after_shutdown(
            &FakeHandle(requests.clone()),
            FakeClient(dropped.clone()),
            cx.executor(),
            Duration::from_secs(30),
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the caller must not wait for the grace period"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(
            !dropped.load(Ordering::SeqCst),
            "the client must outlive the caller for the grace period"
        );
        drop(task);
        cx.run_until_parked();
        assert!(
            dropped.load(Ordering::SeqCst),
            "dropping the grace task releases the client"
        );
    }

    #[gpui_kit::test]
    async fn the_client_is_dropped_once_the_grace_period_expires(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let requests = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let _task = release_after_shutdown(
            &FakeHandle(requests.clone()),
            FakeClient(dropped.clone()),
            cx.executor(),
            Duration::from_millis(10),
        );
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert!(dropped.load(Ordering::SeqCst));
    }
}
