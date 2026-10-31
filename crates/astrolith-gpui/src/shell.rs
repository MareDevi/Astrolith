use std::{
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

use crate::{
    commands::{self, COMMANDS, CommandId},
    editor::session::SessionChanged,
    file::{DocumentSource, read_document},
    frontmatter::{FrontmatterChanged, FrontmatterForm},
    layout::{
        panels::WorkspacePanel,
        state::{LayoutState, PanelKind},
    },
    preview::PreviewPanel,
    project_discovery::ProjectWorkspace,
    runtime,
    settings::{Settings, SettingsStore, settings_path},
    theme::{self, ThemeMode},
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    command::{Command, CommandItem, CommandState},
    dock::{DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, panel_handle},
};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, PathPromptOptions,
    Render, Subscription, Task, Window, WindowOptions, div, prelude::*, px, size,
};

type LatestSettings = Arc<Mutex<(u64, Settings)>>;

/// Open the shell after background preference loading; bootstrap owns no services.
pub fn open(cx: &mut App) {
    if let Err(error) = runtime::init(cx) {
        eprintln!("Could not start background services: {error}");
        cx.quit();
        return;
    }
    commands::bind_keys(cx);
    cx.spawn(async move |cx| {
        let loaded = runtime::blocking(cx, || {
            settings_path().map(|path| {
                let (store, settings, warning) = SettingsStore::load(path);
                (Arc::new(store), settings, warning)
            })
        })
        .await
        .map_err(std::io::Error::other)
        .and_then(|result| result);
        cx.update(|cx| {
            let (store, settings, warning) = match loaded {
                Ok((store, settings, warning)) => (Some(store), settings, warning),
                Err(error) => (
                    None,
                    Settings::default(),
                    Some(format!("Preferences unavailable: {error}")),
                ),
            };
            if let Err(error) = gpui_kit::open_window(
                WindowOptions {
                    window_min_size: Some(size(px(800.), px(600.))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    window.set_window_title("Astrolith");
                    cx.new(|cx| Shell::new(settings, store, warning, window, cx))
                },
            ) {
                eprintln!("Could not open Astrolith: {error:#}");
                cx.quit();
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
    _skin: Rc<DockSkin>,
    settings: Settings,
    store: Option<Arc<SettingsStore>>,
    latest: LatestSettings,
    revision: u64,
    settings_task: Option<Task<()>>,
    warning: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    fn new(
        mut settings: Settings,
        store: Option<Arc<SettingsStore>>,
        warning: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        settings
            .layout
            .clamp(f32::from(window.viewport_size().width));
        theme::apply(settings.theme, window.appearance(), cx);
        let navigator = cx.new(|cx| {
            WorkspacePanel::new(
                "navigator-panel",
                "Files",
                "No file open\n\nOpen a local document to view its source.",
                window,
                cx,
            )
        });
        let document = cx.new(|cx| WorkspacePanel::new("document-panel", "Welcome", "# Astrolith\n\nOpen an Astro project to edit Markdown and MDX.\n\nPress Ctrl/Cmd+S to save the active document.\n\n中文 · 日本語", window, cx));
        let project = cx.new(|cx| ProjectWorkspace::new(document.clone(), window, cx));
        navigator.update(cx, |navigator, cx| {
            navigator.set_view(project.clone().into(), cx)
        });
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
                gpui_kit::rems(10.).to_pixels(window.rem_size()),
                window,
                cx,
            );
        });
        let palette = cx.new(|cx| CommandState::new(window, cx));
        let latest = Arc::new(Mutex::new((0, settings.clone())));
        let mut subscriptions = vec![
            cx.subscribe_in(&dock, window, |this, _, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.constrain_docks(window, cx);
                    this.capture_layout(cx);
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
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
                    if let Some((revision, settings)) = snapshot
                        && let Err(error) = store.save(revision, &settings)
                    {
                        eprintln!("Could not save preferences on exit: {error}");
                    }
                });
                cx.background_executor().spawn(async move {
                    if let Err(error) = flush.await {
                        eprintln!("Could not finish preference flush: {error}");
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
        subscriptions.push(cx.observe(&dock, |_, _, cx| cx.notify()));
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
            _skin: skin,
            settings,
            store,
            latest,
            revision: 0,
            settings_task: None,
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
        self.dock.update(cx, |dock, cx| {
            for (placement, maximum) in [
                (DockPlacement::Left, side_maximum),
                (DockPlacement::Right, side_maximum),
                (DockPlacement::Bottom, bottom_maximum),
            ] {
                if let Some(current) = dock.dock_size(placement) {
                    let minimum = if placement == DockPlacement::Bottom {
                        80.
                    } else {
                        160.
                    };
                    let constrained = f32::from(current).clamp(minimum, maximum);
                    if current != px(constrained) {
                        dock.set_dock_size(placement, px(constrained), window, cx);
                    }
                }
            }
        });
    }

    fn capture_layout(&mut self, cx: &mut Context<Self>) {
        let dock = self.dock.read(cx);
        let layout = LayoutState {
            sidebar_visible: dock.is_dock_open(DockPlacement::Left),
            inspector_visible: dock.is_dock_open(DockPlacement::Right),
            preview_visible: dock.is_dock_open(DockPlacement::Bottom),
            sidebar_width: dock
                .dock_size(DockPlacement::Left)
                .map(f32::from)
                .unwrap_or(224.),
            inspector_width: dock
                .dock_size(DockPlacement::Right)
                .map(f32::from)
                .unwrap_or(224.),
        };
        if layout != self.settings.layout {
            self.settings.layout = layout;
            self.persist(cx);
        }
        cx.notify();
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        let revision = self.revision;
        let settings = self.settings.clone();
        if let Ok(mut latest) = self.latest.lock() {
            *latest = (revision, settings.clone());
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_millis(250));
        self.settings_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let result = runtime::blocking(cx, move || store.save(revision, &settings))
                .await
                .map_err(std::io::Error::other)
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if this.revision == revision {
                    this.warning = result
                        .err()
                        .map(|error| format!("Could not save preferences: {error}"));
                    cx.notify();
                }
            });
        }));
    }

    fn toggle_panel(&mut self, kind: PanelKind, window: &mut Window, cx: &mut Context<Self>) {
        let placement = match kind {
            PanelKind::Sidebar => DockPlacement::Left,
            PanelKind::Inspector => DockPlacement::Right,
            PanelKind::Preview => DockPlacement::Bottom,
        };
        let mut target = self.settings.layout.clone();
        target.toggle(kind);
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
            CommandId::RecoverUnsaved => {
                self.project
                    .update(cx, |project, cx| project.recover_pending(window, cx));
            }
            CommandId::OpenProject => {
                if !self.dock.read(cx).is_dock_open(DockPlacement::Left) {
                    self.toggle_panel(PanelKind::Sidebar, window, cx);
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
            CommandId::FocusDocument => self.document.read(cx).focus_handle(cx).focus(window, cx),
            CommandId::LightTheme | CommandId::DarkTheme | CommandId::SystemTheme => {
                self.settings.theme = match command {
                    CommandId::LightTheme => ThemeMode::Light,
                    CommandId::DarkTheme => ThemeMode::Dark,
                    _ => ThemeMode::System,
                };
                theme::apply(self.settings.theme, window.appearance(), cx);
                self.persist(cx);
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
        let commands: Vec<_> = COMMANDS
            .iter()
            .copied()
            .filter(|command| *command != CommandId::OpenPalette)
            .collect();
        let opening = self.opening;
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let commands_for_confirm = commands.clone();
            dialog.title("Commands").child(
                Command::new(&palette)
                    .bordered(false)
                    .placeholder("Search commands…")
                    .items(commands.iter().map(|command| {
                        CommandItem::new()
                            .label(command.label())
                            .disabled(*command == CommandId::OpenFile && opening)
                    }))
                    .empty(|_, _, _| div().p_3().child("No matching commands"))
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
                    Some(Err(error)) => this.file_status = format!("Could not open file: {error}"),
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
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("open-project-toolbar")
                            .small()
                            .ghost()
                            .label(CommandId::OpenProject.label())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.execute(CommandId::OpenProject, window, cx)
                            })),
                    )
                    .child(
                        Button::new("open-file")
                            .small()
                            .ghost()
                            .label(CommandId::OpenFile.label())
                            .disabled(self.opening)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.execute(CommandId::OpenFile, window, cx)
                            })),
                    )
                    .child(
                        Button::new("command-palette")
                            .small()
                            .ghost()
                            .label(CommandId::OpenPalette.label())
                            .track_focus(&self.palette_focus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.palette_focus.focus(window, cx);
                                this.execute(CommandId::OpenPalette, window, cx);
                            })),
                    )
                    .children(
                        [
                            (
                                "toggle-sidebar",
                                CommandId::ToggleSidebar,
                                if layout.sidebar_visible {
                                    "Hide files"
                                } else {
                                    "Show files"
                                },
                            ),
                            (
                                "toggle-inspector",
                                CommandId::ToggleInspector,
                                if layout.inspector_visible {
                                    "Hide inspector"
                                } else {
                                    "Show inspector"
                                },
                            ),
                            (
                                "toggle-preview",
                                CommandId::TogglePreview,
                                if layout.preview_visible {
                                    "Hide preview"
                                } else {
                                    "Show preview"
                                },
                            ),
                        ]
                        .map(|(id, kind, label)| {
                            Button::new(id)
                                .small()
                                .ghost()
                                .label(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.execute(kind, window, cx)
                                }))
                        }),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(self.dock.clone()))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.opening {
                        "Opening file…".to_owned()
                    } else if self.document.read(cx).is_editable() {
                        "Editing in memory · Ctrl/Cmd+S saves.".into()
                    } else {
                        self.file_status.clone()
                    })
                    .when_some(self.warning.as_ref(), |view, warning| {
                        view.child(div().text_color(cx.theme().warning).child(warning.clone()))
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions, base::Root,
        point, px, size, test::TestWindowExt,
    };

    fn mount(cx: &mut TestAppContext, settings: Settings) -> (WindowHandle<Root>, Entity<Shell>) {
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
        let mut settings = Settings::default();
        settings.layout.sidebar_visible = false;
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
        let (handle, view) = mount(cx, Settings::default());
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
            window.click("toggle-dock:Right", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!view.read(cx).settings.layout.inspector_visible);
            window.render_frame(cx);
            assert_eq!(
                window.find("toggle-inspector").label(),
                Some("Show inspector")
            );
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
    fn preview_commands_are_wired_and_report_without_a_project(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, Settings::default());
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
    fn native_drag_is_captured_and_preferences_restore_in_a_new_window(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx, Settings::default());
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
        let (handle, view) = mount(cx, Settings::default());
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
            let mut settings = Settings::default();
            settings.theme = mode;
            let (handle, _) = mount(cx, settings);
            cx.simulate_window_resize(handle.into(), size(px(800.), px(600.)));
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
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
        let (handle, view) = mount(cx, Settings::default());
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
        let (handle, view) = mount(cx, Settings::default());
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
        let (handle, view) = mount(cx, Settings::default());
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
        let path = directory.path().join("preferences.json");
        let (store, settings, _) = SettingsStore::load(path.clone());
        let (_, view) = mount(cx, settings);
        view.update(cx, |shell, cx| {
            shell.store = Some(Arc::new(store));
            shell.settings.layout.sidebar_width = 230.;
            shell.persist(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        assert!(
            !path.exists(),
            "dragging must not sync every intermediate width"
        );
        view.update(cx, |shell, cx| {
            shell.settings.layout.sidebar_width = 275.;
            shell.persist(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        assert!(!path.exists(), "a new change must restart the debounce");
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.executor().allow_parking();
        cx.condition(&view, |_, _| path.exists()).await;
        let (_, restored, warning) = SettingsStore::load(path);
        assert!(warning.is_none());
        assert_eq!(restored.layout.sidebar_width, 275.);
    }

    #[gpui_kit::test]
    #[ignore = "bounded shell measurements; run mise run test:shell-perf"]
    fn readonly_file_open_workload(cx: &mut TestAppContext) {
        use std::time::Instant;
        let directory = tempfile::tempdir().unwrap();
        let (handle, view) = mount(cx, Settings::default());
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
