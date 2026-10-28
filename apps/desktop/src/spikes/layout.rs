//! Disposable M00.3 layout probe; no workspace or persistence policy.
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme, Sizable, StyledExt, Theme, ThemeMode, WindowExt,
    button::{Button, ButtonVariants},
    dock::{
        BasePanel, DockArea, DockLayout, DockPlacement, DockSkin, Panel, PanelEvent, panel_handle,
    },
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, TestSupportExt, Window, div, prelude::*, px, rems,
};

/// One retained view; the component owns all dock geometry and visibility state.
pub(crate) struct LayoutSpike {
    dock: Entity<DockArea>,
    _skin: Rc<DockSkin>,
    appearance_focus: FocusHandle,
}

impl LayoutSpike {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        apply_theme(ThemeMode::Dark, window, cx);
        let navigator = cx.new(|cx| {
            ProbePanel::new(
                "navigator-panel",
                "Navigator",
                "Embedded fixture\n\ncontent/\n  welcome.md\n  中文.md",
                cx,
            )
        });
        let document = cx.new(|cx| ProbePanel::new("document-panel", "welcome.md", "# Astrolith\n\nTheme and dock feasibility probe.\n\nDrag either divider. Hide and restore the side panels.\n\n中文 · 日本語 · 🦀", cx));
        let inspector = cx.new(|cx| {
            ProbePanel::new(
                "inspector-panel",
                "Inspector",
                "Title\nWelcome\n\nStatus\nDraft\n\nFixture only; no file is opened.",
                cx,
            )
        });
        let (dock, skin) = DockSkin::dock_area("layout-spike", None, window, cx);
        dock.update(cx, |area, cx| {
            area.set_center(
                DockLayout::tabs().panel_view(panel_handle(document), cx),
                window,
                cx,
            );
            area.set_dock(
                DockPlacement::Left,
                DockLayout::tabs().panel_view(panel_handle(navigator), cx),
                window,
                cx,
            );
            area.set_dock(
                DockPlacement::Right,
                DockLayout::tabs().panel_view(panel_handle(inspector), cx),
                window,
                cx,
            );
            // Dock's API takes measured pixels; preferred extents originate in the rem scale.
            let extent = rems(14.).to_pixels(window.rem_size());
            area.set_dock_size(DockPlacement::Left, extent, window, cx);
            area.set_dock_size(DockPlacement::Right, extent, window, cx);
        });
        Self {
            dock,
            _skin: skin,
            appearance_focus: cx.focus_handle().tab_stop(true),
        }
    }
}

/// All surfaces, including DockSkin and an already open Dialog, share this global change.
fn apply_theme(mode: ThemeMode, window: &mut Window, cx: &mut App) {
    Theme::change(mode, Some(window), cx);
    Theme::update(cx, |theme| {
        // A compact theme-level radius, synchronized into component tokens and Base.
        theme.radius = px(4.);
        theme.colors.primary = theme.colors.info;
    });
}

fn toggle_theme(window: &mut Window, cx: &mut App) {
    let mode = if cx.theme().mode == ThemeMode::Dark {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    apply_theme(mode, window, cx);
}

impl Render for LayoutSpike {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let appearance_focus = self.appearance_focus.clone();
        let navigator_open = self.dock.read(cx).is_dock_open(DockPlacement::Left);
        let inspector_open = self.dock.read(cx).is_dock_open(DockPlacement::Right);
        div().v_flex().size_full().bg(cx.theme().background).text_color(cx.theme().foreground)
            .child(div().h_flex().gap_2().p_2().border_b_1().border_color(cx.theme().border)
                .child(Button::new("toggle-navigator").small().ghost()
                    .label(if navigator_open { "Hide navigator" } else { "Show navigator" })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dock.update(cx, |area, cx| area.toggle_dock(DockPlacement::Left, window, cx));
                        cx.notify();
                    })))
                .child(Button::new("toggle-inspector").small().ghost()
                    .label(if inspector_open { "Hide inspector" } else { "Show inspector" })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dock.update(cx, |area, cx| area.toggle_dock(DockPlacement::Right, window, cx));
                        cx.notify();
                    })))
                .child(Button::new("toggle-theme").small().ghost().label("Switch theme")
                    .on_click(|_, window, cx| toggle_theme(window, cx)))
                .child(Button::new("appearance-dialog").small().ghost().label("Appearance…").track_focus(&self.appearance_focus)
                    .on_click(move |_, window, cx| {
                        // Styled buttons preserve pointer focus; Root restores the focus it captures.
                        appearance_focus.focus(window, cx);
                        window.open_dialog(cx, |dialog, _, cx| {
                            dialog.title("Appearance")
                                .child(div().text_color(cx.theme().muted_foreground)
                                    .child("Inspect the dock and dialog in both themes. Escape returns focus to Appearance."))
                                .child(Button::new("dialog-theme").label("Switch theme")
                                    .on_click(|_, window, cx| toggle_theme(window, cx)))
                        });
                    })))
            .child(div().flex_1().min_h_0().min_w_0().child(self.dock.clone()))
            .child(div().px_3().py_1().text_xs().text_color(cx.theme().muted_foreground)
                .border_t_1().border_color(cx.theme().border)
                .child("M00.3 probe · Drag dividers · Tab to controls · Escape closes dialog"))
    }
}

struct ProbePanel {
    id: &'static str,
    title: &'static str,
    content: &'static str,
    focus: FocusHandle,
}

impl ProbePanel {
    fn new(
        id: &'static str,
        title: &'static str,
        content: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id,
            title,
            content,
            focus: cx.focus_handle().tab_stop(true),
        }
    }
}
impl EventEmitter<PanelEvent> for ProbePanel {}
impl Focusable for ProbePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl BasePanel for ProbePanel {
    fn panel_name(&self) -> &'static str {
        self.id
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn zoomable(&self, _: &App) -> bool {
        false
    }
}
impl Panel for ProbePanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.title
    }
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.title.into())
    }
}
impl Render for ProbePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.id)
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .p_3()
            .text_sm()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .focus_visible(|style| style.border_1().border_color(cx.theme().ring))
            .child(self.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
        base::Root,
        component::{ActiveTheme, Theme, ThemeMode, WindowExt},
        point, px, size,
        test::TestWindowExt,
    };

    fn mount(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<LayoutSpike>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let (handle, view) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Point::default(),
                        size(px(1000.), px(700.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LayoutSpike::new(window, cx)),
            )
            .unwrap();
            (handle.downcast::<Root>().unwrap(), view)
        })
    }

    #[gpui_kit::test]
    fn three_panels_have_nonzero_geometry_across_viewports_and_scales(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        for (width, height) in [(800., 600.), (1000., 700.), (1440., 900.)] {
            for scale in [1., 1.25, 1.5, 2.] {
                cx.simulate_window_resize(handle.into(), size(px(width), px(height)));
                cx.simulate_window_scale_factor_change(handle.into(), scale);
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    assert_eq!(window.scale_factor(), scale);
                    let panels = ["navigator-panel", "document-panel", "inspector-panel"]
                        .map(|id| window.find(id).bounds());
                    eprintln!("layout {width}x{height} simulated scale={scale}: {panels:?}");
                    for bounds in panels {
                        assert!(
                            bounds.size.width > px(90.) && bounds.size.height > px(400.),
                            "viewport {width}x{height} scale {scale}: {bounds:?}"
                        );
                        assert!(bounds.left() >= px(0.) && bounds.right() <= px(width));
                    }
                    assert!(panels[0].right() <= panels[1].left());
                    assert!(panels[1].right() <= panels[2].left());
                })
                .unwrap();
            }
        }
    }

    #[gpui_kit::test]
    fn side_docks_hide_and_restore_through_visible_controls(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let initial = window.find("document-panel").bounds();
            for (button, panel) in [
                ("toggle-navigator", "navigator-panel"),
                ("toggle-inspector", "inspector-panel"),
            ] {
                window.click(button, cx);
                assert!(window.try_find(panel).is_none_or(|panel| !panel.visible()));
                assert!(window.find("document-panel").bounds().size.width > initial.size.width);
                window.click(button, cx);
                assert!(window.find(panel).visible());
                assert_eq!(window.find("document-panel").bounds(), initial);
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn native_collapse_control_keeps_toolbar_label_in_sync(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("toggle-dock:Left", cx);
            assert!(
                window
                    .try_find("navigator-panel")
                    .is_none_or(|panel| !panel.visible())
            );
            assert_eq!(
                window.find("toggle-navigator").label(),
                Some("Show navigator")
            );
            window.click("toggle-navigator", cx);
            assert!(window.find("navigator-panel").visible());
            assert_eq!(
                window.find("toggle-navigator").label(),
                Some("Hide navigator")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn dragging_each_native_handle_changes_only_its_panel(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            for (handle, panel, other, delta) in [
                (
                    "resize-handle-left",
                    "navigator-panel",
                    "inspector-panel",
                    40.,
                ),
                (
                    "resize-handle-right",
                    "inspector-panel",
                    "navigator-panel",
                    -40.,
                ),
            ] {
                let before = window.find(panel).bounds().size.width;
                let untouched = window.find(other).bounds().size.width;
                let start = window.find(handle).bounds().center();
                window.drag(start, point(start.x + px(delta), start.y), cx);
                assert!(window.find(panel).bounds().size.width > before + px(20.));
                assert_eq!(window.find(other).bounds().size.width, untouched);
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn theme_changes_with_dialog_open_and_escape_restores_trigger_focus(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("appearance-dialog", cx);
            assert!(window.has_active_dialog(cx));
            assert_ne!(window.find("appearance-dialog").focused(), Some(true));
            let before = cx.theme().mode;
            window.click("dialog-theme", cx);
            assert_ne!(cx.theme().mode, before);
            assert!(window.has_active_dialog(cx));
            assert_eq!(cx.theme().tokens.primary.color, cx.theme().colors.primary);
            window.press("escape", cx);
            assert!(!window.has_active_dialog(cx));
            assert_eq!(window.find("appearance-dialog").focused(), Some(true));
            window.press("enter", cx);
            assert!(
                window.has_active_dialog(cx),
                "restored trigger must remain keyboard-operable"
            );
            window.press("escape", cx);
            window.click("toggle-theme", cx);
            assert_eq!(cx.theme().mode, before);
            window.click("appearance-dialog", cx);
            window.press("escape", cx);
            window.press("shift-tab", cx);
            assert_eq!(window.find("toggle-theme").focused(), Some(true));
            window.press("enter", cx);
            assert_ne!(cx.theme().mode, before);
            Theme::change(ThemeMode::Light, Some(window), cx);
        })
        .unwrap();
    }
}
