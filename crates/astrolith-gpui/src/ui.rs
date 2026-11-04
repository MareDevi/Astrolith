//! Shared presentation primitives for the writing desk.
//!
//! These helpers keep the small, repeated pieces of the shell on one visual
//! system: a quiet wordmark, compact toolbar buttons with readable icons, a
//! sans voice for document names, and the section and empty-state rhythm
//! shared by every panel.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable, StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::{App, ElementId, IntoElement, SharedString, TestSupportExt, div, prelude::*, px};

/// The application mark, set in the neutral foreground.
pub(crate) fn mark(cx: &App) -> impl IntoElement {
    Icon::new(IconName::Feather)
        .small()
        .text_color(cx.theme().foreground)
}

/// The wordmark shown in the window chrome.
pub(crate) fn wordmark(cx: &App) -> impl IntoElement {
    div().h_flex().items_center().gap_2().child(mark(cx)).child(
        div()
            .font_family(crate::theme::display_font(cx))
            .text_sm()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .child("Astrolith"),
    )
}

/// A vertical hairline used between chrome groups.
pub(crate) fn v_divider(cx: &App) -> impl IntoElement {
    div()
        .w(px(1.))
        .h_4()
        .mx_1()
        .bg(cx.theme().border)
        .opacity(0.7)
}

/// A compact ghost toolbar button with a consistently sized icon.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    tooltip: impl Into<SharedString>,
) -> Button {
    Button::new(id).small().ghost().icon(icon).tooltip(tooltip)
}

/// A document title, set in the interface face.
pub(crate) fn document_title(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .min_w_0()
        .truncate()
        .font_family(crate::theme::prose_font(cx))
        .text_sm()
        .child(text.into())
}

/// A calm empty state used by panels that have nothing to show yet.
pub(crate) fn empty_state(
    id: &'static str,
    icon: IconName,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    action: Option<gpui_kit::AnyElement>,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .px_6()
        .py_10()
        .text_center()
        .child(
            div()
                .size(px(44.))
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().group_box)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(icon)
                        .size_5()
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            div()
                .font_family(crate::theme::display_font(cx))
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(280.))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(body.into()),
        )
        .children(action)
}
