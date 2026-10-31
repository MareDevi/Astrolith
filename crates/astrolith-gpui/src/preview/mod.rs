//! Preview panel: Astro dev-server lifecycle and external browser presentation.
//!
//! The panel owns no process state of its own. It talks to a [`PreviewBackend`]
//! through boxed futures so the real Astro CLI can run on the application Tokio
//! runtime while headless tests inject a fake. Starting, stopping and opening a
//! browser never block the UI thread: the server work is spawned with
//! [`crate::runtime::spawn`], and the system browser is only spawned (see
//! [`astrolith_preview::surface`]).

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use astrolith_core::{EntryId, ProjectId};
use astrolith_preview::{
    DevServer, DevServerConfig, ExternalBrowser, PreviewSession, PreviewSurface, RouteMapping,
    ServerStatus,
};
use gpui_kit::component::{ActiveTheme, StyledExt, button::Button};
use gpui_kit::{
    App, Context, FocusHandle, Focusable, IntoElement, Render, Task, Window, div, prelude::*,
};
use url::Url;

use crate::runtime;

/// A future returned by a preview backend operation.
pub(crate) type PreviewFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// The dev-server operations the panel needs, abstracted for headless tests.
pub(crate) trait PreviewBackend: Send + Sync + 'static {
    /// Start the dev server and resolve its base URL once it is ready.
    fn start(&self) -> PreviewFuture<Result<Url, String>>;
    /// Stop the dev server gracefully.
    fn stop(&self) -> PreviewFuture<Result<(), String>>;
    /// Poll once for a natural exit of the ready server (non-blocking).
    fn poll_exit(&self) -> PreviewFuture<Result<Option<ServerStatus>, String>>;
    /// Kill the server immediately (application exit).
    fn abort(&self);
}

/// The real backend: one project-local Astro dev server on the service runtime.
pub(crate) struct AstroPreviewBackend {
    server: Arc<tokio::sync::Mutex<DevServer>>,
}

impl AstroPreviewBackend {
    /// Build a backend for the project at `root`.
    pub(crate) fn new(project: ProjectId, root: PathBuf) -> Self {
        tracing::debug!(
            project = %project.as_str(),
            root = %root.display(),
            "creating the Astro preview backend"
        );
        Self {
            server: Arc::new(tokio::sync::Mutex::new(DevServer::new(
                project,
                DevServerConfig::new(root),
            ))),
        }
    }
}

impl PreviewBackend for AstroPreviewBackend {
    fn start(&self) -> PreviewFuture<Result<Url, String>> {
        let server = self.server.clone();
        Box::pin(async move {
            tracing::info!("starting the Astro preview server");
            let mut server = server.lock().await;
            server.start().await.map_err(|error| error.to_string())
        })
    }

    fn stop(&self) -> PreviewFuture<Result<(), String>> {
        let server = self.server.clone();
        Box::pin(async move {
            tracing::info!("stopping the Astro preview server");
            let mut server = server.lock().await;
            server.stop().await.map_err(|error| error.to_string())
        })
    }

    fn poll_exit(&self) -> PreviewFuture<Result<Option<ServerStatus>, String>> {
        let server = self.server.clone();
        Box::pin(async move {
            let mut server = server.lock().await;
            server.poll_exit().map_err(|error| error.to_string())
        })
    }

    fn abort(&self) {
        if let Ok(mut server) = self.server.try_lock() {
            server.abort();
        }
    }
}

/// The Preview panel: status, start/stop, and opening the active entry.
pub(crate) struct PreviewPanel {
    backend: Option<Arc<dyn PreviewBackend>>,
    project: Option<ProjectId>,
    root: Option<PathBuf>,
    mapping: RouteMapping,
    surface: Arc<dyn PreviewSurface>,
    status: ServerStatus,
    message: Option<String>,
    /// Entry the "Open in browser" button resolves, updated by the shell.
    pending_entry: Option<EntryId>,
    task: Option<Task<()>>,
    /// Watches a ready server for an unexpected exit.
    exit_task: Option<Task<()>>,
    /// Invalidates exit watchers when the panel restarts or reattaches.
    generation: u64,
    focus: FocusHandle,
}

impl PreviewPanel {
    /// Create an empty panel backed by the system browser.
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self::with_surface(cx, Arc::new(ExternalBrowser::new()))
    }

    /// Create a panel that presents URLs through `surface` (tests inject a fake).
    pub(crate) fn with_surface(cx: &mut Context<Self>, surface: Arc<dyn PreviewSurface>) -> Self {
        Self {
            backend: None,
            project: None,
            root: None,
            mapping: RouteMapping::new(),
            surface,
            status: ServerStatus::Stopped,
            message: None,
            pending_entry: None,
            task: None,
            exit_task: None,
            generation: 0,
            focus: cx.focus_handle(),
        }
    }

    /// Current server status.
    #[cfg(test)]
    pub(crate) fn status(&self) -> &ServerStatus {
        &self.status
    }

    /// Last user-facing message (errors, opened URL, start hint).
    #[cfg(test)]
    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Set the entry the toolbar button opens (the active document's entry).
    pub(crate) fn set_entry(&mut self, entry: Option<EntryId>, cx: &mut Context<Self>) {
        if self.pending_entry != entry {
            self.pending_entry = entry;
            cx.notify();
        }
    }

    /// Project this panel is attached to, if any.
    #[cfg(test)]
    pub(crate) fn project(&self) -> Option<&ProjectId> {
        self.project.as_ref()
    }

    /// Attach the real Astro backend for `project` at `root`.
    ///
    /// Reattaching to the *same* project only refreshes the route mapping, so a
    /// collection change does not restart a running server.
    pub(crate) fn ensure_attached(
        &mut self,
        project: ProjectId,
        root: PathBuf,
        mapping: RouteMapping,
        cx: &mut Context<Self>,
    ) {
        if self.project.as_ref() == Some(&project) {
            if self.mapping != mapping {
                tracing::debug!(project = %project.as_str(), "refreshed the preview route mapping");
                self.mapping = mapping;
                cx.notify();
            }
            return;
        }
        tracing::info!(
            project = %project.as_str(),
            root = %root.display(),
            "attaching the preview panel to a project"
        );
        let backend = Arc::new(AstroPreviewBackend::new(project.clone(), root.clone()));
        self.attach(backend, project, root, mapping, cx);
    }

    /// Attach an arbitrary backend (the test seam).
    pub(crate) fn attach(
        &mut self,
        backend: Arc<dyn PreviewBackend>,
        project: ProjectId,
        root: PathBuf,
        mapping: RouteMapping,
        cx: &mut Context<Self>,
    ) {
        self.detach(cx);
        self.backend = Some(backend);
        self.project = Some(project);
        self.root = Some(root);
        self.mapping = mapping;
        cx.notify();
    }

    /// Drop the backend, killing any running server, and reset the panel.
    pub(crate) fn detach(&mut self, cx: &mut Context<Self>) {
        if self.project.is_some() {
            tracing::debug!("detaching the preview panel from its project");
        }
        self.invalidate_exit_watch();
        self.task = None;
        if let Some(backend) = self.backend.take() {
            backend.abort();
        }
        self.project = None;
        self.root = None;
        self.mapping = RouteMapping::new();
        self.status = ServerStatus::Stopped;
        self.message = None;
        cx.notify();
    }

    /// Kill the server synchronously for the application-exit path.
    pub(crate) fn abort(&mut self) {
        self.invalidate_exit_watch();
        self.task = None;
        if let Some(backend) = self.backend.take() {
            backend.abort();
        }
        self.status = ServerStatus::Stopped;
    }

    /// Abandon the current exit watcher; its generation no longer matches.
    fn invalidate_exit_watch(&mut self) {
        self.exit_task = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Start watching a ready server for an unexpected exit.
    fn watch_exit(&mut self, backend: Arc<dyn PreviewBackend>, cx: &mut Context<Self>) {
        let generation = self.generation;
        self.exit_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let polled = runtime::spawn(cx, backend.poll_exit()).await;
                let keep_watching = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        match polled {
                            Ok(Ok(Some(status))) => {
                                tracing::info!(status = status.label(), "Astro preview server exited");
                                this.message = Some(match &status {
                                    ServerStatus::Failed(error) => format!(
                                        "The preview server stopped unexpectedly: {error}"
                                    ),
                                    _ => "The preview server stopped.".to_owned(),
                                });
                                this.status = status;
                                cx.notify();
                                false
                            }
                            Ok(Ok(None)) => true,
                            Ok(Err(error)) => {
                                tracing::warn!(%error, "could not observe the Astro preview server");
                                this.message =
                                    Some(format!("Could not observe the preview server: {error}"));
                                cx.notify();
                                false
                            }
                            Err(error) => {
                                tracing::warn!(%error, "preview exit watcher failed");
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !keep_watching {
                    break;
                }
            }
        }));
    }

    /// Start the dev server (no-op while a start/stop task is in flight).
    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            self.message = Some("Open an Astro project to start a preview.".to_owned());
            cx.notify();
            return;
        };
        if self.task.is_some() {
            return;
        }
        if self.status.is_ready() {
            self.message = Some("The preview server is already running.".to_owned());
            cx.notify();
            return;
        }
        self.invalidate_exit_watch();
        self.status = ServerStatus::Starting;
        self.message = Some("Starting the Astro dev server…".to_owned());
        let started = runtime::spawn(cx, backend.start());
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = started.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(url)) => {
                        tracing::info!(url = %url, "Astro preview ready");
                        this.message = Some(format!("Preview ready at {url}"));
                        this.status = ServerStatus::Ready(url);
                        this.watch_exit(backend.clone(), cx);
                    }
                    Ok(Err(error)) | Err(error) => {
                        tracing::error!(%error, "could not start the Astro preview");
                        this.message = Some(format!("Could not start the preview: {error}"));
                        this.status = ServerStatus::Failed(error);
                    }
                }
                this.task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Stop the dev server.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        if self.task.is_some() {
            return;
        }
        self.invalidate_exit_watch();
        self.status = ServerStatus::Stopping;
        self.message = Some("Stopping the Astro dev server…".to_owned());
        let stopped = runtime::spawn(cx, backend.stop());
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = stopped.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(())) => {
                        tracing::info!("Astro preview stopped");
                        this.status = ServerStatus::Stopped;
                        this.message = Some("Preview stopped.".to_owned());
                    }
                    Ok(Err(error)) | Err(error) => {
                        tracing::error!(%error, "could not stop the Astro preview");
                        this.status = ServerStatus::Failed(error.clone());
                        this.message = Some(format!("Could not stop the preview: {error}"));
                    }
                }
                this.task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Open `entry` in the external browser, reusing the running server.
    pub(crate) fn open_entry(&mut self, entry: &EntryId, cx: &mut Context<Self>) {
        let session = PreviewSession::new(self.mapping.clone(), self.surface.clone());
        match session.open_entry(entry, &self.status) {
            Ok(url) => {
                tracing::info!(url = %url, entry = %entry.id(), "opened a preview in the browser");
                self.message = Some(format!("Opened {url} in the browser."));
            }
            Err(error) => {
                tracing::warn!(%error, "could not open a preview route");
                self.message = Some(error.to_string());
            }
        }
        cx.notify();
    }

    /// Open whichever entry the shell last marked active.
    pub(crate) fn open_active(&mut self, cx: &mut Context<Self>) {
        match self.pending_entry.clone() {
            Some(entry) => self.open_entry(&entry, cx),
            None => {
                self.message = Some("Open a document to preview its entry.".to_owned());
                cx.notify();
            }
        }
    }

    fn status_line(&self) -> String {
        match self.status.url() {
            Some(url) => format!("Preview ready at {url}"),
            None => format!("Dev server: {}", self.status.label()),
        }
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        let status_line = self.status_line();
        let message = self.message.clone();
        let start_owner = owner.clone();
        let stop_owner = owner.clone();
        let open_owner = owner.clone();
        div()
            .id("preview-panel")
            .track_focus(&self.focus)
            .v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .p_3()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().id("preview-status").text_sm().child(status_line))
            .when_some(message, |view, message| {
                view.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(message),
                )
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("preview-start")
                            .label("Start preview")
                            .on_click(move |_, _, cx| {
                                let _ = start_owner.update(cx, |this, cx| this.start(cx));
                            }),
                    )
                    .child(Button::new("preview-stop").label("Stop preview").on_click(
                        move |_, _, cx| {
                            let _ = stop_owner.update(cx, |this, cx| this.stop(cx));
                        },
                    ))
                    .child(
                        Button::new("preview-open")
                            .label("Open in browser")
                            .on_click(move |_, _, cx| {
                                let _ = open_owner.update(cx, |this, cx| this.open_active(cx));
                            }),
                    ),
            )
    }
}

impl Focusable for PreviewPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
