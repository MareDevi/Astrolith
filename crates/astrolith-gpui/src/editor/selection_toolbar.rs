//! The floating format toolbar that follows a writer's selection.
//!
//! Placement is derived from the editor's own `range_to_bounds` geometry, and is
//! clamped so it never leaves the document pane. The toolbar is presentation
//! only: every button dispatches a [`FormatCommand`] back through the session.

use std::ops::Range;

use astrolith_document::{DocumentKind, FormatCommand};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme, StyledExt};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Entity, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, Keystroke, MouseButton, Pixels, SharedString, TestSupportExt,
    WeakEntity, anchored, point, prelude::*, px,
};

use crate::commands::CommandId;
use crate::editor::session::DocumentSession;
use crate::ui;

/// A small Kit icon button, in rem.
const BUTTON_REM: f32 = 1.5;
/// The popover's `.p_1()` inset and `.gap_1()` between buttons, in rem.
const INSET_REM: f32 = 0.25;
/// Space kept between the toolbar and the pane edges.
const MARGIN: f32 = 8.;
/// Space kept between the toolbar and the selection.
const GAP: f32 = 6.;

/// Where the toolbar sits, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToolbarPlacement {
    pub x: f32,
    pub y: f32,
}

/// Whether the toolbar belongs on screen at all.
pub(crate) fn toolbar_visible(selection: &Range<usize>, focused: bool) -> bool {
    focused && !selection.is_empty()
}

/// The toolbar's bounds at a given root font size and button count.
pub(crate) fn toolbar_size(rem_size: f32, buttons: usize) -> (f32, f32) {
    let rem = rem_size.max(1.);
    let buttons = buttons.max(1) as f32;
    let width = (buttons * BUTTON_REM + (buttons - 1.) * INSET_REM + 2. * INSET_REM) * rem;
    let height = (BUTTON_REM + 2. * INSET_REM) * rem;
    (width, height)
}

/// Place the toolbar above the selection, or below it when the top has no
/// room, clamped inside the document pane on both axes.
pub(crate) fn placement(
    selection: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    toolbar_width: f32,
    toolbar_height: f32,
) -> ToolbarPlacement {
    let min_x = f32::from(viewport.origin.x) + MARGIN;
    let max_x = f32::from(viewport.origin.x + viewport.size.width) - toolbar_width - MARGIN;
    let centered =
        f32::from(selection.origin.x) + f32::from(selection.size.width) / 2. - toolbar_width / 2.;
    let x = if max_x < min_x {
        min_x
    } else {
        centered.clamp(min_x, max_x)
    };
    let above = f32::from(selection.origin.y) - toolbar_height - GAP;
    let below = f32::from(selection.origin.y + selection.size.height) + GAP;
    let min_y = f32::from(viewport.origin.y) + MARGIN;
    let max_y = f32::from(viewport.origin.y + viewport.size.height) - toolbar_height - MARGIN;
    let y = if max_y < min_y {
        max_y.max(f32::from(viewport.origin.y))
    } else if above >= min_y {
        above
    } else {
        below
    };
    let y = if max_y < min_y {
        y
    } else {
        y.clamp(min_y, max_y)
    };
    ToolbarPlacement { x, y }
}

/// Build the overlay for the current selection, when one is visible.
pub(crate) fn overlay(
    editor: &Entity<EditorState>,
    session: WeakEntity<DocumentSession>,
    toolbar_focus: &FocusHandle,
    focused: bool,
    cx: &mut App,
) -> Option<AnyElement> {
    let (bounds, viewport) = {
        let state = editor.read(cx);
        let selection = state.selected_range();
        if !toolbar_visible(&selection, focused) {
            return None;
        }
        let writer = session
            .read_with(cx, |session, cx| {
                (!session.is_source_view()).then(|| session.writer_selection_geometry(cx))
            })
            .ok()
            .flatten()
            .flatten();
        match writer {
            Some(geometry) => geometry,
            None => (state.range_to_bounds(&selection)?, state.text_bounds()?),
        }
    };
    let comment = session
        .read_with(cx, |session, _| session.kind() == DocumentKind::Markdown)
        .unwrap_or(false);
    let (width, height) =
        toolbar_size(f32::from(cx.theme().font_size), if comment { 4 } else { 3 });
    let placement = placement(bounds, viewport, width, height);
    Some(
        anchored()
            .position(point(px(placement.x), px(placement.y)))
            .snap_to_window_with_margin(px(4.))
            .child(toolbar_body(
                editor.clone(),
                session,
                comment,
                toolbar_focus,
                cx,
            ))
            .into_any_element(),
    )
}

fn toolbar_body(
    editor: Entity<EditorState>,
    session: WeakEntity<DocumentSession>,
    comment: bool,
    toolbar_focus: &FocusHandle,
    cx: &mut App,
) -> impl IntoElement {
    let enter_duration = cx.theme().motion_tokens().duration_fast;
    let editor_focus = editor.read(cx).focus_handle(cx);
    let mut buttons: Vec<(&'static str, FormatCommand, IconName)> = vec![
        (
            "selection-toolbar-bold",
            FormatCommand::Bold,
            IconName::Bold,
        ),
        (
            "selection-toolbar-italic",
            FormatCommand::Italic,
            IconName::Italic,
        ),
        (
            "selection-toolbar-link",
            FormatCommand::Link,
            IconName::Link,
        ),
    ];
    if comment {
        buttons.push((
            "selection-toolbar-comment",
            FormatCommand::Comment,
            IconName::MessageCircle,
        ));
    }
    let inner_radius = ui::radius_sm(cx);
    ui::popover_surface(cx)
        .id("selection-toolbar")
        .test_support()
        .track_focus(toolbar_focus)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.key != "escape" {
                return;
            }
            window.focus(&editor_focus, cx);
            cx.stop_propagation();
        })
        .h_flex()
        .items_center()
        .gap_1()
        .p_1()
        .children(buttons.into_iter().map(move |(id, command, icon)| {
            let session = session.clone();
            let hint = shortcut_hint(command, cx);
            ui::icon_button_with_shortcut(id, icon, format_label(command), hint.as_deref())
                .rounded(inner_radius)
                .tab_index(0)
                .on_click(move |_, window, cx| {
                    let _ =
                        session.update(cx, |session, cx| session.apply_format(command, window, cx));
                })
        }))
        .with_animation(
            "selection-toolbar-appear",
            Animation::new(enter_duration).with_easing(gpui_kit::base::animation::ease_out_cubic),
            |toolbar, progress| toolbar.opacity(progress),
        )
}

/// The platform-formatted binding that also runs this format command.
fn shortcut_hint(command: FormatCommand, cx: &App) -> Option<String> {
    let shortcut = match command {
        FormatCommand::Bold => CommandId::Bold.shortcut(),
        FormatCommand::Italic => CommandId::Italic.shortcut(),
        FormatCommand::Link => CommandId::InsertLink.shortcut(),
        FormatCommand::Comment => CommandId::ToggleComment.shortcut(),
        FormatCommand::Heading(_) | FormatCommand::Paragraph => return None,
    }?;
    let stroke = Keystroke::parse_with_os(shortcut, cx.platform_info().os).ok()?;
    Some(Kbd::format(&stroke))
}

fn format_label(command: FormatCommand) -> SharedString {
    match command {
        FormatCommand::Bold => tr!("editor.selection_toolbar.bold"),
        FormatCommand::Italic => tr!("editor.selection_toolbar.italic"),
        FormatCommand::Link => tr!("editor.selection_toolbar.link"),
        FormatCommand::Comment => tr!("editor.selection_toolbar.comment"),
        FormatCommand::Heading(_) => tr!("editor.selection_toolbar.heading"),
        FormatCommand::Paragraph => tr!("editor.selection_toolbar.paragraph"),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

    /// The window root font size the tests scale against.
    const REM: f32 = 16.;

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(width), px(height)),
        }
    }

    #[test]
    fn toolbar_hides_without_a_selection_or_focus() {
        assert!(!toolbar_visible(&(4..4), true));
        assert!(!toolbar_visible(&(4..9), false));
        assert!(toolbar_visible(&(4..9), true));
    }

    #[test]
    fn toolbar_size_follows_the_rem_and_the_button_count() {
        let (four_wide, height) = toolbar_size(REM, 4);
        let (three_wide, three_height) = toolbar_size(REM, 3);
        assert_eq!(height, 2. * REM);
        assert_eq!(three_height, height);
        assert_eq!(four_wide - three_wide, (BUTTON_REM + INSET_REM) * REM);
        // A larger root scales the whole toolbar, not just its text.
        let (scaled, _) = toolbar_size(REM * 1.5, 4);
        assert_eq!(scaled, four_wide * 1.5);
    }

    #[test]
    fn toolbar_centres_over_the_selection() {
        let (toolbar_width, toolbar_height) = toolbar_size(REM, 4);
        // 600 px viewport, 100 px selection centred at 300 px.
        let viewport = bounds(0., 100., 600., 400.);
        let selection = bounds(250., 300., 100., 20.);
        let placed = placement(selection, viewport, toolbar_width, toolbar_height);
        assert_eq!(placed.x, 300. - toolbar_width / 2.);
        assert_eq!(placed.y, 300. - toolbar_height - GAP);
    }

    #[test]
    fn toolbar_clamps_inside_the_pane_at_both_edges() {
        let (toolbar_width, toolbar_height) = toolbar_size(REM, 4);
        let viewport = bounds(0., 100., 600., 400.);
        let left = placement(
            bounds(0., 300., 10., 20.),
            viewport,
            toolbar_width,
            toolbar_height,
        );
        assert_eq!(left.x, MARGIN);
        let right = placement(
            bounds(590., 300., 10., 20.),
            viewport,
            toolbar_width,
            toolbar_height,
        );
        assert_eq!(right.x, 600. - toolbar_width - MARGIN);
    }

    #[test]
    fn toolbar_moves_below_when_the_top_has_no_room() {
        let (toolbar_width, toolbar_height) = toolbar_size(REM, 3);
        let viewport = bounds(0., 100., 600., 400.);
        let placed = placement(
            bounds(250., 104., 100., 20.),
            viewport,
            toolbar_width,
            toolbar_height,
        );
        assert_eq!(placed.y, 104. + 20. + GAP);
        assert!(placed.y + toolbar_height <= 100. + 400.);
    }

    #[test]
    fn toolbar_never_leaves_a_short_viewport() {
        let (toolbar_width, toolbar_height) = toolbar_size(REM, 4);
        let viewport = bounds(0., 100., 200., 50.);
        let placed = placement(
            bounds(0., 100., 10., 20.),
            viewport,
            toolbar_width,
            toolbar_height,
        );
        assert_eq!(placed.x, MARGIN);
        // The viewport is shorter than the toolbar plus its margins, so the
        // clamp lands at the largest offset that still keeps it inside.
        assert_eq!(placed.y, 100. + 50. - toolbar_height - MARGIN);
    }
}
