//! Owns project generations, index publication, retained sessions and load admission.
use crate::{
    commands::{CollectionItem, CommandAvailability, CommandId, ENTRY_CONTEXT_IDE_COMMANDS},
    desktop_actions,
    editor::{
        assets as asset_insertion,
        component_builder::{
            BuilderContext, ComponentSelection, ImagePickContext,
            snippets::{InsertionContext, prepare},
        },
        content_finder::{self, FinderEntry},
        copyedit::CopyeditConfig,
        formatting::EditorWiring,
        save_coordinator::{
            SaveCompletion, SaveCoordinator, SaveKind, SaveRequest, SaveResult, SaveStatus,
        },
        save_failure::SaveFailureArtifact,
        session::{DefinitionTarget, DocumentSession, HistoryChanged, SaveOnBlur, SessionChanged},
        writing_modes::WritingModes,
    },
    file_browser::{FileBrowser, FileBrowserEvent},
    frontmatter::{FrontmatterEdit, ReferenceCatalog},
    layout::panels::WorkspacePanel,
    runtime, settings, ui,
};
use astrolith_astro::{
    Cancellation, DiscoveryOutcome, DiscoveryRequest, ProjectManifest,
    components::{
        ComponentIndex, ComponentIndexRequest, CorroboratedImport, inspect_components, scan_imports,
    },
    contained_path, inspect_project, read_bounded,
    schema::generation::{SchemaGenerationRequest, generate_collection_schemas},
    schema::{CollectionSchema, FieldKind, SchemaFreshness, SchemaRefresh, schema_fingerprint},
};
use astrolith_core::{
    CollectionId, ProjectId, SourcePath,
    preferences::{
        FieldMappings, FileExtension, GlobalPreferencesV2, ImagePathMode, ProjectPreferencesV1,
        resolve,
    },
};
use astrolith_document::{
    DocumentIdentity, DocumentKind, DocumentLoadTicket, DocumentRevision, DocumentSnapshot,
    FormatCommand, MAX_DOCUMENT_BYTES, RecoveryRecord, RecoveryStore, SaveOutcome, TokenSummary,
    overwrite_document, parse_document, save_document_copy, save_snapshot,
};
use astrolith_git::{GitRepository, GitStatus};
use astrolith_preview::RouteMapping;
use astrolith_workspace::{
    CollectionEntry, CollectionWatcher, MetaValue, ReferenceOption, WatchBatch,
    assets::AssetLocation,
    list::{self, EntryFilter, EntrySort, SortMode},
    operations::{
        self, CopyPathRequest, CreateRequest, CreationField, CreationInput, CreationInputKind,
        DuplicateRequest, MoveRequest, NewEntryName, PathForm, ProjectScope, RevealRequest,
        ScalarValue, SystemTrash, TrashBackend, TrashRequest,
    },
    paths, references, scan_collections,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IndexPath, Selectable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, EventEmitter, Focusable, IntoElement,
    PathPromptOptions, Render, SharedString, Subscription, Task, TestSupportExt, WeakEntity,
    Window, div, prelude::*, px, rems,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

mod entry_operations;
mod load;
mod lsp;
mod recovery;
mod save;
mod watch;

use entry_operations::PendingCreate;
use lsp::LspRegistry;
use watch::ServiceWatcher;

use save::PendingAfterSave;

#[cfg(test)]
use save::SaveAttempt;
#[cfg(test)]
use watch::WatcherScan;

// Tokio owns the native watcher's final drop; UI cancellation only aborts its task.
/// One clickable ancestor in the library breadcrumb.
#[derive(Clone)]
enum CrumbAction {
    Collections,
    Directory(Vec<String>),
}

/// Translate a project-root refusal for display.
pub(crate) fn project_root_error_message(error: &paths::ProjectRootError) -> String {
    match error {
        paths::ProjectRootError::Unreadable { path, message } => tr!(
            "browser.project_error.unreadable",
            path = path.display(),
            error = message
        ),
        paths::ProjectRootError::NotADirectory(path) => tr!(
            "browser.project_error.not_a_directory",
            path = path.display()
        ),
        paths::ProjectRootError::Blocked(blocked) => blocked_root_message(*blocked),
    }
}

/// Translate the reason a protected directory cannot be a project.
pub(crate) fn blocked_root_message(root: paths::BlockedRoot) -> String {
    match root {
        paths::BlockedRoot::FilesystemRoot => tr!("browser.blocked_root.filesystem_root"),
        paths::BlockedRoot::HomeDirectory => tr!("browser.blocked_root.home_directory"),
        paths::BlockedRoot::SystemDirectory => tr!("browser.blocked_root.system_directory"),
        paths::BlockedRoot::CredentialDirectory => tr!("browser.blocked_root.credential_directory"),
    }
}

fn parent_directory(directory: &[String]) -> Option<Vec<String>> {
    (!directory.is_empty()).then(|| directory[..directory.len() - 1].to_vec())
}

/// One subdirectory row in a collection's browsable tree.
struct DirectoryRow {
    /// Path below the collection base, empty for the collection root.
    segments: Vec<String>,
    /// Recursive entry count below this directory.
    count: usize,
}

/// One rendered project-panel row; labels are built only when the index,
/// filter or ordering changes, never during a frame.
struct EntryRow {
    index: usize,
    id: SharedString,
    title: SharedString,
    filename: SharedString,
    show_filename: bool,
    date: SharedString,
    mdx: bool,
    draft: bool,
    search: SharedString,
    path: PathBuf,
    /// Present when this row is a folder rather than a document.
    directory: Option<DirectoryRow>,
}

/// The directory segments of an entry's source below its collection base.
fn entry_directory(entry: &CollectionEntry) -> Vec<String> {
    entry
        .relative_path()
        .parent()
        .map(path_segments)
        .unwrap_or_default()
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn directory_row(collection: &str, segments: Vec<String>, count: usize) -> EntryRow {
    let joined = segments.join("/");
    EntryRow {
        index: 0,
        id: format!("dir:{collection}:{joined}").into(),
        title: segments.last().cloned().unwrap_or_default().into(),
        filename: SharedString::default(),
        show_filename: false,
        date: SharedString::default(),
        mdx: false,
        draft: false,
        search: joined.to_lowercase().into(),
        path: PathBuf::new(),
        directory: Some(DirectoryRow { segments, count }),
    }
}

struct EntrySection {
    title: SharedString,
    /// Uppercase heading, built once per index refresh.
    heading: SharedString,
    rows: Vec<EntryRow>,
}

struct EntryList {
    owner: WeakEntity<ProjectWorkspace>,
    entries: Arc<Vec<CollectionEntry>>,
    sections: Vec<EntrySection>,
    query: String,
    filter: EntryFilter,
    sort: EntrySort,
    /// When set, only entries from this collection are listed.
    collection_filter: Option<SharedString>,
    /// Directory below the collection base being browsed; empty is its root.
    directory: Vec<String>,
    /// Entries under the browsed directory (recursive) or in the collection.
    scope_total: usize,
    /// Non-directory rows in `sections`, maintained by `rebuild` so the
    /// per-frame header count never walks every entry.
    entry_count: usize,
    /// Resolved field mappings per collection identity.
    mappings: BTreeMap<String, FieldMappings>,
    /// Index publication this list was built from.
    index_revision: u64,
    /// Inputs that produced `sections`; an identical repeat reuses them
    /// instead of re-filtering and re-sorting the whole collection.
    rebuild_key: Option<EntryListKey>,
    /// Number of full rebuilds, for the cache test.
    #[cfg(test)]
    rebuilds: usize,
    selected: Option<IndexPath>,
    active: Option<PathBuf>,
    root: Option<PathBuf>,
}

/// Every input that decides one list rebuild.
#[derive(Clone, PartialEq, Eq)]
struct EntryListKey {
    query: String,
    drafts_only: bool,
    sort: EntrySort,
    collection: Option<SharedString>,
    directory: Vec<String>,
    index_revision: u64,
}

impl EntryList {
    fn new(owner: WeakEntity<ProjectWorkspace>, entries: Arc<Vec<CollectionEntry>>) -> Self {
        let mut list = Self {
            owner,
            entries,
            sections: Vec::new(),
            query: String::new(),
            filter: EntryFilter::default(),
            sort: EntrySort::default(),
            collection_filter: None,
            directory: Vec::new(),
            scope_total: 0,
            entry_count: 0,
            mappings: BTreeMap::new(),
            index_revision: 0,
            rebuild_key: None,
            #[cfg(test)]
            rebuilds: 0,
            selected: None,
            active: None,
            root: None,
        };
        list.rebuild();
        list
    }

    fn rebuild(&mut self) {
        let key = EntryListKey {
            query: self.query.clone(),
            drafts_only: self.filter.drafts_only,
            sort: self.sort.clone(),
            collection: self.collection_filter.clone(),
            directory: self.directory.clone(),
            index_revision: self.index_revision,
        };
        if self.rebuild_key.as_ref() == Some(&key) {
            return;
        }
        let filter = EntryFilter {
            query: self.query.clone(),
            drafts_only: self.filter.drafts_only,
        };
        let filter_active = !filter.query.trim().is_empty() || filter.drafts_only;
        let mut sections = Vec::new();

        if let Some(collection) = self.collection_filter.clone() {
            let collection = collection.as_ref();
            let mappings = self.mappings.get(collection);
            let mut rows = Vec::new();
            if filter_active {
                // Filtering is a flat result set, exactly like the collection
                // index before nested browsing existed.
                let mut matching: Vec<(usize, &CollectionEntry)> = self
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.entry().id().collection().as_str() == collection)
                    .filter(|(_, entry)| list::matches_filter(entry, mappings, &filter))
                    .collect();
                matching.sort_by(|(_, left), (_, right)| {
                    list::compare_entries(left, right, mappings, &self.sort)
                });
                self.scope_total = self
                    .entries
                    .iter()
                    .filter(|entry| entry.entry().id().collection().as_str() == collection)
                    .count();
                for (index, entry) in matching {
                    rows.push(self.entry_row(index, entry, collection, mappings));
                }
            } else {
                // Browse one directory: child folders first, then its own
                // entries. Counts are recursive over the entries below.
                let mut child_counts: BTreeMap<String, usize> = BTreeMap::new();
                let mut direct = Vec::new();
                let mut scope_total = 0;
                for (index, entry) in self.entries.iter().enumerate() {
                    if entry.entry().id().collection().as_str() != collection {
                        continue;
                    }
                    let segments = entry_directory(entry);
                    if !segments.starts_with(&self.directory) {
                        continue;
                    }
                    scope_total += 1;
                    if segments.len() == self.directory.len() {
                        direct.push((index, entry));
                    } else {
                        *child_counts
                            .entry(segments[self.directory.len()].clone())
                            .or_default() += 1;
                    }
                }
                self.scope_total = scope_total;
                direct.sort_by(|(_, left), (_, right)| {
                    list::compare_entries(left, right, mappings, &self.sort)
                });
                for (name, count) in child_counts {
                    let mut segments = self.directory.clone();
                    segments.push(name);
                    rows.push(directory_row(collection, segments, count));
                }
                for (index, entry) in direct {
                    rows.push(self.entry_row(index, entry, collection, mappings));
                }
            }
            sections.push(EntrySection {
                title: collection.to_owned().into(),
                heading: collection.to_uppercase().into(),
                rows,
            });
        } else {
            let mut groups: BTreeMap<&str, Vec<(usize, &CollectionEntry)>> = BTreeMap::new();
            for (index, entry) in self.entries.iter().enumerate() {
                let collection = entry.entry().id().collection().as_str();
                let mappings = self.mappings.get(collection);
                if !list::matches_filter(entry, mappings, &filter) {
                    continue;
                }
                groups.entry(collection).or_default().push((index, entry));
            }
            self.scope_total = self.entries.len();
            for (collection, mut rows) in groups {
                let mappings = self.mappings.get(collection);
                rows.sort_by(|(_, left), (_, right)| {
                    list::compare_entries(left, right, mappings, &self.sort)
                });
                let mut section = EntrySection {
                    title: collection.to_owned().into(),
                    heading: collection.to_uppercase().into(),
                    rows: Vec::with_capacity(rows.len()),
                };
                for (index, entry) in rows {
                    section
                        .rows
                        .push(self.entry_row(index, entry, collection, mappings));
                }
                sections.push(section);
            }
        }
        self.entry_count = sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .filter(|row| row.directory.is_none())
            .count();
        self.sections = sections;
        self.rebuild_key = Some(key);
        #[cfg(test)]
        {
            self.rebuilds += 1;
        }
    }

    fn entry_row(
        &self,
        index: usize,
        entry: &CollectionEntry,
        collection: &str,
        mappings: Option<&FieldMappings>,
    ) -> EntryRow {
        let title = list::display_title(entry, mappings);
        let filename = list::file_name(entry).to_owned();
        let show_filename = title != filename;
        let date = list::publication_date(entry, mappings)
            .map(|date| format!("{date}"))
            .unwrap_or_default();
        let search = format!("{filename}\n{title}").to_lowercase();
        let id = SharedString::from(format!("entry:{collection}:{}", entry.entry().id().id()));
        let path = self
            .root
            .as_ref()
            .map(|root| root.join(entry.entry().source().as_str()))
            .unwrap_or_else(|| PathBuf::from(entry.entry().source().as_str()));
        EntryRow {
            index,
            id,
            title: title.into(),
            filename: filename.into(),
            show_filename,
            date: date.into(),
            mdx: entry.entry().source().as_str().ends_with(".mdx"),
            draft: list::is_draft(entry, mappings),
            search: search.into(),
            path,
            directory: None,
        }
    }

    fn entry_index(&self, ix: IndexPath) -> Option<usize> {
        let row = self.sections.get(ix.section)?.rows.get(ix.row)?;
        row.directory.is_none().then_some(row.index)
    }

    fn directory_at(&self, ix: IndexPath) -> Option<Vec<String>> {
        self.sections
            .get(ix.section)?
            .rows
            .get(ix.row)?
            .directory
            .as_ref()
            .map(|directory| directory.segments.clone())
    }

    fn visible_count(&self) -> usize {
        self.sections.iter().map(|section| section.rows.len()).sum()
    }

    fn entry_count(&self) -> usize {
        self.entry_count
    }

    /// Drop the cached sections so the next rebuild sees changed entries or
    /// configuration even when the query did not change.
    fn invalidate(&mut self) {
        self.rebuild_key = None;
    }

    fn selected_row(&self) -> Option<(usize, usize)> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(section, group)| {
                group
                    .rows
                    .iter()
                    .position(|row| Some(&row.path) == self.active.as_ref())
                    .map(|row| (section, row))
            })
    }
}
impl ListDelegate for EntryList {
    type Item = ListItem;
    fn sections_count(&self, _: &App) -> usize {
        self.sections.len().max(1)
    }
    fn items_count(&self, section: usize, _: &App) -> usize {
        self.sections
            .get(section)
            .map_or(0, |section| section.rows.len())
    }
    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let section = self.sections.get(section)?;
        if self.collection_filter.is_some() {
            return None;
        }
        Some(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .px_4()
                .pt_4()
                .pb_1()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(section.heading.clone())
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .opacity(0.7)
                        .child(SharedString::from(section.rows.len().to_string())),
                ),
        )
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let row = self.sections.get(ix.section)?.rows.get(ix.row)?;
        if let Some(directory) = &row.directory {
            let name = row.title.clone();
            let count = directory.count;
            let label = row.search.clone();
            let id = row.id.clone();
            let selected = self.selected == Some(ix);
            let muted = cx.theme().muted_foreground;
            return Some(
                ListItem::new(id)
                    .min_h(rems(3.25))
                    .selected(selected)
                    .rounded(cx.theme().radius)
                    .when(selected, |item| item.bg(cx.theme().selection))
                    .accessibility_label(label)
                    .child(
                        div()
                            .h_flex()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::Folder).size_4().text_color(muted))
                            .child(div().min_w_0().flex_1().truncate().text_sm().child(name))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(SharedString::from(count.to_string())),
                            )
                            .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted)),
                    ),
            );
        }
        let active = Some(&row.path) == self.active.as_ref();
        let selected = active || self.selected == Some(ix);
        let title = row.title.clone();
        let filename = row.filename.clone();
        let show_filename = row.show_filename;
        let date = row.date.clone();
        let mdx = row.mdx;
        let draft = row.draft;
        let owner = self.owner.clone();
        let path = row.path.clone();
        let busy = self
            .owner
            .upgrade()
            .is_none_or(|project| project.read(cx).is_busy());
        let availability = self
            .owner
            .upgrade()
            .map(|project| project.read(cx).command_availability(cx, window))
            .unwrap_or_default();
        let badge = |label: SharedString, color: gpui_kit::Hsla| {
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(color)
                .opacity(0.9)
                .child(label)
        };
        let row_content = div()
            .h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(
                div()
                    .v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_0p5()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(crate::theme::prose_font(cx))
                            .text_sm()
                            .when(active, |title| {
                                title
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground)
                            })
                            .child(title),
                    )
                    .when(show_filename, |row| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(filename),
                        )
                    }),
            )
            .when(mdx, |row| {
                row.child(badge("MDX".into(), cx.theme().muted_foreground))
            })
            .when(draft, |row| {
                row.child(badge(
                    tr!("browser.library.draft_badge").into(),
                    cx.theme().warning,
                ))
            })
            .when(!date.is_empty(), |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(date),
                )
            })
            .context_menu(move |menu, _, _| {
                let menu = menu
                    .item(entry_menu_item(&owner, CommandId::NewEntry, &path, busy))
                    .separator()
                    .item(entry_menu_item(
                        &owner,
                        CommandId::DuplicateEntry,
                        &path,
                        busy,
                    ))
                    .item(entry_menu_item(&owner, CommandId::RenameEntry, &path, busy))
                    .item(entry_menu_item(&owner, CommandId::DeleteEntry, &path, busy))
                    .separator()
                    .item(entry_menu_item(&owner, CommandId::RevealEntry, &path, busy))
                    .item(entry_menu_item(
                        &owner,
                        CommandId::CopyPathAbsolute,
                        &path,
                        busy,
                    ))
                    .item(entry_menu_item(
                        &owner,
                        CommandId::CopyPathRelative,
                        &path,
                        busy,
                    ))
                    .separator();
                ENTRY_CONTEXT_IDE_COMMANDS
                    .iter()
                    .fold(menu, |menu, command| {
                        let path = (*command == CommandId::OpenEntryInIde).then(|| path.clone());
                        menu.item(ide_menu_item(
                            &owner,
                            *command,
                            path,
                            !command.is_available(availability),
                        ))
                    })
            });
        Some(
            ListItem::new(row.id.clone())
                .min_h(rems(3.25))
                .selected(selected)
                .rounded(cx.theme().radius)
                .accessibility_label(row.search.clone())
                .child(
                    div()
                        .w_full()
                        .h(rems(2.75))
                        .flex_shrink_0()
                        .rounded(cx.theme().radius)
                        .when(selected, |surface| surface.bg(cx.theme().selection))
                        .child(row_content),
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
    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        if self.query != query {
            self.query.clear();
            self.query.push_str(query);
            self.rebuild();
        }
        Task::ready(())
    }
}

/// One context-menu item that dispatches the same [`CommandId`] the palette and
/// toolbar use, for one specific row.
fn entry_menu_item(
    owner: &WeakEntity<ProjectWorkspace>,
    command: CommandId,
    path: &Path,
    disabled: bool,
) -> PopupMenuItem {
    let owner = owner.clone();
    let path = path.to_path_buf();
    PopupMenuItem::new(command.label())
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            let _ = owner.update(cx, |project, cx| {
                project.entry_command(command, Some(path.clone()), window, cx)
            });
        })
}

/// One IDE context-menu item dispatching the same [`CommandId`] as the palette.
fn ide_menu_item(
    owner: &WeakEntity<ProjectWorkspace>,
    command: CommandId,
    path: Option<PathBuf>,
    disabled: bool,
) -> PopupMenuItem {
    let owner = owner.clone();
    PopupMenuItem::new(command.label())
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            let _ = owner.update(cx, |project, cx| {
                project.open_in_ide(command, path.clone(), window, cx)
            });
        })
}

/// A retained document as displayed by the editor tab strip.
#[derive(Clone, Debug)]
pub(crate) struct OpenTab {
    pub path: PathBuf,
    pub label: SharedString,
    pub collection: Option<SharedString>,
    pub dirty: bool,
    pub externally_changed: bool,
}

pub(crate) struct ProjectWorkspace {
    document: Entity<WorkspacePanel>,
    manifest: Option<Arc<ProjectManifest>>,
    entries: Arc<Vec<CollectionEntry>>,
    /// Entry count per collection, rebuilt with the index rather than in render.
    collection_counts: BTreeMap<String, usize>,
    /// `(collection, entry count)` summaries for the library header, rebuilt
    /// with the index rather than per frame.
    collection_summaries: Arc<Vec<(String, usize)>>,
    /// Collection switch commands, rebuilt with the index and every active
    /// document change rather than per frame.
    collection_items: Vec<CollectionItem>,
    list: Entity<ListState<EntryList>>,
    /// Collection whose entries the library shows; `None` is the collection root.
    selected_collection: Option<String>,
    /// Directory below the selected collection's base; empty is its root.
    selected_directory: Vec<String>,
    tabs: Arc<Vec<OpenTab>>,
    tab_order: Vec<PathBuf>,
    display_name: Option<SharedString>,
    generation: u64,
    index_revision: u64,
    project_id: Option<ProjectId>,
    cancellation: Cancellation,
    loading: bool,
    refreshing: bool,
    index_error: Option<String>,
    /// Why the last project open failed; keeps the failure distinct from the
    /// never-opened empty state.
    open_error: Option<String>,
    status: String,
    task: Option<Task<()>>,
    watch_task: Option<Task<()>>,
    schema_task: Option<Task<()>>,
    /// Set while one schema reconciliation task is running.
    schema_in_flight: bool,
    /// Set when a schema batch arrives during a reconciliation, so one more pass
    /// runs after it finishes instead of aborting the pass in flight.
    schema_dirty: bool,
    /// Number of schema reconciliation passes started for the open project.
    ///
    /// Closes over no timer: the initial open and schema watcher batches are the
    /// only sources, so the count stays constant once generation settles.
    #[cfg(test)]
    schema_reconciliations: u32,
    watcher_owner: Option<ServiceWatcher>,
    loads: HashMap<PathBuf, Task<()>>,
    sessions: BTreeMap<PathBuf, Entity<DocumentSession>>,
    /// Collection id for each retained session, used to resolve its schema.
    session_collections: BTreeMap<PathBuf, String>,
    /// Normalized collection schemas keyed by collection id (injected by the
    /// shell from the R1 generation service).
    schemas: BTreeMap<String, Arc<CollectionSchema>>,
    active: Option<PathBuf>,
    _subscriptions: Vec<Subscription>,
    session_subscriptions: HashMap<PathBuf, Vec<Subscription>>,
    recovery: Option<RecoveryStore>,
    pending_recovery: Vec<RecoveryRecord>,
    recovery_task: Option<Task<()>>,
    recovery_load_task: Option<Task<()>>,
    /// In-flight guarded saves, at most one per canonical source.
    save_tasks: HashMap<PathBuf, Task<()>>,
    /// Autosave deadlines and per-document save state.
    save_coordinator: SaveCoordinator,
    /// Timer armed for the earliest autosave deadline.
    autosave_task: Option<Task<()>>,
    /// Window used to run deadline work outside a window event.
    window_handle: Option<AnyWindowHandle>,
    restore_task: Option<Task<()>>,
    /// In-flight index request; only index work owns this slot, so an unrelated
    /// command can never cancel a refresh that still has to clear `refreshing`.
    index_task: Option<Task<()>>,
    /// In-flight entry command tasks. They are independent of the index slot, so
    /// starting one command never cancels another command's completion.
    command_tasks: Vec<Task<()>>,
    /// Token of the newest index request that has been issued.
    index_request: u64,
    /// Token of the newest index result that has been published.
    published_index_request: u64,
    /// Project-relative collection base overrides resolved from preferences and
    /// applied to discovery, scanning and the watcher.
    collection_bases: BTreeMap<String, PathBuf>,
    /// Pristine Astro-configuration bases captured before any override, used to
    /// compute a project-level content root exactly once.
    config_bases: BTreeMap<String, PathBuf>,
    /// Diagnostics from the last content-directory resolution.
    override_problems: Vec<String>,
    /// Resolved global preferences snapshot used by the file list and creation.
    global_preferences: GlobalPreferencesV2,
    /// Open project preferences snapshot; `None` inherits the global scope.
    project_preferences: Option<ProjectPreferencesV1>,
    /// Active file-list filter (the query lives in the list delegate).
    list_filter: EntryFilter,
    /// Active file-list ordering.
    list_sort: EntrySort,
    /// Sort modes offered by the menu, derived from the installed schemas.
    sort_options: Vec<SortMode>,
    /// Operations to run once the guarded save of a dirty session succeeds,
    /// keyed by the canonical source path whose save releases them.
    pending_after_save: BTreeMap<PathBuf, PendingAfterSave>,
    /// Destination for redacted save-failure artifacts; `None` disables them.
    save_failure_dir: Option<PathBuf>,
    /// Project switch waiting for every dirty buffer to save.
    pending_switch: Option<PathBuf>,
    /// Quit waiting for every dirty buffer to save through the coordinator.
    pending_quit: bool,
    /// Desktop trash backend; tests inject a recording backend.
    trash_backend: Arc<dyn TrashBackend>,
    /// Context captured when a create dialog is opened.
    pending_create: Option<PendingCreate>,
    /// Last query observed from the list search box, used to refresh the header.
    last_list_query: String,
    /// Schema freshness state machine; rejects superseded generation results.
    schema_freshness: SchemaFreshness,
    /// Advances whenever a different schema set is installed, rebuilding forms.
    schema_revision: u64,
    /// Reference options for schema reference controls, rebuilt with the index.
    references: Arc<ReferenceCatalog>,
    /// Advances whenever the reference catalog changes, rebuilding forms that
    /// are already open so new options reach their dropdowns.
    references_revision: u64,
    /// Options loaded from JSON `file()` data collections, keyed by collection.
    data_references: BTreeMap<String, Vec<ReferenceOption>>,
    /// In-flight data-collection reference load.
    data_references_task: Option<Task<()>>,
    /// Writing modes applied to every retained session.
    writing: WritingModes,
    /// Language servers shared by every Build document of the open project.
    lsp: LspRegistry,
    git_status: Option<Result<GitStatus, String>>,
    git_task: Option<Task<()>>,
    git_request: u64,
    git_monitoring: bool,
    /// Test seam replacing process discovery and spawning.
    #[cfg(test)]
    lsp_start_hook: Option<Arc<lsp::LspStartHook>>,
    /// Build shows complete source in the active session's editor.
    source_view: bool,
    /// Shell view state mirrored for one authoritative availability value.
    view_sidebar: bool,
    view_inspector: bool,
    view_preview: bool,
    view_fullscreen: bool,
    /// Copyedit configuration applied to every retained session.
    copyedit: CopyeditConfig,
    /// Static component index for the open project generation.
    component_index: Option<Arc<ComponentIndex>>,
    component_error: Option<String>,
    component_loading: bool,
    /// Imports seen in indexed entries, for corroborating component specifiers.
    component_imports: Arc<Vec<CorroboratedImport>>,
    component_task: Option<Task<()>>,
    /// Build's project file explorer; Writing and Preview keep the library list.
    file_browser: Entity<FileBrowser>,
    /// A language-server definition waiting for its file to load.
    pending_reveal: Option<(PathBuf, Option<lsp_types::Position>)>,
}

/// Identity that decides when the frontmatter panel must be rebuilt.
#[derive(Clone, Debug, PartialEq)]
pub struct FrontmatterKey {
    path: PathBuf,
    has_schema: bool,
    schema_revision: u64,
    references_revision: u64,
    mappings: FieldMappings,
}

/// One active document's complete frontmatter-form input.
#[derive(Clone)]
pub struct ActiveFrontmatter {
    /// Generated collection schema, or `None` for a schema-less collection.
    pub schema: Option<Arc<CollectionSchema>>,
    /// Resolved field mappings for the active collection.
    pub mappings: FieldMappings,
    /// Reference options for every schema reference target.
    pub references: Arc<ReferenceCatalog>,
    /// Canonical project root, used to validate in-project image paths.
    pub root: PathBuf,
    /// Project-relative source of the active document.
    pub source: String,
    /// Resolved assets directory, when the project allows one.
    pub assets: Option<AssetLocation>,
    /// Relative or project-root generated image paths.
    pub path_mode: ImagePathMode,
    /// Live editor text, including unsaved changes.
    pub text: String,
}
/// Project file extensions Build edits as retained text documents.
const EDITABLE_SOURCE_EXTENSIONS: &[&str] = &[
    "astro", "ts", "mts", "cts", "tsx", "jsx", "js", "mjs", "cjs", "json", "jsonc", "css", "scss",
    "less", "html", "htm", "svelte", "vue", "md", "mdx", "yaml", "yml", "toml",
];

fn is_editable_extension(extension: &str) -> bool {
    let extension = extension.to_ascii_lowercase();
    EDITABLE_SOURCE_EXTENSIONS
        .iter()
        .any(|candidate| *candidate == extension)
}

/// Upper bound on indexed entries scanned for corroborated imports.
const MAX_COMPONENT_IMPORT_FILES: usize = 256;
/// Upper bound on one scanned entry source.
const MAX_COMPONENT_IMPORT_BYTES: u64 = 256 * 1024;

/// Read the value imports of indexed entries, best effort and bounded.
///
/// A scan never fails an insertion: unreadable, oversized or unparseable entry
/// documents simply contribute nothing.
fn scan_component_imports(sources: &[PathBuf]) -> Vec<CorroboratedImport> {
    let mut imports = Vec::new();
    for source in sources.iter().take(MAX_COMPONENT_IMPORT_FILES) {
        let Ok(metadata) = std::fs::metadata(source) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_COMPONENT_IMPORT_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(source) else {
            continue;
        };
        imports.extend(scan_imports(&text, source));
    }
    imports
}

impl Drop for ProjectWorkspace {
    fn drop(&mut self) {
        tracing::debug!(
            generation = self.generation,
            "dropping the project workspace"
        );
        self.cancellation.cancel();
    }
}
impl EventEmitter<SessionChanged> for ProjectWorkspace {}

/// The active session's complete-source history moved; the inspector must reload.
pub(crate) struct HistoryChangedEvent;
impl EventEmitter<HistoryChangedEvent> for ProjectWorkspace {}

/// Emitted once a project's manifest and entry index are published.
///
/// Carries the canonical root so preference consumers never derive a project
/// identity from the per-open [`ProjectId`].
pub(crate) struct ProjectOpened {
    /// Canonical project root.
    pub root: PathBuf,
}
impl EventEmitter<ProjectOpened> for ProjectWorkspace {}

/// A project file without an editable session was chosen in the Build explorer.
///
/// The shell owns readonly snapshots, so it reads and presents this file instead
/// of the workspace creating a writable session for it.
pub(crate) struct ReadonlyFileRequested {
    /// Canonical path inside the open project.
    pub path: PathBuf,
}
impl EventEmitter<ReadonlyFileRequested> for ProjectWorkspace {}
impl ProjectWorkspace {
    pub fn new(
        document: Entity<WorkspacePanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entries = Arc::new(Vec::new());
        let owner = cx.weak_entity();
        let list = cx.new(|cx| {
            ListState::new(EntryList::new(owner, entries.clone()), window, cx).searchable(true)
        });
        let subscription =
            cx.subscribe_in(&list, window, |this, _, event: &ListEvent, window, cx| {
                if let ListEvent::Confirm(ix) = event {
                    let directory = this.list.read(cx).delegate().directory_at(*ix);
                    if let Some(directory) = directory {
                        this.select_directory(directory, window, cx);
                        return;
                    }
                    let index = this.list.read(cx).delegate().entry_index(*ix);
                    if let Some(index) = index {
                        this.open_entry(index, window, cx);
                    }
                }
            });
        // The header reports the active search text; the list's own search box
        // owns the input, so mirror it only when it actually changes.
        let query_observer = cx.observe(&list, |this, list, cx| {
            let query = list.read(cx).delegate().query.clone();
            if query != this.last_list_query {
                this.last_list_query = query;
                cx.notify();
            }
        });
        let file_browser = cx.new(|cx| FileBrowser::new(window, cx));
        let file_browser_subscription = cx.subscribe_in(
            &file_browser,
            window,
            |this, _, event: &FileBrowserEvent, window, cx| match event {
                FileBrowserEvent::Open(path) => this.open_browser_path(path.clone(), window, cx),
            },
        );
        Self {
            document,
            manifest: None,
            entries,
            collection_counts: BTreeMap::new(),
            collection_summaries: Arc::new(Vec::new()),
            collection_items: Vec::new(),
            list,
            selected_collection: None,
            selected_directory: Vec::new(),
            tabs: Arc::new(Vec::new()),
            tab_order: Vec::new(),
            display_name: None,
            generation: 0,
            index_revision: 0,
            project_id: None,
            cancellation: Cancellation::default(),
            loading: false,
            refreshing: false,
            index_error: None,
            open_error: None,
            status: String::new(),
            task: None,
            watch_task: None,
            schema_task: None,
            schema_in_flight: false,
            schema_dirty: false,
            #[cfg(test)]
            schema_reconciliations: 0,
            watcher_owner: None,
            loads: HashMap::new(),
            sessions: BTreeMap::new(),
            session_collections: BTreeMap::new(),
            schemas: BTreeMap::new(),
            active: None,
            _subscriptions: vec![subscription, query_observer, file_browser_subscription],
            session_subscriptions: HashMap::new(),
            recovery: settings::recovery_dir().ok().map(RecoveryStore::new),
            pending_recovery: Vec::new(),
            recovery_task: None,
            recovery_load_task: None,
            save_tasks: HashMap::new(),
            save_coordinator: SaveCoordinator::new(GlobalPreferencesV2::default().autosave_delay),
            autosave_task: None,
            window_handle: Some(window.window_handle()),
            restore_task: None,
            index_task: None,
            command_tasks: Vec::new(),
            index_request: 0,
            published_index_request: 0,
            collection_bases: BTreeMap::new(),
            config_bases: BTreeMap::new(),
            override_problems: Vec::new(),
            global_preferences: GlobalPreferencesV2::default(),
            project_preferences: None,
            list_filter: EntryFilter::default(),
            list_sort: EntrySort::default(),
            sort_options: Vec::new(),
            pending_after_save: BTreeMap::new(),
            save_failure_dir: settings::save_failure_dir().ok(),
            pending_switch: None,
            pending_quit: false,
            trash_backend: Arc::new(SystemTrash),
            pending_create: None,
            last_list_query: String::new(),
            schema_freshness: SchemaFreshness::default(),
            schema_revision: 0,
            references: Arc::new(ReferenceCatalog::new()),
            references_revision: 0,
            data_references: BTreeMap::new(),
            data_references_task: None,
            writing: WritingModes::default(),
            lsp: LspRegistry::default(),
            git_status: None,
            git_task: None,
            git_request: 0,
            git_monitoring: false,
            #[cfg(test)]
            lsp_start_hook: None,
            source_view: false,
            view_sidebar: true,
            view_inspector: true,
            view_preview: false,
            view_fullscreen: false,
            copyedit: CopyeditConfig::default(),
            component_index: None,
            component_error: None,
            component_loading: false,
            component_imports: Arc::new(Vec::new()),
            component_task: None,
            file_browser,
            pending_reveal: None,
        }
    }
    /// Save status of the active document, for the tab strip status text.
    pub(crate) fn active_save_status(&self) -> Option<SaveStatus> {
        self.active
            .as_ref()
            .and_then(|path| self.save_coordinator.status(path))
    }

    pub fn has_dirty(&self, cx: &App) -> bool {
        self.sessions
            .values()
            .any(|session| session.read(cx).is_dirty())
    }

    /// Project name shown in window chrome; `None` until a project is open.
    pub(crate) fn project_name(&self) -> Option<SharedString> {
        self.display_name.clone()
    }

    pub(crate) fn index_revision(&self) -> u64 {
        self.index_revision
    }

    pub(crate) fn project_root(&self) -> Option<PathBuf> {
        self.manifest
            .as_ref()
            .map(|manifest| manifest.root().to_path_buf())
    }

    /// Current repository state, populated asynchronously for the open project.
    pub(crate) fn git_status(&self) -> Option<Result<GitStatus, String>> {
        self.git_status.clone()
    }

    /// Refresh Git state only while the Source Control panel is visible.
    pub(crate) fn set_git_monitoring(&mut self, active: bool, cx: &mut Context<Self>) {
        self.git_monitoring = active;
        if active {
            self.refresh_git(cx);
        }
    }

    /// Refresh Git status without blocking the window thread.
    pub(crate) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project_root() else {
            self.git_status = None;
            self.git_task = None;
            self.file_browser
                .update(cx, |browser, cx| browser.set_git_status(None, cx));
            return;
        };
        let generation = self.generation;
        self.git_request = self.git_request.wrapping_add(1);
        let request = self.git_request;
        self.git_status = Some(Err(tr!("browser.status.checking_repository")));
        let work = runtime::blocking(cx, move || {
            GitRepository::discover(&root)
                .and_then(|repository| repository.status())
                .map_err(|error| error.to_string())
        });
        self.git_task = Some(cx.spawn(async move |this, cx| {
            let Ok(result) = work.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || this.git_request != request {
                    return;
                }
                this.file_browser.update(cx, |browser, cx| {
                    browser.set_git_status(result.as_ref().ok().cloned(), cx)
                });
                this.git_status = Some(result);
                cx.notify();
            });
        }));
    }

    /// Point the Build explorer at the open project; Build rescans on every
    /// published index so external file changes appear without a manual refresh.
    fn sync_file_browser_root(&mut self, cx: &mut Context<Self>) {
        let root = self.project_root();
        let refresh = self.source_view;
        self.file_browser
            .update(cx, |browser, cx| browser.sync_root(root, refresh, cx));
    }

    /// Collection identities and their generated schemas for the preferences pane.
    ///
    /// Schema keys cover collections discovered after the manifest; the union is
    /// stable and sorted so the picker never reorders between renders.
    pub(crate) fn preference_collections(&self) -> Vec<(String, Option<Arc<CollectionSchema>>)> {
        let mut names: BTreeSet<String> = self.schemas.keys().cloned().collect();
        if let Some(manifest) = self.manifest.as_ref() {
            names.extend(
                manifest
                    .collections()
                    .iter()
                    .map(|descriptor| descriptor.id().as_str().to_owned()),
            );
        }
        names
            .into_iter()
            .map(|name| {
                let schema = self.schemas.get(&name).cloned();
                (name, schema)
            })
            .collect()
    }

    /// Whether the static component index found `astro-auto-import` declared.
    pub(crate) fn auto_import_declared(&self) -> bool {
        self.component_index
            .as_ref()
            .is_some_and(|index| index.auto_import())
    }

    pub(crate) fn open_tabs(&self) -> Arc<Vec<OpenTab>> {
        self.tabs.clone()
    }

    #[cfg(test)]
    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    #[cfg(test)]
    pub(crate) fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    pub(crate) fn active_tab_index(&self) -> usize {
        self.active
            .as_ref()
            .and_then(|active| self.tabs.iter().position(|tab| &tab.path == active))
            .unwrap_or(0)
    }

    pub(crate) fn active_path(&self) -> Option<PathBuf> {
        self.active.clone()
    }

    /// Rebuild the tab strip model from retained sessions. Called only when a
    /// session is opened, closed, saved or reports a state change.
    fn sync_tabs(&mut self, cx: &App) {
        let root = self
            .manifest
            .as_ref()
            .map(|manifest| manifest.root().to_path_buf());
        let mut tabs = Vec::with_capacity(self.sessions.len());
        for path in &self.tab_order {
            let Some(session) = self.sessions.get(path) else {
                continue;
            };
            let session = session.read(cx);
            let relative = root
                .as_ref()
                .and_then(|root| path.strip_prefix(root).ok())
                .unwrap_or(path);
            let label = relative
                .file_name()
                .unwrap_or(relative.as_os_str())
                .to_string_lossy()
                .into_owned();
            tabs.push(OpenTab {
                path: path.clone(),
                label: label.into(),
                collection: self
                    .session_collections
                    .get(path)
                    .map(|collection| collection.as_str().into()),
                dirty: session.dirty,
                externally_changed: session.externally_changed,
            });
        }
        self.tabs = Arc::new(tabs);
    }

    /// Surface one session's refusal notice in the workspace status line.
    fn surface_session_notice(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(path).cloned() else {
            return;
        };
        if let Some(notice) = session.update(cx, |session, _| session.notice.take()) {
            self.status = notice.to_string();
        }
    }

    fn update_tab_state(&mut self, path: &std::path::Path, cx: &App) {
        let Some(session) = self.sessions.get(path) else {
            return;
        };
        let session = session.read(cx);
        let dirty = session.dirty;
        let externally_changed = session.externally_changed;
        if let Some(tab) = Arc::make_mut(&mut self.tabs)
            .iter_mut()
            .find(|tab| tab.path == path)
        {
            tab.dirty = dirty;
            tab.externally_changed = externally_changed;
        }
    }

    /// Subscribe one retained session to text changes and editor blur.
    fn watch_session(
        &mut self,
        path: PathBuf,
        session: Entity<DocumentSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed_path = path.clone();
        let changed = cx.subscribe_in(
            &session,
            window,
            move |this, _, _: &SessionChanged, _window, cx| {
                this.note_session_change(&changed_path, cx);
            },
        );
        let history = cx.subscribe_in(
            &session,
            window,
            move |_this, _, _: &HistoryChanged, _, cx| {
                cx.emit(HistoryChangedEvent);
                cx.notify();
            },
        );
        let blur_path = path.clone();
        let blur = cx.subscribe_in(
            &session,
            window,
            move |this, _, _: &SaveOnBlur, window, cx| {
                this.save_on_blur(&blur_path, window, cx);
            },
        );
        let navigation = cx.subscribe_in(
            &session,
            window,
            |this, _, event: &DefinitionTarget, window, cx| {
                this.open_definition_target(event.clone(), window, cx);
            },
        );
        // Register the initial state so a clean document already has a status and
        // a recovered one is tracked as dirty. Registration never arms the timer:
        // recovered content waits for an edit, a blur or an explicit save before
        // it overwrites the file.
        let (revision, dirty) = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            (session.buffer.revision().get(), session.buffer.is_dirty())
        });
        self.save_coordinator
            .observe(&path, revision, dirty, cx.background_executor().now());
        self.session_subscriptions
            .insert(path, vec![changed, blur, history, navigation]);
    }

    /// Feed one session's revision into the save coordinator.
    fn note_session_change(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(path).cloned() else {
            return;
        };
        // The session absorbed the delivered change before emitting, so the
        // complete source is current here; re-absorbing would copy the whole
        // editor text on every keystroke for no gain.
        let (revision, dirty) = {
            let session = session.read(cx);
            (session.buffer.revision().get(), session.buffer.is_dirty())
        };
        let changed =
            self.save_coordinator
                .observe(path, revision, dirty, cx.background_executor().now());
        self.update_tab_state(path, cx);
        self.surface_session_notice(path, cx);
        self.schedule_recovery(cx);
        cx.emit(SessionChanged);
        if changed {
            self.arm_autosave(cx);
        }
        cx.notify();
    }

    /// Install normalized collection schemas (from the R1 generation service).
    pub fn set_collection_schemas(
        &mut self,
        schemas: Vec<(String, CollectionSchema)>,
        cx: &mut Context<Self>,
    ) {
        let installed: BTreeMap<String, Arc<CollectionSchema>> = schemas
            .into_iter()
            .map(|(collection, schema)| (collection, Arc::new(schema)))
            .collect();
        let changed = installed.len() != self.schemas.len()
            || installed.iter().any(|(collection, schema)| {
                self.schemas
                    .get(collection)
                    .is_none_or(|existing| existing.as_ref() != schema.as_ref())
            });
        self.schemas = installed;
        if changed {
            self.schema_revision += 1;
        }
        tracing::debug!(
            collections = self.schemas.len(),
            revision = self.schema_revision,
            changed,
            "collection schemas installed"
        );
        self.rebuild_sort_options();
        self.rebuild_references();
        self.publish_list_state(cx);
        self.load_components(cx);
        cx.notify();
    }

    /// Rebuild the reference catalog from the current index and data collections.
    fn rebuild_references(&mut self) {
        let mut groups: BTreeMap<String, Vec<&CollectionEntry>> = BTreeMap::new();
        for entry in self.entries.iter() {
            groups
                .entry(entry.entry().id().collection().as_str().to_owned())
                .or_default()
                .push(entry);
        }
        let mut catalog = ReferenceCatalog::new();
        for (collection, entries) in groups {
            let mappings = self.resolved_for(Some(&collection)).field_mappings;
            let owned: Vec<CollectionEntry> = entries.into_iter().cloned().collect();
            catalog.insert(
                collection,
                references::entry_options(&owned, Some(&mappings)),
            );
        }
        for (collection, options) in &self.data_references {
            catalog.insert(collection.clone(), options.clone());
        }
        let catalog = Arc::new(catalog);
        if *self.references != *catalog {
            self.references_revision = self
                .references_revision
                .checked_add(1)
                .expect("references revision exhausted");
            self.references = catalog;
        }
    }

    /// Load JSON `file()` data collections for reference dropdowns.
    fn load_data_references(&mut self, cx: &mut Context<Self>) {
        let Some(manifest) = self.manifest.clone() else {
            return;
        };
        let descriptors: Vec<_> = manifest
            .data_collections()
            .iter()
            .map(|descriptor| {
                (
                    descriptor.id().as_str().to_owned(),
                    descriptor.file().to_path_buf(),
                )
            })
            .collect();
        if descriptors.is_empty() {
            // A project whose data collections were all removed must not keep
            // stale options; the previous load is cancelled by dropping it.
            self.data_references_task = None;
            if !self.data_references.is_empty() {
                self.data_references.clear();
                self.rebuild_references();
                cx.notify();
            }
            return;
        }
        let generation = self.generation;
        let project = self.project_id.clone();
        let mappings: BTreeMap<String, FieldMappings> = descriptors
            .iter()
            .map(|(collection, _)| {
                (
                    collection.clone(),
                    self.resolved_for(Some(collection)).field_mappings,
                )
            })
            .collect();
        let work = runtime::blocking(cx, move || {
            let mut options = BTreeMap::new();
            let mut diagnostics = Vec::new();
            for (collection, file) in descriptors {
                match references::read_data_options(&file, mappings.get(&collection)) {
                    Ok(entries) => {
                        options.insert(collection, entries);
                    }
                    Err(error) => diagnostics.push(error.to_string()),
                }
            }
            (options, diagnostics)
        });
        self.data_references_task = Some(cx.spawn(async move |this, cx| {
            let Ok((options, diagnostics)) = work.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || this.project_id != project {
                    return;
                }
                this.data_references = options;
                this.rebuild_references();
                for diagnostic in diagnostics {
                    tracing::warn!(%diagnostic, "data collection unavailable");
                    this.status = tr!(
                        "browser.status.data_collection_unavailable",
                        diagnostic = diagnostic
                    );
                }
                cx.notify();
            });
        }));
    }

    /// Project identity, root and default route mapping for the preview panel.
    ///
    /// The default mapping is `/<collection>/<entry id>`, matching the fixture
    /// page layout. A project with custom page routes still resolves entries;
    /// only the convention differs.
    pub fn preview_context(&self) -> Option<(ProjectId, PathBuf, RouteMapping)> {
        let manifest = self.manifest.as_ref()?;
        let project = self.project_id.clone()?;
        let mapping =
            RouteMapping::from_collections(manifest.collections().iter().map(|collection| {
                let name = collection.id().as_str().to_owned();
                (name.clone(), name)
            }));
        Some((project, manifest.root().to_owned(), mapping))
    }

    /// The collection entry rendered by the active document, if any.
    pub fn active_entry_id(&self) -> Option<astrolith_core::EntryId> {
        let relative = self.active_relative_source()?;
        self.entries
            .iter()
            .find(|entry| entry.entry().source().as_str() == relative)
            .map(|entry| entry.entry().id().clone())
    }

    /// Project-relative source of the active document, using `/` separators.
    ///
    /// This is the value persisted as a project's last entry.
    pub(crate) fn active_relative_source(&self) -> Option<String> {
        let active = self.active.as_ref()?;
        let root = self.manifest.as_ref()?.root();
        active
            .strip_prefix(root)
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
    }

    /// Identity used to decide when the frontmatter form needs a refresh.
    ///
    /// Includes the schema revision and mapped field names so a regenerated
    /// schema directory or a preferences change rebuilds the panel, while
    /// ordinary typing never does.
    pub fn active_frontmatter_key(&self) -> Option<FrontmatterKey> {
        let path = self.active.clone()?;
        // The entry can become active before its source finishes loading.
        // Don't cache a form key until there is a session to populate it.
        if !supports_frontmatter(&path) || !self.sessions.contains_key(&path) {
            return None;
        }
        let collection = self.session_collections.get(&path)?;
        Some(FrontmatterKey {
            has_schema: self.schemas.contains_key(collection),
            schema_revision: self.schema_revision,
            references_revision: self.references_revision,
            mappings: self.resolved_for(Some(collection)).field_mappings,
            path,
        })
    }

    /// Return the active document's schema, mappings, reference options and live
    /// text, or `None` when the active file is not a collection entry.
    pub fn active_frontmatter(&self, cx: &App) -> Option<ActiveFrontmatter> {
        let path = self.active.as_ref()?;
        if !supports_frontmatter(path) {
            return None;
        }
        let collection = self.session_collections.get(path)?;
        let session = self.sessions.get(path)?;
        // The form parses complete-source text; the editor itself only shows the
        // writer projection, which has no frontmatter to build controls from.
        let text = session.read(cx).buffer.current_text().to_owned();
        let resolved = self.resolved_for(Some(collection));
        Some(ActiveFrontmatter {
            schema: self.schemas.get(collection).cloned(),
            mappings: resolved.field_mappings,
            references: self.references.clone(),
            root: self.project_root()?,
            source: self.active_relative_source()?,
            assets: self.asset_location(Some(collection)).ok(),
            path_mode: resolved.image_path_mode,
            text,
        })
    }

    /// Resolve the effective assets directory for one collection scope.
    fn asset_location(&self, collection: Option<&str>) -> Result<AssetLocation, String> {
        let root = self
            .project_root()
            .ok_or_else(|| tr!("browser.status.open_project_before_copy_assets"))?;
        let resolved = self.resolved_for(collection);
        AssetLocation::resolve(root, resolved.assets_directory.as_deref())
            .map_err(|error| error.to_string())
    }

    /// Every indexed entry as a Content Finder row.
    ///
    /// The configured title field decides the label and the Astro entry id is
    /// always searchable; a missing or non-scalar title falls back to the id.
    /// Index order is preserved so command rows map back to the same entry.
    pub(crate) fn content_finder_entries(&self) -> Vec<FinderEntry> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let collection = entry.entry().id().collection().as_str().to_owned();
                let resolved = self.resolved_for(Some(&collection));
                let title_field = resolved
                    .field_mappings
                    .title
                    .as_deref()
                    .unwrap_or(list::DEFAULT_TITLE_FIELD);
                let id = entry.entry().id().id().to_owned();
                let title = entry
                    .metadata()
                    .field(title_field)
                    .and_then(meta_scalar)
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| id.clone());
                FinderEntry {
                    index,
                    title,
                    id,
                    collection,
                    source: entry.entry().source().as_str().to_owned(),
                    slug: entry.metadata().field("slug").and_then(meta_scalar),
                }
            })
            .collect()
    }

    /// Open the entry a Content Finder row still represents.
    ///
    /// The index is revalidated against the source so a refresh between the
    /// search and Enter can never open a different entry.
    pub(crate) fn open_finder_entry(
        &mut self,
        index: usize,
        source: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position = self
            .entries
            .get(index)
            .filter(|entry| entry.entry().source().as_str() == source)
            .map(|_| index)
            .or_else(|| {
                self.entries
                    .iter()
                    .position(|entry| entry.entry().source().as_str() == source)
            });
        match position {
            Some(position) => self.open_entry(position, window, cx),
            None => {
                self.status = tr!("browser.status.entry_not_indexed");
                cx.notify();
            }
        }
    }

    /// Insert a Content Finder link into the active document.
    pub(crate) fn insert_finder_link(
        &mut self,
        entry: &FinderEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(document) = self.active_relative_source() else {
            self.status = tr!("browser.status.open_document_for_link");
            cx.notify();
            return false;
        };
        let Some(path) = self.active.clone() else {
            return false;
        };
        let Some(session) = self.sessions.get(&path).cloned() else {
            return false;
        };
        let pattern = self
            .resolved_for(Some(&entry.collection))
            .content_finder_url_pattern;
        let markdown = content_finder::markdown_link(entry, &document, pattern.as_deref());
        let inserted = session.update(cx, |session, cx| {
            session.insert_snippet(markdown, window, cx)
        });
        self.status = if inserted {
            tr!("browser.status.link_inserted", title = entry.title)
        } else {
            tr!("browser.status.link_not_inserted")
        };
        cx.notify();
        inserted
    }

    /// Index the project's MDX components and the imports indexed entries use.
    ///
    /// Discovery is bounded static analysis on the blocking lane and carries the
    /// current generation, so a superseded walk is dropped before publication.
    fn load_components(&mut self, cx: &mut Context<Self>) {
        let Some(manifest) = self.manifest.clone() else {
            self.component_index = None;
            self.component_error = None;
            self.component_loading = false;
            return;
        };
        let generation = self.generation;
        let root = manifest.root().to_path_buf();
        let cancellation = self.cancellation.clone();
        let directory = self.resolved_for(None).mdx_component_directory;
        let sources: Vec<PathBuf> = self
            .entries
            .iter()
            .map(|entry| root.join(entry.entry().source().as_str()))
            .collect();
        self.component_loading = true;
        self.component_task = Some(cx.spawn(async move |this, cx| {
            let work = runtime::blocking(cx, move || {
                let mut request = ComponentIndexRequest::new(root, generation, cancellation);
                if let Some(directory) = directory {
                    request = request.with_components_directory(directory);
                }
                let index = inspect_components(&request).map_err(|error| error.to_string())?;
                let imports = scan_component_imports(&sources);
                Ok((index, imports))
            });
            let Ok(result) = work.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.component_loading = false;
                match result {
                    Ok((index, imports)) => {
                        this.component_index = Some(Arc::new(index));
                        this.component_error = None;
                        this.component_imports = Arc::new(imports);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "component index unavailable");
                        this.component_index = None;
                        this.component_error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// The component builder's indexed context for the active document.
    pub(crate) fn component_builder_context(&self) -> Result<BuilderContext, String> {
        let path = self
            .active
            .clone()
            .ok_or_else(|| tr!("browser.status.open_mdx_for_components"))?;
        let collection = self.session_collections.get(&path).cloned();
        let resolved = self.resolved_for(collection.as_deref());
        let images = (|| {
            let root = self.project_root()?;
            let source = self.active_relative_source()?;
            Some(ImagePickContext {
                assets: self.asset_location(collection.as_deref()).ok(),
                root,
                source,
                path_mode: resolved.image_path_mode,
            })
        })();
        let index = self
            .component_index
            .clone()
            .unwrap_or_else(|| Arc::new(ComponentIndex::default()));
        Ok(BuilderContext {
            index,
            imports_enabled: resolved.insert_mdx_imports == Some(true),
            references: (*self.references).clone(),
            images,
            loading: self.component_loading,
            error: self.component_error.clone(),
        })
    }

    /// Insert one chosen component: plan imports again, then edit the writer.
    ///
    /// The plan is computed from the live complete source at this moment, so a
    /// change since the form opened is re-planned rather than applied stale.
    pub(crate) fn insert_component(
        &mut self,
        selection: ComponentSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(path) = self.active.clone() else {
            return Err(tr!("browser.status.open_mdx_for_components"));
        };
        let collection = self.session_collections.get(&path).cloned();
        let Some(session) = self.sessions.get(&path).cloned() else {
            return Err(tr!("browser.status.active_document_not_editable"));
        };
        // Menus reach this executor without the palette's availability gate, so
        // the document kind is checked again here, not only when it opens.
        if session.read(cx).kind() != DocumentKind::Mdx {
            let message = tr!("browser.status.components_only_mdx");
            self.status = message.clone();
            cx.notify();
            return Err(message);
        }
        let root = self
            .project_root()
            .ok_or_else(|| tr!("browser.status.open_project_for_components"))?;
        let source = self
            .active_relative_source()
            .ok_or_else(|| tr!("browser.status.active_document_outside_project"))?;
        let document = root.join(&source);
        session.update(cx, |session, cx| session.sync_buffer(cx));
        let text = session.read(cx).buffer.current_text().to_owned();
        let visible = session.read(cx).projection.visible.clone();
        let editor_selection = session.read(cx).editor.read(cx).selected_range();
        let (current_imports, other_imports) = self.corroborated_imports(collection.as_deref());
        let catalog = self.component_index.clone();
        let prepared = match prepare(&InsertionContext {
            component: &selection.component,
            values: selection.values,
            planned: selection.planned,
            content: selection.content,
            client_directive: selection.client_directive,
            root: &root,
            document: &document,
            source: &text,
            imports_enabled: selection.imports_enabled,
            current_imports: &current_imports,
            other_imports: &other_imports,
            catalog: catalog.as_deref().map_or(&[], ComponentIndex::components),
            visible: &visible,
            selection: editor_selection,
        }) {
            Ok(prepared) => prepared,
            Err(error) => {
                // Planning failures are visible like insertion failures.
                self.status = error.clone();
                cx.notify();
                return Err(error);
            }
        };
        let imported = prepared.changes_imports(&text);
        let warnings = prepared.plan.warnings().join(" ");
        let result = session.update(cx, |session, cx| {
            session.insert_component(prepared, window, cx)
        });
        match result {
            Ok(()) => {
                self.status = if imported {
                    tr!(
                        "browser.status.component_inserted_with_import",
                        component = selection.component.name
                    )
                } else {
                    tr!(
                        "browser.status.component_inserted",
                        component = selection.component.name
                    )
                };
                if !warnings.is_empty() {
                    self.status.push(' ');
                    self.status.push_str(&warnings);
                }
                cx.notify();
                Ok(())
            }
            Err(error) => {
                self.status = error.clone();
                cx.notify();
                Err(error)
            }
        }
    }

    /// Split cached imports into the active collection first, then the rest.
    fn corroborated_imports(
        &self,
        collection: Option<&str>,
    ) -> (Vec<CorroboratedImport>, Vec<CorroboratedImport>) {
        let mut collections: HashMap<PathBuf, String> = HashMap::new();
        if let Some(root) = self.project_root() {
            for entry in self.entries.iter() {
                collections.insert(
                    root.join(entry.entry().source().as_str()),
                    entry.entry().id().collection().as_str().to_owned(),
                );
            }
        }
        let mut current = Vec::new();
        let mut other = Vec::new();
        for import in self.component_imports.iter() {
            if collections.get(&import.document).map(String::as_str) == collection {
                current.push(import.clone());
            } else {
                other.push(import.clone());
            }
        }
        (current, other)
    }

    /// Copy dropped files into assets and insert their Markdown.
    ///
    /// Copying runs on the blocking service lane; nothing here touches the disk
    /// on the render path. Successful files still insert when another file in
    /// the same drop fails, and failures are surfaced instead of inserting the
    /// original host path.
    pub(crate) fn drop_assets(
        &mut self,
        sources: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if sources.is_empty() {
            return;
        }
        let Some(document) = self.active_relative_source() else {
            self.status = tr!("browser.status.open_document_for_drop");
            cx.notify();
            return;
        };
        let Some(active) = self.active.clone() else {
            return;
        };
        let Some(session) = self.sessions.get(&active) else {
            return;
        };
        if session.read(cx).kind() == DocumentKind::Plain {
            self.status = tr!("browser.status.drop_requires_markdown");
            cx.notify();
            return;
        }
        let collection = self.session_collections.get(&active).cloned();
        let resolved = self.resolved_for(collection.as_deref());
        let location = match self.asset_location(collection.as_deref()) {
            Ok(location) => location,
            Err(error) => {
                self.status = tr!("browser.status.assets_dir_unavailable", error = error);
                cx.notify();
                return;
            }
        };
        let mode = resolved.image_path_mode;
        let date = asset_insertion::asset_date();
        let work = runtime::blocking(cx, move || {
            asset_insertion::copy_dropped(&location, &sources, &date, &document, mode)
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_drop(active, result, window, cx)
            });
        });
        self.retain_command_task(task);
    }

    fn finish_drop(
        &mut self,
        path: PathBuf,
        result: Result<asset_insertion::DropOutcome, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (assets, failures) = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.status = tr!("browser.status.assets_not_copied", error = error);
                cx.notify();
                return;
            }
        };
        let mut insert_failed = false;
        if !assets.is_empty()
            && let Some(session) = self.sessions.get(&path).cloned()
        {
            let markdown = asset_insertion::drop_markdown(&assets);
            let inserted = session.update(cx, |session, cx| {
                session.insert_snippet(markdown, window, cx)
            });
            if !inserted {
                insert_failed = true;
            }
        }
        if let Some(failure) = asset_insertion::drop_failure_status(&failures) {
            self.status = failure;
        } else if insert_failed {
            self.status = tr!("browser.status.assets_markdown_not_inserted");
        } else if !assets.is_empty() {
            self.status = asset_insertion::drop_success_status(&assets);
        }
        cx.notify();
    }

    /// Apply one frontmatter form edit to the active session's complete source.
    ///
    /// The session absorbs any queued body edit first, then applies exactly one
    /// lossless document operation to the authoritative complete source and
    /// rebuilds the writer projection. Because the patch is computed from the
    /// live source, concurrent unsaved edits to the document body are preserved
    /// exactly; a stale form snapshot is never written back. No disk I/O happens
    /// here — the conflict-aware save coordinator persists the edit later.
    pub fn apply_frontmatter_edit(
        &mut self,
        edit: FrontmatterEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active) = self.active.clone() else {
            return;
        };
        if !supports_frontmatter(&active) {
            return;
        }
        let Some(session) = self.sessions.get(&active).cloned() else {
            return;
        };
        let patched = session.update(cx, |session, cx| {
            session.patch_complete(window, cx, |live| {
                let mut document = parse_document(live).map_err(|error| error.to_string())?;
                match &edit {
                    FrontmatterEdit::Scalar {
                        path,
                        value,
                        present,
                    } => {
                        if *present {
                            document.set_field(path, value.clone())
                        } else {
                            document.insert_field(path, value.clone())
                        }
                    }
                    FrontmatterEdit::AddItem { path, value, .. } => {
                        if document.has_field(path) {
                            let index = document
                                .field_info(path)
                                .and_then(|info| info.items.as_ref())
                                .map(Vec::len)
                                .unwrap_or(0);
                            document.insert_sequence_item(path, index, value.clone())
                        } else {
                            document.insert_sequence(path, std::slice::from_ref(value))
                        }
                    }
                    FrontmatterEdit::RemoveItem { path, index } => {
                        document.remove_sequence_item(path, *index)
                    }
                    FrontmatterEdit::Remove { path } => document.remove_field(path),
                    FrontmatterEdit::Raw { path, raw } => {
                        astrolith_document::yaml::validate_value(raw).map_err(|reason| {
                            tr!("browser.status.invalid_raw_yaml", error = reason)
                        })?;
                        document.replace_raw(path, raw)
                    }
                }
                .map_err(|error| error.to_string())?;
                document.serialize().map_err(|error| error.to_string())
            })
        });
        match patched {
            Ok(()) => cx.notify(),
            Err(error) => {
                tracing::warn!(%error, "could not apply a frontmatter edit");
                self.status = tr!("browser.status.frontmatter_edit_failed", error = error);
                cx.notify();
            }
        }
    }
    pub fn prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr!("browser.entry.open_project_prompt").into()),
        });
        self.loading = true;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let selected = paths.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match selected {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.loading = false;
                            this.open_root(path, window, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.status = tr!("browser.status.project_directory_select_failed");
                    }
                }
                cx.notify();
            });
        }));
    }
    /// Open one project root through the protected-directory policy.
    ///
    /// Validation runs on the blocking lane and finishes before any watcher or
    /// scan exists. A refused root is shown as a blocking error and never
    /// reaches the registry.
    pub(crate) fn open_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.open_error = None;
        self.selected_collection = None;
        self.selected_directory.clear();
        self.status = tr!("browser.status.checking_project_folder");
        cx.notify();
        let home = paths::home_directory();
        let requested = root.clone();
        let work = runtime::blocking(cx, move || {
            paths::validate_project_root(&requested, home.as_deref())
                .map_err(|error| project_root_error_message(&error))
        });
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work.await.and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(root) => this.start(root, window, cx),
                Err(message) => {
                    tracing::warn!(%message, "refused a project root");
                    this.loading = false;
                    this.open_error = Some(message.clone());
                    this.status = tr!("browser.status.project_open_failed", error = message);
                    cx.notify();
                }
            });
        }));
    }

    /// Re-scan collections and report completion.
    ///
    /// The index publication preserves the selection, tabs, sessions
    /// (recovery keys) and the active entry, so the preview route is retargeted
    /// only when the entry itself changed.
    pub(crate) fn reload_collections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.manifest.is_none() {
            self.status = tr!("browser.status.open_project_before_reload");
            cx.notify();
            return;
        }
        let generation = self.generation;
        let Some(index) = self.start_index_request(cx) else {
            return;
        };
        self.status = tr!("browser.status.reloading_collections");
        self.index_task = Some(cx.spawn_in(window, async move |this, cx| {
            let _ = index.await;
            let _ = this.update_in(cx, |this, _window, cx| {
                if this.generation != generation {
                    return;
                }
                this.status = match this.index_error.as_ref() {
                    Some(error) => tr!("browser.status.collections_reload_failed", error = error),
                    None => tr!(
                        "browser.status.collections_reloaded",
                        count = this.entries.len()
                    ),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Rebuild the per-collection summaries and switch commands after the
    /// index or the active document changed.
    fn rebuild_collection_summaries(&mut self) {
        let active_collection = self
            .active
            .as_ref()
            .and_then(|path| self.collection_of(path));
        let mut summaries = Vec::new();
        let mut items = Vec::new();
        if let Some(manifest) = self.manifest.as_ref() {
            for descriptor in manifest.collections() {
                let name = descriptor.id().as_str();
                summaries.push((
                    name.to_owned(),
                    self.collection_counts.get(name).copied().unwrap_or(0),
                ));
                items.push(CollectionItem {
                    active: active_collection.as_deref() == Some(name),
                    name: name.to_owned(),
                });
            }
        }
        self.collection_summaries = Arc::new(summaries);
        self.collection_items = items;
    }

    /// Collection switch commands generated from the live index.
    pub(crate) fn collection_items(&self) -> &[CollectionItem] {
        &self.collection_items
    }

    /// Select the first entry of one collection without opening it.
    pub(crate) fn open_collection(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let section = self
            .list
            .read(cx)
            .delegate()
            .sections
            .iter()
            .position(|section| section.title.as_ref() == name);
        let Some(section) = section else {
            self.status = tr!("browser.status.collection_no_entries_show", name = name);
            cx.notify();
            return;
        };
        let rows = self.list.read(cx).delegate().sections[section].rows.len();
        if rows == 0 {
            self.status = tr!("browser.status.collection_no_entries", name = name);
            cx.notify();
            return;
        }
        self.list.update(cx, |list, cx| {
            list.set_selected_index(Some(IndexPath::new(0).section(section)), window, cx);
            list.scroll_to_selected_item(window, cx);
            cx.notify();
        });
        self.status = tr!(
            "browser.status.collection_entries",
            name = name,
            count = rows
        );
        cx.notify();
    }

    /// Open one file only when it is an indexed content entry of this project.
    pub(crate) fn open_external_entry(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let root = self
            .project_root()
            .ok_or_else(|| tr!("browser.load.open_project_before_entry"))?;
        let canonical = path.canonicalize().map_err(|error| {
            tr!(
                "browser.status.read_failed",
                path = path.display(),
                error = error
            )
        })?;
        let source = relative_source(&root, &canonical)
            .ok_or_else(|| tr!("browser.status.path_outside_project", path = path.display()))?;
        let index = self
            .entries
            .iter()
            .position(|entry| entry.entry().source() == &source)
            .ok_or_else(|| tr!("browser.load.not_content_entry", path = path.display()))?;
        self.open_entry(index, window, cx);
        Ok(())
    }

    /// Open the file a language server named as a definition target.
    fn open_definition_target(
        &mut self,
        event: DefinitionTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(path) = event.path.canonicalize() else {
            tracing::info!(path = %event.path.display(), "definition target is gone");
            return;
        };
        if let Some(session) = self.sessions.get(&path).cloned() {
            self.activate_session(&path, window, cx);
            session.update(cx, |session, cx| {
                session.reveal_source_position(event.selection, window, cx)
            });
            return;
        }
        self.pending_reveal = Some((path.clone(), event.selection));
        self.open_browser_path(path, window, cx);
    }

    /// Apply a pending definition selection once its file is loaded.
    fn reveal_pending(
        &mut self,
        path: &std::path::Path,
        session: &Entity<DocumentSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target, selection)) = self.pending_reveal.clone() else {
            return;
        };
        if target != path {
            return;
        }
        self.pending_reveal = None;
        session.update(cx, |session, cx| {
            session.reveal_source_position(selection, window, cx)
        });
    }

    /// Open a Build explorer selection.
    ///
    /// Content entries and project `.astro` sources keep their retained,
    /// editable sessions; every other project file is a readonly snapshot that
    /// the shell reads and presents.
    pub(crate) fn open_browser_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.project_root() else {
            self.status = tr!("browser.status.open_project_before_browse");
            cx.notify();
            return;
        };
        let canonical = match path.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) => {
                self.status = tr!(
                    "browser.status.file_open_failed",
                    path = path.display(),
                    error = error
                );
                cx.notify();
                return;
            }
        };
        if !canonical.starts_with(&root) {
            self.status = tr!("browser.status.path_outside_project", path = path.display());
            cx.notify();
            return;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.absolute_source() == canonical)
        {
            self.open_entry(index, window, cx);
            return;
        }
        if self.is_editable_project_source(&canonical) {
            if let Err(error) = self.open_project_source(canonical, window, cx) {
                self.status = error;
                cx.notify();
            }
            return;
        }
        cx.emit(ReadonlyFileRequested { path: canonical });
    }

    /// Open the project, a collection's file list or one entry in the IDE.
    pub(crate) fn open_in_ide(
        &mut self,
        command: CommandId,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let availability = self.command_availability(cx, window);
        if !command.is_available(availability) {
            self.status = tr!("browser.status.ide_unavailable", command = command.label());
            cx.notify();
            return;
        }
        let Some(root) = self.project_root() else {
            return;
        };
        let mut target = desktop_actions::IdeTarget {
            project: root,
            file: None,
            line: None,
            column: None,
        };
        if command == CommandId::OpenEntryInIde {
            let Some(path) = path.or_else(|| self.active.clone()) else {
                return;
            };
            let (line, column) = self
                .active_session(cx)
                .map(|session| session.read(cx).cursor_line_column(cx))
                .unwrap_or((1, 1));
            target.file = Some(path);
            target.line = Some(line);
            target.column = Some(column);
        }
        let argv = match desktop_actions::ide_argv(&self.global_preferences.ide, &target) {
            Ok(argv) => argv,
            Err(message) => {
                self.status = message;
                cx.notify();
                return;
            }
        };
        let spawn = runtime::spawn(cx, async move { desktop_actions::spawn_ide(argv).await });
        self.retain_command_task(cx.spawn(async move |this, cx| {
            let result = spawn.await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(()) => tr!("browser.status.ide_opened"),
                    Err(error) => tr!("browser.status.ide_open_failed", error = error),
                };
                cx.notify();
            });
        }));
    }

    /// Switch projects, flushing every dirty buffer through the guarded path.
    ///
    /// The switch waits for the saves; a conflict or a failure cancels it and
    /// reports why, so no unsaved content is silently lost (ledger rows 7, 68).
    fn start(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_dirty(cx) && !self.save_coordinator.has_pending() {
            self.open_project(root, window, cx);
            return;
        }
        self.pending_switch = Some(root);
        self.flush_dirty_buffers(window, cx);
    }

    /// Start an immediate save for every dirty buffer.
    ///
    /// Used before a project switch and before quitting: the caller waits until
    /// nothing is dirty or pending, and cancels on a conflict or failure.
    fn flush_dirty_buffers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let paths: Vec<PathBuf> = self.sessions.keys().cloned().collect();
        for path in paths {
            if self.save_coordinator.is_saving(&path) {
                continue;
            }
            if matches!(
                self.save_coordinator.status(&path),
                Some(SaveStatus::Conflict)
            ) {
                continue;
            }
            let Some(session) = self.sessions.get(&path).cloned() else {
                continue;
            };
            let (revision, dirty) = session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                (session.buffer.revision().get(), session.buffer.is_dirty())
            });
            self.save_coordinator.observe(&path, revision, dirty, now);
            if let Some(request) =
                self.save_coordinator
                    .request_now(&path, revision, now, SaveKind::Automatic)
            {
                self.begin_save(request, window, cx);
            }
        }
        self.advance_switch(window, cx);
        self.advance_quit(window, cx);
    }

    /// Complete or cancel a switch that was waiting for its flush.
    fn advance_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.pending_switch.clone() else {
            return;
        };
        let mut blocked = None;
        for session in self.sessions.keys() {
            match self.save_coordinator.status(session) {
                Some(SaveStatus::Conflict) => {
                    blocked = Some(tr!("browser.status.switch_cancelled_conflict"));
                }
                Some(SaveStatus::Failed(message)) => {
                    blocked = Some(tr!(
                        "browser.status.switch_cancelled_save_failed",
                        error = message
                    ));
                }
                _ => {}
            }
        }
        if let Some(message) = blocked {
            self.pending_switch = None;
            tracing::warn!(%message, "cancelled a project switch");
            self.status = message;
            cx.notify();
            return;
        }
        if self.has_dirty(cx) || self.save_coordinator.has_pending() {
            return;
        }
        self.pending_switch = None;
        self.open_project(root, window, cx);
    }

    fn open_project(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(root = %root.display(), "opening Astro project");
        // Every server of the outgoing project is generation-scoped: give each
        // one bounded graceful exit before the new generation replaces them.
        self.shutdown_lsp(cx);
        self.cancellation.cancel();
        self.cancellation = Cancellation::default();
        let Some(next) = self.generation.checked_add(1) else {
            self.status = tr!("browser.status.generation_exhausted");
            return;
        };
        self.generation = next;
        let generation = self.generation;
        let project = match ProjectId::new(format!("workspace-{generation}")) {
            Ok(id) => id,
            Err(_) => return,
        };
        self.project_id = Some(project.clone());
        self.index_request = 0;
        self.published_index_request = 0;
        self.index_task = None;
        // Project-scope preferences are installed after the first open, so the
        // initial discovery always uses the Astro configuration bases.
        self.collection_bases.clear();
        self.config_bases.clear();
        self.override_problems.clear();
        // A real project scope names its canonical path; one installed for the
        // outgoing project must not resolve against the incoming manifest.
        if self
            .project_preferences
            .as_ref()
            .is_some_and(|preferences| !preferences.path.trim().is_empty())
        {
            self.project_preferences = None;
        }
        let request =
            DiscoveryRequest::new(project.clone(), generation, root, self.cancellation.clone());
        self.loading = true;
        self.refreshing = false;
        self.index_error = None;
        self.open_error = None;
        self.status = tr!("browser.status.discovering_project");
        self.watch_task = None;
        self.schema_task = None;
        self.schema_in_flight = false;
        self.schema_dirty = false;
        self.watcher_owner = None;
        self.command_tasks.clear();
        self.git_task = None;
        self.git_status = None;
        self.git_request = self.git_request.wrapping_add(1);
        self.loads.clear();
        self.save_coordinator.clear();
        self.save_tasks.clear();
        self.autosave_task = None;
        self.sessions.clear();
        self.session_subscriptions.clear();
        self.session_collections.clear();
        self.tab_order.clear();
        self.display_name = None;
        self.schemas.clear();
        self.schema_freshness = SchemaFreshness::default();
        self.schema_revision = 0;
        self.references = Arc::new(ReferenceCatalog::new());
        self.references_revision = 0;
        self.data_references.clear();
        self.data_references_task = None;
        self.component_index = None;
        self.component_error = None;
        self.component_loading = false;
        self.component_imports = Arc::new(Vec::new());
        self.component_task = None;
        self.pending_create = None;
        self.pending_recovery.clear();
        self.manifest = None;
        self.active = None;
        self.file_browser
            .update(cx, |browser, cx| browser.sync_root(None, false, cx));
        self.publish_entries(Vec::new(), cx);
        self.document
            .update(cx, |document, cx| document.clear_editor(cx));
        let cancellation = self.cancellation.clone();
        let work = runtime::blocking(cx, move || {
            let outcome = inspect_project(&request).map_err(|error| error.to_string())?;
            match outcome {
                DiscoveryOutcome::Ready(manifest) => {
                    let watcher = CollectionWatcher::for_project(&manifest)
                        .map_err(|error| error.to_string())?;
                    let entries = scan_collections(&manifest, &cancellation)
                        .map_err(|error| error.to_string())?;
                    Ok((
                        manifest,
                        entries,
                        ServiceWatcher::new(watcher, cancellation.clone()),
                    ))
                }
                DiscoveryOutcome::Unsupported { diagnostics, .. } => Err(diagnostics.join("; ")),
            }
        });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await.and_then(|value| value);
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation || this.project_id.as_ref() != Some(&project) {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((manifest, entries, watcher)) => {
                        tracing::info!(
                            project = %project.as_str(),
                            generation,
                            entries = entries.len(),
                            "Astro project opened"
                        );
                        this.config_bases = manifest
                            .collections()
                            .iter()
                            .map(|descriptor| {
                                (
                                    descriptor.id().as_str().to_owned(),
                                    descriptor.base().to_path_buf(),
                                )
                            })
                            .collect();
                        this.sessions.clear();
                        this.active = None;
                        this.document
                            .update(cx, |document, cx| document.clear_editor(cx));
                        // One publication path: the initial open installs the
                        // manifest, entries, data-collection references and the
                        // rebuilt reference catalog exactly like a refresh does.
                        this.apply_discovered_index(manifest, entries, cx);
                        if this.git_monitoring {
                            this.refresh_git(cx);
                        }
                        this.start_watcher(watcher, window, cx);
                        this.status.clear();
                        this.load_pending_recovery(window, cx);
                        this.load_collection_schemas(cx);
                        // Project preferences may already be installed; apply any
                        // collection directory override to the published index.
                        this.refresh_collection_bases(cx);
                        // The shell records this open and restores the project's
                        // layout and last entry; publish only after the index.
                        if let Some(root) = this.project_root() {
                            cx.emit(ProjectOpened { root });
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            project = %project.as_str(),
                            generation,
                            %error,
                            "could not open Astro project"
                        );
                        this.status = tr!("browser.status.project_open_failed", error = error);
                        this.open_error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
        // `window` is only needed by the retained GPUI observer, never by service I/O.
        cx.notify();
    }
    /// Reopen the entry recorded as this project's last entry.
    ///
    /// A missing or unindexed entry is ignored: the workspace keeps its
    /// ordinary empty state rather than reporting an error.
    pub(crate) fn reopen_last_entry(
        &mut self,
        source: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = source else {
            return;
        };
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.entry().source().as_str() == source)
        else {
            return;
        };
        self.open_entry(index, window, cx);
    }

    #[cfg(test)]
    pub(crate) fn open_first_entry_for_tests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_entry(0, window, cx);
    }

    #[cfg(test)]
    pub(crate) fn select_first_collection_for_tests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let collection = self
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.collections().first())
            .map(|descriptor| descriptor.id().as_str().to_owned());
        if let Some(collection) = collection {
            self.select_collection(Some(collection), window, cx);
        }
    }

    /// Whether the static component index for this generation has landed.
    #[cfg(test)]
    pub(crate) fn components_loaded(&self) -> bool {
        !self.component_loading && self.component_index.is_some()
    }

    fn open_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_entry_inner(ix, false, window, cx);
    }

    /// Open an entry produced by an operation that just published a fresh index.
    fn open_entry_after_publish(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_entry_inner(ix, true, window, cx);
    }

    pub(crate) fn activate_session(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.sessions.get(path).cloned() else {
            return;
        };
        // Switching files flushes the outgoing dirty buffer (ledger row 44);
        // the save is silent and its completion cannot touch this document.
        // A save already in flight keeps this request as its follow-up.
        if let Some(outgoing) = self.active.clone()
            && outgoing != path
            && self.save_coordinator.is_dirty(&outgoing)
        {
            self.save_path(outgoing, SaveKind::Automatic, window, cx);
        }
        // Only the visible Build document keeps a document open on the
        // project's server; the process itself outlives the switch.
        self.active = Some(path.to_owned());
        self.focus_collection_for(path, cx);
        let source_view = self.source_view;
        session.update(cx, |session, cx| {
            session.set_source_view(source_view, window, cx)
        });
        self.install(path, session, cx);
        self.sync_lsp(cx);
        self.refresh_entry_selection(cx);
        self.document.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Keep the library on the collection and directory that own `path`.
    fn focus_collection_for(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(collection) = self.collection_of(path) else {
            return;
        };
        let directory = self
            .entries
            .iter()
            .find(|entry| entry.absolute_source() == path)
            .map(entry_directory)
            .unwrap_or_default();
        if self.selected_collection.as_deref() == Some(collection.as_str())
            && self.selected_directory == directory
        {
            return;
        }
        self.selected_collection = Some(collection);
        self.selected_directory = directory;
        self.publish_list_state(cx);
    }

    fn refresh_entry_selection(&mut self, cx: &mut Context<Self>) {
        self.rebuild_collection_summaries();
        let active = self.active.clone();
        self.list.update(cx, |list, cx| {
            let delegate = list.delegate_mut();
            delegate.active = active;
            delegate.selected = delegate
                .selected_row()
                .map(|(section, row)| IndexPath::new(row).section(section));
            cx.notify();
        });
    }
    pub(crate) fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active.clone() else {
            return;
        };
        self.close_path(path, window, cx);
    }

    /// Close one retained document. Dirty content is never discarded without
    /// an explicit confirmation naming the affected document.
    pub(crate) fn close_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .sessions
            .get(&path)
            .is_some_and(|session| session.read(cx).is_dirty())
        {
            let title = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let owner = cx.weak_entity();
            window.open_dialog(cx, move |dialog, _, _| {
                let discard_owner = owner.clone();
                let discard_path = path.clone();
                let save_owner = owner.clone();
                let save_path = path.clone();
                dialog
                    .title(tr!("browser.dialog.close_title", title = title))
                    .child(tr!("browser.dialog.close_body"))
                    .footer(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("keep-editing")
                                    .label(tr!("browser.dialog.keep_editing"))
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("discard-document")
                                    .danger()
                                    .label(tr!("browser.dialog.discard_changes"))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = discard_owner.update(cx, |this, cx| {
                                            this.remove_session(&discard_path, cx)
                                        });
                                    }),
                            )
                            .child(
                                Button::new("save-and-close")
                                    .primary()
                                    .label(tr!("browser.dialog.save_and_close"))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = save_owner.update(cx, |this, cx| {
                                            this.pending_after_save.insert(
                                                save_path.clone(),
                                                PendingAfterSave::Close {
                                                    path: save_path.clone(),
                                                },
                                            );
                                            this.save_path(
                                                save_path.clone(),
                                                SaveKind::Manual,
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            ),
                    )
            });
        } else {
            self.remove_session(&path, cx);
        }
    }
    fn remove_session(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.recovery_task = None;
        if let Some(session) = self.sessions.get(path)
            && let Some(store) = self.recovery.clone()
        {
            let identity = session.read(cx).buffer.identity().clone();
            let task = runtime::blocking(cx, move || {
                let _ = store.clear(&identity);
            });
            task.detach();
        }
        tracing::debug!(path = %path.display(), "closing a retained document");
        if let Some(session) = self.sessions.get(path).cloned() {
            // The server keeps running; only this document leaves it.
            self.detach_lsp_document(session, cx);
        }
        self.save_coordinator.forget(path);
        self.save_tasks.remove(path);
        self.sessions.remove(path);
        self.session_subscriptions.remove(path);
        self.session_collections.remove(path);
        self.tab_order.retain(|retained| retained != path);
        if self.active.as_deref() == Some(path) {
            self.active = None;
            self.document
                .update(cx, |document, cx| document.clear_editor(cx));
        }
        self.sync_tabs(cx);
        self.refresh_entry_selection(cx);
        cx.emit(SessionChanged);
        cx.notify();
    }
    pub fn guard_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.has_dirty(cx) {
            self.shutdown_lsp(cx);
            return true;
        }
        if !window.has_active_dialog(cx) {
            let owner = cx.weak_entity();
            let save_owner = owner.clone();
            window.open_dialog(cx, move |dialog, _, _| {
                let owner = owner.clone();
                let save_owner = save_owner.clone();
                dialog
                    .title(tr!("browser.dialog.unsaved_title"))
                    .child(tr!("browser.dialog.unsaved_body"))
                    .footer(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("keep-window")
                                    .label(tr!("browser.dialog.keep_editing"))
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("discard-window")
                                    .danger()
                                    .label(tr!("browser.dialog.discard_and_close"))
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            if let Some(store) = this.recovery.clone() {
                                                let identities: Vec<_> = this
                                                    .sessions
                                                    .values()
                                                    .map(|session| {
                                                        session.read(cx).buffer.identity().clone()
                                                    })
                                                    .collect();
                                                let task = runtime::blocking(cx, move || {
                                                    for identity in identities {
                                                        let _ = store.clear(&identity);
                                                    }
                                                });
                                                task.detach();
                                            }
                                            this.pending_quit = false;
                                            this.shutdown_lsp(cx);
                                            this.sessions.clear();
                                        });
                                        window.close_dialog(cx);
                                        window.remove_window();
                                    }),
                            )
                            .child(
                                Button::new("save-all-and-quit")
                                    .primary()
                                    .label(tr!("browser.dialog.save_all_and_quit"))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = save_owner
                                            .update(cx, |this, cx| this.save_and_quit(window, cx));
                                    }),
                            ),
                    )
            });
        }
        false
    }

    /// Save every dirty buffer, then close the window once nothing is left.
    ///
    /// Quitting never discards content: the window stays open while a save is in
    /// flight, a newer edit is still unsaved, or a conflict/failure needs the
    /// user's attention.
    fn save_and_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_quit = true;
        self.flush_dirty_buffers(window, cx);
    }

    /// Close the window once a pending quit has no dirty or in-flight buffer.
    fn advance_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pending_quit {
            return;
        }
        let mut blocked = None;
        for session in self.sessions.keys() {
            match self.save_coordinator.status(session) {
                Some(SaveStatus::Conflict) => {
                    blocked = Some(tr!("browser.status.quit_cancelled_conflict"));
                }
                Some(SaveStatus::Failed(message)) => {
                    blocked = Some(tr!(
                        "browser.status.quit_cancelled_save_failed",
                        error = message
                    ));
                }
                _ => {}
            }
        }
        if let Some(message) = blocked {
            self.pending_quit = false;
            tracing::warn!(%message, "cancelled a quit");
            self.status = message;
            cx.notify();
            return;
        }
        if self.has_dirty(cx) || self.save_coordinator.has_pending() {
            return;
        }
        self.pending_quit = false;
        self.shutdown_lsp(cx);
        window.remove_window();
    }
}

impl ProjectWorkspace {
    /// Install the resolved preference snapshots used by the file list and
    /// entry creation. The shell calls this whenever the project scope changes.
    pub(crate) fn set_preferences(
        &mut self,
        global: GlobalPreferencesV2,
        project: Option<ProjectPreferencesV1>,
        cx: &mut Context<Self>,
    ) {
        self.global_preferences = global;
        self.project_preferences = project;
        let copyedit = CopyeditConfig::from_preferences(&self.global_preferences.copyedit);
        if copyedit != self.copyedit {
            self.apply_copyedit_config(copyedit, cx);
        }
        // Autosave delay is a global setting; existing dirty runs adopt it.
        self.save_coordinator
            .set_delay(self.global_preferences.autosave_delay);
        self.arm_autosave(cx);
        self.rebuild_sort_options();
        self.rebuild_references();
        self.publish_list_state(cx);
        self.load_components(cx);
        // The initial open happens before the project scope is known; apply the
        // resolved content-directory overrides and re-index when they differ.
        self.refresh_collection_bases(cx);
        cx.notify();
    }

    /// Resolve one collection's effective content directory against the project root.
    ///
    /// Every override comes from the core resolver, is contained by the project root
    /// and must already be a directory. An unusable override is reported instead of
    /// silently falling back to the base written in the Astro configuration.
    fn resolve_collection_bases(
        &self,
        manifest: &ProjectManifest,
    ) -> (BTreeMap<String, PathBuf>, Vec<String>) {
        let root = manifest.root();
        let mut bases = BTreeMap::new();
        let mut problems = Vec::new();
        if let Some(project) = self.project_preferences.as_ref() {
            for name in project.collections.keys() {
                if !manifest
                    .collections()
                    .iter()
                    .any(|descriptor| descriptor.id().as_str() == name)
                {
                    problems.push(tr!("browser.warning.unknown_collection", name = name));
                }
            }
        }
        // A project-level content directory is the root that contains collection
        // subdirectories; a collection-level value is that collection's exact base.
        // Relative paths come from the pristine configuration bases so applying an
        // override twice can never nest it inside itself.
        let shared = shared_collection_base(&self.config_bases);
        let project_directory = self
            .project_preferences
            .as_ref()
            .and_then(|project| project.content_directory.as_deref())
            .map(str::trim)
            .filter(|directory| !directory.is_empty());
        for descriptor in manifest.collections() {
            let collection = descriptor.id().as_str();
            let collection_directory = self
                .project_preferences
                .as_ref()
                .and_then(|project| project.collection(collection))
                .and_then(|preferences| preferences.content_directory.as_deref())
                .map(str::trim)
                .filter(|directory| !directory.is_empty());
            let candidate = match (collection_directory, project_directory) {
                (Some(directory), _) => PathBuf::from(directory),
                (None, Some(directory)) => {
                    let Some(shared) = shared.as_deref() else {
                        problems.push(tr!("browser.warning.no_common_root"));
                        continue;
                    };
                    let Some(config_base) = self.config_bases.get(collection) else {
                        problems.push(tr!(
                            "browser.warning.reopen_to_apply",
                            collection = collection
                        ));
                        continue;
                    };
                    let Ok(relative) = config_base.strip_prefix(shared) else {
                        problems.push(tr!(
                            "browser.warning.base_outside_root",
                            collection = collection
                        ));
                        continue;
                    };
                    Path::new(directory).join(relative)
                }
                (None, None) => continue,
            };
            let shown = candidate.display().to_string();
            match contained_path(root, &candidate) {
                Ok(path) if path.is_dir() => {
                    bases.insert(collection.to_owned(), candidate);
                }
                Ok(_) => problems.push(tr!(
                    "browser.warning.override_not_directory",
                    shown = shown,
                    collection = collection
                )),
                Err(error) => problems.push(tr!(
                    "browser.warning.override_invalid",
                    shown = shown,
                    collection = collection,
                    error = error
                )),
            }
        }
        (bases, problems)
    }

    /// Recompute the applied overrides and re-index when the effective bases change.
    fn refresh_collection_bases(&mut self, cx: &mut Context<Self>) {
        let Some(manifest) = self.manifest.clone() else {
            self.collection_bases.clear();
            self.override_problems.clear();
            return;
        };
        let (bases, problems) = self.resolve_collection_bases(&manifest);
        if !problems.is_empty() {
            tracing::warn!(problems = %problems.join(" "), "unusable content directory override");
        }
        if bases == self.collection_bases {
            if problems != self.override_problems {
                self.override_problems = problems;
                cx.notify();
            }
            return;
        }
        self.override_problems = problems;
        tracing::info!(
            collections = bases.len(),
            overridden = ?bases.keys().cloned().collect::<Vec<_>>(),
            "applying collection content directory overrides"
        );
        self.collection_bases = bases;
        if let Some(task) = self.start_index_request(cx) {
            self.index_task = Some(task);
        }
        cx.notify();
    }

    /// Resolve one collection scope exactly once through the core resolver.
    fn resolved_for(
        &self,
        collection: Option<&str>,
    ) -> astrolith_core::preferences::ResolvedPreferences {
        let collection_preferences = collection.and_then(|name| {
            self.project_preferences
                .as_ref()
                .and_then(|project| project.collection(name))
        });
        resolve(
            &self.global_preferences,
            self.project_preferences.as_ref(),
            collection_preferences,
        )
    }

    fn collection_mappings(&self) -> BTreeMap<String, FieldMappings> {
        let mut mappings = BTreeMap::new();
        for entry in self.entries.iter() {
            let collection = entry.entry().id().collection().as_str();
            if !mappings.contains_key(collection) {
                mappings.insert(
                    collection.to_owned(),
                    self.resolved_for(Some(collection)).field_mappings,
                );
            }
        }
        mappings
    }

    fn rebuild_sort_options(&mut self) {
        let mut dates = BTreeSet::new();
        for schema in self.schemas.values() {
            for field in &schema.fields {
                if matches!(field.kind, FieldKind::Date) {
                    dates.insert(field.name.clone());
                }
            }
        }
        let mut options = vec![
            SortMode::Default,
            SortMode::Title,
            SortMode::Filename,
            SortMode::LastModified,
        ];
        for date in dates {
            options.push(SortMode::DateField(date));
        }
        options.push(SortMode::Order);
        if !options.contains(&self.list_sort.mode) {
            self.list_sort.mode = SortMode::Default;
        }
        self.sort_options = options;
    }

    /// Entry commands wait only for the initial project scan. A watcher refresh
    /// operates on filesystem state directly, so it must not disable them every
    /// poll interval.
    pub(crate) fn is_busy(&self) -> bool {
        self.loading
    }

    /// The session for the active tab, if one is loaded.
    pub(crate) fn active_session(&self, _cx: &App) -> Option<Entity<DocumentSession>> {
        self.active
            .as_ref()
            .and_then(|path| self.sessions.get(path))
            .cloned()
    }

    /// The word, character and line counts of the active session.
    ///
    /// The counts are already scalar: the session never materialises token
    /// ranges for the footer.
    pub(crate) fn active_counts(&self, cx: &App) -> Option<TokenSummary> {
        self.active_session(cx)
            .map(|session| session.read(cx).counts)
    }

    /// The writing modes every retained session applies.
    pub(crate) fn writing_modes(&self) -> WritingModes {
        self.writing
    }

    /// Mirror the shell's dock and window state so every availability reader
    /// gets the same flags.
    pub(crate) fn set_view_state(
        &mut self,
        sidebar: bool,
        inspector: bool,
        preview: bool,
        fullscreen: bool,
    ) {
        self.view_sidebar = sidebar;
        self.view_inspector = inspector;
        self.view_preview = preview;
        self.view_fullscreen = fullscreen;
    }

    /// The one availability snapshot shared by the palette, toolbar and menus.
    ///
    /// Editor focus is read from the window, which is the only source that is
    /// never stale: the session cannot cache it because the facade exposes no
    /// independent focus signal of its own.
    pub(crate) fn command_availability(&self, cx: &App, window: &Window) -> CommandAvailability {
        let session = self.active_session(cx);
        let focused = session.as_ref().is_some_and(|session| {
            session
                .read(cx)
                .editor
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        });
        let (kind, source_view) = session
            .as_ref()
            .map(|session| {
                let session = session.read(cx);
                (session.kind(), session.is_source_view())
            })
            .unwrap_or((DocumentKind::Plain, false));
        let markdown = matches!(kind, DocumentKind::Markdown) && !source_view;
        let mdx = matches!(kind, DocumentKind::Mdx) && !source_view;
        let selection_nonempty = session
            .as_ref()
            .is_some_and(|session| !session.read(cx).editor.read(cx).selected_range().is_empty());
        let save_conflict = self.active.as_ref().is_some_and(|path| {
            let externally_changed_dirty = self.sessions.get(path).is_some_and(|session| {
                let session = session.read(cx);
                session.externally_changed && session.dirty
            });
            externally_changed_dirty
                || matches!(
                    self.save_coordinator.status(path),
                    Some(SaveStatus::Conflict)
                )
        });
        CommandAvailability {
            project_open: self.manifest.is_some(),
            active_entry: self.active.is_some(),
            busy: self.is_busy(),
            editor_focused: focused,
            markdown_document: markdown,
            mdx_document: mdx,
            selection_nonempty,
            ide_configured: self.global_preferences.ide.is_configured(),
            sidebar_visible: self.view_sidebar,
            inspector_visible: self.view_inspector,
            frontmatter_available: self.active_frontmatter_key().is_some(),
            workspace_mode: Default::default(),
            preview_visible: self.view_preview,
            preview_attached: false,
            preview_running: false,
            preview_ready: false,
            focus_mode: self.writing.focus,
            typewriter_mode: self.writing.typewriter,
            fullscreen: self.view_fullscreen,
            updates_configured: false,
            save_conflict,
        }
    }

    /// Apply one formatting command to the active session.
    pub(crate) fn apply_format(
        &mut self,
        command: FormatCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let availability = self.command_availability(cx, window);
        let allowed = availability.editor_focused
            && (command != FormatCommand::Comment || availability.markdown_document);
        if !allowed {
            self.status = tr!("browser.status.formatting_unfocused");
            cx.notify();
            return false;
        }
        let Some(session) = self.active_session(cx) else {
            return false;
        };
        let result = session.update(cx, |session, cx| session.apply_format(command, window, cx));
        if let Err(message) = result {
            self.status = message.to_string();
        }
        cx.notify();
        true
    }

    /// Open the active editor's native find or replace panel.
    pub(crate) fn open_find(&mut self, replace: bool, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.active_session(cx) else {
            return false;
        };
        session.update(cx, |session, cx| {
            session
                .editor
                .update(cx, |editor, cx| editor.open_search(replace, cx));
        });
        true
    }

    /// Step to the next or previous match in the active editor's search panel.
    pub(crate) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.active_session(cx) else {
            return false;
        };
        session.update(cx, |session, cx| {
            session.editor.update(cx, |editor, cx| {
                let found = if forward {
                    editor.next_search_match(cx)
                } else {
                    editor.previous_search_match(cx)
                };
                if found.is_some() {
                    // The source surface owns both layout and scroll: the editor
                    // itself has no mounted element to repaint, so publish the
                    // step here for its observer.
                    cx.notify();
                }
                found.is_some()
            })
        })
    }

    /// Close the active editor's find panel.
    pub(crate) fn close_find(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.active_session(cx) else {
            return false;
        };
        session.update(cx, |session, cx| {
            session
                .editor
                .update(cx, |editor, cx| editor.close_search(cx));
        });
        true
    }

    /// Extend the selection to the next occurrence, active selection only.
    pub(crate) fn select_next_occurrence(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.active_session(cx) else {
            return false;
        };
        session
            .update(cx, |session, cx| session.select_next_occurrence(cx))
            .is_some()
    }

    /// Replace the cursor set with cursors at the main selection's line ends.
    pub(crate) fn cursors_at_line_ends(&mut self, cx: &mut Context<Self>) -> usize {
        let Some(session) = self.active_session(cx) else {
            return 0;
        };
        session.update(cx, |session, cx| session.cursors_at_line_ends(cx))
    }

    /// Switch the active editor between complete source and writer projection.
    pub(crate) fn set_source_view(
        &mut self,
        source: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source_view == source {
            return;
        }
        self.source_view = source;
        if !source {
            self.git_status = None;
            self.git_task = None;
            self.file_browser
                .update(cx, |browser, cx| browser.set_git_status(None, cx));
        }
        let root = self.project_root();
        let active = self.active.clone();
        self.file_browser.update(cx, |browser, cx| {
            browser.sync_root(root, source, cx);
            browser.set_active(active, cx);
        });
        let Some(path) = self.active.clone() else {
            self.sync_lsp(cx);
            return;
        };
        let Some(session) = self.sessions.get(&path).cloned() else {
            self.sync_lsp(cx);
            return;
        };
        session.update(cx, |session, cx| {
            session.set_source_view(source, window, cx)
        });
        self.install(&path, session, cx);
        self.sync_lsp(cx);
    }

    /// Stop caret animation in every retained session while another mode is
    /// shown. Sessions are kept for history and selection, not for rendering.
    pub(crate) fn set_writer_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sessions: Vec<Entity<DocumentSession>> = self.sessions.values().cloned().collect();
        for session in sessions {
            session.update(cx, |session, cx| {
                session.set_writer_visible(visible, window, cx)
            });
        }
    }

    /// Apply the persisted writing modes after the shell replaces preferences.
    pub(crate) fn apply_writing_preferences(
        &mut self,
        focus: bool,
        typewriter: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modes = WritingModes { focus, typewriter };
        if modes != self.writing {
            self.set_writing_modes(modes, window, cx);
        }
    }

    /// Toggle focus mode for every retained session.
    pub(crate) fn toggle_focus_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut modes = self.writing;
        modes.focus = !modes.focus;
        self.set_writing_modes(modes, window, cx);
    }

    /// Toggle typewriter mode for every retained session.
    pub(crate) fn toggle_typewriter_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut modes = self.writing;
        modes.typewriter = !modes.typewriter;
        self.set_writing_modes(modes, window, cx);
    }

    fn set_writing_modes(
        &mut self,
        modes: WritingModes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.writing = modes;
        let sessions: Vec<Entity<DocumentSession>> = self.sessions.values().cloned().collect();
        for session in sessions {
            session.update(cx, |session, cx| {
                session.set_writing_modes(modes, window, cx)
            });
        }
        cx.notify();
    }

    /// Install Copyedit preferences on every retained session.
    pub(crate) fn set_copyedit_config(&mut self, config: CopyeditConfig, cx: &mut Context<Self>) {
        if config == self.copyedit {
            return;
        }
        self.apply_copyedit_config(config, cx);
    }

    fn apply_copyedit_config(&mut self, config: CopyeditConfig, cx: &mut Context<Self>) {
        self.copyedit = config.clone();
        let sessions: Vec<Entity<DocumentSession>> = self.sessions.values().cloned().collect();
        for session in sessions {
            session.update(cx, |session, cx| {
                session.set_copyedit_config(config.clone(), cx)
            });
        }
    }

    /// Move the caret to a 1-based line in the active document.
    pub(crate) fn go_to_line(
        &mut self,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.active_session(cx) else {
            return false;
        };
        let moved = session.update(cx, |session, cx| session.go_to_line(line, window, cx));
        if !moved {
            self.status = tr!("browser.status.line_outside_document", line = line);
        }
        cx.notify();
        moved
    }

    /// Prompt for a line number and move the caret there.
    pub(crate) fn open_go_to_line_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some(session) = self.active_session(cx) else {
            self.status = tr!("browser.status.open_document_for_goto");
            cx.notify();
            return;
        };
        let lines = session.read(cx).line_count(cx);
        let owner = cx.weak_entity();
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("browser.dialog.line_number_placeholder"))
        });
        let focus_input = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let input = input.clone();
            dialog
                .title(tr!("browser.dialog.goto_line_title"))
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(div().text_sm().child(SharedString::from(tr!(
                            "browser.dialog.document_lines",
                            lines = lines
                        ))))
                        .child(Input::new(&input)),
                )
                .footer(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("goto-line-cancel")
                                .label(tr!("browser.dialog.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("goto-line-confirm")
                                .primary()
                                .label(tr!("browser.dialog.go"))
                                .on_click(move |_, window, cx| {
                                    let typed = input.read(cx).value().to_string();
                                    window.close_dialog(cx);
                                    let _ = owner.update(cx, |project, cx| {
                                        match typed.trim().parse::<usize>() {
                                            Ok(line) => {
                                                project.go_to_line(line, window, cx);
                                            }
                                            Err(_) => {
                                                project.status =
                                                    tr!("browser.status.enter_line_number");
                                                cx.notify();
                                            }
                                        }
                                    });
                                }),
                        ),
                )
        });
        focus_input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Re-scan the index after an operation and publish it with any follow-up open.
    fn rescan(&mut self, open: Option<SourcePath>, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.generation;
        let Some(index) = self.start_index_request(cx) else {
            return;
        };
        let Some(source) = open else {
            self.index_task = Some(index);
            return;
        };
        // The follow-up open is bound to this project generation. A newer index
        // may have superseded this request; it still contains the written entry,
        // so the source is opened from whatever index is current.
        self.index_task = Some(cx.spawn_in(window, async move |this, cx| {
            let _ = index.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation == generation {
                    this.open_source(&source, window, cx);
                }
                cx.notify();
            });
        }));
    }

    fn open_source(&mut self, source: &SourcePath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.entry().source() == source)
        else {
            self.status = tr!(
                "browser.status.written_not_indexed",
                source = source.as_str()
            );
            cx.notify();
            return;
        };
        self.open_entry_after_publish(index, window, cx);
    }

    /// Drop clean sessions, selection and tabs whose source is no longer indexed.
    ///
    /// Dirty buffers are always retained: unsaved work is never discarded by a
    /// watcher batch or a failed rescan.
    fn reconcile_sessions(&mut self, cx: &mut Context<Self>) {
        if self.index_error.is_some() {
            return;
        }
        let Some(root) = self.project_root() else {
            return;
        };
        let indexed: BTreeSet<String> = self
            .entries
            .iter()
            .map(|entry| entry.entry().source().as_str().to_owned())
            .collect();
        let stale: Vec<PathBuf> = self
            .sessions
            .iter()
            .filter(|(path, session)| {
                let Some(source) = relative_source(&root, path) else {
                    return false;
                };
                !indexed.contains(source.as_str()) && !session.read(cx).is_dirty()
            })
            .map(|(path, _)| path.clone())
            .collect();
        for path in stale {
            tracing::debug!(path = %path.display(), "closing a session no longer in the index");
            self.remove_session(&path, cx);
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| !self.sessions.contains_key(active))
        {
            self.active = None;
            self.document
                .update(cx, |document, cx| document.clear_editor(cx));
        }
        self.sync_tabs(cx);
        self.refresh_entry_selection(cx);
    }

    fn publish_list_state(&mut self, cx: &mut Context<Self>) {
        let root = self.project_root();
        let active = self.active.clone();
        let entries = self.entries.clone();
        let mappings = self.collection_mappings();
        let filter = self.list_filter.clone();
        let sort = self.list_sort.clone();
        let collection_filter = self.selected_collection.clone().map(Into::into);
        let directory = self.selected_directory.clone();
        let index_revision = self.index_revision;
        self.list.update(cx, |list, cx| {
            let delegate = list.delegate_mut();
            delegate.entries = entries;
            delegate.root = root;
            delegate.active = active;
            delegate.mappings = mappings;
            delegate.filter = filter;
            delegate.sort = sort;
            delegate.collection_filter = collection_filter;
            delegate.directory = directory;
            delegate.index_revision = index_revision;
            delegate.invalidate();
            delegate.rebuild();
            delegate.selected = delegate
                .selected_row()
                .map(|(section, row)| IndexPath::new(row).section(section));
            cx.notify();
        });
    }

    /// Show one collection's entries, or return to the collection root.
    pub(crate) fn select_collection(
        &mut self,
        collection: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_collection == collection {
            return;
        }
        self.selected_collection = collection;
        self.selected_directory.clear();
        self.list_filter = EntryFilter::default();
        self.last_list_query.clear();
        self.list
            .update(cx, |list, cx| list.set_query("", window, cx));
        self.publish_list_state(cx);
        cx.notify();
    }

    /// Browse one directory below the selected collection's base.
    pub(crate) fn select_directory(
        &mut self,
        directory: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_directory == directory {
            return;
        }
        self.selected_directory = directory;
        self.list_filter = EntryFilter::default();
        self.last_list_query.clear();
        self.list
            .update(cx, |list, cx| list.set_query("", window, cx));
        self.publish_list_state(cx);
        cx.notify();
    }

    /// Move one level up: the parent directory, then the collection index.
    pub(crate) fn library_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match parent_directory(&self.selected_directory) {
            Some(parent) => self.select_directory(parent, window, cx),
            None => self.select_collection(None, window, cx),
        }
    }

    /// Drop a browsed directory that the latest index no longer contains.
    fn normalize_browsed_directory(&mut self) {
        let Some(collection) = self.selected_collection.clone() else {
            self.selected_directory.clear();
            return;
        };
        let mut existing: BTreeSet<Vec<String>> = BTreeSet::new();
        for entry in self.entries.iter() {
            if entry.entry().id().collection().as_str() != collection.as_str() {
                continue;
            }
            let segments = entry_directory(entry);
            for depth in 0..=segments.len() {
                existing.insert(segments[..depth].to_vec());
            }
        }
        while !self.selected_directory.is_empty() && !existing.contains(&self.selected_directory) {
            self.selected_directory.pop();
        }
    }

    pub(crate) fn toggle_drafts_only(&mut self, cx: &mut Context<Self>) {
        self.list_filter.drafts_only = !self.list_filter.drafts_only;
        self.publish_list_state(cx);
        cx.notify();
    }

    pub(crate) fn set_sort_mode(&mut self, mode: SortMode, cx: &mut Context<Self>) {
        self.list_sort.mode = mode;
        self.publish_list_state(cx);
        cx.notify();
    }

    pub(crate) fn toggle_sort_direction(&mut self, cx: &mut Context<Self>) {
        if self.list_sort.mode == SortMode::Default {
            return;
        }
        self.list_sort.direction = self.list_sort.direction.flipped();
        self.publish_list_state(cx);
        cx.notify();
    }

    pub(crate) fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list_filter = EntryFilter::default();
        self.last_list_query.clear();
        self.list
            .update(cx, |list, cx| list.set_query("", window, cx));
        self.publish_list_state(cx);
        cx.notify();
    }

    pub(crate) fn retry_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rescan(None, window, cx);
    }

    #[cfg(test)]
    pub(crate) fn visible_entry_names(&self, cx: &App) -> Vec<String> {
        self.list
            .read(cx)
            .delegate()
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .filter(|row| row.directory.is_none())
            .map(|row| row.filename.to_string())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn visible_directory_rows(&self, cx: &App) -> Vec<(String, usize)> {
        self.list
            .read(cx)
            .delegate()
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .filter_map(|row| {
                row.directory
                    .as_ref()
                    .map(|directory| (row.title.to_string(), directory.count))
            })
            .collect()
    }
}

impl Render for ProjectWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_project = self.manifest.is_some();
        let loading = self.loading;
        let availability = self.command_availability(cx, window);
        let (visible_rows, visible_entries, scope_total) = {
            let delegate = self.list.read(cx).delegate();
            (
                delegate.visible_count(),
                delegate.entry_count(),
                delegate.scope_total,
            )
        };
        let visible = visible_entries;
        let selected_collection = self.selected_collection.clone();
        let selected_directory = self.selected_directory.clone();
        let collections = self.collection_summaries.clone();
        let query = self.last_list_query.trim().to_owned();
        let drafts_only = self.list_filter.drafts_only;
        let filter_active = drafts_only || !query.is_empty();
        let total = selected_collection
            .as_ref()
            .map(|selected| {
                if filter_active {
                    collections
                        .iter()
                        .find(|(name, _)| name == selected)
                        .map(|(_, count)| *count)
                        .unwrap_or(0)
                } else {
                    scope_total
                }
            })
            .unwrap_or(self.entries.len());
        let default_sort = self.list_sort.mode == SortMode::Default;
        let sort_options = self.sort_options.clone();
        let current_mode = self.list_sort.mode.clone();
        let index_error = self.index_error.clone();
        let open_error = self.open_error.clone();
        let override_problems = self.override_problems.clone();
        let status = self.status.clone();

        let view = div().v_flex().size_full().min_h_0().min_w_0();
        if !has_project {
            return view
                .child(if loading {
                    project_loading_state(cx).into_any_element()
                } else if let Some(error) = open_error {
                    project_open_error_state(&error, cx).into_any_element()
                } else {
                    project_empty_state(loading, cx).into_any_element()
                })
                .into_any_element();
        }
        // Build navigates the real project files; Writing and Preview keep the
        // collection library. The explorer owns its own scan and expansion state.
        if self.source_view {
            return view.child(self.file_browser.clone()).into_any_element();
        }

        let more_owner = cx.weak_entity();
        let absolute_owner = cx.weak_entity();
        let menu_owner = cx.weak_entity();
        let collection_owner = cx.weak_entity();
        let menu_options = sort_options.clone();
        let menu_current = current_mode.clone();

        let back_owner = cx.weak_entity();
        let back_directory = selected_directory.clone();
        let back = selected_collection.is_some().then(|| {
            Button::new("library-back")
                .small()
                .ghost()
                .icon(IconName::ArrowLeft)
                .tooltip(if back_directory.is_empty() {
                    tr!("browser.library.all_collections")
                } else {
                    tr!("browser.library.up_one_folder")
                })
                .accessibility_label(if back_directory.is_empty() {
                    tr!("browser.library.all_collections")
                } else {
                    tr!("browser.library.up_one_folder")
                })
                .on_click(move |_, window, cx| {
                    let _ = back_owner.update(cx, |project, cx| {
                        project.library_back(window, cx);
                    });
                })
        });

        let crumb_owner = cx.weak_entity();
        let mut trail: Vec<(String, Option<CrumbAction>)> = Vec::new();
        if let Some(collection) = selected_collection.as_ref() {
            trail.push((tr!("browser.library.title"), Some(CrumbAction::Collections)));
            let collection_label = collection_title(collection);
            if selected_directory.is_empty() {
                trail.push((collection_label, None));
            } else {
                trail.push((collection_label, Some(CrumbAction::Directory(Vec::new()))));
                for (index, segment) in selected_directory.iter().enumerate() {
                    let last = index + 1 == selected_directory.len();
                    trail.push((
                        segment.clone(),
                        (!last)
                            .then(|| CrumbAction::Directory(selected_directory[..=index].to_vec())),
                    ));
                }
            }
        }
        let title_row = {
            let mut row = div().h_flex().items_center().gap_2().min_w_0();
            if trail.is_empty() {
                row = row.child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .truncate()
                        .child(SharedString::from(tr!("browser.library.title"))),
                );
            } else {
                let last_index = trail.len().saturating_sub(1);
                for (index, (label, action)) in trail.into_iter().enumerate() {
                    if index > 0 {
                        row = row.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("›"),
                        );
                    }
                    match action {
                        Some(action) => {
                            let owner = crumb_owner.clone();
                            row = row.child(
                                Button::new(format!("library-crumb-{index}"))
                                    .small()
                                    .ghost()
                                    .label(label)
                                    .on_click(move |_, window, cx| {
                                        let _ =
                                            owner.update(cx, |project, cx| match action.clone() {
                                                CrumbAction::Collections => {
                                                    project.select_collection(None, window, cx);
                                                }
                                                CrumbAction::Directory(segments) => {
                                                    project.select_directory(segments, window, cx);
                                                }
                                            });
                                    }),
                            );
                        }
                        None => {
                            let weight = if index == last_index {
                                gpui_kit::FontWeight::SEMIBOLD
                            } else {
                                gpui_kit::FontWeight::MEDIUM
                            };
                            row = row.child(
                                div()
                                    .text_sm()
                                    .font_weight(weight)
                                    .truncate()
                                    .child(SharedString::from(label)),
                            );
                        }
                    }
                }
            }
            row
        };
        let mut view = view.child(
            div()
                .v_flex()
                .flex_shrink_0()
                .border_b_1()
                .border_color(cx.theme().border.opacity(0.55))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .pt_3()
                        .pb_2()
                        .when_some(back, |row, back| row.child(back))
                        .child(div().flex_1().min_w_0().child(title_row))
                        .child(div().flex_1())
                        .child(
                            ui::icon_button(
                                "open-project",
                                IconName::FolderOpen,
                                tr!("browser.library.open_project"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.prompt(window, cx))),
                        )
                        .when(selected_collection.is_some(), |row| {
                            row.child(
                                Button::new("entry-more")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Ellipsis)
                                    .tooltip(tr!("browser.library.actions"))
                                    .accessibility_label(tr!("browser.library.actions"))
                                    .dropdown_menu(move |menu, _, _| {
                                        let mut menu = menu;
                                        for command in [
                                            CommandId::DuplicateEntry,
                                            CommandId::RenameEntry,
                                            CommandId::DeleteEntry,
                                            CommandId::RevealEntry,
                                        ] {
                                            let owner = more_owner.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(command.label())
                                                    .disabled(!command.is_available(availability))
                                                    .on_click(move |_, window, cx| {
                                                        let _ = owner.update(cx, |project, cx| {
                                                            project.entry_command(
                                                                command, None, window, cx,
                                                            )
                                                        });
                                                    }),
                                            );
                                        }
                                        let owner = more_owner.clone();
                                        menu = menu
                                            .separator()
                                            .item(
                                                PopupMenuItem::new(tr!(
                                                    "browser.library.copy_relative_path"
                                                ))
                                                .disabled(
                                                    !CommandId::CopyPathRelative
                                                        .is_available(availability),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let _ = owner.update(cx, |project, cx| {
                                                        project.entry_command(
                                                            CommandId::CopyPathRelative,
                                                            None,
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                            )
                                            .item(
                                                PopupMenuItem::new(tr!(
                                                    "browser.library.copy_absolute_path"
                                                ))
                                                .disabled(
                                                    !CommandId::CopyPathAbsolute
                                                        .is_available(availability),
                                                )
                                                .on_click({
                                                    let owner = absolute_owner.clone();
                                                    move |_, window, cx| {
                                                        let _ = owner.update(cx, |project, cx| {
                                                            project.entry_command(
                                                                CommandId::CopyPathAbsolute,
                                                                None,
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                    }
                                                }),
                                            )
                                            .separator()
                                            .item(
                                                PopupMenuItem::new(tr!(
                                                    "browser.library.show_drafts_only"
                                                ))
                                                .checked(drafts_only)
                                                .on_click({
                                                    let owner = menu_owner.clone();
                                                    move |_, _, cx| {
                                                        let _ = owner.update(cx, |project, cx| {
                                                            project.toggle_drafts_only(cx)
                                                        });
                                                    }
                                                }),
                                            )
                                            .separator();
                                        for mode in menu_options.iter().cloned() {
                                            let owner = menu_owner.clone();
                                            let selected = mode == menu_current;
                                            let chosen = mode.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(sort_label(&mode))
                                                    .checked(selected)
                                                    .on_click(move |_, _, cx| {
                                                        let _ = owner.update(cx, |project, cx| {
                                                            project
                                                                .set_sort_mode(chosen.clone(), cx)
                                                        });
                                                    }),
                                            );
                                        }
                                        let owner = menu_owner.clone();
                                        menu.item(
                                            PopupMenuItem::new(tr!(
                                                "browser.library.reverse_sort_direction"
                                            ))
                                            .disabled(default_sort)
                                            .on_click(move |_, _, cx| {
                                                let _ = owner.update(cx, |project, cx| {
                                                    project.toggle_sort_direction(cx)
                                                });
                                            }),
                                        )
                                        .when(
                                            filter_active,
                                            |menu| {
                                                let owner = menu_owner.clone();
                                                menu.separator().item(
                                                    PopupMenuItem::new(tr!(
                                                        "browser.library.clear_filter"
                                                    ))
                                                    .on_click(move |_, window, cx| {
                                                        let _ = owner.update(cx, |project, cx| {
                                                            project.clear_filter(window, cx)
                                                        });
                                                    }),
                                                )
                                            },
                                        )
                                    }),
                            )
                        }),
                )
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .px_4()
                        .pb_3()
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(if selected_collection.is_some() {
                                    if filter_active {
                                        tr!(
                                            "browser.count.visible_of_total",
                                            visible = visible,
                                            total = total
                                        )
                                    } else {
                                        tr!("browser.count.entries", count = total)
                                    }
                                } else {
                                    tr!("browser.count.collections", count = collections.len())
                                }))
                                .when(filter_active, |row| {
                                    row.child(
                                        div()
                                            .id("filter-indicator")
                                            .test_support()
                                            .text_color(cx.theme().foreground)
                                            .child(SharedString::from(filter_label(
                                                drafts_only,
                                                &query,
                                            ))),
                                    )
                                }),
                        )
                        .child(div().flex_1())
                        .when(selected_collection.is_some(), |row| {
                            row.child(
                                Button::new("entry-new")
                                    .small()
                                    .icon(IconName::PenLine)
                                    .label(tr!("browser.library.new_entry"))
                                    .tooltip(CommandId::NewEntry.label())
                                    .disabled(!CommandId::NewEntry.is_available(availability))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.entry_command(CommandId::NewEntry, None, window, cx)
                                    })),
                            )
                        })
                        .when(filter_active, |row| {
                            row.child(
                                Button::new("clear-filter")
                                    .small()
                                    .ghost()
                                    .label(tr!("browser.library.clear_filter"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_filter(window, cx)
                                    })),
                            )
                        }),
                ),
        );

        if loading {
            view = view.child(project_loading_state(cx));
        } else if index_error.is_none() && selected_collection.is_none() {
            if collections.is_empty() {
                view = view.child(
                    div()
                        .id("project-no-collections")
                        .test_support()
                        .v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .p_6()
                        .child(
                            Icon::new(IconName::BookOpen)
                                .size_6()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .child(tr!("browser.library.no_collections_title")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(tr!("browser.library.no_collections_body")),
                        ),
                );
            } else {
                view =
                    view.child(
                        div()
                            .id("project-collections")
                            .test_support()
                            .v_flex()
                            .flex_1()
                            .min_h_0()
                            .gap_0p5()
                            .p_2()
                            .overflow_y_scroll()
                            .children(collections.iter().enumerate().map(
                                |(index, (name, count))| {
                                    let owner = collection_owner.clone();
                                    let chosen = name.clone();
                                    let title = collection_title(name);
                                    Button::new(("library-collection", index))
                                        .ghost()
                                        .w_full()
                                        .h(px(56.))
                                        .justify_start()
                                        .rounded(cx.theme().radius)
                                        .selected(
                                            selected_collection.as_deref() == Some(name.as_str()),
                                        )
                                        .child(
                                            div()
                                                .h_flex()
                                                .w_full()
                                                .items_center()
                                                .gap_3()
                                                .px_2()
                                                .child(
                                                    Icon::new(IconName::BookOpen)
                                                        .size_4()
                                                        .text_color(cx.theme().muted_foreground),
                                                )
                                                .child(
                                                    div()
                                                        .v_flex()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .gap_0p5()
                                                        .child(
                                                            div()
                                                                .font_family(
                                                                    crate::theme::prose_font(cx),
                                                                )
                                                                .text_sm()
                                                                .font_weight(
                                                                    gpui_kit::FontWeight::MEDIUM,
                                                                )
                                                                .truncate()
                                                                .child(SharedString::from(title)),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child(SharedString::from(
                                                                    if *count == 1 {
                                                                        tr!(
                                                                            "browser.count.entry",
                                                                            count = count
                                                                        )
                                                                    } else {
                                                                        tr!(
                                                                            "browser.count.entries",
                                                                            count = count
                                                                        )
                                                                    },
                                                                )),
                                                        ),
                                                )
                                                .child(
                                                    Icon::new(IconName::ChevronRight)
                                                        .xsmall()
                                                        .text_color(cx.theme().muted_foreground),
                                                ),
                                        )
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |project, cx| {
                                                project.select_collection(
                                                    Some(chosen.clone()),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        })
                                },
                            )),
                    );
            }
        } else if let Some(error) = index_error {
            view = view.child(
                div()
                    .id("project-error")
                    .test_support()
                    .v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_6()
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size_6()
                            .text_color(cx.theme().danger),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(tr!("browser.library.index_unavailable")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(error),
                    )
                    .child(
                        Button::new("retry-index")
                            .small()
                            .label(tr!("browser.library.retry"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.retry_index(window, cx)),
                            ),
                    ),
            );
        } else if visible_rows == 0 {
            view = view.child(if filter_active {
                div()
                    .id("project-filtered-empty")
                    .test_support()
                    .v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_6()
                    .child(
                        Icon::new(IconName::ListFilter)
                            .size_6()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(tr!("browser.library.filtered_empty_title")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr!("browser.library.filtered_empty_body")),
                    )
            } else {
                div()
                    .id("project-no-entries")
                    .test_support()
                    .v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_6()
                    .child(
                        Icon::new(IconName::FileText)
                            .size_6()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().text_sm().child(tr!("browser.library.empty_title")))
                    .child(
                        div()
                            .text_xs()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr!("browser.library.empty_body")),
                    )
            });
        } else {
            view = view.child(div().flex_1().min_h_0().p_2().child(
                List::new(&self.list).search_placeholder(tr!("browser.library.search_placeholder")),
            ));
        }

        if !override_problems.is_empty() {
            view = view.child(
                div()
                    .id("project-override-warning")
                    .test_support()
                    .border_t_1()
                    .border_color(cx.theme().window_border)
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(override_problems.join(" "))),
            );
        }
        if !status.is_empty() {
            view = view.child(
                div()
                    .id("project-status")
                    .test_support()
                    .border_t_1()
                    .border_color(cx.theme().window_border)
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(status)),
            );
        }
        view.into_any_element()
    }
}

/// Render one metadata scalar for Content Finder display.
fn meta_scalar(value: &MetaValue) -> Option<String> {
    match value {
        MetaValue::String(text) => Some(text.clone()),
        MetaValue::Boolean(flag) => Some(flag.to_string()),
        MetaValue::Integer(number) => Some(number.to_string()),
        MetaValue::Real(number) => Some(number.to_string()),
    }
}

/// Reader-facing title for a collection id.
fn collection_title(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn filter_label(drafts_only: bool, query: &str) -> String {
    match (drafts_only, query.is_empty()) {
        (true, true) => tr!("browser.filter.drafts_only"),
        (true, false) => tr!("browser.filter.drafts_query", query = query),
        (false, false) => tr!("browser.filter.query", query = query),
        (false, true) => String::new(),
    }
}

/// Reader-facing label for one sort mode; translated at render time.
fn sort_label(mode: &SortMode) -> SharedString {
    match mode {
        SortMode::Default => tr!("browser.sort.default").into(),
        SortMode::Title => tr!("browser.sort.title").into(),
        SortMode::Filename => tr!("browser.sort.filename").into(),
        SortMode::LastModified => tr!("browser.sort.last_modified").into(),
        SortMode::DateField(field) => tr!("browser.sort.date_field", field = field).into(),
        SortMode::Order => tr!("browser.sort.order").into(),
    }
}

/// The initial-scan state, distinct from the empty and error states.
fn project_loading_state(cx: &mut Context<ProjectWorkspace>) -> impl IntoElement {
    div()
        .id("project-loading")
        .test_support()
        .v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(
            gpui_kit::component::spinner::Spinner::new()
                .large()
                .color(cx.theme().muted_foreground),
        )
        .child(div().text_sm().child(tr!("browser.library.scanning")))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(tr!("browser.library.loading_body")),
        )
}

/// The failed-open state: distinct from "never opened" and from an index error.
fn project_open_error_state(message: &str, cx: &mut Context<ProjectWorkspace>) -> impl IntoElement {
    ui::empty_state(
        "project-open-error",
        IconName::TriangleAlert,
        tr!("browser.library.open_error_title"),
        message.to_owned(),
        Some(
            Button::new("retry-open-project")
                .small()
                .label(tr!("browser.library.choose_another_project"))
                .on_click(cx.listener(|this, _, window, cx| this.prompt(window, cx)))
                .into_any_element(),
        ),
        cx,
    )
}

/// The no-project state.
fn project_empty_state(loading: bool, cx: &mut Context<ProjectWorkspace>) -> impl IntoElement {
    ui::empty_state(
        "project-empty",
        IconName::FolderOpen,
        tr!("browser.library.no_project_title"),
        tr!("browser.library.no_project_body"),
        Some(
            Button::new("open-project")
                .small()
                .label(tr!("browser.library.open_project"))
                .disabled(loading)
                .on_click(cx.listener(|this, _, window, cx| this.prompt(window, cx)))
                .into_any_element(),
        ),
        cx,
    )
}

/// Deepest directory shared by every collection base.
///
/// A project-level content directory replaces that shared root while keeping
/// each collection's relative position underneath it.
fn shared_collection_base(bases: &BTreeMap<String, PathBuf>) -> Option<PathBuf> {
    let mut bases = bases.values();
    let first = bases.next()?;
    let mut shared = first.clone();
    for base in bases {
        while !base.starts_with(&shared) {
            if !shared.pop() {
                return None;
            }
        }
    }
    Some(shared)
}

/// Markdown and MDX define frontmatter; YAML data entries are whole documents.
fn supports_frontmatter(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| matches!(extension, "md" | "mdx"))
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Scan project-owned `.astro` sources for the Build file navigator.
///
/// The walk is bounded by depth, directory and result count, skips dependency
/// and build output directories, and never follows symlinks.
pub(crate) fn scan_project_astro_sources(root: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    const SKIP: [&str; 6] = ["node_modules", "target", "dist", "build", ".git", ".astro"];
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut found = Vec::new();
    let mut inspected = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        if depth > 12 || inspected >= 5_000 || found.len() >= 500 {
            break;
        }
        let entries = std::fs::read_dir(&directory).map_err(|error| {
            tr!(
                "browser.status.read_failed",
                path = directory.display(),
                error = error
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            inspected += 1;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_dir() {
                if !name.starts_with('.') && !SKIP.contains(&name.as_ref()) {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !file_type.is_file() || !name.ends_with(".astro") {
                continue;
            }
            if let Ok(relative) = path.strip_prefix(root) {
                let relative = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                found.push((path, relative));
            }
        }
    }
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/// Project-relative canonical source for one absolute path, if it is inside.
fn relative_source(root: &Path, path: &Path) -> Option<SourcePath> {
    let relative = path.strip_prefix(root).ok()?;
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(segment) => segments.push(segment.to_str()?),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if segments.is_empty() {
        return None;
    }
    SourcePath::new(segments.join("/")).ok()
}

/// Today's local calendar date as `YYYY-MM-DD`, used for new entry names.
fn today() -> String {
    chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

/// Open one directory in the platform file manager without blocking the UI.
async fn open_directory(directory: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = tokio::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = tokio::process::Command::new("xdg-open");
    #[cfg(not(any(unix, windows)))]
    return Err(tr!("browser.status.file_manager_unsupported"));
    #[allow(unreachable_code)]
    {
        let status = command
            .arg(directory)
            .status()
            .await
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(tr!("browser.status.file_manager_exited", status = status))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn build_edits_text_sources_but_not_assets() {
        use super::is_editable_extension;
        for extension in [
            "astro", "ts", "tsx", "js", "jsx", "mjs", "json", "css", "scss", "html", "svelte",
            "vue", "md", "mdx", "yaml", "toml",
        ] {
            assert!(is_editable_extension(extension), "{extension}");
        }
        for extension in ["png", "jpg", "woff2", "wasm", "svg", "exe"] {
            assert!(!is_editable_extension(extension), "{extension}");
        }
        assert!(
            is_editable_extension("ASTRO"),
            "extensions are case-insensitive"
        );
    }
    use super::*;
    use astrolith_core::SourcePath;
    use astrolith_core::preferences::{
        CollectionPreferencesV1, CopyeditCategory, CopyeditPreferences,
    };
    use gpui_kit::{TestAppContext, WindowHandle, WindowOptions, base::Root, test::TestWindowExt};
    use std::sync::Mutex;

    /// Issue a coordinator request without running the guarded save, so a test
    /// can inject the completion seam deterministically.
    pub(super) fn save_request(
        view: &Entity<ProjectWorkspace>,
        path: &Path,
        kind: SaveKind,
        cx: &mut App,
    ) -> SaveRequest {
        let session = view
            .read(cx)
            .sessions
            .get(path)
            .cloned()
            .expect("the document is open");
        let (revision, dirty) = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            (session.buffer.revision().get(), session.buffer.is_dirty())
        });
        view.update(cx, |view, cx| {
            let now = cx.background_executor().now();
            view.save_coordinator.observe(path, revision, dirty, now);
            view.save_coordinator
                .request_now(path, revision, now, kind)
                .expect("a dirty document requests a save")
        })
    }
    pub(super) fn content_finder_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/astro")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/@astrojs/mdx")).unwrap();
        std::fs::write(
            dir.path().join("node_modules/@astrojs/mdx/package.json"),
            r#"{"version":"4.3.14","peerDependencies":{"astro":"^5.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("astro.config.mjs"),"import {defineConfig} from 'astro/config'; import mdx from '@astrojs/mdx'; export default defineConfig({integrations:[mdx()]});").unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"astro":"5.18.2"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("node_modules/astro/package.json"),
            r#"{"version":"5.18.2"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src/posts")).unwrap();
        std::fs::write(dir.path().join("src/content.config.ts"),"import {defineCollection} from 'astro:content'; import {glob} from 'astro/loaders'; const blog=defineCollection({loader:glob({base:'src/posts',pattern:'**/*.{md,mdx}'})}); export const collections={blog};").unwrap();
        std::fs::write(
            dir.path().join("src/posts/alpha.md"),
            "---\ntitle: Alpha Post\nslug: alpha space\n---\n# Alpha\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/posts/beta.mdx"),
            "---\ntitle: Beta Note\n---\n# Beta\n",
        )
        .unwrap();
        dir
    }

    #[gpui_kit::test]
    async fn dropped_files_copy_into_assets_and_insert_markdown(cx: &mut TestAppContext) {
        let dir = content_finder_fixture();
        let external = tempfile::tempdir().unwrap();
        let image = external.path().join("Drop Photo.PNG");
        let pdf = external.path().join("Paper.pdf");
        std::fs::write(&image, b"not really a png").unwrap();
        std::fs::write(&pdf, b"%PDF-1.7").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let (_, session) = open_entry_for(cx, handle, &view, "src/posts/alpha.md").await;
        let baseline = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.drop_assets(vec![image.clone(), pdf.clone()], window, cx)
            });
        })
        .unwrap();

        let mut text = String::new();
        for _ in 0..300 {
            cx.run_until_parked();
            text = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
            if text.contains("drop-photo.png)") && text.contains("paper.pdf)") {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(
            text.contains("![drop-photo](../assets/") && text.contains("drop-photo.png)"),
            "{text:?}"
        );
        assert!(
            text.contains("[paper](../assets/") && text.contains("paper.pdf)"),
            "{text:?}"
        );
        let assets = dir.path().join("src/assets");
        let names: Vec<String> = std::fs::read_dir(&assets)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(
            names.iter().any(|name| name.ends_with("-drop-photo.png")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|name| name.ends_with("-paper.pdf")),
            "{names:?}"
        );
        assert_eq!(
            std::fs::read(
                assets.join(
                    names
                        .iter()
                        .find(|name| name.ends_with("-paper.pdf"))
                        .unwrap()
                )
            )
            .unwrap(),
            b"%PDF-1.7"
        );

        // The whole batch is one contiguous edit, so one undo removes it.
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-z"
                } else {
                    "ctrl-z"
                },
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), baseline);
        });
    }

    #[gpui_kit::test]
    async fn content_finder_searches_opens_and_inserts_title_links(cx: &mut TestAppContext) {
        let dir = content_finder_fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let entries = view.read_with(cx, |view, _| view.content_finder_entries());
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["Alpha Post", "Beta Note"]
        );
        assert_eq!(entries[0].slug.as_deref(), Some("alpha space"));
        assert_eq!(entries[1].id, "beta");
        let matched = content_finder::search(&entries, "BETA");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].title, "Beta Note");

        cx.update(|cx| {
            view.update(cx, |view, cx| {
                let mut preferences = ProjectPreferencesV1::default();
                preferences.collections.insert(
                    "blog".into(),
                    astrolith_core::preferences::CollectionPreferencesV1 {
                        content_finder_url_pattern: Some("/blog/{slug}/".into()),
                        ..Default::default()
                    },
                );
                view.set_preferences(GlobalPreferencesV2::default(), Some(preferences), cx);
            });
        });

        let (alpha_path, alpha) = open_entry_for(cx, handle, &view, "src/posts/alpha.md").await;
        assert_eq!(
            view.read_with(cx, |view, _| view.active.clone()),
            Some(alpha_path)
        );
        let beta = matched[0].clone();
        let inserted = cx
            .update_window(handle.into(), |_, window, cx| {
                view.update(cx, |view, cx| view.insert_finder_link(&beta, window, cx))
            })
            .unwrap();
        assert!(inserted);
        let text = alpha.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        assert!(
            text.contains("[Beta Note](/blog/beta/)"),
            "link uses the id fallback: {text:?}"
        );

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_finder_entry(beta.index, &beta.source, window, cx)
            })
        })
        .unwrap();
        let beta_path = dir.path().join("src/posts/beta.mdx");
        wait_for_session(cx, &view, &beta_path).await;
        assert_eq!(
            view.read_with(cx, |view, _| view.active.clone()),
            Some(beta_path)
        );
    }

    pub(super) fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/astro")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/@astrojs/mdx")).unwrap();
        std::fs::write(
            dir.path().join("node_modules/@astrojs/mdx/package.json"),
            r#"{"version":"4.3.14","peerDependencies":{"astro":"^5.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("astro.config.mjs"),"import {defineConfig} from 'astro/config'; import mdx from '@astrojs/mdx'; export default defineConfig({integrations:[mdx()]});").unwrap();
        std::fs::create_dir_all(dir.path().join("src/posts")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"astro":"5.18.2"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("node_modules/astro/package.json"),
            r#"{"version":"5.18.2"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("src/content.config.ts"),"import {defineCollection} from 'astro:content'; import {glob} from 'astro/loaders'; const blog=defineCollection({loader:glob({base:'src/posts',pattern:'**/*.{md,mdx}'})}); export const collections={blog};").unwrap();
        std::fs::write(dir.path().join("src/posts/a.md"), "# A\n").unwrap();
        std::fs::write(
            dir.path().join("src/posts/b.mdx"),
            "import X from './X';\n<X>{42}</X>\n",
        )
        .unwrap();
        dir
    }
    #[test]
    fn frontmatter_tools_are_limited_to_markdown_sources() {
        for supported in ["entry.md", "entry.mdx", "nested/ENTRY.md"] {
            assert!(
                supports_frontmatter(std::path::Path::new(supported)),
                "{supported}"
            );
        }
        for rejected in ["entry.yaml", "entry.yml", "entry.json", "entry"] {
            assert!(
                !supports_frontmatter(std::path::Path::new(rejected)),
                "{rejected}"
            );
        }
    }

    struct TestHost {
        navigation: Entity<ProjectWorkspace>,
        document: Entity<WorkspacePanel>,
    }
    impl Render for TestHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .h_flex()
                .items_stretch()
                .size_full()
                .child(div().w_64().h_full().child(self.navigation.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.document.clone()),
                )
        }
    }
    pub(super) fn mount(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<ProjectWorkspace>) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::runtime::init(cx).unwrap();
            let mut project = None;
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let document =
                    cx.new(|cx| WorkspacePanel::new("document-panel", "Document", "", window, cx));
                let navigation = cx.new(|cx| ProjectWorkspace::new(document.clone(), window, cx));
                // Tests must never write to the real user recovery directory.
                navigation.update(cx, |project, _| project.recovery = None);
                project = Some(navigation.clone());
                cx.new(|_| TestHost {
                    navigation,
                    document,
                })
            })
            .unwrap();
            (handle.downcast::<Root>().unwrap(), project.unwrap())
        })
    }
    #[gpui_kit::test]
    fn availability_includes_view_state_and_configured_ide(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx);
        view.update(cx, |project, cx| {
            project.set_preferences(
                GlobalPreferencesV2 {
                    ide: astrolith_core::preferences::IdeCommand {
                        executable: "code".into(),
                        arguments: vec!["{project}".into()],
                    },
                    ..GlobalPreferencesV2::default()
                },
                None,
                cx,
            );
            project.set_view_state(false, true, true, true);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            let availability = view.read(cx).command_availability(cx, window);
            assert!(availability.ide_configured);
            assert!(!availability.sidebar_visible);
            assert!(availability.inspector_visible);
            assert!(availability.preview_visible);
            assert!(availability.fullscreen);
            assert!(!CommandId::OpenProjectInIde.is_available(availability));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn reload_collections_preserves_selection_tabs_recovery_and_preview(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(1, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;

        let before = view.read_with(cx, |view, cx| {
            (
                view.active.clone(),
                view.open_tabs()
                    .iter()
                    .map(|tab| tab.path.clone())
                    .collect::<Vec<_>>(),
                view.sessions.keys().cloned().collect::<Vec<_>>(),
                view.list.read(cx).selected_index(),
                view.preview_context()
                    .map(|(project, root, mapping)| (project.as_str().to_owned(), root, mapping)),
                view.active_entry_id(),
            )
        });

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.reload_collections(window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| {
            !view.refreshing && view.status.starts_with("Collections reloaded")
        })
        .await;

        let after = view.read_with(cx, |view, cx| {
            (
                view.active.clone(),
                view.open_tabs()
                    .iter()
                    .map(|tab| tab.path.clone())
                    .collect::<Vec<_>>(),
                view.sessions.keys().cloned().collect::<Vec<_>>(),
                view.list.read(cx).selected_index(),
                view.preview_context()
                    .map(|(project, root, mapping)| (project.as_str().to_owned(), root, mapping)),
                view.active_entry_id(),
            )
        });
        assert_eq!(before, after);
        assert_eq!(
            view.read_with(cx, |view, _| view.entries.len()),
            2,
            "the rescan publishes the same index"
        );
        assert!(view.read_with(cx, |view, _| view.status.contains("2 entries")));
    }

    #[gpui_kit::test]
    async fn discovered_project_opens_edits_and_retains_sessions(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        assert_eq!(view.read_with(cx, |view, _| view.entries.len()), 2);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let first = view.read_with(cx, |view, _| view.active.clone().unwrap());
        cx.update_window(handle.into(), |_, window, cx| {
            let editor = view
                .read(cx)
                .sessions
                .get(&first)
                .unwrap()
                .read(cx)
                .editor
                .clone();
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("# Changed\n", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let mut undo_count = 0;
        while view.read_with(cx, |view, cx| {
            view.sessions
                .get(&first)
                .unwrap()
                .read(cx)
                .buffer
                .is_dirty()
        }) {
            assert!(undo_count < 8, "undo must reach the loaded source baseline");
            cx.update_window(handle.into(), |_, window, cx| {
                window.press(
                    if cfg!(target_os = "macos") {
                        "cmd-z"
                    } else {
                        "ctrl-z"
                    },
                    cx,
                )
            })
            .unwrap();
            cx.run_until_parked();
            undo_count += 1;
        }
        for _ in 0..undo_count {
            cx.update_window(handle.into(), |_, window, cx| {
                window.press(
                    if cfg!(target_os = "macos") {
                        "cmd-shift-z"
                    } else {
                        "ctrl-y"
                    },
                    cx,
                )
            })
            .unwrap();
            cx.run_until_parked();
        }
        assert!(view.read_with(cx, |view, cx| {
            view.sessions.get(&first).unwrap().read(cx).is_dirty()
        }));
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| assert!(!view.guard_window_close(window, cx)));
            assert!(window.has_active_dialog(cx));
            window.press("escape", cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(1, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&first, window, cx))
        })
        .unwrap();
        // Switching away flushed the dirty buffer through the guarded save.
        for _ in 0..300 {
            cx.run_until_parked();
            if std::fs::read_to_string(&first).ok().as_deref() == Some("# Changed\n") {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "# Changed\n");
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&first).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), "# Changed\n");
            assert!(
                !session.is_dirty(),
                "a file switch saves the outgoing buffer"
            );
            assert_eq!(
                view.save_coordinator.status(&first),
                Some(SaveStatus::Saved)
            );
        });
        let next = fixture();
        let generation = view.read_with(cx, |view, _| view.generation);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(next.path().into(), window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.generation) != generation {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_ne!(
            view.read_with(cx, |view, _| view.generation),
            generation,
            "a project switch flushes and then switches"
        );
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "# Changed\n");
        cx.condition(&view, |view, _| !view.loading).await;
        assert_eq!(
            view.read_with(cx, |view, _| view.sessions.len()),
            0,
            "the previous project's sessions close with it"
        );
    }
    #[gpui_kit::test]
    async fn project_picker_cancel_and_watcher_keep_open_buffers(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("open-project", cx);
        })
        .unwrap();
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(options.directories);
            assert!(!options.files);
            Some(vec![dir.path().to_owned()])
        });
        cx.condition(&view, |view, _| !view.loading).await;
        assert_eq!(view.read_with(cx, |view, _| view.entries.len()), 2);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let original = view.read_with(cx, |view, _| view.active.clone().unwrap());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(1, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let other = view.read_with(cx, |view, _| view.active.clone().unwrap());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&original, window, cx))
        })
        .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.prompt(window, cx))
        })
        .unwrap();
        cx.simulate_path_prompt_response(|_| None);
        cx.condition(&view, |view, _| !view.loading).await;
        assert_eq!(
            view.read_with(cx, |view, _| view.active.clone()),
            Some(original.clone())
        );
        std::fs::write(
            dir.path().join("src/posts/a.md"),
            "# External replacement\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/posts/c.md"), "# New\n").unwrap();
        for _ in 0..100 {
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(20))
            })
            .await
            .unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.entries.len() == 3) {
                break;
            }
        }
        view.read_with(cx, |view, cx| {
            assert_eq!(view.entries.len(), 3, "watcher must publish new entries");
            let session = view.sessions.get(&original).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), "# A\n");
            assert!(session.externally_changed);
            assert!(
                !view
                    .sessions
                    .get(&other)
                    .unwrap()
                    .read(cx)
                    .externally_changed,
                "unmodified B must not be marked changed"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.activate_session(&other, window, cx);
                view.activate_session(&original, window, cx);
                assert!(view.status.contains("changed on disk"));
            })
        })
        .unwrap();
        std::fs::write(dir.path().join("src/posts/a.md"), "---\nslug: [\n---\n").unwrap();
        for _ in 0..100 {
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(20))
            })
            .await
            .unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.index_error.is_some()) {
                break;
            }
        }
        view.read_with(cx, |view, cx| {
            assert!(view.index_error.is_some());
            assert!(
                view.sessions
                    .get(&original)
                    .unwrap()
                    .read(cx)
                    .externally_changed
            );
            assert!(
                !view
                    .sessions
                    .get(&other)
                    .unwrap()
                    .read(cx)
                    .externally_changed
            );
        });
        std::fs::write(dir.path().join("src/posts/a.md"), "# Recovered source\n").unwrap();
        for _ in 0..100 {
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(20))
            })
            .await
            .unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.index_error.is_none() && view.entries.len() == 3
            }) {
                break;
            }
        }
        assert!(view.read_with(cx, |view, _| view.index_error.is_none()
            && view.entries.len() == 3));
        // Unsupported reconfiguration withdraws the authoritative index but preserves editors.
        std::fs::write(
            dir.path().join("src/content.config.ts"),
            "export const collections = buildDynamically();",
        )
        .unwrap();
        for _ in 0..100 {
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(20))
            })
            .await
            .unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.entries.is_empty()) {
                break;
            }
        }
        view.read_with(cx, |view, cx| {
            assert!(view.entries.is_empty());
            assert!(view.status.contains("unavailable"));
            assert_eq!(
                view.sessions
                    .get(&original)
                    .unwrap()
                    .read(cx)
                    .editor
                    .read(cx)
                    .value(),
                "# A\n"
            );
        });
    }
    #[gpui_kit::test]
    #[ignore = "actual installed Astro project; invoked by mise run test:fixtures"]
    async fn installed_astro_fixture_editing_workflow(cx: &mut TestAppContext) {
        let root = PathBuf::from(
            std::env::var_os("ASTROLITH_FIXTURE_PROJECT")
                .expect("fixture harness supplies installed project"),
        );
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.prompt(window, cx))
        })
        .unwrap();
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|_| Some(vec![root.clone()]));
        cx.condition(&view, |view, _| !view.loading).await;
        let expected_entries = std::env::var("ASTROLITH_FIXTURE_EXPECTED_ENTRIES")
            .expect("fixture harness supplies the expected entry count")
            .parse::<usize>()
            .expect("fixture harness supplies a numeric entry count");
        let (markdown, mdx) = view.read_with(cx, |view, _| {
            assert_eq!(
                view.entries.len(),
                expected_entries,
                "real installed fixture must remain fully indexed"
            );
            (
                view.entries
                    .iter()
                    .position(|entry| entry.entry().source().as_str().ends_with("introduction.md"))
                    .unwrap(),
                view.entries
                    .iter()
                    .position(|entry| entry.entry().source().as_str().ends_with(".mdx"))
                    .unwrap(),
            )
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(markdown, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let first = view.read_with(cx, |view, _| view.active.clone().unwrap());
        let disk = std::fs::read_to_string(&first).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            let editor = view
                .read(cx)
                .sessions
                .get(&first)
                .unwrap()
                .read(cx)
                .editor
                .clone();
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("# Fixture edit\n", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(mdx, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        view.read_with(cx, |view, cx| {
            let session = view
                .sessions
                .get(view.active.as_ref().unwrap())
                .unwrap()
                .read(cx);
            let visible = session.editor.read(cx).value();
            let complete = session.buffer.current_text();
            assert!(
                visible.starts_with("# 日本語のMDX"),
                "the writer starts at the body, not the hidden prelude: {visible}"
            );
            assert!(
                !visible.contains("export const label"),
                "the MDX prelude is hidden from the writer"
            );
            assert!(
                complete.contains("export const label"),
                "the complete source retains the prelude"
            );
            assert!(complete.contains("<p>{label}</p>"));
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&first, window, cx))
        })
        .unwrap();
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&first).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), "# Fixture edit\n");
            assert!(session.is_dirty());
        });
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            disk,
            "fixture editing must not write source"
        );
    }
    pub(super) async fn edit_active(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        text: &str,
    ) {
        let handle = handle.into();
        cx.update_window(handle, |_, window, cx| {
            let path = view.read(cx).active.clone().unwrap();
            let editor = view
                .read(cx)
                .sessions
                .get(&path)
                .unwrap()
                .read(cx)
                .editor
                .clone();
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input(text, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    /// Mount, open the fixture's first entry and return its path.
    pub(super) async fn open_fixture_entry(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        dir: &tempfile::TempDir,
    ) -> PathBuf {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(view, |view, _| !view.loading).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(view, |view, _| view.loads.is_empty()).await;
        view.read_with(cx, |view, _| view.active.clone().unwrap())
    }

    pub(super) async fn wait_for(
        cx: &mut TestAppContext,
        mut ready: impl FnMut(&mut TestAppContext) -> bool,
    ) {
        for _ in 0..600 {
            // Advance virtual time and pump the foreground executor without
            // taking one of the application's two blocking permits: a test
            // that sleeps on that lane can starve the work it is waiting on.
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(10));
            cx.run_until_parked();
            if ready(cx) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("condition did not become true");
    }

    #[gpui_kit::test]
    async fn yaml_data_entries_open_without_the_frontmatter_form(cx: &mut TestAppContext) {
        use astrolith_astro::schema::{CollectionSchema, Field, FieldKind, SchemaDialect};

        let dir = fixture();
        std::fs::write(
            dir.path().join("src/content.config.ts"),
            "import {defineCollection} from 'astro:content'; import {glob} from 'astro/loaders'; const blog=defineCollection({loader:glob({base:'src/posts',pattern:'**/*.{md,mdx,yaml}'})}); export const collections={blog};",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/posts/data.yaml"), "title: Data\n").unwrap();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        view.update(cx, |view, cx| {
            view.set_collection_schemas(
                vec![(
                    "blog".to_owned(),
                    CollectionSchema {
                        dialect: SchemaDialect::Draft202012,
                        fields: vec![Field {
                            name: "title".to_owned(),
                            required: true,
                            default: None,
                            kind: FieldKind::String { format: None },
                            description: None,
                            constraints: astrolith_astro::schema::FieldConstraints::default(),
                            order: 0,
                        }],
                        diagnostics: Vec::new(),
                    },
                )],
                cx,
            );
        });

        let yaml = dir.path().join("src/posts/data.yaml");
        let index = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == "src/posts/data.yaml")
                .expect("the YAML entry must be indexed")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(index, window, cx))
        })
        .unwrap();
        for _ in 0..200 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.sessions.contains_key(&yaml)) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(
            view.read_with(cx, |view, _| view.sessions.contains_key(&yaml)),
            "the YAML source must load as a raw-text document"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.active_frontmatter_key()),
            None,
            "YAML data entries must not expose the frontmatter form"
        );
        assert!(
            view.read_with(cx, |view, cx| view.active_frontmatter(cx))
                .is_none(),
            "YAML data entries must not be parsed as Markdown frontmatter"
        );
    }

    #[gpui_kit::test]
    async fn data_collection_references_feed_dropdowns_without_sidebar_entries(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        std::fs::remove_file(dir.path().join("src/content.config.ts")).unwrap();
        std::fs::write(
            dir.path().join("src/content.config.mjs"),
            r#"import {defineCollection} from 'astro:content';
import {file, glob} from 'astro/loaders';
const blog = defineCollection({loader: glob({base: './src/posts', pattern: '**/*.{md,mdx}'})});
const dataAuthors = defineCollection({loader: file('./oracles/data-collections/authors.json')});
export const collections = {blog, dataAuthors};
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("oracles/data-collections")).unwrap();
        std::fs::write(
            dir.path().join("oracles/data-collections/authors.json"),
            r#"[{"id":"ada","name":"Ada Lovelace"},{"id":"grace","name":"Grace Hopper"},{"id":"anon"}]"#,
        )
        .unwrap();

        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;

        view.read_with(cx, |view, _| {
            let manifest = view.manifest.as_ref().expect("project must be discovered");
            assert_eq!(
                manifest.data_collections().len(),
                1,
                "the file() loader must be discovered: {:?}",
                manifest.diagnostics()
            );
        });
        for _ in 0..200 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| !view.data_references.is_empty()) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }

        let expected = [
            ReferenceOption {
                value: "ada".to_owned(),
                label: "Ada Lovelace".to_owned(),
            },
            ReferenceOption {
                value: "grace".to_owned(),
                label: "Grace Hopper".to_owned(),
            },
            ReferenceOption {
                value: "anon".to_owned(),
                label: "anon".to_owned(),
            },
        ];
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.data_references.get("dataAuthors").map(Vec::as_slice),
                Some(expected.as_slice()),
                "JSON file() entries label the reference dropdown by name, else id"
            );
            assert_eq!(
                view.references.get("dataAuthors").map(Vec::as_slice),
                Some(expected.as_slice()),
                "the form's reference catalog includes data-collection entries"
            );
            assert!(
                view.entries
                    .iter()
                    .all(|entry| entry.entry().id().collection().as_str() != "dataAuthors"),
                "data collections are read-only and never editable sidebar entries"
            );
            assert_eq!(view.data_references.len(), 1);
        });
    }

    #[gpui_kit::test]
    async fn reference_catalog_updates_reach_an_open_form(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let path = dir.path().join("src/posts/a.md");
        let index = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == "src/posts/a.md")
                .expect("the markdown fixture must be indexed")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(index, window, cx))
        })
        .unwrap();
        wait_for_session(cx, &view, &path).await;

        let before = view
            .read_with(cx, |view, _| view.active_frontmatter_key())
            .expect("an open markdown entry exposes a form key");

        let options = vec![ReferenceOption {
            value: "ada".to_owned(),
            label: "Ada Lovelace".to_owned(),
        }];
        view.update(cx, |view, cx| {
            view.data_references
                .insert("authors".to_owned(), options.clone());
            view.rebuild_references();
            cx.notify();
        });
        let loaded = view
            .read_with(cx, |view, _| view.active_frontmatter_key())
            .expect("the form key survives a reference update");
        assert_ne!(
            before, loaded,
            "new reference options must rebuild an already-open form"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.references.get("authors").cloned()),
            Some(options),
            "the data collection feeds the reference dropdown"
        );

        // A project whose data collections were removed must drop their options
        // instead of keeping the previous catalog forever.
        view.update(cx, |view, cx| view.load_data_references(cx));
        let cleared = view
            .read_with(cx, |view, _| view.active_frontmatter_key())
            .expect("the form key survives the cleared catalog");
        assert_ne!(
            loaded, cleared,
            "a removed data collection must refresh the form"
        );
        assert!(!view.read_with(cx, |view, _| view.references.contains_key("authors")));
        assert!(view.read_with(cx, |view, _| view.data_references.is_empty()));
    }

    #[gpui_kit::test]
    async fn frontmatter_form_field_edit_preserves_concurrent_body_edit(cx: &mut TestAppContext) {
        use astrolith_astro::schema::{CollectionSchema, Field, FieldKind, SchemaDialect};

        let dir = fixture();
        let with_frontmatter = "---\ntitle: Old\ndraft: false\n---\n\n# Body 中文\n";
        std::fs::write(dir.path().join("src/posts/c.md"), with_frontmatter).unwrap();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        // Schemas are per project, so inject after the project has opened (the
        // real shell installs them from the generation task in the same spot).
        view.update(cx, |view, cx| {
            view.set_collection_schemas(
                vec![(
                    "blog".to_owned(),
                    CollectionSchema {
                        dialect: SchemaDialect::Draft202012,
                        fields: vec![
                            Field {
                                name: "title".to_owned(),
                                required: true,
                                default: None,
                                kind: FieldKind::String { format: None },
                                description: None,
                                constraints: astrolith_astro::schema::FieldConstraints::default(),
                                order: 0,
                            },
                            Field {
                                name: "draft".to_owned(),
                                required: false,
                                default: None,
                                kind: FieldKind::Boolean,
                                description: None,
                                constraints: astrolith_astro::schema::FieldConstraints::default(),
                                order: 1,
                            },
                        ],
                        diagnostics: Vec::new(),
                    },
                )],
                cx,
            );
        });

        let path = dir.path().join("src/posts/c.md");
        let index = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == "src/posts/c.md")
                .expect("the frontmatter fixture must be indexed")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(index, window, cx))
        })
        .unwrap();
        for _ in 0..200 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.sessions.contains_key(&path)) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }

        // The active document resolves its schema and current text for the form.
        let ActiveFrontmatter {
            schema, text, root, ..
        } = view
            .read_with(cx, |view, cx| view.active_frontmatter(cx))
            .expect("active document must expose its schema and text");
        let schema = schema.expect("the collection schema must resolve");
        assert!(schema.fields.iter().any(|field| field.name == "title"));
        assert_eq!(text, with_frontmatter);
        assert_eq!(root, dir.path().canonicalize().unwrap());
        let key = view
            .read_with(cx, |view, _| view.active_frontmatter_key())
            .expect("an active markdown entry exposes a form key");
        assert_eq!(key.path, path);
        assert!(
            key.has_schema,
            "the injected schema must be the form schema"
        );

        // The writer shows only the body: the frontmatter is hidden, and the
        // complete source still retains it.
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), "# Body 中文\n");
            assert_eq!(session.buffer.current_text(), with_frontmatter);
        });

        // The user edits the document BODY in the source editor (unsaved, in memory).
        let body_edited = "# Body EDITED 中文\n";
        edit_active(cx, handle, &view, body_edited).await;
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), body_edited);
            assert_eq!(
                session.buffer.current_text(),
                "---\ntitle: Old\ndraft: false\n---\n\n# Body EDITED 中文\n"
            );
        });

        // A form field edit patches ONLY that field on the live text; the
        // concurrent body edit must survive exactly.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply_frontmatter_edit(
                    FrontmatterEdit::Scalar {
                        path: "title".to_owned(),
                        value: astrolith_document::FrontmatterValue::String("Edited".to_owned()),
                        present: true,
                    },
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        let expected = "---\ntitle: Edited\ndraft: false\n---\n\n# Body EDITED 中文\n";
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(
                session.buffer.current_text(),
                expected,
                "only the title field may change; the unsaved body edit must survive"
            );
            assert_eq!(
                session.editor.read(cx).value(),
                body_edited,
                "the visible text must not move when hidden frontmatter changes"
            );
            assert!(session.buffer.is_dirty());
        });

        // The normal save coordinator persists the combined result.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if std::fs::read_to_string(&path).unwrap() == expected {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    }

    /// Build a session outside the project flow, so the writer projection can be
    /// driven directly without rendering the editor.
    pub(super) fn bare_session(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        source_path: &str,
        source: &str,
    ) -> Entity<DocumentSession> {
        cx.update_window(handle.into(), |_, window, cx| {
            let identity = DocumentIdentity::new(
                ProjectId::new("projection").unwrap(),
                1,
                SourcePath::new(source_path).unwrap(),
            );
            cx.new(|cx| DocumentSession::new(identity, source.to_owned(), window, cx))
        })
        .unwrap()
    }

    /// Replace the session's visible text the way a whole-document user edit does.
    pub(super) fn replace_visible(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        session: &Entity<DocumentSession>,
        text: &str,
    ) {
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.replace_all(text, window, cx));
            });
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    async fn projected_writer_keeps_hidden_bytes_across_typing_and_undo(cx: &mut TestAppContext) {
        let dir = fixture();
        let frontmatter = "---\ntitle: \"Exact\" # keep\ndraft: false\n---\n";
        let source = format!("{frontmatter}\n# Body 🦀\n\nSecond line\n");
        std::fs::write(dir.path().join("src/posts/c.md"), &source).unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let path = dir.path().join("src/posts/c.md");
        let index = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == "src/posts/c.md")
                .expect("the projected fixture must be indexed")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(index, window, cx))
        })
        .unwrap();
        wait_for_session(cx, &view, &path).await;

        // The writer shows the body only; the complete source keeps the frontmatter.
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(
                session.editor.read(cx).value(),
                "# Body 🦀\n\nSecond line\n"
            );
            assert_eq!(session.buffer.current_text(), source);
            assert!(!session.is_dirty());
        });

        // Type a replacement body through the real input path.
        let edited = "# Body changed 中文\n\nSecond line\n";
        edit_active(cx, handle, &view, edited).await;
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), edited);
            assert_eq!(
                session.buffer.current_text(),
                format!("{frontmatter}\n{edited}"),
                "every body edit must land after the exact hidden bytes"
            );
            assert!(session.is_dirty());
            // A recovery snapshot always carries the complete source.
            let record = RecoveryRecord::capture(&session.buffer, 0);
            assert_eq!(record.content(), format!("{frontmatter}\n{edited}"));
        });

        // Undo restores the complete source, hidden bytes included.
        let mut undos = 0;
        while view.read_with(cx, |view, cx| {
            view.sessions.get(&path).unwrap().read(cx).is_dirty()
        }) {
            assert!(undos < 8, "undo must reach the loaded baseline");
            cx.update_window(handle.into(), |_, window, cx| {
                window.press(
                    if cfg!(target_os = "macos") {
                        "cmd-z"
                    } else {
                        "ctrl-z"
                    },
                    cx,
                )
            })
            .unwrap();
            cx.run_until_parked();
            undos += 1;
        }
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(session.buffer.current_text(), source);
            assert_eq!(
                session.editor.read(cx).value(),
                "# Body 🦀\n\nSecond line\n"
            );
        });

        // Saving the edited body writes the complete source byte-for-byte.
        edit_active(cx, handle, &view, edited).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        let expected = format!("{frontmatter}\n{edited}");
        for _ in 0..300 {
            cx.run_until_parked();
            if std::fs::read_to_string(&path).unwrap() == expected {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(!session.is_dirty());
            assert_eq!(session.buffer.original_text(), expected);
        });

        // Reopening from disk rebuilds the projection around the saved bytes.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.close_active(window, cx));
            assert!(
                !window.has_active_dialog(cx),
                "a clean document closes plainly"
            );
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(index, window, cx))
        })
        .unwrap();
        wait_for_session(cx, &view, &path).await;
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), edited);
            assert_eq!(session.buffer.current_text(), expected);
            assert!(!session.is_dirty());
        });
    }

    #[gpui_kit::test]
    async fn malformed_frontmatter_stays_visible_and_seeds_the_notice(cx: &mut TestAppContext) {
        // A collection entry with an unclosed opener cannot be indexed, so the
        // session is driven directly; the notice must be seeded by `build` so the
        // workspace's `SessionChanged` subscription can publish it on the first
        // change.
        let source = "---\ntitle: Unterminated\n\n# Body\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/entry.md", source);
        session.read_with(cx, |session, cx| {
            assert_eq!(
                session.editor.read(cx).value(),
                source,
                "undelimitable metadata must stay visible"
            );
            assert_eq!(
                session.projection,
                astrolith_document::WriterProjection::complete(source),
                "nothing may be hidden when the block cannot be delimited"
            );
            let notice = session
                .notice
                .as_ref()
                .expect("the session must record why the source is shown whole");
            assert!(
                notice.contains("Writer metadata stays visible"),
                "{notice:?}"
            );
        });

        // The first delivered change keeps the notice available for publication.
        replace_visible(cx, handle, &session, "# Body edited\n");
        session.read_with(cx, |session, _| {
            assert!(
                session
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.contains("Writer metadata stays visible")),
                "absorbing a change must not swallow the notice"
            );
            assert_eq!(session.buffer.current_text(), "# Body edited\n");
        });
    }

    #[gpui_kit::test]
    async fn complete_source_history_preserves_crlf_and_unicode_bytes(cx: &mut TestAppContext) {
        let source = "---\r\ntitle: Old\r\n---\r\n\r\n# 标题 🦀\r\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/crlf.md", source);
        replace_visible(cx, handle, &session, "# 标题 changed 🦀\r\n");
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .patch_complete(window, cx, |live| {
                        let mut document = parse_document(live).map_err(|e| e.to_string())?;
                        document
                            .set_field(
                                "title",
                                astrolith_document::FrontmatterValue::String("New".to_owned()),
                            )
                            .map_err(|e| e.to_string())?;
                        document.serialize().map_err(|e| e.to_string())
                    })
                    .expect("CRLF frontmatter patch applies");
            });
        })
        .unwrap();
        let edited = "---\r\ntitle: New\r\n---\r\n\r\n# 标题 changed 🦀\r\n";
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), edited);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(session.update(cx, |session, cx| session.undo_source(window, cx)));
            assert!(session.update(cx, |session, cx| session.redo_source(window, cx)));
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), edited);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(session.update(cx, |session, cx| session.undo_source(window, cx)));
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.buffer.current_text(),
                "---\r\ntitle: Old\r\n---\r\n\r\n# 标题 changed 🦀\r\n",
                "undoing the metadata patch must not rewrite any other byte"
            );
        });
    }

    #[test]
    fn project_astro_source_scan_is_sorted_and_skips_dependencies() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src/pages")).unwrap();
        std::fs::create_dir_all(root.path().join("src/layouts")).unwrap();
        std::fs::create_dir_all(root.path().join("node_modules/pkg")).unwrap();
        std::fs::write(root.path().join("src/pages/index.astro"), "").unwrap();
        std::fs::write(root.path().join("src/layouts/Base.astro"), "").unwrap();
        std::fs::write(root.path().join("src/pages/readme.md"), "").unwrap();
        std::fs::write(root.path().join("node_modules/pkg/Bad.astro"), "").unwrap();
        let found = scan_project_astro_sources(root.path()).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|(_, relative)| relative.as_str())
                .collect::<Vec<_>>(),
            ["src/layouts/Base.astro", "src/pages/index.astro"]
        );
    }

    #[gpui_kit::test]
    async fn build_swaps_the_library_for_a_project_file_explorer(cx: &mut TestAppContext) {
        let dir = fixture();
        let astro = dir.path().join("index.astro");
        std::fs::write(&astro, "---\n---\n<h1>Home</h1>\n").unwrap();
        let notes = dir.path().join("notes.astro");
        std::fs::write(&notes, "---\n---\n<p>Notes</p>\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src/pages")).unwrap();
        std::fs::write(
            dir.path().join("src/pages/about.astro"),
            "---\n---\n<h1>About</h1>\n",
        )
        .unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        // Writing keeps the collection library the writer already knows.
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("project-collections").is_some());
            assert!(window.try_find("file-browser").is_none());
        })
        .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.set_source_view(true, window, cx));
            window.render_frame(cx);
            assert!(window.try_find("file-browser").is_some());
            assert!(window.try_find("project-collections").is_none());
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if !view.read_with(cx, |view, cx| view.file_browser.read(cx).is_scanning()) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        let ids = view.read_with(cx, |view, cx| view.file_browser.read(cx).visible_ids(cx));
        assert!(ids.contains(&"src".to_owned()), "{ids:?}");
        assert!(ids.contains(&"index.astro".to_owned()), "{ids:?}");
        assert!(ids.contains(&"package.json".to_owned()), "{ids:?}");
        assert!(
            !ids.iter().any(|id| id.starts_with("node_modules")),
            "{ids:?}"
        );

        // Clicking a directory expands exactly that subtree.
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("src", cx);
        })
        .unwrap();
        let ids = view.read_with(cx, |view, cx| view.file_browser.read(cx).visible_ids(cx));
        assert!(ids.contains(&"src/pages".to_owned()), "{ids:?}");
        assert!(
            !ids.contains(&"src/pages/about.astro".to_owned()),
            "{ids:?}"
        );

        // Clicking a tree row opens the project .astro source in a real session.
        let canonical = std::fs::canonicalize(&astro).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("index.astro", cx);
        })
        .unwrap();
        wait_for_session(cx, &view, &canonical).await;

        // The list keeps a keyboard path: Enter confirms the selected file.
        let notes_canonical = std::fs::canonicalize(&notes).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.file_browser.update(cx, |browser, cx| {
                    browser.select_row("notes.astro", window, cx)
                });
            });
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
        wait_for_session(cx, &view, &notes_canonical).await;

        // A content entry opens through the existing entry path.
        let entry = std::fs::canonicalize(dir.path().join("src/posts/a.md")).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_browser_path(entry.clone(), window, cx)
            });
        })
        .unwrap();
        wait_for_session(cx, &view, &entry).await;

        // Configuration scripts are editable text sources too.
        let config = std::fs::canonicalize(dir.path().join("astro.config.mjs")).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_browser_path(config.clone(), window, cx)
            });
        })
        .unwrap();
        wait_for_session(cx, &view, &config).await;

        // Non-text files are requested as a readonly snapshot from the shell.
        let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = seen.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &ReadonlyFileRequested, _| {
                recorded.borrow_mut().push(event.path.clone());
            })
        });
        let binary = dir.path().join("public/asset.bin");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
        let binary = std::fs::canonicalize(&binary).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_browser_path(binary.clone(), window, cx)
            });
        })
        .unwrap();
        assert_eq!(*seen.borrow(), vec![binary]);

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.set_source_view(false, window, cx));
            window.render_frame(cx);
            assert!(window.try_find("entry-new").is_some());
            assert!(window.try_find("file-browser").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn mode_switches_retain_both_models_and_sync_only_real_edits(cx: &mut TestAppContext) {
        let source = "---\ntitle: Keep\n---\n# Body\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/cached.md", source);
        let writer = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| session.set_source_view(true, window, cx))
        })
        .unwrap();
        let build = session.read_with(cx, |session, _| session.editor.clone());
        assert_ne!(
            writer.entity_id(),
            build.entity_id(),
            "source and projection retain separate display state"
        );
        let writer_snapshot = writer.read_with(cx, |editor, _| editor.text().clone());
        let build_snapshot = build.read_with(cx, |editor, _| editor.text().clone());
        for _ in 0..3 {
            cx.update_window(handle.into(), |_, window, cx| {
                session.update(cx, |session, cx| session.set_source_view(false, window, cx));
                assert_eq!(session.read(cx).editor.entity_id(), writer.entity_id());
                session.update(cx, |session, cx| session.set_source_view(true, window, cx));
                assert_eq!(session.read(cx).editor.entity_id(), build.entity_id());
            })
            .unwrap();
        }
        assert_eq!(
            writer.read_with(cx, |editor, _| editor.text().clone()),
            writer_snapshot
        );
        assert_eq!(
            build.read_with(cx, |editor, _| editor.text().clone()),
            build_snapshot
        );
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| session.set_source_view(false, window, cx))
        })
        .unwrap();
        replace_visible(cx, handle, &session, "# Body edited\n");
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| session.set_source_view(true, window, cx))
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert_eq!(session.editor.entity_id(), build.entity_id());
            assert_eq!(
                session.editor.read(cx).value(),
                "---\ntitle: Keep\n---\n# Body edited\n"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                assert!(session.undo_source(window, cx));
            });
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), source)
        });
    }

    #[gpui_kit::test]
    async fn source_view_edits_the_same_buffer_and_maps_back_to_the_writer(
        cx: &mut TestAppContext,
    ) {
        let source = "---\ntitle: Old\n---\n\n# Body\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/source.md", source);
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| session.set_source_view(true, window, cx));
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert_eq!(session.editor.read(cx).value(), source);
            assert_eq!(session.projection.visible, "# Body\n");
            assert_eq!(
                session.editor.read(cx).language_name(),
                "markdown",
                "Build highlights the complete source"
            );
        });
        let edited = "---\ntitle: New\n---\n\n# Body edited\n";
        replace_visible(cx, handle, &session, edited);
        session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), edited);
            assert_eq!(session.projection.visible, "# Body edited\n");
            assert_eq!(session.editor.read(cx).value(), edited);
            assert!(session.is_dirty());
        });
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(session.update(cx, |session, cx| session.undo_source(window, cx)));
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), source);
            assert_eq!(session.editor.read(cx).value(), source);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| session.set_source_view(false, window, cx));
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert_eq!(session.editor.read(cx).value(), "# Body\n");
            assert_eq!(session.buffer.current_text(), source);
            assert_eq!(
                session.editor.read(cx).language_name(),
                "text",
                "the writer paints prose decorations, not code syntax"
            );
        });
    }

    #[gpui_kit::test]
    async fn complete_source_history_interleaves_body_and_frontmatter_transactions(
        cx: &mut TestAppContext,
    ) {
        let source = "---\ntitle: Old\n---\n\n# Body\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/history.md", source);
        replace_visible(cx, handle, &session, "# Body edited\n");
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .patch_complete(window, cx, |live| {
                        let mut document = parse_document(live).map_err(|e| e.to_string())?;
                        document
                            .set_field(
                                "title",
                                astrolith_document::FrontmatterValue::String("New".to_owned()),
                            )
                            .map_err(|e| e.to_string())?;
                        document.serialize().map_err(|e| e.to_string())
                    })
                    .expect("the frontmatter patch applies over the body edit");
            });
        })
        .unwrap();
        let edited = "---\ntitle: New\n---\n\n# Body edited\n";
        session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), edited);
            assert_eq!(session.projection.visible, "# Body edited\n");
            assert_eq!(session.editor.read(cx).value(), "# Body edited\n");
            assert!(session.is_dirty());
        });

        // Undo the hidden frontmatter change only: body bytes stay untouched.
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(session.update(cx, |session, cx| session.undo_source(window, cx)));
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert_eq!(
                session.buffer.current_text(),
                "---\ntitle: Old\n---\n\n# Body edited\n"
            );
            assert_eq!(session.projection.visible, "# Body edited\n");
            assert_eq!(session.editor.read(cx).value(), "# Body edited\n");
        });

        // A second undo removes the body edit; redo replays in the same order.
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(session.update(cx, |session, cx| session.undo_source(window, cx)));
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), source);
            assert_eq!(session.projection.visible, "# Body\n");
            assert_eq!(session.editor.read(cx).value(), "# Body\n");
        });
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(session.update(cx, |session, cx| session.redo_source(window, cx)));
            assert!(session.update(cx, |session, cx| session.redo_source(window, cx)));
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), edited);
            assert_eq!(session.editor.read(cx).value(), "# Body edited\n");
        });
    }

    #[gpui_kit::test]
    async fn frontmatter_patch_rebuilds_projection_and_keeps_the_caret(cx: &mut TestAppContext) {
        let source = "# Title\n\nBody text\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/entry.md", source);
        let caret = source.find("Body").unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(caret..caret, cx));
                session
                    .patch_complete(window, cx, |live| {
                        let mut document = parse_document(live).map_err(|e| e.to_string())?;
                        document
                            .insert_field(
                                "title",
                                astrolith_document::FrontmatterValue::String("New".to_owned()),
                            )
                            .map_err(|e| e.to_string())?;
                        document.serialize().map_err(|e| e.to_string())
                    })
                    .expect("the patch must apply to the complete source");
            });
        })
        .unwrap();
        cx.run_until_parked();
        let expected = "---\ntitle: New\n---\n\n# Title\n\nBody text\n";
        let revision = session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), expected);
            assert_eq!(session.projection.visible, "# Title\n\nBody text\n");
            assert_eq!(session.editor.read(cx).value(), "# Title\n\nBody text\n");
            assert_eq!(
                session.editor.read(cx).cursor(),
                session.projection.visible.find("Body").unwrap(),
                "the caret must stay on the same body text"
            );
            session.buffer.revision()
        });
        // The session's own rewrite is not a user edit: no second revision.
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.revision(), revision);
        });
    }

    #[gpui_kit::test]
    async fn frontmatter_patch_that_hides_a_separator_rebuilds_the_visible_text(
        cx: &mut TestAppContext,
    ) {
        // No frontmatter yet, so the leading blank line is visible content; the
        // patch creates the frontmatter and turns that line into a hidden
        // separator, which forces a real editor rewrite.
        let source = "\n# Title\n\nBody\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/entry.md", source);
        let before = session.update(cx, |session, cx| {
            assert_eq!(session.editor.read(cx).value(), source);
            let caret = source.find("Body").unwrap();
            session
                .editor
                .update(cx, |editor, cx| editor.set_selected_range(caret..caret, cx));
            session.buffer.revision()
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .patch_complete(window, cx, |live| {
                        let mut document = parse_document(live).map_err(|e| e.to_string())?;
                        document
                            .insert_field(
                                "title",
                                astrolith_document::FrontmatterValue::String("New".to_owned()),
                            )
                            .map_err(|e| e.to_string())?;
                        document.serialize().map_err(|e| e.to_string())
                    })
                    .expect("the patch must apply to the complete source");
            });
        })
        .unwrap();
        cx.run_until_parked();
        let revision = session.read_with(cx, |session, cx| {
            assert_eq!(session.projection.visible, "# Title\n\nBody\n");
            assert_eq!(session.editor.read(cx).value(), "# Title\n\nBody\n");
            assert_eq!(
                session.editor.read(cx).cursor(),
                session.projection.visible.find("Body").unwrap(),
                "the caret must stay on the same body text"
            );
            assert!(
                session
                    .buffer
                    .current_text()
                    .starts_with("---\ntitle: New\n---\n"),
                "the patch must add the frontmatter: {:?}",
                session.buffer.current_text()
            );
            assert!(session.buffer.current_text().ends_with("# Title\n\nBody\n"));
            assert_eq!(
                session.buffer.revision(),
                before.checked_next().unwrap(),
                "the rewrite must not be absorbed as a second edit"
            );
            assert!(session.is_dirty(), "the patch itself is unsaved");
            session.buffer.revision()
        });
        // Only the patch's revision can be marked saved: no second absorb exists.
        let patched = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        let marked = session.update(cx, |session, _| session.confirm_saved(revision, &patched));
        assert!(marked);
        session.read_with(cx, |session, cx| {
            assert!(!session.is_dirty());
            assert_eq!(session.editor.read(cx).value(), "# Title\n\nBody\n");
        });

        // A following keystroke lands in the body, after the hidden frontmatter.
        replace_visible(cx, handle, &session, "# Title\n\nBody!\n");
        session.read_with(cx, |session, cx| {
            let complete = session.buffer.current_text();
            assert!(
                complete.starts_with("---\ntitle: New\n---\n"),
                "{complete:?}"
            );
            assert!(complete.ends_with("# Title\n\nBody!\n"), "{complete:?}");
            assert_eq!(session.editor.read(cx).value(), "# Title\n\nBody!\n");
            assert!(session.is_dirty(), "the keystroke must reach the source");
        });
    }

    #[gpui_kit::test]
    async fn rejected_projection_edit_keeps_the_source_and_reports_a_notice(
        cx: &mut TestAppContext,
    ) {
        let source = "abcHHdef";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/entry.md", source);
        // A synthetic interleaved layout cannot come from `project_writer`, but a
        // session that is handed one must refuse instead of guessing.
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.editor.update(cx, |editor, cx| {
                    editor.replace_all("abcdef", window, cx);
                });
                session.buffer.apply_text(source.to_owned()).unwrap();
                session.projection = astrolith_document::WriterProjection {
                    visible: "abcdef".to_owned(),
                    segments: vec![
                        astrolith_document::ProjectionSegment::Visible {
                            source: 0..3,
                            visible: 0..3,
                        },
                        astrolith_document::ProjectionSegment::Hidden { source: 3..5 },
                        astrolith_document::ProjectionSegment::Visible {
                            source: 5..8,
                            visible: 3..6,
                        },
                    ],
                };
            });
        })
        .unwrap();
        cx.run_until_parked();
        // The queued self-write matches the projection, so nothing is absorbed.
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), source);
        });

        replace_visible(cx, handle, &session, "abZf");
        session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), source, "the source is kept");
            assert!(session.notice.is_some(), "the refusal must be surfaced");
            assert_eq!(
                session.editor.read(cx).value(),
                source,
                "the refused text must be replaced by the authoritative projection"
            );
            assert!(!session.is_dirty(), "a refused edit changes nothing");
        });

        // The latch blocks the divergent text: the next keystroke is typed into
        // whatever the editor now shows, and it must land on the authoritative
        // projection rather than on the bytes that were refused.
        let shown = session.read_with(cx, |session, cx| {
            session.editor.read(cx).value().to_string()
        });
        replace_visible(cx, handle, &session, &format!("{shown}X"));
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), format!("{source}X"));
        });
    }

    #[gpui_kit::test]
    async fn projected_body_and_form_edits_survive_a_stale_save_completion(
        cx: &mut TestAppContext,
    ) {
        let frontmatter = "---\ntitle: Old\n---\n";
        let source = format!("{frontmatter}\n# Body\n");
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/entry.md", &source);

        replace_visible(cx, handle, &session, "# Body edited\n");
        let stale_revision = session.read_with(cx, |session, _| session.buffer.revision());

        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .patch_complete(window, cx, |live| {
                        let mut document = parse_document(live).map_err(|e| e.to_string())?;
                        document
                            .set_field(
                                "title",
                                astrolith_document::FrontmatterValue::String("New".to_owned()),
                            )
                            .map_err(|e| e.to_string())?;
                        document.serialize().map_err(|e| e.to_string())
                    })
                    .unwrap();
            });
        })
        .unwrap();
        cx.run_until_parked();
        let expected = "---\ntitle: New\n---\n\n# Body edited\n";
        let current = session.read_with(cx, |session, cx| {
            assert_eq!(session.buffer.current_text(), expected);
            assert_eq!(session.editor.read(cx).value(), "# Body edited\n");
            session.buffer.revision()
        });
        assert_ne!(stale_revision, current);

        // A completion for the pre-form revision is stale and drops.
        let marked = session.update(cx, |session, _| {
            session.confirm_saved(stale_revision, "---\ntitle: Old\n---\n\n# Body edited\n")
        });
        assert!(!marked, "a stale save must not mark newer edits saved");
        session.read_with(cx, |session, _| {
            assert!(session.is_dirty());
            assert_eq!(session.buffer.current_text(), expected);
        });

        // The matching completion clears it.
        let marked = session.update(cx, |session, _| session.confirm_saved(current, expected));
        assert!(marked);
        session.read_with(cx, |session, _| {
            assert!(!session.is_dirty());
            assert_eq!(session.buffer.original_text(), expected);
        });
    }

    struct WorkloadHost {
        editor: Entity<gpui_kit::component::input::EditorState>,
    }
    impl Render for WorkloadHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(gpui_kit::component::input::Editor::new(&self.editor).size_full())
        }
    }

    /// Open one projected session in a real editor view for typing measurements.
    pub(super) fn mount_workload(
        cx: &mut TestAppContext,
        source: &str,
    ) -> (WindowHandle<Root>, Entity<DocumentSession>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let mut session = None;
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let identity = DocumentIdentity::new(
                    ProjectId::new("projection").unwrap(),
                    1,
                    SourcePath::new("src/posts/entry.md").unwrap(),
                );
                let created =
                    cx.new(|cx| DocumentSession::new(identity, source.to_owned(), window, cx));
                let editor = created.read(cx).editor.clone();
                session = Some(created);
                cx.new(|_| WorkloadHost { editor })
            })
            .unwrap();
            (
                handle.downcast::<Root>().unwrap(),
                session.expect("the workload session must exist"),
            )
        })
    }

    // Diagnostic only: test-platform CPU event dispatch plus frames, not native
    // input-to-present latency. Compare each workload only with its own baseline.
    #[gpui_kit::test]
    #[ignore = "bounded projection measurements; run mise run test:projection-perf"]
    fn projected_writer_workload(cx: &mut TestAppContext) {
        use std::time::Instant;
        let frontmatter = "---\ntitle: Workload\ndraft: false\n---\n";
        for (bytes, long_line) in [
            (1024usize, false),
            (100 * 1024, false),
            (1024 * 1024, false),
            (100 * 1024, true),
        ] {
            // Same body generator as the P0 plain-editor spike, so the two
            // measurements compare identical source text.
            const MATCHED_SAMPLE: &str = "# Astrolith editor spike\n\n中文 · 日本語 · 🦀 · e\u{301}\n\n**Bold** and *emphasis*, `inline code`, [link](https://example.com).\n\n- First item\n- Second item\n\n```rust\nfn main() {}\n```\n";
            let unit = if long_line {
                "中文日本語🦀 **bold** `code` ".to_owned()
            } else {
                format!(
                    "{MATCHED_SAMPLE}{}\n\n",
                    "中文・日本語・🦀 **bold** and `inline code`. ".repeat(16)
                )
            };
            let body = unit.repeat(bytes.div_ceil(unit.len()));
            let source = format!("{frontmatter}{body}");
            let (handle, session) = mount_workload(cx, &source);
            let editor = session.read_with(cx, |session, _| session.editor.clone());
            let visible_bytes = source.len() - frontmatter.len();
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.click(("input", editor.entity_id()), cx);
                window.press("secondary-a", cx);
                window.press("right", cx);
            })
            .unwrap();
            let mut samples = Vec::with_capacity(100);
            for _ in 0..100 {
                // One keystroke per application update: the `Change` it dispatches
                // is delivered and absorbed inside the timed region, so the
                // projection work is included rather than queued past the sample.
                let start = Instant::now();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.input("中", cx);
                    window.render_frame(cx);
                })
                .unwrap();
                samples.push(start.elapsed().as_secs_f64() * 1000.);
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "PROJECTED_WRITER source_bytes={} body_bytes={visible_bytes} lines={} long_line={long_line} samples=100 p50_ms={:.3} p95_ms={:.3} max_ms={:.3}",
                source.len(),
                body.lines().count(),
                samples[49],
                samples[94],
                samples[99],
            );
            cx.update_window(handle.into(), |_, _, cx| {
                assert_eq!(
                    editor.read(cx).value().len(),
                    visible_bytes + 100 * "中".len()
                );
            })
            .unwrap();
            session.read_with(cx, |session, _| {
                assert!(
                    session.buffer.current_text().starts_with(frontmatter),
                    "hidden bytes must survive typing"
                );
                assert_eq!(
                    session.buffer.current_text().len(),
                    source.len() + 100 * "中".len()
                );
            });
        }
    }

    pub(super) fn schema(fields: Vec<astrolith_astro::schema::Field>) -> CollectionSchema {
        CollectionSchema {
            dialect: astrolith_astro::schema::SchemaDialect::Draft202012,
            fields,
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn required_field(name: &str, kind: FieldKind) -> astrolith_astro::schema::Field {
        astrolith_astro::schema::Field {
            name: name.into(),
            required: true,
            default: None,
            kind,
            description: None,
            constraints: astrolith_astro::schema::FieldConstraints::default(),
            order: 0,
        }
    }

    pub(super) fn set_mappings(
        view: &Entity<ProjectWorkspace>,
        cx: &mut TestAppContext,
        mappings: FieldMappings,
    ) {
        view.update(cx, |view, cx| {
            let project = ProjectPreferencesV1 {
                field_mappings: mappings,
                ..ProjectPreferencesV1::default()
            };
            view.set_preferences(GlobalPreferencesV2::default(), Some(project), cx);
        });
    }

    pub(super) async fn open_project_and_wait(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        root: &Path,
    ) {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(root.to_path_buf(), window, cx))
        })
        .unwrap();
        cx.condition(view, |view, _| !view.is_loading()).await;
    }

    /// Open the one entry of a fixture project and return its session.
    pub(super) async fn open_entry_for(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        relative: &str,
    ) -> (PathBuf, Entity<DocumentSession>) {
        let path = view.read_with(cx, |view, _| {
            view.manifest.as_ref().unwrap().root().join(relative)
        });
        let index = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == relative)
                .unwrap_or_else(|| panic!("{relative} must be indexed"))
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(index, window, cx))
        })
        .unwrap();
        wait_for_session(cx, view, &path).await;
        let session = view.read_with(cx, |view, _| view.sessions.get(&path).unwrap().clone());
        (path, session)
    }

    #[gpui_kit::test]
    async fn writer_formatting_undoes_in_one_step_and_keeps_hidden_bytes(cx: &mut TestAppContext) {
        let dir = fixture();
        let frontmatter = "---\ntitle: 'Writer'\nstatus: false\n---\n";
        let body = "# Body\n\nFirst sentence. Second sentence.\n";
        let source = format!("{frontmatter}\n{body}");
        std::fs::write(dir.path().join("src/posts/c.md"), &source).unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let (path, session) = open_entry_for(cx, handle, &view, "src/posts/c.md").await;
        // Focus mode is off, so the writer paints only the prose layer and the
        // footer counts come from the session's one snapshot.
        view.read_with(cx, |view, cx| {
            let counts = view
                .active_counts(cx)
                .expect("an active document has counts");
            assert_eq!(counts.words, 5);
            assert_eq!(counts.lines, 4);
            assert_eq!(
                counts.characters,
                body.chars().filter(|c| !c.is_whitespace()).count()
            );
        });

        // Bold the first word through the registry path.
        let start = session.read_with(cx, |session, _| {
            session.projection.visible.find("First").unwrap()
        });
        cx.update_window(handle.into(), |_, _, cx| {
            let editor = session.read(cx).editor.clone();
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(start..start + 5, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            session
                .update(cx, |session, cx| {
                    session.apply_format(FormatCommand::Bold, window, cx)
                })
                .expect("bold applies through the projection");
        })
        .unwrap();
        cx.run_until_parked();

        let edited = "# Body\n\n**First** sentence. Second sentence.\n";
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, edited);
            assert_eq!(
                session.buffer.current_text(),
                format!("{frontmatter}\n{edited}")
            );
            assert!(session.is_dirty());
        });

        // Exactly one undo step restores the source, hidden bytes included.
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, body);
            assert_eq!(session.buffer.current_text(), source);
            assert!(
                !session.is_dirty(),
                "undo must return to the saved baseline"
            );
        });

        // The Markdown comment toggle is one contiguous edit as well.
        cx.update_window(handle.into(), |_, _, cx| {
            let editor = session.read(cx).editor.clone();
            editor.update(cx, |editor, cx| editor.set_selected_range(0..6, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            session
                .update(cx, |session, cx| {
                    session.apply_format(FormatCommand::Comment, window, cx)
                })
                .expect("the comment toggle applies as one edit");
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.projection.visible,
                "<!-- # Body -->\n\nFirst sentence. Second sentence.\n"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session
                .update(cx, |session, cx| {
                    session.apply_format(FormatCommand::Comment, window, cx)
                })
                .expect("the second toggle restores the line");
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, body);
        });
        assert!(path.ends_with("src/posts/c.md"));
    }

    #[gpui_kit::test]
    fn heading_and_link_transforms_preserve_crlf_and_hidden_metadata(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        let frontmatter = "---\r\ntitle: 'CRLF'\r\n---\r\n\r\n";
        let source = format!("{frontmatter}intro\r\nsecond\r\n");
        let session = bare_session(cx, handle, "note.md", &source);
        let push = handle.into();
        cx.update_window(push, |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..0, cx));
                session
                    .apply_format(
                        FormatCommand::Heading(astrolith_document::HeadingLevel::H2),
                        window,
                        cx,
                    )
                    .unwrap();
            });
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, "## intro\r\nsecond\r\n");
            assert_eq!(
                session.buffer.current_text(),
                format!("{frontmatter}## intro\r\nsecond\r\n"),
                "every hidden byte and line ending is copied verbatim"
            );
        });

        // Paragraph removes the marker again, and a link wraps the selection.
        cx.update_window(push, |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .apply_format(FormatCommand::Paragraph, window, cx)
                    .unwrap();
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..5, cx));
                session
                    .apply_format(FormatCommand::Link, window, cx)
                    .unwrap();
            });
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, app| {
            assert_eq!(session.projection.visible, "[intro]()\r\nsecond\r\n");
            assert_eq!(
                session.editor.read(app).selected_range(),
                8..8,
                "the caret lands inside the empty URL"
            );
        });
    }

    #[gpui_kit::test]
    fn cursor_commands_follow_the_facade_selection_limits(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        let text = "aaa aaa aaa\nbbb\nccc\n";
        let session = bare_session(cx, handle, "cursor.md", text);
        let push = handle.into();
        cx.update_window(push, |_, _, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..3, cx));
                // Select-next only ever reads the active selection (B1), and each
                // press continues after the range the previous one added.
                assert_eq!(session.select_next_occurrence(cx), Some(4..7));
                assert_eq!(session.select_next_occurrence(cx), Some(8..11));
                assert_eq!(
                    session.select_next_occurrence(cx),
                    None,
                    "the third occurrence is the last"
                );
            });
        })
        .unwrap();
        cx.run_until_parked();

        let (handle, _) = mount(cx);
        let session = bare_session(cx, handle, "cursor.md", "aaa aaa\nbbb\nccc\n");
        let push = handle.into();
        cx.update_window(push, |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..3, cx));
                // The cursor set is the main selection's line ends, so a
                // single-line selection yields exactly one cursor.
                assert_eq!(session.cursors_at_line_ends(cx), 1);
                // Upstream keeps the main selection active, so the caret stays on
                // the line end the selection started from.
                assert_eq!(session.editor.read(cx).cursor(), 7);
                assert!(session.go_to_line(2, window, cx));
                assert_eq!(session.editor.read(cx).cursor(), 8);
                assert!(!session.go_to_line(9, window, cx));
            });
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    async fn counts_and_go_to_line_follow_every_keystroke(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/posts/a.md"), "one\ntwo\n").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let (_path, session) = open_entry_for(cx, handle, &view, "src/posts/a.md").await;
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                assert_eq!(session.counts.lines, 3);
                assert!(!session.go_to_line(4, window, cx));
            });
        })
        .unwrap();

        // Typing through the real input path refreshes the one snapshot the
        // footer and go-to-line read.
        edit_active(cx, handle, &view, "one\ntwo\nthree\n").await;
        view.read_with(cx, |view, cx| {
            let counts = view
                .active_counts(cx)
                .expect("an active document has counts");
            assert_eq!(counts.lines, 4);
            assert_eq!(counts.words, 3);
        });
        session.read_with(cx, |session, _| {
            assert!(
                !session.programmatic_change(),
                "typing is not the session's own write"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                assert!(session.go_to_line(4, window, cx));
                assert_eq!(session.editor.read(cx).cursor(), 14);
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn retarget_refreshes_the_counts_the_footer_reads(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        let source = "import A from \"a\";\n# Body\n";
        // `.mdx` hides the provable ESM prelude, so retargeting as `.md` shows
        // more text and must refresh the shared snapshot.
        let session = bare_session(cx, handle, "entry.mdx", source);
        let push = handle.into();
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, "# Body\n");
            assert_eq!(session.counts.lines, 2);
        });
        cx.update_window(push, |_, window, cx| {
            session.update(cx, |session, cx| {
                let identity = DocumentIdentity::new(
                    ProjectId::new("projection").unwrap(),
                    1,
                    SourcePath::new("entry.md").unwrap(),
                );
                session.retarget(identity, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, source);
            assert_eq!(session.counts.lines, 3);
            assert_eq!(session.counts.words, 5);
        });
    }

    #[gpui_kit::test]
    async fn selection_toolbar_is_reachable_from_the_registry(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/posts/a.md"), "# Body\n").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let (_path, session) = open_entry_for(cx, handle, &view, "src/posts/a.md").await;
        let panel = view.read_with(cx, |view, _| view.document.clone());
        let start = session.read_with(cx, |session, _| {
            session
                .projection
                .visible
                .find("Body")
                .expect("the body is visible")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            session.update(cx, |session, cx| {
                session.editor.update(cx, |editor, cx| {
                    editor.set_selected_range(start..start + 4, cx)
                });
            });
            let availability = view.read_with(cx, |view, cx| view.command_availability(cx, window));
            assert!(
                CommandId::FocusSelectionToolbar.is_available(availability),
                "a focused editor with a selection offers the toolbar"
            );
            panel.update(cx, |panel, cx| panel.focus_selection_toolbar(window, cx));
            assert!(
                panel.read(cx).toolbar_focus.is_focused(window),
                "the registry focus path reaches the toolbar despite the editor trapping Tab"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn formatting_dispatches_only_with_live_editor_focus(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/posts/a.md"), "# Body\n").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let (_path, session) = open_entry_for(cx, handle, &view, "src/posts/a.md").await;
        let start = session.read_with(cx, |session, _| {
            session
                .projection
                .visible
                .find("Body")
                .expect("the body is visible")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            session.update(cx, |session, cx| {
                session.editor.update(cx, |editor, cx| {
                    editor.set_selected_range(start..start + 4, cx)
                });
            });
            assert!(
                view.update(cx, |view, cx| view.apply_format(
                    FormatCommand::Bold,
                    window,
                    cx
                )),
                "the focused editor accepts formatting"
            );
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, "# **Body**\n");
            assert!(
                session.programmatic_change(),
                "a formatting command is not typing"
            );
        });

        // Focus anywhere else refuses the same command through the registry and
        // leaves the text alone.
        let elsewhere = cx.update(|cx| cx.focus_handle());
        cx.update_window(handle.into(), |_, window, cx| {
            window.focus(&elsewhere, cx);
            window.render_frame(cx);
            assert!(
                !view.update(cx, |view, cx| view.apply_format(
                    FormatCommand::Italic,
                    window,
                    cx
                )),
                "formatting needs the document's own focus"
            );
            assert_eq!(
                view.read(cx).status,
                "Formatting is unavailable while this document is not focused."
            );
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.projection.visible, "# **Body**\n");
        });
    }

    #[gpui_kit::test]
    async fn selection_toolbar_keeps_focus_and_stays_mounted_for_the_keyboard(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/posts/a.md"), "# Body text\n").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let (_path, session) = open_entry_for(cx, handle, &view, "src/posts/a.md").await;
        let document = cx.update(|cx| view.read(cx).document.clone());
        let toolbar_focus = document.read_with(cx, |panel, _| panel.toolbar_focus.clone());
        let editor_focus =
            session.read_with(cx, |session, cx| session.editor.read(cx).focus_handle(cx));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(2..6, cx));
            });
            editor_focus.focus(window, cx);
            document.update(cx, |_, cx| cx.notify());
            window.render_frame(cx);
            assert!(
                window.try_find("selection-toolbar").is_some(),
                "a selection in the focused editor shows the toolbar"
            );

            // Focus moving onto the toolbar must not unmount the control that
            // just received it.
            toolbar_focus.focus(window, cx);
            document.update(cx, |_, cx| cx.notify());
            window.render_frame(cx);
            assert!(
                window.try_find("selection-toolbar").is_some(),
                "the toolbar keeps itself mounted while it is focused"
            );

            // Tab walks off the container and into the button row.
            window.press("tab", cx);
            window.render_frame(cx);
            assert!(
                !toolbar_focus.is_focused(window) && toolbar_focus.contains_focused(window, cx),
                "Tab reaches a toolbar button"
            );
            assert!(
                window.try_find("selection-toolbar").is_some(),
                "and the overlay stays mounted for it"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn url_paste_wraps_the_selection_and_alt_click_hits_only_external_urls(
        cx: &mut TestAppContext,
    ) {
        let (handle, view) = mount(cx);
        let session = bare_session(
            cx,
            handle,
            "links.md",
            "Read [docs](https://example.dev/guide) or https://other.dev now.\n",
        );
        let push = handle.into();
        // Mount the session in the document pane so the editor is laid out and
        // `range_to_bounds` returns real geometry for the hit test.
        let document = cx.update(|cx| view.read(cx).document.clone());
        cx.update(|cx| {
            document.update(cx, |panel, cx| {
                panel.show_editor("links.md".into(), session.read(cx).editor.clone(), None, cx)
            });
        });
        cx.update_window(push, |_, window, cx| {
            window.render_frame(cx);
            session.update(cx, |session, cx| {
                // Paste-over-selection wraps the selection in one link.
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..4, cx));
                assert!(session.wrap_selection_as_link("https://wrap.dev/x", window, cx));
            });
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.projection.visible,
                "[Read](https://wrap.dev/x) [docs](https://example.dev/guide) or https://other.dev now.\n"
            );
        });

        // A non-URL clipboard payload never takes over the paste.
        cx.update_window(push, |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..4, cx));
                assert!(!session.wrap_selection_as_link("plain text", window, cx));
            });
        })
        .unwrap();
        cx.run_until_parked();

        // The URL hit test uses the editor's own range geometry.
        let bounds = cx
            .update_window(push, |_, window, cx| {
                window.render_frame(cx);
                let editor = session.read(cx).editor.clone();
                editor.update(cx, |editor, cx| {
                    let start = session
                        .read(cx)
                        .projection
                        .visible
                        .find("https://other.dev")
                        .unwrap();
                    editor.set_selected_range(0..0, cx);
                    // The neutral sans face is wider than the old serif and can
                    // wrap a long URL. Click the first glyph, as a user would,
                    // instead of the centre of an inverted multi-line rectangle.
                    editor
                        .range_to_bounds(&(start..start + 1))
                        .expect("the URL is laid out")
                })
            })
            .unwrap();
        let opened = cx
            .update_window(push, |_, _, cx| {
                session.update(cx, |session, cx| session.open_url_at(bounds.center(), cx))
            })
            .unwrap();
        assert_eq!(opened.as_deref(), Some("https://other.dev/"));
    }

    #[gpui_kit::test]
    async fn copyedit_decorates_after_the_debounce_and_suppresses_the_active_token(
        cx: &mut TestAppContext,
    ) {
        let (handle, _) = mount(cx);
        let text = "The editor writes words.\n";
        let session = bare_session(cx, handle, "copyedit.md", text);
        let mut preferences = CopyeditPreferences::default();
        preferences.set_enabled(CopyeditCategory::Noun, true);
        let config = CopyeditConfig::from_preferences(&preferences);
        cx.update_window(handle.into(), |_, _, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..0, cx));
                session.set_copyedit_config(config, cx);
            });
        })
        .unwrap();
        wait_for(cx, |cx| {
            session.read_with(cx, |session, _| session.copyedit.has_analysis())
        })
        .await;

        let editor_range = text.find("editor").unwrap()..text.find("editor").unwrap() + 6;
        session.read_with(cx, |session, cx| {
            assert!(
                session
                    .copyedit
                    .ranges(CopyeditCategory::Noun, cx)
                    .contains(&editor_range),
                "the background result decorates the noun after the debounce"
            );
        });

        // Moving into the word drops its decoration over the ±2-byte window.
        cx.update_window(handle.into(), |_, _, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(6..6, cx));
            });
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, cx| {
            assert!(
                !session
                    .copyedit
                    .ranges(CopyeditCategory::Noun, cx)
                    .contains(&editor_range),
                "the active token is left undecorated"
            );
        });

        // An edit clears the old layers immediately and the debounced result
        // reapplies them to the new snapshot.
        let second = "The editor writes words.\n\nNew words.\n";
        replace_visible(cx, handle, &session, second);
        session.read_with(cx, |session, cx| {
            assert!(
                session
                    .copyedit
                    .ranges(CopyeditCategory::Noun, cx)
                    .is_empty(),
                "editing clears Copyedit decorations until the next idle result"
            );
        });
        wait_for(cx, |cx| {
            session.read_with(cx, |session, _| session.copyedit.has_analysis())
        })
        .await;
        let first_words = second.find("words").unwrap();
        session.read_with(cx, |session, cx| {
            assert!(
                session
                    .copyedit
                    .ranges(CopyeditCategory::Noun, cx)
                    .contains(&(first_words..first_words + 5)),
                "the next background result decorates the edited snapshot"
            );
        });
    }

    #[gpui_kit::test]
    async fn pasted_link_reclassifies_copyedit_with_shifted_offsets(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        let text = "The editor writes.\n";
        let session = bare_session(cx, handle, "copyedit-link.md", text);
        let mut preferences = CopyeditPreferences::default();
        preferences.set_enabled(CopyeditCategory::Noun, true);
        let config = CopyeditConfig::from_preferences(&preferences);
        let push = handle.into();
        cx.update_window(push, |_, _, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(18..18, cx));
                session.set_copyedit_config(config, cx);
            });
        })
        .unwrap();
        wait_for(cx, |cx| {
            session.read_with(cx, |session, _| session.copyedit.has_analysis())
        })
        .await;
        session.read_with(cx, |session, cx| {
            assert!(
                session
                    .copyedit
                    .ranges(CopyeditCategory::Noun, cx)
                    .contains(&(4..10)),
                "the noun is decorated before the paste"
            );
        });

        cx.update_window(push, |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..10, cx));
                assert!(session.wrap_selection_as_link("https://wrap.dev/x", window, cx));
            });
        })
        .unwrap();
        session.read_with(cx, |session, cx| {
            assert!(
                session
                    .copyedit
                    .ranges(CopyeditCategory::Noun, cx)
                    .is_empty(),
                "the paste-over-selection clears the stale layers"
            );
        });
        wait_for(cx, |cx| {
            session.read_with(cx, |session, _| session.copyedit.has_analysis())
        })
        .await;
        session.read_with(cx, |session, cx| {
            let nouns = session.copyedit.ranges(CopyeditCategory::Noun, cx);
            assert!(
                nouns.contains(&(5..11)),
                "the debounced result uses the shifted offset: {nouns:?}"
            );
            assert!(!nouns.contains(&(4..10)), "the stale offset is gone");
        });
    }

    pub(super) async fn wait_for_session(
        cx: &mut TestAppContext,
        view: &Entity<ProjectWorkspace>,
        path: &Path,
    ) {
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.sessions.contains_key(path) && view.loads.is_empty()
            }) {
                return;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        panic!("session never appeared for {}", path.display());
    }

    #[gpui_kit::test]
    async fn draft_filter_and_default_order_use_mapped_fields(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(
            dir.path().join("src/posts/a.md"),
            "---\ntitle: 'Alpha'\npubDate: 2024-01-01\nstatus: true\n---\n# A\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/posts/b.mdx"),
            "---\ntitle: 'Bravo'\npubDate: 2025-01-01\nstatus: 'true'\n---\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/posts/c.md"),
            "---\ntitle: 'Charlie'\nstatus: false\n---\n# C\n",
        )
        .unwrap();
        let (handle, view) = mount(cx);
        set_mappings(
            &view,
            cx,
            FieldMappings {
                draft: Some("status".into()),
                ..FieldMappings::default()
            },
        );
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        // Default order: undated first, then newest publication date.
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["c.md", "b.mdx", "a.md"]
        );
        view.update(cx, |view, cx| view.toggle_drafts_only(cx));
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["a.md"],
            "only the literal boolean true in the mapped field is a draft"
        );
        // Case-insensitive search over the mapped title.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.toggle_drafts_only(cx);
                view.list
                    .update(cx, |list, cx| list.set_query("BRAVO", window, cx));
            })
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["b.mdx"]
        );
        // A non-default mode honours the direction toggle.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.clear_filter(window, cx);
                view.set_sort_mode(SortMode::Filename, cx);
                view.toggle_sort_direction(cx);
            })
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["c.md", "b.mdx", "a.md"]
        );
    }

    #[gpui_kit::test]
    async fn library_list_count_and_search_reuse_cached_results(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        let count = view.read_with(cx, |view, cx| {
            let delegate = view.list.read(cx).delegate();
            (delegate.entry_count(), delegate.rebuilds)
        });
        assert_eq!(count.0, 2, "the cached count tracks the published entries");

        // An identical rebuild reuses the sections instead of re-filtering and
        // re-sorting the whole collection.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.list
                    .update(cx, |list, cx| list.set_query("a", window, cx));
            });
        })
        .unwrap();
        let repeated = view.read_with(cx, |view, cx| {
            let delegate = view.list.read(cx).delegate();
            (delegate.entry_count(), delegate.rebuilds)
        });
        assert_eq!(repeated.0, 1, "the filtered count is cached too");
        cx.update_window(handle.into(), |_, _window, cx| {
            view.update(cx, |view, cx| {
                view.list.update(cx, |list, _| {
                    let delegate = list.delegate_mut();
                    delegate.rebuild();
                    delegate.rebuild();
                });
            });
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.list.read(cx).delegate().rebuilds),
            repeated.1,
            "an identical rebuild is skipped"
        );

        // A new index revision rebuilds against the published entries even
        // though the query stayed the same.
        view.update(cx, |view, cx| {
            let entries = vec![view.entries[1].clone()];
            view.publish_entries(entries, cx);
        });
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            Vec::<String>::new(),
            "the surviving entry does not match the active filter"
        );
        assert_eq!(
            view.read_with(cx, |view, cx| view.list.read(cx).delegate().entry_count()),
            0,
            "the cached count follows removals"
        );
        assert!(
            view.read_with(cx, |view, cx| view.list.read(cx).delegate().rebuilds) > repeated.1,
            "a redrawn index invalidates the cached sections"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.list
                    .update(cx, |list, cx| list.set_query("b", window, cx));
            });
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["b.mdx"],
            "the result still reflects the current index revision"
        );
    }

    pub(super) struct RecordingTrash {
        pub(super) fail: bool,
        pub(super) saved: Mutex<Option<Vec<u8>>>,
    }

    impl TrashBackend for RecordingTrash {
        fn trash(&self, path: &Path) -> Result<(), String> {
            if self.fail {
                return Err("trash is unavailable".into());
            }
            *self.saved.lock().unwrap() = Some(std::fs::read(path).unwrap());
            std::fs::remove_file(path).map_err(|error| error.to_string())
        }
    }

    #[gpui_kit::test]
    async fn empty_loading_and_error_states_stay_distinguishable(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::remove_file(dir.path().join("src/posts/a.md")).unwrap();
        std::fs::remove_file(dir.path().join("src/posts/b.mdx")).unwrap();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.start(dir.path().to_path_buf(), window, cx)
            });
            window.render_frame(cx);
            assert!(
                window.try_find("project-loading").is_some(),
                "the initial scan must render its own state"
            );
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.is_loading()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("project-collections").is_some());
            assert!(window.try_find("project-loading").is_none());
            view.update(cx, |view, cx| {
                view.select_collection(Some("blog".into()), window, cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("project-no-entries").is_some());
            view.update(cx, |view, _| {
                view.index_error = Some("index exploded".into())
            });
            window.render_frame(cx);
            assert!(window.try_find("project-error").is_some());
            assert!(window.try_find("project-no-entries").is_none());
        })
        .unwrap();

        let filtered = fixture();
        let (filtered_handle, filtered_view) = mount(cx);
        open_project_and_wait(cx, filtered_handle, &filtered_view, filtered.path()).await;
        cx.update_window(filtered_handle.into(), |_, window, cx| {
            filtered_view.update(cx, |view, cx| {
                view.select_collection(Some("blog".into()), window, cx);
                view.toggle_drafts_only(cx);
            });
            window.render_frame(cx);
            assert!(window.try_find("project-filtered-empty").is_some());
            assert!(window.try_find("project-no-entries").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn library_lists_collections_first_and_scopes_entries(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join("src/notes")).unwrap();
        std::fs::write(dir.path().join("src/notes/note.md"), "# Note\n").unwrap();
        std::fs::write(
            dir.path().join("src/content.config.ts"),
            "import {defineCollection} from 'astro:content'; import {glob} from 'astro/loaders'; const blog=defineCollection({loader:glob({base:'src/posts',pattern:'**/*.{md,mdx}'})}); const notes=defineCollection({loader:glob({base:'src/notes',pattern:'**/*.md'})}); export const collections={blog,notes};",
        )
        .unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("project-collections").is_some(),
                "the library opens on its collection index"
            );
            assert!(window.try_find("project-no-entries").is_none());
            view.update(cx, |view, cx| {
                view.select_collection(Some("blog".into()), window, cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("project-collections").is_none());
            assert!(window.try_find("library-back").is_some());
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["a.md", "b.mdx"],
            "only the selected collection's entries are listed"
        );

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.select_collection(Some("notes".into()), window, cx)
            });
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["note.md"]
        );

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.select_collection(None, window, cx));
            window.render_frame(cx);
            assert!(window.try_find("project-collections").is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn library_browses_nested_directories_with_breadcrumbs(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join("src/posts/zh-cn")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/posts/ja")).unwrap();
        std::fs::write(dir.path().join("src/posts/zh-cn/one.md"), "# One\n").unwrap();
        std::fs::write(dir.path().join("src/posts/zh-cn/two.md"), "# Two\n").unwrap();
        std::fs::write(dir.path().join("src/posts/ja/three.md"), "# Three\n").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.select_collection(Some("blog".into()), window, cx)
            });
            window.render_frame(cx);
            assert!(
                window.try_find("library-crumb-0").is_some(),
                "the breadcrumb starts at the collections index"
            );
            assert!(
                window.try_find("library-crumb-1").is_none(),
                "the current collection is plain text, not a link"
            );
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["a.md", "b.mdx"],
            "a collection shows only entries at its own level"
        );
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_directory_rows(cx)),
            [("ja".to_owned(), 1), ("zh-cn".to_owned(), 2)],
            "child folders carry recursive entry counts"
        );

        // Filtering still searches the whole selected collection, flat.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.list
                    .update(cx, |list, cx| list.set_query("two", window, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["two.md"]
        );
        assert!(
            view.read_with(cx, |view, cx| view.visible_directory_rows(cx))
                .is_empty()
        );
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.clear_filter(window, cx));
        })
        .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.select_directory(vec!["zh-cn".into()], window, cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("library-crumb-1").is_some());
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["one.md", "two.md"],
            "a browsed folder lists its own entries"
        );
        assert!(
            view.read_with(cx, |view, cx| view.visible_directory_rows(cx))
                .is_empty()
        );

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.library_back(window, cx));
            window.render_frame(cx);
            assert!(window.try_find("library-crumb-1").is_none());
        })
        .unwrap();
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["a.md", "b.mdx"],
            "back returns to the collection root"
        );

        let nested = std::fs::canonicalize(dir.path().join("src/posts/zh-cn/two.md")).unwrap();
        view.update(cx, |view, cx| view.focus_collection_for(&nested, cx));
        assert_eq!(
            view.read_with(cx, |view, _| view.selected_directory.clone()),
            ["zh-cn"],
            "opening a nested entry follows it in the library"
        );
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["one.md", "two.md"]
        );

        view.update(cx, |view, cx| {
            view.set_collection_schemas(
                vec![(
                    "blog".to_owned(),
                    schema(vec![required_field(
                        "subtitle",
                        FieldKind::String { format: None },
                    )]),
                )],
                cx,
            )
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.entry_command(CommandId::NewEntry, None, window, cx)
            });
            assert!(
                window.has_active_dialog(cx),
                "required inputs must be collected before anything is written"
            );
            assert_eq!(
                view.read_with(cx, |view, _| {
                    view.pending_create
                        .as_ref()
                        .and_then(|pending| pending.directory.as_ref())
                        .map(|directory| directory.as_str().to_owned())
                }),
                Some("src/posts/zh-cn".to_owned()),
                "New entry targets the browsed folder"
            );
            window.press("escape", cx);
        })
        .unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.select_directory(Vec::new(), window, cx));
            view.update(cx, |view, cx| view.library_back(window, cx));
            window.render_frame(cx);
            assert!(window.try_find("project-collections").is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn library_entry_rows_leave_room_for_the_filename(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join("src/posts/nested")).unwrap();
        std::fs::write(dir.path().join("src/posts/nested/one.md"), "# One\n").unwrap();
        for (name, title) in [("a.md", "First entry"), ("b.mdx", "Second entry")] {
            std::fs::write(
                dir.path().join("src/posts").join(name),
                format!("---\ntitle: {title}\n---\n\n# Body\n"),
            )
            .unwrap();
        }
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.select_collection(Some("blog".into()), window, cx)
            });
            window.render_frame(cx);
            let first = window.find("entry:blog:a").bounds();
            let second = window.find("entry:blog:b").bounds();
            assert!(
                second.origin.y >= first.origin.y + first.size.height,
                "entry rows must not overlap: first={first:?} second={second:?}"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn failed_open_shows_a_distinct_recoverable_state(cx: &mut TestAppContext) {
        let not_a_project = tempfile::tempdir().unwrap();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.start(not_a_project.path().to_path_buf(), window, cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("project-loading").is_some());
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.is_loading()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("project-open-error").is_some(),
                "a failed open must not look like the never-opened state"
            );
            assert!(window.try_find("project-empty").is_none());
        })
        .unwrap();

        let dir = fixture();
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("project-open-error").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn resolved_collection_directory_drives_index_creation_and_watcher(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join("src/notes")).unwrap();
        std::fs::write(dir.path().join("src/notes/note.md"), "# Note\n").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["a.md", "b.mdx"],
            "the first open uses the Astro configuration base"
        );

        view.update(cx, |view, cx| {
            let mut project = ProjectPreferencesV1::default();
            project.collections.insert(
                "blog".to_owned(),
                CollectionPreferencesV1 {
                    content_directory: Some("src/notes".into()),
                    ..CollectionPreferencesV1::default()
                },
            );
            view.set_preferences(GlobalPreferencesV2::default(), Some(project), cx);
        });
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.entries.len() == 1
                    && view.entries[0].entry().source().as_str() == "src/notes/note.md"
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(
            view.read_with(cx, |view, cx| view.visible_entry_names(cx)),
            ["note.md"],
            "opening lists the resolved override directory"
        );

        let expected = dir.path().join("src/notes").join(format!("{}.md", today()));
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.entry_command(CommandId::NewEntry, None, window, cx)
            })
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if expected.exists()
                && view.read_with(cx, |view, _| view.sessions.contains_key(&expected))
            {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(expected.exists(), "creation writes under the override");
        assert!(
            !dir.path()
                .join("src/posts")
                .join(format!("{}.md", today()))
                .exists(),
            "creation must not write into the Astro configuration base"
        );
        let created = format!("src/notes/{}.md", today());
        assert!(
            view.read_with(cx, |view, _| view.entries.iter().any(|entry| entry
                .entry()
                .source()
                .as_str()
                == created)),
            "the created entry is indexed"
        );

        // A watcher batch re-indexes the override directory, never the config base.
        let before = view.read_with(cx, |view, _| view.index_revision());
        std::fs::write(dir.path().join("src/notes/second.md"), "# Second\n").unwrap();
        for _ in 0..300 {
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(20))
            })
            .await
            .unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.index_revision() > before
                    && view
                        .entries
                        .iter()
                        .any(|entry| entry.entry().source().as_str() == "src/notes/second.md")
            }) {
                break;
            }
        }
        view.read_with(cx, |view, _| {
            assert!(
                view.entries
                    .iter()
                    .any(|entry| entry.entry().source().as_str() == "src/notes/second.md"),
                "the watcher batch indexes the override directory"
            );
            assert!(
                view.entries.iter().all(|entry| entry
                    .entry()
                    .source()
                    .as_str()
                    .starts_with("src/notes/")),
                "the watcher batch keeps the override base, not the config base"
            );
            assert!(
                view.entries
                    .iter()
                    .any(|entry| entry.entry().source().as_str() == created),
                "the created entry survives the watcher batch"
            );
        });

        // An escaping override is reported instead of silently ignored.
        view.update(cx, |view, cx| {
            let mut project = ProjectPreferencesV1::default();
            project.collections.insert(
                "blog".to_owned(),
                CollectionPreferencesV1 {
                    content_directory: Some("../escape".into()),
                    ..CollectionPreferencesV1::default()
                },
            );
            view.set_preferences(GlobalPreferencesV2::default(), Some(project), cx);
        });
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.entries
                    .iter()
                    .any(|entry| entry.entry().source().as_str() == "src/posts/a.md")
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        view.read_with(cx, |view, _| {
            assert!(
                view.override_problems
                    .iter()
                    .any(|problem| problem.contains("content directory override")),
                "an unusable override is reported: {:?}",
                view.override_problems
            );
            assert!(
                view.entries
                    .iter()
                    .any(|entry| entry.entry().source().as_str() == "src/posts/a.md"),
                "the config base is used again after the report"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("project-override-warning").is_some(),
                "the override diagnostic is visible, not only logged"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn project_content_directory_rebases_each_collection_under_it(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join("src/notes2")).unwrap();
        std::fs::write(dir.path().join("src/notes2/n.md"), "# N\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src/content/posts")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/content/notes2")).unwrap();
        std::fs::write(
            dir.path().join("src/content/posts/rebased.md"),
            "# Rebased\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/content/notes2/note.md"), "# Note\n").unwrap();
        std::fs::write(dir.path().join("src/content.config.ts"),"import {defineCollection} from 'astro:content'; import {glob} from 'astro/loaders'; const blog=defineCollection({loader:glob({base:'src/posts',pattern:'**/*.md'})}); const notes=defineCollection({loader:glob({base:'src/notes2',pattern:'**/*.md'})}); export const collections={blog,notes};").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        view.update(cx, |view, cx| {
            let project = ProjectPreferencesV1 {
                content_directory: Some("src/content".into()),
                ..ProjectPreferencesV1::default()
            };
            view.set_preferences(GlobalPreferencesV2::default(), Some(project), cx);
        });
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                let sources: Vec<&str> = view
                    .entries
                    .iter()
                    .map(|entry| entry.entry().source().as_str())
                    .collect();
                sources.contains(&"src/content/posts/rebased.md")
                    && sources.contains(&"src/content/notes2/note.md")
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.collection_bases.get("blog").map(PathBuf::as_path),
                Some(Path::new("src/content/posts"))
            );
            assert_eq!(
                view.collection_bases.get("notes").map(PathBuf::as_path),
                Some(Path::new("src/content/notes2"))
            );
            let mut names = view.visible_entry_names(cx);
            names.sort();
            assert_eq!(names, ["note.md", "rebased.md"]);
            assert!(
                view.entries.iter().all(|entry| entry
                    .entry()
                    .source()
                    .as_str()
                    .starts_with("src/content/")),
                "a project content root rebases every collection under it"
            );
        });

        // Re-applying the same scope must resolve from the pristine configuration
        // bases, never nest the project root inside an already-rebased base.
        view.update(cx, |view, cx| {
            let project = ProjectPreferencesV1 {
                content_directory: Some("src/content".into()),
                ..ProjectPreferencesV1::default()
            };
            view.set_preferences(GlobalPreferencesV2::default(), Some(project), cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(
                view.override_problems.is_empty(),
                "{:?}",
                view.override_problems
            );
            assert_eq!(
                view.collection_bases.get("blog").map(PathBuf::as_path),
                Some(Path::new("src/content/posts"))
            );
            assert_eq!(
                view.collection_bases.get("notes").map(PathBuf::as_path),
                Some(Path::new("src/content/notes2"))
            );
        });

        // A collection-scope value is that collection's exact base, while the
        // project scope still rebases the others.
        view.update(cx, |view, cx| {
            let mut project = ProjectPreferencesV1 {
                content_directory: Some("src/content".into()),
                ..ProjectPreferencesV1::default()
            };
            project.collections.insert(
                "notes".to_owned(),
                CollectionPreferencesV1 {
                    content_directory: Some("src/notes2".into()),
                    ..CollectionPreferencesV1::default()
                },
            );
            view.set_preferences(GlobalPreferencesV2::default(), Some(project), cx);
        });
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.collection_bases.get("notes").map(PathBuf::as_path)
                    == Some(Path::new("src/notes2"))
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        view.read_with(cx, |view, _| {
            assert!(
                view.override_problems.is_empty(),
                "{:?}",
                view.override_problems
            );
            assert_eq!(
                view.collection_bases.get("blog").map(PathBuf::as_path),
                Some(Path::new("src/content/posts"))
            );
            assert_eq!(
                view.collection_bases.get("notes").map(PathBuf::as_path),
                Some(Path::new("src/notes2"))
            );
        });
    }

    /// A real Astro project with one indexed project component and two entries.
    pub(super) fn component_builder_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("node_modules/astro")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/@astrojs/mdx")).unwrap();
        std::fs::write(
            root.join("node_modules/astro/package.json"),
            r#"{"version":"5.18.2"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("node_modules/@astrojs/mdx/package.json"),
            r#"{"version":"4.3.14","peerDependencies":{"astro":"^5.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"builder-fixture","dependencies":{"astro":"5.18.2","@astrojs/starlight":"0.42.8"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("astro.config.mjs"),
            "import { defineConfig } from 'astro/config';\nimport mdx from '@astrojs/mdx';\nexport default defineConfig({ integrations: [mdx()] });\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::write(
            root.join("src/components/Callout.astro"),
            "---\n/**\n * A framed callout.\n */\ninterface Props {\n  /** Callout heading. */\n  title: string;\n  /** Visual tone. */\n  tone?: \"note\" | \"tip\";\n  /** Cover image. */\n  cover?: ImageMetadata;\n}\n---\n\n<section><slot /></section>\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/components/lower.astro"),
            "---\ninterface Props {\n  label: string;\n}\n---\n\n<span>{label}</span>\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/content/blog")).unwrap();
        std::fs::write(
            root.join("src/content.config.ts"),
            "import { defineCollection } from 'astro:content';\nimport { glob } from 'astro/loaders';\nconst blog = defineCollection({ loader: glob({ base: 'src/content', pattern: '**/*.{md,mdx}' }) });\nexport const collections = { blog };\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/content/blog/post.mdx"),
            "---\ntitle: Post\n---\n\n# Body\n",
        )
        .unwrap();
        dir
    }

    pub(super) async fn wait_for_components(
        cx: &mut TestAppContext,
        view: &Entity<ProjectWorkspace>,
    ) {
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                !view.component_loading && view.component_index.is_some()
            }) {
                return;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        panic!("the component index never loaded");
    }

    pub(super) fn callout(
        view: &Entity<ProjectWorkspace>,
        cx: &mut TestAppContext,
    ) -> astrolith_astro::components::Component {
        view.read_with(cx, |view, _| {
            view.component_index
                .as_ref()
                .expect("the component index is loaded")
                .components()
                .iter()
                .find(|component| {
                    component.name == "Callout"
                        && matches!(
                            component.location,
                            astrolith_astro::components::ComponentLocation::Project { .. }
                        )
                })
                .expect("the fixture Callout is indexed")
                .clone()
        })
    }

    pub(super) fn component_selection(
        component: &astrolith_astro::components::Component,
        title: &str,
        imports_enabled: bool,
    ) -> ComponentSelection {
        let mut form = crate::editor::component_builder::form::FormState::new(component, None);
        let index = form
            .fields()
            .iter()
            .position(|field| field.prop.name == "title")
            .expect("Callout has a title prop");
        form.set_text(index, title);
        ComponentSelection {
            component: component.clone(),
            values: form.values(),
            planned: form.planned_values(),
            content: None,
            client_directive: None,
            imports_enabled,
        }
    }

    pub(super) fn select_visible_end(session: &Entity<DocumentSession>, cx: &mut TestAppContext) {
        let end = session.read_with(cx, |session, _| session.projection.visible.len());
        cx.update(|cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(end..end, cx));
            });
        });
    }

    #[gpui_kit::test]
    async fn component_insertion_writes_the_tag_and_only_the_planned_import(
        cx: &mut TestAppContext,
    ) {
        let dir = component_builder_fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        wait_for_components(cx, &view).await;
        let (_, session) = open_entry_for(cx, handle, &view, "src/content/blog/post.mdx").await;
        let component = callout(&view, cx);

        select_visible_end(&session, cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.insert_component(component_selection(&component, "Hello", false), window, cx)
            })
        })
        .unwrap()
        .expect("the tag inserts without imports");
        assert_eq!(
            session.read_with(cx, |session, _| session.buffer.current_text().to_owned()),
            "---\ntitle: Post\n---\n\n# Body\n\n<Callout title=\"Hello\">\n  \n</Callout>\n",
            "imports off must add the tag only"
        );

        // Undo the tag, then insert the same component with imports enabled.
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        select_visible_end(&session, cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.insert_component(component_selection(&component, "Hello", true), window, cx)
            })
        })
        .unwrap()
        .expect("the tag inserts with its planned import");
        let imported = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        assert_eq!(
            imported,
            "---\ntitle: Post\n---\n\nimport Callout from \"../../components/Callout.astro\";\n\n# Body\n\n<Callout title=\"Hello\">\n  \n</Callout>\n",
            "imports on must add the planned import and the tag"
        );
        assert!(view.read_with(cx, |view, _| view.status.contains("Its import was added")));
    }

    pub(super) fn named_component(
        view: &Entity<ProjectWorkspace>,
        cx: &mut TestAppContext,
        name: &str,
    ) -> astrolith_astro::components::Component {
        view.read_with(cx, |view, _| {
            view.component_index
                .as_ref()
                .expect("the component index is loaded")
                .components()
                .iter()
                .find(|component| component.name == name)
                .unwrap_or_else(|| panic!("{name} is indexed"))
                .clone()
        })
    }

    #[gpui_kit::test]
    async fn insertion_plans_against_a_dirty_body(cx: &mut TestAppContext) {
        let dir = component_builder_fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        wait_for_components(cx, &view).await;
        let (_, session) = open_entry_for(cx, handle, &view, "src/content/blog/post.mdx").await;
        let component = callout(&view, cx);

        // Type without saving: the plan and the tag must both use the live text.
        select_visible_end(&session, cx);
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.insert_snippet("Unsaved paragraph.\n".to_owned(), window, cx)
            })
        })
        .unwrap();
        assert!(session.read_with(cx, |session, _| session.buffer.is_dirty()));
        select_visible_end(&session, cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.insert_component(component_selection(&component, "Hello", true), window, cx)
            })
        })
        .unwrap()
        .expect("a dirty body still takes the tag and its import");
        assert_eq!(
            session.read_with(cx, |session, _| session.buffer.current_text().to_owned()),
            concat!(
                "---\ntitle: Post\n---\n\n",
                "import Callout from \"../../components/Callout.astro\";\n",
                "\n# Body\nUnsaved paragraph.\n\n",
                "<Callout title=\"Hello\">\n  \n</Callout>\n",
            )
        );
        assert!(session.read_with(cx, |session, _| session.buffer.is_dirty()));
    }

    #[gpui_kit::test]
    async fn a_non_importable_name_inserts_the_tag_and_reports_the_warning(
        cx: &mut TestAppContext,
    ) {
        let dir = component_builder_fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        wait_for_components(cx, &view).await;
        let (_, session) = open_entry_for(cx, handle, &view, "src/content/blog/post.mdx").await;
        let lower = named_component(&view, cx, "lower");
        let selection = ComponentSelection {
            values: crate::editor::component_builder::form::FormState::new(&lower, None).values(),
            planned: Vec::new(),
            content: None,
            client_directive: None,
            imports_enabled: true,
            component: lower,
        };
        select_visible_end(&session, cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.insert_component(selection, window, cx))
        })
        .unwrap()
        .expect("the tag still inserts; only the import is refused");
        let text = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        assert!(text.contains("<lower"), "{text:?}");
        assert!(!text.contains("import lower"), "{text:?}");
        assert!(
            view.read_with(cx, |view, _| view.status.contains("uppercase")),
            "the planning warning is surfaced: {}",
            view.read_with(cx, |view, _| view.status.clone())
        );
    }

    #[gpui_kit::test]
    async fn stale_import_plan_is_refused_at_insertion_and_surfaced(cx: &mut TestAppContext) {
        let dir = component_builder_fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        wait_for_components(cx, &view).await;
        let (path, session) = open_entry_for(cx, handle, &view, "src/content/blog/post.mdx").await;
        let component = callout(&view, cx);
        let root = view.read_with(cx, |view, _| view.project_root().unwrap());
        let source = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        let visible = session.read_with(cx, |session, _| session.projection.visible.clone());
        let selection = component_selection(&component, "Hello", true);
        let prepared = prepare(&InsertionContext {
            component: &component,
            values: selection.values,
            planned: selection.planned,
            content: None,
            client_directive: None,
            root: &root,
            document: &path,
            source: &source,
            imports_enabled: true,
            current_imports: &[],
            other_imports: &[],
            catalog: &[],
            visible: &visible,
            selection: 0..0,
        })
        .expect("the plan is valid for the original source");

        // The document changes between planning and insertion.
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.insert_snippet("changed ".to_owned(), window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        let changed = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        assert_ne!(changed, source);
        let error = cx
            .update_window(handle.into(), |_, window, cx| {
                session.update(cx, |session, cx| {
                    session.insert_component(prepared, window, cx)
                })
            })
            .unwrap()
            .expect_err("a stale plan must be refused");
        assert!(error.contains("stale"), "{error}");
        assert_eq!(
            session.read_with(cx, |session, _| session.buffer.current_text().to_owned()),
            changed,
            "a refused stale plan must not write anything"
        );

        // A real planning failure is shown in the workspace status and keeps the source.
        let mut form = crate::editor::component_builder::form::FormState::new(&component, None);
        let title = form
            .fields()
            .iter()
            .position(|field| field.prop.name == "title")
            .unwrap();
        form.set_text(title, "Hello");
        let cover = form
            .fields()
            .iter()
            .position(|field| field.prop.name == "cover")
            .unwrap();
        form.set_text(cover, "../assets/missing.png");
        let failing = ComponentSelection {
            component: component.clone(),
            values: form.values(),
            planned: form.planned_values(),
            content: None,
            client_directive: None,
            imports_enabled: true,
        };
        let before = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        let error = cx
            .update_window(handle.into(), |_, window, cx| {
                view.update(cx, |view, cx| view.insert_component(failing, window, cx))
            })
            .unwrap()
            .expect_err("a missing planned image must fail visibly");
        assert!(error.contains("missing.png"), "{error}");
        assert_eq!(view.read_with(cx, |view, _| view.status.clone()), error);
        assert_eq!(
            session.read_with(cx, |session, _| session.buffer.current_text().to_owned()),
            before
        );
    }

    #[gpui_kit::test]
    async fn undo_removes_the_inserted_tag_and_leaves_the_import(cx: &mut TestAppContext) {
        let dir = component_builder_fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        wait_for_components(cx, &view).await;
        let (_, session) = open_entry_for(cx, handle, &view, "src/content/blog/post.mdx").await;
        let component = callout(&view, cx);
        select_visible_end(&session, cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.insert_component(component_selection(&component, "Hello", true), window, cx)
            })
        })
        .unwrap()
        .unwrap();
        assert!(session.read_with(cx, |session, _| {
            session.buffer.current_text().contains("<Callout")
        }));

        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("ctrl-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let text = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        assert!(!text.contains("<Callout"), "{text:?}");
        assert!(
            text.contains("import Callout from \"../../components/Callout.astro\";"),
            "undo intentionally keeps the harmless import: {text:?}"
        );
        assert!(text.contains("# Body"), "{text:?}");
    }

    #[gpui_kit::test]
    async fn placeholder_navigation_visits_the_blank_required_prop_and_exits(
        cx: &mut TestAppContext,
    ) {
        let dir = component_builder_fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        wait_for_components(cx, &view).await;
        let (_, session) = open_entry_for(cx, handle, &view, "src/content/blog/post.mdx").await;
        let component = callout(&view, cx);
        select_visible_end(&session, cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.insert_component(component_selection(&component, "", false), window, cx)
            })
        })
        .unwrap()
        .unwrap();

        let ranges = session.read_with(cx, |session, _| session.placeholder_ranges().to_vec());
        assert_eq!(
            ranges.len(),
            2,
            "blank title and wrapped content placeholders"
        );
        let caret = session.read_with(cx, |session, cx| session.editor.read(cx).selected_range());
        assert_eq!(
            caret.start, ranges[0].start,
            "the empty required prop receives the caret"
        );

        let editor = session.read_with(cx, |session, _| session.editor.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", editor.entity_id()), cx);
            window.press("tab", cx);
        })
        .unwrap();
        let caret = session.read_with(cx, |session, cx| session.editor.read(cx).selected_range());
        assert_eq!(
            caret.start, ranges[1].start,
            "Tab moves to the content placeholder"
        );

        cx.update_window(handle.into(), |_, window, cx| {
            window.press("shift-tab", cx);
        })
        .unwrap();
        let caret = session.read_with(cx, |session, cx| session.editor.read(cx).selected_range());
        assert_eq!(
            caret.start, ranges[0].start,
            "Shift+Tab returns to the required prop"
        );
        let before = session.read_with(cx, |session, _| session.buffer.current_text().to_owned());
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("escape", cx);
        })
        .unwrap();
        assert!(
            session.read_with(cx, |session, _| session.placeholder_ranges().is_empty()),
            "Escape ends placeholder navigation"
        );
        assert_eq!(
            session.read_with(cx, |session, _| session.buffer.current_text().to_owned()),
            before,
            "placeholder navigation must not edit the source"
        );
    }
}
