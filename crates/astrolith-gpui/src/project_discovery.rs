//! Owns project generations, index publication, retained sessions and load admission.
use crate::{
    editor::session::{DocumentSession, SessionChanged},
    layout::panels::WorkspacePanel,
    runtime, settings,
};
use astrolith_astro::{
    Cancellation, DiscoveryOutcome, DiscoveryRequest, ProjectManifest, contained_path,
    inspect_project, read_bounded,
    schema::CollectionSchema,
    schema::generation::{SchemaGenerationRequest, generate_collection_schemas},
};
use astrolith_core::ProjectId;
use astrolith_document::{
    DocumentIdentity, DocumentLoadTicket, DocumentRevision, DocumentSnapshot, FrontmatterValue,
    RecoveryRecord, RecoveryStore, SaveOutcome, parse_document, save_snapshot,
};
use astrolith_preview::RouteMapping;
use astrolith_workspace::{CollectionEntry, CollectionWatcher, scan_collections};
use gpui_kit::component::{
    ActiveTheme, Disableable, IndexPath, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
    tab::{Tab, TabBar},
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, Focusable, IntoElement, PathPromptOptions,
    Render, SharedString, Subscription, Task, Window, div, prelude::*,
};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
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

struct EntryList {
    entries: Arc<Vec<CollectionEntry>>,
}
impl ListDelegate for EntryList {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.entries.len()
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let entry = self.entries.get(ix.row)?;
        let id = entry.entry().id();
        let label = format!(
            "{} / {}",
            id.collection().as_str(),
            entry.entry().source().as_str()
        );
        Some(
            ListItem::new(SharedString::from(format!(
                "entry:{}:{}",
                id.collection().as_str(),
                id.id()
            )))
            .accessibility_label(label.clone())
            .child(div().text_sm().truncate().child(label)),
        )
    }
    fn set_selected_index(
        &mut self,
        _: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
    }
}

pub(crate) struct ProjectWorkspace {
    document: Entity<WorkspacePanel>,
    manifest: Option<Arc<ProjectManifest>>,
    entries: Arc<Vec<CollectionEntry>>,
    list: Entity<ListState<EntryList>>,
    generation: u64,
    index_revision: u64,
    project_id: Option<ProjectId>,
    cancellation: Cancellation,
    loading: bool,
    refreshing: bool,
    index_error: Option<String>,
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
impl ProjectWorkspace {
    pub fn new(
        document: Entity<WorkspacePanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entries = Arc::new(Vec::new());
        let list = cx.new(|cx| {
            ListState::new(
                EntryList {
                    entries: entries.clone(),
                },
                window,
                cx,
            )
        });
        let subscription =
            cx.subscribe_in(&list, window, |this, _, event: &ListEvent, window, cx| {
                if let ListEvent::Confirm(ix) = event {
                    this.open_entry(ix.row, window, cx);
                }
            });
        Self {
            document,
            manifest: None,
            entries,
            list,
            generation: 0,
            index_revision: 0,
            project_id: None,
            cancellation: Cancellation::default(),
            loading: false,
            refreshing: false,
            index_error: None,
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
            _subscriptions: vec![subscription],
            session_subscriptions: HashMap::new(),
            recovery: settings::recovery_dir().ok().map(RecoveryStore::new),
            pending_recovery: Vec::new(),
            recovery_task: None,
            recovery_load_task: None,
            save_task: None,
            restore_task: None,
        }
    }
    pub fn has_dirty(&self, cx: &App) -> bool {
        self.sessions
            .values()
            .any(|session| session.read(cx).is_dirty(cx))
    }

    /// Install normalized collection schemas (from the R1 generation service).
    pub fn set_collection_schemas(
        &mut self,
        schemas: Vec<(String, CollectionSchema)>,
        cx: &mut Context<Self>,
    ) {
        self.schemas = schemas
            .into_iter()
            .map(|(collection, schema)| (collection, Arc::new(schema)))
            .collect();
        tracing::debug!(
            collections = self.schemas.len(),
            "collection schemas installed"
        );
        cx.notify();
    }

    /// Generate and load collection schemas from the project's own Astro install.
    ///
    /// Runs entirely off the UI thread. A missing or failing Astro installation is
    /// non-fatal: the frontmatter form simply stays unavailable for that project.
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
            "generating collection schemas"
        );
        let task = runtime::blocking(cx, move || {
            let request = SchemaGenerationRequest::new(root, cancellation);
            generate_collection_schemas(&request)
        });
        self.schema_task = Some(cx.spawn(async move |this, cx| {
            let result = task
                .await
                .and_then(|value| value.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || this.project_id != project {
                    return;
                }
                match result {
                    Ok(report) => {
                        tracing::info!(
                            collections = report.schemas.len(),
                            "collection schemas generated"
                        );
                        let schemas = report
                            .schemas
                            .into_iter()
                            .map(|entry| (entry.collection, entry.schema))
                            .collect();
                        this.set_collection_schemas(schemas, cx);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "collection schema generation failed");
                    }
                }
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
        let active = self.active.as_ref()?;
        let root = self.manifest.as_ref()?.root();
        let relative = active
            .strip_prefix(root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        self.entries
            .iter()
            .find(|entry| entry.entry().source().as_str() == relative)
            .map(|entry| entry.entry().id().clone())
    }

    /// Identity used to decide when the frontmatter form needs a refresh.
    pub fn active_frontmatter_key(&self) -> Option<(PathBuf, bool)> {
        let path = self.active.clone()?;
        let has_schema = self
            .session_collections
            .get(&path)
            .is_some_and(|collection| self.schemas.contains_key(collection));
        Some((path, has_schema))
    }

    /// Return the active document's resolved schema and current text, if any.
    pub fn active_frontmatter(&self, cx: &App) -> Option<(Arc<CollectionSchema>, String)> {
        let path = self.active.as_ref()?;
        let collection = self.session_collections.get(path)?;
        let schema = self.schemas.get(collection)?.clone();
        let session = self.sessions.get(path)?;
        let text = session.read(cx).editor.read(cx).text().to_string();
        Some((schema, text))
    }

    /// Apply one frontmatter field edit to the active session's *current* text.
    ///
    /// Parses the live editor text, patches exactly `path` through the lossless
    /// [`astrolith_document::ParsedDocument::set_field`] contract, and replaces
    /// the session text with the result. Because the patch is computed from the
    /// live text, concurrent unsaved edits to the document body are preserved
    /// exactly; a stale form snapshot is never written back. No disk I/O happens
    /// here — the conflict-aware save coordinator persists the edit later.
    pub fn apply_frontmatter_field(
        &mut self,
        path: String,
        value: FrontmatterValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active) = self.active.clone() else {
            return;
        };
        let Some(session) = self.sessions.get(&active).cloned() else {
            return;
        };
        let live = session.read(cx).editor.read(cx).text().to_string();
        let patched = (|| -> Result<String, String> {
            let mut document = parse_document(&live).map_err(|error| error.to_string())?;
            document
                .set_field(&path, value)
                .map_err(|error| error.to_string())?;
            document.serialize().map_err(|error| error.to_string())
        })();
        match patched {
            Ok(text) => {
                session.update(cx, |session, cx| {
                    session
                        .editor
                        .update(cx, |editor, cx| editor.set_value(text.clone(), window, cx));
                    let current = session.editor.read(cx).text().to_string();
                    let _ = session.buffer.apply_text(current);
                    session.dirty = session.buffer.is_dirty();
                    cx.emit(SessionChanged);
                    cx.notify();
                });
                cx.notify();
            }
            Err(error) => {
                tracing::warn!(field = %path, %error, "could not edit a frontmatter field");
                self.status = format!("Could not edit frontmatter field `{path}`: {error}");
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
        let request =
            DiscoveryRequest::new(project.clone(), generation, root, self.cancellation.clone());
        self.loading = true;
        self.refreshing = false;
        self.index_error = None;
        self.status = "Discovering project…".into();
        self.watch_task = None;
        self.schema_task = None;
        self.watcher_owner = None;
        self.loads.clear();
        self.sessions.clear();
        self.session_subscriptions.clear();
        self.session_collections.clear();
        self.schemas.clear();
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
                    let manifest =
                        match inspect_project(&request).map_err(|error| error.to_string())? {
                            DiscoveryOutcome::Ready(manifest) => manifest,
                            DiscoveryOutcome::Unsupported { diagnostics, .. } => {
                                return Err(diagnostics.join("; "));
                            }
                        };
                    watcher
                        .refresh(&manifest)
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
                        this.manifest = Some(Arc::new(manifest));
                        this.sessions.clear();
                        this.active = None;
                        this.document
                            .update(cx, |document, cx| document.clear_editor(cx));
                        this.publish_entries(entries, cx);
                        this.start_watcher(watcher, window, cx);
                        this.status = format!(
                            "{} entries · Ctrl/Cmd+S saves the active document.",
                            this.entries.len()
                        );
                        this.load_pending_recovery(window, cx);
                        this.load_collection_schemas(cx);
                    }
                    Err(error) => {
                        tracing::warn!(
                            project = %project.as_str(),
                            generation,
                            %error,
                            "could not open Astro project"
                        );
                        this.status = format!("Could not open project: {error}");
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
        let root = manifest.root().to_owned();
        tracing::debug!(
            project = %manifest.project().as_str(),
            generation,
            root = %root.display(),
            "project watcher started"
        );
        let request = DiscoveryRequest::new(
            manifest.project().clone(),
            generation,
            root.clone(),
            self.cancellation.clone(),
        );
        self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                let snapshots = this.update_in(cx, |this, _, _| {
                    this.refreshing = true;
                    this.sessions.keys().chain(this.loads.keys()).cloned().collect::<std::collections::BTreeSet<_>>()
                }).unwrap_or_default();
                let project = request.project().clone();
                let request = request.clone();
                let watcher = weak_watcher.clone();
                let root = root.clone();
                let result = runtime::blocking(cx, move || {
                    request.cancellation().check().map_err(|error| error.to_string())?;
                    let Some(watcher) = watcher.upgrade() else { return Err("Watcher stopped".into()); };
                    let batch = watcher.drain();
                    if !batch.changed && !batch.needs_rescan { return Ok(None); }
                    tracing::debug!(
                        changed = batch.changed,
                        needs_rescan = batch.needs_rescan,
                        "project watcher produced a change batch"
                    );
                    // Compare retained and in-flight sources even when reindexing fails.
                    let changes = snapshots.into_iter().map(|path| {
                        let fingerprint = contained_path(&root, path.strip_prefix(&root).unwrap_or(&path)).ok()
                            .filter(|actual| actual == &path)
                            .and_then(|actual| read_bounded(&actual, 8 * 1024 * 1024).ok())
                            .map(|text| crate::editor::session::fingerprint(&text));
                        (path, fingerprint)
                    }).collect::<Vec<_>>();
                    let index: Result<_, String> = (|| {
                        match inspect_project(&request).map_err(|error| error.to_string())? {
                            DiscoveryOutcome::Ready(manifest) => {
                                watcher.refresh(&manifest).map_err(|error| error.to_string())?;
                                let entries = scan_collections(&manifest, request.cancellation()).map_err(|error| error.to_string())?;
                                Ok((manifest, entries))
                            }
                            DiscoveryOutcome::Unsupported { diagnostics, .. } => Err(diagnostics.join("; ")),
                        }
                    })();
                    Ok(Some((index, changes)))
                }).await.and_then(|result| result);
                let still_current = this.update_in(cx, |this, _, cx| {
                    if this.generation != generation || this.project_id.as_ref() != Some(&project) { return false; }
                    this.refreshing = false;
                    match result {
                        Ok(Some((index, changes))) => {
                            for (path, fingerprint) in changes {
                                if let Some(session) = this.sessions.get(&path) {
                                    session.update(cx, |session, _| session.externally_changed = fingerprint != Some(session.disk_fingerprint));
                                }
                            }
                            match index {
                                Ok((manifest, entries)) => {
                                    this.index_error = None;
                                    this.manifest = Some(Arc::new(manifest));
                                    this.publish_entries(entries, cx);
                                    this.status = "Index refreshed; open buffers retained. Close and reopen an unchanged document to reload it.".into();
                                }
                                Err(error) => {
                                    tracing::warn!(%error, "project reindex failed; buffers retained");
                                    this.publish_entries(Vec::new(), cx);
                                    this.index_error = Some(error);
                                    this.status = "Project index unavailable. Open buffers retained.".into();
                                }
                            }
                            cx.notify();
                        }
                        Err(error) => {
                            tracing::warn!(%error, "project watcher failed; buffers retained");
                            this.publish_entries(Vec::new(), cx);
                            this.index_error = Some(error);
                            this.status = "Project index unavailable. Open buffers retained.".into();
                            cx.notify();
                        }
                        Ok(None) => {}
                    }
                    true
                }).unwrap_or(false);
                if !still_current { break; }
            }
        }));
    }
    fn publish_entries(&mut self, entries: Vec<CollectionEntry>, cx: &mut Context<Self>) {
        self.index_revision = self
            .index_revision
            .checked_add(1)
            .expect("index revision exhausted");
        self.loads.clear();
        self.entries = Arc::new(entries);
        tracing::debug!(
            entries = self.entries.len(),
            index_revision = self.index_revision,
            "published project entries"
        );
        self.list.update(cx, |list, cx| {
            list.delegate_mut().entries = self.entries.clone();
            cx.notify();
        });
    }
    fn open_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
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
        if self.refreshing {
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
                let subscription = cx.subscribe(&session, |this, _, _: &SessionChanged, cx| {
                    this.schedule_recovery(cx);
                    cx.emit(SessionChanged);
                    cx.notify();
                });
                self.session_subscriptions.insert(key.clone(), subscription);
                self.sessions.insert(key.clone(), session.clone());
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
    fn activate_session(
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
        self.document.read(cx).focus_handle(cx).focus(window, cx);
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
        self.document
            .update(cx, |document, cx| document.show_editor(title, editor, cx));
        self.status = if session.read(cx).externally_changed {"Source changed on disk. This buffer is retained; close and reopen an unchanged document to reload."} else {"Editing in memory · Ctrl/Cmd+S saves."}.into();
        cx.notify();
    }
    fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active.clone() else {
            return;
        };
        if self
            .sessions
            .get(&path)
            .is_some_and(|session| session.read(cx).is_dirty(cx))
        {
            let owner = cx.weak_entity();
            window.open_dialog(cx,move|dialog,_,_| {
                let owner=owner.clone();let path=path.clone();
                dialog.title("Discard unsaved changes?")
                    .child("Saving is not available yet. Keep editing to preserve this document in memory.")
                    .footer(div().h_flex().gap_2()
                        .child(Button::new("keep-editing").primary().label("Keep editing").on_click(|_,window,cx|window.close_dialog(cx)))
                        .child(Button::new("discard-document").danger().label("Discard changes").on_click(move|_,window,cx| {
                            let _=owner.update(cx,|this,cx|this.remove_session(&path,cx));window.close_dialog(cx);
                        })))
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
        self.active = None;
        self.document
            .update(cx, |document, cx| document.clear_editor(cx));
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
        let Some(session) = self.sessions.get(&path).cloned() else {
            return;
        };
        // Capture the latest editor text into the buffer before snapshotting so a
        // queued Change event cannot cause a stale snapshot.
        session.update(cx, |session, cx| {
            let text = session.editor.read(cx).text().to_string();
            let _ = session.buffer.apply_text(text);
            session.dirty = session.buffer.is_dirty();
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.sessions.get(&path).cloned() else {
            self.status = "Document closed before the save finished.".into();
            cx.notify();
            return;
        };
        match result {
            Ok(SaveOutcome::Saved { revision }) => {
                tracing::info!(
                    path = %path.display(),
                    revision,
                    "document saved"
                );
                let content = session.read(cx).buffer.current_text().to_owned();
                let identity = session.read(cx).buffer.identity().clone();
                let marked = session.update(cx, |session, cx| {
                    let marked = session
                        .buffer
                        .confirm_saved(DocumentRevision::new(revision), &content);
                    session.dirty = session.buffer.is_dirty();
                    if marked {
                        session.externally_changed = false;
                    }
                    cx.notify();
                    marked
                });
                if marked && let Some(store) = self.recovery.clone() {
                    let task = runtime::blocking(cx, move || {
                        let _ = store.clear_after_save(&identity, &content);
                    });
                    task.detach();
                }
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
        cx.notify();
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

    fn persist_recovery_records(&self, cx: &App) -> (Vec<RecoveryRecord>, Vec<DocumentIdentity>) {
        let now = now_secs();
        let mut dirty = Vec::new();
        let mut clean = Vec::new();
        for session in self.sessions.values() {
            let session = session.read(cx);
            if session.is_dirty(cx) {
                dirty.push(RecoveryRecord::capture(&session.buffer, now));
            } else {
                clean.push(session.buffer.identity().clone());
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
                    let subscription = cx.subscribe(&session, |this, _, _: &SessionChanged, cx| {
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

impl Render for ProjectWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_session = self
            .active
            .as_ref()
            .and_then(|path| self.sessions.get(path));
        let active_loaded = active_session.is_some();
        let paths: Vec<_> = self.sessions.keys().cloned().collect();
        let selected = paths
            .iter()
            .position(|path| Some(path) == self.active.as_ref())
            .unwrap_or(0);
        let tabs = TabBar::new("open-documents")
            .selected_index(selected)
            .children(paths.iter().map(|path| {
                Tab::new().label(
                    path.strip_prefix(
                        self.manifest
                            .as_ref()
                            .map_or(std::path::Path::new(""), |manifest| manifest.root()),
                    )
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned(),
                )
            }))
            .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                if let Some(path) = paths.get(*ix) {
                    this.activate_session(path, window, cx);
                }
            }));
        let external = active_session.is_some_and(|session| session.read(cx).externally_changed);
        let dirty = active_session.is_some_and(|session| session.read(cx).dirty);
        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                div().p_2().child(
                    Button::new("open-project")
                        .small()
                        .ghost()
                        .label("Open project…")
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, window, cx| this.prompt(window, cx))),
                ),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.status.clone()),
            )
            .when_some(self.index_error.as_ref(), |view, error| {
                view.child(
                    div()
                        .px_3()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(format!("Index unavailable: {error}")),
                )
            })
            .when(!self.sessions.is_empty(), |view| view.child(tabs))
            .child(div().flex_1().min_h_0().child(List::new(&self.list)))
            .when(active_loaded, |view| {
                view.child(
                    div()
                        .p_2()
                        .v_flex()
                        .gap_1()
                        .when(external, |view| {
                            view.child(
                                div()
                                    .text_color(cx.theme().warning)
                                    .child("Source changed on disk · Buffer retained"),
                            )
                        })
                        .child(if dirty {
                            "Unsaved changes"
                        } else {
                            "Unchanged"
                        })
                        .child(
                            Button::new("save-document")
                                .small()
                                .ghost()
                                .label("Save")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_active(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("close-document")
                                .small()
                                .ghost()
                                .label("Close document")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.close_active(window, cx)
                                    }),
                                ),
                        ),
                )
            })
    }
}
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::SourcePath;
    use gpui_kit::{TestAppContext, WindowHandle, WindowOptions, base::Root, test::TestWindowExt};
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/astro")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/@astrojs/mdx")).unwrap();
        std::fs::write(
            dir.path().join("node_modules/@astrojs/mdx/package.json"),
            r#"{"version":"4.3.14"}"#,
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
            view.sessions
                .get(&first)
                .unwrap()
                .read(cx)
                .buffer
                .is_dirty()
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
            assert!(session.buffer.is_dirty());
            assert!(session.buffer.revision().get() > 0);
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
            let mdx = view
                .sessions
                .get(view.active.as_ref().unwrap())
                .unwrap()
                .read(cx)
                .editor
                .read(cx)
                .value();
            assert!(mdx.contains("export const label"));
            assert!(mdx.contains("<p>{label}</p>"));
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&first, window, cx))
        })
        .unwrap();
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&first).unwrap().read(cx);
            assert_eq!(session.editor.read(cx).value(), "# Fixture edit\n");
            assert!(session.is_dirty(cx));
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
            view.sessions.get(&path).unwrap().read(cx).buffer.is_dirty()
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
                            },
                            Field {
                                name: "draft".to_owned(),
                                required: false,
                                default: None,
                                kind: FieldKind::Boolean,
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
        let (schema, text) = view
            .read_with(cx, |view, cx| view.active_frontmatter(cx))
            .expect("active document must expose its schema and text");
        assert!(schema.fields.iter().any(|field| field.name == "title"));
        assert_eq!(text, with_frontmatter);
        assert_eq!(
            view.read_with(cx, |view, _| view.active_frontmatter_key()),
            Some((path.clone(), true))
        );

        // The user edits the document BODY in the source editor (unsaved, in memory).
        let body_edited = "---\ntitle: Old\ndraft: false\n---\n\n# Body EDITED 中文\n";
        edit_active(cx, handle, &view, body_edited).await;
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.sessions
                    .get(&path)
                    .unwrap()
                    .read(cx)
                    .editor
                    .read(cx)
                    .value(),
                body_edited
            );
        });

        // A form field edit patches ONLY that field on the live text; the
        // concurrent body edit must survive exactly.
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply_frontmatter_field(
                    "title".to_owned(),
                    FrontmatterValue::String("Edited".to_owned()),
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
                session.editor.read(cx).value(),
                expected,
                "only the title field may change; the unsaved body edit must survive"
            );
            assert_eq!(session.buffer.current_text(), expected);
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
}
