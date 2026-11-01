use std::{
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

use crate::{
    commands::{self, COMMANDS, CommandId},
    editor::{header::DocumentHeader, session::SessionChanged},
    file::{DocumentSource, read_document},
    frontmatter::{FrontmatterChanged, FrontmatterForm},
    layout::{
        panels::WorkspacePanel,
        state::{LayoutState, PanelKind},
    },
    preview::PreviewPanel,
    project_discovery::{ProjectOpened, ProjectWorkspace, now_secs},
    runtime,
    settings::{
        GlobalPreferencesV2, PreferencesStore, ProjectPreferencesV1, resolve, settings_path,
    },
    theme::{self, ThemeMode},
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt, TitleBar, WindowExt,
    button::{Button, ButtonVariants},
    command::{Command, CommandItem, CommandState},
    dock::{DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, panel_handle},
    kbd::Kbd,
    status_bar::StatusBar,
};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, PathPromptOptions,
    Render, SharedString, Subscription, Task, TestSupportExt, Window, WindowOptions, div,
    prelude::*, px, size,
};

/// Latest global and project snapshots, flushed by the quit observer.
#[derive(Clone)]
struct LatestPreferences {
    global: (u64, GlobalPreferencesV2),
    project: Option<(u64, ProjectPreferencesV1)>,
}
type LatestSettings = Arc<Mutex<LatestPreferences>>;

/// Preference scope of the project open in the shell.
///
/// The registry identity is the stable preferences key; the runtime
/// `ProjectId` (`workspace-{generation}`) must never address a preferences file.
struct ProjectScope {
    /// Canonical project root this scope was loaded for.
    root: PathBuf,
    preferences: ProjectPreferencesV1,
    /// Last revision accepted by the store for this project's file.
    revision: u64,
}

/// Open the shell after background preference loading; bootstrap owns no services.
pub fn open(cx: &mut App) {
    if let Err(error) = runtime::init(cx) {
        tracing::error!(%error, "could not start background services");
        eprintln!("Could not start background services: {error}");
        cx.quit();
        return;
    }
    commands::bind_keys(cx);
    commands::install_menus(cx);
    cx.spawn(async move |cx| {
        let loaded = runtime::blocking(cx, || {
            settings_path().map(|path| {
                let (store, settings, warning) = PreferencesStore::load(path);
                (Arc::new(store), settings, warning)
            })
        })
        .await
        .map_err(std::io::Error::other)
        .and_then(|result| result);
        cx.update(|cx| {
            let (store, settings, warning) = match loaded {
                Ok((store, settings, warning)) => {
                    if let Some(warning) = &warning {
                        tracing::warn!(%warning, "preferences loaded with a warning");
                    } else {
                        tracing::info!(theme = ?settings.theme, "preferences loaded");
                    }
                    (Some(store), settings, warning)
                }
                Err(error) => {
                    tracing::warn!(%error, "preferences unavailable; using defaults");
                    (
                        None,
                        GlobalPreferencesV2::default(),
                        Some(format!("Preferences unavailable: {error}")),
                    )
                }
            };
            if let Err(error) = gpui_kit::open_window(
                WindowOptions {
                    window_min_size: Some(size(px(800.), px(600.))),
                    ..TitleBar::window_options()
                },
                cx,
                |window, cx| {
                    window.set_window_title("Astrolith");
                    cx.new(|cx| Shell::new(settings, store, warning, window, cx))
                },
            ) {
                tracing::error!(error = %format_args!("{error:#}"), "could not open the main window");
                eprintln!("Could not open Astrolith: {error:#}");
                cx.quit();
            } else {
                tracing::info!("main window opened");
            }
        });
    })
    .detach();
}

pub(crate) struct Shell {
    dock: Entity<DockArea>,
    document: Entity<WorkspacePanel>,
    project: Entity<ProjectWorkspace>,
    navigator: Entity<WorkspacePanel>,
    frontmatter: Entity<FrontmatterForm>,
    frontmatter_key: Option<(PathBuf, bool)>,
    preview: Entity<PreviewPanel>,
    palette: Entity<CommandState>,
    palette_focus: FocusHandle,
    opening: bool,
    opening_task: Option<Task<()>>,
    file_status: String,
    preview_sync_key: Option<(u64, Option<PathBuf>)>,
    last_viewport: gpui_kit::Size<gpui_kit::Pixels>,
    _skin: Rc<DockSkin>,
    settings: GlobalPreferencesV2,
    /// Layout last written to (or loaded from) the global scope.
    global_layout: LayoutState,
    /// Project scope currently open, if any; absorbs layout and last-entry writes.
    project_scope: Option<ProjectScope>,
    /// A project is opening: its scope is not installed yet, so layout writes
    /// are held back until the registry lookup finishes.
    scope_loading: Option<PathBuf>,
    /// The dock is being set programmatically; its intermediate events are not
    /// user changes and must not be persisted.
    applying_layout: bool,
    store: Option<Arc<PreferencesStore>>,
    latest: LatestSettings,
    revision: u64,
    settings_task: Option<Task<()>>,
    project_task: Option<Task<()>>,
    scope_task: Option<Task<()>>,
    warning: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    fn new(
        mut settings: GlobalPreferencesV2,
        store: Option<Arc<PreferencesStore>>,
        warning: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        settings
            .layout
            .clamp(f32::from(window.viewport_size().width));
        let global_layout = settings.layout.clone();
        theme::apply(
            resolve(&settings, None, None).theme,
            window.appearance(),
            cx,
        );
        let navigator = cx.new(|cx| {
            WorkspacePanel::new(
                "navigator-panel",
                "Project",
                "No project open\n\nOpen a local document to view its source.",
                window,
                cx,
            )
        });
        let document = cx.new(|cx| WorkspacePanel::new("document-panel", "Welcome", "# Astrolith\n\nOpen an Astro project to edit Markdown and MDX.\n\nPress Ctrl/Cmd+S to save the active document.\n\n中文 · 日本語", window, cx));
        let project = cx.new(|cx| ProjectWorkspace::new(document.clone(), window, cx));
        project.update(cx, |project, cx| {
            project.set_preferences(settings.clone(), None, cx)
        });
        navigator.update(cx, |navigator, cx| {
            navigator.set_view(project.clone().into(), cx)
        });
        let header = cx.new(|cx| DocumentHeader::new(project.clone(), cx));
        document.update(cx, |document, cx| document.set_header(header.into(), cx));
        let project_owner = project.downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            project_owner
                .update(cx, |project, cx| project.guard_window_close(window, cx))
                .unwrap_or(true)
        });
        let frontmatter = cx.new(FrontmatterForm::new);
        let inspector = cx.new(|cx| WorkspacePanel::new("inspector-panel", "Inspector", "No collection selected\n\nCollection metadata becomes available when an Astro workspace is open.", window, cx));
        inspector.update(cx, |panel, cx| {
            panel.set_view(frontmatter.clone().into(), cx)
        });
        let preview = cx.new(PreviewPanel::new);
        let preview_panel = cx.new(|cx| WorkspacePanel::new("preview-panel", "Preview", "No preview running\n\nStart the Astro dev server, then open the active entry in your browser.", window, cx));
        preview_panel.update(cx, |panel, cx| panel.set_view(preview.clone().into(), cx));
        let (dock, skin) = DockSkin::dock_area("astrolith-workspace", Some(1), window, cx);
        dock.update(cx, |area, cx| {
            area.set_center(
                DockLayout::tabs().panel_view(panel_handle(document.clone()), cx),
                window,
                cx,
            );
            for (placement, panel, visible) in [
                (
                    DockPlacement::Left,
                    panel_handle(navigator.clone()),
                    settings.layout.sidebar_visible,
                ),
                (
                    DockPlacement::Right,
                    panel_handle(inspector),
                    settings.layout.inspector_visible,
                ),
                (
                    DockPlacement::Bottom,
                    panel_handle(preview_panel),
                    settings.layout.preview_visible,
                ),
            ] {
                area.set_dock(
                    placement,
                    DockLayout::tabs().panel_view(panel, cx),
                    window,
                    cx,
                );
                if !visible {
                    area.toggle_dock(placement, window, cx);
                }
            }
            // Dock sizes are measured logical pixels, not application spacing.
            area.set_dock_size(
                DockPlacement::Left,
                px(settings.layout.sidebar_width),
                window,
                cx,
            );
            area.set_dock_size(
                DockPlacement::Right,
                px(settings.layout.inspector_width),
                window,
                cx,
            );
            area.set_dock_size(
                DockPlacement::Bottom,
                px(settings.layout.preview_height),
                window,
                cx,
            );
        });
        let palette = cx.new(|cx| CommandState::new(window, cx));
        let latest = Arc::new(Mutex::new(LatestPreferences {
            global: (0, settings.clone()),
            project: None,
        }));
        let mut subscriptions = vec![
            cx.subscribe_in(&dock, window, |this, _, event: &DockEvent, _, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    // Dock events already carry the new geometry. Clamping here
                    // would mutate the dock on every splitter move.
                    this.capture_layout(cx);
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                let viewport = window.viewport_size();
                if viewport == this.last_viewport {
                    return;
                }
                this.last_viewport = viewport;
                this.constrain_docks(window, cx);
                this.capture_layout(cx);
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.settings.theme == ThemeMode::System {
                    theme::apply(ThemeMode::System, window.appearance(), cx);
                }
            }),
        ];
        subscriptions.push(cx.subscribe_in(
            &project,
            window,
            |this, _, _: &SessionChanged, _window, cx| this.sync_preview(cx),
        ));
        // Recording the open, restoring the project layout and reopening its
        // last entry all need the window the project open completed in.
        subscriptions.push(cx.subscribe_in(
            &project,
            window,
            |this, _, event: &ProjectOpened, window, cx| {
                this.open_project_scope(event.root.clone(), window, cx)
            },
        ));
        // Project load, entry activation and watcher refreshes notify the
        // workspace without emitting a text-edit event; the preview panel must
        // still attach on open and follow the active entry.
        subscriptions.push(cx.observe(&project, |this, _, cx| {
            this.sync_preview(cx);
            this.track_active_entry(cx);
        }));
        let apply_owner = project.downgrade();
        subscriptions.push(cx.subscribe_in(
            &frontmatter,
            window,
            move |_this, _form, event: &FrontmatterChanged, window, cx| {
                let path = event.path.clone();
                let value = event.value.clone();
                let _ = apply_owner.update(cx, |project, cx| {
                    project.apply_frontmatter_field(path, value, window, cx)
                });
            },
        ));
        if let Some(store) = store.clone() {
            let latest = latest.clone();
            // Register on App rather than the view: last-window closure may drop the view first.
            App::on_app_quit(cx, move |cx| {
                let store = store.clone();
                let latest = latest.clone();
                let flush = runtime::flush_on_quit(cx, move || {
                    let snapshot = latest.lock().ok().map(|snapshot| snapshot.clone());
                    if let Some(snapshot) = snapshot {
                        let (revision, settings) = &snapshot.global;
                        if let Err(error) = store.save_global(*revision, settings) {
                            tracing::error!(%error, "could not save preferences on exit");
                            eprintln!("Could not save preferences on exit: {error}");
                        }
                        if let Some((revision, preferences)) = &snapshot.project
                            && let Err(error) = store.save_project(*revision, preferences)
                        {
                            tracing::error!(%error, "could not save project preferences on exit");
                            eprintln!("Could not save project preferences on exit: {error}");
                        }
                    }
                });
                cx.background_executor().spawn(async move {
                    if let Err(error) = flush.await {
                        tracing::error!(%error, "could not finish the preference flush");
                        eprintln!("Could not finish preference flush: {error}");
                    } else {
                        tracing::debug!("preference flush finished");
                    }
                })
            })
            .detach();
        }
        // Quit observers run before windows are cleared, so the workspace entities
        // are still alive here and can hand their latest dirty text to a durable
        // flush even when the 500 ms recovery debounce has not fired.
        let recovery_owner = project.downgrade();
        App::on_app_quit(cx, move |cx| {
            let flush = recovery_owner
                .update(cx, |project, cx| project.flush_recovery_on_quit(cx))
                .ok()
                .flatten();
            async move {
                if let Some(flush) = flush
                    && let Err(error) = flush.await
                {
                    tracing::error!(%error, "could not flush unsaved recovery on exit");
                    eprintln!("Could not flush unsaved recovery on exit: {error}");
                }
            }
        })
        .detach();
        let preview_owner = preview.downgrade();
        App::on_app_quit(cx, move |cx| {
            let _ = preview_owner.update(cx, |preview, _| preview.abort());
            async move {}
        })
        .detach();
        let focus = cx.focus_handle().tab_stop(true);
        focus.focus(window, cx);
        Self {
            dock,
            document,
            project,
            navigator,
            frontmatter,
            frontmatter_key: None,
            preview,
            palette,
            palette_focus: cx.focus_handle().tab_stop(true),
            opening: false,
            opening_task: None,
            file_status: "Readonly · No file open".into(),
            preview_sync_key: None,
            last_viewport: window.viewport_size(),
            _skin: skin,
            settings,
            global_layout,
            project_scope: None,
            scope_loading: None,
            applying_layout: false,
            store,
            latest,
            revision: 0,
            settings_task: None,
            project_task: None,
            scope_task: None,
            warning,
            focus,
            _subscriptions: subscriptions,
        }
    }

    fn constrain_docks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let side_maximum = ((f32::from(viewport.width) - 320.) / 2.).clamp(160., 400.);
        // Reserve vertical room for document content, tabs, toolbar, and status.
        let bottom_maximum = (f32::from(viewport.height) - 300.).max(80.);
        let mut next = [None; 3];
        {
            let dock = self.dock.read(cx);
            for (slot, (placement, maximum)) in next.iter_mut().zip([
                (DockPlacement::Left, side_maximum),
                (DockPlacement::Right, side_maximum),
                (DockPlacement::Bottom, bottom_maximum),
            ]) {
                if let Some(current) = dock.dock_size(placement) {
                    let minimum = if placement == DockPlacement::Bottom {
                        80.
                    } else {
                        160.
                    };
                    let constrained = f32::from(current).clamp(minimum, maximum);
                    if current != px(constrained) {
                        *slot = Some((placement, px(constrained)));
                    }
                }
            }
        }
        if next.iter().any(Option::is_some) {
            self.dock.update(cx, |dock, cx| {
                for (placement, size) in next.into_iter().flatten() {
                    dock.set_dock_size(placement, size, window, cx);
                }
            });
        }
    }

    /// The layout the dock currently reports.
    fn read_dock_layout(&self, cx: &App) -> LayoutState {
        let dock = self.dock.read(cx);
        LayoutState {
            sidebar_visible: dock.is_dock_open(DockPlacement::Left),
            inspector_visible: dock.is_dock_open(DockPlacement::Right),
            preview_visible: dock.is_dock_open(DockPlacement::Bottom),
            sidebar_width: dock
                .dock_size(DockPlacement::Left)
                .map(f32::from)
                .unwrap_or(240.),
            inspector_width: dock
                .dock_size(DockPlacement::Right)
                .map(f32::from)
                .unwrap_or(260.),
            // A closed bottom dock has no settled size; keep the stored value.
            preview_height: dock
                .dock_size(DockPlacement::Bottom)
                .map(f32::from)
                .filter(|_| self.settings.layout.preview_visible)
                .unwrap_or(self.settings.layout.preview_height),
        }
    }

    fn capture_layout(&mut self, cx: &mut Context<Self>) {
        // A project's scope is being looked up; there is no owner for this
        // change yet and the loaded project layout will replace it anyway.
        if self.scope_loading.is_some() || self.applying_layout {
            return;
        }
        let layout = self.read_dock_layout(cx);
        let visibility_changed = layout.sidebar_visible != self.settings.layout.sidebar_visible
            || layout.inspector_visible != self.settings.layout.inspector_visible
            || layout.preview_visible != self.settings.layout.preview_visible;
        if layout == self.settings.layout {
            return;
        }
        self.settings.layout = layout.clone();
        // With a project open the dock belongs to that project's scope; a
        // project file records both visibility and the live widths.
        if let Some(scope) = self.project_scope.as_mut() {
            scope.preferences.layout = Some(layout);
            self.persist_project(cx);
        } else {
            self.persist_global(cx);
        }
        // Width-only changes are carried by the dock itself. Notifying here
        // would rebuild the whole shell once per resize frame.
        if visibility_changed {
            cx.notify();
        }
    }

    /// Persist the global scope, keeping a project layout out of the global file.
    fn persist_global(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        let revision = self.revision;
        let mut settings = self.settings.clone();
        if self.project_scope.is_some() || self.scope_loading.is_some() {
            settings.layout = self.global_layout.clone();
        } else {
            self.global_layout = settings.layout.clone();
        }
        self.record_latest(|latest| latest.global = (revision, settings.clone()));
        let Some(store) = self.store.clone() else {
            return;
        };
        self.settings_task = Some(debounced_save(cx, SaveScope::Global, revision, move || {
            store.save_global(revision, &settings)
        }));
    }

    /// Persist the open project's scope; a no-op when no project is open.
    fn persist_project(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.project_scope.as_mut() else {
            return;
        };
        scope.revision += 1;
        let revision = scope.revision;
        let preferences = scope.preferences.clone();
        self.record_latest(|latest| latest.project = Some((revision, preferences.clone())));
        let Some(store) = self.store.clone() else {
            return;
        };
        self.project_task = Some(debounced_save(
            cx,
            SaveScope::Project,
            revision,
            move || store.save_project(revision, &preferences),
        ));
    }

    fn record_latest(&self, update: impl FnOnce(&mut LatestPreferences)) {
        if let Ok(mut latest) = self.latest.lock() {
            update(&mut latest);
        }
    }

    /// Take the pending project snapshot so a scope switch can flush it before
    /// the registry revision moves.
    fn take_pending_project(&self) -> Option<(u64, ProjectPreferencesV1)> {
        self.latest
            .lock()
            .ok()
            .and_then(|mut latest| latest.project.take())
    }

    /// Store the open project as the active preferences scope, then restore it.
    fn open_project_scope(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // A newly opened project owns the scope immediately: its layout must
        // never be written into the previously open project's file while the
        // registry lookup is still running.
        self.project_scope = None;
        self.scope_loading = Some(root.clone());
        let Some(store) = self.store.clone() else {
            self.scope_loading = None;
            return;
        };
        // The outgoing project's debounced write may still be pending. Flush it
        // here, before this open bumps the registry revision, so a newer layout
        // cannot be rejected as stale or overwritten by the stored snapshot.
        let pending = self.take_pending_project();
        let work = {
            let root = root.clone();
            move || {
                let flush_warning = pending.and_then(|(revision, preferences)| {
                    store
                        .save_project(revision, &preferences)
                        .err()
                        .map(|error| {
                            format!("Could not save previous project preferences: {error}")
                        })
                });
                let package_name = project_package_name(&root);
                let open = store
                    .record_project_open(&root, package_name.as_deref(), now_secs())
                    .map_err(|error| error.to_string())?;
                let (preferences, revision, warning) =
                    store.project(&open.id).map_err(|error| error.to_string())?;
                Ok((preferences, revision, warning.or(flush_warning)))
            }
        };
        let task = runtime::blocking(cx, work);
        self.scope_task = Some(cx.spawn_in(window, async move |this, cx| {
            let loaded = task.await.and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| {
                if this.project.read(cx).project_root().as_ref() != Some(&root)
                    || this.scope_loading.as_ref() != Some(&root)
                {
                    return;
                }
                this.install_project_scope(root, loaded, window, cx);
            });
        }));
    }

    fn install_project_scope(
        &mut self,
        root: PathBuf,
        loaded: Result<(ProjectPreferencesV1, u64, Option<String>), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scope_loading = None;
        let (preferences, revision, warning) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                tracing::warn!(%error, "could not load project preferences");
                self.warning = Some(format!("Could not load project preferences: {error}"));
                // The scope is global again; make the visible layout match it
                // so later changes are persisted where they belong.
                let layout = self.global_layout.clone();
                self.apply_layout(layout, window, cx);
                cx.notify();
                return;
            }
        };
        if let Some(warning) = warning {
            tracing::warn!(%warning, "project preferences loaded with a warning");
            self.warning = Some(warning);
        }
        let global = GlobalPreferencesV2 {
            layout: self.global_layout.clone(),
            ..self.settings.clone()
        };
        let resolved = resolve(&global, Some(&preferences), None);
        self.project_scope = Some(ProjectScope {
            root,
            preferences: preferences.clone(),
            revision,
        });
        self.sync_project_preferences(cx);
        self.apply_layout(resolved.layout, window, cx);
        self.project.update(cx, |project, cx| {
            project.reopen_last_entry(resolved.last_entry.as_deref(), window, cx)
        });
        cx.notify();
    }

    /// Push the resolved preference snapshots the workspace uses for the file
    /// list and entry creation.
    fn sync_project_preferences(&mut self, cx: &mut Context<Self>) {
        let global = self.settings.clone();
        let preferences = self
            .project_scope
            .as_ref()
            .map(|scope| scope.preferences.clone());
        self.project.update(cx, |project, cx| {
            project.set_preferences(global, preferences, cx)
        });
    }

    /// Apply a resolved layout to the dock without persisting it back.
    fn apply_layout(
        &mut self,
        mut layout: LayoutState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        layout.clamp(f32::from(window.viewport_size().width));
        // Set the model first: dock events arrive afterwards and must not look
        // like a user change.
        self.settings.layout = layout.clone();
        self.applying_layout = true;
        self.dock.update(cx, |dock, cx| {
            for (placement, visible) in [
                (DockPlacement::Left, layout.sidebar_visible),
                (DockPlacement::Right, layout.inspector_visible),
                (DockPlacement::Bottom, layout.preview_visible),
            ] {
                if dock.is_dock_open(placement) != visible {
                    dock.toggle_dock(placement, window, cx);
                }
            }
            dock.set_dock_size(DockPlacement::Left, px(layout.sidebar_width), window, cx);
            dock.set_dock_size(DockPlacement::Right, px(layout.inspector_width), window, cx);
            if layout.preview_visible {
                dock.set_dock_size(DockPlacement::Bottom, px(layout.preview_height), window, cx);
            }
        });
        self.applying_layout = false;
    }

    /// Record the active document as the open project's last entry.
    fn track_active_entry(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.project_scope.as_ref() else {
            return;
        };
        if self.project.read(cx).project_root().as_ref() != Some(&scope.root) {
            return;
        }
        let Some(relative) = self.project.read(cx).active_relative_source() else {
            return;
        };
        let scope = self.project_scope.as_mut().expect("scope checked above");
        if scope.preferences.last_entry.as_deref() == Some(relative.as_str()) {
            return;
        }
        scope.preferences.last_entry = Some(relative);
        self.persist_project(cx);
    }

    fn toggle_panel(&mut self, kind: PanelKind, window: &mut Window, cx: &mut Context<Self>) {
        let placement = match kind {
            PanelKind::Sidebar => DockPlacement::Left,
            PanelKind::Inspector => DockPlacement::Right,
            PanelKind::Preview => DockPlacement::Bottom,
        };
        self.dock
            .update(cx, |area, cx| area.toggle_dock(placement, window, cx));
        // LayoutChanged also captures native collapse/resize controls.
        cx.notify();
    }
}

impl Shell {
    /// Attach the preview panel to the open project and its active entry.
    ///
    /// Reattaching to the same project only refreshes the mapping, so a session
    /// change never restarts a running dev server.
    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        let key = {
            let project = self.project.read(cx);
            (project.index_revision(), project.active_path())
        };
        if self.preview_sync_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_sync_key = Some(key);
        let context = self.project.read(cx).preview_context();
        let entry = self.project.read(cx).active_entry_id();
        self.preview.update(cx, |preview, cx| {
            match context {
                Some((project, root, mapping)) => {
                    preview.ensure_attached(project, root, mapping, cx)
                }
                None => preview.detach(cx),
            }
            preview.set_entry(entry, cx);
        });
    }

    fn execute(&mut self, command: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            CommandId::OpenFile => self.open_file(window, cx),
            CommandId::Save => {
                self.project
                    .update(cx, |project, cx| project.save_active(window, cx));
            }
            CommandId::CloseDocument => {
                self.project
                    .update(cx, |project, cx| project.close_active(window, cx));
            }
            CommandId::RecoverUnsaved => {
                self.project
                    .update(cx, |project, cx| project.recover_pending(window, cx));
            }
            CommandId::NewEntry
            | CommandId::DuplicateEntry
            | CommandId::RenameEntry
            | CommandId::DeleteEntry
            | CommandId::RevealEntry
            | CommandId::CopyPathAbsolute
            | CommandId::CopyPathRelative => {
                self.project.update(cx, |project, cx| {
                    project.entry_command(command, None, window, cx)
                });
            }
            CommandId::OpenProject => {
                if !self.dock.read(cx).is_dock_open(DockPlacement::Left) {
                    self.toggle_panel(PanelKind::Sidebar, window, cx);
                    // Revealing the navigator is a transient UI action for the
                    // project about to open. Mirror the dock so the deferred
                    // layout event compares equal, but never persist it into
                    // the outgoing project's or the global scope.
                    self.settings.layout = self.read_dock_layout(cx);
                }
                self.project
                    .update(cx, |project, cx| project.prompt(window, cx));
            }
            CommandId::OpenPalette => self.open_palette(window, cx),
            CommandId::ToggleSidebar => self.toggle_panel(PanelKind::Sidebar, window, cx),
            CommandId::ToggleInspector => self.toggle_panel(PanelKind::Inspector, window, cx),
            CommandId::TogglePreview => self.toggle_panel(PanelKind::Preview, window, cx),
            CommandId::StartPreview => self.preview.update(cx, |preview, cx| preview.start(cx)),
            CommandId::StopPreview => self.preview.update(cx, |preview, cx| preview.stop(cx)),
            CommandId::OpenPreview => {
                let entry = self.project.read(cx).active_entry_id();
                self.preview.update(cx, |preview, cx| {
                    preview.set_entry(entry, cx);
                    preview.open_active(cx);
                });
            }
            CommandId::Quit => {
                // Reuse the window-close guard so unsaved work prompts exactly
                // as a titlebar close would.
                let clean = self
                    .project
                    .update(cx, |project, cx| project.guard_window_close(window, cx));
                if clean {
                    cx.quit();
                }
            }
            CommandId::FocusDocument => self.document.read(cx).focus_handle(cx).focus(window, cx),
            CommandId::LightTheme | CommandId::DarkTheme | CommandId::SystemTheme => {
                self.settings.theme = match command {
                    CommandId::LightTheme => ThemeMode::Light,
                    CommandId::DarkTheme => ThemeMode::Dark,
                    _ => ThemeMode::System,
                };
                theme::apply(self.settings.theme, window.appearance(), cx);
                self.persist_global(cx);
            }
            CommandId::ResetLayout => {
                let mut layout = LayoutState::default();
                layout.clamp(f32::from(window.viewport_size().width));
                self.dock.update(cx, |area, cx| {
                    for (placement, visible) in [
                        (DockPlacement::Left, true),
                        (DockPlacement::Right, true),
                        (DockPlacement::Bottom, false),
                    ] {
                        if area.is_dock_open(placement) != visible {
                            area.toggle_dock(placement, window, cx);
                        }
                    }
                    area.set_dock_size(DockPlacement::Left, px(layout.sidebar_width), window, cx);
                    area.set_dock_size(
                        DockPlacement::Right,
                        px(layout.inspector_width),
                        window,
                        cx,
                    );
                });
            }
        }
        cx.notify();
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        self.palette
            .update(cx, |state, cx| state.set_query("", window, cx));
        let palette = self.palette.clone();
        let owner = cx.weak_entity();
        let availability = self.project.read(cx).command_availability(cx);
        let commands: Vec<_> = COMMANDS
            .iter()
            .copied()
            .filter(|command| {
                *command != CommandId::OpenPalette && command.is_available(availability)
            })
            .collect();
        let opening = self.opening;
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let commands_for_confirm = commands.clone();
            dialog.title("Command palette").w_128().child(
                Command::new(&palette)
                    .bordered(false)
                    .placeholder("Search commands…")
                    .items(commands.iter().map(|command| {
                        let label: gpui_kit::SharedString = command.label().into();
                        let shortcut: gpui_kit::SharedString = command
                            .shortcut()
                            .and_then(|key| gpui_kit::Keystroke::parse(key).ok())
                            .map(|key| Kbd::format(&key).into())
                            .unwrap_or_default();
                        let command = *command;
                        CommandItem::new()
                            .label(label.clone())
                            .icon(command_icon(command))
                            .child(move |_, cx| {
                                div()
                                    .h_flex()
                                    .w_full()
                                    .items_center()
                                    .justify_between()
                                    .gap_4()
                                    .child(
                                        div()
                                            .h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                Icon::new(command_icon(command))
                                                    .small()
                                                    .text_color(cx.theme().muted_foreground),
                                            )
                                            .child(label.clone()),
                                    )
                                    .when(!shortcut.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(shortcut.clone()),
                                        )
                                    })
                            })
                            .disabled(command == CommandId::OpenFile && opening)
                    }))
                    .empty(|_, _, cx| {
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No matching commands")
                    })
                    .on_confirm(move |index, window, cx| {
                        let Some(command) = commands_for_confirm.get(index.row).copied() else {
                            return;
                        };
                        window.close_dialog(cx);
                        let _ = owner.update(cx, |this, cx| this.execute(command, window, cx));
                    }),
            )
        });
        self.palette.update(cx, |state, cx| state.focus(window, cx));
    }

    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening {
            return;
        }
        self.opening = true;
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open UTF-8 text file".into()),
        });
        self.opening_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match selected.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => Some(
                        runtime::blocking(cx, move || read_document(path))
                            .await
                            .and_then(|result| result.map_err(|error| error.to_string())),
                    ),
                    None => None,
                },
                Ok(Ok(None)) => None,
                Ok(Err(error)) => Some(Err(error.to_string())),
                Err(error) => Some(Err(error.to_string())),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.opening = false;
                match result {
                    Some(Ok(source)) => this.show_document(source, window, cx),
                    Some(Err(error)) => {
                        tracing::warn!(%error, "could not open a text file");
                        this.file_status = format!("Could not open file: {error}");
                    }
                    None => {}
                }
                cx.notify();
            });
        }));
    }

    fn show_document(
        &mut self,
        source: DocumentSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::info!(
            path = %source.path.display(),
            bytes = source.source.len(),
            "opened a readonly text file"
        );
        self.navigator.update(cx, |panel, cx| {
            panel.set_content(source.path.display().to_string(), cx)
        });
        window.set_window_title(&format!(
            "{} · Astrolith",
            source
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        self.file_status = format!(
            "Readonly · {} bytes · {}",
            source.source.len(),
            source.path.display()
        );
        self.document
            .update(cx, |panel, cx| panel.show_source(source, window, cx));
        self.document.read(cx).focus_handle(cx).focus(window, cx);
    }
}

fn command_icon(command: CommandId) -> IconName {
    match command {
        CommandId::OpenFile => IconName::FileText,
        CommandId::OpenProject => IconName::FolderOpen,
        CommandId::NewEntry => IconName::FilePlus,
        CommandId::DuplicateEntry => IconName::Copy,
        CommandId::RenameEntry => IconName::SquarePen,
        CommandId::DeleteEntry => IconName::Trash,
        CommandId::RevealEntry => IconName::FolderOpen,
        CommandId::CopyPathAbsolute | CommandId::CopyPathRelative => IconName::Copy,
        CommandId::Save => IconName::Save,
        CommandId::CloseDocument => IconName::X,
        CommandId::RecoverUnsaved => IconName::RotateCcw,
        CommandId::OpenPalette => IconName::Command,
        CommandId::ToggleSidebar => IconName::PanelLeft,
        CommandId::ToggleInspector => IconName::PanelRight,
        CommandId::TogglePreview => IconName::PanelBottom,
        CommandId::StartPreview => IconName::Play,
        CommandId::StopPreview => IconName::Square,
        CommandId::OpenPreview => IconName::Globe,
        CommandId::FocusDocument => IconName::FileText,
        CommandId::LightTheme => IconName::Sun,
        CommandId::DarkTheme => IconName::Moon,
        CommandId::SystemTheme => IconName::Monitor,
        CommandId::ResetLayout => IconName::RotateCcw,
        CommandId::Quit => IconName::X,
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Activate the frontmatter form whenever the active document (or its
        // schema availability) changes. Editing the form does not change this
        // key, so live edits never rebuild the controls mid-entry.
        let key = self.project.read(cx).active_frontmatter_key();
        if key != self.frontmatter_key {
            self.frontmatter_key = key;
            match self.project.read(cx).active_frontmatter(cx) {
                Some((schema, text)) => {
                    self.frontmatter
                        .update(cx, |form, cx| form.set_document(schema, text, window, cx));
                }
                None => {
                    self.frontmatter.update(cx, |form, cx| form.clear(cx));
                }
            }
        }
        let layout = &self.settings.layout;
        let (project_name, tabs, active_index) = {
            let project = self.project.read(cx);
            (
                project.project_name(),
                project.open_tabs(),
                project.active_tab_index(),
            )
        };
        let active_tab = tabs.get(active_index);
        let document_state: SharedString = match active_tab {
            None if self.opening => "Opening file…".into(),
            None if self.document.read(cx).is_editable() => self.file_status.clone().into(),
            None => "No file open".into(),
            Some(tab) if tab.externally_changed => "Changed on disk".into(),
            Some(tab) if tab.dirty => "Unsaved changes".into(),
            Some(_) => "Saved".into(),
        };
        let language = active_tab
            .map(|tab| {
                if tab.label.ends_with(".mdx") {
                    "MDX"
                } else {
                    "Markdown"
                }
            })
            .unwrap_or("—");

        let close_owner = self.project.downgrade();
        div()
            .v_flex()
            .track_focus(&self.focus)
            .key_context("Astrolith")
            .on_action(cx.listener(|this, command: &CommandId, window, cx| {
                this.execute(*command, window, cx)
            }))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TitleBar::new()
                    .on_close_window(move |_, window, cx| {
                        let clean = close_owner
                            .update(cx, |project, cx| project.guard_window_close(window, cx))
                            .unwrap_or(true);
                        if clean {
                            window.remove_window();
                        }
                    })
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .h_full()
                            .items_center()
                            .gap_2()
                            .pr_2()
                            .child(
                                Icon::new(IconName::Asterisk)
                                    .small()
                                    .text_color(cx.theme().accent),
                            )
                            .child(div().text_sm().child("Astrolith"))
                            .when_some(project_name.clone(), |row, name| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(name),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                Button::new("command-palette")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Search)
                                    .label("Search or run a command…")
                                    .tooltip(CommandId::OpenPalette.label())
                                    .track_focus(&self.palette_focus)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.palette_focus.focus(window, cx);
                                        this.execute(CommandId::OpenPalette, window, cx);
                                    })),
                            )
                            .child({
                                let availability = self.project.read(cx).command_availability(cx);
                                Button::new("toolbar-new-entry")
                                    .small()
                                    .ghost()
                                    .icon(IconName::FilePlus)
                                    .tooltip(CommandId::NewEntry.label())
                                    .disabled(!CommandId::NewEntry.is_available(availability))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.execute(CommandId::NewEntry, window, cx)
                                    }))
                            })
                            .child(div().flex_1())
                            .children(
                                [
                                    (
                                        "toggle-sidebar",
                                        CommandId::ToggleSidebar,
                                        IconName::PanelLeft,
                                        layout.sidebar_visible,
                                        "Toggle project panel",
                                    ),
                                    (
                                        "toggle-preview",
                                        CommandId::TogglePreview,
                                        IconName::PanelBottom,
                                        layout.preview_visible,
                                        "Toggle preview panel",
                                    ),
                                    (
                                        "toggle-inspector",
                                        CommandId::ToggleInspector,
                                        IconName::PanelRight,
                                        layout.inspector_visible,
                                        "Toggle inspector",
                                    ),
                                ]
                                .map(
                                    |(id, kind, icon, visible, tooltip)| {
                                        Button::new(id)
                                            .xsmall()
                                            .ghost()
                                            .icon(icon)
                                            .tooltip(tooltip)
                                            .toggled(visible)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.execute(kind, window, cx)
                                            }))
                                    },
                                ),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(self.dock.clone()))
            .child({
                let bar = StatusBar::new()
                    .h_6()
                    .px_3()
                    .text_xs()
                    .bg(cx.theme().status_bar)
                    .border_t_1()
                    .border_color(cx.theme().window_border)
                    .text_color(cx.theme().muted_foreground)
                    .left(Icon::new(IconName::Folder).xsmall())
                    .left(project_name.unwrap_or_else(|| "No project".into()))
                    .left(div().w_2())
                    .left(document_state)
                    .right(language)
                    .right("UTF-8")
                    .right(if cx.theme().is_dark() {
                        "One Dark"
                    } else {
                        "One Light"
                    });
                let bar = if let Some(warning) = self.warning.as_ref() {
                    bar.left(div().text_color(cx.theme().warning).child(warning.clone()))
                } else {
                    bar
                };
                div().id("status-bar").test_support().child(bar)
            })
    }
}

/// Scope a debounced write reports its outcome to.
#[derive(Clone, Copy)]
enum SaveScope {
    Global,
    Project,
}

/// Debounce one scoped preference write by 250 ms.
///
/// A drag replaces the pending task instead of writing every intermediate
/// width; the last snapshot always wins.
fn debounced_save<F>(cx: &mut Context<Shell>, scope: SaveScope, revision: u64, work: F) -> Task<()>
where
    F: FnOnce() -> std::io::Result<bool> + Send + 'static,
{
    let timer = cx
        .background_executor()
        .timer(std::time::Duration::from_millis(250));
    cx.spawn(async move |this, cx| {
        timer.await;
        let result = runtime::blocking(cx, work)
            .await
            .map_err(std::io::Error::other)
            .and_then(|result| result);
        let _ = this.update(cx, |this, cx| {
            // A stale result must not overwrite the status of a newer snapshot.
            let current = match scope {
                SaveScope::Global => this.revision == revision,
                SaveScope::Project => this
                    .project_scope
                    .as_ref()
                    .is_some_and(|scope| scope.revision == revision),
            };
            if current {
                this.warning = result
                    .err()
                    .map(|error| format!("Could not save preferences: {error}"));
            }
            // Test and status consumers wait for this notification, not just
            // for the file to appear.
            cx.notify();
        });
    })
}

/// The project's declared package name, used as the registry's move identity.
fn project_package_name(root: &std::path::Path) -> Option<String> {
    let path = root.join("package.json");
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions, base::Root,
        point, px, size, test::TestWindowExt,
    };

    fn mount(
        cx: &mut TestAppContext,
        settings: GlobalPreferencesV2,
    ) -> (WindowHandle<Root>, Entity<Shell>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            runtime::init(cx).unwrap();
            commands::bind_keys(cx);
            let (handle, view) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Point::default(),
                        size(px(1100.), px(720.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Shell::new(settings, None, None, window, cx)),
            )
            .unwrap();
            (handle.downcast::<Root>().unwrap(), view)
        })
    }

    #[gpui_kit::test]
    async fn opening_project_reveals_hidden_navigator(cx: &mut TestAppContext) {
        let settings = GlobalPreferencesV2 {
            layout: LayoutState {
                sidebar_visible: false,
                ..LayoutState::default()
            },
            ..GlobalPreferencesV2::default()
        };
        let (handle, view) = mount(cx, settings);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenProject, window, cx)
            })
        })
        .unwrap();
        assert!(view.read_with(cx, |view, cx| {
            view.dock.read(cx).is_dock_open(DockPlacement::Left)
        }));
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
    }
    #[gpui_kit::test]
    fn toolbar_and_native_collapse_share_live_visibility_and_restore_document_space(
        cx: &mut TestAppContext,
    ) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let initial = cx
            .update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let initial = window.find("document-panel").bounds();
                window.click("toggle-sidebar", cx);
                initial
            })
            .unwrap();
        // GPUI delivers emitted DockEvent effects after the window update returns.
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!view.read(cx).settings.layout.sidebar_visible);
            window.render_frame(cx);
            assert!(window.find("document-panel").bounds().size.width > initial.size.width);
            window.click("toggle-sidebar", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("document-panel").bounds(), initial);
            window.click("toggle-inspector", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!view.read(cx).settings.layout.inspector_visible);
            window.render_frame(cx);
            window.click("toggle-preview", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(view.read(cx).settings.layout.preview_visible);
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(window.find("preview-panel").visible());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn opening_a_project_attaches_the_preview_without_an_edit(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("node_modules/astro")).unwrap();
        std::fs::write(
            root.join("node_modules/astro/package.json"),
            r#"{"version":"7.4.4"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"astro":"7.4.4"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("astro.config.mjs"),
            "import { defineConfig } from 'astro/config';\nexport default defineConfig({});\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/content")).unwrap();
        std::fs::write(
            root.join("src/content.config.ts"),
            "import { defineCollection } from 'astro:content';\nimport { glob } from 'astro/loaders';\nconst blog = defineCollection({ loader: glob({ base: './src/content', pattern: '**/*.md' }) });\nexport const collections = { blog };\n",
        )
        .unwrap();
        std::fs::write(root.join("src/content/intro.md"), "# Intro\n").unwrap();

        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenProject, window, cx)
            })
        })
        .unwrap();
        assert!(cx.did_prompt_for_paths());
        let selected = root.to_path_buf();
        cx.simulate_path_prompt_response(move |_| Some(vec![selected.clone()]));
        let preview = view.read_with(cx, |shell, _| shell.preview.clone());
        cx.condition(&preview, |preview, _| preview.project().is_some())
            .await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(
                shell
                    .preview
                    .read(cx)
                    .project()
                    .map(|project| project.as_str()),
                Some("workspace-1"),
                "opening a project must attach the preview without an edit"
            );
            assert!(
                shell.project.read(cx).preview_context().is_some(),
                "the workspace must expose the preview context after open"
            );
        });
    }

    #[gpui_kit::test]
    fn preview_commands_are_wired_and_report_without_a_project(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::StartPreview, window, cx)
            });
            let message = view
                .read(cx)
                .preview
                .read(cx)
                .message()
                .unwrap_or_default()
                .to_owned();
            assert!(
                message.contains("Open an Astro project"),
                "start without a project must explain the requirement, was {message:?}"
            );
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenPreview, window, cx)
            });
            let message = view
                .read(cx)
                .preview
                .read(cx)
                .message()
                .unwrap_or_default()
                .to_owned();
            assert!(
                message.contains("Open a document"),
                "open without an active entry must explain the requirement, was {message:?}"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn splitter_drag_updates_geometry_without_repainting_the_shell(cx: &mut TestAppContext) {
        use std::rc::Rc;
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let notifications = Rc::new(std::cell::Cell::new(0usize));
        let count = notifications.clone();
        cx.update(|cx| {
            cx.observe(&view, move |_, _| count.set(count.get() + 1))
                .detach();
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(36.), start.y), cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert!(
                view.read(cx).settings.layout.sidebar_width > 250.,
                "the drag must still be captured"
            );
            assert_eq!(
                notifications.get(),
                0,
                "a width-only dock resize must not notify the shell"
            );
        });
        cx.simulate_window_resize(handle.into(), size(px(800.), px(600.)));
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|_| {
            assert_eq!(
                notifications.get(),
                0,
                "a window resize must not schedule an extra shell frame"
            );
        });
    }

    #[gpui_kit::test]
    fn native_drag_is_captured_and_preferences_restore_in_a_new_window(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(45.), start.y), cx);
            window.click("toggle-inspector", cx);
        })
        .unwrap();
        let settings = cx.update(|cx| {
            assert!(view.read(cx).settings.layout.sidebar_width > 250.);
            view.read(cx).settings.clone()
        });
        let (restored, _) = mount(cx, settings);
        cx.update_window(restored.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("navigator-panel").bounds().size.width > px(250.));
            assert!(
                window
                    .try_find("inspector-panel")
                    .is_none_or(|panel| !panel.visible())
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn shrinking_after_expanded_docks_preserves_document_and_panel_geometry(
        cx: &mut TestAppContext,
    ) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.simulate_window_resize(handle.into(), size(px(1800.), px(1100.)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            view.read(cx).dock.clone().update(cx, |dock, cx| {
                for placement in [DockPlacement::Left, DockPlacement::Right] {
                    dock.set_dock_size(placement, px(650.), window, cx);
                }
                dock.toggle_dock(DockPlacement::Bottom, window, cx);
                dock.set_dock_size(DockPlacement::Bottom, px(650.), window, cx);
            });
        })
        .unwrap();
        cx.simulate_window_resize(handle.into(), size(px(800.), px(600.)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let document = window.find("document-panel").bounds();
            assert!(document.size.width >= px(280.));
            assert!(document.size.height >= px(150.));
            for id in ["navigator-panel", "inspector-panel", "preview-panel"] {
                let bounds = window.find(id).bounds();
                assert!(bounds.left() >= px(0.) && bounds.right() <= px(800.));
                assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(600.));
            }
            assert!(view.read(cx).settings.layout.sidebar_width <= 240.);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn shell_panels_stay_inside_minimum_viewport_in_both_themes(cx: &mut TestAppContext) {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let settings = GlobalPreferencesV2 {
                theme: mode,
                ..GlobalPreferencesV2::default()
            };
            let (handle, _) = mount(cx, settings);
            cx.simulate_window_resize(handle.into(), size(px(800.), px(600.)));
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                assert!(window.find("title-bar").visible());
                assert!(window.find("status-bar").visible());
                assert!(window.find("document-header").visible());
                for id in ["navigator-panel", "document-panel", "inspector-panel"] {
                    let bounds = window.find(id).bounds();
                    assert!(bounds.size.width > px(50.) && bounds.size.height > px(100.));
                    assert!(bounds.left() >= px(0.) && bounds.right() <= px(800.));
                    assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(600.));
                }
            })
            .unwrap();
        }
    }
    #[gpui_kit::test]
    fn palette_filters_confirms_original_command_and_restores_trigger_focus(
        cx: &mut TestAppContext,
    ) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("command-palette", cx);
            assert!(window.has_active_dialog(cx));
            window.input("Dark theme", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(view.read(cx).palette.read(cx).matched_count(), 1);
            window.press("enter", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(view.read(cx).settings.theme, ThemeMode::Dark);
            assert!(!window.has_active_dialog(cx));
            window.render_frame(cx);
            assert_eq!(window.find("command-palette").focused(), Some(true));
            window.press("enter", cx);
            window.input("no-command-matches-this", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(view.read(cx).palette.read(cx).matched_count(), 0);
            window.press("escape", cx);
            window.press("escape", cx);
            assert!(!window.has_active_dialog(cx));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn shortcuts_work_from_document_focus_and_palette_cancel_returns_there(
        cx: &mut TestAppContext,
    ) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx).document.read(cx).editor.clone().unwrap();
            window.render_frame(cx);
            window.click(("input", state.entity_id()), cx);
            window.press(
                crate::commands::CommandId::ToggleSidebar
                    .shortcut()
                    .unwrap(),
                cx,
            );
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            use gpui_kit::{Focusable, component::WindowExt};
            assert!(!view.read(cx).settings.layout.sidebar_visible);
            window.press(
                crate::commands::CommandId::OpenPalette.shortcut().unwrap(),
                cx,
            );
            assert!(window.has_active_dialog(cx));
            window.press("escape", cx);
            assert!(!window.has_active_dialog(cx));
            let editor = view.read(cx).document.read(cx).editor.clone().unwrap();
            assert!(editor.read(cx).focus_handle(cx).is_focused(window));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn file_picker_opens_readonly_source_and_cancel_preserves_the_buffer(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("中文.mdx");
        let source = "# 日本語\n<Example>{42}</Example> 🦀\n";
        std::fs::write(&path, source).unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::commands::CommandId::OpenFile.shortcut().unwrap(), cx);
        })
        .unwrap();
        assert!(cx.did_prompt_for_paths());
        let selected = path.clone();
        cx.simulate_path_prompt_response(move |options| {
            assert!(options.files && !options.directories && !options.multiple);
            Some(vec![selected])
        });
        cx.condition(&view, |shell, _| !shell.opening).await;
        cx.update_window(handle.into(), |_, window, cx| {
            let editor = view.read(cx).document.read(cx).editor.clone().unwrap();
            assert_eq!(editor.read(cx).value(), source);
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("must not replace source", cx);
            assert_eq!(editor.read(cx).value(), source);
            window.press(crate::commands::CommandId::OpenFile.shortcut().unwrap(), cx);
        })
        .unwrap();
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(!view.read(cx).opening);
            assert_eq!(
                view.read(cx)
                    .document
                    .read(cx)
                    .editor
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value(),
                source
            );
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    }

    #[gpui_kit::test]
    async fn settings_burst_does_not_write_intermediate_drag_positions(cx: &mut TestAppContext) {
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(path.clone());
        let (_, view) = mount(cx, settings);
        view.update(cx, |shell, cx| {
            shell.store = Some(Arc::new(store));
            shell.settings.layout.sidebar_width = 230.;
            shell.persist_global(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        assert!(
            !path.exists(),
            "dragging must not sync every intermediate width"
        );
        view.update(cx, |shell, cx| {
            shell.settings.layout.sidebar_width = 275.;
            shell.persist_global(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        assert!(!path.exists(), "a new change must restart the debounce");
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.executor().allow_parking();
        cx.condition(&view, |_, _| path.exists()).await;
        let (_, restored, warning) = PreferencesStore::load(path);
        assert!(warning.is_none());
        assert_eq!(restored.layout.sidebar_width, 275.);
    }

    /// Minimal Astro project with one `blog` collection holding two entries.
    fn project_fixture(package_name: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("node_modules/astro")).unwrap();
        std::fs::write(
            root.join("node_modules/astro/package.json"),
            r#"{"version":"7.4.4"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            format!(r#"{{"name":"{package_name}","dependencies":{{"astro":"7.4.4"}}}}"#),
        )
        .unwrap();
        std::fs::write(
            root.join("astro.config.mjs"),
            "import { defineConfig } from 'astro/config';\nexport default defineConfig({});\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/content")).unwrap();
        std::fs::write(
            root.join("src/content.config.ts"),
            "import { defineCollection } from 'astro:content';\nimport { glob } from 'astro/loaders';\nconst blog = defineCollection({ loader: glob({ base: './src/content', pattern: '**/*.md' }) });\nexport const collections = { blog };\n",
        )
        .unwrap();
        std::fs::write(root.join("src/content/first.md"), "# First\n").unwrap();
        std::fs::write(root.join("src/content/second.md"), "# Second\n").unwrap();
        directory
    }

    /// Persist one project scope before the app opens it, as a restart would find it.
    fn seed_project(
        store: &PreferencesStore,
        root: &std::path::Path,
        preferences: impl FnOnce(&mut ProjectPreferencesV1),
    ) -> String {
        let open = store.record_project_open(root, Some("site"), 1).unwrap();
        let (mut stored, revision, _) = store.project(&open.id).unwrap();
        preferences(&mut stored);
        assert!(store.save_project(revision + 1, &stored).unwrap());
        open.id
    }

    fn open_project(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<Shell>,
        root: std::path::PathBuf,
    ) {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenProject, window, cx)
            })
        })
        .unwrap();
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(move |_| Some(vec![root.clone()]));
    }

    fn project_file(preferences_root: &std::path::Path, id: &str) -> serde_json::Value {
        let path = preferences_root.join("projects").join(format!("{id}.json"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[gpui_kit::test]
    async fn project_layout_overrides_the_global_layout(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        store.save_global(1, &settings).unwrap();
        let project = project_fixture("layout-site");
        let saved = LayoutState {
            sidebar_visible: true,
            inspector_visible: false,
            preview_visible: true,
            sidebar_width: 200.,
            inspector_width: 320.,
            preview_height: 220.,
        };
        {
            let saved = saved.clone();
            seed_project(&store, project.path(), |preferences| {
                preferences.layout = Some(saved);
            });
        }
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));

        open_project(cx, handle, &view, project.path().to_path_buf());
        let canonical = std::fs::canonicalize(project.path()).unwrap();
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == canonical)
        })
        .await;

        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.settings.layout, saved);
            let dock = shell.dock.read(cx);
            assert!(dock.is_dock_open(DockPlacement::Left));
            assert!(!dock.is_dock_open(DockPlacement::Right));
            assert!(dock.is_dock_open(DockPlacement::Bottom));
            assert_eq!(dock.dock_size(DockPlacement::Left), Some(px(200.)));
            assert_eq!(dock.dock_size(DockPlacement::Right), Some(px(320.)));
            assert_eq!(dock.dock_size(DockPlacement::Bottom), Some(px(220.)));
            assert_eq!(
                shell.global_layout,
                LayoutState::default(),
                "restoring a project layout must not rewrite the global layout"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("navigator-panel").bounds().size.width, px(200.));
            assert!(
                window
                    .try_find("inspector-panel")
                    .is_none_or(|panel| !panel.visible())
            );
            assert!(window.find("preview-panel").visible());
        })
        .unwrap();

        // The open is recorded under the stable registry identity, using the
        // package name from package.json — never the runtime workspace id.
        let (fresh, _, warning) = PreferencesStore::load(global_path);
        assert!(warning.is_none());
        let (registry, _, warning) = fresh.registry();
        assert!(warning.is_none());
        let path = canonical.to_string_lossy().into_owned();
        let record = registry.get_by_path(&path).expect("project registered");
        assert_eq!(record.package_name.as_deref(), Some("layout-site"));
        let scope_id = view.read_with(cx, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .unwrap()
                .preferences
                .project_id
                .clone()
        });
        assert_eq!(record.id, scope_id);
        assert!(!scope_id.starts_with("workspace-"));
    }

    #[gpui_kit::test]
    async fn switching_projects_switches_the_saved_layouts(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let first = project_fixture("first-site");
        let second = project_fixture("second-site");
        let first_layout = LayoutState {
            sidebar_visible: false,
            inspector_visible: true,
            preview_visible: false,
            sidebar_width: 190.,
            inspector_width: 280.,
            preview_height: 160.,
        };
        let second_layout = LayoutState {
            sidebar_visible: true,
            inspector_visible: false,
            preview_visible: true,
            sidebar_width: 230.,
            inspector_width: 320.,
            preview_height: 240.,
        };
        {
            let layout = first_layout.clone();
            seed_project(&store, first.path(), |preferences| {
                preferences.layout = Some(layout);
            });
        }
        {
            let layout = second_layout.clone();
            seed_project(&store, second.path(), |preferences| {
                preferences.layout = Some(layout);
            });
        }
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let first_root = std::fs::canonicalize(first.path()).unwrap();
        let second_root = std::fs::canonicalize(second.path()).unwrap();

        open_project(cx, handle, &view, first.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == first_root)
        })
        .await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.settings.layout, first_layout);
            assert!(!shell.dock.read(cx).is_dock_open(DockPlacement::Left));
        });

        open_project(cx, handle, &view, second.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == second_root)
        })
        .await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.settings.layout, second_layout);
            assert!(shell.dock.read(cx).is_dock_open(DockPlacement::Left));
            assert!(!shell.dock.read(cx).is_dock_open(DockPlacement::Right));
            assert!(shell.dock.read(cx).is_dock_open(DockPlacement::Bottom));
            assert_eq!(
                shell.dock.read(cx).dock_size(DockPlacement::Bottom),
                Some(px(240.))
            );
        });

        open_project(cx, handle, &view, first.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == first_root)
        })
        .await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.settings.layout, first_layout);
            assert!(!shell.dock.read(cx).is_dock_open(DockPlacement::Left));
        });
    }

    #[gpui_kit::test]
    async fn moved_project_rebinds_and_keeps_its_layout(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let project = project_fixture("moving-site");
        let saved = LayoutState {
            sidebar_visible: false,
            inspector_visible: true,
            preview_visible: true,
            sidebar_width: 210.,
            inspector_width: 300.,
            preview_height: 180.,
        };
        let id = {
            let layout = saved.clone();
            seed_project(&store, project.path(), |preferences| {
                preferences.layout = Some(layout);
            })
        };
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let original_root = std::fs::canonicalize(project.path()).unwrap();

        open_project(cx, handle, &view, project.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == original_root)
        })
        .await;

        // Move the project directory, then reopen it at the new location.
        let moved = workspace.path().join("moved");
        std::fs::rename(project.path(), &moved).unwrap();
        let moved_root = std::fs::canonicalize(&moved).unwrap();
        open_project(cx, handle, &view, moved.clone());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == moved_root)
        })
        .await;

        view.read_with(cx, |shell, cx| {
            let scope = shell.project_scope.as_ref().unwrap();
            assert_eq!(
                scope.preferences.project_id, id,
                "a moved project keeps its registry identity and preferences"
            );
            assert_eq!(shell.settings.layout, saved);
            assert!(!shell.dock.read(cx).is_dock_open(DockPlacement::Left));
            assert!(shell.dock.read(cx).is_dock_open(DockPlacement::Bottom));
        });
    }

    #[gpui_kit::test]
    async fn project_width_change_persists_to_the_project_file_only(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        store.save_global(1, &settings).unwrap();
        let project = project_fixture("width-site");
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let canonical = std::fs::canonicalize(project.path()).unwrap();
        open_project(cx, handle, &view, project.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == canonical)
        })
        .await;
        view.read_with(cx, |shell, _| {
            assert_eq!(
                shell.settings.layout, shell.global_layout,
                "a project without a saved layout inherits the global layout"
            );
            assert!(
                shell
                    .project_scope
                    .as_ref()
                    .unwrap()
                    .preferences
                    .layout
                    .is_none()
            );
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(36.), start.y), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.condition(&view, |shell, _| {
            let Some(scope) = shell.project_scope.as_ref() else {
                return false;
            };
            let path = preferences_root
                .join("projects")
                .join(format!("{}.json", scope.preferences.project_id));
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["layout"]["sidebar_width"].as_f64())
                .is_some_and(|width| width > 250.)
        })
        .await;

        let id = view.read_with(cx, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .unwrap()
                .preferences
                .project_id
                .clone()
        });
        let project_preferences = project_file(&preferences_root, &id);
        assert_eq!(
            project_preferences["layout"]["sidebar_visible"], true,
            "the project file must record the captured layout"
        );
        let (_, global, warning) = PreferencesStore::load(global_path);
        assert!(warning.is_none());
        assert_eq!(
            global.layout.sidebar_width, 240.,
            "a project width must never be written to the global file"
        );
    }

    #[gpui_kit::test]
    async fn preview_height_change_persists_to_the_project_file(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let project = project_fixture("preview-site");
        {
            let (store, _, _) = PreferencesStore::load(global_path.clone());
            seed_project(&store, project.path(), |preferences| {
                preferences.layout = Some(LayoutState {
                    preview_visible: true,
                    ..LayoutState::default()
                });
            });
        }
        let (store, settings, _) = PreferencesStore::load(global_path);
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        open_project(cx, handle, &view, project.path().to_path_buf());
        let canonical = std::fs::canonicalize(project.path()).unwrap();
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == canonical)
        })
        .await;

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-bottom").bounds().center();
            window.drag(start, point(start.x, start.y - px(48.)), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));

        let id = view.read_with(cx, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .unwrap()
                .preferences
                .project_id
                .clone()
        });
        cx.condition(&view, |shell, _| {
            let Some(scope) = shell.project_scope.as_ref() else {
                return false;
            };
            let path = preferences_root
                .join("projects")
                .join(format!("{}.json", scope.preferences.project_id));
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["layout"]["preview_height"].as_f64())
                .is_some_and(|height| height > 190.)
        })
        .await;
        let preferences = project_file(&preferences_root, &id);
        assert_eq!(preferences["layout"]["preview_visible"], true);
    }

    #[gpui_kit::test]
    async fn pending_project_layout_is_flushed_before_switching(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let first = project_fixture("pending-first");
        let second = project_fixture("pending-second");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let first_root = std::fs::canonicalize(first.path()).unwrap();
        let second_root = std::fs::canonicalize(second.path()).unwrap();

        open_project(cx, handle, &view, first.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == first_root)
        })
        .await;
        // A layout change whose debounced write has not run yet.
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(36.), start.y), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let first_id = view.read_with(cx, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .unwrap()
                .preferences
                .project_id
                .clone()
        });

        // Switch before the debounce fires, then change the second project so a
        // single shared pending task would replace the first project's write.
        open_project(cx, handle, &view, second.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == second_root)
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(60.), start.y), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));

        let path = preferences_root
            .join("projects")
            .join(format!("{first_id}.json"));
        cx.condition(&view, move |_, _| {
            std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["layout"]["sidebar_width"].as_f64())
                .is_some_and(|width| width > 250.)
        })
        .await;
    }

    #[gpui_kit::test]
    async fn last_entry_reopens_on_open_and_is_ignored_when_missing(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let project = project_fixture("entry-site");
        let missing = project_fixture("missing-site");
        {
            let (store, _, _) = PreferencesStore::load(global_path.clone());
            seed_project(&store, project.path(), |preferences| {
                preferences.last_entry = Some("src/content/second.md".into());
            });
            seed_project(&store, missing.path(), |preferences| {
                preferences.last_entry = Some("src/content/gone.md".into());
            });
        }
        // Reopen the store from disk: this is the restart the acceptance covers.
        let (store, settings, _) = PreferencesStore::load(global_path);
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));

        open_project(cx, handle, &view, project.path().to_path_buf());
        let canonical = std::fs::canonicalize(project.path()).unwrap();
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == canonical)
        })
        .await;
        let project_handle = view.read_with(cx, |shell, _| shell.project.clone());
        cx.condition(&project_handle, |project, _| project.open_tabs().len() == 1)
            .await;
        view.read_with(cx, |shell, cx| {
            let project = shell.project.read(cx);
            assert_eq!(
                project.active_relative_source().as_deref(),
                Some("src/content/second.md")
            );
            assert_eq!(
                project.active_path(),
                Some(canonical.join("src/content/second.md"))
            );
        });

        // Opening a different entry records it as this project's last entry.
        cx.update_window(handle.into(), |_, window, cx| {
            project_handle.update(cx, |project, cx| {
                project.open_first_entry_for_tests(window, cx)
            });
        })
        .unwrap();
        cx.condition(&project_handle, |project, _| project.open_tabs().len() == 2)
            .await;
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.condition(&view, |shell, _| {
            let Some(scope) = shell.project_scope.as_ref() else {
                return false;
            };
            let path = preferences_root
                .join("projects")
                .join(format!("{}.json", scope.preferences.project_id));
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["last_entry"].as_str().map(str::to_owned))
                .as_deref()
                == Some("src/content/first.md")
        })
        .await;

        open_project(cx, handle, &view, missing.path().to_path_buf());
        let missing_root = std::fs::canonicalize(missing.path()).unwrap();
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == missing_root)
        })
        .await;
        cx.run_until_parked();
        view.read_with(cx, |shell, cx| {
            let project = shell.project.read(cx);
            assert!(project.open_tabs().is_empty());
            assert!(project.active_path().is_none());
        });
    }

    #[gpui_kit::test]
    async fn layout_without_a_project_still_persists_globally(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(36.), start.y), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.condition(&view, |shell, _| {
            let persisted = std::fs::read(&global_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["layout"]["sidebar_width"].as_f64());
            shell.global_layout.sidebar_width > 250. && persisted.is_some_and(|w| w > 250.)
        })
        .await;
        let (_, global, warning) = PreferencesStore::load(global_path);
        assert!(warning.is_none());
        assert!(
            global.layout.sidebar_width > 250.,
            "without a project the live layout keeps persisting globally"
        );
        assert!(!workspace.path().join("preferences/projects").exists());
    }

    #[gpui_kit::test]
    async fn entry_commands_share_availability_across_toolbar_and_palette(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let project = project_fixture("availability-site");
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("toolbar-new-entry").disabled(),
                Some(true),
                "no project means no new entry"
            );
        })
        .unwrap();
        open_project(cx, handle, &view, project.path().to_path_buf());
        let project_handle = view.read_with(cx, |shell, _| shell.project.clone());
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |shell, cx| !shell.project.read(cx).is_loading()) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(view.read_with(cx, |shell, cx| { !shell.project.read(cx).is_loading() }));

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_ne!(window.find("toolbar-new-entry").disabled(), Some(true));
            assert_ne!(window.find("entry-new").disabled(), Some(true));
            assert_eq!(
                window.find("entry-rename").disabled(),
                Some(true),
                "row commands need an active entry"
            );
            window.click("command-palette", cx);
            window.input("Duplicate file", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(
                view.read(cx).palette.read(cx).matched_count(),
                0,
                "the palette hides entry commands without an active entry"
            );
            window.press("escape", cx);
            window.press("escape", cx);
            assert!(!window.has_active_dialog(cx));
        })
        .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            project_handle.update(cx, |project, cx| {
                project.open_first_entry_for_tests(window, cx)
            });
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |shell, cx| {
                shell.project.read(cx).open_tabs().len() == 1
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            for id in ["entry-rename", "entry-delete", "entry-reveal", "copy-path"] {
                assert_ne!(window.find(id).disabled(), Some(true), "{id}");
            }
            window.click("command-palette", cx);
            assert!(window.has_active_dialog(cx));
            window.input("Duplicate file", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(view.read(cx).palette.read(cx).matched_count(), 1);
            window.press("escape", cx);
            window.press("escape", cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    #[ignore = "bounded window-resize measurements; run with --ignored --nocapture"]
    async fn window_resize_workload(cx: &mut TestAppContext) {
        use std::time::Instant;
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("node_modules/astro")).unwrap();
        std::fs::write(
            root.join("node_modules/astro/package.json"),
            r#"{"version":"7.4.4"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"astro":"7.4.4"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("astro.config.mjs"),
            "import { defineConfig } from 'astro/config';\nexport default defineConfig({});\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/content")).unwrap();
        std::fs::write(
            root.join("src/content.config.ts"),
            "import { defineCollection } from 'astro:content';\nimport { glob } from 'astro/loaders';\nconst blog = defineCollection({ loader: glob({ base: './src/content', pattern: '**/*.md' }) });\nexport const collections = { blog };\n",
        )
        .unwrap();
        let paragraph = "# 中文 🦀\nA multilingual paragraph with **emphasis**, `inline code`, 日本語, and enough ordinary words to represent a real article.\n";
        std::fs::write(root.join("src/content/intro.md"), paragraph.repeat(8000)).unwrap();

        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenProject, window, cx)
            })
        })
        .unwrap();
        let selected = root.to_path_buf();
        cx.simulate_path_prompt_response(move |_| Some(vec![selected.clone()]));
        let project = view.read_with(cx, |view, _| view.project.clone());
        cx.condition(&project, |project, _| {
            !project.is_loading() && !project.is_refreshing()
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.project.update(cx, |project, cx| {
                    project.open_first_entry_for_tests(window, cx)
                })
            })
        })
        .unwrap();
        cx.condition(&project, |project, _| project.open_tabs().len() == 1)
            .await;
        // Install a wide schema so the inspector renders a realistic dense form.
        let fields = (0..80)
            .map(|index| astrolith_astro::schema::Field {
                name: format!("field{index:02}"),
                required: index % 3 == 0,
                default: None,
                kind: astrolith_astro::schema::FieldKind::String { format: None },
            })
            .collect();
        let schema = astrolith_astro::schema::CollectionSchema {
            dialect: astrolith_astro::schema::SchemaDialect::Draft7,
            fields,
            diagnostics: Vec::new(),
        };
        cx.update_window(handle.into(), |_, window, cx| {
            project.update(cx, |project, cx| {
                project.set_collection_schemas(vec![("blog".to_owned(), schema)], cx)
            });
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();

        let mut times = Vec::new();
        for index in 0..90 {
            let width = 820. + (index % 30) as f32 * 22.;
            let height = 620. + (index % 20) as f32 * 16.;
            cx.simulate_window_resize(handle.into(), size(px(width), px(height)));
            let elapsed = cx
                .update_window(handle.into(), |_, window, cx| {
                    let start = Instant::now();
                    window.render_frame(cx);
                    start.elapsed().as_secs_f64() * 1000.
                })
                .unwrap();
            times.push(elapsed);
        }
        let average = times.iter().sum::<f64>() / times.len() as f64;
        let worst = times.iter().copied().fold(0.0_f64, f64::max);
        let mut sorted = times.clone();
        sorted.sort_by(|left, right| left.total_cmp(right));
        let p95 = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
        println!(
            "SHELL_RESIZE frames={} avg_ms={average:.3} p95_ms={p95:.3} worst_ms={worst:.3} bytes={}",
            times.len(),
            paragraph.len() * 8000
        );
    }

    #[gpui_kit::test]
    #[ignore = "bounded shell measurements; run mise run test:shell-perf"]
    fn readonly_file_open_workload(cx: &mut TestAppContext) {
        use std::time::Instant;
        let directory = tempfile::tempdir().unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        for (bytes, long_line) in [
            (1024, false),
            (100 * 1024, false),
            (1024 * 1024, false),
            (100 * 1024, true),
        ] {
            let unit = if long_line {
                "中文 🦀 **hello** "
            } else {
                "# 中文 🦀\nA multilingual paragraph with **emphasis**, `inline code`, 日本語, and enough ordinary words to represent real article content.\n"
            };
            let text = unit.repeat(bytes / unit.len() + 1);
            let path = directory.path().join("measurement.md");
            std::fs::write(&path, &text).unwrap();
            let start = Instant::now();
            let source = read_document(path).unwrap();
            let read_ms = start.elapsed().as_secs_f64() * 1000.;
            cx.update_window(handle.into(), |_, window, cx| {
                let start = Instant::now();
                view.update(cx, |shell, cx| shell.show_document(source, window, cx));
                let install_ms = start.elapsed().as_secs_f64() * 1000.;
                let start = Instant::now();
                window.render_frame(cx);
                let frame_ms = start.elapsed().as_secs_f64() * 1000.;
                println!("SHELL_OPEN bytes={} lines={} long_line={long_line} read_ms={read_ms:.3} install_ms={install_ms:.3} frame_ms={frame_ms:.3}", text.len(), text.lines().count());
                let editor = view.read(cx).document.read(cx).editor.as_ref().unwrap();
                assert_eq!(editor.read(cx).value(), text);
                assert!(editor.read(cx).focus_handle(cx).is_focused(window));
            }).unwrap();
        }
    }
}
