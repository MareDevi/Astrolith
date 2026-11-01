//! Owns project generations, index publication, retained sessions and load admission.
use crate::{
    commands::{CommandAvailability, CommandId},
    editor::{
        formatting::EditorWiring,
        session::{DocumentSession, SessionChanged},
        writing_modes::WritingModes,
    },
    frontmatter::{FrontmatterEdit, ReferenceCatalog},
    layout::panels::WorkspacePanel,
    runtime, settings,
};
use astrolith_astro::{
    Cancellation, DiscoveryOutcome, DiscoveryRequest, ProjectManifest, contained_path,
    inspect_project, read_bounded,
    schema::generation::{SchemaGenerationRequest, generate_collection_schemas},
    schema::{CollectionSchema, FieldKind, SchemaFreshness, SchemaRefresh, schema_fingerprint},
};
use astrolith_core::{
    CollectionId, ProjectId, SourcePath,
    preferences::{
        FieldMappings, FileExtension, GlobalPreferencesV2, ProjectPreferencesV1, resolve,
    },
};
use astrolith_document::{
    DocumentIdentity, DocumentKind, DocumentLoadTicket, DocumentRevision, DocumentSnapshot,
    FormatCommand, RecoveryRecord, RecoveryStore, SaveOutcome, TokenSummary, parse_document,
    save_snapshot,
};
use astrolith_preview::RouteMapping;
use astrolith_workspace::{
    CollectionEntry, CollectionWatcher, ReferenceOption,
    list::{self, EntryFilter, EntrySort, SortDirection, SortMode},
    operations::{
        self, CopyPathRequest, CreateRequest, CreationField, CreationInput, CreationInputKind,
        DuplicateRequest, MoveRequest, NewEntryName, PathForm, ProjectScope, RevealRequest,
        ScalarValue, SystemTrash, TrashBackend, TrashRequest,
    },
    references, scan_collections,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IndexPath, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, Focusable, IntoElement, PathPromptOptions,
    Render, SharedString, Subscription, Task, TestSupportExt, WeakEntity, Window, div, prelude::*,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

// Tokio owns the native watcher's final drop; UI cancellation only aborts its task.
struct ServiceWatcher {
    weak: std::sync::Weak<CollectionWatcher>,
    owner: tokio::task::JoinHandle<()>,
}
impl ServiceWatcher {
    fn new(watcher: CollectionWatcher, cancellation: Cancellation) -> Self {
        let watcher = Arc::new(watcher);
        let weak = Arc::downgrade(&watcher);
        let owner = tokio::spawn(async move {
            while cancellation.check().is_ok() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            drop(watcher);
        });
        Self { weak, owner }
    }
}
impl Drop for ServiceWatcher {
    fn drop(&mut self) {
        self.owner.abort();
    }
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
}

struct EntrySection {
    title: SharedString,
    rows: Vec<EntryRow>,
}

struct EntryList {
    owner: WeakEntity<ProjectWorkspace>,
    entries: Arc<Vec<CollectionEntry>>,
    sections: Vec<EntrySection>,
    query: String,
    filter: EntryFilter,
    sort: EntrySort,
    /// Resolved field mappings per collection identity.
    mappings: BTreeMap<String, FieldMappings>,
    selected: Option<IndexPath>,
    active: Option<PathBuf>,
    root: Option<PathBuf>,
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
            mappings: BTreeMap::new(),
            selected: None,
            active: None,
            root: None,
        };
        list.rebuild();
        list
    }

    fn rebuild(&mut self) {
        let filter = EntryFilter {
            query: self.query.clone(),
            drafts_only: self.filter.drafts_only,
        };
        let mut groups: BTreeMap<&str, Vec<(usize, &CollectionEntry)>> = BTreeMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let collection = entry.entry().id().collection().as_str();
            let mappings = self.mappings.get(collection);
            if !list::matches_filter(entry, mappings, &filter) {
                continue;
            }
            groups.entry(collection).or_default().push((index, entry));
        }
        let mut sections = Vec::with_capacity(groups.len());
        for (collection, mut rows) in groups {
            let mappings = self.mappings.get(collection);
            rows.sort_by(|(_, left), (_, right)| {
                list::compare_entries(left, right, mappings, &self.sort)
            });
            let mut section = EntrySection {
                title: collection.to_owned().into(),
                rows: Vec::with_capacity(rows.len()),
            };
            for (index, entry) in rows {
                let title = list::display_title(entry, mappings);
                let filename = list::file_name(entry).to_owned();
                let show_filename = title != filename;
                let date = list::publication_date(entry, mappings)
                    .map(|date| format!("{date}"))
                    .unwrap_or_default();
                let search = format!("{filename}\n{title}").to_lowercase();
                let id =
                    SharedString::from(format!("entry:{collection}:{}", entry.entry().id().id()));
                let path = self
                    .root
                    .as_ref()
                    .map(|root| root.join(entry.entry().source().as_str()))
                    .unwrap_or_else(|| PathBuf::from(entry.entry().source().as_str()));
                section.rows.push(EntryRow {
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
                });
            }
            sections.push(section);
        }
        self.sections = sections;
    }

    fn entry_index(&self, ix: IndexPath) -> Option<usize> {
        self.sections
            .get(ix.section)?
            .rows
            .get(ix.row)
            .map(|row| row.index)
    }

    fn visible_count(&self) -> usize {
        self.sections.iter().map(|section| section.rows.len()).sum()
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
        Some(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_1()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child(section.title.clone())
                .child(SharedString::from(section.rows.len().to_string())),
        )
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let row = self.sections.get(ix.section)?.rows.get(ix.row)?;
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
        let badge = |label: &'static str, color: gpui_kit::Hsla| {
            div()
                .px_1()
                .rounded_sm()
                .border_1()
                .border_color(color)
                .text_color(color)
                .text_xs()
                .child(label)
        };
        let row_content = div()
            .h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(Icon::new(IconName::FileText).small().text_color(if active {
                cx.theme().foreground
            } else {
                cx.theme().muted_foreground
            }))
            .child(
                div()
                    .v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(div().min_w_0().truncate().child(title))
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
            .when(mdx, |row| row.child(badge("MDX", cx.theme().accent)))
            .when(draft, |row| row.child(badge("Draft", cx.theme().warning)))
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
                menu.item(entry_menu_item(&owner, CommandId::NewEntry, &path, busy))
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
            });
        Some(
            ListItem::new(row.id.clone())
                .selected(selected)
                .accessibility_label(row.search.clone())
                .child(row_content),
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
    list: Entity<ListState<EntryList>>,
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
    session_subscriptions: HashMap<PathBuf, Subscription>,
    recovery: Option<RecoveryStore>,
    pending_recovery: Vec<RecoveryRecord>,
    recovery_task: Option<Task<()>>,
    recovery_load_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
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
    /// Sort menu entries derived from the installed collection schemas.
    sort_options: Vec<(SortMode, SharedString)>,
    /// Operation to run once a guarded save of a dirty session succeeds.
    pending_after_save: Option<PendingAfterSave>,
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
}

/// What to do after a dirty session has been saved through the guarded path.
enum PendingAfterSave {
    Rename { from: PathBuf, to: SourcePath },
    Duplicate { path: PathBuf },
    Trash { path: PathBuf },
}

/// One completed watcher scan, published only when its request is the newest.
struct WatcherScan {
    index: Result<(ProjectManifest, Vec<CollectionEntry>), String>,
    fingerprints: Vec<(PathBuf, Option<u64>)>,
}

/// Everything a create dialog needs after its required-field inputs are filled.
struct PendingCreate {
    collection: CollectionId,
    directory: Option<SourcePath>,
    extension: FileExtension,
    fields: Vec<CreationField>,
    today: String,
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
    /// Live editor text, including unsaved changes.
    pub text: String,
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

/// Emitted once a project's manifest and entry index are published.
///
/// Carries the canonical root so preference consumers never derive a project
/// identity from the per-open [`ProjectId`].
pub(crate) struct ProjectOpened {
    /// Canonical project root.
    pub root: PathBuf,
}
impl EventEmitter<ProjectOpened> for ProjectWorkspace {}
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
        Self {
            document,
            manifest: None,
            entries,
            list,
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
            status: "Open an Astro project to edit local Markdown and MDX.".into(),
            task: None,
            watch_task: None,
            schema_task: None,
            watcher_owner: None,
            loads: HashMap::new(),
            sessions: BTreeMap::new(),
            session_collections: BTreeMap::new(),
            schemas: BTreeMap::new(),
            active: None,
            _subscriptions: vec![subscription, query_observer],
            session_subscriptions: HashMap::new(),
            recovery: settings::recovery_dir().ok().map(RecoveryStore::new),
            pending_recovery: Vec::new(),
            recovery_task: None,
            recovery_load_task: None,
            save_task: None,
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
            pending_after_save: None,
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
        }
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
        cx.notify();
    }

    /// Install one completed schema generation unless a newer request superseded it.
    ///
    /// Returns `false` when the report is stale; the caller must then drop it
    /// without touching the form so a superseded generation can never replace
    /// fresh schemas or reload the panel.
    fn apply_generated_schemas(
        &mut self,
        request: u64,
        fingerprint: u64,
        schemas: Vec<(String, CollectionSchema)>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.schema_freshness.accept(request, fingerprint) {
            tracing::debug!(request, "dropped a superseded schema generation result");
            return false;
        }
        tracing::info!(
            collections = schemas.len(),
            revision = self.schema_revision + 1,
            "collection schemas generated"
        );
        self.set_collection_schemas(schemas, cx);
        true
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
                    this.status = format!("Data collection unavailable: {diagnostic}");
                }
                cx.notify();
            });
        }));
    }

    /// Generate collection schemas from the project's own Astro install and
    /// keep them fresh while the project is open.
    ///
    /// Generation and the freshness fingerprint both run on the blocking lane.
    /// A missing or failing Astro install is non-fatal: the frontmatter form
    /// keeps whatever schemas it already had. A change to the generated schema
    /// directory or the content/astro config starts a new generation; results
    /// from a superseded request are dropped before touching the form, and the
    /// form reloads from the live editor text so dirty edits survive.
    fn load_collection_schemas(&mut self, cx: &mut Context<Self>) {
        let Some(manifest) = self.manifest.clone() else {
            return;
        };
        let generation = self.generation;
        let project = self.project_id.clone();
        let root = manifest.root().to_owned();
        let cancellation = self.cancellation.clone();
        tracing::debug!(
            project = %project.as_ref().map_or("<unknown>", |project| project.as_str()),
            root = %root.display(),
            "starting schema generation and freshness polling"
        );
        self.schema_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let probe_root = root.clone();
                let fingerprint =
                    runtime::blocking(cx, move || schema_fingerprint(&probe_root)).await;
                let Ok(Ok(fingerprint)) = fingerprint else {
                    break;
                };
                let decision = this.update(cx, |this, _| {
                    if this.generation != generation || this.project_id != project {
                        return Err(());
                    }
                    Ok(match this.schema_freshness.reconcile(fingerprint) {
                        SchemaRefresh::Current => None,
                        SchemaRefresh::Regenerate { request } => Some(request),
                    })
                });
                let Ok(Ok(Some(request))) = decision else {
                    if !matches!(decision, Ok(Ok(None))) {
                        break;
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(750))
                        .await;
                    continue;
                };
                let generation_root = root.clone();
                let cancellation = cancellation.clone();
                let report = runtime::blocking(cx, move || {
                    let request =
                        SchemaGenerationRequest::new(generation_root.clone(), cancellation);
                    let report = generate_collection_schemas(&request);
                    // Astro rewrites the schema files on every sync, so the
                    // fingerprint is re-read after generation and that exact
                    // value is installed. Otherwise the next poll would see a
                    // "different" fingerprint and sync forever.
                    let fingerprint = schema_fingerprint(&generation_root).ok();
                    (report, fingerprint)
                })
                .await;
                let serviced = this.update(cx, |this, cx| {
                    if this.generation != generation || this.project_id != project {
                        return Err(());
                    }
                    match report {
                        Ok((Ok(report), Some(fingerprint))) => {
                            let schemas = report
                                .schemas
                                .into_iter()
                                .map(|entry| (entry.collection, entry.schema))
                                .collect();
                            this.apply_generated_schemas(request, fingerprint, schemas, cx);
                            Ok(())
                        }
                        Ok((Ok(_), None)) => {
                            this.schema_freshness.reject(request);
                            tracing::warn!(
                                "could not re-read the schema fingerprint after generation"
                            );
                            Ok(())
                        }
                        Ok((Err(error), _)) => {
                            this.schema_freshness.reject(request);
                            tracing::warn!(%error, "collection schema generation failed");
                            Ok(())
                        }
                        Err(error) => {
                            this.schema_freshness.reject(request);
                            tracing::warn!(%error, "collection schema generation task failed");
                            Ok(())
                        }
                    }
                });
                if !matches!(serviced, Ok(Ok(()))) {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(750))
                    .await;
            }
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
        if !supports_frontmatter(&path) {
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
        Some(ActiveFrontmatter {
            schema: self.schemas.get(collection).cloned(),
            mappings: self.resolved_for(Some(collection)).field_mappings,
            references: self.references.clone(),
            root: self.project_root()?,
            text,
        })
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
                        astrolith_document::yaml::validate_value(raw)
                            .map_err(|reason| format!("invalid raw YAML: {reason}"))?;
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
                self.status = format!("Could not edit frontmatter: {error}");
                cx.notify();
            }
        }
    }
    pub fn prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if self.has_dirty(cx) {
            self.status="Keep this project open while documents have unsaved changes. Close edited documents explicitly before switching projects.".into();
            cx.notify();
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Astro project".into()),
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
                            this.start(path, window, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.status = "Could not select a project directory.".into();
                    }
                }
                cx.notify();
            });
        }));
    }
    fn start(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(root = %root.display(), "opening Astro project");
        if self.has_dirty(cx) {
            self.status = "Close unsaved documents before switching projects.".into();
            cx.notify();
            return;
        }
        self.cancellation.cancel();
        self.cancellation = Cancellation::default();
        let Some(next) = self.generation.checked_add(1) else {
            self.status = "Project generation exhausted; restart the application.".into();
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
        self.status = "Discovering project…".into();
        self.watch_task = None;
        self.schema_task = None;
        self.watcher_owner = None;
        self.command_tasks.clear();
        self.loads.clear();
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
        self.pending_create = None;
        self.pending_recovery.clear();
        self.manifest = None;
        self.active = None;
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
                        this.start_watcher(watcher, window, cx);
                        this.status = format!(
                            "{} entries · Ctrl/Cmd+S saves the active document.",
                            this.entries.len()
                        );
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
                        this.status = format!("Could not open project: {error}");
                        this.open_error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
        // `window` is only needed by the retained GPUI observer, never by service I/O.
        cx.notify();
    }
    fn start_watcher(
        &mut self,
        watcher: ServiceWatcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(manifest) = self.manifest.clone() else {
            return;
        };
        let weak_watcher = watcher.weak.clone();
        self.watcher_owner = Some(watcher);
        let generation = self.generation;
        let project = manifest.project().clone();
        let root = manifest.root().to_owned();
        tracing::debug!(
            project = %manifest.project().as_str(),
            generation,
            root = %root.display(),
            "project watcher started"
        );
        let cancellation = self.cancellation.clone();
        self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                // Poll for a merged batch off the render path. The scan itself is a
                // separate index request so it can be ordered against rescans.
                let polled = weak_watcher.clone();
                let pending = runtime::blocking(cx, move || {
                    let Some(watcher) = polled.upgrade() else {
                        return Err("Watcher stopped".to_string());
                    };
                    let batch = watcher.drain();
                    Ok(batch.changed || batch.needs_rescan)
                })
                .await
                .and_then(|result| result);
                match pending {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(error) => {
                        tracing::warn!(%error, "project watcher failed; buffers retained");
                        let _ = this.update_in(cx, |this, _, cx| {
                            if this.generation != generation
                                || this.project_id.as_ref() != Some(&project)
                            {
                                return;
                            }
                            this.apply_index_failure(error, cx);
                            cx.notify();
                        });
                        break;
                    }
                }
                let Some((token, request, snapshots)) = this
                    .update_in(cx, |this, _, _| {
                        if this.generation != generation
                            || this.project_id.as_ref() != Some(&project)
                        {
                            return None;
                        }
                        let manifest = this.manifest.clone()?;
                        let request = DiscoveryRequest::new(
                            manifest.project().clone(),
                            generation,
                            root.clone(),
                            cancellation.clone(),
                        )
                        .with_collection_bases(this.collection_bases.clone());
                        let snapshots = this
                            .sessions
                            .keys()
                            .chain(this.loads.keys())
                            .cloned()
                            .collect::<BTreeSet<_>>();
                        // Claimed here so any rescan issued afterwards supersedes
                        // this watcher scan instead of being overwritten by it.
                        Some((this.begin_index_request(), request, snapshots))
                    })
                    .ok()
                    .flatten()
                else {
                    break;
                };
                let scanning = weak_watcher.clone();
                let scan_root = root.clone();
                let result = runtime::blocking(cx, move || {
                    request
                        .cancellation()
                        .check()
                        .map_err(|error| error.to_string())?;
                    let Some(watcher) = scanning.upgrade() else {
                        return Err("Watcher stopped".to_string());
                    };
                    // Compare retained and in-flight sources even when reindexing fails.
                    let fingerprints = snapshots
                        .into_iter()
                        .map(|path| {
                            let fingerprint = contained_path(
                                &scan_root,
                                path.strip_prefix(&scan_root).unwrap_or(&path),
                            )
                            .ok()
                            .filter(|actual| actual == &path)
                            .and_then(|actual| read_bounded(&actual, 8 * 1024 * 1024).ok())
                            .map(|text| crate::editor::session::fingerprint(&text));
                            (path, fingerprint)
                        })
                        .collect::<Vec<_>>();
                    let index: Result<_, String> =
                        (|| match inspect_project(&request).map_err(|error| error.to_string())? {
                            DiscoveryOutcome::Ready(manifest) => {
                                if let Err(error) = watcher.refresh(&manifest) {
                                    tracing::warn!(%error, "collection watcher refresh failed");
                                }
                                let entries = scan_collections(&manifest, request.cancellation())
                                    .map_err(|error| error.to_string())?;
                                Ok((manifest, entries))
                            }
                            DiscoveryOutcome::Unsupported { diagnostics, .. } => {
                                Err(diagnostics.join("; "))
                            }
                        })();
                    Ok(WatcherScan {
                        index,
                        fingerprints,
                    })
                })
                .await
                .and_then(|result| result);
                let still_current = this
                    .update_in(cx, |this, _, cx| {
                        if this.generation != generation
                            || this.project_id.as_ref() != Some(&project)
                        {
                            return false;
                        }
                        this.apply_watcher_scan(token, result, cx);
                        true
                    })
                    .unwrap_or(false);
                if !still_current {
                    break;
                }
            }
        }));
    }

    /// Publish one watcher scan unless a newer index request already won.
    ///
    /// External-change flags and session reconciliation follow the same ordering:
    /// an older scan never overwrites the newest published index, so it can never
    /// close a clean session that the newer index still contains.
    fn apply_watcher_scan(
        &mut self,
        token: u64,
        scan: Result<WatcherScan, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_index_result(token) {
            tracing::debug!(
                token,
                published = self.published_index_request,
                "dropping a stale watcher scan"
            );
            self.finish_index_request(token);
            return;
        }
        self.published_index_request = token;
        match scan {
            Ok(scan) => {
                for (path, fingerprint) in scan.fingerprints {
                    if let Some(session) = self.sessions.get(&path) {
                        session.update(cx, |session, _| {
                            session.externally_changed =
                                fingerprint != Some(session.disk_fingerprint)
                        });
                        self.update_tab_state(&path, cx);
                    }
                }
                match scan.index {
                    Ok((manifest, entries)) => {
                        // A watcher batch reconciles the same state an operation
                        // does: no stale tab or selection survives.
                        self.apply_discovered_index(manifest, entries, cx);
                        self.status = "Index refreshed; open buffers retained. Close and reopen an unchanged document to reload it.".into();
                    }
                    Err(error) => {
                        tracing::warn!(%error, "project reindex failed; buffers retained");
                        self.apply_index_failure(error, cx);
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "project watcher failed; buffers retained");
                self.apply_index_failure(error, cx);
            }
        }
        self.finish_index_request(token);
        cx.notify();
    }
    fn publish_entries(&mut self, entries: Vec<CollectionEntry>, cx: &mut Context<Self>) {
        self.index_revision = self
            .index_revision
            .checked_add(1)
            .expect("index revision exhausted");
        self.loads.clear();
        self.entries = Arc::new(entries);
        self.display_name = self.manifest.as_ref().map(|manifest| {
            let root = manifest.root();
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string())
                .into()
        });
        self.sync_tabs(cx);
        tracing::debug!(
            entries = self.entries.len(),
            index_revision = self.index_revision,
            "published project entries"
        );
        self.rebuild_sort_options();
        let root = self.project_root();
        let active = self.active.clone();
        let entries = self.entries.clone();
        let mappings = self.collection_mappings();
        let filter = self.list_filter.clone();
        let sort = self.list_sort.clone();
        self.list.update(cx, |list, cx| {
            let delegate = list.delegate_mut();
            delegate.entries = entries;
            delegate.root = root;
            delegate.active = active;
            delegate.mappings = mappings;
            delegate.filter = filter;
            delegate.sort = sort;
            delegate.rebuild();
            delegate.selected = delegate
                .selected_row()
                .map(|(section, row)| IndexPath::new(row).section(section));
            cx.notify();
        });
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

    fn open_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_entry_inner(ix, false, window, cx);
    }

    /// Open an entry produced by an operation that just published a fresh index.
    fn open_entry_after_publish(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_entry_inner(ix, true, window, cx);
    }

    fn open_entry_inner(
        &mut self,
        ix: usize,
        forced: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.entries.get(ix) else {
            return;
        };
        let Some(manifest) = self.manifest.clone() else {
            return;
        };
        let source = entry.entry().source().clone();
        let collection = entry.entry().id().collection().as_str().to_owned();
        let path = manifest.root().join(source.as_str());
        tracing::debug!(
            path = %path.display(),
            collection = %collection,
            source = %source.as_str(),
            "opening a collection entry"
        );
        if self.sessions.contains_key(&path) {
            self.activate_session(&path, window, cx);
            return;
        }
        if self.loads.contains_key(&path) {
            self.active = Some(path);
            self.document
                .update(cx, |document, cx| document.clear_editor(cx));
            return;
        }
        if self.refreshing && !forced {
            self.status =
                "Refreshing the project index; select this entry again when it finishes.".into();
            cx.notify();
            return;
        }
        if self.loads.len() >= 2 {
            self.status =
                "Two documents are loading; select this entry again when one finishes.".into();
            cx.notify();
            return;
        }
        if self.sessions.len() + self.loads.len() >= 32 {
            self.status = "Close an open document before opening more than 32 documents.".into();
            cx.notify();
            return;
        }
        let retained_bytes: usize = self
            .sessions
            .values()
            .map(|session| session.read(cx).buffer.original_text().len())
            .sum();
        if retained_bytes + (self.loads.len() + 1) * 8 * 1024 * 1024 > 32 * 1024 * 1024 {
            self.status =
                "Close documents before exceeding the 32 MiB loaded-source budget.".into();
            cx.notify();
            return;
        }
        self.active = Some(path.clone());
        self.session_collections
            .insert(path.clone(), collection.clone());
        self.document
            .update(cx, |document, cx| document.clear_editor(cx));
        let generation = self.generation;
        let identity =
            DocumentIdentity::new(manifest.project().clone(), generation, source.clone());
        let ticket = DocumentLoadTicket::new(identity, DocumentRevision::INITIAL);
        let index_revision = self.index_revision;
        let expected = path.clone();
        let root = manifest.root().to_owned();
        let cancellation = self.cancellation.clone();
        let load = runtime::blocking(cx, move || {
            cancellation.check().map_err(|error| error.to_string())?;
            let actual = contained_path(&root, std::path::Path::new(source.as_str()))
                .map_err(|error| error.to_string())?;
            if actual != expected {
                return Err("Source mapping changed; rescan the project.".into());
            }
            let source =
                read_bounded(&actual, 8 * 1024 * 1024).map_err(|error| error.to_string())?;
            if source.lines().count() > 50_000 {
                return Err("Documents are limited to 50,000 lines.".into());
            }
            cancellation.check().map_err(|error| error.to_string())?;
            Ok(source)
        });
        let key = path.clone();
        self.loads.insert(
            path.clone(),
            cx.spawn_in(window, async move |this, cx| {
                let result = load.await.and_then(|result| result);
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_load(ticket, index_revision, key, result, window, cx);
                });
            }),
        );
        self.status = "Loading document…".into();
        cx.notify();
    }
    fn finish_load(
        &mut self,
        ticket: DocumentLoadTicket,
        index_revision: u64,
        key: PathBuf,
        result: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let identity = ticket.identity();
        if self.generation != identity.project_generation()
            || self.project_id.as_ref() != Some(identity.project())
            || self.index_revision != index_revision
            || ticket.expected_revision() != DocumentRevision::INITIAL
            || self.sessions.contains_key(&key)
        {
            return;
        }
        self.loads.remove(&key);
        match result {
            Ok(source) => {
                tracing::info!(
                    path = %key.display(),
                    bytes = source.len(),
                    "document loaded"
                );
                // Only a coalesced first load can create an editor. Refresh never replaces it.
                let session =
                    cx.new(|cx| DocumentSession::new(identity.clone(), source, window, cx));
                // Every session starts with the writer's current modes.
                let modes = self.writing;
                session.update(cx, |session, cx| {
                    session.set_writing_modes(modes, window, cx)
                });
                let tab_key = key.clone();
                let subscription =
                    cx.subscribe(&session, move |this, _, _: &SessionChanged, cx| {
                        this.update_tab_state(&tab_key, cx);
                        this.surface_session_notice(&tab_key, cx);
                        this.schedule_recovery(cx);
                        cx.emit(SessionChanged);
                        cx.notify();
                    });
                self.session_subscriptions.insert(key.clone(), subscription);
                self.sessions.insert(key.clone(), session.clone());
                if !self.tab_order.contains(&key) {
                    self.tab_order.push(key.clone());
                }
                self.sync_tabs(cx);
                if self.active.as_ref() == Some(&key) {
                    self.install(&key, session, cx);
                    self.document.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
            Err(error) => {
                tracing::warn!(path = %key.display(), %error, "could not load a collection entry");
                if self.active.as_ref() == Some(&key) {
                    self.active = None;
                }
                self.status = format!("Could not open entry: {error}");
            }
        }
        cx.notify();
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
        self.active = Some(path.to_owned());
        self.install(path, session, cx);
        self.refresh_entry_selection(cx);
        self.document.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn refresh_entry_selection(&mut self, cx: &mut Context<Self>) {
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
    fn install(
        &mut self,
        path: &std::path::Path,
        session: Entity<DocumentSession>,
        cx: &mut Context<Self>,
    ) {
        let title = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let editor = session.read(cx).editor.clone();
        let wiring = EditorWiring::new(session.downgrade());
        self.document.update(cx, |document, cx| {
            document.show_editor(title, editor, Some(wiring), cx)
        });
        self.status = if session.read(cx).externally_changed {"Source changed on disk. This buffer is retained; close and reopen an unchanged document to reload."} else {"Editing in memory · Ctrl/Cmd+S saves."}.into();
        cx.notify();
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
                let owner = owner.clone();
                let path = path.clone();
                dialog
                    .title(format!("Discard changes to “{title}”?"))
                    .child("The document has unsaved changes. Keeping it open preserves them.")
                    .footer(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("keep-editing")
                                    .primary()
                                    .label("Keep editing")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("discard-document")
                                    .danger()
                                    .label("Discard changes")
                                    .on_click(move |_, window, cx| {
                                        let _ = owner
                                            .update(cx, |this, cx| this.remove_session(&path, cx));
                                        window.close_dialog(cx);
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
            return true;
        }
        if !window.has_active_dialog(cx) {
            let owner = cx.weak_entity();
            window.open_dialog(cx,move|dialog,_,_| {
                let owner=owner.clone();
                dialog.title("Discard unsaved documents?")
                    .child("Changes exist only in memory. Keep the window open to preserve your documents.")
                    .footer(div().h_flex().gap_2()
                        .child(Button::new("keep-window").primary().label("Keep editing").on_click(|_,window,cx|window.close_dialog(cx)))
                        .child(Button::new("discard-window").danger().label("Discard and close").on_click(move|_,window,cx| {
                            let _=owner.update(cx,|this,cx| {
                                if let Some(store)=this.recovery.clone() {
                                    let identities: Vec<_> = this.sessions.values().map(|session| session.read(cx).buffer.identity().clone()).collect();
                                    let task = runtime::blocking(cx, move || { for identity in identities { let _ = store.clear(&identity); } });
                                    task.detach();
                                }
                                this.sessions.clear();
                            });
                            window.close_dialog(cx);window.remove_window();
                        })))
            });
        }
        false
    }

    /// Save the active document with conflict detection.
    pub fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active.clone() else {
            self.status = "No document is open to save.".into();
            cx.notify();
            return;
        };
        self.save_path(path, window, cx);
    }

    /// Save one retained document with conflict detection.
    fn save_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(&path).cloned() else {
            return;
        };
        // Capture the latest editor text into the buffer before snapshotting so a
        // queued Change event cannot cause a stale snapshot.
        session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            cx.notify();
        });
        let snapshot = DocumentSnapshot::capture(&session.read(cx).buffer, path.clone());
        tracing::debug!(
            path = %path.display(),
            revision = snapshot.revision().get(),
            "starting document save"
        );
        self.status = "Saving…".into();
        cx.notify();
        let save = runtime::spawn(cx, async move { save_snapshot(&snapshot).await });
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = save
                .await
                .and_then(|outcome| outcome.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_save(path, result, window, cx);
            });
        }));
    }

    fn finish_save(
        &mut self,
        path: PathBuf,
        result: Result<SaveOutcome, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.sessions.get(&path).cloned() else {
            self.pending_after_save = None;
            self.status = "Document closed before the save finished.".into();
            cx.notify();
            return;
        };
        let mut saved = false;
        let mut newer_edits = false;
        match result {
            Ok(SaveOutcome::Saved { revision }) => {
                saved = true;
                tracing::info!(
                    path = %path.display(),
                    revision,
                    "document saved"
                );
                let identity = session.read(cx).buffer.identity().clone();
                let (marked, content) = session.update(cx, |session, cx| {
                    // A newer edit after the snapshot may still be pending;
                    // materialize it so `confirm_saved` sees the newer revision.
                    session.sync_buffer(cx);
                    let content = session.buffer.current_text().to_owned();
                    let marked = session.confirm_saved(DocumentRevision::new(revision), &content);
                    session.dirty = session.buffer.is_dirty();
                    if marked {
                        session.externally_changed = false;
                    }
                    cx.notify();
                    (marked, content)
                });
                if marked && let Some(store) = self.recovery.clone() {
                    let task = runtime::blocking(cx, move || {
                        let _ = store.clear_after_save(&identity, &content);
                    });
                    task.detach();
                }
                // A pending trash/rename/duplicate must never act on an earlier
                // revision: the buffer is re-checked after the save completed.
                newer_edits = session.read(cx).buffer.is_dirty();
                self.status = if marked {
                    "Saved.".into()
                } else {
                    "Saved an earlier revision; newer edits remain unsaved.".into()
                };
            }
            Ok(SaveOutcome::Conflict) => {
                tracing::warn!(
                    path = %path.display(),
                    "save conflict: the file changed on disk; edits retained"
                );
                session.update(cx, |session, cx| {
                    session.externally_changed = true;
                    cx.notify();
                });
                self.status = "Save conflict: the file changed on disk. Your edits are kept; reopen the document to reload, then try again.".into();
            }
            Err(error) => {
                tracing::error!(path = %path.display(), %error, "document save failed");
                self.status = format!("Could not save: {error}");
            }
        }
        match self.pending_after_save.take() {
            Some(action) if saved && newer_edits => {
                self.run_after_saved_revision(action, window, cx);
            }
            Some(action) if saved => self.run_after_save(action, window, cx),
            _ => {}
        }
        cx.notify();
    }

    /// Handle a post-save action when the saved revision is already outdated.
    ///
    /// Trash and duplicate are abandoned: both would act on the persisted earlier
    /// revision while the newer edit is still only in memory. A rename still
    /// retargets the retained session and schedules recovery for the dirty buffer
    /// so the newer edits stay recoverable under the new source.
    fn run_after_saved_revision(
        &mut self,
        action: PendingAfterSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        };
        match action {
            PendingAfterSave::Rename { from, to } => {
                self.perform_rename(from, to, window, cx);
            }
            PendingAfterSave::Trash { path } => {
                tracing::warn!(
                    path = %path.display(),
                    "aborting a pending trash: newer edits are still unsaved"
                );
                self.status = format!(
                    "{} was not moved to the Trash because newer edits were typed while saving. Save again, then trash it.",
                    name(&path)
                );
                self.schedule_recovery(cx);
            }
            PendingAfterSave::Duplicate { path } => {
                tracing::warn!(
                    path = %path.display(),
                    "aborting a pending duplicate: newer edits are still unsaved"
                );
                self.status = format!(
                    "{} was not duplicated because newer edits were typed while saving. Save again, then duplicate it.",
                    name(&path)
                );
                self.schedule_recovery(cx);
            }
        }
    }

    fn schedule_recovery(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.recovery.clone() else {
            return;
        };
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_millis(500));
        self.recovery_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let Ok((dirty, clean)) = this.update(cx, |this, cx| this.persist_recovery_records(cx))
            else {
                return;
            };
            if dirty.is_empty() && clean.is_empty() {
                return;
            }
            let result = runtime::blocking(cx, move || {
                let mut written = 0usize;
                let mut cleared = 0usize;
                for record in dirty {
                    match store.save(&record) {
                        Ok(()) => written += 1,
                        Err(error) => {
                            tracing::warn!(
                                source = %record.identity().canonical_source().as_str(),
                                %error,
                                "could not write a recovery record"
                            );
                        }
                    }
                }
                for identity in clean {
                    match store.clear(&identity) {
                        Ok(_) => cleared += 1,
                        Err(error) => {
                            tracing::warn!(
                                source = %identity.canonical_source().as_str(),
                                %error,
                                "could not clear a recovery record"
                            );
                        }
                    }
                }
                (written, cleared)
            })
            .await;
            match result {
                Ok((written, cleared)) => {
                    tracing::debug!(written, cleared, "recovery records persisted")
                }
                Err(error) => tracing::warn!(%error, "recovery persistence task failed"),
            }
        }));
    }

    fn persist_recovery_records(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (Vec<RecoveryRecord>, Vec<DocumentIdentity>) {
        let now = now_secs();
        let mut dirty = Vec::new();
        let mut clean = Vec::new();
        for session in self.sessions.values().cloned().collect::<Vec<_>>() {
            let (is_dirty, identity) = session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                (session.buffer.is_dirty(), session.buffer.identity().clone())
            });
            let session = session.read(cx);
            if is_dirty {
                dirty.push(RecoveryRecord::capture(&session.buffer, now));
            } else {
                clean.push(identity);
            }
        }
        (dirty, clean)
    }

    /// Flush every dirty session's current content to the recovery store on the
    /// native-quit path.
    ///
    /// Only the editor-text snapshot runs on the UI thread; the writes run on the
    /// service runtime via [`runtime::flush_on_quit`], which is submitted outside
    /// ordinary file admission and drains before shutdown. That way a 500 ms
    /// recovery debounce that has not yet fired cannot lose unsaved content when
    /// the application exits.
    ///
    /// Returns the flush task so a caller (or a test) can await it, or `None`
    /// when recovery is disabled or nothing is dirty.
    pub fn flush_recovery_on_quit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<(), String>>> {
        let store = self.recovery.clone()?;
        let (dirty, _clean) = self.persist_recovery_records(cx);
        if dirty.is_empty() {
            return None;
        }
        tracing::debug!(
            documents = dirty.len(),
            "flushing unsaved recovery records on exit"
        );
        Some(runtime::flush_on_quit(cx, move || {
            for record in dirty {
                if let Err(error) = store.save(&record) {
                    tracing::error!(
                        source = %record.identity().canonical_source().as_str(),
                        %error,
                        "could not flush a recovery record on exit"
                    );
                }
            }
        }))
    }

    fn load_pending_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.recovery.clone() else {
            return;
        };
        let sources: std::collections::HashSet<String> = self
            .entries
            .iter()
            .map(|entry| entry.entry().source().as_str().to_owned())
            .collect();
        if sources.is_empty() {
            self.pending_recovery.clear();
            return;
        }
        let generation = self.generation;
        let task = runtime::blocking(cx, move || {
            store.load().map(|load| {
                let diagnostics = load.diagnostics().to_vec();
                let records = load
                    .records()
                    .iter()
                    .filter(|record| {
                        sources.contains(record.identity().canonical_source().as_str())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                (records, diagnostics)
            })
        });
        self.recovery_load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .and_then(|value| value.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                let (records, diagnostics) = match result {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(%error, "could not load recovery records");
                        return;
                    }
                };
                for diagnostic in diagnostics {
                    tracing::warn!(%diagnostic, "recovery record skipped");
                }
                let records: Vec<_> = records
                    .into_iter()
                    .filter(|record| this.project_id.as_ref() == Some(record.identity().project()))
                    .collect();
                tracing::debug!(
                    records = records.len(),
                    "recovery records matched the open project"
                );
                if records.is_empty() {
                    this.pending_recovery.clear();
                    cx.notify();
                    return;
                }
                let count = records.len();
                this.pending_recovery = records;
                this.status = format!("Restoring {count} unsaved document(s) from recovery…");
                // Auto-apply: restore pending records as dirty sessions immediately
                // instead of only prompting. `recover_pending` is idempotent and never
                // writes to disk, so the manual command remains safe afterwards.
                this.recover_pending(window, cx);
                cx.notify();
            });
        }));
    }

    /// Restore pending recovery records as dirty sessions (reads happen off the UI thread).
    pub fn recover_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_recovery.is_empty() {
            self.status = "No recovery records to restore.".into();
            cx.notify();
            return;
        }
        let Some(manifest) = self.manifest.clone() else {
            return;
        };
        let root = manifest.root().to_owned();
        let records = std::mem::take(&mut self.pending_recovery);
        tracing::info!(documents = records.len(), "restoring unsaved documents");
        let reads = runtime::blocking(cx, move || {
            let mut loaded = Vec::new();
            for record in records {
                let source = record.identity().canonical_source().clone();
                let Ok(actual) = contained_path(&root, std::path::Path::new(source.as_str()))
                else {
                    tracing::warn!(
                        source = %source.as_str(),
                        "recovery record path is outside the project"
                    );
                    continue;
                };
                let Ok(disk) = read_bounded(&actual, 8 * 1024 * 1024) else {
                    tracing::warn!(
                        path = %actual.display(),
                        "could not read the current document while restoring recovery"
                    );
                    continue;
                };
                loaded.push((
                    actual,
                    record.identity().clone(),
                    disk,
                    record.content().to_owned(),
                ));
            }
            loaded
        });
        self.restore_task = Some(cx.spawn_in(window, async move |this, cx| {
            let loaded = reads.await.unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                let mut restored = 0;
                for (path, identity, disk, content) in loaded {
                    if this.sessions.contains_key(&path) {
                        this.activate_session(&path, window, cx);
                        restored += 1;
                        continue;
                    }
                    let session = cx.new(|cx| {
                        DocumentSession::from_recovered(identity, disk, content, window, cx)
                    });
                    let key = path.clone();
                    let notice_key = key.clone();
                    let subscription =
                        cx.subscribe(&session, move |this, _, _: &SessionChanged, cx| {
                            this.surface_session_notice(&notice_key, cx);
                            this.schedule_recovery(cx);
                            cx.emit(SessionChanged);
                            cx.notify();
                        });
                    this.session_subscriptions.insert(key.clone(), subscription);
                    this.sessions.insert(key.clone(), session.clone());
                    this.active = Some(key.clone());
                    this.install(&key, session.clone(), cx);
                    restored += 1;
                }
                tracing::info!(restored, "recovered unsaved documents");
                this.status = format!("Restored {restored} unsaved document(s) from recovery.");
                cx.notify();
            });
        }));
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
        self.rebuild_sort_options();
        self.rebuild_references();
        self.publish_list_state(cx);
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
                    problems.push(format!(
                        "The content directory override names unknown collection “{name}”; it was ignored."
                    ));
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
                        problems.push(
                            "The project content directory cannot be applied because the collection bases share no common root."
                                .to_owned(),
                        );
                        continue;
                    };
                    let Some(config_base) = self.config_bases.get(collection) else {
                        problems.push(format!(
                            "Reopen the project to apply the project content directory to {collection}."
                        ));
                        continue;
                    };
                    let Ok(relative) = config_base.strip_prefix(shared) else {
                        problems.push(format!(
                            "The project content directory cannot be applied to {collection} because its base is outside the shared collection root."
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
                Ok(_) => problems.push(format!(
                    "The content directory override “{shown}” for {collection} is not a directory; using the collection base from the Astro configuration."
                )),
                Err(error) => problems.push(format!(
                    "The content directory override “{shown}” for {collection} is invalid ({error}); using the collection base from the Astro configuration."
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

    /// Keep one entry command task alive until it settles.
    ///
    /// Commands never share the index slot, so starting a copy-path, reveal or
    /// another command cannot cancel an in-flight rescan or another command.
    fn retain_command_task(&mut self, task: Task<()>) {
        self.command_tasks.retain(|task| !task.is_ready());
        self.command_tasks.push(task);
    }

    /// Claim the next index request token and mark the index busy.
    ///
    /// Every published index result carries the token of the request that produced
    /// it, so an older watcher scan can never overwrite a newer operation rescan.
    fn begin_index_request(&mut self) -> u64 {
        let next = self
            .index_request
            .checked_add(1)
            .expect("index request sequence exhausted");
        self.index_request = next;
        self.refreshing = true;
        next
    }

    /// Whether a result produced by `token` may publish.
    fn accepts_index_result(&self, token: u64) -> bool {
        token >= self.published_index_request
    }

    /// Clear the busy flag unless a newer index request is still outstanding.
    fn finish_index_request(&mut self, token: u64) {
        if token == self.index_request {
            self.refreshing = false;
        }
    }

    /// Publish a discovered manifest and entry index.
    fn apply_discovered_index(
        &mut self,
        manifest: ProjectManifest,
        entries: Vec<CollectionEntry>,
        cx: &mut Context<Self>,
    ) {
        // A collection discovered after the first open has no override yet, so
        // its manifest base is the pristine configuration base.
        for descriptor in manifest.collections() {
            let name = descriptor.id().as_str();
            if !self.config_bases.contains_key(name) && !self.collection_bases.contains_key(name) {
                self.config_bases
                    .insert(name.to_owned(), descriptor.base().to_path_buf());
            }
        }
        let diagnostics: Vec<String> = manifest.diagnostics().to_vec();
        self.manifest = Some(Arc::new(manifest));
        self.index_error = None;
        self.publish_entries(entries, cx);
        self.reconcile_sessions(cx);
        self.load_data_references(cx);
        self.rebuild_references();
        if !diagnostics.is_empty() {
            self.status = format!("Project diagnostics: {}", diagnostics.join("; "));
        }
    }

    /// Publish an unavailable index while every open buffer is retained.
    fn apply_index_failure(&mut self, error: String, cx: &mut Context<Self>) {
        self.index_error = Some(error);
        self.publish_entries(Vec::new(), cx);
        self.status = "Project index unavailable. Open buffers retained.".into();
    }

    /// Start one index request with the currently applied overrides.
    ///
    /// The returned task publishes only when its result is not older than the
    /// last published index.
    fn start_index_request(&mut self, cx: &mut Context<Self>) -> Option<Task<()>> {
        let manifest = self.manifest.clone()?;
        let generation = self.generation;
        let request = DiscoveryRequest::new(
            manifest.project().clone(),
            generation,
            manifest.root().to_path_buf(),
            self.cancellation.clone(),
        )
        .with_collection_bases(self.collection_bases.clone());
        let watcher = self.watcher_owner.as_ref().map(|owner| owner.weak.clone());
        let token = self.begin_index_request();
        let work = runtime::blocking(cx, move || {
            match inspect_project(&request).map_err(|error| error.to_string())? {
                DiscoveryOutcome::Ready(manifest) => {
                    // Registrations follow the effective bases, not the config file.
                    if let Some(watcher) = watcher.and_then(|weak| weak.upgrade())
                        && let Err(error) = watcher.refresh(&manifest)
                    {
                        tracing::warn!(%error, "collection watcher refresh failed");
                    }
                    let entries = scan_collections(&manifest, request.cancellation())
                        .map_err(|error| error.to_string())?;
                    Ok((manifest, entries))
                }
                DiscoveryOutcome::Unsupported { diagnostics, .. } => Err(diagnostics.join("; ")),
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.apply_index_result(token, result, cx);
                cx.notify();
            });
        });
        Some(task)
    }

    /// Publish one operation rescan result unless a newer index already won.
    fn apply_index_result(
        &mut self,
        token: u64,
        result: Result<(ProjectManifest, Vec<CollectionEntry>), String>,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_index_result(token) {
            tracing::debug!(
                token,
                published = self.published_index_request,
                "dropping a stale index result"
            );
            self.finish_index_request(token);
            return;
        }
        self.published_index_request = token;
        match result {
            Ok((manifest, entries)) => self.apply_discovered_index(manifest, entries, cx),
            Err(error) => {
                tracing::warn!(%error, "post-operation rescan failed; buffers retained");
                self.index_error = Some(error);
                self.status = "Project index unavailable. Open buffers retained.".into();
            }
        }
        self.finish_index_request(token);
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
            (SortMode::Default, SharedString::from("Default")),
            (SortMode::Title, SharedString::from("Title")),
            (SortMode::Filename, SharedString::from("Filename")),
            (SortMode::LastModified, SharedString::from("Last modified")),
        ];
        for date in dates {
            let label = SharedString::from(format!("Date · {date}"));
            options.push((SortMode::DateField(date), label));
        }
        options.push((SortMode::Order, SharedString::from("Order")));
        if !options.iter().any(|(mode, _)| mode == &self.list_sort.mode) {
            self.list_sort.mode = SortMode::Default;
        }
        self.sort_options = options;
    }

    fn sort_label(&self) -> SharedString {
        self.sort_options
            .iter()
            .find(|(mode, _)| mode == &self.list_sort.mode)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| SharedString::from("Default"))
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
    /// Only the scalars are copied: the token ranges stay in the session's one
    /// snapshot instead of being cloned on every render.
    pub(crate) fn active_counts(&self, cx: &App) -> Option<TokenSummary> {
        self.active_session(cx)
            .map(|session| session.read(cx).counts.summary())
    }

    /// The writing modes every retained session applies.
    pub(crate) fn writing_modes(&self) -> WritingModes {
        self.writing
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
        let markdown = session
            .as_ref()
            .is_some_and(|session| session.read(cx).kind() == DocumentKind::Markdown);
        let selection_nonempty = session
            .as_ref()
            .is_some_and(|session| !session.read(cx).editor.read(cx).selected_range().is_empty());
        CommandAvailability {
            project_open: self.manifest.is_some(),
            active_entry: self.active.is_some(),
            busy: self.is_busy(),
            editor_focused: focused,
            markdown_document: markdown,
            selection_nonempty,
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
            self.status = "Formatting is unavailable while this document is not focused.".into();
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
            self.status = format!("Line {line} is outside this document.");
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
            self.status = "Open a document before going to a line.".into();
            cx.notify();
            return;
        };
        let lines = session.read(cx).line_count(cx);
        let owner = cx.weak_entity();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Line number"));
        let focus_input = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let input = input.clone();
            dialog
                .title("Go to line")
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .child(SharedString::from(format!("Document has {lines} lines"))),
                        )
                        .child(Input::new(&input)),
                )
                .footer(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("goto-line-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("goto-line-confirm")
                                .primary()
                                .label("Go")
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
                                                    "Enter a line number to go to.".into();
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

    /// Run one entry command against the active entry or a specific row.
    pub(crate) fn entry_command(
        &mut self,
        command: CommandId,
        target: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let availability = self.command_availability(cx, window);
        let availability = CommandAvailability {
            active_entry: target.is_some() || availability.active_entry,
            ..availability
        };
        if !command.is_available(availability) {
            self.status = format!("{} is unavailable right now.", command.label());
            cx.notify();
            return;
        }
        let target = target.or_else(|| self.active.clone());
        match command {
            CommandId::NewEntry => self.start_new_entry(target.as_deref(), window, cx),
            CommandId::DuplicateEntry => {
                if let Some(path) = target {
                    self.duplicate_entry(path, window, cx);
                }
            }
            CommandId::RenameEntry => {
                if let Some(path) = target {
                    self.open_rename_dialog(path, window, cx);
                }
            }
            CommandId::DeleteEntry => {
                if let Some(path) = target {
                    self.request_trash(path, window, cx);
                }
            }
            CommandId::RevealEntry => {
                if let Some(path) = target {
                    self.reveal_entry(path, window, cx);
                }
            }
            CommandId::CopyPathAbsolute => {
                if let Some(path) = target {
                    self.copy_entry_path(path, PathForm::Absolute, window, cx);
                }
            }
            CommandId::CopyPathRelative => {
                if let Some(path) = target {
                    self.copy_entry_path(path, PathForm::Relative, window, cx);
                }
            }
            _ => {}
        }
    }

    fn report_entry_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.status = message;
        cx.notify();
    }

    fn operation_scope(&self) -> Option<ProjectScope> {
        self.manifest
            .as_ref()
            .map(|manifest| ProjectScope::new(manifest.root().to_path_buf(), self.generation))
    }

    fn source_path(&self, path: &Path) -> Option<SourcePath> {
        let root = self.manifest.as_ref()?.root();
        relative_source(root, path)
    }

    fn collection_of(&self, path: &Path) -> Option<String> {
        self.session_collections.get(path).cloned().or_else(|| {
            self.entries
                .iter()
                .find(|entry| entry.absolute_source() == path)
                .map(|entry| entry.entry().id().collection().as_str().to_owned())
        })
    }

    fn needs_save(&self, path: &Path, cx: &App) -> bool {
        self.sessions
            .get(path)
            .is_some_and(|session| session.read(cx).is_dirty())
    }

    fn start_new_entry(
        &mut self,
        target: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            self.status = "Wait for the project index to finish before creating an entry.".into();
            cx.notify();
            return;
        }
        let Some(manifest) = self.manifest.clone() else {
            return;
        };
        let root = manifest.root().to_path_buf();
        let target_entry = target
            .and_then(|path| {
                self.entries
                    .iter()
                    .find(|entry| entry.absolute_source() == path)
            })
            .or_else(|| {
                self.active.as_ref().and_then(|active| {
                    self.entries
                        .iter()
                        .find(|entry| entry.absolute_source() == active)
                })
            });
        let Some((collection, descriptor)) = target_entry
            .map(|entry| {
                let name = entry.entry().id().collection().as_str();
                manifest
                    .collections()
                    .iter()
                    .find(|descriptor| descriptor.id().as_str() == name)
                    .map(|descriptor| (descriptor.id().clone(), descriptor))
            })
            .unwrap_or_else(|| {
                manifest
                    .collections()
                    .first()
                    .map(|descriptor| (descriptor.id().clone(), descriptor))
            })
        else {
            self.status =
                "Open an Astro project with a content collection to create an entry.".into();
            cx.notify();
            return;
        };
        // The manifest's collection base is the resolved directory: discovery
        // applies the collection's resolved content-directory override (P1) before
        // scanning, so creation, the file list and the watcher all agree on it.
        let directory_absolute = target_entry
            .and_then(|entry| entry.absolute_source().parent())
            .filter(|parent| parent.starts_with(descriptor.base()))
            .unwrap_or(descriptor.base());
        let directory = relative_source(&root, directory_absolute);
        let resolved = self.resolved_for(Some(collection.as_str()));
        let extension = resolved.default_extension;
        let probe = Path::new("entry").with_extension(extension.as_str());
        if descriptor.pattern().entry_kind(&probe).is_none() {
            self.status = format!(
                "The {} collection does not include .{} entries; change the default extension.",
                collection.as_str(),
                extension.as_str()
            );
            cx.notify();
            return;
        }
        let today = today();
        let plan = operations::creation_plan(
            self.schemas.get(collection.as_str()).map(Arc::as_ref),
            collection.as_str(),
            &resolved.field_mappings,
            &today,
        );
        if !plan.unsupported.is_empty() {
            self.status = format!(
                "Cannot create a {} entry: required field(s) {} are containers the create form cannot represent. Nothing was written.",
                collection.as_str(),
                plan.unsupported.join(", ")
            );
            cx.notify();
            return;
        }
        let inputs = plan.inputs.clone();
        self.pending_create = Some(PendingCreate {
            collection,
            directory,
            extension,
            fields: plan.fields,
            today,
        });
        if inputs.is_empty() {
            self.submit_create(None, Vec::new(), window, cx);
        } else {
            self.open_create_dialog(inputs, window, cx);
        }
    }

    /// Human hint for one pre-write creation input.
    fn creation_hint(&self, input: &CreationInput) -> Option<SharedString> {
        match &input.kind {
            CreationInputKind::Date => Some(SharedString::from("YYYY-MM-DD")),
            CreationInputKind::Image => Some(SharedString::from(
                "Project-relative image path; external copy arrives with the asset service",
            )),
            CreationInputKind::ImageList => Some(SharedString::from(
                "Comma-separated project-relative image paths",
            )),
            CreationInputKind::Tags => Some(SharedString::from("Comma-separated values")),
            CreationInputKind::Reference { collections } => Some(self.reference_hint(collections)),
            CreationInputKind::ReferenceList { collections } => Some(SharedString::from(format!(
                "Comma-separated ids · {}",
                self.reference_hint(collections)
            ))),
            CreationInputKind::Enum(_) => None,
            CreationInputKind::Text
            | CreationInputKind::Number
            | CreationInputKind::Integer
            | CreationInputKind::Boolean => None,
        }
    }

    /// List available reference ids and labels for a creation input.
    fn reference_hint(&self, collections: &[String]) -> SharedString {
        let mut parts = Vec::new();
        for collection in collections {
            let Some(options) = self.references.get(collection) else {
                parts.push(format!("{collection}: no entries"));
                continue;
            };
            let labels = options
                .iter()
                .take(12)
                .map(|option| format!("{} ({})", option.label, option.value))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("{collection}: {labels}"));
        }
        if parts.is_empty() {
            SharedString::from("Reference id")
        } else {
            SharedString::from(parts.join(" · "))
        }
    }

    fn open_create_dialog(
        &mut self,
        inputs: Vec<CreationInput>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_create.as_ref() else {
            return;
        };
        let collection = pending.collection.as_str().to_owned();
        let directory = pending
            .directory
            .as_ref()
            .map(|directory| directory.as_str().to_owned())
            .unwrap_or_else(|| "(project root)".to_owned());
        let extension = pending.extension.suffix().to_owned();
        let default_name = pending.today.clone();
        let owner = cx.weak_entity();
        let name_input =
            cx.new(|cx| InputState::new(window, cx).default_value(default_name.clone()));
        let rows: Vec<(CreationInput, Entity<InputState>, Option<SharedString>)> = inputs
            .into_iter()
            .map(|input| {
                let initial = operations::input_text(&input, &default_name);
                let state = cx.new(|cx| InputState::new(window, cx).default_value(initial));
                let hint = self.creation_hint(&input);
                (input, state, hint)
            })
            .collect();
        window.open_dialog(cx, move |dialog, _, cx| {
            let owner = owner.clone();
            let name_input = name_input.clone();
            let rows = rows.clone();
            let default_name = default_name.clone();
            let mut body = div()
                .v_flex()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!(
                            "{collection} · {directory} · {extension}"
                        ))),
                )
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(div().text_sm().child("File name"))
                        .child(Input::new(&name_input)),
                );
            for (input, state, hint) in &rows {
                let mut row = div()
                    .v_flex()
                    .gap_1()
                    .child(div().text_sm().child(input.name.clone()))
                    .child(Input::new(state));
                if let Some(hint) = hint {
                    row = row.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(hint.clone()),
                    );
                }
                body = body.child(row);
            }
            dialog.title("New entry").child(body).footer(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("create-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("create-confirm")
                            .primary()
                            .label("Create")
                            .on_click(move |_, window, cx| {
                                let typed_name = name_input.read(cx).value().to_string();
                                let mut values = Vec::with_capacity(rows.len());
                                for (input, state, _) in &rows {
                                    let raw = state.read(cx).value().to_string();
                                    match operations::parse_input(input, &raw) {
                                        Ok(value) => values.push((input.name.clone(), value)),
                                        Err(error) => {
                                            let _ = owner.update(cx, |project, cx| {
                                                project.pending_create = None;
                                                project.report_entry_error(error, cx)
                                            });
                                            window.close_dialog(cx);
                                            return;
                                        }
                                    }
                                }
                                let name = if typed_name.trim() == default_name {
                                    None
                                } else {
                                    Some(typed_name.trim().to_owned())
                                };
                                let _ = owner.update(cx, |project, cx| {
                                    project.submit_create(name, values, window, cx)
                                });
                                window.close_dialog(cx);
                            }),
                    ),
            )
        });
    }

    /// Write the pending create request with the resolved fields and inputs.
    fn submit_create(
        &mut self,
        name: Option<String>,
        values: Vec<(String, ScalarValue)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_create.take() else {
            return;
        };
        let mut fields = pending.fields;
        for (name, value) in values {
            if !fields.iter().any(|field| field.name == name) {
                fields.push(CreationField { name, value });
            }
        }
        let entry_name = match name {
            None => NewEntryName::Date(pending.today),
            Some(name) => NewEntryName::Explicit(name),
        };
        let request = CreateRequest {
            generation: self.generation,
            collection: pending.collection,
            directory: pending.directory,
            extension: pending.extension,
            name: entry_name,
            fields,
        };
        let Some(scope) = self.operation_scope() else {
            return;
        };
        let generation = self.generation;
        self.status = "Creating entry…".into();
        cx.notify();
        let work = runtime::blocking(cx, move || operations::create(&scope, &request));
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(created) => {
                        tracing::info!(source = %created.source().as_str(), "entry created");
                        this.status = format!("Created {}.", created.source().as_str());
                        this.rescan(Some(created.source().clone()), window, cx);
                    }
                    Err(error) => {
                        this.status = format!("Could not create the entry: {error}");
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn open_rename_dialog(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.source_path(&path) else {
            return;
        };
        let owner = cx.weak_entity();
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(source.as_str().to_owned()));
        window.open_dialog(cx, move |dialog, _, cx| {
            let owner = owner.clone();
            let input = input.clone();
            let path = path.clone();
            dialog
                .title("Rename or move entry")
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Project-relative path"),
                        )
                        .child(Input::new(&input)),
                )
                .footer(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("rename-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rename-confirm")
                                .primary()
                                .label("Rename")
                                .on_click(move |_, window, cx| {
                                    let destination = input.read(cx).value().to_string();
                                    window.close_dialog(cx);
                                    let _ = owner.update(cx, |project, cx| {
                                        project.submit_rename(path.clone(), destination, window, cx)
                                    });
                                }),
                        ),
                )
        });
    }

    fn submit_rename(
        &mut self,
        path: PathBuf,
        destination: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = destination.trim();
        let destination = match SourcePath::new(destination) {
            Ok(destination) => destination,
            Err(_) => {
                self.report_entry_error(
                    format!("“{destination}” is not a valid project-relative path."),
                    cx,
                );
                return;
            }
        };
        let Some(current) = self.source_path(&path) else {
            return;
        };
        if current == destination {
            self.status = "The entry path is unchanged.".into();
            cx.notify();
            return;
        }
        if let Some(extension) = destination
            .as_str()
            .rsplit_once('.')
            .map(|(_, extension)| extension)
        {
            let accepts = self
                .collection_of(&path)
                .and_then(|collection| {
                    self.manifest.as_ref().and_then(|manifest| {
                        manifest
                            .collections()
                            .iter()
                            .find(|descriptor| descriptor.id().as_str() == collection)
                    })
                })
                .is_some_and(|descriptor| {
                    descriptor
                        .pattern()
                        .entry_kind(Path::new(&format!("entry.{extension}")))
                        .is_some()
                });
            if !accepts {
                self.report_entry_error(
                    format!(
                        "This collection does not include .{extension} entries; the file would not be indexed."
                    ),
                    cx,
                );
                return;
            }
        }
        if self.needs_save(&path, cx) {
            self.pending_after_save = Some(PendingAfterSave::Rename {
                from: path.clone(),
                to: destination,
            });
            self.save_path(path, window, cx);
        } else {
            self.perform_rename(path, destination, window, cx);
        }
    }

    fn perform_rename(
        &mut self,
        from: PathBuf,
        destination: SourcePath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.operation_scope() else {
            return;
        };
        let Some(source) = self.source_path(&from) else {
            return;
        };
        let generation = self.generation;
        let request = MoveRequest {
            generation,
            source,
            destination,
        };
        self.status = "Renaming entry…".into();
        cx.notify();
        let work = runtime::blocking(cx, move || operations::rename(&scope, &request));
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(moved) => {
                        let root = this
                            .manifest
                            .as_ref()
                            .map(|manifest| manifest.root().to_path_buf());
                        let mut dirty = false;
                        if let Some(root) = root {
                            let to = root.join(moved.to().as_str());
                            this.retarget_session(&from, &to, moved.to().clone(), window, cx);
                            dirty = this
                                .sessions
                                .get(&to)
                                .is_some_and(|session| session.read(cx).buffer.is_dirty());
                        }
                        this.status = if dirty {
                            format!(
                                "Renamed to {}. Newer edits remain unsaved; they are recorded for recovery.",
                                moved.to().as_str()
                            )
                        } else {
                            format!("Renamed to {}.", moved.to().as_str())
                        };
                        this.rescan(None, window, cx);
                    }
                    Err(error) => {
                        this.status = format!("Could not rename the entry: {error}");
                        cx.notify();
                    }
                }
            });
        }));
    }

    /// Move one retained session, tab, recovery key and selection to a new source.
    fn retarget_session(
        &mut self,
        from: &Path,
        to: &Path,
        source: SourcePath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.sessions.remove(from) else {
            return;
        };
        let identity = session.read(cx).buffer.identity().clone();
        if let Some(store) = self.recovery.clone() {
            let stale = identity.clone();
            let task = runtime::blocking(cx, move || {
                let _ = store.clear(&stale);
            });
            task.detach();
        }
        let retargeted = DocumentIdentity::new(
            identity.project().clone(),
            identity.project_generation(),
            source,
        );
        session.update(cx, |session, cx| session.retarget(retargeted, window, cx));
        if let Some(subscription) = self.session_subscriptions.remove(from) {
            self.session_subscriptions
                .insert(to.to_path_buf(), subscription);
        }
        if let Some(collection) = self.session_collections.remove(from) {
            self.session_collections
                .insert(to.to_path_buf(), collection);
        }
        self.sessions.insert(to.to_path_buf(), session.clone());
        for path in &mut self.tab_order {
            if path == from {
                *path = to.to_path_buf();
            }
        }
        if self.active.as_deref() == Some(from) {
            self.active = Some(to.to_path_buf());
        }
        self.sync_tabs(cx);
        self.refresh_entry_selection(cx);
        let dirty = session.read(cx).buffer.is_dirty();
        if self.active.as_deref() == Some(to) {
            self.install(to, session, cx);
        }
        tracing::debug!(
            from = %from.display(),
            to = %to.display(),
            "retargeted a retained session"
        );
        // A dirty buffer keeps its newer edits recoverable under the new source;
        // the stale record for the old identity was cleared above.
        if dirty {
            self.schedule_recovery(cx);
        }
    }

    fn duplicate_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.needs_save(&path, cx) {
            self.pending_after_save = Some(PendingAfterSave::Duplicate { path: path.clone() });
            self.save_path(path, window, cx);
        } else {
            self.perform_duplicate(path, window, cx);
        }
    }

    fn perform_duplicate(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.operation_scope() else {
            return;
        };
        let Some(source) = self.source_path(&path) else {
            return;
        };
        let generation = self.generation;
        let request = DuplicateRequest { generation, source };
        self.status = "Duplicating entry…".into();
        cx.notify();
        let work = runtime::blocking(cx, move || operations::duplicate(&scope, &request));
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(duplicated) => {
                        this.status = format!("Duplicated as {}.", duplicated.source().as_str());
                        this.rescan(Some(duplicated.source().clone()), window, cx);
                    }
                    Err(error) => {
                        this.status = format!("Could not duplicate the entry: {error}");
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn request_trash(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let dirty = self.needs_save(&path, cx);
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            if dirty {
                let discard_owner = owner.clone();
                let discard_path = path.clone();
                let save_owner = owner.clone();
                let save_path = path.clone();
                dialog
                    .title(format!("Move “{title}” to the Trash?"))
                    .child("The document has unsaved changes. Save them first, or discard them and move the file to the Trash.")
                    .footer(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("trash-cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("trash-discard")
                                    .danger()
                                    .label("Discard changes and trash")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = discard_owner.update(cx, |project, cx| {
                                            project.discard_and_trash(
                                                discard_path.clone(),
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            )
                            .child(
                                Button::new("trash-save")
                                    .primary()
                                    .label("Save and trash")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = save_owner.update(cx, |project, cx| {
                                            project.save_and_trash(
                                                save_path.clone(),
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            ),
                    )
            } else {
                let owner = owner.clone();
                let path = path.clone();
                dialog
                    .title(format!("Move “{title}” to the Trash?"))
                    .child("The file moves to the desktop Trash; it is never permanently deleted here.")
                    .footer(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("trash-cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("trash-confirm")
                                    .danger()
                                    .label("Move to Trash")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = owner.update(cx, |project, cx| {
                                            project.perform_trash(path.clone(), window, cx)
                                        });
                                    }),
                            ),
                    )
            }
        });
    }

    fn save_and_trash(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.needs_save(&path, cx) {
            self.pending_after_save = Some(PendingAfterSave::Trash { path: path.clone() });
            self.save_path(path, window, cx);
        } else {
            self.perform_trash(path, window, cx);
        }
    }

    fn discard_and_trash(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.remove_session(&path, cx);
        self.perform_trash(path, window, cx);
    }

    fn perform_trash(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.operation_scope() else {
            return;
        };
        let Some(source) = self.source_path(&path) else {
            return;
        };
        let generation = self.generation;
        let backend = self.trash_backend.clone();
        let request = TrashRequest { generation, source };
        self.status = "Moving entry to the Trash…".into();
        cx.notify();
        let work = runtime::blocking(cx, move || {
            operations::trash(&scope, &request, backend.as_ref())
        });
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(trashed) => {
                        this.remove_session(&path, cx);
                        this.status = format!("Moved {} to the Trash.", trashed.source().as_str());
                        this.rescan(None, window, cx);
                    }
                    Err(error) => {
                        this.status = format!(
                            "Could not move the entry to the Trash: {error}. The file is unchanged."
                        );
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn reveal_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.operation_scope() else {
            return;
        };
        let Some(source) = self.source_path(&path) else {
            return;
        };
        let generation = self.generation;
        let request = RevealRequest { generation, source };
        let work = runtime::blocking(cx, move || operations::reveal_target(&scope, &request));
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, _window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(target) => {
                        let directory = target.directory().to_path_buf();
                        this.status = format!("Opening {}…", directory.display());
                        cx.notify();
                        let opened =
                            runtime::spawn(cx, async move { open_directory(&directory).await });
                        cx.spawn(async move |this, cx| {
                            let result = opened.await.and_then(|result| result);
                            let _ = this.update(cx, |this, cx| {
                                this.status = match result {
                                    Ok(()) => "Opened the containing folder.".into(),
                                    Err(error) => {
                                        format!("Could not open the file manager: {error}")
                                    }
                                };
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    Err(error) => {
                        this.status = format!("Could not reveal the entry: {error}");
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn copy_entry_path(
        &mut self,
        path: PathBuf,
        form: PathForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.operation_scope() else {
            return;
        };
        let Some(source) = self.source_path(&path) else {
            return;
        };
        let generation = self.generation;
        let request = CopyPathRequest {
            generation,
            source,
            form,
        };
        let work = runtime::blocking(cx, move || operations::copy_path(&scope, &request));
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work
                .await
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update_in(cx, |this, _window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(copied) => {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                            copied.text().to_owned(),
                        ));
                        this.status = format!("Copied {}.", copied.text());
                    }
                    Err(error) => {
                        this.status = format!("Could not copy the path: {error}");
                    }
                }
                cx.notify();
            });
        }));
    }

    fn run_after_save(
        &mut self,
        action: PendingAfterSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PendingAfterSave::Rename { from, to } => self.perform_rename(from, to, window, cx),
            PendingAfterSave::Duplicate { path } => self.perform_duplicate(path, window, cx),
            PendingAfterSave::Trash { path } => self.perform_trash(path, window, cx),
        }
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
            self.status = format!(
                "{} was written but is not indexed by its collection pattern.",
                source.as_str()
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
        self.list.update(cx, |list, cx| {
            let delegate = list.delegate_mut();
            delegate.entries = entries;
            delegate.root = root;
            delegate.active = active;
            delegate.mappings = mappings;
            delegate.filter = filter;
            delegate.sort = sort;
            delegate.rebuild();
            delegate.selected = delegate
                .selected_row()
                .map(|(section, row)| IndexPath::new(row).section(section));
            cx.notify();
        });
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
            .map(|row| row.filename.to_string())
            .collect()
    }

    fn visible_entry_count(&self, cx: &App) -> usize {
        self.list.read(cx).delegate().visible_count()
    }
}

impl Render for ProjectWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_project = self.manifest.is_some();
        let loading = self.loading;
        let availability = self.command_availability(cx, window);
        let visible = self.visible_entry_count(cx);
        let total = self.entries.len();
        let query = self.last_list_query.trim().to_owned();
        let drafts_only = self.list_filter.drafts_only;
        let filter_active = drafts_only || !query.is_empty();
        let sort_label = self.sort_label();
        let default_sort = self.list_sort.mode == SortMode::Default;
        let sort_options = self.sort_options.clone();
        let current_mode = self.list_sort.mode.clone();
        let direction_icon = match self.list_sort.direction {
            SortDirection::Ascending => IconName::ArrowUp,
            SortDirection::Descending => IconName::ArrowDown,
        };
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

        let mut view = view.child(
            div()
                .v_flex()
                .border_b_1()
                .border_color(cx.theme().window_border)
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .py_1()
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(SharedString::from(format!(
                                            "{visible}/{total} entries"
                                        ))),
                                )
                                .when(filter_active, |row| {
                                    row.child(
                                        div()
                                            .id("filter-indicator")
                                            .test_support()
                                            .h_flex()
                                            .items_center()
                                            .gap_1()
                                            .px_1()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(cx.theme().accent)
                                            .text_xs()
                                            .text_color(cx.theme().accent)
                                            .child(SharedString::from(filter_label(
                                                drafts_only,
                                                &query,
                                            ))),
                                    )
                                }),
                        )
                        .child(
                            Button::new("open-project")
                                .xsmall()
                                .ghost()
                                .icon(IconName::FolderOpen)
                                .tooltip("Open project…")
                                .disabled(loading)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.prompt(window, cx)),
                                ),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .pb_1()
                        .child(entry_toolbar_button(
                            "entry-new",
                            CommandId::NewEntry,
                            IconName::FilePlus,
                            availability,
                            cx,
                        ))
                        .child(entry_toolbar_button(
                            "entry-duplicate",
                            CommandId::DuplicateEntry,
                            IconName::Copy,
                            availability,
                            cx,
                        ))
                        .child(entry_toolbar_button(
                            "entry-rename",
                            CommandId::RenameEntry,
                            IconName::SquarePen,
                            availability,
                            cx,
                        ))
                        .child(entry_toolbar_button(
                            "entry-delete",
                            CommandId::DeleteEntry,
                            IconName::Trash,
                            availability,
                            cx,
                        ))
                        .child(entry_toolbar_button(
                            "entry-reveal",
                            CommandId::RevealEntry,
                            IconName::FolderOpen,
                            availability,
                            cx,
                        ))
                        .child(copy_path_button(availability, cx))
                        .child(div().flex_1())
                        .child(
                            Button::new("filter-drafts")
                                .xsmall()
                                .ghost()
                                .icon(IconName::ListFilter)
                                .tooltip("Show drafts only")
                                .toggled(drafts_only)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.toggle_drafts_only(cx)),
                                ),
                        )
                        .child({
                            let owner = cx.weak_entity();
                            let options = sort_options;
                            let current = current_mode.clone();
                            Button::new("sort-mode")
                                .xsmall()
                                .ghost()
                                .icon(IconName::ArrowUpDown)
                                .label(sort_label)
                                .tooltip("Sort entries")
                                .dropdown_menu(move |menu, _, _| {
                                    let mut menu = menu;
                                    for (mode, label) in options.iter().cloned() {
                                        let owner = owner.clone();
                                        let selected = mode == current;
                                        let chosen = mode.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(label).checked(selected).on_click(
                                                move |_, _, cx| {
                                                    let _ = owner.update(cx, |project, cx| {
                                                        project.set_sort_mode(chosen.clone(), cx)
                                                    });
                                                },
                                            ),
                                        );
                                    }
                                    menu
                                })
                        })
                        .child(
                            Button::new("sort-direction")
                                .xsmall()
                                .ghost()
                                .icon(direction_icon)
                                .tooltip("Reverse sort direction")
                                .disabled(default_sort)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.toggle_sort_direction(cx)),
                                ),
                        )
                        .when(filter_active, |row| {
                            row.child(
                                Button::new("clear-filter")
                                    .xsmall()
                                    .ghost()
                                    .label("Clear")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_filter(window, cx)
                                    })),
                            )
                        }),
                ),
        );

        if loading {
            view = view.child(project_loading_state(cx));
        } else if let Some(error) = index_error {
            view =
                view.child(
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
                                .child("Project index unavailable"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(error),
                        )
                        .child(Button::new("retry-index").small().label("Retry").on_click(
                            cx.listener(|this, _, window, cx| this.retry_index(window, cx)),
                        )),
                );
        } else if visible == 0 {
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
                    .child(div().text_sm().child("No entries match the filter"))
                    .child(
                        div()
                            .text_xs()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child("Change the search text or clear the draft filter."),
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
                    .child(div().text_sm().child("No entries"))
                    .child(
                        div()
                            .text_xs()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child("This project's collections contain no Markdown or MDX files."),
                    )
            });
        } else {
            view = view.child(
                div().flex_1().min_h_0().child(
                    List::new(&self.list)
                        .small()
                        .search_placeholder("Filter by file or title"),
                ),
            );
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

/// The initial-scan state, distinct from the empty and error states.
fn project_loading_state(cx: &mut Context<ProjectWorkspace>) -> impl IntoElement {
    div()
        .id("project-loading")
        .test_support()
        .v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(
            Icon::new(IconName::RotateCcw)
                .size_6()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().text_sm().child("Scanning collections…"))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("The project index is loading."),
        )
}

/// The failed-open state: distinct from "never opened" and from an index error.
fn project_open_error_state(message: &str, cx: &mut Context<ProjectWorkspace>) -> impl IntoElement {
    div()
        .id("project-open-error")
        .test_support()
        .v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(
            Icon::new(IconName::TriangleAlert)
                .size_8()
                .text_color(cx.theme().danger),
        )
        .child(div().text_sm().child("Could not open project"))
        .child(
            div()
                .text_xs()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(message.to_owned())),
        )
        .child(
            Button::new("retry-open-project")
                .small()
                .label("Choose another project…")
                .on_click(cx.listener(|this, _, window, cx| this.prompt(window, cx))),
        )
}

/// The no-project state.
fn project_empty_state(loading: bool, cx: &mut Context<ProjectWorkspace>) -> impl IntoElement {
    div()
        .id("project-empty")
        .test_support()
        .v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(
            Icon::new(IconName::FolderOpen)
                .size_8()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().text_sm().child("No project open"))
        .child(
            div()
                .text_xs()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child("Open an Astro project to browse its content collections."),
        )
        .child(
            Button::new("open-project")
                .small()
                .label("Open project…")
                .disabled(loading)
                .on_click(cx.listener(|this, _, window, cx| this.prompt(window, cx))),
        )
}

/// One toolbar button that dispatches a registry command with shared availability.
fn entry_toolbar_button(
    id: &'static str,
    command: CommandId,
    icon: IconName,
    availability: CommandAvailability,
    cx: &mut Context<ProjectWorkspace>,
) -> Button {
    Button::new(id)
        .xsmall()
        .ghost()
        .icon(icon)
        .tooltip(command.label())
        .disabled(!command.is_available(availability))
        .on_click(
            cx.listener(move |this, _, window, cx| this.entry_command(command, None, window, cx)),
        )
}

/// The copy-path control offers both forms through the same command IDs.
fn copy_path_button(
    availability: CommandAvailability,
    cx: &mut Context<ProjectWorkspace>,
) -> impl IntoElement {
    let owner = cx.weak_entity();
    let available = CommandId::CopyPathAbsolute.is_available(availability);
    Button::new("copy-path")
        .xsmall()
        .ghost()
        .icon(IconName::Copy)
        .tooltip("Copy path")
        .disabled(!available)
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu;
            for (command, label) in [
                (CommandId::CopyPathAbsolute, "Absolute path"),
                (CommandId::CopyPathRelative, "Project-relative path"),
            ] {
                let owner = owner.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |project, cx| {
                        project.entry_command(command, None, window, cx)
                    });
                }));
            }
            menu
        })
}

fn filter_label(drafts_only: bool, query: &str) -> String {
    match (drafts_only, query.is_empty()) {
        (true, true) => "Drafts only".to_owned(),
        (true, false) => format!("Drafts · “{query}”"),
        (false, false) => format!("“{query}”"),
        (false, true) => String::new(),
    }
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
    return Err("opening a file manager is unsupported on this platform".into());
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
            Err(format!("the file manager exited with {status}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::SourcePath;
    use astrolith_core::preferences::CollectionPreferencesV1;
    use gpui_kit::{TestAppContext, WindowHandle, WindowOptions, base::Root, test::TestWindowExt};
    use std::sync::Mutex;
    fn fixture() -> tempfile::TempDir {
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
    fn mount(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<ProjectWorkspace>) {
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
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&first).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), "# Changed\n");
            assert!(session.is_dirty());
        });
        let next = fixture();
        let generation = view.read_with(cx, |view, _| view.generation);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(next.path().into(), window, cx))
        })
        .unwrap();
        assert_eq!(view.read_with(cx, |view, _| view.generation), generation);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/posts/a.md")).unwrap(),
            "# A\n"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.close_active(window, cx));
            assert!(window.has_active_dialog(cx));
            window.press("escape", cx);
        })
        .unwrap();
        assert_eq!(view.read_with(cx, |view, _| view.sessions.len()), 2);
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
    async fn newer_index_rejects_late_first_load_and_readonly_still_mounts(
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
            view.update(cx, |view, cx| {
                let manifest = view.manifest.as_ref().unwrap();
                let entry = &view.entries[0];
                let path = entry.absolute_source().to_owned();
                let identity = DocumentIdentity::new(
                    manifest.project().clone(),
                    view.generation,
                    entry.entry().source().clone(),
                );
                let ticket = DocumentLoadTicket::new(identity, DocumentRevision::INITIAL);
                let revision = view.index_revision;
                view.publish_entries(Vec::new(), cx);
                view.finish_load(
                    ticket,
                    revision,
                    path,
                    Ok("obsolete text".into()),
                    window,
                    cx,
                );
                assert!(
                    view.sessions.is_empty(),
                    "an obsolete index result cannot create a tab"
                );
                let readonly = crate::file::DocumentSource {
                    path: dir.path().join("src/posts/a.md"),
                    source: "# Readonly\n".into(),
                };
                view.document
                    .update(cx, |panel, cx| panel.show_source(readonly, window, cx));
                assert_eq!(
                    view.document
                        .read(cx)
                        .editor
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value(),
                    "# Readonly\n"
                );
                assert!(!view.document.read(cx).is_editable());
            })
        })
        .unwrap();
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
    #[gpui_kit::test]
    async fn newest_project_wins_and_duplicate_source_loads_coalesce(cx: &mut TestAppContext) {
        let a = fixture();
        let b = fixture();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.start(a.path().into(), window, cx);
                view.start(b.path().into(), window, cx);
            })
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        assert_eq!(
            view.read_with(cx, |view, _| view
                .manifest
                .as_ref()
                .unwrap()
                .root()
                .to_owned()),
            b.path().canonicalize().unwrap()
        );
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_entry(0, window, cx);
                view.open_entry(0, window, cx);
                assert_eq!(view.loads.len(), 1);
            })
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        assert_eq!(view.read_with(cx, |view, _| view.sessions.len()), 1);
    }

    async fn edit_active(
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

    #[gpui_kit::test]
    async fn saving_the_active_document_writes_disk_and_clears_dirty(cx: &mut TestAppContext) {
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
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());

        edit_active(cx, handle, &view, "# Saved\n").await;
        assert!(view.read_with(cx, |view, cx| {
            view.sessions.get(&path).unwrap().read(cx).is_dirty()
        }));

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if std::fs::read_to_string(&path).unwrap() == "# Saved\n" {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Saved\n");
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(!session.buffer.is_dirty());
            assert!(!view.has_dirty(cx));
            assert!(view.status.contains("Saved"));
        });
    }

    #[gpui_kit::test]
    async fn save_conflict_preserves_disk_and_keeps_edits(cx: &mut TestAppContext) {
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
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());

        std::fs::write(&path, "# External\n").unwrap();
        edit_active(cx, handle, &view, "# Mine\n").await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.status.contains("conflict")) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# External\n");
        view.read_with(cx, |view, cx| {
            assert!(view.status.contains("conflict"));
            assert!(view.sessions.get(&path).unwrap().read(cx).buffer.is_dirty());
        });
    }

    #[gpui_kit::test]
    async fn dirty_edits_are_recorded_for_recovery_and_cleared_after_save(cx: &mut TestAppContext) {
        let dir = fixture();
        let recovery_dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery_dir.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
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
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());

        edit_active(cx, handle, &view, "# Recovered\n").await;
        for _ in 0..300 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if !store.load().unwrap().records().is_empty() {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        let loaded = store.load().unwrap();
        assert_eq!(loaded.records().len(), 1, "dirty edit must be recorded");
        assert_eq!(loaded.records()[0].content(), "# Recovered\n");

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            if store.load().unwrap().records().is_empty()
                && std::fs::read_to_string(&path).unwrap() == "# Recovered\n"
            {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(
            store.load().unwrap().records().is_empty(),
            "recovery record must be cleared after a successful save"
        );
    }

    #[gpui_kit::test]
    async fn startup_auto_applies_pending_recovery_and_never_overwrites_disk(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let recovery_dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery_dir.path());
        let identity = DocumentIdentity::new(
            ProjectId::new("workspace-1").unwrap(),
            1,
            SourcePath::new("src/posts/a.md").unwrap(),
        );
        store
            .save(&RecoveryRecord::new(
                identity,
                DocumentRevision::new(1),
                "# Recovered\n",
                1,
            ))
            .unwrap();

        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;

        let path = dir.path().join("src/posts/a.md");
        for _ in 0..300 {
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
        view.read_with(cx, |view, cx| {
            let session = view
                .sessions
                .get(&path)
                .expect("pending recovery must auto-apply at project open");
            let session = session.read(cx);
            assert_eq!(session.editor.read(cx).value(), "# Recovered\n");
            assert_eq!(session.buffer.original_text(), "# A\n");
            assert!(session.buffer.is_dirty());
            assert!(
                view.pending_recovery.is_empty(),
                "auto-applied records are consumed"
            );
            assert!(
                view.status.contains("Restored"),
                "status should report the restore, was {:?}",
                view.status
            );
        });
        // Auto-apply must never write the recovered text over the original file.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# A\n");
        // The record survives until its content is saved or discarded.
        assert_eq!(store.load().unwrap().records().len(), 1);

        // The manual command stays safe and idempotent after auto-apply.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.recover_pending(window, cx))
        })
        .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.status.contains("No recovery records"));
            assert_eq!(
                view.sessions.len(),
                1,
                "manual recover must not duplicate an auto-applied session"
            );
        });
    }

    #[gpui_kit::test]
    async fn quitting_flushes_unsaved_debounced_content_to_recovery(cx: &mut TestAppContext) {
        let dir = fixture();
        let recovery_dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery_dir.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
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

        // A dirty edit whose 500 ms recovery debounce has not elapsed yet.
        edit_active(cx, handle, &view, "# Quit flush\n").await;
        assert!(
            store.load().unwrap().records().is_empty(),
            "debounce must not have written before the exit flush"
        );

        // The native-quit observer flushes the latest dirty text despite the debounce.
        let flush = view
            .update(cx, |view, cx| view.flush_recovery_on_quit(cx))
            .expect("a dirty session must produce an exit flush");
        flush.await.unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.records().len(), 1, "exit must flush unsaved content");
        assert_eq!(loaded.records()[0].content(), "# Quit flush\n");
        // The flush must not write the recovered text over the original file.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/posts/a.md")).unwrap(),
            "# A\n"
        );
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
    async fn superseded_schema_generation_results_never_reach_the_form(cx: &mut TestAppContext) {
        let (_handle, view) = mount(cx);
        view.update(cx, |view, cx| {
            let SchemaRefresh::Regenerate { request: first } = view.schema_freshness.reconcile(1)
            else {
                panic!("the first fingerprint must regenerate");
            };
            let SchemaRefresh::Regenerate { request: latest } = view.schema_freshness.reconcile(2)
            else {
                panic!("a changed fingerprint must regenerate");
            };
            assert_ne!(first, latest);

            let schema_for = |name: &str| {
                schema(vec![required_field(
                    name,
                    FieldKind::String { format: None },
                )])
            };
            assert!(
                !view.apply_generated_schemas(
                    first,
                    1,
                    vec![("blog".to_owned(), schema_for("stale"))],
                    cx
                ),
                "a superseded report must be dropped"
            );
            assert!(
                view.schemas.is_empty(),
                "a stale schema must never reach the panel"
            );
            assert_eq!(view.schema_revision, 0);

            assert!(view.apply_generated_schemas(
                latest,
                2,
                vec![("blog".to_owned(), schema_for("title"))],
                cx
            ));
            assert!(view.schemas.contains_key("blog"));
            assert_eq!(view.schema_revision, 1);
        });
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
    fn bare_session(
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
    fn replace_visible(
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
    async fn retargeting_a_renamed_source_reprojects_its_kind(cx: &mut TestAppContext) {
        // The retention rule hides the ESM prelude for MDX only, so a rename that
        // crosses `.mdx` ↔ `.md` must re-project the retained session.
        let source = "---\ntitle: Entry\n---\nimport Callout from \"./Callout.astro\";\n\n# Body\n";
        let (handle, _view) = mount(cx);
        let session = bare_session(cx, handle, "src/posts/entry.mdx", source);
        session.read_with(cx, |session, cx| {
            assert_eq!(session.projection.visible, "# Body\n");
            assert_eq!(session.editor.read(cx).value(), "# Body\n");
        });

        let retarget = |cx: &mut TestAppContext, path: &str| {
            cx.update_window(handle.into(), |_, window, cx| {
                session.update(cx, |session, cx| {
                    let identity = DocumentIdentity::new(
                        ProjectId::new("projection").unwrap(),
                        1,
                        SourcePath::new(path).unwrap(),
                    );
                    session.retarget(identity, window, cx);
                });
            })
            .unwrap();
            cx.run_until_parked();
        };

        retarget(cx, "src/posts/entry.md");
        session.read_with(cx, |session, cx| {
            assert_eq!(
                session.projection.visible, "import Callout from \"./Callout.astro\";\n\n# Body\n",
                "markdown keeps the import visible"
            );
            assert_eq!(
                session.editor.read(cx).value(),
                session.projection.visible,
                "the editor must follow the new projection"
            );
            assert_eq!(session.buffer.current_text(), source, "bytes are untouched");
        });

        retarget(cx, "src/posts/entry.mdx");
        session.read_with(cx, |session, cx| {
            assert_eq!(session.projection.visible, "# Body\n");
            assert_eq!(session.editor.read(cx).value(), "# Body\n");
            assert_eq!(session.buffer.current_text(), source);
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
    fn mount_workload(
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

    fn schema(fields: Vec<astrolith_astro::schema::Field>) -> CollectionSchema {
        CollectionSchema {
            dialect: astrolith_astro::schema::SchemaDialect::Draft202012,
            fields,
            diagnostics: Vec::new(),
        }
    }

    fn required_field(name: &str, kind: FieldKind) -> astrolith_astro::schema::Field {
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

    fn set_mappings(
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

    async fn open_project_and_wait(
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
    async fn open_entry_for(
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
            assert_eq!(session.counts.words(), 5);
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
                    editor
                        .range_to_bounds(&(start..start + "https://other.dev".len()))
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
    fn focus_mode_dims_other_sentences_and_the_prose_layer_skips_code(cx: &mut TestAppContext) {
        let (handle, _) = mount(cx);
        let text = "First sentence. Second sentence.\n\n> quoted `code` line\n\n```\n# not a heading\n```\n";
        let session = bare_session(cx, handle, "focus.md", text);
        let push = handle.into();
        cx.update_window(push, |_, window, cx| {
            session.update(cx, |session, cx| {
                session.editor.update(cx, |editor, cx| {
                    editor.set_selected_range(0..0, cx);
                });
                session.set_writing_modes(
                    crate::editor::writing_modes::WritingModes {
                        focus: true,
                        typewriter: false,
                    },
                    window,
                    cx,
                );
            });
        })
        .unwrap();
        cx.run_until_parked();

        session.read_with(cx, |session, app| {
            let dimmed = session.presentation.focus_ranges(app);
            let first = text.find("First sentence. ").unwrap() + "First sentence. ".len();
            assert!(
                dimmed.iter().all(|range| range.start >= first),
                "the cursor sentence stays undimmed: {dimmed:?}"
            );
            assert_eq!(dimmed.first().map(|range| range.start), Some(first));
            assert_eq!(dimmed.last().map(|range| range.end), Some(text.len()));

            // Prose presentation: markers dimmed, code spans and blocks carry
            // their own layer and never receive a prose token inside them.
            let ranges = session.presentation.prose_ranges(app);
            let slices: Vec<&str> = ranges.iter().map(|range| &text[range.clone()]).collect();
            let code_span = text.find("`code`").unwrap();
            assert!(slices.contains(&"> "));
            assert!(slices.contains(&"`code`"));
            let fence = text.find("```").unwrap();
            assert!(
                ranges
                    .iter()
                    .any(|range| range.start == fence && range.end == fence + 3),
                "the fence line is a code block span"
            );
            for range in &ranges {
                let inside_code = (code_span..code_span + "`code`".len()).contains(&range.start)
                    && range.end <= code_span + "`code`".len()
                    && *range != (code_span..code_span + "`code`".len());
                assert!(!inside_code, "no prose token inside a code span: {range:?}");
            }
        });

        // Moving the caret to the second sentence flips the dimmed set.
        let second = text.find("Second").unwrap();
        cx.update_window(push, |_, _, cx| {
            let editor = session.read(cx).editor.clone();
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(second..second, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, app| {
            let dimmed = session.presentation.focus_ranges(app);
            assert!(
                dimmed.iter().any(|range| range.start == 0),
                "the first sentence is dimmed once the caret leaves it: {dimmed:?}"
            );
        });
    }

    async fn wait_for_session(
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
    async fn create_writes_required_defaults_and_opens_the_result(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        view.update(cx, |view, cx| {
            let mut project = ProjectPreferencesV1::default();
            project.collections.insert(
                "blog".to_owned(),
                CollectionPreferencesV1 {
                    default_extension: Some(FileExtension::Mdx),
                    ..CollectionPreferencesV1::default()
                },
            );
            view.set_preferences(GlobalPreferencesV2::default(), Some(project), cx);
        });
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        view.update(cx, |view, cx| {
            view.set_collection_schemas(
                vec![(
                    "blog".to_owned(),
                    schema(vec![
                        required_field("title", FieldKind::String { format: None }),
                        required_field("pubDate", FieldKind::Date),
                    ]),
                )],
                cx,
            )
        });
        let today = today();
        let expected = dir.path().join(format!("src/posts/{today}.mdx"));
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
        assert_eq!(
            std::fs::read_to_string(&expected).unwrap(),
            format!("---\ntitle: 'New Blog'\npubDate: '{today}'\n---\n\n")
        );
        let canonical = std::fs::canonicalize(&expected).unwrap();
        view.read_with(cx, |view, _| {
            assert_eq!(view.active.as_ref(), Some(&canonical));
            assert!(
                view.open_tabs()
                    .iter()
                    .any(|tab| tab.label == format!("{today}.mdx"))
            );
        });
    }

    #[gpui_kit::test]
    async fn create_collects_typed_required_fields_before_writing(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        view.update(cx, |view, cx| {
            view.set_collection_schemas(
                vec![(
                    "blog".to_owned(),
                    schema(vec![
                        required_field("title", FieldKind::String { format: None }),
                        required_field("cover", FieldKind::Image),
                        required_field(
                            "author",
                            FieldKind::Reference {
                                collections: vec!["authors".into()],
                            },
                        ),
                        required_field(
                            "tags",
                            FieldKind::Array {
                                items: Box::new(FieldKind::String { format: None }),
                            },
                        ),
                        required_field(
                            "related",
                            FieldKind::Array {
                                items: Box::new(FieldKind::Reference {
                                    collections: vec!["blog".into()],
                                }),
                            },
                        ),
                    ]),
                )],
                cx,
            )
        });
        let before = std::fs::read_dir(dir.path().join("src/posts"))
            .unwrap()
            .count();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.entry_command(CommandId::NewEntry, None, window, cx)
            });
            assert!(
                window.has_active_dialog(cx),
                "typed required fields must be collected before anything is written"
            );
            window.press("escape", cx);
        })
        .unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path().join("src/posts"))
                .unwrap()
                .count(),
            before,
            "nothing may be written before the typed form is submitted"
        );

        // The dialog hands its entered values back through the normal submit path.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.submit_create(
                    None,
                    vec![
                        (
                            "cover".to_owned(),
                            ScalarValue::String("images/cover.png".to_owned()),
                        ),
                        ("author".to_owned(), ScalarValue::String("ada".to_owned())),
                        (
                            "tags".to_owned(),
                            ScalarValue::List(vec!["news".to_owned(), "rust".to_owned()]),
                        ),
                        (
                            "related".to_owned(),
                            ScalarValue::List(vec!["other-post".to_owned()]),
                        ),
                    ],
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        let expected = dir.path().join(format!("src/posts/{}.md", today()));
        wait_for_session(cx, &view, &expected).await;
        assert_eq!(
            std::fs::read_to_string(&expected).unwrap(),
            "---\ntitle: 'New Blog'\ncover: 'images/cover.png'\nauthor: 'ada'\ntags: ['news', 'rust']\nrelated: ['other-post']\n---\n\n"
        );
    }

    struct RecordingTrash {
        fail: bool,
        saved: Mutex<Option<Vec<u8>>>,
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
    async fn rename_saves_dirty_content_and_retargets_every_reference(cx: &mut TestAppContext) {
        let dir = fixture();
        let recovery = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let original = view.read_with(cx, |view, _| view.active.clone().unwrap());

        // A stale recovery record for the old canonical source must not survive.
        let identity = view.read_with(cx, |view, cx| {
            view.sessions
                .get(&original)
                .unwrap()
                .read(cx)
                .buffer
                .identity()
                .clone()
        });
        store
            .save(&RecoveryRecord::new(
                identity,
                DocumentRevision::new(1),
                "stale",
                1,
            ))
            .unwrap();

        let renamed = dir.path().join("src/posts/renamed.md");
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.submit_rename(
                    original.clone(),
                    "src/posts/renamed.md".to_owned(),
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.sessions.contains_key(&renamed) && view.active_entry_id().is_some()
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(!original.exists(), "the old source must be gone");
        view.read_with(cx, |view, cx| {
            assert!(!view.sessions.contains_key(&original));
            assert_eq!(view.active.as_ref(), Some(&renamed));
            assert!(
                view.open_tabs()
                    .iter()
                    .any(|tab| tab.path == renamed && tab.label == "renamed.md")
            );
            assert_eq!(
                view.active_entry_id().map(|id| id.id().to_owned()),
                Some("renamed".to_owned())
            );
            let identity = view
                .sessions
                .get(&renamed)
                .unwrap()
                .read(cx)
                .buffer
                .identity()
                .clone();
            assert_eq!(identity.canonical_source().as_str(), "src/posts/renamed.md");
        });
        let records = store.load().unwrap();
        assert!(
            records
                .records()
                .iter()
                .all(|record| record.identity().canonical_source().as_str() != "src/posts/a.md"),
            "the old recovery key must be cleared"
        );

        // A dirty rename saves through the guarded path before moving the file.
        edit_active(cx, handle, &view, "# Renamed body\n").await;
        let second = dir.path().join("src/posts/renamed-2.md");
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.submit_rename(
                    renamed.clone(),
                    "src/posts/renamed-2.md".to_owned(),
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if second.exists() && !renamed.exists() {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            "# Renamed body\n"
        );
        assert!(!renamed.exists());
    }

    #[gpui_kit::test]
    async fn trash_requires_save_or_discard_and_reconciles_state(cx: &mut TestAppContext) {
        let dir = fixture();
        std::fs::write(dir.path().join("src/posts/c.md"), "# Original C\n").unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;

        // Save and trash: the guarded save runs before the file leaves.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                let index = view
                    .entries
                    .iter()
                    .position(|entry| entry.entry().source().as_str() == "src/posts/a.md")
                    .unwrap();
                view.open_entry(index, window, cx)
            })
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let first = view.read_with(cx, |view, _| view.active.clone().unwrap());
        edit_active(cx, handle, &view, "# Trashed with save\n").await;
        let backend = Arc::new(RecordingTrash {
            fail: false,
            saved: Mutex::new(None),
        });
        view.update(cx, |view, _| view.trash_backend = backend.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.request_trash(first.clone(), window, cx));
            assert!(window.has_active_dialog(cx));
            window.click("trash-save", cx);
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if !first.exists() {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(!first.exists(), "the file must leave after saving");
        assert_eq!(
            backend.saved.lock().unwrap().as_deref(),
            Some("# Trashed with save\n".as_bytes())
        );
        // The post-trash rescan is asynchronous; wait for it to publish.
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.entries
                    .iter()
                    .all(|entry| entry.entry().source().as_str() != "src/posts/a.md")
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
            assert!(!view.sessions.contains_key(&first));
            assert!(view.open_tabs().iter().all(|tab| tab.path != first));
            assert!(
                view.entries
                    .iter()
                    .all(|entry| entry.entry().source().as_str() != "src/posts/a.md")
            );
        });

        // Discard: the unsaved edit never reaches the trashed bytes.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                let index = view
                    .entries
                    .iter()
                    .position(|entry| entry.entry().source().as_str() == "src/posts/c.md")
                    .unwrap();
                view.open_entry(index, window, cx)
            })
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let third = view.read_with(cx, |view, _| view.active.clone().unwrap());
        edit_active(cx, handle, &view, "# Discarded edit\n").await;
        let discard_backend = Arc::new(RecordingTrash {
            fail: false,
            saved: Mutex::new(None),
        });
        view.update(cx, |view, _| view.trash_backend = discard_backend.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.request_trash(third.clone(), window, cx));
            assert!(window.has_active_dialog(cx));
            window.click("trash-discard", cx);
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if !third.exists() {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(!third.exists());
        assert_eq!(
            discard_backend.saved.lock().unwrap().as_deref(),
            Some("# Original C\n".as_bytes()),
            "discard must not write the unsaved edit"
        );
    }

    #[gpui_kit::test]
    async fn trash_failure_keeps_the_file_and_the_session(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());
        edit_active(cx, handle, &view, "# Kept\n").await;
        view.update(cx, |view, _| {
            view.trash_backend = Arc::new(RecordingTrash {
                fail: true,
                saved: Mutex::new(None),
            })
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.request_trash(path.clone(), window, cx));
            window.click("trash-save", cx);
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.status.contains("Could not move")) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(path.exists(), "a refused trash never deletes the file");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Kept\n");
        view.read_with(cx, |view, _| {
            assert!(view.sessions.contains_key(&path));
            assert!(view.status.contains("Could not move"));
        });
    }

    #[gpui_kit::test]
    async fn watcher_batches_drop_stale_tabs_and_selection(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_entry(0, window, cx);
                view.open_entry(1, window, cx);
            })
        })
        .unwrap();
        for _ in 0..100 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.sessions.len() == 2) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(20))
            })
            .await
            .unwrap();
        }
        let stale = view.read_with(cx, |view, _| view.active.clone().unwrap());
        std::fs::remove_file(&stale).unwrap();
        for _ in 0..100 {
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(20))
            })
            .await
            .unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.entries.len() == 1) {
                break;
            }
        }
        view.read_with(cx, |view, _| {
            assert_eq!(view.entries.len(), 1);
            assert!(
                !view.sessions.contains_key(&stale),
                "a clean tab for a removed source must not survive"
            );
            assert!(view.open_tabs().iter().all(|tab| tab.path != stale));
            assert_eq!(view.active, None);
        });
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
            assert!(window.try_find("project-no-entries").is_some());
            assert!(window.try_find("project-loading").is_none());
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
        filtered_view.update(cx, |view, cx| view.toggle_drafts_only(cx));
        cx.update_window(filtered_handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("project-filtered-empty").is_some());
            assert!(window.try_find("project-no-entries").is_none());
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
    async fn create_dialog_collects_required_fields_before_writing(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
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
        let before = std::fs::read_dir(dir.path().join("src/posts"))
            .unwrap()
            .count();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.entry_command(CommandId::NewEntry, None, window, cx)
            });
            assert!(
                window.has_active_dialog(cx),
                "required inputs must be collected before anything is written"
            );
            window.press("escape", cx);
        })
        .unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path().join("src/posts"))
                .unwrap()
                .count(),
            before,
            "opening the dialog must not write the entry"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.submit_create(
                    None,
                    vec![("subtitle".to_owned(), ScalarValue::String("Hello".into()))],
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        let expected = dir.path().join(format!("src/posts/{}.md", today()));
        wait_for_session(cx, &view, &expected).await;
        let content = std::fs::read_to_string(&expected).unwrap();
        assert!(content.contains("subtitle: 'Hello'"), "{content}");
    }

    #[gpui_kit::test]
    async fn duplicate_saves_dirty_content_and_names_the_copy(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let original = view.read_with(cx, |view, _| view.active.clone().unwrap());
        edit_active(cx, handle, &view, "# Copy me\n").await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.entry_command(CommandId::DuplicateEntry, None, window, cx)
            })
        })
        .unwrap();
        let copy = dir.path().join("src/posts/a-copy.md");
        wait_for_session(cx, &view, &copy).await;
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "# Copy me\n");
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "# Copy me\n");
        view.read_with(cx, |view, _| {
            assert!(
                view.entries
                    .iter()
                    .any(|entry| { entry.entry().source().as_str() == "src/posts/a-copy.md" })
            );
        });
    }

    #[gpui_kit::test]
    async fn newer_edits_abort_a_pending_trash_and_keep_buffer_and_recovery(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let recovery = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());

        // The revision a Save snapshot would carry, plus its recovery record.
        edit_active(cx, handle, &view, "# Saved revision\n").await;
        let saved_revision = view.update(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().clone();
            session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                session.buffer.revision().get()
            })
        });
        for _ in 0..300 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if !store.load().unwrap().records().is_empty() {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(store.load().unwrap().records().len(), 1);

        // Newer edits typed while the save was in flight.
        edit_active(cx, handle, &view, "# Newer revision\n").await;
        let backend = Arc::new(RecordingTrash {
            fail: false,
            saved: Mutex::new(None),
        });
        view.update(cx, |view, _| view.trash_backend = backend.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.pending_after_save = Some(PendingAfterSave::Trash { path: path.clone() });
                // The completion seam: the snapshot's revision is already outdated.
                view.finish_save(
                    path.clone(),
                    Ok(SaveOutcome::Saved {
                        revision: saved_revision,
                    }),
                    window,
                    cx,
                );
            })
        })
        .unwrap();
        assert!(
            view.read_with(cx, |view, _| view.pending_after_save.is_none()),
            "the pending trash is consumed exactly once"
        );
        assert!(backend.saved.lock().unwrap().is_none(), "no trash happened");
        assert!(path.exists(), "the file must stay on disk");
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(session.buffer.is_dirty());
            assert_eq!(session.buffer.current_text(), "# Newer revision\n");
            assert!(
                view.status.contains("not moved to the Trash"),
                "{}",
                view.status
            );
        });
        for _ in 0..300 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if store
                .load()
                .unwrap()
                .records()
                .iter()
                .any(|record| record.content() == "# Newer revision\n")
            {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(
            store
                .load()
                .unwrap()
                .records()
                .iter()
                .any(|record| record.content() == "# Newer revision\n"),
            "the newer edits stay recoverable"
        );
    }

    #[gpui_kit::test]
    async fn newer_edits_survive_a_pending_rename_and_stay_recoverable(cx: &mut TestAppContext) {
        let dir = fixture();
        let recovery = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());

        edit_active(cx, handle, &view, "# Saved revision\n").await;
        let saved_revision = view.update(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().clone();
            session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                session.buffer.revision().get()
            })
        });
        edit_active(cx, handle, &view, "# Newer revision\n").await;

        let destination = SourcePath::new("src/posts/renamed.md").unwrap();
        let target = dir.path().join("src/posts/renamed.md");
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.pending_after_save = Some(PendingAfterSave::Rename {
                    from: path.clone(),
                    to: destination.clone(),
                });
                view.finish_save(
                    path.clone(),
                    Ok(SaveOutcome::Saved {
                        revision: saved_revision,
                    }),
                    window,
                    cx,
                );
            })
        })
        .unwrap();

        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.sessions.contains_key(&target)) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(target.exists(), "the rename still runs");
        assert!(!path.exists());
        view.read_with(cx, |view, cx| {
            assert!(!view.sessions.contains_key(&path));
            let session = view.sessions.get(&target).unwrap().read(cx);
            assert!(session.buffer.is_dirty(), "the newer edits are retained");
            assert_eq!(session.buffer.current_text(), "# Newer revision\n");
        });
        for _ in 0..300 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            if store
                .load()
                .unwrap()
                .records()
                .iter()
                .any(|record| record.content() == "# Newer revision\n")
            {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(
            store.load().unwrap().records().iter().any(|record| {
                record.content() == "# Newer revision\n"
                    && record.identity().canonical_source().as_str() == destination.as_str()
            }),
            "the retargeted dirty buffer is recovered under its new path"
        );
    }

    #[gpui_kit::test]
    async fn an_older_watcher_scan_cannot_overwrite_a_newer_operation_rescan(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let stale_entries = view.read_with(cx, |view, _| view.entries.as_ref().clone());
        let stale_manifest = view.read_with(cx, |view, _| {
            view.manifest.as_ref().unwrap().as_ref().clone()
        });
        // A watcher scan claimed its request before the operation started.
        let stale_token = view.update(cx, |view, _| view.begin_index_request());

        std::fs::write(dir.path().join("src/posts/new.md"), "# New\n").unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.rescan(None, window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.published_index_request > stale_token
                    && view
                        .entries
                        .iter()
                        .any(|entry| entry.entry().source().as_str() == "src/posts/new.md")
            }) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(!view.read_with(cx, |view, _| view.is_refreshing()));
        let index = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == "src/posts/new.md")
                .expect("the newer index contains the new file")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(index, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let added = view.read_with(cx, |view, _| view.active.clone().unwrap());

        // The older watcher scan lands last and must be dropped.
        view.update(cx, |view, cx| {
            view.apply_watcher_scan(
                stale_token,
                Ok(WatcherScan {
                    index: Ok((stale_manifest, stale_entries)),
                    fingerprints: Vec::new(),
                }),
                cx,
            )
        });
        view.read_with(cx, |view, _| {
            assert!(
                view.entries
                    .iter()
                    .any(|entry| entry.entry().source().as_str() == "src/posts/new.md"),
                "the newer index must win"
            );
            assert!(
                view.sessions.contains_key(&added),
                "a stale index must not close a clean session the newer index retains"
            );
        });

        // A stale result must not clear the busy flag of a newer in-flight request.
        let in_flight = view.update(cx, |view, _| view.begin_index_request());
        view.update(cx, |view, cx| {
            view.apply_watcher_scan(
                stale_token,
                Ok(WatcherScan {
                    index: Ok((view.manifest.as_ref().unwrap().as_ref().clone(), Vec::new())),
                    fingerprints: Vec::new(),
                }),
                cx,
            )
        });
        assert!(
            view.read_with(cx, |view, _| view.is_refreshing()),
            "a stale result must not clear a newer request's busy flag"
        );
        view.update(cx, |view, _| view.finish_index_request(in_flight));
        assert!(!view.read_with(cx, |view, _| view.is_refreshing()));
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

    #[gpui_kit::test]
    async fn copy_path_offers_both_forms(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        for (command, expected) in [
            (CommandId::CopyPathRelative, "src/posts/a.md"),
            (
                CommandId::CopyPathAbsolute,
                std::fs::canonicalize(dir.path().join("src/posts/a.md"))
                    .unwrap()
                    .to_string_lossy()
                    .as_ref(),
            ),
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.status.clear();
                    view.entry_command(command, None, window, cx)
                })
            })
            .unwrap();
            for _ in 0..300 {
                cx.run_until_parked();
                if view.read_with(cx, |view, _| view.status.starts_with("Copied")) {
                    break;
                }
                runtime::blocking(cx, || {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                })
                .await
                .unwrap();
            }
            let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
            assert_eq!(copied.as_deref(), Some(expected), "{}", command.label());
        }
    }
}
