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
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    Animation, AnimationExt as _, Context, Entity, EventEmitter, IntoElement, Render, SharedString,
    Subscription, TestSupportExt, Window, div, prelude::*, px,
};

pub(crate) struct InspectorRequested;
pub(crate) struct LibraryRequested;
pub(crate) struct WorkspaceCommandRequested(pub(crate) CommandId);

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
        let active = self.project.read(cx).active_tab_index();
        if tabs.is_empty() {
            return div()
                .id("document-header")
                .test_support()
                .h_flex()
                .flex_shrink_0()
                .items_center()
                .h(px(36.))
                .px_4()
                .bg(cx.theme().background)
                .border_b_1()
                .border_color(cx.theme().border.opacity(0.55))
                .child(self.library_control(cx))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr!("editor.header.no_document")),
                )
                .into_any_element();
        }
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
        let enter_duration = cx.theme().motion_tokens().duration_fast;

        let save_status = self.project.read(cx).active_save_status();
        let (state, state_tone): (SharedString, _) = match save_status {
            Some(SaveStatus::Conflict) => {
                (tr!("editor.save_status.conflict").into(), cx.theme().danger)
            }
            Some(SaveStatus::Saving) => {
                (tr!("editor.save_status.saving").into(), cx.theme().warning)
            }
            Some(SaveStatus::Failed(_)) => {
                (tr!("editor.save_status.failed").into(), cx.theme().danger)
            }
            Some(SaveStatus::Unsaved) => {
                (tr!("editor.save_status.unsaved").into(), cx.theme().warning)
            }
            Some(SaveStatus::Saved) if externally_changed => (
                tr!("editor.save_status.changed_on_disk").into(),
                cx.theme().warning,
            ),
            Some(SaveStatus::Saved) => (tr!("editor.save_status.saved").into(), cx.theme().success),
            None if externally_changed => (
                tr!("editor.save_status.changed_on_disk").into(),
                cx.theme().warning,
            ),
            None if dirty => (tr!("editor.save_status.unsaved").into(), cx.theme().warning),
            None => (tr!("editor.save_status.saved").into(), cx.theme().success),
        };
        let counts = self.project.read(cx).active_counts(cx).map(|counts| {
            tr!(
                "editor.header.word_count",
                words = counts.words,
                characters = counts.characters
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
        div()
            .id("document-header")
            .test_support()
            .v_flex()
            .flex_shrink_0()
            .bg(cx.theme().background)
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.55))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_3()
                    .h(px(52.))
                    .px_4()
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .when_some(collection.clone(), |row, _| {
                                row.child(self.library_control(cx))
                            })
                            .child(crate::ui::document_title(title, cx))
                            .when_some(collection.clone(), |row, collection| {
                                row.child(crate::ui::v_divider(cx)).child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .truncate()
                                        .child(collection),
                                )
                            }),
                    )
                    .child(div().flex_1())
                    .when_some((!compact).then_some(counts).flatten(), |row, counts| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(counts),
                        )
                    })
                    .child(ui::v_divider(cx))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(state_tone)
                            .child(div().size_1p5().rounded_full().bg(state_tone))
                            .child(state),
                    )
                    .child(
                        ui::icon_button(
                            "save-document",
                            IconName::Save,
                            tr!("editor.header.save_file"),
                        )
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
                        Button::new("document-more")
                            .small()
                            .ghost()
                            .icon(IconName::Ellipsis)
                            .tooltip(tr!("editor.header.document_actions"))
                            .accessibility_label(tr!("editor.header.document_actions"))
                            .dropdown_menu(move |menu, _, _| {
                                let mut menu = menu;
                                if let Some(path) = menu_active.clone() {
                                    let owner = menu_owner.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(tr!("editor.header.close_document"))
                                            .on_click(move |_, window, cx| {
                                                let _ = owner.update(cx, |project, cx| {
                                                    project.close_path(path.clone(), window, cx)
                                                });
                                            }),
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
    fn writing_mode_control(&self, control: WritingModeControl, cx: &mut Context<Self>) -> Button {
        let WritingModeControl {
            id,
            label,
            tooltip,
            icon,
            command,
            active,
        } = control;
        Button::new(id)
            .small()
            .ghost()
            .icon(icon)
            .selected(active)
            .tooltip(tooltip)
            .accessibility_label(label)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(WorkspaceCommandRequested(command));
            }))
    }

    fn library_control(&self, cx: &mut Context<Self>) -> Button {
        Button::new("toggle-library")
            .small()
            .ghost()
            .icon(IconName::PanelLeft)
            .tooltip(tr!("editor.header.toggle_library"))
            .accessibility_label(tr!("editor.header.toggle_library"))
            .selected(self.library_open)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(LibraryRequested)))
    }

    fn inspector_control(&self, cx: &mut Context<Self>) -> Button {
        let available = self.project.read(cx).active_frontmatter_key().is_some();
        Button::new("toggle-inspector")
            .small()
            .ghost()
            .icon(IconName::SlidersHorizontal)
            .tooltip(if available {
                tr!("editor.header.edit_frontmatter")
            } else {
                tr!("editor.header.no_frontmatter")
            })
            .accessibility_label(if available {
                tr!("editor.header.edit_frontmatter")
            } else {
                tr!("editor.header.frontmatter_unavailable")
            })
            .selected(self.inspector_open)
            .disabled(!available)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(InspectorRequested)))
    }
}
