//! Project file explorer for the Build sidebar.
//!
//! The browser keeps a bounded tree snapshot plus the expanded folders, and
//! flattens them into a list. Scanning and opening files never happen in
//! `render`: the scan runs on the service lane, and a confirmed row only emits
//! the chosen path for the workspace to resolve.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use astrolith_workspace::files::{ProjectFile, absolute_path, scan_project_files};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Icon, IndexPath, Sizable, StyledExt,
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
    spinner::Spinner,
};
use gpui_kit::{
    Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Task,
    TestSupportExt, Window, div, prelude::*, px, rems,
};

use crate::runtime;

/// A file chosen in the explorer; the workspace resolves it to a document.
pub(crate) enum FileBrowserEvent {
    Open(PathBuf),
}

impl EventEmitter<FileBrowserEvent> for FileBrowser {}

/// One visible row: a file, or a directory the writer can expand.
struct FileRow {
    id: SharedString,
    label: SharedString,
    path: PathBuf,
    depth: usize,
    directory: bool,
    has_children: bool,
    expanded: bool,
}

struct FileList {
    rows: Vec<FileRow>,
    selected: Option<IndexPath>,
    active: Option<PathBuf>,
}

impl FileList {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: None,
            active: None,
        }
    }

    fn replace(&mut self, rows: Vec<FileRow>, selected: Option<usize>) {
        self.rows = rows;
        self.selected = selected.map(IndexPath::new);
    }
}

impl ListDelegate for FileList {
    type Item = ListItem;

    fn items_count(&self, _: usize, _: &gpui_kit::App) -> usize {
        self.rows.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let row = self.rows.get(ix.row)?;
        let active = self.active.as_ref() == Some(&row.path);
        let icon = if row.directory {
            if row.expanded {
                IconName::FolderOpen
            } else {
                IconName::Folder
            }
        } else {
            file_icon(row.label.as_ref())
        };
        Some(
            ListItem::new(row.id.clone())
                .h(rems(1.6))
                .px_1()
                .py_0()
                .accessibility_label(row.id.clone())
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_1p5()
                        .min_w_0()
                        .w_full()
                        .pl(rems(0.25 + row.depth as f32 * 0.75))
                        .rounded_md()
                        .hover(|row| row.bg(cx.theme().list_hover))
                        .when(active, |row| row.bg(cx.theme().list_active))
                        .child(
                            div()
                                .w(px(14.))
                                .flex_shrink_0()
                                .when(row.has_children, |slot| {
                                    slot.child(
                                        Icon::new(if row.expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .xsmall()
                                        .text_color(cx.theme().muted_foreground),
                                    )
                                }),
                        )
                        .child(
                            Icon::new(icon)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .child(row.label.clone()),
                        ),
                ),
        )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
    }
}

fn file_icon(name: &str) -> IconName {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("astro") => IconName::FileCode,
        Some("md" | "mdx") => IconName::FileText,
        _ => IconName::File,
    }
}

pub(crate) struct FileBrowser {
    root: Option<PathBuf>,
    report: Vec<ProjectFile>,
    /// Folder ids the writer expanded, preserved across rescans.
    expanded: HashSet<SharedString>,
    list: Entity<ListState<FileList>>,
    active: Option<PathBuf>,
    loading: bool,
    error: Option<String>,
    truncated: bool,
    /// Newest scan request; older results are discarded.
    request: u64,
    task: Option<Task<()>>,
    _subscription: Subscription,
}

impl FileBrowser {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = cx.new(|cx| ListState::new(FileList::new(), window, cx));
        let subscription = cx.subscribe(&list, |this, _, event: &ListEvent, cx| {
            if let ListEvent::Confirm(ix) = event {
                this.confirm(ix.row, cx);
            }
        });
        Self {
            root: None,
            report: Vec::new(),
            expanded: HashSet::new(),
            list,
            active: None,
            loading: false,
            error: None,
            truncated: false,
            request: 0,
            task: None,
            _subscription: subscription,
        }
    }

    /// Point the explorer at a project root and rescan when it changes.
    ///
    /// `refresh` also rescans an unchanged root, so entering Build after files
    /// changed elsewhere shows the current project.
    pub(crate) fn sync_root(
        &mut self,
        root: Option<PathBuf>,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        if self.root != root {
            self.root = root;
            self.expanded.clear();
            self.active = None;
            self.refresh(cx);
            return;
        }
        if refresh && self.root.is_some() {
            self.refresh(cx);
        }
    }

    /// Start one bounded scan; a newer request supersedes this one.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.request = self.request.wrapping_add(1);
            self.loading = false;
            self.error = None;
            self.truncated = false;
            self.report.clear();
            self.publish_rows(cx);
            cx.notify();
            return;
        };
        self.request = self.request.wrapping_add(1);
        let request = self.request;
        self.loading = true;
        self.error = None;
        let work = runtime::blocking(cx, move || scan_project_files(&root));
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| this.publish(request, result, cx));
        }));
        cx.notify();
    }

    fn publish(
        &mut self,
        request: u64,
        result: Result<astrolith_workspace::files::ProjectFileTree, String>,
        cx: &mut Context<Self>,
    ) {
        if request != self.request {
            return;
        }
        self.loading = false;
        self.task = None;
        match result {
            Ok(tree) => {
                self.error = None;
                self.truncated = tree.truncated;
                self.report = tree.entries;
            }
            Err(error) => {
                self.error = Some(error);
                self.truncated = false;
                self.report.clear();
            }
        }
        self.publish_rows(cx);
        cx.notify();
    }

    /// Highlight the file the workspace has open.
    pub(crate) fn set_active(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.active == path {
            return;
        }
        self.active = path;
        let active = self.active.clone();
        self.list.update(cx, |list, cx| {
            list.delegate_mut().active = active;
            cx.notify();
        });
    }

    /// Confirm one row: expand a directory, or open a file.
    fn confirm(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((id, path, directory, has_children)) =
            self.list.read(cx).delegate().rows.get(index).map(|row| {
                (
                    row.id.clone(),
                    row.path.clone(),
                    row.directory,
                    row.has_children,
                )
            })
        else {
            return;
        };
        if directory {
            if !has_children {
                return;
            }
            if !self.expanded.remove(&id) {
                self.expanded.insert(id);
            }
            self.publish_rows(cx);
            return;
        }
        cx.emit(FileBrowserEvent::Open(path));
    }

    /// Rebuild the visible rows after a scan or an expansion change.
    fn publish_rows(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .list
            .read(cx)
            .delegate()
            .selected
            .and_then(|ix| self.list.read(cx).delegate().rows.get(ix.row))
            .map(|row| row.id.clone());
        let Some(root) = self.root.clone() else {
            self.list
                .update(cx, |list, _| list.delegate_mut().replace(Vec::new(), None));
            return;
        };
        let rows = visible_rows(&self.report, &root, &self.expanded);
        let selected = selected.and_then(|id| rows.iter().position(|row| row.id == id));
        let active = self.active.clone();
        self.list.update(cx, |list, cx| {
            let delegate = list.delegate_mut();
            delegate.active = active;
            delegate.replace(rows, selected);
            cx.notify();
        });
    }

    #[cfg(test)]
    pub(crate) fn is_scanning(&self) -> bool {
        self.loading
    }

    #[cfg(test)]
    pub(crate) fn visible_ids(&self, cx: &gpui_kit::App) -> Vec<String> {
        self.list
            .read(cx)
            .delegate()
            .rows
            .iter()
            .map(|row| row.id.to_string())
            .collect()
    }

    /// Select a row and focus the list, as arrow-key navigation would.
    #[cfg(test)]
    pub(crate) fn select_row(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let index = self
            .list
            .read(cx)
            .delegate()
            .rows
            .iter()
            .position(|row| row.id.as_ref() == id);
        let Some(index) = index else {
            return;
        };
        self.list.update(cx, |list, cx| {
            list.set_selected_index(Some(IndexPath::new(index)), window, cx);
            list.focus(window, cx);
        });
    }
}

fn visible_rows(
    entries: &[ProjectFile],
    root: &Path,
    expanded: &HashSet<SharedString>,
) -> Vec<FileRow> {
    let mut rows = Vec::new();
    push_rows(entries, root, expanded, 0, &mut rows);
    rows
}

fn push_rows(
    entries: &[ProjectFile],
    root: &Path,
    expanded: &HashSet<SharedString>,
    depth: usize,
    rows: &mut Vec<FileRow>,
) {
    for entry in entries {
        let id: SharedString = entry.relative.clone().into();
        let open = entry.directory && expanded.contains(&id);
        rows.push(FileRow {
            id: id.clone(),
            label: entry.name.clone().into(),
            path: absolute_path(root, entry),
            depth,
            directory: entry.directory,
            has_children: !entry.children.is_empty(),
            expanded: open,
        });
        if open {
            push_rows(&entry.children, root, expanded, depth + 1, rows);
        }
    }
}

impl Render for FileBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .h_flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.55))
            .px_3()
            .pt_3()
            .pb_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("Explorer"),
            )
            .when(self.loading, |header| header.child(Spinner::new().xsmall()))
            .child(
                crate::ui::icon_button(
                    "file-browser-refresh",
                    IconName::RefreshCw,
                    "Refresh files",
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );

        let body = if let Some(error) = self.error.clone() {
            div()
                .id("file-browser-error")
                .test_support()
                .flex_1()
                .v_flex()
                .items_center()
                .justify_center()
                .gap_2()
                .p_4()
                .text_center()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size_5()
                        .text_color(cx.theme().danger),
                )
                .child(div().text_sm().child("Could not read project files"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(error),
                )
                .into_any_element()
        } else if self.report.is_empty() && !self.loading {
            div()
                .id("file-browser-empty")
                .test_support()
                .flex_1()
                .v_flex()
                .items_center()
                .justify_center()
                .gap_2()
                .p_4()
                .text_center()
                .child(
                    Icon::new(IconName::Folder)
                        .size_5()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(div().text_sm().child("No project files"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("This project contains no readable files."),
                )
                .into_any_element()
        } else {
            div()
                .id("file-browser-tree")
                .test_support()
                .flex_1()
                .min_h_0()
                .child(List::new(&self.list))
                .into_any_element()
        };

        div()
            .id("file-browser")
            .test_support()
            .v_flex()
            .size_full()
            .min_h_0()
            .child(header)
            .child(body)
            .when(self.truncated, |browser| {
                browser.child(
                    div()
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(cx.theme().border.opacity(0.55))
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Some project files are hidden by the scan limit."),
                )
            })
    }
}
