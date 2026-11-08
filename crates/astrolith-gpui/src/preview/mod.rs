//! Preview panel: Astro dev-server lifecycle and external browser presentation.
//!
//! The panel owns no process state of its own. It talks to a [`PreviewBackend`]
//! through boxed futures so the real Astro CLI can run on the application Tokio
//! runtime while headless tests inject a fake. Starting, stopping and opening a
//! browser never block the UI thread: the server work is spawned with
//! [`crate::runtime::spawn`], and the system browser is only spawned (see
//! [`astrolith_preview::surface`]).

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use astrolith_core::{
    EntryId, ProjectId,
    preferences::{PreviewPreferences, PreviewPresentation},
};
use astrolith_preview::{
    DevServer, DevServerConfig, EmbeddedUnavailable, ExternalBrowser, PreviewLifecycle,
    PreviewSession, PreviewSurface, PreviewTarget, RouteMapping, ServerStatus, StartupFailure,
    TargetChange, target_change,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Task, Window, div,
    prelude::*,
};
use url::Url;

use crate::runtime;

pub(crate) mod embedded;

/// A future returned by a preview backend operation.
pub(crate) type PreviewFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// The dev-server operations the panel needs, abstracted for headless tests.
pub(crate) trait PreviewBackend: Send + Sync + 'static {
    /// Start the dev server and resolve its base URL once it is ready.
    fn start(&self) -> PreviewFuture<Result<Url, StartupFailure>>;
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
    /// Cancels a start that is still waiting for readiness (project switch).
    cancellation: std::sync::Mutex<Option<astrolith_astro::Cancellation>>,
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
            cancellation: std::sync::Mutex::new(None),
        }
    }
}

impl PreviewBackend for AstroPreviewBackend {
    fn start(&self) -> PreviewFuture<Result<Url, StartupFailure>> {
        let server = self.server.clone();
        let cancellation = astrolith_astro::Cancellation::default();
        if let Ok(mut slot) = self.cancellation.lock() {
            *slot = Some(cancellation.clone());
        }
        Box::pin(async move {
            tracing::info!("starting the Astro preview server");
            let mut server = server.lock().await;
            match server.start_cancellable(&cancellation).await {
                Ok(url) => Ok(url),
                Err(error) => Err(server
                    .failure()
                    .cloned()
                    .unwrap_or_else(|| StartupFailure::new(error.to_string(), None))),
            }
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
        if let Ok(mut slot) = self.cancellation.lock()
            && let Some(cancellation) = slot.take()
        {
            cancellation.cancel();
        }
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
    /// System browser, also the fallback when a preferred browser fails.
    surface: Arc<dyn PreviewSurface>,
    /// Configured browser argv, when the preference is not empty.
    preferred_surface: Option<Arc<dyn PreviewSurface>>,
    presentation: PreviewPresentation,
    open_on_enter: bool,
    /// The user is explicitly in Preview mode.
    visiting: bool,
    /// An Open-on-Enter launch is still waiting for the server and route.
    pending_open: bool,
    /// Open-on-Enter already ran for this explicit visit.
    opened_this_visit: bool,
    /// URL whose preferred-browser launch failed; offers the system opener.
    fallback_url: Option<Url>,
    /// Generation-checked dev-server state.
    lifecycle: PreviewLifecycle,
    message: Option<PreviewMessage>,
    /// Entry the "Open in browser" button resolves, updated by the shell.
    pending_entry: Option<EntryId>,
    task: Option<Task<()>>,
    /// Watches a ready server for an unexpected exit.
    exit_task: Option<Task<()>>,
    focus: FocusHandle,
}

/// Emitted whenever the dev-server generation or status changes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreviewStateChanged;

impl EventEmitter<PreviewStateChanged> for PreviewPanel {}

/// The lifecycle phase shown in the panel and used by command availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewPhase {
    /// No server is running.
    Stopped,
    /// A start or restart is in flight.
    Starting,
    /// The server is serving requests.
    Ready,
    /// A stop is in flight.
    Stopping,
    /// The last start or observation failed.
    Failed,
}

impl PreviewPhase {
    fn from_status(status: &ServerStatus) -> Self {
        match status {
            ServerStatus::Stopped => Self::Stopped,
            ServerStatus::Starting => Self::Starting,
            ServerStatus::Ready(_) => Self::Ready,
            ServerStatus::Stopping => Self::Stopping,
            ServerStatus::Failed(_) => Self::Failed,
        }
    }

    /// Localized phase name shown next to the status dot.
    pub(crate) fn text(self) -> String {
        match self {
            Self::Stopped => tr!("preview.phase.stopped"),
            Self::Starting => tr!("preview.phase.starting"),
            Self::Ready => tr!("preview.phase.ready"),
            Self::Stopping => tr!("preview.phase.stopping"),
            Self::Failed => tr!("preview.phase.failed"),
        }
    }
}

/// A message shown under the panel controls.
///
/// The message keeps its parameters instead of a rendered string, so changing
/// the interface language retranslates it on the next frame.
#[derive(Clone, Debug)]
pub(crate) enum PreviewMessage {
    OpenProjectToStart,
    OpenProjectToRestart,
    AlreadyRunning,
    Starting,
    Stopping,
    Restarting,
    Ready { url: String },
    Stopped,
    StoppedUnexpectedly { error: String },
    StoppedOnItsOwn,
    ObserveFailed { error: String },
    StartFailed { error: String },
    StartFailedWithOutput { error: String, output: String },
    StopFailed { error: String },
    UrlUnresolved { error: String },
    OpenedInBrowser { url: String },
    PreferredBrowserFailed { error: String },
    OpenedInSystemBrowser { url: String },
    SystemBrowserFailed { error: String },
    EmbeddedUnavailable { reason: String },
    OpenDocumentToPreview,
    OpenDocumentToCopyUrl,
    CopiedUrl { url: String },
}

impl PreviewMessage {
    /// The message rendered in the current interface language.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::OpenProjectToStart => tr!("preview.message.open_project_to_start"),
            Self::OpenProjectToRestart => tr!("preview.message.open_project_to_restart"),
            Self::AlreadyRunning => tr!("preview.message.already_running"),
            Self::Starting => tr!("preview.message.starting"),
            Self::Stopping => tr!("preview.message.stopping"),
            Self::Restarting => tr!("preview.message.restarting"),
            Self::Ready { url } => tr!("preview.message.ready", url = url.as_str()),
            Self::Stopped => tr!("preview.message.stopped"),
            Self::StoppedUnexpectedly { error } => {
                tr!(
                    "preview.message.stopped_unexpectedly",
                    error = error.as_str()
                )
            }
            Self::StoppedOnItsOwn => tr!("preview.message.stopped_on_its_own"),
            Self::ObserveFailed { error } => {
                tr!("preview.message.observe_failed", error = error.as_str())
            }
            Self::StartFailed { error } => {
                tr!("preview.message.start_failed", error = error.as_str())
            }
            Self::StartFailedWithOutput { error, output } => tr!(
                "preview.message.start_failed_with_output",
                error = error.as_str(),
                output = output.as_str()
            ),
            Self::StopFailed { error } => {
                tr!("preview.message.stop_failed", error = error.as_str())
            }
            Self::UrlUnresolved { error } => {
                tr!("preview.message.url_unresolved", error = error.as_str())
            }
            Self::OpenedInBrowser { url } => {
                tr!("preview.message.opened_in_browser", url = url.as_str())
            }
            Self::PreferredBrowserFailed { error } => tr!(
                "preview.message.preferred_browser_failed",
                error = error.as_str()
            ),
            Self::OpenedInSystemBrowser { url } => tr!(
                "preview.message.opened_in_system_browser",
                url = url.as_str()
            ),
            Self::SystemBrowserFailed { error } => tr!(
                "preview.message.system_browser_failed",
                error = error.as_str()
            ),
            Self::EmbeddedUnavailable { reason } => tr!(
                "preview.message.embedded_unavailable",
                reason = reason.as_str()
            ),
            Self::OpenDocumentToPreview => tr!("preview.message.open_document_to_preview"),
            Self::OpenDocumentToCopyUrl => tr!("preview.message.open_document_to_copy_url"),
            Self::CopiedUrl { url } => tr!("preview.message.copied_url", url = url.as_str()),
        }
    }
}

/// The message for a failed start, keeping any captured Astro output.
fn start_failure_message(failure: &StartupFailure) -> PreviewMessage {
    match failure.stderr_excerpt() {
        Some(output) => PreviewMessage::StartFailedWithOutput {
            error: failure.message().to_owned(),
            output: output.to_owned(),
        },
        None => PreviewMessage::StartFailed {
            error: failure.message().to_owned(),
        },
    }
}

/// Translate a host reason for unavailable embedded preview.
pub(crate) fn embedded_unavailable_text(reason: EmbeddedUnavailable) -> String {
    match reason {
        EmbeddedUnavailable::NotCompiled => tr!("preview.embedded.capability.not_compiled"),
        EmbeddedUnavailable::Wayland => tr!("preview.embedded.capability.wayland"),
        EmbeddedUnavailable::Headless => tr!("preview.embedded.capability.headless"),
        EmbeddedUnavailable::UnsupportedPlatform => {
            tr!("preview.embedded.capability.unsupported_platform")
        }
    }
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
            preferred_surface: None,
            presentation: PreviewPresentation::External,
            open_on_enter: false,
            visiting: false,
            pending_open: false,
            opened_this_visit: false,
            fallback_url: None,
            lifecycle: PreviewLifecycle::new(),
            message: None,
            pending_entry: None,
            task: None,
            exit_task: None,
            focus: cx.focus_handle(),
        }
    }

    /// Current server status.
    #[cfg(test)]
    pub(crate) fn status(&self) -> &ServerStatus {
        self.lifecycle.status()
    }

    /// Last user-facing message (errors, opened URL, start hint).
    #[cfg(test)]
    pub(crate) fn message(&self) -> Option<String> {
        self.message.as_ref().map(PreviewMessage::text)
    }

    /// URL waiting for the system-browser fallback, in tests.
    #[cfg(test)]
    pub(crate) fn fallback_url(&self) -> Option<&Url> {
        self.fallback_url.as_ref()
    }

    /// Current lifecycle phase.
    pub(crate) fn phase(&self) -> PreviewPhase {
        PreviewPhase::from_status(self.lifecycle.status())
    }

    /// Whether the panel is attached to an open project.
    pub(crate) fn is_attached(&self) -> bool {
        self.backend.is_some()
    }

    /// Whether the panel can open or copy a URL right now.
    pub(crate) fn is_ready(&self) -> bool {
        self.lifecycle.status().is_ready()
    }

    /// Whether a server process is attached (starting, ready or stopping).
    pub(crate) fn is_running(&self) -> bool {
        matches!(
            self.phase(),
            PreviewPhase::Starting | PreviewPhase::Ready | PreviewPhase::Stopping
        )
    }

    /// Whether the last operation failed and can be retried.
    pub(crate) fn is_failed(&self) -> bool {
        self.phase() == PreviewPhase::Failed
    }

    /// Whether a start/stop/restart operation is in flight.
    pub(crate) fn is_busy(&self) -> bool {
        self.task.is_some()
    }

    /// Apply the preview preferences to this panel.
    ///
    /// The browser surface is rebuilt only from the stored argv; it is never
    /// interpreted by a shell. An empty executable keeps the desktop opener.
    pub(crate) fn set_preferences(
        &mut self,
        preferences: &PreviewPreferences,
        cx: &mut Context<Self>,
    ) {
        self.open_on_enter = preferences.open_on_enter;
        self.presentation = preferences.presentation;
        self.preferred_surface = preferences.browser.is_configured().then(|| {
            Arc::new(ExternalBrowser::with_preferred(preferences.browser.clone()))
                as Arc<dyn PreviewSurface>
        });
        if !preferences.open_on_enter {
            self.pending_open = false;
        }
        cx.notify();
    }

    /// The URL the embedded surface shows: the active entry route when it
    /// resolves, otherwise the ready server's base URL.
    pub(crate) fn embedded_url(&self) -> Option<Url> {
        let status = self.lifecycle.status();
        let base = status.url()?.clone();
        let session = PreviewSession::new(self.mapping.clone(), self.surface.clone());
        self.pending_entry
            .as_ref()
            .and_then(|entry| session.url_for_entry(entry, status).ok())
            .or(Some(base))
    }

    /// The user explicitly entered Preview mode.
    ///
    /// This is the only place Open-on-Enter arms a launch. It never runs from
    /// rendering, and a pending launch is cleared on leave or failure.
    pub(crate) fn enter_preview(&mut self, cx: &mut Context<Self>) {
        self.visiting = true;
        self.fallback_url = None;
        self.pending_open =
            self.open_on_enter && self.presentation == PreviewPresentation::External;
        self.resume_visit(cx);
    }

    /// The user left Preview mode; drop the pending launch, not the server.
    pub(crate) fn leave_preview(&mut self, cx: &mut Context<Self>) {
        if !self.visiting && !self.pending_open {
            return;
        }
        self.visiting = false;
        self.pending_open = false;
        self.opened_this_visit = false;
        cx.notify();
    }

    /// Continue an explicit Preview visit after the project or server changes.
    pub(crate) fn resume_visit(&mut self, cx: &mut Context<Self>) {
        if !self.visiting {
            return;
        }
        if self.backend.is_none() {
            // Keep an Open-on-Enter launch armed: the explicit visit may have
            // begun before the project finished attaching.
            if self.pending_open {
                self.message = Some(PreviewMessage::OpenProjectToStart);
                cx.notify();
            }
            return;
        }
        // A failed server needs an explicit Retry; do not relaunch in a loop.
        if self.phase() == PreviewPhase::Failed {
            self.pending_open = false;
            cx.notify();
            return;
        }
        if !self.pending_open
            && !self.opened_this_visit
            && self.open_on_enter
            && self.presentation == PreviewPresentation::External
        {
            // The project attached after the explicit visit began; attach()
            // cleared the transient pending flag, so arm it once more.
            self.pending_open = true;
        }
        let wants_embedded = self.presentation == PreviewPresentation::Embedded;
        if !self.is_running() && (wants_embedded || self.pending_open) {
            self.start(cx);
            return;
        }
        self.maybe_open_pending(cx);
    }

    /// Open the armed entry once the server is ready and the route resolves.
    fn maybe_open_pending(&mut self, cx: &mut Context<Self>) {
        if !self.pending_open || !self.is_ready() {
            return;
        }
        let Some(entry) = self.pending_entry.clone() else {
            return;
        };
        self.pending_open = false;
        self.opened_this_visit = true;
        self.open_entry(&entry, cx);
    }

    /// Set the entry the toolbar button opens (the active document's entry).
    pub(crate) fn set_entry(&mut self, entry: Option<EntryId>, cx: &mut Context<Self>) {
        if self.pending_entry != entry {
            self.pending_entry = entry;
            // The shell also follows this event to retarget the embedded view.
            cx.emit(PreviewStateChanged);
            self.maybe_open_pending(cx);
            cx.notify();
        }
    }

    /// Project this panel is attached to, if any.
    #[cfg(test)]
    pub(crate) fn project(&self) -> Option<&ProjectId> {
        self.project.as_ref()
    }

    /// Root this panel is attached to, if any.
    #[cfg(test)]
    pub(crate) fn root(&self) -> Option<&PathBuf> {
        self.root.as_ref()
    }

    /// Route mapping currently in use.
    #[cfg(test)]
    pub(crate) fn mapping(&self) -> &RouteMapping {
        &self.mapping
    }

    /// Mark the server ready in a unit test without a backend round trip.
    #[cfg(test)]
    pub(crate) fn force_ready(&mut self, url: Url, cx: &mut Context<Self>) {
        let generation = self.lifecycle.invalidate();
        self.lifecycle.ready(generation, url);
        cx.notify();
    }

    /// Attach the real Astro backend for `project` at `root`.
    ///
    /// Reattaching to the same root only refreshes the project id and route
    /// mapping, so a collection change or reopen does not restart a running
    /// server. A different root stops the previous server before rebinding.
    pub(crate) fn ensure_attached(
        &mut self,
        project: ProjectId,
        root: PathBuf,
        mapping: RouteMapping,
        cx: &mut Context<Self>,
    ) {
        let current = match (self.project.as_ref(), self.root.as_ref()) {
            (Some(project), Some(root)) => Some(PreviewTarget::new(project.clone(), root.clone())),
            _ => None,
        };
        let next = PreviewTarget::new(project.clone(), root.clone());
        match target_change(current.as_ref(), Some(&next)) {
            TargetChange::Unchanged => {
                let changed = self.project.as_ref() != Some(&project) || self.mapping != mapping;
                if self.project.as_ref() != Some(&project) {
                    tracing::debug!(
                        project = %project.as_str(),
                        "reusing the preview server for the same project root"
                    );
                    self.project = Some(project.clone());
                }
                if self.mapping != mapping {
                    tracing::debug!(project = %project.as_str(), "refreshed the preview route mapping");
                    self.mapping = mapping;
                }
                if changed {
                    cx.notify();
                    cx.emit(PreviewStateChanged);
                }
            }
            TargetChange::Attach | TargetChange::Retarget => {
                tracing::info!(
                    project = %project.as_str(),
                    root = %root.display(),
                    "attaching the preview panel to a project"
                );
                let backend = Arc::new(AstroPreviewBackend::new(project.clone(), root.clone()));
                self.attach(backend, project, root, mapping, cx);
            }
            TargetChange::Detach => {}
        }
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
        cx.emit(PreviewStateChanged);
        cx.notify();
    }

    /// Drop the backend, killing any running server, and reset the panel.
    pub(crate) fn detach(&mut self, cx: &mut Context<Self>) {
        if self.project.is_some() {
            tracing::debug!("detaching the preview panel from its project");
        }
        self.exit_task = None;
        self.task = None;
        if let Some(backend) = self.backend.take() {
            backend.abort();
        }
        self.project = None;
        self.root = None;
        self.mapping = RouteMapping::new();
        self.pending_entry = None;
        self.pending_open = false;
        self.fallback_url = None;
        self.lifecycle.detach();
        self.message = None;
        cx.emit(PreviewStateChanged);
        cx.notify();
    }

    /// Kill the server synchronously for the application-exit path.
    pub(crate) fn abort(&mut self) {
        self.exit_task = None;
        self.task = None;
        if let Some(backend) = self.backend.take() {
            backend.abort();
        }
        self.lifecycle.detach();
    }

    /// Start watching a ready server for an unexpected exit.
    fn watch_exit(&mut self, backend: Arc<dyn PreviewBackend>, cx: &mut Context<Self>) {
        let generation = self.lifecycle.generation();
        self.exit_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let polled = runtime::spawn(cx, backend.poll_exit()).await;
                let keep_watching = this
                    .update(cx, |this, cx| {
                        if !this.lifecycle.is_current(generation) {
                            return false;
                        }
                        match polled {
                            Ok(Ok(Some(status))) => {
                                tracing::info!(status = status.label(), "Astro preview server exited");
                                if !this.lifecycle.observed_exit(generation, status.clone()) {
                                    return false;
                                }
                                this.message = Some(match &status {
                                    ServerStatus::Failed(error) => {
                                        PreviewMessage::StoppedUnexpectedly {
                                            error: error.clone(),
                                        }
                                    }
                                    _ => PreviewMessage::StoppedOnItsOwn,
                                });
                                cx.emit(PreviewStateChanged);
                                cx.notify();
                                false
                            }
                            Ok(Ok(None)) => true,
                            Ok(Err(error)) => {
                                tracing::warn!(%error, "could not observe the Astro preview server");
                                this.message =
                                    Some(PreviewMessage::ObserveFailed { error });
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
            self.message = Some(PreviewMessage::OpenProjectToStart);
            cx.notify();
            return;
        };
        if self.task.is_some() {
            return;
        }
        if self.is_ready() {
            self.message = Some(PreviewMessage::AlreadyRunning);
            cx.notify();
            return;
        }
        self.exit_task = None;
        let generation = self.lifecycle.begin_start();
        self.message = Some(PreviewMessage::Starting);
        let started = runtime::spawn(cx, backend.start());
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = started.await;
            let _ = this.update(cx, |this, cx| {
                if !this.lifecycle.is_current(generation) {
                    tracing::debug!("discarding a stale preview start result");
                    return;
                }
                match result {
                    Ok(Ok(url)) => {
                        tracing::info!(url = %url, "Astro preview ready");
                        this.message = Some(PreviewMessage::Ready {
                            url: url.to_string(),
                        });
                        this.lifecycle.ready(generation, url);
                        this.maybe_open_pending(cx);
                        this.watch_exit(backend.clone(), cx);
                    }
                    Ok(Err(failure)) => {
                        tracing::error!(%failure, "could not start the Astro preview");
                        this.pending_open = false;
                        this.message = Some(start_failure_message(&failure));
                        this.lifecycle
                            .failed(generation, failure.message().to_owned());
                    }
                    Err(error) => {
                        tracing::error!(%error, "could not start the Astro preview");
                        this.pending_open = false;
                        let failure = StartupFailure::new(error, None);
                        this.message = Some(start_failure_message(&failure));
                        this.lifecycle
                            .failed(generation, failure.message().to_owned());
                    }
                }
                this.task = None;
                cx.emit(PreviewStateChanged);
                cx.notify();
            });
        }));
        cx.emit(PreviewStateChanged);
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
        self.exit_task = None;
        let generation = self.lifecycle.begin_stop();
        self.message = Some(PreviewMessage::Stopping);
        let stopped = runtime::spawn(cx, backend.stop());
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = stopped.await;
            let _ = this.update(cx, |this, cx| {
                if !this.lifecycle.is_current(generation) {
                    tracing::debug!("discarding a stale preview stop result");
                    return;
                }
                match result {
                    Ok(Ok(())) => {
                        tracing::info!("Astro preview stopped");
                        this.lifecycle.stopped(generation);
                        this.message = Some(PreviewMessage::Stopped);
                    }
                    Ok(Err(error)) | Err(error) => {
                        tracing::error!(%error, "could not stop the Astro preview");
                        this.lifecycle.failed(generation, error.clone());
                        this.message = Some(PreviewMessage::StopFailed { error });
                    }
                }
                this.task = None;
                cx.emit(PreviewStateChanged);
                cx.notify();
            });
        }));
        cx.emit(PreviewStateChanged);
        cx.notify();
    }

    /// Stop and start the server so configuration or dependency changes apply.
    pub(crate) fn restart(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            self.message = Some(PreviewMessage::OpenProjectToRestart);
            cx.notify();
            return;
        };
        if self.task.is_some() {
            return;
        }
        self.exit_task = None;
        let generation = self.lifecycle.begin_restart();
        self.message = Some(PreviewMessage::Restarting);
        let restarting = backend.clone();
        let restarted = runtime::spawn(cx, async move {
            restarting
                .stop()
                .await
                .map_err(|message| StartupFailure::new(message, None))?;
            restarting.start().await
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = restarted.await;
            let _ = this.update(cx, |this, cx| {
                if !this.lifecycle.is_current(generation) {
                    tracing::debug!("discarding a stale preview restart result");
                    return;
                }
                match result {
                    Ok(Ok(url)) => {
                        tracing::info!(url = %url, "Astro preview restarted");
                        this.message = Some(PreviewMessage::Ready {
                            url: url.to_string(),
                        });
                        this.lifecycle.ready(generation, url);
                        this.maybe_open_pending(cx);
                        this.watch_exit(backend.clone(), cx);
                    }
                    Ok(Err(failure)) => {
                        tracing::error!(%failure, "could not restart the Astro preview");
                        this.message = Some(start_failure_message(&failure));
                        this.lifecycle
                            .failed(generation, failure.message().to_owned());
                    }
                    Err(error) => {
                        tracing::error!(%error, "could not restart the Astro preview");
                        this.pending_open = false;
                        let failure = StartupFailure::new(error, None);
                        this.message = Some(start_failure_message(&failure));
                        this.lifecycle
                            .failed(generation, failure.message().to_owned());
                    }
                }
                this.task = None;
                cx.emit(PreviewStateChanged);
                cx.notify();
            });
        }));
        cx.emit(PreviewStateChanged);
        cx.notify();
    }

    /// Open `entry` in the external browser, reusing the running server.
    ///
    /// The preferred browser and the fallback share one resolved URL, so a
    /// failed launch can retry the exact route with the system opener.
    pub(crate) fn open_entry(&mut self, entry: &EntryId, cx: &mut Context<Self>) {
        let session = PreviewSession::new(self.mapping.clone(), self.surface.clone());
        let url = match session.url_for_entry(entry, self.lifecycle.status()) {
            Ok(url) => url,
            Err(error) => {
                tracing::warn!(%error, "could not open a preview route");
                self.pending_open = false;
                self.fallback_url = None;
                self.message = Some(PreviewMessage::UrlUnresolved {
                    error: error.to_string(),
                });
                cx.notify();
                return;
            }
        };
        let surface = self
            .preferred_surface
            .clone()
            .unwrap_or_else(|| self.surface.clone());
        match surface.open(&url) {
            Ok(()) => {
                tracing::info!(url = %url, entry = %entry.id(), "opened a preview in the browser");
                self.fallback_url = None;
                self.message = Some(PreviewMessage::OpenedInBrowser {
                    url: url.to_string(),
                });
            }
            Err(error) => {
                tracing::warn!(%error, url = %url, "could not launch the preview browser");
                self.fallback_url = Some(url);
                self.message = Some(PreviewMessage::PreferredBrowserFailed {
                    error: error.to_string(),
                });
            }
        }
        cx.notify();
    }

    /// Retry the failed URL in the system default browser.
    pub(crate) fn open_fallback(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.fallback_url.clone() else {
            return;
        };
        match self.surface.open(&url) {
            Ok(()) => {
                tracing::info!(url = %url, "opened the preview in the system browser");
                self.fallback_url = None;
                self.message = Some(PreviewMessage::OpenedInSystemBrowser {
                    url: url.to_string(),
                });
            }
            Err(error) => {
                self.message = Some(PreviewMessage::SystemBrowserFailed {
                    error: error.to_string(),
                });
            }
        }
        cx.notify();
    }

    /// Report a host or adapter reason for unavailable embedded preview.
    pub(crate) fn note_embedded_unavailable(&mut self, reason: &str, cx: &mut Context<Self>) {
        self.message = Some(PreviewMessage::EmbeddedUnavailable {
            reason: reason.to_owned(),
        });
        self.fallback_url = self.lifecycle.status().url().cloned();
        cx.notify();
    }

    /// Open whichever entry the shell last marked active.
    pub(crate) fn open_active(&mut self, cx: &mut Context<Self>) {
        self.pending_open = false;
        self.opened_this_visit = true;
        match self.pending_entry.clone() {
            Some(entry) => self.open_entry(&entry, cx),
            None => {
                self.message = Some(PreviewMessage::OpenDocumentToPreview);
                cx.notify();
            }
        }
    }

    /// Copy the active entry's preview URL, once the server is ready.
    pub(crate) fn copy_url(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.pending_entry.clone() else {
            self.message = Some(PreviewMessage::OpenDocumentToCopyUrl);
            cx.notify();
            return;
        };
        let session = PreviewSession::new(self.mapping.clone(), self.surface.clone());
        match session.url_for_entry(&entry, self.lifecycle.status()) {
            Ok(url) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.to_string()));
                tracing::info!(url = %url, entry = %entry.id(), "copied the preview URL");
                self.message = Some(PreviewMessage::CopiedUrl {
                    url: url.to_string(),
                });
            }
            Err(error) => {
                tracing::warn!(%error, "could not resolve the preview URL");
                self.message = Some(PreviewMessage::UrlUnresolved {
                    error: error.to_string(),
                });
            }
        }
        cx.notify();
    }

    fn status_line(&self) -> String {
        match self.lifecycle.status().url() {
            Some(url) => tr!("preview.status.at", url = url.as_str()),
            None => tr!("preview.status.phase", phase = self.phase().text()),
        }
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        let status_line = self.status_line();
        let message = self.message.as_ref().map(PreviewMessage::text);
        let start_owner = owner.clone();
        let stop_owner = owner.clone();
        let restart_owner = owner.clone();
        let open_owner = owner.clone();
        let copy_owner = owner.clone();
        let fallback_owner = owner.clone();
        let fallback_url = self.fallback_url.clone();
        let phase = self.phase();
        let ready = self.is_ready();
        let running = self.is_running();
        let failed = self.is_failed();
        let busy = self.is_busy();
        let attached = self.backend.is_some();
        let url = self.lifecycle.status().url().map(|url| url.to_string());
        let status_color = match phase {
            PreviewPhase::Ready => cx.theme().success,
            PreviewPhase::Starting | PreviewPhase::Stopping => cx.theme().warning,
            PreviewPhase::Failed => cx.theme().danger,
            PreviewPhase::Stopped => cx.theme().muted_foreground,
        };
        div()
            .id("preview-panel")
            .track_focus(&self.focus)
            .v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.55))
                    .child(div().size_2().rounded_full().bg(status_color))
                    .child(
                        div()
                            .font_family(crate::theme::display_font(cx))
                            .text_base()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(tr!("preview.title")),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("preview-state")
                            .max_w(gpui_kit::px(420.))
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status_line),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .p_5()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("preview-start")
                                    .small()
                                    .label(if failed {
                                        tr!("preview.actions.retry")
                                    } else {
                                        tr!("preview.actions.start")
                                    })
                                    .disabled(running || busy)
                                    .on_click(move |_, _, cx| {
                                        let _ = start_owner.update(cx, |this, cx| this.start(cx));
                                    }),
                            )
                            .child(
                                Button::new("preview-stop")
                                    .small()
                                    .ghost()
                                    .label(tr!("preview.actions.stop"))
                                    .disabled(!running || busy)
                                    .on_click(move |_, _, cx| {
                                        let _ = stop_owner.update(cx, |this, cx| this.stop(cx));
                                    }),
                            )
                            .child(
                                Button::new("preview-restart")
                                    .small()
                                    .ghost()
                                    .label(tr!("preview.actions.restart"))
                                    .disabled(!attached || busy)
                                    .on_click(move |_, _, cx| {
                                        let _ =
                                            restart_owner.update(cx, |this, cx| this.restart(cx));
                                    }),
                            )
                            .child(
                                Button::new("preview-open")
                                    .small()
                                    .outline()
                                    .label(tr!("preview.actions.open"))
                                    .disabled(!ready)
                                    .on_click(move |_, _, cx| {
                                        let _ =
                                            open_owner.update(cx, |this, cx| this.open_active(cx));
                                    }),
                            )
                            .child(
                                Button::new("preview-copy-url")
                                    .small()
                                    .ghost()
                                    .label(tr!("preview.actions.copy_url"))
                                    .disabled(!ready)
                                    .on_click(move |_, _, cx| {
                                        let _ = copy_owner.update(cx, |this, cx| this.copy_url(cx));
                                    }),
                            )
                            .when(fallback_url.is_some(), |row| {
                                row.child(
                                    Button::new("preview-open-system")
                                        .small()
                                        .outline()
                                        .label(tr!("preview.actions.open_system"))
                                        .on_click(move |_, _, cx| {
                                            let _ = fallback_owner
                                                .update(cx, |this, cx| this.open_fallback(cx));
                                        }),
                                )
                            }),
                    )
                    .when_some(message, |view, message| {
                        view.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(message),
                        )
                    })
                    .when_some(url, |view, url| {
                        view.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(tr!("preview.status.serving", url = url.as_str())),
                        )
                    }),
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
