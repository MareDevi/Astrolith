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
use crate::frontmatter::FrontmatterForm;
use crate::project_discovery::ProjectWorkspace;
use crate::ui;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    popover::Popover,
};
use gpui_kit::{
    Anchor, AnyElement, Context, Entity, IntoElement, Render, SharedString, Subscription,
    TestSupportExt, Window, div, prelude::*, px,
};

pub(crate) struct DocumentHeader {
    project: Entity<ProjectWorkspace>,
    frontmatter: Entity<FrontmatterForm>,
    inspector_open: bool,
    _subscription: Subscription,
}

impl DocumentHeader {
    pub(crate) fn new(
        project: Entity<ProjectWorkspace>,
        frontmatter: Entity<FrontmatterForm>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&project, |_, _, cx| cx.notify());
        Self {
            project,
            frontmatter,
            inspector_open: false,
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

    /// Whether the floating inspector is open.
    pub(crate) fn inspector_open(&self) -> bool {
        self.inspector_open
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
                        ui::icon_button("save-document", IconName::Save, "Save file").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.project
                                    .update(cx, |project, cx| project.save_active(window, cx));
                            }),
                        ),
                    )
                    .child(self.inspector_control(collection, cx))
                    .child(
                        Button::new("document-more")
                            .small()
                            .ghost()
                            .icon(IconName::Ellipsis)
                            .tooltip("Document actions")
                            .accessibility_label("Document actions")
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

impl DocumentHeader {
    /// The inspector trigger and its floating form.
    fn inspector_control(
        &self,
        collection: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let available = self.project.read(cx).active_frontmatter_key().is_some();
        if !available {
            return Button::new("toggle-inspector")
                .small()
                .ghost()
                .icon(IconName::SlidersHorizontal)
                .tooltip("No frontmatter for the active document")
                .accessibility_label("Frontmatter unavailable for the active document")
                .disabled(true)
                .into_any_element();
        }
        let form = self.frontmatter.clone();
        let subtitle = match collection {
            Some(collection) => SharedString::from(format!("Entry details · {collection}")),
            None => SharedString::from("Entry details"),
        };
        Popover::new("frontmatter-inspector")
            .anchor(Anchor::TopRight)
            .offset(px(6.))
            .open(self.inspector_open)
            .on_open_change(cx.listener(|this, open, _, cx| {
                this.inspector_open = *open;
                cx.notify();
            }))
            .trigger(
                Button::new("toggle-inspector")
                    .small()
                    .ghost()
                    .icon(IconName::SlidersHorizontal)
                    .tooltip("Edit frontmatter")
                    .accessibility_label("Edit frontmatter")
                    .selected(self.inspector_open),
            )
            .content(move |_, window, cx| {
                let viewport = window.viewport_size();
                let width = (f32::from(viewport.width) - 32.).clamp(280., 416.);
                let height = (f32::from(viewport.height) - 144.).max(240.);
                let body = form.clone();
                let state = cx.entity();
                let subtitle = subtitle.clone();
                div()
                    .id("frontmatter-inspector-surface")
                    .test_support()
                    .v_flex()
                    .w(px(width))
                    .h(px(height))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_flex()
                            .items_start()
                            .justify_between()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.55))
                            .child(
                                div()
                                    .v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .font_family(crate::theme::display_font(cx))
                                            .text_sm()
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .child("Frontmatter"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(subtitle),
                                    ),
                            )
                            .child(
                                Button::new("inspector-close")
                                    .small()
                                    .ghost()
                                    .icon(IconName::X)
                                    .tooltip("Close frontmatter")
                                    .accessibility_label("Close frontmatter")
                                    .on_click(move |_, window, cx| {
                                        state.update(cx, |state, cx| state.dismiss(window, cx));
                                    }),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(body))
            })
            .into_any_element()
    }
}
