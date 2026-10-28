use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

use crate::{
    layout::{
        panels::WorkspacePanel,
        state::{LayoutState, PanelKind},
    },
    settings::{Settings, SettingsStore, settings_path},
    theme::{self, ThemeMode},
};
use gpui_kit::component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    dock::{DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, panel_handle},
};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, Render, Subscription, Window,
    WindowOptions, div, prelude::*, px, size,
};

type LatestSettings = Arc<Mutex<(u64, Settings)>>;

/// Open the shell after background preference loading; bootstrap owns no services.
pub fn open(cx: &mut App) {
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
                DockLayout::tabs().panel_view(panel_handle(document), cx),
                window,
                cx,
            );
            for (placement, panel, visible) in [
                (
                    DockPlacement::Left,
                    navigator,
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

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = &self.settings.layout;
        div()
            .v_flex()
            .track_focus(&self.focus)
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
                    .child(div().text_sm().child("Astrolith"))
                    .children(
                        [
                            (
                                "toggle-sidebar",
                                PanelKind::Sidebar,
                                if layout.sidebar_visible {
                                    "Hide files"
                                } else {
                                    "Show files"
                                },
                            ),
                            (
                                "toggle-inspector",
                                PanelKind::Inspector,
                                if layout.inspector_visible {
                                    "Hide inspector"
                                } else {
                                    "Show inspector"
                                },
                            ),
                            (
                                "toggle-preview",
                                PanelKind::Preview,
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
                                    this.toggle_panel(kind, window, cx)
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
                    .child("Readonly · No project open")
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
}
