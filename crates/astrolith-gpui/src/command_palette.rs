//! Application-owned command palette surface and navigation.

use std::rc::Rc;

use crate::commands::CommandId;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Icon, StyledExt};
use gpui_kit::{
    App, Context, Entity, Focusable, IntoElement, Render, ScrollHandle, SharedString, Subscription,
    Window, div, prelude::*,
};

#[derive(Clone, Debug)]
pub(crate) enum PaletteAction {
    Command(CommandId),
    RecentProject(String),
    Collection(String),
}

#[derive(Clone, Debug)]
pub(crate) struct PaletteEntry {
    pub(crate) label: SharedString,
    pub(crate) shortcut: SharedString,
    pub(crate) icon: IconName,
    pub(crate) action: PaletteAction,
    pub(crate) disabled: bool,
}

impl PaletteEntry {
    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled
    }
}

pub(crate) type PaletteGroups = Vec<(String, Vec<PaletteEntry>)>;
type Confirm = Rc<dyn Fn(PaletteAction, &mut Window, &mut App)>;

pub(crate) struct CommandPalette {
    query: Entity<InputState>,
    groups: PaletteGroups,
    selected: Option<usize>,
    matched_count: usize,
    scroll: ScrollHandle,
    on_confirm: Option<Confirm>,
    _subscriptions: Vec<Subscription>,
}

impl CommandPalette {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr!("shell.palette.search")));
        let subscription = cx.subscribe_in(
            &query,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let query = this.query.read(cx).value();
                    this.matched_count = this.filtered(&query).iter().map(Vec::len).sum();
                    this.selected = this.first_enabled(&query);
                    this.scroll_to_selection(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.confirm(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            },
        );
        Self {
            query,
            groups: Vec::new(),
            selected: None,
            matched_count: 0,
            scroll: ScrollHandle::new(),
            on_confirm: None,
            _subscriptions: vec![subscription],
        }
    }

    pub(crate) fn set_groups(
        &mut self,
        mut groups: PaletteGroups,
        opening: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (_, entries) in &mut groups {
            for entry in entries {
                entry.disabled =
                    opening && matches!(entry.action, PaletteAction::Command(CommandId::OpenFile));
            }
        }
        self.groups = groups;
        self.query
            .update(cx, |query, cx| query.set_value("", window, cx));
        self.matched_count = self.groups.iter().map(|(_, entries)| entries.len()).sum();
        self.selected = self.first_enabled(&SharedString::default());
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    pub(crate) fn set_on_confirm(&mut self, on_confirm: Confirm) {
        self.on_confirm = Some(on_confirm);
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.query.read(cx).focus_handle(cx).focus(window, cx);
    }

    #[cfg(test)]
    pub(crate) fn matched_count(&self) -> usize {
        self.matched_count
    }

    fn matches(&self, label: &str, query: &str) -> bool {
        query.is_empty() || label.to_lowercase().contains(&query.to_lowercase())
    }

    fn filtered(&self, query: &str) -> Vec<Vec<usize>> {
        self.groups
            .iter()
            .map(|(_, entries)| {
                entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        self.matches(&entry.label, query) || self.matches(&entry.shortcut, query)
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .collect()
    }

    fn first_enabled(&self, query: &str) -> Option<usize> {
        self.filtered(query)
            .into_iter()
            .flatten()
            .scan(0, |flat_index, entry_ix| {
                let index = *flat_index;
                *flat_index += 1;
                Some((index, entry_ix))
            })
            .find_map(|(index, _)| {
                self.entry_at(query, index)
                    .filter(|entry| !entry.is_disabled())
                    .map(|_| index)
            })
    }

    fn entry_at(&self, query: &str, flat_index: usize) -> Option<&PaletteEntry> {
        let mut index = 0;
        for (group_ix, (_, entries)) in self.groups.iter().enumerate() {
            let visible = self.filtered_group(query, group_ix);
            for entry_ix in visible {
                if index == flat_index {
                    return entries.get(entry_ix);
                }
                index += 1;
            }
        }
        None
    }

    fn filtered_group(&self, query: &str, group_ix: usize) -> Vec<usize> {
        self.groups
            .get(group_ix)
            .map(|(_, entries)| {
                entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        self.matches(&entry.label, query) || self.matches(&entry.shortcut, query)
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn move_selection(&mut self, direction: isize, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value();
        let filtered = self.filtered(&query);
        let count = filtered.iter().map(Vec::len).sum::<usize>();
        if count == 0 {
            self.selected = None;
            return;
        }
        let start = self
            .selected
            .unwrap_or(if direction > 0 { count - 1 } else { 0 });
        for step in 1..=count {
            let index =
                (start as isize + direction * step as isize).rem_euclid(count as isize) as usize;
            if self
                .entry_at(&query, index)
                .is_some_and(|entry| !entry.is_disabled())
            {
                self.selected = Some(index);
                self.scroll_to_selection(cx);
                cx.notify();
                return;
            }
        }
    }

    fn scroll_to_selection(&self, cx: &App) {
        let Some(selected) = self.selected else {
            return;
        };
        let query = self.query.read(cx).value();
        let filtered = self.filtered(&query);
        let mut item_index = 0;
        let mut rendered_index = 0;
        for entries in filtered {
            if entries.is_empty() {
                continue;
            }
            rendered_index += 1;
            for _ in entries {
                if item_index == selected {
                    self.scroll.scroll_to_item(rendered_index);
                    return;
                }
                item_index += 1;
                rendered_index += 1;
            }
        }
    }

    fn confirm(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self.selected else {
            return;
        };
        let query = self.query.read(cx).value();
        let Some(action) = self
            .entry_at(&query, selected)
            .map(|entry| entry.action.clone())
        else {
            return;
        };
        if let Some(on_confirm) = self.on_confirm.clone() {
            on_confirm(action, window, cx);
        }
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query.read(cx).value();
        let filtered = self.filtered(&query);
        let has_results = filtered.iter().any(|entries| !entries.is_empty());
        let mut flat_index = 0;
        let mut results = div()
            .id("command-palette-results")
            .v_flex()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_2()
            .gap_1();

        for (group_index, entry_indices) in filtered.iter().enumerate() {
            if entry_indices.is_empty() {
                continue;
            }
            let Some((title, entries)) = self.groups.get(group_index) else {
                continue;
            };
            results = results.child(
                div()
                    .w_full()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.clone()),
            );
            for entry_index in entry_indices {
                let Some(entry) = entries.get(*entry_index) else {
                    continue;
                };
                let selected = self.selected == Some(flat_index) && !entry.is_disabled();
                let label = entry.label.clone();
                let shortcut = entry.shortcut.clone();
                let icon = entry.icon;
                let disabled = entry.is_disabled();
                let item_index = flat_index;
                results = results.child(
                    div()
                        .id(format!("command-palette-item-{item_index}"))
                        .w_full()
                        .min_w_0()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded(cx.theme().radius)
                        .when(selected, |row| {
                            row.bg(cx.theme().accent)
                                .text_color(cx.theme().accent_foreground)
                        })
                        .when(!selected && !disabled, |row| {
                            row.hover(|style| style.bg(cx.theme().list_hover))
                        })
                        .when(disabled, |row| row.text_color(cx.theme().muted_foreground))
                        .when(!disabled, |row| {
                            row.on_click(cx.listener(move |this, _, window, cx| {
                                this.selected = Some(item_index);
                                this.confirm(window, cx);
                            }))
                        })
                        .child(Icon::new(icon).size_4().text_color(if selected {
                            cx.theme().accent_foreground
                        } else {
                            cx.theme().muted_foreground
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(label),
                        )
                        .when(!shortcut.is_empty(), |row| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(shortcut),
                            )
                        }),
                );
                flat_index += 1;
            }
        }

        if !has_results {
            results = results.child(
                div()
                    .px_3()
                    .py_8()
                    .w_full()
                    .text_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr!("shell.palette.empty")),
            );
        }

        div()
            .id("command-palette-surface")
            .w_full()
            .h_full()
            .min_h_0()
            .v_flex()
            .overflow_hidden()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .track_focus(&self.query.read(cx).focus_handle(cx))
            .key_context("AstrolithPalette")
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "down" => this.move_selection(1, cx),
                        "up" => this.move_selection(-1, cx),
                        "escape" if !this.query.read(cx).value().is_empty() => {
                            this.query
                                .update(cx, |query, cx| query.set_value("", window, cx));
                            this.matched_count = this.filtered("").iter().map(Vec::len).sum();
                            this.selected = this.first_enabled("");
                            this.scroll_to_selection(cx);
                            cx.notify();
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(IconName::Search)
                            .size_4()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(Input::new(&self.query).appearance(false).p_0()),
            )
            .child(results)
    }
}
