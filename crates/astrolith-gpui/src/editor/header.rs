//! Zed-style document chrome: open-document tabs above a breadcrumb bar.
//!
//! The tab strip and breadcrumb are a projection of [`ProjectWorkspace`]'s
//! retained sessions. This view owns no document state; it only renders the
//! cached tab model and dispatches activation/save/close back to the workspace.

use crate::project_discovery::ProjectWorkspace;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    tab::{Tab, TabBar},
};
use gpui_kit::{
    Context, Entity, IntoElement, Render, SharedString, Subscription, TestSupportExt, Window, div,
    prelude::*,
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.project.read(cx).open_tabs();
        let active = self.project.read(cx).active_tab_index();
        let project_name = self.project.read(cx).project_name();
        let has_tabs = !tabs.is_empty();

        let tab_bar = if has_tabs {
            let tabs_for_click = tabs.clone();
            TabBar::new("document-tabs")
                .small()
                .selected_index(active)
                .menu(true)
                .children(tabs.iter().enumerate().map(|(index, tab)| {
                    let close_path = tab.path.clone();
                    let close_owner = cx.weak_entity();
                    Tab::new()
                        .label(tab.label.clone())
                        .when(tab.dirty, |tab| {
                            tab.prefix(div().size_2().rounded_full().bg(cx.theme().foreground))
                        })
                        .suffix(
                            Button::new(("tab-close", index))
                                .xsmall()
                                .ghost()
                                .icon(IconName::X)
                                .tooltip("Close file")
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    let _ = close_owner
                                        .update(cx, |this, cx| this.close(&close_path, window, cx));
                                }),
                        )
                }))
                .on_click(cx.listener(move |this, index: &usize, window, cx| {
                    if let Some(tab) = tabs_for_click.get(*index) {
                        let path = tab.path.clone();
                        this.project.update(cx, |project, cx| {
                            project.activate_session(&path, window, cx)
                        });
                    }
                }))
                .into_any_element()
        } else {
            div()
                .h_flex()
                .h_9()
                .items_center()
                .px_3()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("No file open")
                .into_any_element()
        };

        let active_tab = tabs.get(active);
        div()
            .id("document-header")
            .test_support()
            .v_flex()
            .flex_shrink_0()
            .bg(cx.theme().tab_bar)
            .border_b_1()
            .border_color(cx.theme().window_border)
            .child(tab_bar)
            .child(
                div()
                    .h_flex()
                    .h_7()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .border_t_1()
                    .border_color(cx.theme().window_border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::new(IconName::Folder).small())
                    .child(
                        project_name
                            .clone()
                            .unwrap_or_else(|| SharedString::from("Astrolith")),
                    )
                    .when_some(
                        active_tab.and_then(|tab| tab.collection.clone()),
                        |row, collection| {
                            row.child(Icon::new(IconName::ChevronRight).xsmall())
                                .child(collection)
                        },
                    )
                    .when_some(active_tab, |row, tab| {
                        row.child(Icon::new(IconName::ChevronRight).xsmall())
                            .child(tab.label.clone())
                    })
                    .child(div().flex_1())
                    .when_some(active_tab, |row, tab| {
                        row.when(tab.externally_changed, |row| {
                            row.child(
                                div()
                                    .h_flex()
                                    .gap_1()
                                    .text_color(cx.theme().warning)
                                    .child(Icon::new(IconName::TriangleAlert).xsmall())
                                    .child("Changed on disk"),
                            )
                        })
                        .child(
                            Button::new("save-document")
                                .xsmall()
                                .ghost()
                                .icon(IconName::Save)
                                .tooltip("Save")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.project
                                        .update(cx, |project, cx| project.save_active(window, cx));
                                })),
                        )
                        .child(
                            Button::new("close-document")
                                .xsmall()
                                .ghost()
                                .icon(IconName::X)
                                .tooltip("Close file")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let path = this.project.read(cx).active_path();
                                    if let Some(path) = path {
                                        this.project.update(cx, |project, cx| {
                                            project.close_path(path, window, cx)
                                        });
                                    }
                                })),
                        )
                    }),
            )
    }
}

impl DocumentHeader {
    fn close(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            project.close_path(path.to_path_buf(), window, cx)
        });
    }
}
