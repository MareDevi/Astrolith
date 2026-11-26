//! Document rail: the active document's name and the documents beside it.
//!
//! The strip is a projection of [`ProjectWorkspace`]'s retained sessions. This
//! view owns no document state; it only renders the cached tab model and
//! dispatches activation/save/close back to the workspace.
//!
//! It is deliberately a single quiet line rather than a stack of tab chrome:
//! the writer should see the page, not an editor window. Additional open
//! documents stay one click away in the overflow menu.

use crate::commands::CommandId;
use crate::editor::save_coordinator::SaveStatus;
use crate::project_discovery::ProjectWorkspace;
use crate::ui;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable, StyledExt,
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
    spinner::Spinner,
    tooltip::Tooltip,
};
use gpui_kit::{
    Animation, AnimationExt as _, Context, Entity, EventEmitter, Hsla, IntoElement, Render,
    SharedString, Subscription, TestSupportExt, Window, div, prelude::*, rems,
};

pub(crate) struct InspectorRequested;
pub(crate) struct LibraryRequested;
pub(crate) struct WorkspaceCommandRequested(pub(crate) CommandId);

/// The rail height, in rem, shared by the empty and active headers.
pub(crate) const RAIL_HEIGHT_REM: f32 = 2.75;

/// The single rail geometry both header states lay their content on.
fn rail() -> gpui_kit::Div {
    div()
        .h_flex()
        .items_center()
        .h(rems(RAIL_HEIGHT_REM))
        .px_4()
}

/// Saved and Saving have nothing left to write, so the save action is inert.
fn save_disabled(status: Option<&SaveStatus>) -> bool {
    matches!(status, Some(SaveStatus::Saved | SaveStatus::Saving))
}

/// One writing-mode toggle rendered on the document rail.
struct WritingModeControl {
    id: &'static str,
    label: String,
    tooltip: String,
    icon: IconName,
    command: CommandId,
    active: bool,
}

pub(crate) struct DocumentHeader {
    project: Entity<ProjectWorkspace>,
    inspector_open: bool,
    library_open: bool,
    _subscription: Subscription,
}

impl DocumentHeader {
    pub(crate) fn new(project: Entity<ProjectWorkspace>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&project, |_, _, cx| cx.notify());
        Self {
            project,
            inspector_open: false,
            library_open: false,
            _subscription: subscription,
        }
    }

    /// Mirror the shell's transient inspector state.
    pub(crate) fn set_inspector_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.inspector_open == open {
            return;
        }
        self.inspector_open = open;
        cx.notify();
    }

    pub(crate) fn set_library_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.library_open == open {
            return;
        }
        self.library_open = open;
        cx.notify();
    }
}

impl EventEmitter<InspectorRequested> for DocumentHeader {}
impl EventEmitter<LibraryRequested> for DocumentHeader {}
impl EventEmitter<WorkspaceCommandRequested> for DocumentHeader {}

impl Render for DocumentHeader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.project.read(cx).open_tabs();
        let compact = f32::from(window.viewport_size().width) < 1100.;
        let enter_duration = cx.theme().motion_tokens().duration_fast;

        let row = if tabs.is_empty() {
            rail()
                .gap_2()
                .child(self.library_control(cx))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr!("editor.header.no_document")),
                )
                .into_any_element()
        } else {
            self.active_row(&tabs, compact, cx).into_any_element()
        };

        div()
            .id("document-header")
            .test_support()
            .v_flex()
            .flex_shrink_0()
            .bg(cx.theme().background)
            .border_b_1()
            .border_color(cx.theme().window_border)
            .child(row)
            .with_animation(
                "document-header-appear",
                Animation::new(enter_duration)
                    .with_easing(gpui_kit::base::animation::ease_out_cubic),
                |header, progress| header.opacity(progress),
            )
            .into_any_element()
    }
}

impl DocumentHeader {
    /// The rail for an open document.
    fn active_row(
        &self,
        tabs: &[crate::project_discovery::OpenTab],
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.project.read(cx).active_tab_index();
        let active_tab = tabs.get(active).cloned();
        let title = active_tab
            .as_ref()
            .map(|tab| tab.label.clone())
            .unwrap_or_else(|| tr!("editor.header.document_fallback").into());
        let collection = active_tab.as_ref().and_then(|tab| tab.collection.clone());
        let dirty = active_tab.as_ref().is_some_and(|tab| tab.dirty);
        let externally_changed = active_tab
            .as_ref()
            .is_some_and(|tab| tab.externally_changed);

        let status = self.project.read(cx).active_save_status();
        let save_disabled = save_disabled(status.as_ref());
        let (state, state_tone, saving, failure): (SharedString, Hsla, bool, Option<SharedString>) =
            match status {
                Some(SaveStatus::Conflict) => (
                    tr!("editor.save_status.conflict").into(),
                    cx.theme().danger,
                    false,
                    None,
                ),
                Some(SaveStatus::Saving) => (
                    tr!("editor.save_status.saving").into(),
                    cx.theme().muted_foreground,
                    true,
                    None,
                ),
                Some(SaveStatus::Failed(reason)) => (
                    tr!("editor.save_status.failed").into(),
                    cx.theme().danger,
                    false,
                    Some(SharedString::from(reason)),
                ),
                Some(SaveStatus::Unsaved) => (
                    tr!("editor.save_status.unsaved").into(),
                    cx.theme().warning,
                    false,
                    None,
                ),
                Some(SaveStatus::Saved) if externally_changed => (
                    tr!("editor.save_status.changed_on_disk").into(),
                    cx.theme().warning,
                    false,
                    None,
                ),
                Some(SaveStatus::Saved) => (
                    tr!("editor.save_status.saved").into(),
                    cx.theme().muted_foreground,
                    false,
                    None,
                ),
                None if externally_changed => (
                    tr!("editor.save_status.changed_on_disk").into(),
                    cx.theme().warning,
                    false,
                    None,
                ),
                None if dirty => (
                    tr!("editor.save_status.unsaved").into(),
                    cx.theme().warning,
                    false,
                    None,
                ),
                None => (
                    tr!("editor.save_status.saved").into(),
                    cx.theme().muted_foreground,
                    false,
                    None,
                ),
            };
        let counts = (!compact)
            .then(|| self.project.read(cx).active_counts(cx))
            .flatten()
            .map(|counts| {
                tr!(
                    "editor.header.word_count",
                    words = counts.words,
                    characters = counts.characters
                )
            });

        let leading = div()
            .h_flex()
            .items_center()
            .gap_2()
            .flex_1()
            .min_w_0()
            .child(self.library_control(cx))
            .child(ui::document_title(title, cx))
            .when_some(collection, |row, collection| {
                row.child(ui::v_divider(cx)).child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(collection),
                )
            });

        let active_path = self.project.read(cx).active_path();
        let menu_owner = self.project.downgrade();
        let menu_tabs: Vec<(SharedString, std::path::PathBuf)> = tabs
            .iter()
            .map(|tab| (tab.label.clone(), tab.path.clone()))
            .collect();
        let menu_active = active_path.clone();
        let writing_modes = self.project.read(cx).writing_modes();

        rail().gap_3().child(leading).child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .when_some(counts, |row, counts| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(counts),
                    )
                    .child(ui::v_divider(cx))
                })
                .child(save_indicator(state, state_tone, saving, failure))
                .child(
                    ui::icon_button(
                        "save-document",
                        IconName::Save,
                        tr!("editor.header.save_file"),
                    )
                    .disabled(save_disabled)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.project
                            .update(cx, |project, cx| project.save_active(window, cx));
                    })),
                )
                .child(self.writing_mode_control(
                    WritingModeControl {
                        id: "toggle-focus-mode",
                        label: tr!("editor.header.focus_mode"),
                        tooltip: if writing_modes.focus {
                            tr!("editor.header.focus_mode_off")
                        } else {
                            tr!("editor.header.focus_mode_on")
                        },
                        icon: IconName::Eye,
                        command: CommandId::ToggleFocusMode,
                        active: writing_modes.focus,
                    },
                    cx,
                ))
                .child(self.writing_mode_control(
                    WritingModeControl {
                        id: "toggle-typewriter-mode",
                        label: tr!("editor.header.typewriter_mode"),
                        tooltip: if writing_modes.typewriter {
                            tr!("editor.header.typewriter_mode_off")
                        } else {
                            tr!("editor.header.typewriter_mode_on")
                        },
                        icon: IconName::Type,
                        command: CommandId::ToggleTypewriterMode,
                        active: writing_modes.typewriter,
                    },
                    cx,
                ))
                .child(self.inspector_control(cx))
                .child(
                    ui::icon_button(
                        "document-more",
                        IconName::Ellipsis,
                        tr!("editor.header.document_actions"),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu;
                        if let Some(path) = menu_active.clone() {
                            let owner = menu_owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(tr!("editor.header.close_document")).on_click(
                                    move |_, window, cx| {
                                        let _ = owner.update(cx, |project, cx| {
                                            project.close_path(path.clone(), window, cx)
                                        });
                                    },
                                ),
                            );
                        }
                        if menu_tabs.len() > 1 {
                            menu = menu.separator();
                            for (label, path) in menu_tabs.iter().cloned() {
                                let owner = menu_owner.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .checked(menu_active.as_ref() == Some(&path))
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |project, cx| {
                                                project.activate_session(&path, window, cx)
                                            });
                                        }),
                                );
                            }
                        }
                        menu
                    }),
                ),
        )
    }

    fn writing_mode_control(&self, control: WritingModeControl, cx: &mut Context<Self>) -> Button {
        let WritingModeControl {
            id,
            label,
            tooltip,
            icon,
            command,
            active,
        } = control;
        ui::icon_button(id, icon, label)
            .selected(active)
            .tooltip(tooltip)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(WorkspaceCommandRequested(command));
            }))
    }

    fn library_control(&self, cx: &mut Context<Self>) -> Button {
        ui::icon_button(
            "toggle-library",
            IconName::PanelLeft,
            tr!("editor.header.toggle_library"),
        )
        .selected(self.library_open)
        .on_click(cx.listener(|_, _, _, cx| cx.emit(LibraryRequested)))
    }

    fn inspector_control(&self, cx: &mut Context<Self>) -> Button {
        let available = self.project.read(cx).active_frontmatter_key().is_some();
        ui::icon_button(
            "toggle-inspector",
            IconName::SlidersHorizontal,
            if available {
                tr!("editor.header.edit_frontmatter")
            } else {
                tr!("editor.header.frontmatter_unavailable")
            },
        )
        .selected(self.inspector_open)
        .disabled(!available)
        .on_click(cx.listener(|_, _, _, cx| cx.emit(InspectorRequested)))
    }
}

/// The save state: a named dot (or spinner while writing) plus its reason.
fn save_indicator(
    label: SharedString,
    tone: Hsla,
    saving: bool,
    failure: Option<SharedString>,
) -> impl IntoElement {
    let row = div()
        .h_flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(tone)
        .when(saving, |row| row.child(Spinner::new().xsmall()))
        .when(!saving, |row| row.child(ui::status_dot(tone)))
        .child(label);
    match failure {
        Some(reason) => row
            .id("save-status")
            .tooltip(move |window, cx| Tooltip::new(reason.clone()).build(window, cx))
            .into_any_element(),
        None => row.into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::save_disabled;
    use crate::editor::save_coordinator::SaveStatus;

    #[test]
    fn save_is_inert_only_while_clean_or_saving() {
        assert!(!save_disabled(None));
        assert!(!save_disabled(Some(&SaveStatus::Unsaved)));
        assert!(!save_disabled(Some(&SaveStatus::Conflict)));
        assert!(!save_disabled(Some(&SaveStatus::Failed("disk".to_owned()))));
        assert!(save_disabled(Some(&SaveStatus::Saved)));
        assert!(save_disabled(Some(&SaveStatus::Saving)));
    }
}
