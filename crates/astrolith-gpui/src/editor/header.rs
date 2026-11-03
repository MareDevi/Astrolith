//! Document rail: the active document's name and the documents beside it.
//!
//! The strip is a projection of [`ProjectWorkspace`]'s retained sessions. This
//! view owns no document state; it only renders the cached tab model and
//! dispatches activation/save/close back to the workspace.
//!
//! It is deliberately a single quiet line rather than a stack of tab chrome:
//! the writer should see the page, not an editor window. Additional open
//! documents stay one click away in the overflow menu.

use crate::editor::save_coordinator::SaveStatus;
use crate::project_discovery::ProjectWorkspace;
use crate::ui;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    Context, Entity, IntoElement, Render, SharedString, Subscription, TestSupportExt, Window, div,
    prelude::*, px,
};

pub(crate) struct DocumentHeader {
    project: Entity<ProjectWorkspace>,
    _subscription: Subscription,
}

impl DocumentHeader {
    pub(crate) fn new(project: Entity<ProjectWorkspace>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&project, |_, _, cx| cx.notify());
        Self {
            project,
            _subscription: subscription,
        }
    }
}

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
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("No document open"),
                )
                .into_any_element();
        }
        let active_tab = tabs.get(active).cloned();
        let title = active_tab
            .as_ref()
            .map(|tab| tab.label.clone())
            .unwrap_or_else(|| "Document".into());
        let collection = active_tab.as_ref().and_then(|tab| tab.collection.clone());
        let dirty = active_tab.as_ref().is_some_and(|tab| tab.dirty);
        let externally_changed = active_tab
            .as_ref()
            .is_some_and(|tab| tab.externally_changed);

        let save_status = self.project.read(cx).active_save_status();
        let (state, state_tone): (SharedString, _) = match save_status {
            Some(SaveStatus::Conflict) => ("Conflict".into(), cx.theme().danger),
            Some(SaveStatus::Saving) => ("Saving…".into(), cx.theme().warning),
            Some(SaveStatus::Failed(_)) => ("Save failed".into(), cx.theme().danger),
            Some(SaveStatus::Unsaved) => ("Unsaved".into(), cx.theme().warning),
            Some(SaveStatus::Saved) if externally_changed => {
                ("Changed on disk".into(), cx.theme().warning)
            }
            Some(SaveStatus::Saved) => ("Saved".into(), cx.theme().success),
            None if externally_changed => ("Changed on disk".into(), cx.theme().warning),
            None if dirty => ("Unsaved".into(), cx.theme().warning),
            None => ("Saved".into(), cx.theme().success),
        };
        let counts = self
            .project
            .read(cx)
            .active_counts(cx)
            .map(|counts| format!("{} words · {} characters", counts.words, counts.characters));

        let active_path = self.project.read(cx).active_path();
        let menu_owner = self.project.downgrade();
        let menu_tabs: Vec<(SharedString, std::path::PathBuf)> = tabs
            .iter()
            .map(|tab| (tab.label.clone(), tab.path.clone()))
            .collect();
        let menu_active = active_path.clone();

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
                            .child(crate::ui::document_title(title, cx))
                            .when_some(collection, |row, collection| {
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
                        ui::icon_button("save-document", IconName::Save, "Save file").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.project
                                    .update(cx, |project, cx| project.save_active(window, cx));
                            }),
                        ),
                    )
                    .child(
                        Button::new("document-more")
                            .small()
                            .ghost()
                            .icon(IconName::Ellipsis)
                            .tooltip("Document actions")
                            .dropdown_menu(move |menu, _, _| {
                                let mut menu = menu;
                                if let Some(path) = menu_active.clone() {
                                    let owner = menu_owner.clone();
                                    menu =
                                        menu.item(PopupMenuItem::new("Close document").on_click(
                                            move |_, window, cx| {
                                                let _ = owner.update(cx, |project, cx| {
                                                    project.close_path(path.clone(), window, cx)
                                                });
                                            },
                                        ));
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
            .into_any_element()
    }
}
