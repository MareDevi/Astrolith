//! Shared presentation primitives for the writing desk.
//!
//! These helpers keep the small, repeated pieces of the shell on one visual
//! system: a quiet wordmark, compact toolbar buttons with readable icons, a
//! sans voice for document names, and the section, notice, picker and
//! empty-state rhythm shared by every panel.
//!
//! The primitives below are the single home for the patterns the shell, the
//! preferences panes and the explorer keep re-implementing, so a screen builds
//! its surfaces from the shared vocabulary instead of rebuilding them ad hoc.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    kbd::Kbd,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Hsla, IntoElement, Keystroke, Rems, SharedString,
    TestSupportExt, Window, div, img, prelude::*, px, rems,
};

/// The application mark for the active appearance.
pub(crate) fn mark(cx: &App) -> impl IntoElement {
    let path = if cx.theme().mode.is_dark() {
        "astrolith-dark.svg"
    } else {
        "astrolith-light.svg"
    };
    img(path).size_5()
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
///
/// The label names the button for assistive technology and doubles as the
/// tooltip, so an icon-only control is never nameless.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
) -> Button {
    let label = label.into();
    Button::new(id)
        .small()
        .ghost()
        .icon(icon)
        .tooltip(label.clone())
        .accessibility_label(label)
}

/// An icon-only toolbar button whose tooltip names its keyboard shortcut.
pub(crate) fn icon_button_with_shortcut(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    shortcut: Option<&str>,
) -> Button {
    let label = label.into();
    let tooltip = match shortcut {
        Some(shortcut) => SharedString::from(format!("{label} ({shortcut})")),
        None => label.clone(),
    };
    Button::new(id)
        .small()
        .ghost()
        .icon(icon)
        .tooltip(tooltip)
        .accessibility_label(label)
}

/// Compact concentric radius for a control nested inside a rounded panel.
pub(crate) fn radius_sm(cx: &App) -> gpui_kit::Pixels {
    crate::theme::radius_sm(cx)
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
///
/// The container fills its host and centers on both axes, so a panel with a
/// bounded height (the preview pane) and one that grows (the explorer) place
/// the state identically.
pub(crate) fn empty_state(
    id: &'static str,
    icon: IconName,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    action: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .size_full()
        .flex_1()
        .min_h_0()
        .v_flex()
        .items_center()
        .justify_center()
        .gap_3()
        .px_6()
        .py_8()
        .text_center()
        .child(
            div()
                .size_11()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().window_border)
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
                .max_w(rems(20.))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(body.into()),
        )
        .children(action)
}

/// The one header row shared by every docked panel.
///
/// `trailing` holds the panel's own controls; the leading column keeps the
/// title and subtitle on one quiet baseline and truncates rather than wraps.
pub(crate) fn panel_header(
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    trailing: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> Div {
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_2()
        .flex_shrink_0()
        .min_h(rems(2.5))
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(cx.theme().window_border)
        .child(
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(title.into()),
                )
                .when_some(subtitle, |this, subtitle| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(subtitle),
                    )
                }),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .children(trailing),
        )
}

/// The shell of an anchored Level-2 overlay: menu, popover or selection bar.
pub(crate) fn popover_surface(cx: &App) -> Div {
    div()
        .bg(cx.theme().popover)
        .text_color(cx.theme().popover_foreground)
        .border_1()
        .border_color(cx.theme().border)
        .rounded(cx.theme().radius_lg)
        .shadow_lg()
}

/// A small round indicator for a state that is also named in text.
pub(crate) fn status_dot(color: Hsla) -> Div {
    div().size_2().rounded_full().flex_shrink_0().bg(color)
}

/// The four message tones an [`inline_notice`] can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeTone {
    Info,
    Success,
    Warning,
    Danger,
}

/// An inline message with a tone icon.
///
/// Dismissal belongs to the caller: chain `.on_close(...)` when the notice can
/// be closed and leave it off for persistent context, so a notice never grows
/// a close button the screen cannot act on.
pub(crate) fn inline_notice(
    id: impl Into<ElementId>,
    tone: NoticeTone,
    message: impl Into<SharedString>,
) -> Alert {
    let message = message.into();
    match tone {
        NoticeTone::Info => Alert::info(id, message),
        NoticeTone::Success => Alert::success(id, message),
        NoticeTone::Warning => Alert::warning(id, message),
        NoticeTone::Danger => Alert::error(id, message),
    }
    .small()
}

/// A form field's label row: label, required marker, meta text and control.
pub(crate) fn form_field_header(
    label: impl Into<SharedString>,
    required: bool,
    meta: Option<SharedString>,
    trailing: Option<AnyElement>,
    cx: &App,
) -> Div {
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_2()
        .w_full()
        .min_w_0()
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().font_medium().truncate().child(label.into()))
                .when(required, |this| {
                    this.child(div().text_sm().text_color(cx.theme().danger).child("*"))
                }),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .when_some(meta, |this, meta| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(meta),
                    )
                })
                .children(trailing),
        )
}

/// A removable token: a quiet chip with a named remove button.
pub(crate) fn removable_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    remove_label: impl Into<SharedString>,
    on_remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let remove_label = remove_label.into();
    div()
        .h_flex()
        .items_center()
        .gap_1()
        .min_h_6()
        .py_0p5()
        .pl_2()
        .pr_1()
        .rounded(radius_sm(cx))
        .bg(cx.theme().muted)
        .text_xs()
        .child(label.into())
        .child(
            Button::new(id)
                .xsmall()
                .ghost()
                .icon(IconName::X)
                .tooltip(remove_label.clone())
                .accessibility_label(remove_label)
                .on_click(on_remove),
        )
}

/// A keyboard-shortcut hint; unparseable shortcuts fall back to the raw text.
pub(crate) fn kbd_hint(shortcut: &str, cx: &App) -> AnyElement {
    match Keystroke::parse_with_os(shortcut, cx.platform_info().os) {
        Ok(stroke) => Kbd::new(stroke).outline().into_any_element(),
        Err(_) => div()
            .px_1()
            .rounded(radius_sm(cx))
            .border_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(shortcut.to_owned()))
            .into_any_element(),
    }
}

/// A dialog's action row: right-aligned, one gap, no per-dialog decisions.
pub(crate) fn dialog_footer(children: impl IntoIterator<Item = AnyElement>) -> Div {
    div()
        .h_flex()
        .w_full()
        .items_center()
        .justify_end()
        .gap_2()
        .children(children)
}

/// One entry of a [`picker_button`] dropdown.
///
/// Selection and disabled state travel with the entry so a picker cannot
/// render without them.
pub(crate) struct PickerOption {
    pub label: SharedString,
    pub selected: bool,
    pub disabled: bool,
}

impl PickerOption {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            selected: false,
            disabled: false,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Handler invoked with the index of the chosen picker option.
type PickerHandler = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A compact outline button that always renders its menu with checkmarks.
pub(crate) fn picker_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    options: Vec<PickerOption>,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    picker_button_with(id, label, options, false, None, on_select)
}

/// A picker whose trigger can be disabled and whose menu can scroll.
///
/// A bounded menu (`max_h`) scrolls; an unbounded one sizes to its items. The
/// height is expressed in `rem` so it follows the interface zoom.
pub(crate) fn picker_button_with(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    options: Vec<PickerOption>,
    disabled: bool,
    max_h: Option<Rems>,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_select: PickerHandler = std::rc::Rc::new(on_select);
    Button::new(id)
        .small()
        .outline()
        .dropdown_caret(true)
        .disabled(disabled)
        .label(label)
        .dropdown_menu(move |menu, window, _| {
            let mut menu = menu.scrollable(max_h.is_some());
            if let Some(max_h) = max_h {
                menu = menu.max_h(max_h.to_pixels(window.rem_size()));
            }
            for (index, option) in options.iter().enumerate() {
                let handler = on_select.clone();
                menu = menu.item(
                    PopupMenuItem::new(option.label.clone())
                        .checked(option.selected)
                        .disabled(option.disabled)
                        .on_click(move |_, window, cx| handler(index, window, cx)),
                );
            }
            menu
        })
}
