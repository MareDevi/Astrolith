use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(debug_assertions)]
mod perf;

use crate::{
    commands::{self, COMMANDS, CollectionItem, CommandAvailability, CommandId},
    drag_drop,
    editor::{
        component_builder::{ComponentBuilder, ComponentBuilderEvent},
        content_finder::{self, FinderEntry},
        copyedit::CopyeditConfig,
        header::{DocumentHeader, LibraryRequested},
        rendered::RenderedDocument,
        session::SessionChanged,
        writer_chrome::{ChromeVisibility, TypedCharacters},
    },
    file::{DocumentSource, read_document},
    frontmatter::{FrontmatterDocument, FrontmatterEdit, FrontmatterForm},
    help::{self, HelpSurface},
    layout::{
        panels::WorkspacePanel,
        state::{LayoutState, PanelKind},
    },
    preferences::{
        CollectionEntry, PreferenceEdits, PreferencesContext, PreferencesEvent, PreferencesSheet,
    },
    preview::{
        PreviewPanel, PreviewStateChanged,
        embedded::{EmbeddedPreview, EmbeddedPreviewEvent},
    },
    project_discovery::{
        FrontmatterKey, HistoryChangedEvent, ProjectOpened, ProjectWorkspace,
        ReadonlyFileRequested, now_secs, scan_project_astro_sources,
    },
    recent_projects::{self, RecentEntry},
    runtime,
    settings::{
        GlobalPreferencesV2, PreferencesStore, ProjectPreferencesV1, ProjectRecord, WindowGeometry,
        resolve, settings_path,
    },
    single_instance,
    terminal::TerminalPanel,
    theme::{self, ThemeMode},
    update,
    url_scheme::{self, StartupRequest, StartupTarget},
};
use astrolith_core::preferences::{ModeLayouts, PreviewPresentation, WorkspaceMode};
use astrolith_document::{DocumentKind, DocumentRevision};
use astrolith_preview::{HostEnvironment, PreviewCapability};
use gpui_kit::assets::IconName;
use gpui_kit::base::{Presence, Sheet, Transition};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Selectable, Sizable, StyledExt, TitleBar, WindowExt,
    button::{Button, ButtonVariants},
    command::{Command, CommandGroup, CommandItem, CommandState},
    dock::{BasePanelView, DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, panel_handle},
    kbd::Kbd,
    menu::{DropdownMenu, PopupMenuItem},
    scroll::ScrollableElement as _,
};
use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext, Bounds, Context, Entity, ExternalPaths,
    FocusHandle, Focusable, IntoElement, PathPromptOptions, Render, ScrollHandle, SharedString,
    Subscription, Task, TestSupportExt, WeakFocusHandle, Window, WindowBounds, WindowOptions,
    deferred, div, point, prelude::*, px, rems, size,
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
    let arguments: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let startup = url_scheme::parse_startup(std::env::args_os().skip(1));
    let socket = single_instance::socket_path();
    if single_instance::forward(&socket, &arguments) {
        tracing::info!("forwarded this launch to the running Astrolith window");
        // `App::quit` only stops an already running loop; before the first
        // window exists it would leave this process waiting indefinitely.
        std::process::exit(0);
    }
    tracing::info!(
        gpui_fast = gpui_kit::GPUI_FAST,
        "GPUI backend selected for this build"
    );
    if let Err(error) = runtime::init(cx) {
        tracing::error!(%error, "could not start background services");
        eprintln!("Could not start background services: {error}");
        cx.quit();
        return;
    }
    let listener = match single_instance::bind(&socket) {
        Ok(listener) => Some(listener),
        Err(error) => {
            tracing::warn!(%error, "single instance is unavailable; continuing");
            None
        }
    };
    commands::bind_keys(cx);
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
            commands::install_menus(
                cx,
                &settings.copyedit,
                CommandAvailability::default(),
                &[],
                &[],
            );
            let window_bounds = settings.window.and_then(restored_bounds);
            let opened = gpui_kit::open_window(
                WindowOptions {
                    window_bounds,
                    window_min_size: Some(size(px(1000.), px(700.))),
                    ..TitleBar::window_options()
                },
                cx,
                |window, cx| {
                    window.set_window_title("Astrolith");
                    cx.new(|cx| Shell::new(settings, store, warning, window, cx))
                },
            );
            match opened {
                Ok((window_handle, shell)) => {
                    tracing::info!("main window opened");
                    let _ = window_handle.update(cx, |_, window, cx| {
                        shell.update(cx, |shell, cx| {
                            shell.begin(startup, listener, window, cx)
                        })
                    });
                }
                Err(error) => {
                    tracing::error!(error = %format_args!("{error:#}"), "could not open the main window");
                    eprintln!("Could not open Astrolith: {error:#}");
                    cx.quit();
                }
            }
        });
    })
    .detach();
}

/// Restore stored bounds without ever opening below the enforced minimum.
fn restored_bounds(geometry: WindowGeometry) -> Option<WindowBounds> {
    if geometry.width < 1000. || geometry.height < 700. {
        return None;
    }
    let bounds = Bounds::new(
        point(px(geometry.x), px(geometry.y)),
        size(px(geometry.width), px(geometry.height)),
    );
    Some(if geometry.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    })
}

/// One actionable palette row.
#[derive(Clone, Debug)]
enum PaletteAction {
    Command(CommandId),
    RecentProject(String),
    Collection(String),
}

#[derive(Clone, Debug)]
struct PaletteEntry {
    label: SharedString,
    shortcut: SharedString,
    icon: IconName,
    action: PaletteAction,
}

/// Identity of the document last installed in the reading pane.
///
/// [`RenderedDocument::set_document`] already ignores identical text, but only
/// after the caller copied the whole visible body to reach that check.
/// Comparing this key first keeps an unchanged session out of the copy.
#[derive(PartialEq, Eq)]
struct RenderedSyncKey {
    root: Option<PathBuf>,
    path: Option<PathBuf>,
    kind: Option<DocumentKind>,
    revision: Option<DocumentRevision>,
}

/// The settings and theme inputs that decide the resolved reading typography.
///
/// Resolution scans the installed font families and allocates the family
/// string, so it happens only when one of these inputs changes.
struct ProseStyleInputs {
    family: String,
    font_size: f32,
    line_height: f32,
    page_width: f32,
    ui_family: SharedString,
}

impl ProseStyleInputs {
    fn matches(
        &self,
        family: &str,
        font_size: f32,
        line_height: f32,
        page_width: f32,
        ui_family: &SharedString,
    ) -> bool {
        self.family == family
            && self.font_size == font_size
            && self.line_height == line_height
            && self.page_width == page_width
            && &self.ui_family == ui_family
    }
}

pub(crate) struct Shell {
    dock: Entity<DockArea>,
    document: Entity<WorkspacePanel>,
    document_header: Entity<crate::editor::header::DocumentHeader>,
    project: Entity<ProjectWorkspace>,
    navigator: Entity<WorkspacePanel>,
    frontmatter: Entity<FrontmatterForm>,
    frontmatter_key: Option<FrontmatterKey>,
    /// Floating frontmatter inspector visibility; never persisted.
    inspector_open: bool,
    inspector_focus: FocusHandle,
    inspector_return_focus: Option<WeakFocusHandle>,
    /// Writing's library is a transient left-side sheet over the document.
    library_open: bool,
    library_focus: FocusHandle,
    library_return_focus: Option<WeakFocusHandle>,
    preview: Entity<PreviewPanel>,
    /// Rendered Markdown/MDX reading pane shown in Preview mode.
    rendered: Entity<RenderedDocument>,
    rendered_panel: Entity<WorkspacePanel>,
    /// The reading-view panel; kept here because the bottom dock is removed
    /// entirely while the view is hidden.
    preview_panel: Entity<WorkspacePanel>,
    /// Retained embedded WebView surface for the Astro preview dock.
    embedded: Entity<EmbeddedPreview>,
    embedded_panel: Entity<WorkspacePanel>,
    /// The centre split currently contains the embedded preview.
    embedded_center_active: bool,
    /// Whether this build/host may host the embedded surface.
    preview_capability: PreviewCapability,
    /// Why the embedded surface is unavailable before any attempt.
    preview_unavailable_reason: Option<String>,
    /// Why the embedded surface failed in this session, if it did.
    embedded_failure: Option<String>,
    /// Project terminal, retained while its dock is hidden.
    terminal: Entity<TerminalPanel>,
    terminal_panel: Entity<WorkspacePanel>,
    /// Horizontal position of the Build buffer tabs.
    build_tabs: ScrollHandle,
    palette: Entity<CommandState>,
    palette_focus: FocusHandle,
    finder: Entity<CommandState>,
    finder_hits: Arc<Mutex<Vec<FinderEntry>>>,
    builder: Entity<ComponentBuilder>,
    preferences_sheet: Entity<PreferencesSheet>,
    /// Reset All is deleting files; every pending and quit-time write is suppressed.
    resetting: Arc<AtomicBool>,
    opening: bool,
    opening_task: Option<Task<()>>,
    source_picker_task: Option<Task<()>>,
    file_status: String,
    preview_sync_key: Option<(u64, Option<PathBuf>, astrolith_preview::RouteMapping)>,
    last_viewport: gpui_kit::Size<gpui_kit::Pixels>,
    _skin: Rc<DockSkin>,
    /// Global preferences; `settings.layouts` is the global layout authority and
    /// the only layout source when no project is open.
    settings: GlobalPreferencesV2,
    /// Active workspace mode; state and services are independent of it.
    mode: WorkspaceMode,
    /// Project scope currently open, if any; absorbs layout and last-entry writes.
    project_scope: Option<ProjectScope>,
    /// A project is opening: its scope is not installed yet, so layout writes
    /// are held back until the registry lookup finishes.
    scope_loading: Option<PathBuf>,
    /// The dock is being set programmatically; its intermediate events are not
    /// user changes and must not be persisted.
    applying_layout: bool,
    /// A dock change whose layout event must not be persisted; revealing the
    /// navigator before the open-project prompt is transient.
    ignoring_layout_event: bool,
    store: Option<Arc<PreferencesStore>>,
    latest: LatestSettings,
    revision: u64,
    settings_task: Option<Task<()>>,
    project_task: Option<Task<()>>,
    scope_task: Option<Task<()>>,
    warning: Option<String>,
    /// Compile-time update capability and the latest check result.
    updates: update::CapabilityState,
    update_status: update::Status,
    update_task: Option<Task<()>>,
    /// Registered projects ordered by most recent open.
    recent_records: Vec<ProjectRecord>,
    /// Labels generated once for menus and the palette.
    recent: Vec<RecentEntry>,
    /// Entry waiting for the project index that proves membership.
    pending_entry: Option<PathBuf>,
    /// Last menu model installed, so render does not rebuild unchanged menus.
    menu_signature: Option<(CommandAvailability, Vec<RecentEntry>, Vec<CollectionItem>)>,
    /// Set by the change handlers that can change menu inputs (mode, project
    /// notify, session change, preferences, recent list, collections, dock).
    /// The next frame installs the menus once instead of every frame.
    menus_dirty: bool,
    /// The editor focus/selection pair the installed menu model was built from.
    menu_editor_key: Option<(bool, bool)>,
    /// Menu recomputations and installs, for the per-frame cost test.
    #[cfg(test)]
    menu_refreshes: usize,
    #[cfg(test)]
    menu_installs: usize,
    /// Single-instance listener task, kept alive for the window's lifetime.
    instance_task: Option<Task<()>>,
    focus: FocusHandle,
    /// Writer chrome auto-hide state.
    chrome: ChromeVisibility,
    /// Typed characters per active document, for the chrome auto-hide policy.
    typed: TypedCharacters,
    /// Document identity last pushed into the reading pane.
    rendered_sync_key: Option<RenderedSyncKey>,
    /// Resolved reading typography and the inputs that produced it.
    prose_style: Option<(ProseStyleInputs, crate::editor::writing_modes::ProseStyle)>,
    /// Number of typography resolutions, for the per-frame cost test.
    #[cfg(test)]
    prose_style_resolutions: usize,
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
            .layouts
            .clamp(f32::from(window.viewport_size().width));
        let mode = settings.default_mode;
        let layout = settings.layouts.get(mode).clone();
        theme::apply(&settings, window.appearance(), cx);
        let navigator = cx.new(|cx| {
            WorkspacePanel::new(
                "navigator-panel",
                "Project",
                "No project open\n\nOpen a project to browse its library.",
                window,
                cx,
            )
            .with_surface()
        });
        let document = cx.new(|cx| {
            WorkspacePanel::new(
                "document-panel",
                "Welcome",
                document_empty_copy(mode, false),
                window,
                cx,
            )
        });
        let project = cx.new(|cx| ProjectWorkspace::new(document.clone(), window, cx));
        project.update(cx, |project, cx| {
            project.set_preferences(settings.clone(), None, cx);
            project.apply_writing_preferences(
                settings.focus_mode,
                settings.typewriter_mode,
                window,
                cx,
            );
        });
        navigator.update(cx, |navigator, cx| {
            navigator.set_view(project.clone().into(), cx)
        });
        let frontmatter = cx.new(FrontmatterForm::new);
        let header = cx.new(|cx| DocumentHeader::new(project.clone(), cx));
        document.update(cx, |document, cx| {
            document.set_header(header.clone().into(), cx)
        });
        let project_owner = project.downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            project_owner
                .update(cx, |project, cx| project.guard_window_close(window, cx))
                .unwrap_or(true)
        });
        let host = HostEnvironment::detect_with(cfg!(feature = "embedded-webview"));
        let preview_capability = host.capability();
        let preview_unavailable_reason =
            host.embedded_unavailable().map(|reason| reason.to_string());
        let preview = cx.new(PreviewPanel::new);
        let preview_panel = cx.new(|cx| WorkspacePanel::new("preview-panel", "Astro preview", "No preview running\n\nStart the Astro dev server, then open the active entry in your browser.", window, cx));
        preview_panel.update(cx, |panel, cx| panel.set_view(preview.clone().into(), cx));
        let rendered = cx.new(RenderedDocument::new);
        let rendered_panel =
            cx.new(|cx| WorkspacePanel::new("rendered-preview-panel", "Preview", "", window, cx));
        rendered_panel.update(cx, |panel, cx| panel.set_view(rendered.clone().into(), cx));
        let embedded = cx.new(EmbeddedPreview::new);
        let embedded_panel = cx.new(|cx| {
            WorkspacePanel::new(
                "embedded-preview-panel",
                "Embedded preview",
                "Start the preview server to load the page.",
                window,
                cx,
            )
        });
        embedded_panel.update(cx, |panel, cx| panel.set_view(embedded.clone().into(), cx));
        let mut initial_preview = settings.preview.clone();
        if initial_preview.presentation == PreviewPresentation::Embedded
            && !preview_capability.is_embedded()
        {
            initial_preview.presentation = PreviewPresentation::External;
        }
        preview.update(cx, |preview, cx| {
            preview.set_preferences(&initial_preview, cx)
        });
        let embedded_center_active = false;
        let terminal = cx.new(|cx| TerminalPanel::new(window, cx));
        let terminal_panel = cx.new(|cx| {
            WorkspacePanel::new(
                "terminal-panel",
                "Terminal",
                "No project terminal\n\nOpen a project to start an interactive shell.",
                window,
                cx,
            )
        });
        terminal_panel.update(cx, |panel, cx| panel.set_view(terminal.clone().into(), cx));
        let (dock, skin) = DockSkin::dock_area("astrolith-workspace", Some(1), window, cx);
        let initial_center = center_layout_for(
            &document,
            &rendered_panel,
            &embedded_panel,
            mode,
            embedded_center_active,
            window,
            cx,
        );
        dock.update(cx, |area, cx| {
            area.set_center(initial_center, window, cx);
            area.set_dock(
                DockPlacement::Left,
                DockLayout::tabs().panel_view(panel_handle(navigator.clone()), cx),
                window,
                cx,
            );
            if !layout.sidebar_visible || mode == WorkspaceMode::Writing {
                area.toggle_dock(DockPlacement::Left, window, cx);
            }
            // Dock sizes are measured logical pixels, not application spacing.
            area.set_dock_size(DockPlacement::Left, px(layout.sidebar_width), window, cx);
            // The reading view exists only while it is visible: a closed bottom
            // dock would otherwise keep a collapsed strip that can be dragged
            // back out.
            if layout.preview_visible {
                attach_preview_dock(
                    area,
                    panel_handle(preview_panel.clone()),
                    layout.preview_height,
                    window,
                    cx,
                );
            }
        });
        let palette = cx.new(|cx| CommandState::new(window, cx));
        let finder = cx.new(|cx| CommandState::new(window, cx));
        let builder = cx.new(|cx| ComponentBuilder::new(window, cx));
        let preferences_sheet = cx.new(|cx| PreferencesSheet::new(window, cx));
        let resetting = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new(LatestPreferences {
            global: (0, settings.clone()),
            project: None,
        }));
        let mut subscriptions = vec![
            cx.subscribe_in(&dock, window, |this, _, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    // Dock events already carry the new geometry. Clamping here
                    // would mutate the dock on every splitter move.
                    this.capture_layout(cx);
                    this.sync_terminal_activity(window, cx);
                    // Sidebar/preview visibility is part of menu availability.
                    this.menus_dirty = true;
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                this.capture_window_geometry(window, cx);
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
                    theme::apply(&this.settings, window.appearance(), cx);
                }
            }),
        ];
        subscriptions.push(cx.subscribe_in(
            &header,
            window,
            |this, _, _: &crate::editor::header::InspectorRequested, window, cx| {
                this.toggle_panel(PanelKind::Inspector, window, cx);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &header,
            window,
            |this, _, _: &LibraryRequested, window, cx| {
                this.toggle_panel(PanelKind::Sidebar, window, cx);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &project,
            window,
            |this, _, _: &SessionChanged, _window, cx| {
                this.sync_preview(cx);
                this.note_typing(cx);
                // Formatting availability follows the active document.
                this.menus_dirty = true;
            },
        ));
        // Recording the open, restoring the project layout and reopening its
        // last entry all need the window the project open completed in.
        subscriptions.push(cx.subscribe_in(
            &project,
            window,
            |this, _, event: &ProjectOpened, window, cx| {
                let root = event.root.clone();
                this.open_project_scope(root.clone(), window, cx);
                this.terminal
                    .update(cx, |terminal, cx| terminal.attach(&root, cx));
                this.sync_terminal_activity(window, cx);
                this.finish_pending_entry(window, cx);
            },
        ));
        // Build's file explorer resolves non-editable project files here, so the
        // shell owns their readonly load and status copy.
        subscriptions.push(cx.subscribe_in(
            &project,
            window,
            |this, _, event: &ReadonlyFileRequested, window, cx| {
                this.open_readonly_path(event.path.clone(), window, cx);
            },
        ));
        // Project load, entry activation and watcher refreshes notify the
        // workspace without emitting a text-edit event; the preview panel must
        // still attach on open and follow the active entry.
        subscriptions.push(cx.observe(&project, |this, project, cx| {
            this.sync_preview(cx);
            this.track_active_entry(cx);
            this.refresh_component_builder(&project, cx);
            // Collections, availability and the active document all live in the
            // project; the menu model follows its notifications.
            this.menus_dirty = true;
        }));
        // Undo/redo can change hidden frontmatter without changing the field key;
        // force the form to reload from the restored complete source.
        subscriptions.push(
            cx.subscribe(&project, |this, _, _: &HistoryChangedEvent, cx| {
                this.frontmatter_key = None;
                cx.notify();
            }),
        );
        // Menu and palette availability follows the dev-server phase; the
        // embedded surface follows the same ready URL and entry changes.
        subscriptions.push(cx.subscribe_in(
            &preview,
            window,
            |this, _, _: &PreviewStateChanged, window, cx| {
                this.refresh_menus(window, cx);
                this.sync_embedded(window, cx);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &embedded,
            window,
            |this, _, event: &EmbeddedPreviewEvent, window, cx| match event {
                EmbeddedPreviewEvent::OpenExternal => {
                    this.preview
                        .update(cx, |preview, cx| preview.open_active(cx));
                    this.sync_embedded(window, cx);
                }
            },
        ));
        let apply_owner = project.downgrade();
        subscriptions.push(cx.subscribe_in(
            &frontmatter,
            window,
            move |_this, _form, event: &FrontmatterEdit, window, cx| {
                let edit = event.clone();
                let _ = apply_owner.update(cx, |project, cx| {
                    project.apply_frontmatter_edit(edit, window, cx)
                });
            },
        ));
        // The builder form owns its own inputs; the shell only performs the
        // insertion it asks for and closes the dialog on success.
        subscriptions.push(cx.subscribe_in(
            &builder,
            window,
            |this, builder, event: &ComponentBuilderEvent, window, cx| {
                let ComponentBuilderEvent::Insert(selection) = event;
                let result = this.project.update(cx, |project, cx| {
                    project.insert_component(selection.clone(), window, cx)
                });
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        builder.update(cx, |builder, cx| builder.close(cx));
                    }
                    Err(message) => {
                        builder.update(cx, |builder, cx| builder.fail(message, cx));
                    }
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &preferences_sheet,
            window,
            |this, _, event: &PreferencesEvent, window, cx| match event {
                PreferencesEvent::Apply(edits) => {
                    this.apply_preferences((**edits).clone(), window, cx);
                    window.close_dialog(cx);
                }
                PreferencesEvent::ResetAll => {
                    window.close_all_dialogs(cx);
                    this.reset_all(cx);
                }
                PreferencesEvent::Cancel => window.close_dialog(cx),
                PreferencesEvent::OpenUserGuide => {
                    window.close_dialog(cx);
                    this.open_help(HelpSurface::UserGuide, window, cx);
                }
                PreferencesEvent::OpenKeyboardShortcuts => {
                    window.close_dialog(cx);
                    this.open_help(HelpSurface::KeyboardShortcuts, window, cx);
                }
            },
        ));
        if let Some(store) = store.clone() {
            let latest = latest.clone();
            let resetting = resetting.clone();
            // Register on App rather than the view: last-window closure may drop the view first.
            App::on_app_quit(cx, move |cx| {
                let store = store.clone();
                let latest = latest.clone();
                let flush_resetting = resetting.clone();
                let flush = runtime::flush_on_quit(cx, move || {
                    if flush_resetting.load(Ordering::SeqCst) {
                        return;
                    }
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
        // The terminal child is a process group; drop it through its explicit
        // shutdown path rather than relying on window teardown to run `Drop`.
        let terminal_owner = terminal.downgrade();
        App::on_app_quit(cx, move |cx| {
            let _ = terminal_owner.update(cx, |terminal, cx| terminal.shutdown(cx));
            async move {}
        })
        .detach();
        // Project language servers exit through the same bounded, graceful
        // shutdown a project switch uses.
        let lsp_owner = project.downgrade();
        App::on_app_quit(cx, move |cx| {
            let _ = lsp_owner.update(cx, |project, cx| project.shutdown_lsp(cx));
            async move {}
        })
        .detach();
        let focus = cx.focus_handle().tab_stop(true);
        focus.focus(window, cx);
        let updates = update::CapabilityState::detect();
        let update_status = update::Status::initial(&updates);
        Self {
            dock,
            document,
            document_header: header,
            project,
            navigator,
            frontmatter,
            frontmatter_key: None,
            inspector_open: false,
            inspector_focus: cx.focus_handle(),
            inspector_return_focus: None,
            library_open: mode == WorkspaceMode::Writing && layout.sidebar_visible,
            library_focus: cx.focus_handle(),
            library_return_focus: if mode == WorkspaceMode::Writing && layout.sidebar_visible {
                window.focused(cx).map(|focus| focus.downgrade())
            } else {
                None
            },
            preview,
            rendered,
            rendered_panel,
            preview_panel,
            embedded,
            embedded_panel,
            embedded_center_active,
            preview_capability,
            preview_unavailable_reason,
            embedded_failure: None,
            terminal,
            terminal_panel,
            build_tabs: ScrollHandle::new(),
            palette,
            palette_focus: cx.focus_handle().tab_stop(true),
            finder,
            finder_hits: Arc::new(Mutex::new(Vec::new())),
            builder,
            preferences_sheet,
            resetting,
            opening: false,
            opening_task: None,
            source_picker_task: None,
            file_status: "Readonly · No file open".into(),
            preview_sync_key: None,
            last_viewport: window.viewport_size(),
            _skin: skin,
            settings,
            mode,
            project_scope: None,
            scope_loading: None,
            applying_layout: false,
            ignoring_layout_event: false,
            store,
            latest,
            revision: 0,
            settings_task: None,
            project_task: None,
            scope_task: None,
            warning,
            updates,
            update_status,
            update_task: None,
            recent_records: Vec::new(),
            recent: Vec::new(),
            pending_entry: None,
            menu_signature: None,
            menus_dirty: true,
            menu_editor_key: None,
            #[cfg(test)]
            menu_refreshes: 0,
            #[cfg(test)]
            menu_installs: 0,
            instance_task: None,
            focus,
            chrome: ChromeVisibility::default(),
            typed: TypedCharacters::default(),
            rendered_sync_key: None,
            prose_style: None,
            #[cfg(test)]
            prose_style_resolutions: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Finish window setup: registry loading, startup routing and the
    /// single-instance listener.
    fn begin(
        &mut self,
        startup: StartupRequest,
        listener: Option<single_instance::Listener>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.listen_for_instances(listener, window, cx);
        if self.astro_preview_visible() {
            self.preview
                .update(cx, |preview, cx| preview.enter_preview(cx));
        }
        self.sync_embedded(window, cx);
        let store = self.store.clone();
        let work = runtime::blocking(cx, move || match store {
            Some(store) => store.recent_projects(),
            None => (Vec::new(), None),
        });
        cx.spawn_in(window, async move |this, cx| {
            let loaded = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match loaded {
                    Ok((records, warning)) => {
                        if let Some(warning) = warning {
                            this.warning = Some(warning);
                        }
                        this.set_registry(records);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not load the project registry");
                    }
                }
                let records = this.recent_records.clone();
                if startup.has_arguments() {
                    this.apply_startup(startup, &records, window, cx);
                } else if let Some(root) = recent_projects::last_openable(&records) {
                    tracing::info!(root = %root.display(), "reopening the last project");
                    this.project
                        .update(cx, |project, cx| project.open_root(root, window, cx));
                }
                this.refresh_menus(window, cx);
                cx.notify();
            });
        })
        .detach();
        self.schedule_launch_update_check(window, cx);
        #[cfg(debug_assertions)]
        self.start_perf_driver(window, cx);
    }

    fn set_registry(&mut self, records: Vec<ProjectRecord>) {
        self.recent = recent_projects::entries(&records);
        self.recent_records = records;
        self.menus_dirty = true;
    }

    /// Route one startup or forwarded request.
    fn apply_startup(
        &mut self,
        request: StartupRequest,
        records: &[ProjectRecord],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = request.target else {
            if let Some(problem) = request.problem {
                self.warning = Some(problem);
                cx.notify();
            }
            return;
        };
        match url_scheme::route(
            &target,
            records,
            astrolith_workspace::paths::home_directory().as_deref(),
        ) {
            url_scheme::Route::Project(root) => {
                self.pending_entry = None;
                if self.project.read(cx).project_root().as_deref() != Some(root.as_path()) {
                    self.project
                        .update(cx, |project, cx| project.open_root(root, window, cx));
                }
            }
            url_scheme::Route::Entry { root, entry } => {
                let already_open =
                    self.project.read(cx).project_root().as_deref() == Some(root.as_path());
                self.pending_entry = Some(entry);
                if already_open {
                    self.finish_pending_entry(window, cx);
                } else {
                    self.project
                        .update(cx, |project, cx| project.open_root(root, window, cx));
                }
            }
            url_scheme::Route::Invalid(message) => {
                tracing::warn!(%message, "refused a launch target");
                self.warning = Some(message);
                cx.notify();
            }
        }
    }

    /// Open a startup entry once its project index proves membership.
    fn finish_pending_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.pending_entry.take() else {
            return;
        };
        let result = self.project.update(cx, |project, cx| {
            project.open_external_entry(&path, window, cx)
        });
        if let Err(message) = result {
            tracing::warn!(%message, "refused a startup entry");
            self.warning = Some(message);
            cx.notify();
        }
    }

    /// Accept one forwarded launch and activate this window.
    fn listen_for_instances(
        &mut self,
        listener: Option<single_instance::Listener>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(listener) = listener else {
            return;
        };
        #[cfg(not(unix))]
        {
            let _ = (listener, window, cx);
        }
        #[cfg(unix)]
        {
            let listener = match runtime::chain_listener(cx, listener) {
                Ok(listener) => listener,
                Err(error) => {
                    tracing::warn!(%error, "could not listen for a second launch");
                    return;
                }
            };
            self.instance_task = Some(cx.spawn_in(window, async move |this, cx| {
                use tokio::io::AsyncBufReadExt as _;
                let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
                // Tokio I/O must be polled inside the service runtime; the
                // decoded request crosses back to this entity over a channel.
                let _service = runtime::spawn(cx, async move {
                    loop {
                        let Ok((stream, _)) = listener.accept().await else {
                            break;
                        };
                        let mut reader = tokio::io::BufReader::new(stream);
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.is_err() {
                            continue;
                        }
                        let arguments: Vec<std::ffi::OsString> = single_instance::decode(&line)
                            .into_iter()
                            .map(std::ffi::OsString::from)
                            .collect();
                        let request = url_scheme::parse_startup(arguments);
                        if sender.send(request).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(request) = receiver.recv().await {
                    let _ = this.update_in(cx, |this, window, cx| {
                        window.activate_window();
                        let records = this.recent_records.clone();
                        this.apply_startup(request, &records, window, cx);
                    });
                }
            }));
        }
    }

    /// Live availability shared by menus, palette, toolbar and context menus.
    fn availability(&mut self, window: &Window, cx: &mut Context<Self>) -> CommandAvailability {
        let layout = self.read_dock_layout(cx);
        let fullscreen = window.is_fullscreen();
        self.project.update(cx, |project, _| {
            project.set_view_state(
                layout.sidebar_visible,
                self.inspector_open,
                layout.preview_visible,
                fullscreen,
            )
        });
        let mut availability = self.project.read(cx).command_availability(cx, window);
        let preview = self.preview.read(cx);
        availability.preview_attached = preview.is_attached();
        availability.preview_running = preview.is_running();
        availability.preview_ready = preview.is_ready();
        availability.updates_configured = self.updates.is_enabled();
        availability.workspace_mode = self.mode;
        availability
    }

    /// Rebuild the native menu bar only when its model changed.
    fn refresh_menus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.menu_refreshes += 1;
        }
        self.menus_dirty = false;
        let availability = self.availability(window, cx);
        let unchanged = {
            let project = self.project.read(cx);
            let collections = project.collection_items();
            self.menu_signature
                .as_ref()
                .is_some_and(|(last, recent, last_collections)| {
                    *last == availability
                        && *recent == self.recent
                        && last_collections.as_slice() == collections
                })
        };
        if unchanged {
            return;
        }
        #[cfg(test)]
        {
            self.menu_installs += 1;
        }
        let collections = self.project.read(cx).collection_items().to_vec();
        commands::install_menus(
            cx,
            &self.settings.copyedit,
            availability,
            &self.recent,
            &collections,
        );
        self.menu_signature = Some((availability, self.recent.clone(), collections));
    }

    /// Open one collection through its generated registry command.
    fn open_collection(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.read_dock_layout(cx).sidebar_visible {
            self.toggle_panel(PanelKind::Sidebar, window, cx);
        }
        self.project
            .update(cx, |project, cx| project.open_collection(id, window, cx));
    }

    /// Reopen one recent project from the registry.
    fn open_recent(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(record) = self
            .recent_records
            .iter()
            .find(|record| record.id == id)
            .cloned()
        else {
            self.warning = Some("That project is no longer in the recent list.".into());
            cx.notify();
            return;
        };
        let target = StartupTarget::Project(PathBuf::from(&record.path));
        let records = self.recent_records.clone();
        let request = StartupRequest {
            target: Some(target),
            problem: None,
        };
        self.apply_startup(request, &records, window, cx);
    }

    /// Remove one registry entry and refresh every surface that lists it.
    fn remove_recent(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let id = id.to_owned();
        let requested = id.clone();
        let work = runtime::blocking(cx, move || store.remove_recent_project(&requested));
        cx.spawn_in(window, async move |this, cx| {
            let removed = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match removed {
                    Ok(Ok(true)) => {
                        this.recent_records.retain(|record| record.id != id);
                        this.recent = recent_projects::entries(&this.recent_records);
                        this.refresh_menus(window, cx);
                    }
                    Ok(Ok(false)) => {}
                    Ok(Err(error)) => {
                        this.warning = Some(format!("Could not update recent projects: {error}"));
                    }
                    Err(error) => {
                        this.warning = Some(format!("Could not update recent projects: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Recent Projects dialog: open or remove registry entries.
    fn open_recent_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let entries = self.recent.clone();
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let mut body = div().v_flex().gap_2();
            if entries.is_empty() {
                body = body.child(div().text_sm().child("No projects have been opened yet."));
            }
            for entry in &entries {
                let open_owner = owner.clone();
                let remove_owner = owner.clone();
                let open_id = entry.id.clone();
                let remove_id = entry.id.clone();
                body = body.child(
                    div()
                        .h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            div().v_flex().child(entry.label.clone()).child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(entry.path.display().to_string()),
                            ),
                        )
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "recent-open-{}",
                                        entry.id
                                    )))
                                    .label("Open")
                                    .on_click(
                                        move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ = open_owner.update(cx, |this, cx| {
                                                this.open_recent(&open_id, window, cx)
                                            });
                                        },
                                    ),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "recent-remove-{}",
                                        entry.id
                                    )))
                                    .label("Remove")
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = remove_owner.update(cx, |this, cx| {
                                                this.remove_recent(&remove_id, window, cx)
                                            });
                                        },
                                    ),
                                ),
                        ),
                );
            }
            dialog.title("Recent projects").w(px(560.)).child(body)
        });
    }

    /// Check for updates shortly after launch without touching the render path.
    ///
    /// The capability is compiled in; a build without it never schedules work.
    fn schedule_launch_update_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let update::CapabilityState::Enabled(capability) = self.updates.clone() else {
            return;
        };
        if !matches!(&self.update_status, update::Status::Idle) {
            return;
        }
        self.update_status = update::Status::Checking;
        let running = update::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("the crate version is a semantic version triple");
        let skipped = self.settings.skipped_update_version.clone();
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_secs(2));
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            timer.await;
            let checked = runtime::blocking(cx, move || {
                update::check(&update::HttpTransport, &capability, running)
            })
            .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.apply_update_check(checked, skipped.as_deref());
                if matches!(&this.update_status, update::Status::Available(_))
                    && !window.has_active_dialog(cx)
                {
                    this.show_update_status(window, cx);
                }
                this.refresh_menus(window, cx);
                cx.notify();
            });
        }));
    }

    /// On-demand check from the menu, palette or a protocol surface.
    fn check_for_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A capability-disabled build still explains itself when dispatched
        // programmatically, even though the command is unavailable to users.
        let update::CapabilityState::Enabled(capability) = self.updates.clone() else {
            self.show_update_status(window, cx);
            return;
        };
        if matches!(&self.update_status, update::Status::Checking) {
            self.show_update_status(window, cx);
            return;
        }
        if window.has_active_dialog(cx) {
            return;
        }
        let running = update::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("the crate version is a semantic version triple");
        let skipped = self.settings.skipped_update_version.clone();
        self.update_status = update::Status::Checking;
        self.show_update_status(window, cx);
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            let checked = runtime::blocking(cx, move || {
                update::check(&update::HttpTransport, &capability, running)
            })
            .await;
            let _ = this.update_in(cx, |this, window, cx| {
                window.close_dialog(cx);
                this.apply_update_check(checked, skipped.as_deref());
                this.show_update_status(window, cx);
                this.refresh_menus(window, cx);
                cx.notify();
            });
        }));
    }

    /// Fold a finished background check into status and the visible warning.
    fn apply_update_check(
        &mut self,
        checked: Result<Result<update::CheckOutcome, update::UpdateError>, String>,
        skipped: Option<&str>,
    ) {
        let checked = checked.unwrap_or_else(|error| Err(update::UpdateError::Network(error)));
        self.update_status = update::Status::finish(checked, skipped);
        if let update::Status::Failed(reason) = &self.update_status {
            self.warning = Some(format!("Could not check for updates. {reason}"));
        }
    }

    /// Show the latest update status, with an external release download only.
    fn show_update_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let status = self.update_status.clone();
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let title = match &status {
                update::Status::Disabled(_) => "Updates unavailable",
                update::Status::Failed(_) => "Update check failed",
                update::Status::UpToDate(_) => "No updates",
                update::Status::Available(_) | update::Status::Skipped(_) => "Update available",
                update::Status::Idle | update::Status::Checking => "Update check",
            };
            let mut body = div()
                .v_flex()
                .gap_3()
                .text_sm()
                .child(SharedString::from(status.message()));
            if let Some(release) = status.release() {
                body = body
                    .child(
                        div()
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Release notes"),
                            )
                            .child(div().text_sm().child(release.notes.clone())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format!("SHA-256 · {}", release.sha256))),
                    );
                let mut actions = div().h_flex().gap_2();
                let download = release.artifact_url.clone();
                actions = actions.child(
                    Button::new("update-download")
                        .label("Download for this platform")
                        .on_click(move |_, _, cx| cx.open_url(&download)),
                );
                let page = release.page.clone();
                actions = actions.child(
                    Button::new("update-release-page")
                        .label("Open release page")
                        .on_click(move |_, _, cx| cx.open_url(&page)),
                );
                if matches!(&status, update::Status::Available(_)) {
                    let version = release.version.to_string();
                    let skip_owner = owner.clone();
                    actions = actions.child(
                        Button::new("update-skip-version")
                            .label("Skip this version")
                            .on_click(move |_, window, cx| {
                                let _ = skip_owner
                                    .update(cx, |this, cx| this.skip_update_version(&version, cx));
                                window.close_dialog(cx);
                            }),
                    );
                }
                body = body.child(actions);
            }
            dialog.title(title).w(px(520.)).child(body)
        });
    }

    /// Remember one release version so launch checks stop offering it.
    fn skip_update_version(&mut self, version: &str, cx: &mut Context<Self>) {
        self.settings.skipped_update_version = Some(version.to_owned());
        if let update::Status::Available(release) = &self.update_status
            && release.version.to_string() == version
        {
            self.update_status = update::Status::Skipped(release.clone());
        }
        self.persist_global(cx);
        cx.notify();
    }

    /// About: version and build identity, with no update action.
    fn open_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        window.open_dialog(cx, |dialog, _, cx| {
            dialog.title("About Astrolith").w(px(420.)).child(
                div()
                    .v_flex()
                    .gap_1()
                    .text_sm()
                    .child(SharedString::from(commands::about_text()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("A local-first Astro content editor."),
                    ),
            )
        });
    }

    /// Open a bundled help surface. Nothing here reaches the network.
    fn open_help(&mut self, surface: HelpSurface, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let shortcuts = help::shortcut_groups(&self.settings.copyedit);
        let paths = help::local_paths();
        window.open_dialog(cx, move |dialog, _, cx| {
            let body = match surface {
                HelpSurface::UserGuide => help::guide_body(cx),
                HelpSurface::KeyboardShortcuts => help::shortcuts_body(&shortcuts, cx),
                HelpSurface::LocalData => help::local_data_body(&paths, cx),
            };
            dialog
                .title(surface.title())
                .w(px(680.))
                .max_h(px(620.))
                .child(body)
        });
    }

    /// Open the project, collection or active entry in the configured IDE.
    fn open_in_ide(&mut self, command: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            project.open_in_ide(command, None, window, cx)
        });
        cx.notify();
    }

    /// Handle a shell-level drop: one project directory or one in-project entry.
    fn drop_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.read(cx).project_root();
        match drag_drop::classify(&paths, root.as_deref()) {
            Ok(drag_drop::DropTarget::Project(root)) => {
                self.project
                    .update(cx, |project, cx| project.open_root(root, window, cx));
            }
            Ok(drag_drop::DropTarget::Entry(entry)) => {
                let result = self.project.update(cx, |project, cx| {
                    project.open_external_entry(&entry, window, cx)
                });
                if let Err(message) = result {
                    self.warning = Some(message);
                    cx.notify();
                }
            }
            Err(message) => {
                self.warning = Some(message);
                cx.notify();
            }
        }
    }

    /// Feed typed characters into the chrome auto-hide policy.
    ///
    /// Only characters typed into the active document count: deleting, cursor
    /// moves and the session's own formatting or paste edits never hide the
    /// chrome, switching documents restarts the count, and the policy itself
    /// refuses to hide while a sidebar is open.
    fn note_typing(&mut self, cx: &mut Context<Self>) {
        let layout = self.read_dock_layout(cx);
        let both_closed =
            self.mode == WorkspaceMode::Writing && !layout.sidebar_visible && !self.inspector_open;
        let typed = {
            let project = self.project.read(cx);
            match (project.active_path(), project.active_session(cx)) {
                (Some(path), Some(session)) => {
                    let session = session.read(cx);
                    self.typed.observe(
                        path,
                        &session.projection.visible,
                        session.programmatic_change(),
                    )
                }
                _ => {
                    self.typed.forget();
                    0
                }
            }
        };
        self.chrome.set_sidebars(both_closed);
        if self.chrome.type_characters(typed, both_closed) {
            cx.notify();
        }
    }

    /// A hover at the window's top edge restores auto-hidden chrome.
    fn hover_top_edge(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if hovered && self.chrome.restore() {
            cx.notify();
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

    /// The layout in force for one mode.
    ///
    /// The open project's override wins where it recorded one; otherwise the
    /// global preference applies. Clamping here keeps a stored layout inside
    /// the current viewport, exactly as the dock is sized.
    fn active_layout(&self, mode: WorkspaceMode) -> LayoutState {
        let mut layout = match self.project_scope.as_ref() {
            Some(scope) => scope
                .preferences
                .layout_overrides
                .get(mode)
                .unwrap_or_else(|| self.settings.layouts.get(mode)),
            None => self.settings.layouts.get(mode),
        }
        .clone();
        layout.clamp(f32::from(self.last_viewport.width));
        layout
    }

    /// The layout the dock currently reports.
    fn read_dock_layout(&self, cx: &App) -> LayoutState {
        let current = self.active_layout(self.mode);
        let dock = self.dock.read(cx);
        LayoutState {
            sidebar_visible: if self.mode == WorkspaceMode::Writing {
                self.library_open
            } else {
                dock.is_dock_open(DockPlacement::Left)
            },
            preview_visible: dock.is_dock_open(DockPlacement::Bottom),
            sidebar_width: dock
                .dock_size(DockPlacement::Left)
                .map(f32::from)
                .unwrap_or(240.),
            // A closed bottom dock has no settled size; keep the stored value.
            preview_height: dock
                .dock_size(DockPlacement::Bottom)
                .map(f32::from)
                .filter(|_| current.preview_visible)
                .unwrap_or(current.preview_height),
        }
    }

    fn capture_layout(&mut self, cx: &mut Context<Self>) {
        // A project's scope is being looked up; there is no owner for this
        // change yet and the loaded project layout will replace it anyway.
        if self.scope_loading.is_some() || self.applying_layout {
            return;
        }
        // A reveal that only serves the next command is not a preference.
        if std::mem::take(&mut self.ignoring_layout_event) {
            return;
        }
        let layout = self.read_dock_layout(cx);
        let current = self.active_layout(self.mode);
        let visibility_changed = layout.sidebar_visible != current.sidebar_visible
            || layout.preview_visible != current.preview_visible;
        if layout == current {
            return;
        }
        // The dock belongs to the open project's scope when there is one,
        // otherwise to the global preference; it never reaches both.
        if let Some(scope) = self.project_scope.as_mut() {
            scope
                .preferences
                .layout_overrides
                .set(self.mode, Some(layout));
            self.persist_project(cx);
        } else {
            self.settings.layouts.set(self.mode, layout);
            self.persist_global(cx);
        }
        // Width-only changes are carried by the dock itself. Notifying here
        // would rebuild the whole shell once per resize frame.
        if visibility_changed {
            cx.notify();
        }
    }

    /// Persist the live window geometry; the 1000×700 minimum is enforced by
    /// the window itself and this never shrinks stored values below it.
    fn capture_window_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let bounds = window.window_bounds();
        let maximized = matches!(bounds, WindowBounds::Maximized(_));
        let bounds = bounds.get_bounds();
        let geometry = WindowGeometry {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width).max(1000.),
            height: f32::from(bounds.size.height).max(700.),
            maximized,
        };
        if self.settings.window == Some(geometry) {
            return;
        }
        self.settings.window = Some(geometry);
        self.persist_global(cx);
    }

    /// Persist the global scope.
    fn persist_global(&mut self, cx: &mut Context<Self>) {
        if self.resetting.load(Ordering::SeqCst) {
            return;
        }
        self.revision += 1;
        let revision = self.revision;
        let settings = self.settings.clone();
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
        if self.resetting.load(Ordering::SeqCst) {
            return;
        }
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
                let layout = self.active_layout(self.mode);
                self.apply_layout(layout, window, cx);
                cx.notify();
                return;
            }
        };
        if let Some(warning) = warning {
            tracing::warn!(%warning, "project preferences loaded with a warning");
            self.warning = Some(warning);
        }
        let resolved = resolve(&self.settings, Some(&preferences), None);
        self.project_scope = Some(ProjectScope {
            root,
            preferences: preferences.clone(),
            revision,
        });
        self.sync_project_preferences(cx);
        self.mode = resolved.last_mode.unwrap_or(resolved.default_mode);
        self.sync_document_empty_copy(cx);
        let source_view = self.mode == WorkspaceMode::Build;
        self.project.update(cx, |project, cx| {
            project.set_source_view(source_view, window, cx)
        });
        let layout = self.active_layout(self.mode);
        self.apply_layout(layout, window, cx);
        self.sync_center(window, cx);
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

    /// Apply a layout to the dock without touching either layout authority.
    ///
    /// The applied layout is what [`Self::active_layout`] derives, so the
    /// deferred layout event compares equal and a restore never becomes a
    /// write.
    fn apply_layout(
        &mut self,
        mut layout: LayoutState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        layout.clamp(f32::from(window.viewport_size().width));
        self.applying_layout = true;
        if self.mode == WorkspaceMode::Writing {
            self.library_open = layout.sidebar_visible;
            self.document_header.update(cx, |header, cx| {
                header.set_library_open(layout.sidebar_visible, cx)
            });
        }
        self.dock.update(cx, |dock, cx| {
            let sidebar_visible = layout.sidebar_visible && self.mode != WorkspaceMode::Writing;
            if dock.is_dock_open(DockPlacement::Left) != sidebar_visible {
                dock.toggle_dock(DockPlacement::Left, window, cx);
            }
            dock.set_dock_size(DockPlacement::Left, px(layout.sidebar_width), window, cx);
        });
        self.set_preview_visible(layout.preview_visible, layout.preview_height, window, cx);
        self.applying_layout = false;
    }

    /// Show or hide the reading view by the bottom dock's presence.
    ///
    /// A closed bottom dock is not hidden: the dock keeps a collapsed strip
    /// that still paints and can be dragged back over the document. Removing
    /// the dock is the only state that leaves no trace, so hiding removes it
    /// and showing attaches it again at `height`.
    fn set_preview_visible(
        &mut self,
        visible: bool,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = if self.mode == WorkspaceMode::Build {
            panel_handle(self.terminal_panel.clone())
        } else {
            panel_handle(self.preview_panel.clone())
        };
        self.dock.update(cx, |dock, cx| match visible {
            true => {
                // Switching between Build and Preview swaps the bottom panel.
                if dock.has_dock(DockPlacement::Bottom) {
                    dock.remove_dock(DockPlacement::Bottom, window, cx);
                }
                attach_preview_dock(dock, panel, height, window, cx)
            }
            false => dock.remove_dock(DockPlacement::Bottom, window, cx),
        });
        // The Astro preview is a dock capability, not a reading-mode flag:
        // showing it is what arms Open-on-Enter and the embedded split.
        let astro_visible = visible && self.mode != WorkspaceMode::Build;
        self.preview.update(cx, |preview, cx| {
            if astro_visible {
                preview.enter_preview(cx);
            } else {
                preview.leave_preview(cx);
            }
        });
        if astro_visible {
            self.note_preview_fallback(cx);
        }
        self.sync_embedded(window, cx);
        self.sync_terminal_activity(window, cx);
        self.menus_dirty = true;
    }

    /// The terminal polls at frame rate only while Build shows its dock.
    ///
    /// Hiding the dock, or leaving Build, drops it to the slow cadence so a
    /// hidden shell is not a repaint source; the exit is still noticed.
    fn sync_terminal_activity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active =
            self.mode == WorkspaceMode::Build && self.dock.read(cx).has_dock(DockPlacement::Bottom);
        let terminal_focused = self.terminal.read(cx).has_focus(window);
        let became_active = self
            .terminal
            .update(cx, |terminal, cx| terminal.set_active(active, cx));
        if became_active {
            self.terminal.read(cx).focus_handle(cx).focus(window, cx);
        }
        // gpui focus is pure handle equality: removing the dock does not move
        // it, so a hidden terminal would keep every keystroke.
        if !active && terminal_focused {
            self.focus_document_surface(window, cx);
        }
    }

    /// Move focus to the active editor, or the document when none is open.
    fn focus_document_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.project.read(cx).active_session(cx) {
            session
                .read(cx)
                .editor
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        } else {
            self.document.read(cx).focus_handle(cx).focus(window, cx);
        }
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

    /// Build's buffer tabs, backed by retained sessions.
    ///
    /// The strip is only drawn while at least one buffer exists. Many tabs
    /// scroll horizontally instead of being clipped, and each tab uses the
    /// shared tab tokens so it reads as the same surface as the rest of the
    /// application.
    fn build_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.project.read(cx).open_tabs();
        let active = self.project.read(cx).active_path();
        let owner = cx.weak_entity();
        let project = self.project.clone();
        div()
            .id("build-tabs")
            .test_support()
            .h(px(40.))
            .flex_shrink_0()
            .h_flex()
            .items_end()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                div()
                    .id("build-tabs-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .h_flex()
                    .items_end()
                    .gap_1()
                    .px(px(12.))
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .track_scroll(&self.build_tabs)
                    .children(tabs.iter().enumerate().map(|(index, tab)| {
                        let owner = owner.clone();
                        let project = project.clone();
                        let path = tab.path.clone();
                        let close_owner = owner.clone();
                        let close_project = project.clone();
                        let close_path = tab.path.clone();
                        let selected = active.as_ref() == Some(&tab.path);
                        div()
                            .id(format!("build-tab-{index}"))
                            .test_support()
                            .flex_none()
                            .h_flex()
                            .h(px(32.))
                            .items_center()
                            .gap_2()
                            .px(px(10.))
                            .rounded_t(px(6.))
                            .when(selected, |tab| {
                                tab.bg(cx.theme().tab_active)
                                    .text_color(cx.theme().tab_active_foreground)
                            })
                            .when(!selected, |tab| {
                                tab.text_color(cx.theme().tab_foreground)
                                    .hover(|tab| tab.bg(cx.theme().tab_bar))
                            })
                            .cursor_pointer()
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |_this, cx| {
                                    project.update(cx, |project, cx| {
                                        project.activate_session(&path, window, cx)
                                    });
                                });
                            })
                            .child(
                                div()
                                    .max_w(px(200.))
                                    .truncate()
                                    .text_xs()
                                    .child(tab.label.clone()),
                            )
                            .when(tab.dirty, |tab| {
                                tab.child(div().size_1p5().rounded_full().bg(cx.theme().foreground))
                            })
                            .child(
                                Button::new(("build-tab-close", index))
                                    .small()
                                    .ghost()
                                    .icon(IconName::X)
                                    .tooltip("Close file")
                                    .accessibility_label(format!("Close {}", tab.label))
                                    .on_click(move |_, window, cx| {
                                        let _ = close_owner.update(cx, |_, cx| {
                                            close_project.update(cx, |project, cx| {
                                                project.close_path(close_path.clone(), window, cx)
                                            });
                                        });
                                    }),
                            )
                    })),
            )
    }

    /// The centered three-way mode switch.
    fn mode_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.mode;
        let owner = cx.weak_entity();
        div()
            .h_flex()
            .items_center()
            .gap_0p5()
            .p(px(3.))
            .rounded_full()
            .bg(cx.theme().tab_bar_segmented)
            .children(WorkspaceMode::ALL.map(|mode| {
                let owner = owner.clone();
                Button::new(mode.label())
                    .ghost()
                    .rounded_full()
                    .label(mode.label())
                    .selected(mode == current)
                    .tooltip(mode.label())
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| this.set_mode(mode, window, cx));
                    })
            }))
    }

    /// Keep the document empty state aligned with the navigation the mode shows.
    fn sync_document_empty_copy(&mut self, cx: &mut Context<Self>) {
        let has_project = self.project.read(cx).project_root().is_some();
        let copy = document_empty_copy(self.mode, has_project);
        self.document
            .update(cx, |document, cx| document.set_content(copy.to_owned(), cx));
    }

    /// Switch the workspace mode without touching the open document.
    ///
    /// Layouts are per mode, the floating inspector is transient, and no mode
    /// change reopens the project or a document.
    fn set_mode(&mut self, mode: WorkspaceMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        // Fold any pending splitter movement into the outgoing mode first.
        self.capture_layout(cx);
        let inspector_open = self.inspector_open;
        self.set_inspector_open(false, window, cx);
        self.mode = mode;
        if mode == WorkspaceMode::Preview {
            // Re-install the reading pane once when the view becomes visible,
            // even when the active document identity did not change.
            self.rendered_sync_key = None;
        }
        self.sync_document_empty_copy(cx);
        let mode_started = crate::editor::perf::start();
        self.project.update(cx, |project, cx| {
            project.set_source_view(mode == WorkspaceMode::Build, window, cx);
            project.set_writer_visible(mode == WorkspaceMode::Writing, window, cx);
        });
        if let Some(started) = mode_started {
            crate::editor::perf::span("mode_handler", mode.label(), started.elapsed());
        }
        let layout = self.active_layout(mode);
        self.apply_layout(layout, window, cx);
        if let Some(scope) = self.project_scope.as_mut()
            && scope.preferences.last_mode != Some(mode)
        {
            scope.preferences.last_mode = Some(mode);
            self.persist_project(cx);
        }
        self.sync_center(window, cx);
        self.sync_embedded(window, cx);
        if inspector_open {
            self.document.read(cx).focus_handle(cx).focus(window, cx);
        } else if mode == WorkspaceMode::Preview {
            let focus = self.rendered.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        } else if let Some(session) = self.project.read(cx).active_session(cx) {
            session
                .read(cx)
                .editor
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        }
        self.refresh_menus(window, cx);
        cx.notify();
    }

    /// `ToggleTerminal` from another mode enters Build and *shows* the dock.
    ///
    /// Entering Build applies its remembered layout, which may have the bottom
    /// dock hidden; the command must not then toggle it closed. Inside Build
    /// the command is the ordinary bottom-dock toggle, with no mode change.
    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != WorkspaceMode::Build {
            self.set_mode(WorkspaceMode::Build, window, cx);
            if !self.dock.read(cx).has_dock(DockPlacement::Bottom) {
                let height = self.active_layout(self.mode).preview_height;
                self.set_preview_visible(true, height, window, cx);
            }
            return;
        }
        self.toggle_bottom_dock(window, cx);
    }

    /// Toggle Build's bottom dock (the terminal) without touching the mode.
    fn toggle_bottom_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_panel(PanelKind::Preview, window, cx);
    }

    fn toggle_panel(&mut self, kind: PanelKind, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(kind, PanelKind::Inspector) {
            if self.mode != WorkspaceMode::Writing
                || (!self.inspector_open
                    && self.project.read(cx).active_frontmatter_key().is_none())
            {
                return;
            }
            self.set_inspector_open(!self.inspector_open, window, cx);
            return;
        }
        if matches!(kind, PanelKind::Preview) {
            // Presence, not open state: a closed bottom dock would leave its
            // collapsed strip on screen.
            let visible = !self.dock.read(cx).has_dock(DockPlacement::Bottom);
            let height = self.active_layout(self.mode).preview_height;
            self.set_preview_visible(visible, height, window, cx);
            // LayoutChanged also captures the new visibility.
            cx.notify();
            return;
        }
        if self.mode == WorkspaceMode::Writing {
            self.set_library_open(!self.library_open, window, cx);
            return;
        }
        let placement = match kind {
            PanelKind::Sidebar => DockPlacement::Left,
            PanelKind::Inspector | PanelKind::Preview => unreachable!("handled above"),
        };
        self.dock
            .update(cx, |area, cx| area.toggle_dock(placement, window, cx));
        // LayoutChanged also captures native collapse/resize controls.
        cx.notify();
    }

    fn set_library_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.library_open == open {
            return;
        }
        if open {
            if window.has_active_sheet(cx) || window.has_active_dialog(cx) {
                return;
            }
            self.library_return_focus = window.focused(cx).map(|focus| focus.downgrade());
            self.library_focus.focus(window, cx);
        } else if let Some(focus) = self
            .library_return_focus
            .take()
            .and_then(|focus| focus.upgrade())
        {
            focus.focus(window, cx);
        }
        self.library_open = open;
        self.document_header
            .update(cx, |header, cx| header.set_library_open(open, cx));
        self.menus_dirty = true;
        self.capture_layout(cx);
        cx.notify();
    }

    /// Transfer focus to the floating inspector and restore it on dismissal.
    fn set_inspector_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector_open == open {
            return;
        }
        if open {
            if window.has_active_sheet(cx) || window.has_active_dialog(cx) {
                return;
            }
            self.inspector_return_focus = window.focused(cx).map(|focus| focus.downgrade());
            self.inspector_focus.focus(window, cx);
        } else if let Some(focus) = self
            .inspector_return_focus
            .take()
            .and_then(|focus| focus.upgrade())
        {
            focus.focus(window, cx);
        }
        self.inspector_open = open;
        self.menus_dirty = true;
        self.document_header
            .update(cx, |header, cx| header.set_inspector_open(open, cx));
        cx.notify();
    }

    fn render_inspector(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let motion = cx.theme().motion_tokens();
        let transition = Transition::new(motion.duration_normal).easing(motion.easing_move.clone());
        let presence = Presence::new("frontmatter-inspector-motion", self.inspector_open)
            .transition(transition)
            .sample(window, cx);
        if !self.inspector_open && presence.progress <= 0. {
            return None;
        }
        let inset = window.rem_size();
        let width = (window.viewport_size().width - inset * 2.)
            .max(px(0.))
            .min(px(416.));
        let right = inset - (width + inset) * (1. - presence.progress);
        let project = self.project.read(cx);
        let subtitle = project
            .open_tabs()
            .get(project.active_tab_index())
            .and_then(|tab| tab.collection.as_ref())
            .map_or_else(
                || "Entry details".to_owned(),
                |collection| format!("Entry details · {collection}"),
            );
        let surface = div()
            .id("frontmatter-inspector-surface")
            .test_support()
            .absolute()
            .top(rems(4.5))
            .right(right)
            .bottom(inset)
            .w(width)
            .occlude()
            .v_flex()
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_xl()
            .child(
                div()
                    .v_flex()
                    .flex_shrink_0()
                    .gap_0p5()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().text_sm().font_semibold().child("Frontmatter"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(self.frontmatter.clone()),
            );
        let owner = cx.weak_entity();
        let sheet = Sheet::new(cx)
            .focus_handle(self.inspector_focus.clone())
            .request_close(move |window, cx| {
                let _ = owner.update(cx, |this, cx| this.set_inspector_open(false, window, cx));
            })
            .surface(surface);
        let sheet = if self.inspector_open {
            sheet.overlay(div().absolute().inset_0().occlude())
        } else {
            sheet
        };
        Some(deferred(sheet).into_any_element())
    }

    fn render_library(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        if self.mode != WorkspaceMode::Writing {
            return None;
        }
        let motion = cx.theme().motion_tokens();
        let transition = Transition::new(motion.duration_normal).easing(motion.easing_move.clone());
        let presence = Presence::new("writing-library-motion", self.library_open)
            .transition(transition)
            .sample(window, cx);
        if !self.library_open && presence.progress <= 0. {
            return None;
        }
        let inset = window.rem_size();
        let width = px(self.active_layout(self.mode).sidebar_width)
            .min((window.viewport_size().width - inset * 2.).max(px(0.)))
            .min(px(400.));
        let left = inset - (width + inset) * (1. - presence.progress);
        let surface = div()
            .id("writing-library-surface")
            .test_support()
            .absolute()
            .left(left)
            .top(rems(4.5))
            .bottom(inset)
            .w(width)
            .occlude()
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_xl()
            .track_focus(&self.library_focus)
            .key_context("WritingLibrary")
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::actions::Cancel, window, cx| {
                    this.set_library_open(false, window, cx);
                }),
            )
            .child(self.navigator.clone());
        let mut layer = div().absolute().inset_0();
        if self.library_open {
            let owner = cx.weak_entity();
            layer = layer.child(
                div()
                    .id("writing-library-dismiss")
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(rems(4.5))
                    .bottom(inset)
                    .on_mouse_down(gpui_kit::MouseButton::Left, move |_, window, cx| {
                        let _ =
                            owner.update(cx, |this, cx| this.set_library_open(false, window, cx));
                    }),
            );
        }
        Some(layer.child(surface).into_any_element())
    }
}

impl Shell {
    /// Attach the preview panel to the open project and its active entry.
    ///
    /// Reattaching to the same project root only refreshes the mapping, so a
    /// session change never restarts a running dev server.
    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        let key = {
            let project = self.project.read(cx);
            let mapping = project
                .preview_context()
                .map(|(_, _, mapping)| mapping)
                .unwrap_or_default();
            (project.index_revision(), project.active_path(), mapping)
        };
        if self.preview_sync_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_sync_key = Some(key);
        let context = self.project.read(cx).preview_context();
        let entry = self.project.read(cx).active_entry_id();
        let in_preview = self.astro_preview_visible();
        self.preview.update(cx, |preview, cx| {
            match context {
                Some((project, root, mapping)) => {
                    preview.ensure_attached(project, root, mapping, cx)
                }
                None => preview.detach(cx),
            }
            preview.set_entry(entry, cx);
            if in_preview {
                preview.resume_visit(cx);
            }
        });
        if in_preview {
            self.note_preview_fallback(cx);
        }
    }

    /// The presentation the workspace can actually use right now.
    fn effective_preview_presentation(&self) -> PreviewPresentation {
        if self.settings.preview.presentation == PreviewPresentation::Embedded
            && self.preview_capability.is_embedded()
            && self.embedded_failure.is_none()
        {
            PreviewPresentation::Embedded
        } else {
            PreviewPresentation::External
        }
    }

    /// Push the (possibly downgraded) preference into the preview panel.
    fn sync_preview_preferences(&mut self, cx: &mut Context<Self>) {
        let mut preferences = self.settings.preview.clone();
        preferences.presentation = self.effective_preview_presentation();
        self.preview
            .update(cx, |preview, cx| preview.set_preferences(&preferences, cx));
    }

    /// Explain an embedded request that this host/build downgraded to external.
    fn note_preview_fallback(&mut self, cx: &mut Context<Self>) {
        if self.settings.preview.presentation != PreviewPresentation::Embedded
            || self.effective_preview_presentation() == PreviewPresentation::Embedded
        {
            return;
        }
        let reason = self
            .embedded_failure
            .clone()
            .or_else(|| self.preview_unavailable_reason.clone())
            .unwrap_or_else(|| "embedded preview is unavailable on this host".to_owned());
        self.preview.update(cx, |preview, cx| {
            preview.note_embedded_unavailable(&reason, cx)
        });
    }

    /// Whether the Astro preview panel is on screen for the current mode.
    ///
    /// Build reserves the bottom dock for the terminal, so the Astro preview is
    /// a Writing/Preview capability; Preview mode itself only decides that the
    /// centre shows the rendered document.
    fn astro_preview_visible(&self) -> bool {
        self.mode != WorkspaceMode::Build && self.active_layout(self.mode).preview_visible
    }

    /// Put the panel the current mode reads into the centre.
    fn sync_center(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = center_layout_for(
            &self.document,
            &self.rendered_panel,
            &self.embedded_panel,
            self.mode,
            self.embedded_center_active,
            window,
            cx,
        );
        self.dock
            .update(cx, |dock, cx| dock.set_center(layout, window, cx));
    }

    /// Mount, retarget or remove the embedded surface for the current mode.
    fn sync_embedded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = self.astro_preview_visible()
            && self.effective_preview_presentation() == PreviewPresentation::Embedded;
        tracing::debug!(
            mode = ?self.mode,
            wanted,
            active = self.embedded_center_active,
            failure = ?self.embedded_failure,
            "sync embedded preview"
        );
        if !wanted {
            self.embedded
                .update(cx, |embedded, cx| embedded.set_active(false, cx));
            if self.embedded_center_active {
                self.set_embedded_center(false, window, cx);
            }
            return;
        }
        let url = self.preview.read(cx).embedded_url();
        let result = self
            .embedded
            .update(cx, |embedded, cx| embedded.set_url(url, window, cx));
        match result {
            Ok(()) => {
                self.embedded
                    .update(cx, |embedded, cx| embedded.set_active(true, cx));
                if !self.embedded_center_active {
                    self.set_embedded_center(true, window, cx);
                }
            }
            Err(reason) => {
                tracing::warn!(%reason, "embedded preview unavailable; using the external browser");
                self.embedded_failure = Some(reason.clone());
                self.preview.update(cx, |preview, cx| {
                    preview.note_embedded_unavailable(&reason, cx)
                });
                self.sync_preview_preferences(cx);
                self.embedded
                    .update(cx, |embedded, cx| embedded.set_active(false, cx));
                if self.embedded_center_active {
                    self.set_embedded_center(false, window, cx);
                }
            }
        }
    }

    fn set_embedded_center(&mut self, embedded: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.embedded_center_active = embedded;
        self.sync_center(window, cx);
    }

    fn execute(&mut self, command: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            CommandId::OpenFile => self.open_file(window, cx),
            CommandId::OpenProjectSource => self.open_project_source_picker(window, cx),
            CommandId::Save => {
                self.project
                    .update(cx, |project, cx| project.save_active(window, cx));
            }
            CommandId::ResolveConflictReload => {
                self.project.update(cx, |project, cx| {
                    project.resolve_conflict_reload(window, cx)
                });
            }
            CommandId::ResolveConflictOverwrite => {
                self.project.update(cx, |project, cx| {
                    project.resolve_conflict_overwrite(window, cx)
                });
            }
            CommandId::ResolveConflictSaveCopy => {
                self.project.update(cx, |project, cx| {
                    project.resolve_conflict_save_copy(window, cx)
                });
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
                if !self.read_dock_layout(cx).sidebar_visible {
                    // Revealing the navigator is a transient UI action for the
                    // project about to open: its layout event must never reach
                    // the outgoing project's or the global scope.
                    self.ignoring_layout_event = true;
                    self.toggle_panel(PanelKind::Sidebar, window, cx);
                }
                self.project
                    .update(cx, |project, cx| project.prompt(window, cx));
            }
            CommandId::OpenPalette => self.open_palette(window, cx),
            CommandId::OpenPreferences => self.open_preferences(window, cx),
            CommandId::OpenRecentProjects => self.open_recent_projects(window, cx),
            CommandId::ReloadCollections => {
                self.project
                    .update(cx, |project, cx| project.reload_collections(window, cx));
            }
            CommandId::OpenProjectInIde
            | CommandId::OpenCollectionInIde
            | CommandId::OpenEntryInIde => self.open_in_ide(command, window, cx),
            CommandId::About => self.open_about(window, cx),
            CommandId::OpenUserGuide => self.open_help(HelpSurface::UserGuide, window, cx),
            CommandId::OpenKeyboardShortcuts => {
                self.open_help(HelpSurface::KeyboardShortcuts, window, cx)
            }
            CommandId::OpenLocalData => self.open_help(HelpSurface::LocalData, window, cx),
            CommandId::CheckForUpdates => self.check_for_updates(window, cx),
            CommandId::ToggleFullscreen => {
                window.toggle_fullscreen();
                self.refresh_menus(window, cx);
            }
            CommandId::OpenContentFinder => self.open_content_finder(window, cx),
            CommandId::OpenComponentBuilder => self.open_component_builder(window, cx),
            CommandId::WorkspaceWriting => self.set_mode(WorkspaceMode::Writing, window, cx),
            CommandId::WorkspaceBuild => self.set_mode(WorkspaceMode::Build, window, cx),
            CommandId::WorkspacePreview => self.set_mode(WorkspaceMode::Preview, window, cx),
            CommandId::CycleWorkspaceMode => {
                let next = self.mode.next();
                self.set_mode(next, window, cx)
            }
            CommandId::ToggleSidebar => self.toggle_panel(PanelKind::Sidebar, window, cx),
            CommandId::ToggleInspector => self.toggle_panel(PanelKind::Inspector, window, cx),
            // In Build the bottom dock is the terminal, so "toggle preview"
            // there would silently toggle the terminal. Target what the label
            // names: preview elsewhere, terminal in Build.
            CommandId::TogglePreview if self.mode == WorkspaceMode::Build => {
                self.toggle_bottom_dock(window, cx)
            }
            CommandId::TogglePreview => self.toggle_panel(PanelKind::Preview, window, cx),
            CommandId::ToggleTerminal => self.toggle_terminal(window, cx),
            CommandId::StartPreview => self.preview.update(cx, |preview, cx| preview.start(cx)),
            CommandId::StopPreview => self.preview.update(cx, |preview, cx| preview.stop(cx)),
            CommandId::RestartPreview => self.preview.update(cx, |preview, cx| preview.restart(cx)),
            CommandId::CopyPreviewUrl => {
                self.preview.update(cx, |preview, cx| preview.copy_url(cx))
            }
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
            CommandId::FocusSelectionToolbar => self
                .document
                .update(cx, |panel, cx| panel.focus_selection_toolbar(window, cx)),
            CommandId::FindInDocument => {
                self.project
                    .update(cx, |project, cx| project.open_find(false, cx));
            }
            CommandId::ReplaceInDocument => {
                self.project
                    .update(cx, |project, cx| project.open_find(true, cx));
            }
            CommandId::FindNextMatch => {
                self.project
                    .update(cx, |project, cx| project.find_step(true, cx));
            }
            CommandId::FindPreviousMatch => {
                self.project
                    .update(cx, |project, cx| project.find_step(false, cx));
            }
            CommandId::CloseFind => {
                self.project
                    .update(cx, |project, cx| project.close_find(cx));
            }
            CommandId::GoToLine => {
                self.project
                    .update(cx, |project, cx| project.open_go_to_line_dialog(window, cx));
            }
            CommandId::SelectNextOccurrence => {
                self.project
                    .update(cx, |project, cx| project.select_next_occurrence(cx));
            }
            CommandId::CursorsAtLineEnds => {
                self.project
                    .update(cx, |project, cx| project.cursors_at_line_ends(cx));
            }
            CommandId::Bold
            | CommandId::Italic
            | CommandId::InsertLink
            | CommandId::Heading1
            | CommandId::Heading2
            | CommandId::Heading3
            | CommandId::Heading4
            | CommandId::Paragraph
            | CommandId::ToggleComment => {
                let command = format_command(command);
                self.project
                    .update(cx, |project, cx| project.apply_format(command, window, cx));
            }
            CommandId::ToggleFocusMode => {
                let modes = self.project.update(cx, |project, cx| {
                    project.toggle_focus_mode(window, cx);
                    project.writing_modes()
                });
                self.settings.focus_mode = modes.focus;
                self.settings.typewriter_mode = modes.typewriter;
                self.persist_global(cx);
            }
            CommandId::ToggleTypewriterMode => {
                let modes = self.project.update(cx, |project, cx| {
                    project.toggle_typewriter_mode(window, cx);
                    project.writing_modes()
                });
                self.settings.focus_mode = modes.focus;
                self.settings.typewriter_mode = modes.typewriter;
                self.persist_global(cx);
            }
            CommandId::ToggleCopyeditNouns
            | CommandId::ToggleCopyeditVerbs
            | CommandId::ToggleCopyeditAdjectives
            | CommandId::ToggleCopyeditAdverbs
            | CommandId::ToggleCopyeditConjunctions
            | CommandId::ToggleAllCopyedit => {
                if let Some(category) = command.copyedit_category() {
                    let enabled = !self.settings.copyedit.enabled(category);
                    self.settings.copyedit.set_enabled(category, enabled);
                } else {
                    let enabled = !self.settings.copyedit.all_enabled();
                    self.settings.copyedit.set_all(enabled);
                }
                self.persist_global(cx);
                let config = CopyeditConfig::from_preferences(&self.settings.copyedit);
                self.project
                    .update(cx, |project, cx| project.set_copyedit_config(config, cx));
                self.refresh_menus(window, cx);
            }
            CommandId::LightTheme | CommandId::DarkTheme | CommandId::SystemTheme => {
                self.settings.theme = match command {
                    CommandId::LightTheme => ThemeMode::Light,
                    CommandId::DarkTheme => ThemeMode::Dark,
                    _ => ThemeMode::System,
                };
                theme::apply(&self.settings, window.appearance(), cx);
                self.persist_global(cx);
            }
            CommandId::ResetLayout => {
                let mut layout = ModeLayouts::default().get(self.mode).clone();
                layout.clamp(f32::from(window.viewport_size().width));
                if let Some(scope) = self.project_scope.as_mut() {
                    scope
                        .preferences
                        .layout_overrides
                        .set(self.mode, Some(layout.clone()));
                    self.persist_project(cx);
                } else {
                    self.settings.layouts.set(self.mode, layout.clone());
                    self.persist_global(cx);
                }
                self.apply_layout(layout, window, cx);
            }
        }
        cx.notify();
    }

    /// Build the grouped palette rows shared by the UI and its tests.
    fn palette_groups(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, Vec<PaletteEntry>)> {
        let copyedit = self.settings.copyedit.clone();
        let availability = self.availability(window, cx);
        let order = [
            "File",
            "View",
            "Format",
            "Writer",
            "Preview",
            "Help",
            "Application",
            "Recent projects",
            "Collections",
        ];
        let mut groups: Vec<(&'static str, Vec<PaletteEntry>)> =
            order.iter().map(|title| (*title, Vec::new())).collect();
        for command in COMMANDS
            .iter()
            .copied()
            .filter(|command| *command != CommandId::OpenPalette)
        {
            if !command.is_available(availability) {
                continue;
            }
            let shortcut: SharedString = command
                .shortcut()
                .and_then(|key| gpui_kit::Keystroke::parse(key).ok())
                .map(|key| Kbd::format(&key).into())
                .unwrap_or_default();
            let entry = PaletteEntry {
                label: command
                    .label_in(&copyedit, availability.workspace_mode)
                    .into(),
                shortcut,
                icon: command_icon(command),
                action: PaletteAction::Command(command),
            };
            let title = commands::command_group(command);
            if let Some((_, entries)) = groups.iter_mut().find(|(name, _)| *name == title) {
                entries.push(entry);
            }
        }
        if !self.recent.is_empty() {
            let entries = groups
                .iter_mut()
                .find(|(name, _)| *name == "Recent projects")
                .map(|(_, entries)| entries)
                .expect("the recent group exists");
            for entry in &self.recent {
                entries.push(PaletteEntry {
                    label: entry.label.clone().into(),
                    shortcut: SharedString::default(),
                    icon: IconName::FolderOpen,
                    action: PaletteAction::RecentProject(entry.id.clone()),
                });
            }
        }
        let project = self.project.read(cx);
        let available_collections: Vec<_> = project
            .collection_items()
            .iter()
            .filter(|collection| !collection.active)
            .collect();
        if !available_collections.is_empty() {
            let entries = groups
                .iter_mut()
                .find(|(name, _)| *name == "Collections")
                .map(|(_, entries)| entries)
                .expect("the collections group exists");
            for collection in available_collections {
                entries.push(PaletteEntry {
                    label: commands::collection_command_label(&collection.name).into(),
                    shortcut: SharedString::default(),
                    icon: IconName::Folder,
                    action: PaletteAction::Collection(collection.name.clone()),
                });
            }
        }
        let groups: Vec<(&'static str, Vec<PaletteEntry>)> = groups
            .into_iter()
            .filter(|(_, entries)| !entries.is_empty())
            .collect();
        groups
            .into_iter()
            .filter(|(_, entries)| !entries.is_empty())
            .collect()
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        self.palette
            .update(cx, |state, cx| state.set_query("", window, cx));
        let palette = self.palette.clone();
        let owner = cx.weak_entity();
        let groups = self.palette_groups(window, cx);
        let opening = self.opening;
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let groups_for_confirm = groups.clone();
            let mut palette_command = Command::new(&palette)
                .bordered(false)
                .placeholder("Search commands…");
            for (title, entries) in &groups {
                let items = entries.iter().map(|entry| {
                    let entry = entry.clone();
                    let label = entry.label.clone();
                    let shortcut = entry.shortcut.clone();
                    let icon = entry.icon;
                    CommandItem::new()
                        .label(label.clone())
                        .icon(icon)
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
                                            Icon::new(icon)
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
                        .disabled(
                            matches!(entry.action, PaletteAction::Command(CommandId::OpenFile))
                                && opening,
                        )
                });
                palette_command =
                    palette_command.group(CommandGroup::new().label(*title).items(items));
            }
            let palette_command = palette_command
                .empty(|_, _, cx| {
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No matching commands")
                })
                .on_confirm(move |index, window, cx| {
                    let Some(entry) = groups_for_confirm
                        .get(index.section)
                        .and_then(|(_, entries)| entries.get(index.row))
                    else {
                        return;
                    };
                    let action = entry.action.clone();
                    window.close_dialog(cx);
                    let _ = owner.update(cx, |this, cx| match action {
                        PaletteAction::Command(command) => this.execute(command, window, cx),
                        PaletteAction::RecentProject(id) => this.open_recent(&id, window, cx),
                        PaletteAction::Collection(id) => this.open_collection(&id, window, cx),
                    });
                });
            dialog
                .title("Command palette")
                .w_128()
                .child(palette_command)
        });
        self.palette.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Open the single preferences sheet with the current snapshots.
    fn open_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let collections = self
            .project
            .read(cx)
            .preference_collections()
            .into_iter()
            .map(|(name, schema)| CollectionEntry { name, schema })
            .collect();
        let preferences_folder = settings_path()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf));
        let context = PreferencesContext {
            global: self.settings.clone(),
            project: self
                .project_scope
                .as_ref()
                .map(|scope| scope.preferences.clone()),
            collections,
            auto_import: self.project.read(cx).auto_import_declared(),
            preferences_folder,
            warning: self.warning.clone(),
            embedded_unavailable_reason: self
                .embedded_failure
                .clone()
                .or_else(|| self.preview_unavailable_reason.clone()),
        };
        self.preferences_sheet
            .update(cx, |sheet, cx| sheet.open(context, window, cx));
        let view = self.preferences_sheet.clone();
        let close = self.preferences_sheet.downgrade();
        let initial_focus = std::cell::Cell::new(true);
        // The sheet scales with the window instead of staying at a small fixed
        // size; the minimum keeps the navigation and form columns readable.
        let viewport = window.viewport_size();
        let dialog_width = px((f32::from(viewport.width) - 64.).clamp(720., 1040.));
        let dialog_height = px((f32::from(viewport.height) - 64.).clamp(560., 1040.));
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = close.clone();
            if initial_focus.replace(false) {
                view.read(cx).focus_handle(cx).focus(window, cx);
            }
            dialog
                .title("Preferences")
                .w(dialog_width)
                .max_h(dialog_height)
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |sheet, cx| sheet.cancel(cx));
                })
                .child(view.clone())
        });
    }

    /// Replace both scopes from one validated draft and apply live effects.
    fn apply_preferences(
        &mut self,
        edits: PreferenceEdits,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.resetting.load(Ordering::SeqCst) {
            return;
        }
        let mut global = edits.global;
        // The dock may have been resized while the sheet was open; the live
        // global layouts stay authoritative unless the draft requested an
        // explicit per-mode reset.
        global.layouts = self.settings.layouts.clone();
        let defaults = ModeLayouts::default();
        for mode in &edits.layout_resets.global {
            global.layouts.set(*mode, defaults.get(*mode).clone());
        }
        global
            .layouts
            .clamp(f32::from(window.viewport_size().width));
        global.window = self.settings.window;
        self.settings = global;
        if let Some(preferences) = edits.project
            && let Some(scope) = self.project_scope.as_mut()
        {
            scope.preferences = preferences;
            if edits.layout_resets.project {
                scope.preferences.layout_overrides = Default::default();
            }
        }
        theme::apply(&self.settings, window.appearance(), cx);
        self.sync_project_preferences(cx);
        let (focus, typewriter) = (self.settings.focus_mode, self.settings.typewriter_mode);
        self.project.update(cx, |project, cx| {
            project.apply_writing_preferences(focus, typewriter, window, cx)
        });
        self.embedded_failure = None;
        self.sync_preview_preferences(cx);
        self.preview
            .update(cx, |preview, cx| preview.resume_visit(cx));
        self.sync_embedded(window, cx);
        // Only an explicit reset changes the authoritative layout; a plain
        // Apply leaves the live dock (and its retained bottom panel) untouched.
        if !edits.layout_resets.global.is_empty() || edits.layout_resets.project {
            let layout = self.active_layout(self.mode);
            self.apply_layout(layout, window, cx);
        }
        self.persist_global(cx);
        self.persist_project(cx);
        // Focus/typewriter mode and copyedit labels are menu state.
        self.menus_dirty = true;
        cx.notify();
    }

    /// Delete preferences and the registry, then restart with defaults.
    fn reset_all(&mut self, cx: &mut Context<Self>) {
        self.resetting.store(true, Ordering::SeqCst);
        self.store = None;
        self.settings_task = None;
        self.project_task = None;
        self.record_latest(|latest| latest.project = None);
        let folder = settings_path()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf));
        let Some(folder) = folder else {
            cx.restart();
            return;
        };
        let work = runtime::blocking(cx, move || {
            crate::preferences::advanced::reset_preferences(&folder)
        });
        cx.spawn(async move |_this, cx| {
            if let Err(error) = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()))
            {
                tracing::error!(%error, "could not reset preferences");
            }
            cx.update(|cx| cx.restart());
        })
        .detach();
    }

    /// Open the component builder for the active MDX document.
    ///
    /// The Format menu is not gated before P12, so the command itself re-checks
    /// the document kind: an MDX tag must never be offered for Markdown.
    fn open_component_builder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        if !self
            .project
            .read(cx)
            .command_availability(cx, window)
            .mdx_document
        {
            self.file_status = "The component builder is available in MDX documents only.".into();
            cx.notify();
            return;
        }
        let context = match self.project.read(cx).component_builder_context() {
            Ok(context) => context,
            Err(message) => {
                self.file_status = message;
                cx.notify();
                return;
            }
        };
        self.builder
            .update(cx, |builder, cx| builder.open(context, window, cx));
        let view = self.builder.clone();
        let close = self.builder.downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let close = close.clone();
            dialog
                .title("Insert MDX component")
                .w_128()
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |builder, cx| builder.close(cx));
                })
                .child(view.clone())
        });
        self.builder
            .update(cx, |builder, cx| builder.focus_search(window, cx));
    }

    /// Hand a republished component index to an open builder.
    ///
    /// Opening the builder during the initial walk would otherwise leave a
    /// permanently empty palette; the project notifies on publication, and an
    /// open form keeps its state.
    fn refresh_component_builder(
        &mut self,
        project: &Entity<ProjectWorkspace>,
        cx: &mut Context<Self>,
    ) {
        if !self.builder.read(cx).is_open() {
            return;
        }
        if let Ok(context) = project.read(cx).component_builder_context() {
            self.builder
                .update(cx, |builder, cx| builder.refresh(context, cx));
        }
    }

    /// `Mod+/` dispatch: a comment in `.md`, the component builder in `.mdx`.
    fn mod_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let availability = self.project.read(cx).command_availability(cx, window);
        if !availability.editor_focused {
            return;
        }
        if availability.markdown_document {
            self.execute(CommandId::ToggleComment, window, cx);
        } else if availability.mdx_document {
            self.execute(CommandId::OpenComponentBuilder, window, cx);
        }
    }

    /// Open the searchable index of every entry in the project.
    fn open_content_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let entries = Arc::new(self.project.read(cx).content_finder_entries());
        self.finder_hits = Arc::new(Mutex::new(content_finder::search(&entries, "")));
        self.finder
            .update(cx, |state, cx| state.set_query("", window, cx));
        let palette = self.finder.clone();
        let hits = self.finder_hits.clone();
        let query_hits = self.finder_hits.clone();
        let query_entries = entries.clone();
        let owner = cx.weak_entity();
        let insert_owner = owner.clone();
        let insert_hits = self.finder_hits.clone();
        let insert_palette = self.finder.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let rows = hits.lock().map(|hits| hits.clone()).unwrap_or_default();
            let confirm_rows = rows.clone();
            let confirm_owner = owner.clone();
            let query_hits = query_hits.clone();
            let query_entries = query_entries.clone();
            let insert_owner = insert_owner.clone();
            let insert_hits = insert_hits.clone();
            let insert_palette = insert_palette.clone();
            dialog.title("Content Finder").w_128().child(
                div()
                    .key_context("ContentFinder")
                    .on_action(move |_: &commands::ContentFinderInsert, window, cx| {
                        let Some(index) = insert_palette.read(cx).selected_index() else {
                            return;
                        };
                        let hit = insert_hits
                            .lock()
                            .ok()
                            .and_then(|hits| hits.get(index.row).cloned());
                        if let Some(hit) = hit {
                            window.close_dialog(cx);
                            let _ = insert_owner
                                .update(cx, |this, cx| this.finder_insert(hit, window, cx));
                        }
                    })
                    .child(
                        Command::new(&palette)
                            .bordered(false)
                            .placeholder("Search entries by title or ID…")
                            .filterable(false)
                            .on_query(move |query, window, _| {
                                if let Ok(mut slot) = query_hits.lock() {
                                    *slot = content_finder::search(&query_entries, query);
                                }
                                window.refresh();
                            })
                            .items(rows.into_iter().map(|entry| {
                                let title: SharedString = entry.title.clone().into();
                                CommandItem::new()
                                    .label(title.clone())
                                    .icon(IconName::FileText)
                                    .child(move |_, _| {
                                        div()
                                            .h_flex()
                                            .w_full()
                                            .items_center()
                                            .gap_2()
                                            .child(title.clone())
                                    })
                            }))
                            .empty(|_, _, cx| {
                                div()
                                    .p_4()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("No matching entries")
                            })
                            .on_confirm(move |index, window, cx| {
                                let Some(entry) = confirm_rows.get(index.row).cloned() else {
                                    return;
                                };
                                window.close_dialog(cx);
                                let _ = confirm_owner
                                    .update(cx, |this, cx| this.finder_open(entry, window, cx));
                            }),
                    ),
            )
        });
        self.finder.update(cx, |state, cx| state.focus(window, cx));
    }

    fn finder_open(&mut self, entry: FinderEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            project.open_finder_entry(entry.index, &entry.source, window, cx)
        });
    }

    fn finder_insert(&mut self, entry: FinderEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            project.insert_finder_link(&entry, window, cx)
        });
        self.document.read(cx).focus_handle(cx).focus(window, cx);
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
                    Some(Ok(source)) => {
                        if this
                            .project
                            .read(cx)
                            .is_editable_project_source(&source.path)
                        {
                            if let Err(error) = this.project.update(cx, |project, cx| {
                                project.open_project_source(source.path, window, cx)
                            }) {
                                this.file_status = error;
                            }
                        } else {
                            this.show_document(source, window, cx);
                        }
                    }
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

    /// List project `.astro` sources and open one as an editable retained document.
    fn open_project_source_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some(root) = self.project.read(cx).project_root() else {
            self.file_status = "Open a project before opening its sources.".into();
            return;
        };
        let scan = runtime::blocking(cx, move || scan_project_astro_sources(&root));
        self.source_picker_task = Some(cx.spawn_in(window, async move |this, cx| {
            let sources = scan.await.and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| match sources {
                Ok(sources) => this.show_source_picker(sources, window, cx),
                Err(error) => {
                    this.file_status = format!("Could not scan project sources: {error}");
                    cx.notify();
                }
            });
        }));
    }

    fn show_source_picker(
        &mut self,
        sources: Vec<(PathBuf, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let rows = if sources.is_empty() {
                div()
                    .p_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No .astro sources were found in this project.")
                    .into_any_element()
            } else {
                div()
                    .id("project-source-list")
                    .test_support()
                    .v_flex()
                    .max_h(px(380.))
                    .overflow_y_scroll()
                    .children(sources.iter().enumerate().map(|(index, (path, label))| {
                        let owner = owner.clone();
                        let path = path.clone();
                        Button::new(format!("project-source-{index}"))
                            .small()
                            .ghost()
                            .label(label.clone())
                            .w_full()
                            .justify_start()
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = owner.update(cx, |this, cx| {
                                    if let Err(error) = this.project.update(cx, |project, cx| {
                                        project.open_project_source(path.clone(), window, cx)
                                    }) {
                                        this.file_status = error;
                                        cx.notify();
                                    }
                                });
                            })
                    }))
                    .into_any_element()
            };
            dialog.title("Open project source").w(px(560.)).child(rows)
        });
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

    /// Read one project file selected in Build's explorer as a readonly snapshot.
    fn open_readonly_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening {
            return;
        }
        self.opening = true;
        let read = runtime::blocking(cx, move || read_document(path));
        self.opening_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = read
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.opening = false;
                match result {
                    Ok(source) => this.show_document(source, window, cx),
                    Err(error) => {
                        tracing::warn!(%error, "could not open a project file");
                        this.file_status = format!("Could not open file: {error}");
                        cx.notify();
                    }
                }
            });
        }));
    }
}

/// Map a registry command to the writer transform it runs.
fn format_command(command: CommandId) -> astrolith_document::FormatCommand {
    use astrolith_document::{FormatCommand, HeadingLevel};
    match command {
        CommandId::Bold => FormatCommand::Bold,
        CommandId::Italic => FormatCommand::Italic,
        CommandId::InsertLink => FormatCommand::Link,
        CommandId::Heading1 => FormatCommand::Heading(HeadingLevel::H1),
        CommandId::Heading2 => FormatCommand::Heading(HeadingLevel::H2),
        CommandId::Heading3 => FormatCommand::Heading(HeadingLevel::H3),
        CommandId::Heading4 => FormatCommand::Heading(HeadingLevel::H4),
        CommandId::Paragraph => FormatCommand::Paragraph,
        _ => FormatCommand::Comment,
    }
}

fn command_icon(command: CommandId) -> IconName {
    match command {
        CommandId::OpenFile => IconName::FileText,
        CommandId::OpenProjectSource => IconName::FileSearch,
        CommandId::OpenProject => IconName::FolderOpen,
        CommandId::OpenRecentProjects => IconName::FolderOpen,
        CommandId::ReloadCollections => IconName::RotateCcw,
        CommandId::OpenProjectInIde
        | CommandId::OpenCollectionInIde
        | CommandId::OpenEntryInIde => IconName::SquarePen,
        CommandId::NewEntry => IconName::FilePlus,
        CommandId::DuplicateEntry => IconName::Copy,
        CommandId::RenameEntry => IconName::SquarePen,
        CommandId::DeleteEntry => IconName::Trash,
        CommandId::RevealEntry => IconName::FolderOpen,
        CommandId::CopyPathAbsolute | CommandId::CopyPathRelative => IconName::Copy,
        CommandId::Save => IconName::Save,
        CommandId::ResolveConflictReload => IconName::RotateCcw,
        CommandId::ResolveConflictOverwrite => IconName::Save,
        CommandId::ResolveConflictSaveCopy => IconName::Copy,
        CommandId::CloseDocument => IconName::X,
        CommandId::RecoverUnsaved => IconName::RotateCcw,
        CommandId::OpenPalette => IconName::Command,
        CommandId::WorkspaceWriting => IconName::Feather,
        CommandId::WorkspaceBuild => IconName::SquarePen,
        CommandId::WorkspacePreview => IconName::Eye,
        CommandId::CycleWorkspaceMode => IconName::RefreshCw,
        CommandId::ToggleSidebar => IconName::PanelLeft,
        CommandId::ToggleInspector => IconName::PanelRight,
        CommandId::TogglePreview => IconName::PanelBottom,
        CommandId::ToggleTerminal => IconName::Terminal,
        CommandId::StartPreview => IconName::Play,
        CommandId::StopPreview => IconName::Square,
        CommandId::RestartPreview => IconName::RefreshCw,
        CommandId::OpenPreview => IconName::Globe,
        CommandId::CopyPreviewUrl => IconName::Copy,
        CommandId::FocusDocument => IconName::FileText,
        CommandId::FocusSelectionToolbar => IconName::TextCursor,
        CommandId::FindInDocument | CommandId::ReplaceInDocument => IconName::Search,
        CommandId::FindNextMatch => IconName::ChevronDown,
        CommandId::FindPreviousMatch => IconName::ChevronUp,
        CommandId::CloseFind => IconName::X,
        CommandId::GoToLine => IconName::ListOrdered,
        CommandId::OpenContentFinder => IconName::FileSearch,
        CommandId::SelectNextOccurrence => IconName::TextCursor,
        CommandId::CursorsAtLineEnds => IconName::List,
        CommandId::Bold => IconName::Bold,
        CommandId::Italic => IconName::Italic,
        CommandId::InsertLink => IconName::Link,
        CommandId::Heading1 => IconName::Heading1,
        CommandId::Heading2 => IconName::Heading2,
        CommandId::Heading3 => IconName::Heading3,
        CommandId::Heading4 => IconName::Heading4,
        CommandId::Paragraph => IconName::Pilcrow,
        CommandId::ToggleComment => IconName::MessageCircle,
        CommandId::OpenComponentBuilder => IconName::FilePlus,
        CommandId::ToggleFocusMode => IconName::Eye,
        CommandId::ToggleTypewriterMode => IconName::Type,
        CommandId::ToggleCopyeditNouns
        | CommandId::ToggleCopyeditVerbs
        | CommandId::ToggleCopyeditAdjectives
        | CommandId::ToggleCopyeditAdverbs
        | CommandId::ToggleCopyeditConjunctions => IconName::Type,
        CommandId::ToggleAllCopyedit => IconName::Asterisk,
        CommandId::LightTheme => IconName::Sun,
        CommandId::DarkTheme => IconName::Moon,
        CommandId::SystemTheme => IconName::Monitor,
        CommandId::ResetLayout => IconName::RotateCcw,
        CommandId::OpenPreferences => IconName::Settings,
        CommandId::ToggleFullscreen => IconName::Maximize,
        CommandId::About => IconName::Info,
        CommandId::OpenUserGuide => IconName::Info,
        CommandId::OpenKeyboardShortcuts => IconName::Command,
        CommandId::OpenLocalData => IconName::FolderOpen,
        CommandId::CheckForUpdates => IconName::CloudDownload,
        CommandId::Quit => IconName::X,
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Editor focus and selection change availability without an event;
        // compare the cheap pair and rebuild menus only when it moved.
        let editor_key = self.project.read(cx).active_session(cx).map(|session| {
            let session = session.read(cx);
            let editor = session.editor.read(cx);
            (
                editor.focus_handle(cx).is_focused(window),
                !editor.selected_range().is_empty(),
            )
        });
        if self.menu_editor_key != editor_key {
            self.menu_editor_key = editor_key;
            self.menus_dirty = true;
        }
        // Every other menu input is cached by the change handlers; an
        // unrelated frame does no project-sized work here.
        if self.menus_dirty {
            self.refresh_menus(window, cx);
        }
        // Activate the frontmatter form whenever the active document, its schema
        // revision or its mapped fields change. Editing the form does not change
        // this key, so live edits never rebuild the controls mid-entry.
        let key = self.project.read(cx).active_frontmatter_key();
        if key != self.frontmatter_key {
            self.frontmatter_key = key;
            match self.project.read(cx).active_frontmatter(cx) {
                Some(active) => {
                    self.frontmatter.update(cx, |form, cx| {
                        form.set_document(
                            FrontmatterDocument {
                                schema: active.schema,
                                mappings: active.mappings,
                                references: active.references,
                                root: Some(active.root),
                                source: Some(active.source),
                                assets: active.assets,
                                path_mode: active.path_mode,
                                text: active.text,
                            },
                            window,
                            cx,
                        )
                    });
                }
                None => {
                    self.frontmatter.update(cx, |form, cx| form.clear(cx));
                }
            }
        }
        if self.inspector_open && self.frontmatter_key.is_none() {
            let owner = cx.weak_entity();
            window.defer(cx, move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if this.project.read(cx).active_frontmatter_key().is_none() {
                        this.set_inspector_open(false, window, cx);
                    }
                });
            });
        }
        let layout = self.active_layout(self.mode);
        // The document face, size, line height and measure are explicit
        // preferences; the pane width no longer rescales the prose. Resolving
        // the family scans the installed set, so it is cached until one of the
        // inputs changes.
        let ui_family = cx.theme().font_family.clone();
        let (font_family, font_size) = if self.mode == WorkspaceMode::Preview {
            (
                &self.settings.preview_font_family,
                self.settings.preview_font_size,
            )
        } else {
            (
                &self.settings.prose_font_family,
                self.settings.editor_font_size,
            )
        };
        let style_changed = self.prose_style.as_ref().is_none_or(|(inputs, _)| {
            !inputs.matches(
                font_family,
                font_size,
                self.settings.editor_line_height,
                self.settings.page_width_percent,
                &ui_family,
            )
        });
        if style_changed {
            #[cfg(test)]
            {
                self.prose_style_resolutions += 1;
            }
            let style = crate::editor::writing_modes::ProseStyle::new(
                theme::configured_prose_font(cx, font_family),
                font_size,
                self.settings.editor_line_height,
                self.settings.page_width_percent,
            );
            self.prose_style = Some((
                ProseStyleInputs {
                    family: font_family.clone(),
                    font_size,
                    line_height: self.settings.editor_line_height,
                    page_width: self.settings.page_width_percent,
                    ui_family,
                },
                style,
            ));
        }
        let code_mode = self.mode == WorkspaceMode::Build;
        if style_changed && let Some((_, style)) = self.prose_style.as_ref() {
            let style = style.clone();
            self.document.update(cx, |panel, cx| {
                panel.set_prose_style(style.clone(), cx);
            });
            self.rendered
                .update(cx, |rendered, cx| rendered.set_prose_style(style, cx));
        }
        self.document
            .update(cx, |panel, cx| panel.set_code(code_mode, cx));
        if self.mode == WorkspaceMode::Preview {
            // Only the identity of the active document is compared per frame;
            // the visible body is copied only when that identity changed.
            let (root, path, key) = {
                let project = self.project.read(cx);
                let root = project.project_root();
                let path = project.active_path();
                let key = project.active_session(cx).map(|session| {
                    let session = session.read(cx);
                    (session.kind(), session.buffer.revision())
                });
                (root, path, key)
            };
            let sync_key = RenderedSyncKey {
                root,
                path,
                kind: key.map(|(kind, _)| kind),
                revision: key.map(|(_, revision)| revision),
            };
            if self.rendered_sync_key.as_ref() != Some(&sync_key) {
                let (body, shared) = self
                    .project
                    .read(cx)
                    .active_session(cx)
                    .map(|session| {
                        let session = session.read(cx);
                        let shared = session.writer.read(cx).shared_document();
                        let writer = session.writer.clone();
                        (
                            (session.kind(), session.projection.visible.clone()),
                            (shared, writer),
                        )
                    })
                    .map_or((None, (None, None)), |(body, shared)| {
                        (Some(body), (shared.0, Some(shared.1)))
                    });
                let root = sync_key.root.clone();
                let path = sync_key.path.clone();
                self.rendered.update(cx, |rendered, cx| {
                    rendered.set_document(
                        root.as_deref(),
                        path.as_deref(),
                        body.as_ref().map(|(kind, text)| (*kind, text.as_str())),
                        shared.0,
                        shared.1,
                        cx,
                    )
                });
                self.rendered_sync_key = Some(sync_key);
            }
        }
        self.chrome.set_sidebars(
            self.mode == WorkspaceMode::Writing && !layout.sidebar_visible && !self.inspector_open,
        );
        let chrome_hidden = self.chrome.is_hidden() && self.warning.is_none();
        let project_name = self.project.read(cx).project_name();
        let has_buffers = self.project.read(cx).has_open_tabs();
        // Open, scan and builder failures used to be stored and never shown.
        // Surface them in a compact status line; the neutral "no file" copy
        // stays implicit so a healthy window keeps its original layout.
        let file_status = self.file_status.clone();
        let show_file_status = file_status != "Readonly · No file open" && !file_status.is_empty();

        let close_owner = self.project.downgrade();
        div()
            .v_flex()
            .track_focus(&self.focus)
            .key_context("Astrolith")
            .on_action(cx.listener(|this, command: &CommandId, window, cx| {
                this.execute(*command, window, cx)
            }))
            .on_action(
                cx.listener(|this, event: &commands::OpenCollection, window, cx| {
                    this.open_collection(&event.id, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, event: &commands::OpenRecentProject, window, cx| {
                    this.open_recent(&event.id, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, event: &commands::RemoveRecentProject, window, cx| {
                    this.remove_recent(&event.id, window, cx)
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drop_paths(paths.paths().to_vec(), window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &commands::ModSlash, window, cx| this.mod_slash(window, cx)),
            )
            .size_full()
            .relative()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(if chrome_hidden {
                // Only the top edge remains: hovering it restores the chrome.
                div()
                    .id("chrome-top-edge")
                    .test_support()
                    .h(px(4.))
                    .w_full()
                    .flex_shrink_0()
                    .bg(cx.theme().tab_bar)
                    .on_hover(cx.listener(|this, hovered, _, cx| this.hover_top_edge(*hovered, cx)))
                    .into_any_element()
            } else {
                div()
                    .flex_shrink_0()
                    .child(
                        TitleBar::new()
                            .h(rems(3.5))
                            .on_close_window(move |_, window, cx| {
                                let clean = close_owner
                                    .update(cx, |project, cx| {
                                        project.guard_window_close(window, cx)
                                    })
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
                                        div()
                                            .h_flex()
                                            .items_center()
                                            .gap_2()
                                            .flex_1()
                                            .min_w_0()
                                            .child(crate::ui::wordmark(cx))
                                            .when_some(project_name.clone(), |row, name| {
                                                row.child(
                                                    div()
                                                        .max_w(px(220.))
                                                        .truncate()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(name),
                                                )
                                            })
                                            .when_some(self.warning.clone(), |row, warning| {
                                                row.child(crate::ui::v_divider(cx)).child(
                                                    div()
                                                        .max_w(px(320.))
                                                        .truncate()
                                                        .text_xs()
                                                        .text_color(cx.theme().warning)
                                                        .child(warning),
                                                )
                                            }),
                                    )
                                    .child(self.mode_switch(cx))
                                    .child(
                                        div()
                                            .h_flex()
                                            .items_center()
                                            .gap_2()
                                            .flex_1()
                                            .justify_end()
                                            .child(
                                                Button::new("command-palette")
                                                    .small()
                                                    .ghost()
                                                    .icon(IconName::Search)
                                                    .tooltip(CommandId::OpenPalette.label())
                                                    .accessibility_label(
                                                        CommandId::OpenPalette.label(),
                                                    )
                                                    .track_focus(&self.palette_focus)
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.palette_focus.focus(window, cx);
                                                            this.execute(
                                                                CommandId::OpenPalette,
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                            .child({
                                                let owner = cx.weak_entity();
                                                let sidebar = layout.sidebar_visible;
                                                let preview = layout.preview_visible;
                                                let inspector = self.inspector_open;
                                                let modes = self.project.read(cx).writing_modes();
                                                let writing = self.mode == WorkspaceMode::Writing;
                                                let build = self.mode == WorkspaceMode::Build;
                                                Button::new("view-menu")
                                                    .small()
                                                    .ghost()
                                                    .icon(IconName::PanelsTopLeft)
                                                    .tooltip("Panels")
                                                    .accessibility_label("Panels and writing modes")
                                                    .dropdown_menu(move |menu, _, _| {
                                                        let item = |label: &'static str,
                                                            kind: CommandId,
                                                            checked: bool| {
                                                    let owner = owner.clone();
                                                    PopupMenuItem::new(label)
                                                        .checked(checked)
                                                        .on_click(move |_, window, cx| {
                                                            let _ = owner.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.execute(kind, window, cx)
                                                                },
                                                            );
                                                        })
                                                };
                                                        let mut menu = menu
                                                            .item(item(
                                                                "Focus mode",
                                                                CommandId::ToggleFocusMode,
                                                                modes.focus,
                                                            ))
                                                            .item(item(
                                                                "Typewriter mode",
                                                                CommandId::ToggleTypewriterMode,
                                                                modes.typewriter,
                                                            ))
                                                            .separator()
                                                            .item(item(
                                                                "Library",
                                                                CommandId::ToggleSidebar,
                                                                sidebar,
                                                            ))
                                                            .item(item(
                                                                if build {
                                                                    "Terminal"
                                                                } else {
                                                                    "Astro preview"
                                                                },
                                                                CommandId::TogglePreview,
                                                                preview,
                                                            ));
                                                        if writing {
                                                            menu = menu.item(item(
                                                                "Details",
                                                                CommandId::ToggleInspector,
                                                                inspector,
                                                            ));
                                                        }
                                                        menu
                                                    })
                                            })
                                            .child(
                                                Button::new("open-preferences")
                                                    .small()
                                                    .ghost()
                                                    .icon(IconName::Settings)
                                                    .tooltip(CommandId::OpenPreferences.label())
                                                    .accessibility_label(
                                                        CommandId::OpenPreferences.label(),
                                                    )
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.execute(
                                                                CommandId::OpenPreferences,
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    )
                    // The reveal animation is a no-op (static end state, no
                    // frames) when the platform asks for reduced motion.
                    .with_animation(
                        "writer-chrome-reveal",
                        Animation::new(std::time::Duration::from_millis(140)),
                        |element, delta| element.opacity(delta),
                    )
                    .into_any_element()
            })
            .when(self.mode == WorkspaceMode::Build && has_buffers, |shell| {
                shell.child(self.build_tab_strip(cx))
            })
            .child(div().flex_1().min_h_0().min_w_0().child(self.dock.clone()))
            .children(self.render_library(window, cx))
            .children(self.render_inspector(window, cx))
            .when(show_file_status, |shell| {
                shell.child(
                    div()
                        .id("file-status")
                        .test_support()
                        .flex_shrink_0()
                        .h(px(20.))
                        .w_full()
                        .h_flex()
                        .items_center()
                        .px_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().tab_bar)
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(file_status),
                )
            })
    }
}

/// Empty document copy for the navigation the active mode shows.
fn document_empty_copy(mode: WorkspaceMode, has_project: bool) -> &'static str {
    match (mode, has_project) {
        (WorkspaceMode::Build, true) => "Choose a file from the explorer to open it.",
        (WorkspaceMode::Build, false) => "Open a project to browse its files.",
        (WorkspaceMode::Writing | WorkspaceMode::Preview, _) => {
            "Open a project, then choose an entry from the library to begin writing."
        }
    }
}

/// The centre layout: the mode's reading surface, or that surface beside the
/// embedded Astro preview with a draggable splitter. The embedded slot keeps
/// roughly the mock's inverse ratio (source 42%, preview 58%).
fn center_layout_for(
    document: &Entity<WorkspacePanel>,
    rendered: &Entity<WorkspacePanel>,
    embedded: &Entity<WorkspacePanel>,
    mode: WorkspaceMode,
    embedded_active: bool,
    window: &Window,
    cx: &App,
) -> DockLayout {
    // Preview reads the document; Writing and Build edit it.
    let main = if mode == WorkspaceMode::Preview {
        rendered
    } else {
        document
    };
    if !embedded_active {
        return DockLayout::tabs().panel_view(panel_handle(main.clone()), cx);
    }
    let source_width = (f32::from(window.viewport_size().width) * 0.42).max(360.);
    DockLayout::h_split()
        .child(
            DockLayout::tabs().panel_view(panel_handle(main.clone()), cx),
            Some(px(source_width)),
        )
        .child(
            DockLayout::tabs().panel_view(panel_handle(embedded.clone()), cx),
            None,
        )
}

/// Attach the reading view as the bottom dock.
///
/// The bottom dock is deliberately not collapsible. A closed bottom dock keeps
/// a 29px strip so it can be dragged or clicked back open, which is not
/// "hidden" in a writer's sense; the shell hides the reading view by removing
/// the dock instead, and reopening restores it at `height`.
fn attach_preview_dock(
    dock: &mut DockArea,
    panel: Arc<dyn BasePanelView>,
    height: f32,
    window: &mut Window,
    cx: &mut Context<DockArea>,
) {
    if !dock.has_dock(DockPlacement::Bottom) {
        dock.set_dock(
            DockPlacement::Bottom,
            DockLayout::tabs().panel_view(panel, cx),
            window,
            cx,
        );
    } else if !dock.is_dock_open(DockPlacement::Bottom) {
        dock.toggle_dock(DockPlacement::Bottom, window, cx);
    }
    dock.set_dock_collapsible(DockPlacement::Bottom, false, window, cx);
    dock.set_dock_size(DockPlacement::Bottom, px(height), window, cx);
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
    use crate::{
        editor::component_builder::BuilderContext,
        frontmatter::ReferenceCatalog,
        preferences::{FieldId, Pane as PreferencesPane},
    };
    use astrolith_astro::components::ComponentIndex;
    use astrolith_core::preferences::{FileExtension, PanelLayout};
    use gpui_kit::{
        Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions, base::Root,
        point, px, size, test::TestWindowExt,
    };

    fn mount(
        cx: &mut TestAppContext,
        settings: GlobalPreferencesV2,
    ) -> (WindowHandle<Root>, Entity<Shell>) {
        mount_sized(cx, settings, 1100., 720.)
    }

    fn mount_sized(
        cx: &mut TestAppContext,
        settings: GlobalPreferencesV2,
        width: f32,
        height: f32,
    ) -> (WindowHandle<Root>, Entity<Shell>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            runtime::init(cx).unwrap();
            commands::bind_keys(cx);
            let (handle, view) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Point::default(),
                        size(px(width), px(height)),
                    ))),
                    window_min_size: Some(size(px(1000.), px(700.))),
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
    async fn copyedit_commands_toggle_each_category_and_the_all_switch(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        assert!(!view.read_with(cx, |view, _| view.settings.copyedit.all_enabled()));
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::ToggleCopyeditNouns, window, cx)
            });
        })
        .unwrap();
        assert!(view.read_with(cx, |view, _| {
            view.settings
                .copyedit
                .enabled(astrolith_core::preferences::CopyeditCategory::Noun)
        }));
        assert!(!view.read_with(cx, |view, _| view.settings.copyedit.all_enabled()));

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::ToggleAllCopyedit, window, cx)
            });
        })
        .unwrap();
        assert!(view.read_with(cx, |view, _| view.settings.copyedit.all_enabled()));

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::ToggleAllCopyedit, window, cx)
            });
        })
        .unwrap();
        assert!(view.read_with(cx, |view, _| {
            astrolith_core::preferences::CopyeditCategory::ALL
                .iter()
                .all(|category| !view.settings.copyedit.enabled(*category))
        }));
    }

    #[gpui_kit::test]
    async fn prose_typography_resolves_once_per_settings_change(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let resolved = view.read(cx).prose_style_resolutions;
            assert_eq!(resolved, 1, "the first frame resolves the typography");
            window.render_frame(cx);
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).prose_style_resolutions,
                resolved,
                "an unrelated frame must not scan the installed fonts again"
            );

            view.update(cx, |shell, _| shell.settings.editor_font_size += 1.);
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).prose_style_resolutions,
                resolved + 1,
                "a prose preference change resolves exactly once"
            );
            let size = view
                .read(cx)
                .prose_style
                .as_ref()
                .expect("a resolved style is cached")
                .1
                .font_size;
            assert_eq!(size, view.read(cx).settings.editor_font_size);

            window.render_frame(cx);
            assert_eq!(view.read(cx).prose_style_resolutions, resolved + 1);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn unrelated_frames_do_not_rebuild_menus_but_changes_do(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        let (refreshes, installs) =
            view.read_with(cx, |view, _| (view.menu_refreshes, view.menu_installs));
        assert!(refreshes > 0, "the first frame installs the menus");
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.menu_refreshes, refreshes,
                "an unrelated frame must not recompute menu availability"
            );
            assert_eq!(view.menu_installs, installs);
        });

        // A mode change is one of the menu inputs.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_mode(WorkspaceMode::Build, window, cx)
            });
            window.render_frame(cx);
        })
        .unwrap();
        view.read_with(cx, |view, _| {
            assert!(
                view.menu_installs > installs,
                "a mode change updates the menu model"
            );
        });

        // Project notifications are the collection/availability path.
        let refreshes = view.read_with(cx, |view, _| view.menu_refreshes);
        view.update(cx, |view, cx| {
            view.project.update(cx, |_, cx| cx.notify());
        });
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.menu_refreshes,
                refreshes + 1,
                "a project change schedules exactly one menu refresh"
            );
        });
    }

    #[gpui_kit::test]
    async fn preview_frames_do_not_replace_an_unchanged_reading_document(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let directory = project_fixture("preview-frame-site");
        std::fs::write(
            directory.path().join("src/content/first.md"),
            "---\ntitle: First\n---\n# First\n\nA body that the reading pane shows.\n",
        )
        .unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let (project, _) =
            open_first_entry(cx, handle, &view, directory.path().to_path_buf()).await;
        let editor = project
            .read_with(cx, |project, cx| project.active_session(cx))
            .unwrap()
            .read_with(cx, |session, _| session.editor.clone());
        let session = project
            .read_with(cx, |project, cx| project.active_session(cx))
            .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Preview, window, cx)
            });
            window.render_frame(cx);
            let installed = view.read(cx).rendered.read(cx).sync_count();
            assert_eq!(installed, 1, "Preview installs the body once");

            window.render_frame(cx);
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).rendered.read(cx).sync_count(),
                installed,
                "an unchanged session must not copy or replace the body"
            );

            session.update(cx, |session, cx| {
                session
                    .patch_complete(window, cx, |source| {
                        Ok(format!("{source}\nAn appended paragraph.\n"))
                    })
                    .expect("the buffer accepts an edit")
            });
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).rendered.read(cx).sync_count(),
                installed + 1,
                "a buffer edit reinstalls the body exactly once"
            );

            window.render_frame(cx);
            assert_eq!(view.read(cx).rendered.read(cx).sync_count(), installed + 1);
        })
        .unwrap();
        // The editor entity stays mounted and unchanged by the reading pane.
        assert!(!editor.read_with(cx, |editor, _| editor.value().is_empty()));
    }

    #[gpui_kit::test]
    async fn writing_mode_toggles_persist_and_restore(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::ToggleFocusMode, window, cx);
                shell.execute(CommandId::ToggleTypewriterMode, window, cx);
            });
        })
        .unwrap();
        assert!(view.read_with(cx, |shell, _| shell.settings.focus_mode));
        assert!(view.read_with(cx, |shell, _| shell.settings.typewriter_mode));

        cx.executor().advance_clock(Duration::from_millis(400));
        let written = global_path.clone();
        cx.condition(&view, move |_, _| {
            std::fs::read(&written)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["focus_mode"].as_bool())
                .is_some_and(|enabled| enabled)
        })
        .await;
        let (_, restored, _) = PreferencesStore::load(global_path);
        assert!(restored.focus_mode);
        assert!(restored.typewriter_mode);

        // Applying preferences replaces the same live presentation state.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.settings.focus_mode = false;
                shell.settings.typewriter_mode = false;
                shell.sync_project_preferences(cx);
                let project = shell.project.clone();
                project.update(cx, |project, cx| {
                    project.apply_writing_preferences(false, false, window, cx)
                });
                assert!(!project.read(cx).writing_modes().focus);
                assert!(!project.read(cx).writing_modes().typewriter);
            });
        })
        .unwrap();
    }

    async fn open_first_entry(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<Shell>,
        root: std::path::PathBuf,
    ) -> (Entity<ProjectWorkspace>, std::path::PathBuf) {
        open_project(cx, handle, view, root);
        let project = view.read_with(cx, |shell, _| shell.project.clone());
        cx.condition(&project, |project, _| !project.is_loading())
            .await;
        cx.update_window(handle.into(), |_, window, cx| {
            project.update(cx, |project, cx| {
                project.open_first_entry_for_tests(window, cx)
            });
        })
        .unwrap();
        let path = wait_for_active_session(cx, &project).await;
        (project, path)
    }

    #[gpui_kit::test]
    async fn writing_surface_click_moves_the_caret_and_keeps_typing_working(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = project_fixture("wysiwyg-click");
        std::fs::write(
            directory.path().join("src/content/first.md"),
            "# A rendered heading\n\nA paragraph with enough words to click into the middle of the rendered page and keep typing.\n\n- one item\n- two items\n",
        )
        .unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let (project, _) =
            open_first_entry(cx, handle, &view, directory.path().to_path_buf()).await;
        let editor = project
            .read_with(cx, |project, cx| project.active_session(cx))
            .unwrap()
            .read_with(cx, |session, _| session.editor.clone());
        let original = editor.read_with(cx, |editor, _| editor.value().to_string());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            editor.update(cx, |editor, cx| editor.set_selected_range(0..0, cx));
            window.click_at(("input", editor.entity_id()), point(px(120.), px(20.)), cx);
            window.render_frame(cx);
            assert!(
                editor.read(cx).cursor() > 0,
                "clicking the rendered page must place the caret in the document"
            );
            window.input("Z", cx);
            let after_typing = editor.read(cx).cursor();
            window.press("left", cx);
            assert!(
                editor.read(cx).cursor() < after_typing,
                "left arrow must move the caret through the rendered text"
            );
            window.press("up", cx);
        })
        .unwrap();
        let typed = editor.read_with(cx, |editor, _| editor.value().to_string());
        assert_ne!(
            typed, original,
            "typing after the click must reach the buffer"
        );
        assert!(typed.contains('Z'));
    }

    #[gpui_kit::test]
    async fn frontmatter_inspector_is_an_overlay_with_unchanged_document_bounds(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = project_fixture("inspector-site");
        std::fs::write(
            directory.path().join("src/content/first.md"),
            "---\ntitle: First\n---\n# First\n",
        )
        .unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let (project, _) =
            open_first_entry(cx, handle, &view, directory.path().to_path_buf()).await;
        assert!(view.read_with(cx, |shell, cx| {
            !shell.inspector_open && shell.project.read(cx).active_frontmatter_key().is_some()
        }));
        let before = cx
            .update_window(handle.into(), |_, window, cx| {
                cx.set_reduce_motion(true);
                window.render_frame(cx);
                assert!(window.try_find("frontmatter-inspector-surface").is_none());
                window.find("document-panel").bounds()
            })
            .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                window.try_find("toggle-inspector").is_some(),
                "inspector trigger must render"
            );
            window.click("toggle-inspector", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("frontmatter-inspector-surface").is_some(),
                "the frontmatter form must be mounted in the sheet"
            );
            assert!(window.find("frontmatter-form").visible());
            let drawer = window.find("frontmatter-inspector-surface").bounds();
            assert!(
                (window.viewport_size().width - drawer.right() - window.rem_size()).abs() <= px(1.)
            );
            assert_eq!(window.find("document-panel").bounds(), before);
            assert!(view.read(cx).inspector_open);
        })
        .unwrap();

        // Outside dismissal keeps the drawer mounted while it slides out.
        cx.update_window(handle.into(), |_, window, cx| {
            cx.set_reduce_motion(false);
            window.click_at("document-panel", point(px(40.), px(400.)), cx);
            window.render_frame(cx);
            assert!(!view.read(cx).inspector_open);
            assert!(window.try_find("frontmatter-inspector-surface").is_some());
            assert_eq!(window.find("document-panel").bounds(), before);
        })
        .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("frontmatter-inspector-surface").is_none());
            assert_eq!(window.find("document-panel").bounds(), before);
        })
        .unwrap();
        assert!(cx.update(|cx| !view.read(cx).inspector_open));

        assert!(project.read_with(cx, |_, _| true));
    }

    #[gpui_kit::test]
    async fn inspector_escape_restores_editor_focus_and_mode_switch_closes_it(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = project_fixture("inspector-focus-site");
        std::fs::write(
            directory.path().join("src/content/first.md"),
            "---\ntitle: First\n---\n# First\n",
        )
        .unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let (project, _) =
            open_first_entry(cx, handle, &view, directory.path().to_path_buf()).await;
        let editor = project
            .read_with(cx, |project, cx| project.active_session(cx))
            .unwrap()
            .read_with(cx, |session, _| session.editor.clone());

        cx.update_window(handle.into(), |_, window, cx| {
            cx.set_reduce_motion(true);
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-2", cx);
            window.render_frame(cx);
            assert!(window.find("frontmatter-inspector-surface").visible());
            assert!(!editor.read(cx).focus_handle(cx).is_focused(window));
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("frontmatter-inspector-surface").is_none());
            assert!(
                editor.read(cx).focus_handle(cx).is_focused(window),
                "Escape returns focus to the document"
            );
        })
        .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::ToggleInspector, window, cx)
            });
            window.render_frame(cx);
            assert!(window.find("frontmatter-inspector-surface").visible());
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Preview, window, cx)
            });
            window.render_frame(cx);
            assert!(!view.read(cx).inspector_open);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn content_finder_opens_entries_and_inserts_links_with_undo(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let directory = project_fixture("site");
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        open_project(cx, handle, &view, directory.path().to_path_buf());
        let project = view.read_with(cx, |view, _| view.project.clone());
        cx.condition(&project, |project, _| !project.is_loading())
            .await;
        cx.update_window(handle.into(), |_, window, cx| {
            project.update(cx, |project, cx| {
                project.open_first_entry_for_tests(window, cx)
            });
        })
        .unwrap();
        let first = wait_for_active_session(cx, &project).await;
        assert!(first.ends_with("first.md"));

        // Enter opens the highlighted entry.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenContentFinder, window, cx)
            });
            assert!(window.has_active_dialog(cx));
            window.input("second", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let query = view.read(cx).finder.read(cx).query(cx);
            let count = view.read(cx).finder.read(cx).matched_count();
            assert_eq!(query, "second", "the finder owns the typed query");
            assert_eq!(count, 1, "one entry matches the query");
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let second = wait_for_active_session(cx, &project).await;
        assert!(second.ends_with("second.md"));

        // Ctrl+Enter inserts a title-text link in the active document.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenContentFinder, window, cx)
            });
            window.input("first", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("ctrl-enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let session = project
            .read_with(cx, |project, cx| project.active_session(cx))
            .unwrap();
        assert!(session.read_with(cx, |session, _| {
            session.buffer.current_text().contains("[first](first.md)")
        }));

        // The insertion is one undo step.
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("ctrl-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!session.read_with(cx, |session, _| {
            session.buffer.current_text().contains("[first](first.md)")
        }));
    }

    async fn wait_for_active_session(
        cx: &mut TestAppContext,
        project: &Entity<ProjectWorkspace>,
    ) -> std::path::PathBuf {
        for _ in 0..300 {
            cx.run_until_parked();
            if let Some(path) = project.read_with(cx, |project, cx| {
                project
                    .active_path()
                    .filter(|_| project.active_session(cx).is_some())
            }) {
                return path;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        panic!("no active session appeared");
    }

    #[gpui_kit::test]
    async fn opening_project_reveals_hidden_navigator(cx: &mut TestAppContext) {
        let settings = GlobalPreferencesV2 {
            layouts: ModeLayouts {
                writing: LayoutState {
                    sidebar_visible: false,
                    ..LayoutState::default()
                },
                ..ModeLayouts::default()
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
            view.library_open && !view.dock.read(cx).is_dock_open(DockPlacement::Left)
        }));
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
    }
    #[gpui_kit::test]
    async fn switching_workspace_modes_keeps_one_session_text_selection_and_history(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let project = project_fixture("mode-site");
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        cx.update_window(handle.into(), |_, _, cx| {
            assert_eq!(view.read(cx).mode, WorkspaceMode::Writing);
            assert!(
                !view
                    .read(cx)
                    .dock
                    .read(cx)
                    .is_dock_open(DockPlacement::Left)
            );
            assert!(
                !view
                    .read(cx)
                    .dock
                    .read(cx)
                    .is_dock_open(DockPlacement::Right)
            );
            assert!(!view.read(cx).dock.read(cx).has_dock(DockPlacement::Bottom));
        })
        .unwrap();
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
            view.update(cx, |view, cx| {
                view.project.update(cx, |project, cx| {
                    project.open_first_entry_for_tests(window, cx)
                })
            });
        })
        .unwrap();
        let project_entity = view.read_with(cx, |shell, _| shell.project.clone());
        let _session = wait_for_active_session(cx, &project_entity).await;
        let (session, editor, original) = view.read_with(cx, |shell, cx| {
            let session = shell
                .project
                .read(cx)
                .active_session(cx)
                .expect("the first entry opened");
            let editor = session.read(cx).editor.clone();
            (
                session.entity_id(),
                editor.clone(),
                editor.read(cx).value().to_string(),
            )
        });
        assert!(!original.is_empty());

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.input("x", cx);
            editor.update(cx, |editor, cx| editor.set_selected_range(0..3, cx));
        })
        .unwrap();
        let typed = editor.read_with(cx, |editor, _| editor.value().to_string());
        assert_ne!(typed, original);
        let entity = editor.entity_id();
        let mut build_entity = None;

        for mode in [
            WorkspaceMode::Build,
            WorkspaceMode::Preview,
            WorkspaceMode::Writing,
        ]
        .into_iter()
        .cycle()
        .take(6)
        {
            cx.update_window(handle.into(), |_, window, cx| {
                view.update(cx, |view, cx| view.set_mode(mode, window, cx));
            })
            .unwrap();
            cx.run_until_parked();
            view.read_with(cx, |shell, cx| {
                assert_eq!(shell.mode, mode);
                let current = shell
                    .project
                    .read(cx)
                    .active_session(cx)
                    .expect("the session stays open");
                let current_editor = current.read(cx).editor.clone();
                assert_eq!(
                    current.entity_id(),
                    session,
                    "all modes share the document session and history"
                );
                if mode == WorkspaceMode::Build {
                    let expected = *build_entity.get_or_insert(current_editor.entity_id());
                    assert_eq!(
                        current_editor.entity_id(),
                        expected,
                        "Build retains its display state"
                    );
                    assert_ne!(current_editor.entity_id(), entity);
                } else {
                    assert_eq!(
                        current_editor.entity_id(),
                        entity,
                        "Writing retains its input model"
                    );
                }
                assert_eq!(current_editor.read(cx).value(), typed);
                assert_eq!(current_editor.read(cx).selected_range(), 0..3);
            });
        }

        // Typed text and its undo history survived every mode change.
        cx.update_window(handle.into(), |_, window, cx| {
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-z", cx);
        })
        .unwrap();
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.value().to_string()),
            original
        );
        assert_eq!(
            view.read_with(cx, |shell, cx| shell
                .project
                .read(cx)
                .active_session(cx)
                .expect("the session is still open")
                .entity_id()),
            session
        );
    }

    #[gpui_kit::test]
    async fn modes_show_writing_build_and_rendered_preview_surfaces(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let directory = project_fixture("mode-surfaces-site");
        std::fs::write(
            directory.path().join("src/content/first.md"),
            "---\ntitle: First\n---\n# First body\n\nText *here*.\n",
        )
        .unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let (project, _) =
            open_first_entry(cx, handle, &view, directory.path().to_path_buf()).await;
        let (session, projection) = project.read_with(cx, |project, cx| {
            let session = project.active_session(cx).expect("the entry opened");
            let projection = session.read(cx).projection.visible.clone();
            (session, projection)
        });
        assert_eq!(projection, "# First body\n\nText *here*.\n");

        // Writing: writer page with the document rail and its inspector trigger.
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("document-panel").visible());
            assert!(window.find("document-header").visible());
            assert!(window.try_find("toggle-inspector").is_some());
            assert!(window.try_find("rendered-preview-panel").is_none());
        })
        .unwrap();

        // Preview: the rendered body replaces the editor, and the frontmatter
        // inspector is unavailable outside Writing.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Preview, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("rendered-preview-panel").visible());
            assert!(window.try_find("document-panel").is_none());
            assert!(window.try_find("document-header").is_none());
            assert!(
                window.try_find("preview-panel").is_none(),
                "the Astro preview dock stays closed until requested"
            );
            assert_eq!(view.read(cx).rendered.read(cx).text(), projection);
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::ToggleInspector, window, cx)
            });
            assert!(!view.read(cx).inspector_open);
            assert!(window.try_find("frontmatter-inspector-surface").is_none());
            // The reading pane owns focus, so typing cannot reach the session.
            window.input("zzz", cx);
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.buffer.current_text(),
                "---\ntitle: First\n---\n# First body\n\nText *here*.\n"
            );
        });

        // Build: complete source as an IDE surface, no writer header.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Build, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("document-panel").visible());
            assert!(window.try_find("document-header").is_none());
            assert_eq!(
                session.read(cx).editor.read(cx).value(),
                "---\ntitle: First\n---\n# First body\n\nText *here*.\n"
            );
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::ToggleInspector, window, cx)
            });
            assert!(!view.read(cx).inspector_open);
        })
        .unwrap();

        // Returning to Writing restores the writer rail.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Writing, window, cx)
            });
            window.render_frame(cx);
            assert!(window.find("document-header").visible());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn mode_layouts_persist_per_mode_and_a_project_restores_its_last_mode(
        cx: &mut TestAppContext,
    ) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        let project = project_fixture("mode-layout-site");
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
            view.update(cx, |view, cx| {
                view.set_mode(WorkspaceMode::Build, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                view.read(cx)
                    .dock
                    .read(cx)
                    .is_dock_open(DockPlacement::Left)
            );
            view.update(cx, |view, cx| {
                view.execute(CommandId::ToggleSidebar, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!view.read(cx).inspector_open);
            view.update(cx, |view, cx| {
                view.set_mode(WorkspaceMode::Preview, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.mode, WorkspaceMode::Preview);
            assert!(
                !shell.dock.read(cx).is_dock_open(DockPlacement::Bottom),
                "Preview reads the document; the Astro preview dock starts closed"
            );
        });

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_mode(WorkspaceMode::Build, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |shell, cx| {
            assert!(
                !shell.dock.read(cx).is_dock_open(DockPlacement::Left),
                "Build keeps its own saved sidebar state"
            );
            let scope = shell.project_scope.as_ref().unwrap();
            assert_eq!(scope.preferences.last_mode, Some(WorkspaceMode::Build));
            assert_eq!(
                scope
                    .preferences
                    .layout_overrides
                    .build
                    .as_ref()
                    .map(|layout| layout.sidebar_visible),
                Some(false)
            );
        });

        let id = view.read_with(cx, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .unwrap()
                .preferences
                .project_id
                .clone()
        });
        let saved_path = preferences_root.join("projects").join(format!("{id}.json"));
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        let persisted = saved_path.clone();
        cx.condition(&view, move |_, _| {
            std::fs::read(&persisted)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["last_mode"].as_str().map(str::to_owned))
                .as_deref()
                == Some("build")
        })
        .await;
        let saved = project_file(&preferences_root, &id);
        assert_eq!(saved["last_mode"], "build");
        assert_eq!(saved["layout_overrides"]["build"]["sidebar_visible"], false);

        // A fresh process restores the project's last mode, not the global default.
        let (store, settings, _) = PreferencesStore::load(global_path);
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        open_project(cx, handle, &view, project.path().to_path_buf());
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == canonical)
        })
        .await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.mode, WorkspaceMode::Build);
            assert!(!shell.dock.read(cx).is_dock_open(DockPlacement::Left));
        });
    }

    #[gpui_kit::test]
    async fn build_buffer_tabs_appear_only_while_buffers_are_open(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let project = project_fixture("build-tabs-site");
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

        // An empty Build leaves no buffer strip behind.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Build, window, cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("build-tabs").is_none());
        })
        .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.project.update(cx, |project, cx| {
                    project.open_first_entry_for_tests(window, cx)
                });
            });
        })
        .unwrap();
        let project_entity = view.read_with(cx, |shell, _| shell.project.clone());
        wait_for_active_session(cx, &project_entity).await;

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("build-tabs").visible());
            assert!(window.find("build-tab-0").visible());
        })
        .unwrap();

        // Closing the last buffer removes the strip instead of leaving a blank row.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell
                    .project
                    .update(cx, |project, cx| project.close_active(window, cx));
            });
            window.render_frame(cx);
            assert!(window.try_find("build-tabs").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn build_mode_shows_complete_source_and_real_document_tabs(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let project = project_fixture("build-source-site");
        let source = "---\ntitle: Hidden\n---\n\n# Body\n";
        std::fs::write(project.path().join("src/content/first.md"), source).unwrap();
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
            view.update(cx, |view, cx| {
                view.project.update(cx, |project, cx| {
                    project.open_first_entry_for_tests(window, cx)
                })
            });
        })
        .unwrap();
        let project_entity = view.read_with(cx, |shell, _| shell.project.clone());
        wait_for_active_session(cx, &project_entity).await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_mode(WorkspaceMode::Build, window, cx)
            });
            window.render_frame(cx);
            assert!(window.find("build-tab-0").visible());
            view.update(cx, |view, cx| {
                let session = view
                    .project
                    .read(cx)
                    .active_session(cx)
                    .expect("the entry stays open");
                assert_eq!(
                    session.read(cx).editor.read(cx).value(),
                    source,
                    "Build must show the complete source"
                );
            });
            view.update(cx, |view, cx| {
                view.set_mode(WorkspaceMode::Writing, window, cx)
            });
            view.update(cx, |view, cx| {
                let session = view
                    .project
                    .read(cx)
                    .active_session(cx)
                    .expect("the entry stays open");
                assert_eq!(
                    session.read(cx).editor.read(cx).value(),
                    "# Body\n",
                    "Writing hides the frontmatter again"
                );
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn terminal_is_project_bound_and_survives_mode_and_dock_changes(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let first = project_fixture("terminal-first");
        let second = project_fixture("terminal-second");
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));

        open_project(cx, handle, &view, first.path().to_path_buf());
        let first_root = std::fs::canonicalize(first.path()).unwrap();
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == first_root)
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_mode(WorkspaceMode::Build, window, cx)
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::TogglePreview, window, cx)
            });
            window.render_frame(cx);
        })
        .unwrap();
        view.read_with(cx, |shell, cx| {
            assert_eq!(
                shell
                    .terminal
                    .read(cx)
                    .root()
                    .and_then(|root| std::fs::canonicalize(root).ok()),
                Some(first_root.clone())
            );
            assert!(shell.terminal.read(cx).is_running());
            assert!(shell.dock.read(cx).has_dock(DockPlacement::Bottom));
        });
        let first_pid = view.read_with(cx, |shell, cx| shell.terminal.read(cx).process_id());
        assert!(first_pid.is_some());

        // Real shell input and output reach the parsed terminal screen.
        view.update(cx, |shell, cx| {
            shell.terminal.update(cx, |terminal, _| {
                terminal.write(b"printf 'terminal-ready\\n'\r")
            });
        });
        let mut saw_output = false;
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |shell, cx| {
                shell
                    .terminal
                    .read(cx)
                    .screen_contents()
                    .contains("terminal-ready")
            }) {
                saw_output = true;
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(saw_output, "the shell command must render in the terminal");

        // Hiding the dock and changing modes never kills the session.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::TogglePreview, window, cx);
                view.set_mode(WorkspaceMode::Writing, window, cx);
            });
        })
        .unwrap();
        view.read_with(cx, |shell, cx| {
            assert!(shell.terminal.read(cx).is_running());
            assert_eq!(shell.terminal.read(cx).process_id(), first_pid);
        });

        // Switching projects terminates the old child and starts the new one.
        open_project(cx, handle, &view, second.path().to_path_buf());
        let second_root = std::fs::canonicalize(second.path()).unwrap();
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == second_root)
        })
        .await;
        let second_pid = view.read_with(cx, |shell, cx| {
            assert_eq!(
                shell
                    .terminal
                    .read(cx)
                    .root()
                    .and_then(|root| std::fs::canonicalize(root).ok()),
                Some(second_root.clone())
            );
            assert!(shell.terminal.read(cx).is_running());
            shell.terminal.read(cx).process_id()
        });
        assert!(second_pid.is_some());
        assert_ne!(second_pid, first_pid);
        #[cfg(target_os = "linux")]
        if let Some(pid) = first_pid {
            let mut gone = false;
            for _ in 0..300 {
                if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    gone = true;
                    break;
                }
                runtime::blocking(cx, || {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                })
                .await
                .unwrap();
            }
            assert!(gone, "the previous project's shell must be terminated");
        }
    }

    /// Build's remembered layout may have the bottom dock hidden; entering
    /// Build through `ToggleTerminal` must still show it.
    #[gpui_kit::test]
    async fn toggle_terminal_enters_build_and_shows_the_dock(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Build, window, cx);
                if shell.dock.read(cx).has_dock(DockPlacement::Bottom) {
                    shell.execute(CommandId::ToggleTerminal, window, cx);
                }
                assert!(
                    !shell.dock.read(cx).has_dock(DockPlacement::Bottom),
                    "the fixture starts with Build's terminal hidden"
                );
                shell.set_mode(WorkspaceMode::Writing, window, cx);
                shell.execute(CommandId::ToggleTerminal, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.mode, WorkspaceMode::Build);
            assert!(
                shell.dock.read(cx).has_dock(DockPlacement::Bottom),
                "ToggleTerminal must show the terminal, not toggle it closed"
            );
        });

        // Idempotent show path: calling it again from Build closes the dock,
        // but does not churn the mode or reapply a layout.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::ToggleTerminal, window, cx)
            });
        })
        .unwrap();
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.mode, WorkspaceMode::Build);
            assert!(!shell.dock.read(cx).has_dock(DockPlacement::Bottom));
        });
    }

    /// In Build the bottom dock is the terminal, so `TogglePreview` must
    /// target the terminal instead of silently toggling a preview panel.
    #[gpui_kit::test]
    async fn toggle_preview_in_build_targets_the_terminal(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Build, window, cx);
                if !shell.dock.read(cx).has_dock(DockPlacement::Bottom) {
                    shell.execute(CommandId::ToggleTerminal, window, cx);
                }
                assert!(shell.dock.read(cx).has_dock(DockPlacement::Bottom));
                shell.execute(CommandId::TogglePreview, window, cx);
            });
        })
        .unwrap();
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.mode, WorkspaceMode::Build);
            assert!(
                !shell.dock.read(cx).has_dock(DockPlacement::Bottom),
                "TogglePreview in Build toggles the terminal dock"
            );
        });
        assert_eq!(
            CommandId::TogglePreview.label_in(
                &astrolith_core::preferences::CopyeditPreferences::default(),
                WorkspaceMode::Build
            ),
            "Toggle terminal"
        );
        assert_eq!(
            CommandId::TogglePreview.label_in(
                &astrolith_core::preferences::CopyeditPreferences::default(),
                WorkspaceMode::Writing
            ),
            "Toggle preview"
        );
    }

    /// Leaving Build while the terminal owns the keyboard must hand focus to
    /// the editor; otherwise every keystroke still reaches the hidden shell.
    #[gpui_kit::test]
    async fn leaving_build_moves_focus_off_the_terminal(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, settings, _) = PreferencesStore::load(global_path);
        let project = project_fixture("terminal-first");
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
            view.update(cx, |shell, cx| {
                shell.project.update(cx, |project, cx| {
                    project.open_first_entry_for_tests(window, cx)
                })
            });
        })
        .unwrap();
        let project_entity = view.read_with(cx, |shell, _| shell.project.clone());
        wait_for_active_session(cx, &project_entity).await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Build, window, cx);
                if !shell.dock.read(cx).has_dock(DockPlacement::Bottom) {
                    shell.execute(CommandId::ToggleTerminal, window, cx);
                }
                let focus = shell.terminal.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            });
            assert!(
                view.read(cx).terminal.read(cx).has_focus(window),
                "the fixture must start with terminal focus"
            );
            view.update(cx, |shell, cx| {
                shell.set_mode(WorkspaceMode::Writing, window, cx)
            });
            assert!(
                !view.read(cx).terminal.read(cx).has_focus(window),
                "focus must leave the terminal when its dock leaves the screen"
            );
            let before = view.read_with(cx, |shell, cx| {
                shell
                    .project
                    .read(cx)
                    .active_session(cx)
                    .expect("the entry stays open")
                    .read(cx)
                    .editor
                    .read(cx)
                    .value()
            });
            window.input("zz", cx);
            let after = view.read_with(cx, |shell, cx| {
                shell
                    .project
                    .read(cx)
                    .active_session(cx)
                    .expect("the entry stays open")
                    .read(cx)
                    .editor
                    .read(cx)
                    .value()
            });
            assert_ne!(
                before, after,
                "keystrokes must reach the editor after the terminal hides"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn toolbar_and_native_collapse_share_live_visibility_and_restore_document_space(
        cx: &mut TestAppContext,
    ) {
        let settings = GlobalPreferencesV2 {
            layouts: ModeLayouts {
                writing: LayoutState {
                    ..LayoutState::default()
                },
                ..ModeLayouts::default()
            },
            ..GlobalPreferencesV2::default()
        };
        let (handle, view) = mount(cx, settings);
        let initial = cx
            .update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let initial = window.find("document-panel").bounds();
                view.update(cx, |view, cx| {
                    view.execute(CommandId::ToggleSidebar, window, cx)
                });
                initial
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!view.read(cx).settings.layouts.writing.sidebar_visible);
            window.render_frame(cx);
            assert_eq!(window.find("document-panel").bounds(), initial);
            view.update(cx, |view, cx| {
                view.execute(CommandId::ToggleSidebar, window, cx)
            });
        })
        .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(view.read(cx).library_open);
            assert!(window.find("writing-library-surface").visible());
            assert_eq!(window.find("document-panel").bounds(), initial);
            view.update(cx, |view, cx| {
                view.execute(CommandId::TogglePreview, window, cx)
            });
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(view.read(cx).settings.layouts.writing.preview_visible);
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(window.find("preview-panel").visible());
        })
        .unwrap();
        let stored_height = cx.update(|cx| view.read(cx).settings.layouts.writing.preview_height);
        let opened = cx
            .update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let opened = window.find("document-panel").bounds();
                view.update(cx, |view, cx| {
                    view.execute(CommandId::TogglePreview, window, cx)
                });
                opened
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!view.read(cx).settings.layouts.writing.preview_visible);
            assert!(
                !view.read(cx).dock.read(cx).has_dock(DockPlacement::Bottom),
                "hiding the reading view must remove the dock, not leave a \
                 collapsed strip that can be dragged back"
            );
            window.render_frame(cx);
            assert!(
                window.find("document-panel").bounds().size.height > opened.size.height,
                "the document must reclaim the reading view's height"
            );
            assert!(
                window
                    .try_find("preview-panel")
                    .is_none_or(|panel| !panel.visible()),
                "no part of the reading view may stay on screen"
            );
            view.update(cx, |view, cx| {
                view.execute(CommandId::TogglePreview, window, cx)
            });
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(view.read(cx).settings.layouts.writing.preview_visible);
            window.render_frame(cx);
            assert!(view.read(cx).dock.read(cx).has_dock(DockPlacement::Bottom));
            assert_eq!(
                view.read(cx).dock.read(cx).dock_size(DockPlacement::Bottom),
                Some(px(stored_height)),
                "reopening must restore the height the reading view had"
            );
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
        for (command, expected) in [
            (CommandId::RestartPreview, "Open an Astro project"),
            (CommandId::CopyPreviewUrl, "Open a document"),
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                view.update(cx, |view, cx| view.execute(command, window, cx));
                let message = view
                    .read(cx)
                    .preview
                    .read(cx)
                    .message()
                    .unwrap_or_default()
                    .to_owned();
                assert!(
                    message.contains(expected),
                    "{command:?} must explain the requirement, was {message:?}"
                );
            })
            .unwrap();
        }
    }

    #[gpui_kit::test]
    fn splitter_drag_updates_geometry_without_repainting_the_shell(cx: &mut TestAppContext) {
        use std::rc::Rc;
        let settings = GlobalPreferencesV2 {
            default_mode: WorkspaceMode::Build,
            ..GlobalPreferencesV2::default()
        };
        let (handle, view) = mount(cx, settings);
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
                view.read(cx).settings.layouts.build.sidebar_width > 250.,
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
        let settings = GlobalPreferencesV2 {
            default_mode: WorkspaceMode::Build,
            layouts: ModeLayouts {
                build: LayoutState {
                    ..LayoutState::default()
                },
                ..ModeLayouts::default()
            },
            ..GlobalPreferencesV2::default()
        };
        let (handle, view) = mount(cx, settings);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(45.), start.y), cx);
        })
        .unwrap();
        let settings = cx.update(|cx| {
            assert!(view.read(cx).settings.layouts.build.sidebar_width > 250.);
            view.read(cx).settings.clone()
        });
        let (restored, _) = mount(cx, settings);
        cx.update_window(restored.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("navigator-panel").bounds().size.width > px(250.));
            assert!(
                window
                    .try_find("frontmatter-inspector-surface")
                    .is_none_or(|surface| !surface.visible())
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn shrinking_after_expanded_docks_preserves_document_and_panel_geometry(
        cx: &mut TestAppContext,
    ) {
        let settings = GlobalPreferencesV2 {
            default_mode: WorkspaceMode::Build,
            layouts: ModeLayouts {
                build: LayoutState {
                    ..LayoutState::default()
                },
                ..ModeLayouts::default()
            },
            ..GlobalPreferencesV2::default()
        };
        let (handle, view) = mount(cx, settings);
        cx.simulate_window_resize(handle.into(), size(px(1800.), px(1100.)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            view.update(cx, |view, cx| {
                view.execute(CommandId::TogglePreview, window, cx)
            });
            view.read(cx).dock.clone().update(cx, |dock, cx| {
                dock.set_dock_size(DockPlacement::Left, px(650.), window, cx);
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
            for id in ["navigator-panel", "terminal-panel"] {
                let bounds = window.find(id).bounds();
                assert!(bounds.left() >= px(0.) && bounds.right() <= px(800.));
                assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(600.));
            }
            assert!(view.read(cx).settings.layouts.build.sidebar_width <= 240.);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn shell_panels_stay_inside_minimum_viewport_in_both_themes(cx: &mut TestAppContext) {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let settings = GlobalPreferencesV2 {
                theme: mode,
                default_mode: WorkspaceMode::Build,
                layouts: ModeLayouts {
                    writing: LayoutState {
                        ..LayoutState::default()
                    },
                    ..ModeLayouts::default()
                },
                ..GlobalPreferencesV2::default()
            };
            let (handle, _) = mount(cx, settings);
            cx.simulate_window_resize(handle.into(), size(px(800.), px(600.)));
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                assert!(window.find("title-bar").visible());
                for id in ["navigator-panel", "document-panel"] {
                    let bounds = window.find(id).bounds();
                    assert!(bounds.size.width > px(50.) && bounds.size.height > px(100.));
                    assert!(
                        bounds.left() >= px(0.) && bounds.right() <= px(800.),
                        "{id} bounds: {bounds:?}"
                    );
                    assert!(
                        bounds.top() >= px(0.) && bounds.bottom() <= px(600.),
                        "{id} bounds: {bounds:?}"
                    );
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
        let mut settings = GlobalPreferencesV2::default();
        settings.layouts.writing.sidebar_visible = false;
        let (handle, view) = mount(cx, settings);
        cx.update_window(handle.into(), |_, window, cx| {
            let editor = cx.new(|cx| {
                gpui_kit::component::input::EditorState::new(window, cx)
                    .default_value("Start writing")
            });
            let document = view.read(cx).document.clone();
            document.update(cx, |panel, cx| {
                panel.show_editor("Welcome".into(), editor.clone(), None, cx)
            });
            let state = editor;
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
            assert!(view.read(cx).settings.layouts.writing.sidebar_visible);
            view.update(cx, |view, cx| {
                view.execute(CommandId::ToggleSidebar, window, cx)
            });
            assert!(!view.read(cx).settings.layouts.writing.sidebar_visible);
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
            shell.settings.layouts.writing.sidebar_width = 230.;
            shell.persist_global(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        assert!(
            !path.exists(),
            "dragging must not sync every intermediate width"
        );
        view.update(cx, |shell, cx| {
            shell.settings.layouts.writing.sidebar_width = 275.;
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
        assert_eq!(restored.layouts.writing.sidebar_width, 275.);
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
            preview_visible: true,
            sidebar_width: 200.,
            preview_height: 220.,
        };
        {
            let saved = saved.clone();
            seed_project(&store, project.path(), |preferences| {
                preferences
                    .layout_overrides
                    .set(WorkspaceMode::Writing, Some(saved));
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
            assert_eq!(shell.active_layout(WorkspaceMode::Writing), saved);
            let dock = shell.dock.read(cx);
            assert!(shell.library_open);
            assert!(!dock.is_dock_open(DockPlacement::Left));
            assert!(dock.is_dock_open(DockPlacement::Bottom));
            assert_eq!(dock.dock_size(DockPlacement::Bottom), Some(px(220.)));
            assert_eq!(
                shell.settings.layouts.writing,
                ModeLayouts::default().writing,
                "restoring a project layout must not rewrite the global layout"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("writing-library-surface").bounds().size.width,
                px(200.)
            );
            assert!(
                window
                    .try_find("frontmatter-inspector-surface")
                    .is_none_or(|surface| !surface.visible())
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
            preview_visible: false,
            sidebar_width: 190.,
            preview_height: 160.,
        };
        let second_layout = LayoutState {
            sidebar_visible: true,
            preview_visible: true,
            sidebar_width: 230.,
            preview_height: 240.,
        };
        {
            let layout = first_layout.clone();
            seed_project(&store, first.path(), |preferences| {
                preferences
                    .layout_overrides
                    .set(WorkspaceMode::Writing, Some(layout));
            });
        }
        {
            let layout = second_layout.clone();
            seed_project(&store, second.path(), |preferences| {
                preferences
                    .layout_overrides
                    .set(WorkspaceMode::Writing, Some(layout));
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
            assert_eq!(shell.active_layout(WorkspaceMode::Writing), first_layout);
            assert!(!shell.read_dock_layout(cx).sidebar_visible);
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
            assert_eq!(shell.active_layout(WorkspaceMode::Writing), second_layout);
            assert!(shell.read_dock_layout(cx).sidebar_visible);
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
            assert_eq!(shell.active_layout(WorkspaceMode::Writing), first_layout);
            assert!(!shell.read_dock_layout(cx).sidebar_visible);
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
            preview_visible: true,
            sidebar_width: 210.,
            preview_height: 180.,
        };
        let id = {
            let layout = saved.clone();
            seed_project(&store, project.path(), |preferences| {
                preferences
                    .layout_overrides
                    .set(WorkspaceMode::Writing, Some(layout));
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
            assert_eq!(shell.active_layout(WorkspaceMode::Writing), saved);
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
        let (store, mut settings, _) = PreferencesStore::load(global_path.clone());
        settings.default_mode = WorkspaceMode::Build;
        settings.layouts.build.sidebar_visible = true;
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
                shell.active_layout(WorkspaceMode::Build),
                shell.settings.layouts.build,
                "a project without a saved layout inherits the global layout"
            );
            assert!(
                shell
                    .project_scope
                    .as_ref()
                    .unwrap()
                    .preferences
                    .layout_overrides
                    .build
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
                .and_then(|value| value["layout_overrides"]["build"]["sidebar_width"].as_f64())
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
            project_preferences["layout_overrides"]["build"]["sidebar_visible"], true,
            "the project file must record the captured layout"
        );
        let (_, global, warning) = PreferencesStore::load(global_path);
        assert!(warning.is_none());
        assert_eq!(
            global.layouts.build.sidebar_width, 224.,
            "a project width must never be written to the global file"
        );
    }

    #[gpui_kit::test]
    async fn scoped_layout_change_leaves_the_global_layout_untouched(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let global_layout = LayoutState {
            sidebar_visible: true,
            preview_visible: false,
            sidebar_width: 300.,
            preview_height: 160.,
        };
        let second_layout = LayoutState {
            sidebar_visible: true,
            preview_visible: false,
            sidebar_width: 190.,
            preview_height: 140.,
        };
        let first = project_fixture("first-scope-site");
        let second = project_fixture("second-scope-site");
        let (store, mut settings, _) = PreferencesStore::load(global_path.clone());
        settings.default_mode = WorkspaceMode::Build;
        settings.layouts.build = global_layout.clone();
        store.save_global(1, &settings).unwrap();
        let second_id = {
            let layout = second_layout.clone();
            seed_project(&store, second.path(), |preferences| {
                preferences
                    .layout_overrides
                    .set(WorkspaceMode::Build, Some(layout));
            })
        };
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));

        open_project(cx, handle, &view, first.path().to_path_buf());
        let first_id = wait_for_project_scope(cx, &view, first.path()).await;

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("resize-handle-left").bounds().center();
            window.drag(start, point(start.x + px(36.), start.y), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        let first_file = preferences_root
            .join("projects")
            .join(format!("{first_id}.json"));
        let written = first_file.clone();
        cx.condition(&view, move |_, _| {
            std::fs::read(&written)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["layout_overrides"]["build"]["sidebar_width"].as_f64())
                .is_some_and(|width| width > 250.)
        })
        .await;

        // The change belongs to the open project alone: the global preference
        // and its file are untouched.
        view.read_with(cx, |shell, _| {
            assert_eq!(
                shell.settings.layouts.build, global_layout,
                "a project layout must never reach the global preference"
            );
            assert!(shell.active_layout(WorkspaceMode::Build).sidebar_width > 250.);
        });
        let (_, global, _) = PreferencesStore::load(global_path.clone());
        assert_eq!(global.layouts.build, global_layout);

        // The next project restores only its own file, on the same global.
        open_project(cx, handle, &view, second.path().to_path_buf());
        wait_for_project_scope(cx, &view, second.path()).await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.project_scope.as_ref().unwrap().preferences.project_id,
                second_id
            );
            assert_eq!(shell.active_layout(WorkspaceMode::Build), second_layout);
            assert_eq!(shell.settings.layouts.build, global_layout);
            assert_eq!(
                shell.dock.read(cx).dock_size(DockPlacement::Left),
                Some(px(190.))
            );
        });
        let (_, global, _) = PreferencesStore::load(global_path);
        assert_eq!(global.layouts.build, global_layout);
    }

    #[gpui_kit::test]
    fn active_layout_prefers_the_project_override(cx: &mut TestAppContext) {
        let mut settings = GlobalPreferencesV2::default();
        settings.layouts.writing = LayoutState {
            sidebar_visible: true,
            preview_visible: false,
            sidebar_width: 300.,
            preview_height: 180.,
        };
        let (_handle, view) = mount(cx, settings.clone());
        let override_layout = LayoutState {
            sidebar_visible: false,
            preview_visible: true,
            sidebar_width: 200.,
            preview_height: 220.,
        };
        view.update(cx, |shell, _| {
            assert_eq!(
                shell.active_layout(WorkspaceMode::Writing),
                settings.layouts.writing,
                "without a project the global layout applies"
            );
            shell.project_scope = Some(ProjectScope {
                root: PathBuf::from("/layout-lookup"),
                preferences: ProjectPreferencesV1::default(),
                revision: 0,
            });
            assert_eq!(
                shell.active_layout(WorkspaceMode::Writing),
                settings.layouts.writing,
                "a project without an override still inherits the global layout"
            );
            shell
                .project_scope
                .as_mut()
                .unwrap()
                .preferences
                .layout_overrides
                .set(WorkspaceMode::Writing, Some(override_layout.clone()));
            assert_eq!(shell.active_layout(WorkspaceMode::Writing), override_layout);
            assert_eq!(
                shell.active_layout(WorkspaceMode::Build),
                settings.layouts.build,
                "modes without an override keep the global layout"
            );
        });
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
                preferences.layout_overrides.set(
                    WorkspaceMode::Writing,
                    Some(LayoutState {
                        preview_visible: true,
                        ..LayoutState::default()
                    }),
                );
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
                .and_then(|value| value["layout_overrides"]["writing"]["preview_height"].as_f64())
                .is_some_and(|height| height > 190.)
        })
        .await;
        let preferences = project_file(&preferences_root, &id);
        assert_eq!(
            preferences["layout_overrides"]["writing"]["preview_visible"],
            true
        );
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
        let (store, mut settings, _) = PreferencesStore::load(global_path);
        settings.default_mode = WorkspaceMode::Build;
        settings.layouts.build.sidebar_visible = true;
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
                .and_then(|value| value["layout_overrides"]["build"]["sidebar_width"].as_f64())
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
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::ToggleSidebar, window, cx)
            });
            assert!(view.read(cx).library_open);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.condition(&view, |shell, _| {
            let persisted = std::fs::read(&global_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["layouts"]["writing"]["sidebar_visible"].as_bool());
            shell.settings.layouts.writing.sidebar_visible && persisted == Some(true)
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::ToggleSidebar, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.condition(&view, |shell, _| {
            let persisted = std::fs::read(&global_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["layouts"]["writing"]["sidebar_visible"].as_bool());
            !shell.settings.layouts.writing.sidebar_visible && persisted == Some(false)
        })
        .await;
        let (_, global, warning) = PreferencesStore::load(global_path);
        assert!(warning.is_none());
        assert!(
            !global.layouts.writing.sidebar_visible,
            "without a project the library visibility keeps persisting globally"
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
            assert!(
                window.try_find("entry-new").is_none(),
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
            project_handle.update(cx, |project, cx| {
                project.select_first_collection_for_tests(window, cx)
            });
            window.render_frame(cx);
            assert_ne!(window.find("entry-new").disabled(), Some(true));
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
            assert_ne!(window.find("entry-new").disabled(), Some(true));
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
                description: None,
                constraints: astrolith_astro::schema::FieldConstraints::default(),
                order: index,
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

    /// A real Astro project with a project component, a package catalog and
    /// both a Markdown and an MDX entry.
    fn component_shell_fixture(auto_import: bool) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("node_modules/astro")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/@astrojs/mdx")).unwrap();
        std::fs::write(
            root.join("node_modules/astro/package.json"),
            r#"{"version":"5.18.2"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("node_modules/@astrojs/mdx/package.json"),
            r#"{"version":"4.3.14","peerDependencies":{"astro":"^5.0.0"}}"#,
        )
        .unwrap();
        let dependencies = if auto_import {
            r#"{"name":"builder-site","dependencies":{"astro":"5.18.2","@astrojs/starlight":"0.42.8"},"devDependencies":{"astro-auto-import":"0.4.4"}}"#
        } else {
            r#"{"name":"builder-site","dependencies":{"astro":"5.18.2","@astrojs/starlight":"0.42.8"}}"#
        };
        std::fs::write(root.join("package.json"), dependencies).unwrap();
        std::fs::write(
            root.join("astro.config.mjs"),
            "import { defineConfig } from 'astro/config';\nimport mdx from '@astrojs/mdx';\nexport default defineConfig({ integrations: [mdx()] });\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::write(
            root.join("src/components/Callout.astro"),
            "---\n/**\n * A framed callout.\n */\ninterface Props {\n  /** Callout heading. */\n  title: string;\n  /** Visual tone. */\n  tone?: \"note\" | \"tip\";\n}\n---\n\n<section><slot /></section>\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/content")).unwrap();
        std::fs::write(
            root.join("src/content.config.ts"),
            "import { defineCollection } from 'astro:content';\nimport { glob } from 'astro/loaders';\nconst blog = defineCollection({ loader: glob({ base: './src/content', pattern: '**/*.{md,mdx}' }) });\nexport const collections = { blog };\n",
        )
        .unwrap();
        std::fs::write(root.join("src/content/first.md"), "# First\n").unwrap();
        std::fs::write(root.join("src/content/doc.mdx"), "# Document\n").unwrap();
        directory
    }

    async fn wait_for_components(cx: &mut TestAppContext, project: &Entity<ProjectWorkspace>) {
        for _ in 0..300 {
            cx.run_until_parked();
            if project.read_with(cx, |project, _| project.components_loaded()) {
                return;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        panic!("the component index never loaded");
    }

    async fn open_source(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        project: &Entity<ProjectWorkspace>,
        source: &str,
    ) -> std::path::PathBuf {
        let source = source.to_owned();
        cx.update_window(handle.into(), |_, window, cx| {
            project.update(cx, |project, cx| {
                project.reopen_last_entry(Some(&source), window, cx)
            })
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if let Some(path) = project.read_with(cx, |project, cx| {
                project
                    .active_path()
                    .filter(|_| project.active_session(cx).is_some())
                    .filter(|path| path.ends_with(&source))
            }) {
                return path;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        panic!("the session never opened for {source}");
    }

    #[gpui_kit::test]
    async fn mod_slash_comments_markdown_and_opens_the_component_builder_for_mdx(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = component_shell_fixture(false);
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        open_project(cx, handle, &view, directory.path().to_path_buf());
        let project = view.read_with(cx, |view, _| view.project.clone());
        cx.condition(&project, |project, _| !project.is_loading())
            .await;
        wait_for_components(cx, &project).await;

        open_source(cx, handle, &project, "src/content/first.md").await;
        let session = project.read_with(cx, |project, cx| project.active_session(cx).unwrap());
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..7, cx));
            });
            window.press("ctrl-/", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                !window.has_active_dialog(cx),
                "Mod+/ in Markdown must not open the MDX builder"
            );
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert!(
                session.projection.visible.contains("<!--"),
                "Mod+/ in Markdown toggles a comment: {:?}",
                session.projection.visible
            );
        });

        open_source(cx, handle, &project, "src/content/doc.mdx").await;
        let session = project.read_with(cx, |project, cx| project.active_session(cx).unwrap());
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-/", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                window.has_active_dialog(cx),
                "Mod+/ in MDX opens the component builder"
            );
            window.press("escape", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                !window.has_active_dialog(cx),
                "Esc closes the builder from the palette"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn builder_palette_groups_searches_and_warns_without_changing_the_setting(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = component_shell_fixture(true);
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        open_project(cx, handle, &view, directory.path().to_path_buf());
        let project = view.read_with(cx, |view, _| view.project.clone());
        cx.condition(&project, |project, _| !project.is_loading())
            .await;
        wait_for_components(cx, &project).await;
        open_source(cx, handle, &project, "src/content/doc.mdx").await;
        let session = project.read_with(cx, |project, cx| project.active_session(cx).unwrap());
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-/", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let builder = view.read_with(cx, |view, _| view.builder.clone());

        let rows = builder.read_with(cx, |builder, _| builder.palette_rows());
        let first = rows.first().expect("the palette has rows");
        assert_eq!(first.0, "Callout", "project components come first");
        assert_eq!(first.1, None, "a project component has no package group");
        assert_eq!(first.2, "Requires title");
        let first_package = rows
            .iter()
            .position(|row| row.1.is_some())
            .expect("package components follow the project group");
        assert!(rows[..first_package].iter().all(|row| row.1.is_none()));
        let tabs = rows
            .iter()
            .find(|row| row.0 == "Tabs")
            .expect("the Starlight catalog is indexed");
        assert_eq!(tabs.1.as_deref(), Some("@astrojs/starlight/components"));
        assert_eq!(tabs.2, "1 optional");
        assert!(
            builder.read_with(cx, |builder, _| builder.auto_import_warning().is_some()),
            "declaring astro-auto-import shows the warning"
        );

        // Search through the real palette input.
        cx.update_window(handle.into(), |_, window, cx| {
            window.input("tabs", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let hits = builder.read_with(cx, |builder, _| builder.palette_rows());
        assert_eq!(hits[0].0, "Tabs", "exact names rank first: {hits:?}");
        assert_eq!(hits[0].1.as_deref(), Some("@astrojs/starlight/components"));

        // Opening and searching never flip the per-project import opt-in.
        assert!(
            !project.read_with(cx, |project, _| {
                project
                    .component_builder_context()
                    .expect("an active document has a builder context")
                    .imports_enabled
            }),
            "the warning must not mutate the project setting"
        );
    }

    #[gpui_kit::test]
    async fn the_format_menu_command_cannot_open_the_builder_for_markdown(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let directory = component_shell_fixture(false);
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        open_project(cx, handle, &view, directory.path().to_path_buf());
        let project = view.read_with(cx, |view, _| view.project.clone());
        cx.condition(&project, |project, _| !project.is_loading())
            .await;
        wait_for_components(cx, &project).await;
        open_source(cx, handle, &project, "src/content/first.md").await;

        // The menu item is not gated before P12, so the executor must refuse a
        // Markdown document even though the command was dispatched directly.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.execute(CommandId::OpenComponentBuilder, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                !window.has_active_dialog(cx),
                "the builder must not open for Markdown"
            );
        })
        .unwrap();
        assert!(
            view.read_with(cx, |view, _| view.file_status.contains("MDX")),
            "the refusal is visible: {}",
            view.read_with(cx, |view, _| view.file_status.clone())
        );
        assert!(view.read_with(cx, |view, cx| {
            view.builder.read_with(cx, |builder, _| !builder.is_open())
        }));
    }

    #[gpui_kit::test]
    async fn builder_opened_while_indexing_refreshes_when_the_index_publishes(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = component_shell_fixture(false);
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        open_project(cx, handle, &view, directory.path().to_path_buf());
        let project = view.read_with(cx, |view, _| view.project.clone());
        cx.condition(&project, |project, _| !project.is_loading())
            .await;
        wait_for_components(cx, &project).await;
        open_source(cx, handle, &project, "src/content/doc.mdx").await;

        // Open the builder with the context it would have had mid-walk.
        let builder = view.read_with(cx, |view, _| view.builder.clone());
        let loading = BuilderContext {
            index: Arc::new(ComponentIndex::default()),
            imports_enabled: false,
            references: ReferenceCatalog::new(),
            images: None,
            loading: true,
            error: None,
        };
        cx.update_window(handle.into(), |_, window, cx| {
            builder.update(cx, |builder, cx| builder.open(loading, window, cx));
        })
        .unwrap();
        assert!(
            builder.read_with(cx, |builder, _| builder.palette_rows().is_empty()),
            "a mid-walk context starts empty"
        );

        // The project's publication notification refreshes the open palette.
        cx.update(|cx| project.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        let rows = builder.read_with(cx, |builder, _| builder.palette_rows());
        assert!(
            rows.iter().any(|row| row.0 == "Callout"),
            "the published index replaces the loading palette: {rows:?}"
        );
        assert!(
            !builder.read_with(cx, |builder, _| builder.is_loading()),
            "the loading note is cleared once the index publishes"
        );
    }

    #[gpui_kit::test]
    async fn builder_escape_returns_to_the_palette_and_ctrl_enter_inserts(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let directory = component_shell_fixture(false);
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        open_project(cx, handle, &view, directory.path().to_path_buf());
        let project = view.read_with(cx, |view, _| view.project.clone());
        cx.condition(&project, |project, _| !project.is_loading())
            .await;
        wait_for_components(cx, &project).await;
        open_source(cx, handle, &project, "src/content/doc.mdx").await;
        let session = project.read_with(cx, |project, cx| project.active_session(cx).unwrap());
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-/", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let builder = view.read_with(cx, |view, _| view.builder.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.input("callout", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("enter", cx);
        })
        .unwrap();
        assert!(
            builder.read_with(cx, |builder, _| builder.stage_is_form()),
            "Enter opens the highlighted component's form"
        );

        // Esc goes back to the palette without closing the dialog.
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("escape", cx);
        })
        .unwrap();
        assert!(!builder.read_with(cx, |builder, _| builder.stage_is_form()));
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(window.has_active_dialog(cx));
            window.press("enter", cx);
        })
        .unwrap();
        assert!(builder.read_with(cx, |builder, _| builder.stage_is_form()));
        let preview = builder
            .read_with(cx, |builder, _| builder.preview_tag())
            .expect("the form previews its tag");
        assert!(preview.starts_with("<Callout"), "{preview:?}");

        // Ctrl+Enter inserts and closes the dialog.
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("ctrl-enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!window.has_active_dialog(cx));
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert!(
                session
                    .buffer
                    .current_text()
                    .contains("<Callout title=\"\">"),
                "Ctrl+Enter inserts the tag: {:?}",
                session.buffer.current_text()
            );
        });
    }
    fn open_preferences_sheet(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<Shell>,
    ) {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::OpenPreferences, window, cx)
            });
            window.render_frame(cx);
        })
        .unwrap();
        // The sheet slides in; complete the entrance before querying content.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                window.has_active_dialog(cx),
                "Ctrl+, opens the preferences dialog"
            );
            window.render_frame(cx);
        })
        .unwrap();
    }

    async fn wait_for_project_scope(
        cx: &mut TestAppContext,
        view: &Entity<Shell>,
        root: &std::path::Path,
    ) -> String {
        let canonical = std::fs::canonicalize(root).unwrap();
        cx.condition(view, move |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.root == canonical)
        })
        .await;
        view.read_with(cx, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .unwrap()
                .preferences
                .project_id
                .clone()
        })
    }

    #[gpui_kit::test]
    async fn preferences_shortcut_opens_one_sheet_and_project_panes_need_a_project(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let (store, settings, _) =
            PreferencesStore::load(workspace.path().join("preferences/global.json"));
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("ctrl-,", cx);
        })
        .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(window.has_active_dialog(cx));
        })
        .unwrap();
        assert_eq!(
            sheet.read_with(cx, |sheet, _| sheet.visible_panes()),
            [
                PreferencesPane::Appearance,
                PreferencesPane::Workspace,
                PreferencesPane::Preview,
                PreferencesPane::Advanced
            ]
        );
        assert_eq!(
            sheet.read_with(cx, |sheet, _| sheet.active_pane()),
            PreferencesPane::Appearance
        );

        // Escape dismisses the sheet without applying anything.
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("escape", cx);
            assert!(!window.has_active_dialog(cx));
        })
        .unwrap();

        let project = project_fixture("prefs-panes");
        open_project(cx, handle, &view, project.path().to_path_buf());
        wait_for_project_scope(cx, &view, project.path()).await;
        open_preferences_sheet(cx, handle, &view);
        assert_eq!(
            sheet.read_with(cx, |sheet, _| sheet.visible_panes()),
            [
                PreferencesPane::Appearance,
                PreferencesPane::Workspace,
                PreferencesPane::Preview,
                PreferencesPane::Project,
                PreferencesPane::Collections,
                PreferencesPane::Advanced,
            ]
        );
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("escape", cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn startup_restores_the_default_mode_layout_typography_and_writing_modes(
        cx: &mut TestAppContext,
    ) {
        use astrolith_core::preferences::ThemePreference;

        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        std::fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        std::fs::write(
            &global_path,
            r#"{
                "version": 2,
                "theme": "dark",
                "default_mode": "build",
                "layouts": {
                    "writing": {"sidebar_visible": true, "preview_visible": false, "sidebar_width": 10000.0, "preview_height": 10000.0},
                    "build": {"sidebar_visible": false, "preview_visible": true, "sidebar_width": 10000.0, "preview_height": 10000.0},
                    "preview": {"sidebar_visible": false, "preview_visible": true, "sidebar_width": 224.0, "preview_height": 160.0}
                },
                "editor_font_size": 20.0,
                "editor_line_height": 2.4,
                "page_width_percent": 75.0,
                "focus_mode": true,
                "typewriter_mode": true,
                "preview": {
                    "presentation": "external",
                    "browser": {"executable": "", "arguments": []},
                    "open_on_enter": false
                }
            }"#,
        )
        .unwrap();
        let (_store, settings, warning) = PreferencesStore::load(global_path);
        assert!(warning.is_none());
        let (handle, view) = mount(cx, settings);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();

        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.mode, WorkspaceMode::Build);
            assert_eq!(shell.settings.theme, ThemePreference::Dark);
            assert_eq!(shell.settings.editor_font_size, 20.);
            assert_eq!(shell.settings.editor_line_height, 2.4);
            assert_eq!(shell.settings.page_width_percent, 75.);
            assert!(!shell.settings.preview.open_on_enter);
            assert!(!shell.settings.preview.browser.is_configured());
            // Stored sizes larger than the window can host are clamped on load.
            assert_eq!(shell.settings.layouts.build.sidebar_width, 390.);
            assert_eq!(shell.settings.layouts.build.preview_height, 400.);
            assert!(!shell.dock.read(cx).is_dock_open(DockPlacement::Left));
            assert!(shell.dock.read(cx).is_dock_open(DockPlacement::Bottom));
            assert_eq!(
                shell.dock.read(cx).dock_size(DockPlacement::Bottom),
                Some(px(400.))
            );
            let modes = shell.project.read(cx).writing_modes();
            assert!(modes.focus);
            assert!(modes.typewriter);
            assert!(
                !shell.inspector_open,
                "the transient inspector never restores"
            );
        });
    }

    #[gpui_kit::test]
    async fn workspace_settings_apply_the_default_mode_and_reset_one_layout_scope(
        cx: &mut TestAppContext,
    ) {
        use std::time::Duration;

        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let global_path = workspace.path().join("preferences/global.json");
        let (store, mut settings, _) = PreferencesStore::load(global_path.clone());
        settings.layouts.writing = PanelLayout {
            sidebar_visible: true,
            preview_visible: false,
            sidebar_width: 300.,
            preview_height: 160.,
        };
        store.save_global(1, &settings).unwrap();
        // Reload so the shell starts from the stored revision, like a restart.
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        assert!(view.read_with(cx, |shell, cx| {
            shell.library_open && !shell.dock.read(cx).is_dock_open(DockPlacement::Left)
        }));

        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_pane_for_tests(PreferencesPane::Workspace, cx);
                sheet.set_default_mode_for_tests(WorkspaceMode::Build, cx);
                sheet.toggle_layout_reset_for_tests(WorkspaceMode::Writing, cx);
            });
            window.render_frame(cx);
            assert!(window.find("preferences-field-layout-writing").visible());
            window.click("preferences-apply", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.condition(&view, |shell, _| {
            shell.settings.default_mode == WorkspaceMode::Build
                && shell.settings.layouts.writing == ModeLayouts::default().writing
        })
        .await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.settings.layouts.writing,
                ModeLayouts::default().writing,
                "the resolved Writing layout follows the reset global default"
            );
            assert!(
                !shell.dock.read(cx).is_dock_open(DockPlacement::Left),
                "the reset layout is applied to the live dock"
            );
        });
        let written = global_path.clone();
        cx.condition(&view, move |_, _| {
            std::fs::read(&written)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|value| {
                    value["default_mode"] == "build"
                        && value["layouts"]["writing"]["sidebar_width"] == 224.0
                })
        })
        .await;
    }

    #[gpui_kit::test]
    async fn project_layout_reset_clears_overrides_while_global_defaults_stay(
        cx: &mut TestAppContext,
    ) {
        use std::time::Duration;

        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let project = project_fixture("layout-reset-site");
        let override_layout = LayoutState {
            sidebar_visible: false,
            preview_visible: true,
            sidebar_width: 200.,
            preview_height: 220.,
        };
        let id = {
            let (store, settings, _) = PreferencesStore::load(global_path.clone());
            store.save_global(1, &settings).unwrap();
            let layout = override_layout.clone();
            seed_project(&store, project.path(), |preferences| {
                preferences
                    .layout_overrides
                    .set(WorkspaceMode::Writing, Some(layout));
            })
        };
        let (store, settings, _) = PreferencesStore::load(global_path);
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        open_project(cx, handle, &view, project.path().to_path_buf());
        wait_for_project_scope(cx, &view, project.path()).await;
        view.read_with(cx, |shell, cx| {
            assert_eq!(shell.active_layout(WorkspaceMode::Writing), override_layout);
            assert!(shell.dock.read(cx).is_dock_open(DockPlacement::Bottom));
        });

        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_pane_for_tests(PreferencesPane::Workspace, cx);
                sheet.set_project_layout_reset_for_tests(true, cx);
            });
            window.render_frame(cx);
            window.click("preferences-apply", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.condition(&view, |shell, _| {
            shell
                .project_scope
                .as_ref()
                .is_some_and(|scope| scope.preferences.layout_overrides == Default::default())
        })
        .await;
        view.read_with(cx, |shell, _| {
            assert_eq!(
                shell.active_layout(WorkspaceMode::Writing),
                shell.settings.layouts.writing,
                "the project now inherits every global layout"
            );
        });
        let written = preferences_root.join("projects").join(format!("{id}.json"));
        cx.condition(&view, move |_, _| {
            std::fs::read(&written)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|value| {
                    value["layout_overrides"]["writing"].is_null()
                        && value["layout_overrides"]["build"].is_null()
                })
        })
        .await;
    }

    #[gpui_kit::test]
    async fn preferences_sheet_names_controls_and_fits_the_minimum_window(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let settings = GlobalPreferencesV2 {
            ui_font_size: 28.,
            ..GlobalPreferencesV2::default()
        };
        let (handle, view) = mount_sized(cx, settings, 1000., 700.);
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("open-preferences").label(),
                Some(CommandId::OpenPreferences.label())
            );
            assert_eq!(
                window.find("command-palette").label(),
                Some(CommandId::OpenPalette.label())
            );
            assert_eq!(
                window.find("view-menu").label(),
                Some("Panels and writing modes")
            );
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |shell, _| shell.settings.ui_font_size),
            28.
        );
        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, _cx| {
            let viewport = window.viewport_size();
            for id in ["preferences-apply", "preferences-cancel"] {
                let bounds = window.find(id).bounds();
                assert!(
                    bounds.origin.x >= px(0.)
                        && bounds.origin.y >= px(0.)
                        && bounds.origin.x + bounds.size.width <= viewport.width
                        && bounds.origin.y + bounds.size.height <= viewport.height,
                    "{id} is clipped at {:?} in {:?}",
                    bounds,
                    viewport
                );
            }
            for pane in PreferencesPane::ALL {
                if !pane.available(false) {
                    continue;
                }
                let snapshot = window.find(pane.element_id());
                assert_eq!(snapshot.label(), Some(pane.label()));
                assert!(snapshot.visible(), "{} is not visible", pane.label());
            }
        })
        .unwrap();
        assert_eq!(
            sheet.read_with(cx, |sheet, _| sheet.active_pane()),
            PreferencesPane::Appearance
        );

        // Tab order is navigation → body → actions, not the visual order of
        // the action row.
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("tab", cx);
            assert_eq!(
                window.find("preferences-pane-appearance").focused(),
                Some(true)
            );
        })
        .unwrap();
        let _ = view;
    }

    #[gpui_kit::test]
    async fn preferences_apply_persists_both_scopes_and_cancel_changes_nothing(
        cx: &mut TestAppContext,
    ) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let project = project_fixture("preferences-site");
        {
            let (store, settings, _) = PreferencesStore::load(global_path.clone());
            store.save_global(1, &settings).unwrap();
            seed_project(&store, project.path(), |preferences| {
                preferences.content_directory = Some("src/content".into());
            });
        }
        let (store, settings, _) = PreferencesStore::load(global_path.clone());
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        open_project(cx, handle, &view, project.path().to_path_buf());
        let id = wait_for_project_scope(cx, &view, project.path()).await;

        let interface_font = cx.update(|cx| {
            cx.text_system()
                .all_font_names()
                .into_iter()
                .next()
                .unwrap_or_default()
        });
        open_preferences_sheet(cx, handle, &view);
        // An out-of-range font size blocks the whole apply.
        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_text_for_tests(FieldId::FontSize, "31", window, cx)
            });
            window.render_frame(cx);
            assert_eq!(window.find("preferences-apply").disabled(), Some(true));
        })
        .unwrap();
        assert!(sheet.read_with(cx, |sheet, _| sheet.has_errors()));

        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_text_for_tests(FieldId::FontSize, "22", window, cx);
                sheet.set_text_for_tests(FieldId::UiFontFamily, &interface_font, window, cx);
                sheet.set_text_for_tests(FieldId::UiFontSize, "16", window, cx);
                sheet.set_text_for_tests(FieldId::BuildFontFamily, &interface_font, window, cx);
                sheet.set_text_for_tests(FieldId::BuildFontSize, "18", window, cx);
                sheet.set_text_for_tests(FieldId::PreviewFontFamily, &interface_font, window, cx);
                sheet.set_text_for_tests(FieldId::PreviewFontSize, "20", window, cx);
                sheet.set_text_for_tests(FieldId::ProjectContent, "src/entries", window, cx);
            });
            window.render_frame(cx);
            assert_ne!(window.find("preferences-apply").disabled(), Some(true));
            window.click("preferences-apply", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.condition(&view, |shell, _| {
            shell.settings.editor_font_size == 22.
                && shell.settings.ui_font_size == 16.
                && shell.settings.ui_font_family == interface_font
                && shell.settings.build_font_family == interface_font
                && shell.settings.build_font_size == 18.
                && shell.settings.preview_font_family == interface_font
                && shell.settings.preview_font_size == 20.
        })
        .await;
        let written = global_path.clone();
        cx.condition(&view, move |_, _| {
            std::fs::read(&written)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["editor_font_size"].as_f64())
                .is_some_and(|size| size == 22.)
        })
        .await;
        let global = std::fs::read(&global_path).unwrap();
        let global: serde_json::Value = serde_json::from_slice(&global).unwrap();
        assert_eq!(global["editor_font_size"], 22.0);
        assert_eq!(global["ui_font_size"], 16.0);
        assert_eq!(global["ui_font_family"], interface_font);
        assert_eq!(global["build_font_family"], interface_font);
        assert_eq!(global["build_font_size"], 18.0);
        assert_eq!(global["preview_font_family"], interface_font);
        assert_eq!(global["preview_font_size"], 20.0);
        let project = project_file(&preferences_root, &id);
        assert_eq!(project["content_directory"], "src/entries");

        // Reopen, change a value, then Cancel: neither scope moves.
        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_text_for_tests(FieldId::FontSize, "25", window, cx);
                sheet.set_text_for_tests(FieldId::ProjectContent, "src/other", window, cx);
            });
            window.render_frame(cx);
            window.click("preferences-cancel", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |shell, _| shell.settings.editor_font_size),
            22.
        );
        let project = project_file(&preferences_root, &id);
        assert_eq!(project["content_directory"], "src/entries");
        let global = std::fs::read(&global_path).unwrap();
        let global: serde_json::Value = serde_json::from_slice(&global).unwrap();
        assert_eq!(global["editor_font_size"], 22.0);
    }

    #[gpui_kit::test]
    async fn preferences_reset_control_removes_only_its_own_value(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let workspace = tempfile::tempdir().unwrap();
        let preferences_root = workspace.path().join("preferences");
        let global_path = preferences_root.join("global.json");
        let project = project_fixture("preferences-reset");
        {
            let (store, settings, _) = PreferencesStore::load(global_path.clone());
            store.save_global(1, &settings).unwrap();
            seed_project(&store, project.path(), |preferences| {
                preferences.content_directory = Some("src/content".into());
                preferences.assets_directory = Some("public/images".into());
                preferences.default_extension = Some(FileExtension::Mdx);
            });
        }
        let (store, settings, _) = PreferencesStore::load(global_path);
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        open_project(cx, handle, &view, project.path().to_path_buf());
        let id = wait_for_project_scope(cx, &view, project.path()).await;
        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_pane_for_tests(PreferencesPane::Project, cx)
            });
            window.render_frame(cx);
            window.click("preferences-reset-project-content", cx);
            window.render_frame(cx);
            window.click("preferences-apply", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        let written = preferences_root.join("projects").join(format!("{id}.json"));
        cx.condition(&view, move |_, _| {
            std::fs::read(&written)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|value| value["content_directory"].is_null())
        })
        .await;
        let project = project_file(&preferences_root, &id);
        assert!(project["content_directory"].is_null());
        assert_eq!(project["assets_directory"], "public/images");
        assert_eq!(project["default_extension"], "mdx");
    }

    #[gpui_kit::test]
    async fn auto_import_reminder_shows_without_changing_the_setting(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let project = component_shell_fixture(true);
        let workspace = tempfile::tempdir().unwrap();
        let (store, settings, _) =
            PreferencesStore::load(workspace.path().join("preferences/global.json"));
        let (handle, view) = mount(cx, settings);
        view.update(cx, |shell, _| shell.store = Some(Arc::new(store)));
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        open_project(cx, handle, &view, project.path().to_path_buf());
        wait_for_project_scope(cx, &view, project.path()).await;
        let project_handle = view.read_with(cx, |shell, _| shell.project.clone());
        wait_for_components(cx, &project_handle).await;
        assert!(project_handle.read_with(cx, |project, _| project.auto_import_declared()));

        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_pane_for_tests(PreferencesPane::Project, cx)
            });
            window.render_frame(cx);
            assert!(
                window.try_find("preferences-auto-import-notice").is_some(),
                "the reminder is visible in the project pane"
            );
        })
        .unwrap();
        assert!(sheet.read_with(cx, |sheet, _| sheet.context_auto_import()));
        assert_eq!(
            sheet.read_with(cx, |sheet, _| sheet.project_insert_imports()),
            None,
            "the reminder never changes the user's setting"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("escape", cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn appearance_commands_apply_the_neutral_palette_and_persist(cx: &mut TestAppContext) {
        use crate::theme::ResolveTheme;

        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        for (command, mode) in [
            (CommandId::LightTheme, ThemeMode::Light),
            (CommandId::DarkTheme, ThemeMode::Dark),
            (CommandId::SystemTheme, ThemeMode::System),
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                view.update(cx, |shell, cx| shell.execute(command, window, cx))
            })
            .unwrap();
            let (settings_theme, resolved) = view.read_with(cx, |shell, cx| {
                (shell.settings.theme, mode.resolve(cx.window_appearance()))
            });
            assert_eq!(settings_theme, mode);
            let tokens = theme::palette(resolved);
            cx.update(|cx| {
                assert_eq!(cx.theme().background, tokens.background);
                assert_eq!(cx.theme().primary, tokens.primary);
                assert_eq!(cx.theme().ring, tokens.focus);
            });
        }
    }

    #[gpui_kit::test]
    async fn preferences_heading_colour_applies_live_and_reset_restores_the_token(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.execute(CommandId::LightTheme, window, cx)
            });
        })
        .unwrap();
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, cx| {
            sheet.update(cx, |sheet, cx| {
                sheet.set_text_for_tests(FieldId::HeadingDark, "#112233", window, cx);
                sheet.set_text_for_tests(FieldId::HeadingLight, "#445566", window, cx);
            });
            window.render_frame(cx);
            window.click("preferences-apply", cx);
        })
        .unwrap();
        let expected = theme::color_from_hex("#445566").unwrap();
        assert_eq!(
            view.read_with(cx, |_, cx| theme::prose_heading_color(cx)),
            expected,
            "the light heading colour is live after Apply"
        );

        // Reset removes only the light heading colour.
        open_preferences_sheet(cx, handle, &view);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            sheet.update(cx, |sheet, cx| {
                sheet.reset_field(FieldId::HeadingLight, window, cx)
            });
            window.render_frame(cx);
            window.click("preferences-apply", cx);
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |_, cx| theme::prose_heading_color(cx)),
            view.read_with(cx, |_, cx| cx.theme().foreground),
            "reset restores the theme heading token"
        );
    }

    fn registry_store() -> (tempfile::TempDir, Arc<PreferencesStore>) {
        let workspace = tempfile::tempdir().unwrap();
        let (store, settings, _) =
            PreferencesStore::load(workspace.path().join("preferences/global.json"));
        store.save_global(1, &settings).unwrap();
        (workspace, Arc::new(store))
    }

    #[gpui_kit::test]
    async fn startup_reopens_the_most_recent_project_without_arguments(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (_workspace, store) = registry_store();
        let older = project_fixture("older-site");
        let newer = project_fixture("newer-site");
        store
            .record_project_open(older.path(), Some("older-site"), 10)
            .unwrap();
        store
            .record_project_open(newer.path(), Some("newer-site"), 20)
            .unwrap();
        let expected = newer.path().canonicalize().unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        view.update(cx, |shell, _| shell.store = Some(store));
        let project = view.read_with(cx, |shell, _| shell.project.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.begin(StartupRequest::default(), None, window, cx)
            });
        })
        .unwrap();
        cx.condition(&project, |project, _| project.project_root().is_some())
            .await;
        assert_eq!(
            project.read_with(cx, |project, _| project.project_root()),
            Some(expected)
        );
    }

    #[gpui_kit::test]
    async fn a_startup_entry_wins_over_the_last_project(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (_workspace, store) = registry_store();
        let older = project_fixture("older-site");
        let newer = project_fixture("newer-site");
        store
            .record_project_open(older.path(), Some("older-site"), 10)
            .unwrap();
        store
            .record_project_open(newer.path(), Some("newer-site"), 20)
            .unwrap();
        let entry = newer.path().join("src/content/second.md");
        let entry = entry.canonicalize().unwrap();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        view.update(cx, |shell, _| shell.store = Some(store));
        let project = view.read_with(cx, |shell, _| shell.project.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.begin(
                    StartupRequest {
                        target: Some(StartupTarget::Entry(entry.clone())),
                        problem: None,
                    },
                    None,
                    window,
                    cx,
                )
            });
        })
        .unwrap();
        cx.condition(&project, |project, _| {
            project.active_path().as_ref() == Some(&entry)
        })
        .await;
        assert_eq!(
            project.read_with(cx, |project, _| project.project_root()),
            Some(newer.path().canonicalize().unwrap())
        );
    }

    #[gpui_kit::test]
    async fn a_protected_startup_root_is_refused_before_it_opens(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let project = view.read_with(cx, |shell, _| shell.project.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.begin(
                    StartupRequest {
                        target: Some(StartupTarget::Project(PathBuf::from("/etc"))),
                        problem: None,
                    },
                    None,
                    window,
                    cx,
                )
            });
        })
        .unwrap();
        cx.condition(&view, |shell, _| shell.warning.is_some())
            .await;
        assert!(
            view.read_with(cx, |shell, _| shell.warning.clone())
                .unwrap()
                .contains("System directories")
        );
        assert_eq!(
            project.read_with(cx, |project, _| project.project_root()),
            None
        );
    }

    #[gpui_kit::test]
    async fn recent_project_actions_open_and_remove_through_the_registry(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (_workspace, store) = registry_store();
        let fixture = project_fixture("action-site");
        let open = store
            .record_project_open(fixture.path(), Some("action-site"), 10)
            .unwrap();
        let records = store.recent_projects().0;
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        view.update(cx, |shell, _| {
            shell.store = Some(store.clone());
            shell.set_registry(records);
        });
        let project = view.read_with(cx, |shell, _| shell.project.clone());
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.dispatch_action(
            handle.into(),
            commands::OpenRecentProject {
                id: open.id.clone(),
            },
        );
        cx.condition(&project, |project, _| project.project_root().is_some())
            .await;
        assert_eq!(
            project.read_with(cx, |project, _| project.project_root()),
            Some(fixture.path().canonicalize().unwrap())
        );

        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.dispatch_action(
            handle.into(),
            commands::RemoveRecentProject {
                id: open.id.clone(),
            },
        );
        cx.condition(&view, |shell, _| shell.recent.is_empty())
            .await;
        assert!(store.recent_projects().0.is_empty());
    }

    #[gpui_kit::test]
    fn about_is_reachable_from_its_command_id(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| shell.execute(CommandId::About, window, cx));
            assert!(window.has_active_dialog(cx));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn help_surfaces_open_from_their_command_ids(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            for command in [
                CommandId::OpenUserGuide,
                CommandId::OpenKeyboardShortcuts,
                CommandId::OpenLocalData,
            ] {
                view.update(cx, |shell, cx| shell.execute(command, window, cx));
                assert!(
                    window.has_active_dialog(cx),
                    "{} did not open a dialog",
                    command.label()
                );
                window.close_all_dialogs(cx);
                assert!(!window.has_active_dialog(cx));
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn preferences_documentation_links_open_help(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let sheet = view.read_with(cx, |shell, _| shell.preferences_sheet.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| shell.open_preferences(window, cx));
            assert!(window.has_active_dialog(cx));
            sheet.update(cx, |_, cx| cx.emit(PreferencesEvent::OpenUserGuide));
            assert!(window.has_active_dialog(cx));
            window.close_all_dialogs(cx);

            view.update(cx, |shell, cx| shell.open_preferences(window, cx));
            sheet.update(cx, |_, cx| cx.emit(PreferencesEvent::OpenKeyboardShortcuts));
            assert!(window.has_active_dialog(cx));
            window.close_all_dialogs(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn shell_drop_rejects_multiple_and_mixed_drops_visibly(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("note.md");
        std::fs::write(&file, "# note\n").unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.drop_paths(
                    vec![directory.path().to_path_buf(), file.clone()],
                    window,
                    cx,
                )
            });
        })
        .unwrap();
        assert!(
            view.read_with(cx, |shell, _| shell.warning.clone())
                .unwrap()
                .contains("exactly one")
        );
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.drop_paths(vec![file.clone()], window, cx)
            });
        })
        .unwrap();
        assert!(
            view.read_with(cx, |shell, _| shell.warning.clone())
                .unwrap()
                .contains("Open the project first")
        );
    }

    #[gpui_kit::test]
    async fn palette_rows_include_registry_generated_recent_and_collection_commands(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        let fixture = project_fixture("palette-site");
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        let record = ProjectRecord {
            id: "recent-one".into(),
            path: fixture.path().display().to_string(),
            display_name: "Palette Site".into(),
            package_name: Some("palette-site".into()),
            last_opened_at: 1,
        };
        view.update(cx, |shell, _| shell.set_registry(vec![record]));
        open_project(cx, handle, &view, fixture.path().to_path_buf());
        let project = view.read_with(cx, |shell, _| shell.project.clone());
        cx.condition(&project, |project, _| project.project_root().is_some())
            .await;
        let groups = cx
            .update_window(handle.into(), |_, window, cx| {
                view.update(cx, |shell, cx| shell.palette_groups(window, cx))
            })
            .unwrap();
        let recent = groups
            .iter()
            .find(|(title, _)| *title == "Recent projects")
            .expect("recent rows are grouped");
        assert!(recent.1.iter().any(|entry| {
            entry.label == "Open Palette Site"
                && matches!(&entry.action, PaletteAction::RecentProject(id) if id == "recent-one")
        }));
        let collections = groups
            .iter()
            .find(|(title, _)| *title == "Collections")
            .expect("collection rows are generated from the live index");
        assert!(collections.1.iter().any(|entry| {
            entry.label == "Open Collection: blog"
                && matches!(&entry.action, PaletteAction::Collection(id) if id == "blog")
        }));
        assert!(groups.iter().any(|(title, entries)| {
            *title == "File"
                && entries.iter().any(|entry| {
                    entry.label == "Reload collections"
                        && matches!(
                            entry.action,
                            PaletteAction::Command(CommandId::ReloadCollections)
                        )
                })
        }));
    }

    #[gpui_kit::test]
    async fn file_status_surfaces_a_failure_in_the_window(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (handle, view) = mount(cx, GlobalPreferencesV2::default());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("file-status").is_none(),
                "a healthy window keeps no status line"
            );
            view.update(cx, |shell, cx| {
                shell.file_status = "Could not open file: permissions".into();
                cx.notify();
            });
            window.render_frame(cx);
            assert!(
                window.try_find("file-status").is_some(),
                "a stored open/scan failure must be visible"
            );
        })
        .unwrap();
    }
}
