use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

use crate::{
    commands::{self, COMMANDS, CommandId},
    file::{DocumentSource, read_document},
    layout::{
        panels::WorkspacePanel,
        state::{LayoutState, PanelKind},
    },
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
    Render, Subscription, Window, WindowOptions, div, prelude::*, px, size,
};

type LatestSettings = Arc<Mutex<(u64, Settings)>>;

/// Open the shell after background preference loading; bootstrap owns no services.
pub fn open(cx: &mut App) {
    commands::bind_keys(cx);
    cx.spawn(async move |cx| {
        let loaded = cx
            .background_spawn(async {
                settings_path().map(|path| {
                    let (store, settings, warning) = SettingsStore::load(path);
                    (Arc::new(store), settings, warning)
                })
            })
            .await;
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
    navigator: Entity<WorkspacePanel>,
    palette: Entity<CommandState>,
    palette_focus: FocusHandle,
    opening: bool,
    file_status: String,
    _skin: Rc<DockSkin>,
    settings: Settings,
    store: Option<Arc<SettingsStore>>,
    latest: LatestSettings,
    revision: u64,
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
        let document = cx.new(|cx| WorkspacePanel::new("document-panel", "Welcome", "# Astrolith\n\nOpen a local Markdown or MDX document to view its source.\n\nThis stage provides readonly file navigation.\n\n中文 · 日本語", window, cx));
        let inspector = cx.new(|cx| WorkspacePanel::new("inspector-panel", "Inspector", "No collection selected\n\nCollection metadata becomes available when an Astro workspace is open.", window, cx));
        let preview = cx.new(|cx| WorkspacePanel::new("preview-panel", "Preview", "No preview running\n\nAstro preview will use your project's development server and an external browser.", window, cx));
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
                    navigator.clone(),
                    settings.layout.sidebar_visible,
                ),
                (
                    DockPlacement::Right,
                    inspector,
                    settings.layout.inspector_visible,
                ),
                (
                    DockPlacement::Bottom,
                    preview,
                    settings.layout.preview_visible,
                ),
            ] {
                area.set_dock(
                    placement,
                    DockLayout::tabs().panel_view(panel_handle(panel), cx),
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
            cx.subscribe_in(&dock, window, |this, _, event: &DockEvent, _, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.capture_layout(cx);
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.settings.theme == ThemeMode::System {
                    theme::apply(ThemeMode::System, window.appearance(), cx);
                }
            }),
        ];
        if let Some(store) = store.clone() {
            let latest = latest.clone();
            // Register on App rather than the view: last-window closure may drop the view first.
            App::on_app_quit(cx, move |cx| {
                let store = store.clone();
                let latest = latest.clone();
                cx.background_executor().spawn(async move {
                    let snapshot = latest.lock().ok().map(|snapshot| snapshot.clone());
                    if let Some((revision, settings)) = snapshot
                        && let Err(error) = store.save(revision, &settings)
                    {
                        eprintln!("Could not save preferences on exit: {error}");
                    }
                })
            })
            .detach();
        }
        let focus = cx.focus_handle().tab_stop(true);
        focus.focus(window, cx);
        subscriptions.push(cx.observe(&dock, |_, _, cx| cx.notify()));
        Self {
            dock,
            document,
            navigator,
            palette,
            palette_focus: cx.focus_handle().tab_stop(true),
            opening: false,
            file_status: "Readonly · No file open".into(),
            _skin: skin,
            settings,
            store,
            latest,
            revision: 0,
            warning,
            focus,
            _subscriptions: subscriptions,
        }
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
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.save(revision, &settings) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.revision == revision {
                    this.warning = result
                        .err()
                        .map(|error| format!("Could not save preferences: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
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
    fn execute(&mut self, command: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            CommandId::OpenFile => self.open_file(window, cx),
            CommandId::OpenPalette => self.open_palette(window, cx),
            CommandId::ToggleSidebar => self.toggle_panel(PanelKind::Sidebar, window, cx),
            CommandId::ToggleInspector => self.toggle_panel(PanelKind::Inspector, window, cx),
            CommandId::TogglePreview => self.toggle_panel(PanelKind::Preview, window, cx),
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
        cx.spawn_in(window, async move |this, cx| {
            let result = match selected.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => Some(
                        cx.background_spawn(async move { read_document(path) })
                            .await
                            .map_err(|error| error.to_string()),
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
        })
        .detach();
    }

    fn show_document(
        &mut self,
        source: DocumentSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.document
            .update(cx, |panel, cx| panel.show_source(&source, window, cx));
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
        self.document.read(cx).focus_handle(cx).focus(window, cx);
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            assert!(window.find("preview-panel").visible());
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
    fn file_picker_opens_readonly_source_and_cancel_preserves_the_buffer(cx: &mut TestAppContext) {
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
        cx.run_until_parked();
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
}
