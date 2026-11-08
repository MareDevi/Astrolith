//! Contextual formatting toolbar anchored to the selection.
//!
//! The toolbar is a *selection popover*: it appears only for a non-empty
//! selection while the editor or the toolbar itself holds focus, is positioned
//! from the editor's own `range_to_bounds` geometry, and is clamped so it never
//! leaves the text viewport. Placement is a pure function of two boxes, so the
//! clamping rules are unit-tested without a window.
//!
//! The toolbar owns a focus handle and tracks it on its container, so moving
//! focus onto one of its buttons with Tab keeps it mounted instead of hiding
//! the control that just received focus.
//!
//! IME note: the pinned facade has no public composition accessor (the marked
//! range is `pub(super)`), so composition cannot be observed directly. In this
//! editor a composition keeps the active selection empty, which is exactly the
//! state the toolbar refuses to show, and a blur hides it as well.

use std::ops::Range;

use astrolith_document::{DocumentKind, FormatCommand};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Entity, FocusHandle, IntoElement,
    MouseButton, Pixels, SharedString, TestSupportExt, WeakEntity, anchored, div, point,
    prelude::*, px,
};

use crate::editor::session::DocumentSession;

/// Space the toolbar needs for its four controls.
pub(crate) const TOOLBAR_WIDTH: f32 = 176.;
/// Toolbar height including its padding.
pub(crate) const TOOLBAR_HEIGHT: f32 = 38.;
const MARGIN: f32 = 8.;
const GAP: f32 = 6.;

/// Window-space top-left corner of the floating toolbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToolbarPlacement {
    /// Left edge in window coordinates.
    pub x: f32,
    /// Top edge in window coordinates.
    pub y: f32,
}

/// Whether the toolbar may show for an active selection.
///
/// An empty selection has nothing to format, a blurred editor has no caret to
/// act on, and both are the states a composition passes through.
pub(crate) fn toolbar_visible(selection: &Range<usize>, focused: bool) -> bool {
    focused && !selection.is_empty()
}

/// Place the toolbar above `selection`, inside `viewport`.
///
/// The toolbar is centred over the selection, clamped horizontally to the text
/// viewport, and moved below the selection when there is no room above it.
pub(crate) fn placement(
    selection: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    toolbar_width: f32,
    toolbar_height: f32,
) -> ToolbarPlacement {
    let viewport_left = f32::from(viewport.origin.x);
    let viewport_top = f32::from(viewport.origin.y);
    // A viewport smaller than the toolbar plus its margins cannot honour both
    // edges; keep the far edge (the visible content end) inside the viewport.
    let max_x = (viewport_left + f32::from(viewport.size.width) - toolbar_width - MARGIN)
        .max(viewport_left);
    let min_x = (viewport_left + MARGIN).min(max_x);
    let desired_x =
        f32::from(selection.origin.x) + f32::from(selection.size.width) / 2. - toolbar_width / 2.;
    let x = desired_x.clamp(min_x, max_x);

    let max_y = (viewport_top + f32::from(viewport.size.height) - toolbar_height - MARGIN)
        .max(viewport_top);
    let min_y = (viewport_top + MARGIN).min(max_y);
    let above = f32::from(selection.origin.y) - toolbar_height - GAP;
    let below = f32::from(selection.origin.y) + f32::from(selection.size.height) + GAP;
    let y = (if above >= min_y { above } else { below }).clamp(min_y, max_y);
    ToolbarPlacement { x, y }
}

/// Build the overlay for the current selection, when one is formattable.
///
/// Returns `None` for an empty selection, when neither the editor nor the
/// toolbar holds focus, or for a selection the editor has not laid out yet.
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
    let placement = placement(bounds, viewport, TOOLBAR_WIDTH, TOOLBAR_HEIGHT);
    let comment = session
        .read_with(cx, |session, _| session.kind() == DocumentKind::Markdown)
        .unwrap_or(false);
    Some(
        anchored()
            .position(point(px(placement.x), px(placement.y)))
            .snap_to_window_with_margin(px(4.))
            .child(toolbar_body(session, comment, toolbar_focus, cx))
            .into_any_element(),
    )
}

fn toolbar_body(
    session: WeakEntity<DocumentSession>,
    comment: bool,
    toolbar_focus: &FocusHandle,
    cx: &mut App,
) -> impl IntoElement {
    let enter_duration = cx.theme().motion_tokens().duration_fast;
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
    div()
        .id("selection-toolbar")
        .test_support()
        .track_focus(toolbar_focus)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .h_flex()
        .items_center()
        .gap_1()
        .p_1p5()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .shadow_lg()
        .text_color(cx.theme().popover_foreground)
        .children(buttons.into_iter().map(|(id, command, icon)| {
            let session = session.clone();
            Button::new(id)
                .small()
                .ghost()
                .tab_index(0)
                .icon(icon)
                .tooltip(format_label(command))
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

fn format_label(command: FormatCommand) -> SharedString {
    match command {
        FormatCommand::Bold => "Bold (Ctrl/Cmd+B)",
        FormatCommand::Italic => "Italic (Ctrl/Cmd+I)",
        FormatCommand::Link => "Link (Ctrl/Cmd+K)",
        FormatCommand::Comment => "Markdown comment (Ctrl/Cmd+/)",
        FormatCommand::Heading(_) => "Heading",
        FormatCommand::Paragraph => "Paragraph",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

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
    fn toolbar_centres_over_the_selection() {
        // 600 px viewport, 100 px selection centred at 300 px.
        let viewport = bounds(0., 100., 600., 400.);
        let selection = bounds(250., 300., 100., 20.);
        let placed = placement(selection, viewport, TOOLBAR_WIDTH, TOOLBAR_HEIGHT);
        assert_eq!(placed.x, 300. - TOOLBAR_WIDTH / 2.);
        assert_eq!(placed.y, 300. - TOOLBAR_HEIGHT - GAP);
    }

    #[test]
    fn toolbar_clamps_inside_the_pane_at_both_edges() {
        let viewport = bounds(0., 100., 600., 400.);
        let left = placement(
            bounds(0., 300., 10., 20.),
            viewport,
            TOOLBAR_WIDTH,
            TOOLBAR_HEIGHT,
        );
        assert_eq!(left.x, MARGIN);
        let right = placement(
            bounds(590., 300., 10., 20.),
            viewport,
            TOOLBAR_WIDTH,
            TOOLBAR_HEIGHT,
        );
        assert_eq!(right.x, 600. - TOOLBAR_WIDTH - MARGIN);
    }

    #[test]
    fn toolbar_moves_below_when_the_top_has_no_room() {
        let viewport = bounds(0., 100., 600., 400.);
        let placed = placement(
            bounds(250., 104., 100., 20.),
            viewport,
            TOOLBAR_WIDTH,
            TOOLBAR_HEIGHT,
        );
        assert_eq!(placed.y, 104. + 20. + GAP);
        assert!(placed.y + TOOLBAR_HEIGHT <= 100. + 400.);
    }

    #[test]
    fn toolbar_never_leaves_a_short_viewport() {
        let viewport = bounds(0., 100., 200., 50.);
        let placed = placement(
            bounds(0., 100., 10., 20.),
            viewport,
            TOOLBAR_WIDTH,
            TOOLBAR_HEIGHT,
        );
        assert_eq!(placed.x, MARGIN);
        // The viewport is shorter than the toolbar plus its margins, so the
        // clamp lands at the largest offset that still keeps it inside.
        assert_eq!(placed.y, 100. + 50. - TOOLBAR_HEIGHT - MARGIN);
    }
}
