//! Filesystem watcher loop, index publication and batched schema reconciliation.
use super::*;

pub(super) struct ServiceWatcher {
    weak: std::sync::Weak<CollectionWatcher>,
    owner: tokio::task::JoinHandle<()>,
}

impl ServiceWatcher {
    pub(super) fn new(watcher: CollectionWatcher, cancellation: Cancellation) -> Self {
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

/// How long a file's mtime must be old before its cached fingerprint is
/// trusted. Filesystems commonly store whole-second timestamps, so an edit
/// inside the same second can leave `(mtime, len)` unchanged.
pub(super) const FINGERPRINT_RACY_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

/// One cached document fingerprint and the stat values it was taken from.
pub(super) struct FingerprintSample {
    modified: Option<std::time::SystemTime>,
    len: u64,
    fingerprint: u64,
}

/// Per-path fingerprint cache for open documents, kept between watcher batches
/// so an unchanged buffer is not re-read and re-hashed on every burst.
#[derive(Default)]
pub(super) struct DocumentFingerprints {
    samples: HashMap<PathBuf, FingerprintSample>,
    /// Number of content reads, for the cache test.
    #[cfg(test)]
    reads: usize,
}

impl DocumentFingerprints {
    /// Fingerprint `path`, reading it only when the cached sample is stale or
    /// the file was modified too recently to trust its mtime.
    fn sample(&mut self, path: &Path, now: std::time::SystemTime) -> Option<u64> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata.modified().ok();
        let len = metadata.len();
        if metadata.is_file()
            && let Some(cached) = self.samples.get(path)
            && cached.len == len
            && cached.modified == modified
            && modified.is_some_and(|mtime| {
                now.duration_since(mtime)
                    .is_ok_and(|age| age >= FINGERPRINT_RACY_WINDOW)
            })
        {
            return Some(cached.fingerprint);
        }
        let text = read_bounded(path, MAX_DOCUMENT_BYTES).ok()?;
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let fingerprint = crate::editor::session::fingerprint(&text);
        self.samples.insert(
            path.to_path_buf(),
            FingerprintSample {
                modified,
                len,
                fingerprint,
            },
        );
        Some(fingerprint)
    }

    /// Drop samples for documents that are no longer retained or loading.
    fn retain_paths(&mut self, paths: &BTreeSet<PathBuf>) {
        self.samples.retain(|path, _| paths.contains(path));
    }
}

/// One completed watcher scan, published only when its request is the newest.
pub(super) struct WatcherScan {
    pub(super) index: Result<(ProjectManifest, Vec<CollectionEntry>), String>,
    pub(super) fingerprints: Vec<(PathBuf, Option<u64>)>,
}

impl ProjectWorkspace {
    /// Install one completed schema generation unless a newer request superseded it.
    ///
    /// Returns `false` when the report is stale; the caller must then drop it
    /// without touching the form so a superseded generation can never replace
    /// fresh schemas or reload the panel.
    pub(super) fn apply_generated_schemas(
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

    /// Reconcile collection schemas with the project's own Astro install.
    ///
    /// Called once when a project opens and once per watcher batch that touched
    /// a schema input. Generation and the freshness fingerprint both run on the
    /// blocking lane. A missing or failing Astro install is non-fatal: the
    /// frontmatter form keeps whatever schemas it already had. The fingerprint
    /// observed before generation is compared with [`SchemaFreshness`], so an
    /// unchanged input does no work and a superseded request is dropped before
    /// touching the form. There is no periodic poll: an idle project performs no
    /// schema work at all.
    pub(super) fn load_collection_schemas(&mut self, cx: &mut Context<Self>) {
        let Some(manifest) = self.manifest.clone() else {
            return;
        };
        if self.schema_in_flight {
            // A reconciliation is already probing the same inputs; run one more
            // pass when it finishes instead of aborting it (Astro writes many
            // schema files while it runs).
            self.schema_dirty = true;
            return;
        }
        let generation = self.generation;
        let project = self.project_id.clone();
        let root = manifest.root().to_owned();
        let cancellation = self.cancellation.clone();
        self.schema_in_flight = true;
        self.schema_dirty = false;
        #[cfg(test)]
        {
            self.schema_reconciliations += 1;
        }
        tracing::debug!(
            project = %project.as_ref().map_or("<unknown>", |project| project.as_str()),
            root = %root.display(),
            "reconciling collection schemas"
        );
        self.schema_task = Some(cx.spawn(async move |this, cx| {
            let mut first = true;
            loop {
                // The first pass always runs; later passes only when a schema
                // batch arrived while this task was working. The last update also
                // clears the in-flight flag atomically with the decision to stop,
                // so a following batch starts a new task.
                let step = this.update(cx, |this, _| {
                    if this.generation != generation || this.project_id != project {
                        // A project switch already released this flag for the
                        // generation that replaced us.
                        return None;
                    }
                    let dirty = std::mem::take(&mut this.schema_dirty);
                    if !first && !dirty {
                        this.schema_in_flight = false;
                        return None;
                    }
                    first = false;
                    Some(())
                });
                if !matches!(step, Ok(Some(()))) {
                    break;
                }
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
                    // value is installed. Otherwise the next pass would see a
                    // "different" fingerprint and sync again.
                    let fingerprint = schema_fingerprint(&generation_root).ok();
                    (report, fingerprint)
                })
                .await;
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation || this.project_id != project {
                        return;
                    }
                    match report {
                        Ok((Ok(report), Some(fingerprint))) => {
                            let schemas = report
                                .schemas
                                .into_iter()
                                .map(|entry| (entry.collection, entry.schema))
                                .collect();
                            this.apply_generated_schemas(request, fingerprint, schemas, cx);
                        }
                        Ok((Ok(_), None)) => {
                            this.schema_freshness.reject(request);
                            tracing::warn!(
                                "could not re-read the schema fingerprint after generation"
                            );
                        }
                        Ok((Err(error), _)) => {
                            this.schema_freshness.reject(request);
                            tracing::warn!(%error, "collection schema generation failed");
                        }
                        Err(error) => {
                            this.schema_freshness.reject(request);
                            tracing::warn!(%error, "collection schema generation task failed");
                        }
                    }
                });
            }
            let _ = this.update(cx, |this, _| {
                if this.generation == generation && this.project_id == project {
                    this.schema_in_flight = false;
                }
            });
        }));
    }

    /// Run the non-index work of one merged watcher batch.
    ///
    /// A batch that touched a schema input reconciles the collection schemas
    /// exactly once; every other batch does nothing here. The return value tells
    /// the watcher loop whether an index scan is still required, so schema-only
    /// traffic never triggers a project walk.
    pub(super) fn handle_watcher_batch(
        &mut self,
        batch: WatchBatch,
        cx: &mut Context<Self>,
    ) -> bool {
        if batch.schema {
            self.load_collection_schemas(cx);
        }
        if self.git_monitoring && (batch.changed || batch.needs_rescan) {
            self.refresh_git(cx);
        }
        batch.changed || batch.needs_rescan
    }

    pub(super) fn start_watcher(
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
        let document_fingerprints =
            Arc::new(std::sync::Mutex::new(DocumentFingerprints::default()));
        self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                // Poll for a merged batch off the render path. The scan itself is a
                // separate index request so it can be ordered against rescans.
                // Schema invalidation rides on the same batch: a schema-only batch
                // reconciles once and performs no project walk.
                let polled = weak_watcher.clone();
                let pending = runtime::blocking(cx, move || {
                    let Some(watcher) = polled.upgrade() else {
                        return Err("Watcher stopped".to_string());
                    };
                    Ok(watcher.drain())
                })
                .await
                .and_then(|result| result);
                let batch = match pending {
                    Ok(batch) => batch,
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
                };
                if !batch.changed && !batch.needs_rescan && !batch.schema {
                    continue;
                }
                let rescan = this
                    .update_in(cx, |this, _, cx| {
                        if this.generation != generation
                            || this.project_id.as_ref() != Some(&project)
                        {
                            return None;
                        }
                        Some(this.handle_watcher_batch(batch, cx))
                    })
                    .ok()
                    .flatten();
                let Some(rescan) = rescan else {
                    break;
                };
                if !rescan {
                    continue;
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
                let cached = document_fingerprints.clone();
                let result = runtime::blocking(cx, move || {
                    request
                        .cancellation()
                        .check()
                        .map_err(|error| error.to_string())?;
                    let Some(watcher) = scanning.upgrade() else {
                        return Err("Watcher stopped".to_string());
                    };
                    // Compare retained and in-flight sources even when reindexing fails.
                    let mut cache = cached.lock().unwrap_or_else(|error| error.into_inner());
                    cache.retain_paths(&snapshots);
                    let now = std::time::SystemTime::now();
                    let fingerprints = snapshots
                        .into_iter()
                        .map(|path| {
                            let fingerprint = contained_path(
                                &scan_root,
                                path.strip_prefix(&scan_root).unwrap_or(&path),
                            )
                            .ok()
                            .filter(|actual| actual == &path)
                            .and_then(|actual| cache.sample(&actual, now));
                            (path, fingerprint)
                        })
                        .collect::<Vec<_>>();
                    drop(cache);
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
                        this.apply_watcher_scan(generation, token, result, cx);
                        true
                    })
                    .unwrap_or(false);
                if !still_current {
                    break;
                }
            }
        }));
    }

    /// Publish one watcher scan unless a newer project generation or index
    /// request already won.
    ///
    /// The generation check is repeated here, after the scan returns, because a
    /// project switch resets [`Self::published_index_request`] to zero: an old
    /// loop's token then compares as "not older" and would otherwise publish the
    /// outgoing project's entries into the incoming one. External-change flags
    /// and session reconciliation follow the same ordering, so a superseded scan
    /// can never close a clean session that the newer index still contains.
    pub(super) fn apply_watcher_scan(
        &mut self,
        generation: u64,
        token: u64,
        scan: Result<WatcherScan, String>,
        cx: &mut Context<Self>,
    ) {
        if self.generation != generation || !self.accepts_index_result(token) {
            tracing::debug!(
                generation,
                current_generation = self.generation,
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

    pub(super) fn publish_entries(
        &mut self,
        entries: Vec<CollectionEntry>,
        cx: &mut Context<Self>,
    ) {
        self.index_revision = self
            .index_revision
            .checked_add(1)
            .expect("index revision exhausted");
        self.loads.clear();
        self.entries = Arc::new(entries);
        self.collection_counts.clear();
        for entry in self.entries.iter() {
            *self
                .collection_counts
                .entry(entry.entry().id().collection().as_str().to_owned())
                .or_default() += 1;
        }
        if let Some(selected) = self.selected_collection.as_deref()
            && !self.manifest.as_ref().is_some_and(|manifest| {
                manifest
                    .collections()
                    .iter()
                    .any(|descriptor| descriptor.id().as_str() == selected)
            })
        {
            self.selected_collection = None;
        }
        self.normalize_browsed_directory();
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
        self.rebuild_collection_summaries();
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

    /// Keep one entry command task alive until it settles.
    ///
    /// Commands never share the index slot, so starting a copy-path, reveal or
    /// another command cannot cancel an in-flight rescan or another command.
    pub(super) fn retain_command_task(&mut self, task: Task<()>) {
        self.command_tasks.retain(|task| !task.is_ready());
        self.command_tasks.push(task);
    }

    /// Claim the next index request token and mark the index busy.
    ///
    /// Every published index result carries the token of the request that produced
    /// it, so an older watcher scan can never overwrite a newer operation rescan.
    pub(super) fn begin_index_request(&mut self) -> u64 {
        let next = self
            .index_request
            .checked_add(1)
            .expect("index request sequence exhausted");
        self.index_request = next;
        self.refreshing = true;
        next
    }

    /// Whether a result produced by `token` may publish.
    pub(super) fn accepts_index_result(&self, token: u64) -> bool {
        token >= self.published_index_request
    }

    /// Clear the busy flag unless a newer index request is still outstanding.
    pub(super) fn finish_index_request(&mut self, token: u64) {
        if token == self.index_request {
            self.refreshing = false;
        }
    }

    /// Publish a discovered manifest and entry index.
    pub(super) fn apply_discovered_index(
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
        self.sync_file_browser_root(cx);
        self.index_error = None;
        self.publish_entries(entries, cx);
        self.reconcile_sessions(cx);
        self.load_data_references(cx);
        self.rebuild_references();
        self.load_components(cx);
        if !diagnostics.is_empty() {
            self.status = format!("Project diagnostics: {}", diagnostics.join("; "));
        }
    }

    /// Publish an unavailable index while every open buffer is retained.
    pub(super) fn apply_index_failure(&mut self, error: String, cx: &mut Context<Self>) {
        self.index_error = Some(error);
        self.publish_entries(Vec::new(), cx);
        self.status = "Project index unavailable. Open buffers retained.".into();
    }

    /// Start one index request with the currently applied overrides.
    ///
    /// The returned task publishes only when its result is not older than the
    /// last published index.
    pub(super) fn start_index_request(&mut self, cx: &mut Context<Self>) -> Option<Task<()>> {
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
    pub(super) fn apply_index_result(
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
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use gpui_kit::TestAppContext;

    #[test]
    fn document_fingerprints_reuse_settled_samples_and_detect_edits() {
        use std::{fs::FileTimes, time::Duration};

        fn set_modified(path: &std::path::Path, time: std::time::SystemTime) {
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(time))
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "alpha").unwrap();
        let mtime = std::time::SystemTime::now() - Duration::from_secs(10);
        set_modified(&path, mtime);
        let mut cache = DocumentFingerprints::default();
        let first = cache.sample(&path, mtime + Duration::from_secs(5)).unwrap();
        assert_eq!(cache.reads, 1);

        // The same length and timestamp with a settled mtime reuses the
        // sample instead of reading the file again.
        assert_eq!(
            cache.sample(&path, mtime + Duration::from_secs(6)),
            Some(first)
        );
        assert_eq!(cache.reads, 1, "a settled sample is not re-read");

        // A same-length edit that restores its timestamp is still inside the
        // racy window, so it is re-read and the change is reported.
        std::fs::write(&path, "bravo").unwrap();
        set_modified(&path, mtime);
        let edited = cache.sample(&path, mtime + Duration::from_millis(500));
        assert_eq!(cache.reads, 2, "a racy timestamp forces a read");
        assert_ne!(edited, Some(first), "a real content change is detected");

        // A different length is detected even with a stale mtime.
        std::fs::write(&path, "bravo and more").unwrap();
        set_modified(&path, mtime);
        assert_ne!(
            cache.sample(&path, mtime + Duration::from_secs(10)),
            Some(first),
            "a size change is always re-read"
        );
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
    async fn schema_reconciliation_runs_once_per_watcher_batch_without_polling(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        for _ in 0..100 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| !view.schema_in_flight) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        let settled = view.read_with(cx, |view, _| {
            assert!(
                !view.schema_in_flight,
                "the initial schema generation must finish"
            );
            assert_eq!(
                view.schema_reconciliations, 1,
                "the open reconciles the schemas exactly once"
            );
            view.schema_reconciliations
        });

        // The old implementation probed every 750 ms forever. No timer may start
        // another pass once generation has settled.
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(120));
        cx.run_until_parked();
        runtime::blocking(cx, || {
            std::thread::sleep(std::time::Duration::from_millis(50))
        })
        .await
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.schema_reconciliations),
            settled,
            "an idle project must not reconcile schemas periodically"
        );

        view.update(cx, |view, cx| {
            assert!(
                !view.handle_watcher_batch(WatchBatch::default(), cx),
                "a batch with nothing relevant does not rescan the index"
            );
            assert_eq!(view.schema_reconciliations, settled);
            assert!(
                !view.handle_watcher_batch(
                    WatchBatch {
                        schema: true,
                        ..WatchBatch::default()
                    },
                    cx
                ),
                "a schema-only batch must not walk the project"
            );
            assert_eq!(
                view.schema_reconciliations,
                settled + 1,
                "a schema batch reconciles exactly once"
            );
        });
    }

    #[gpui_kit::test]
    async fn superseded_watcher_generation_publishes_nothing(cx: &mut TestAppContext) {
        let outgoing = fixture();
        let incoming = fixture();
        std::fs::write(
            incoming.path().join("src/posts/c.md"),
            "---\ntitle: Incoming\n---\n# C\n",
        )
        .unwrap();

        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.start(outgoing.path().into(), window, cx)
            })
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        let (stale_generation, stale_manifest, stale_entries) = view.read_with(cx, |view, _| {
            (
                view.generation,
                view.manifest
                    .as_ref()
                    .expect("the first project is open")
                    .as_ref()
                    .clone(),
                view.entries.clone(),
            )
        });

        // Queue a batch for the outgoing watcher, then switch before the loop
        // can drain it. The old loop must never publish into the new project.
        std::fs::write(
            outgoing.path().join("src/posts/queued.md"),
            "---\ntitle: Queued\n---\n# Queued\n",
        )
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.start(incoming.path().into(), window, cx)
            })
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;
        let (current_generation, entries, revision) = view.read_with(cx, |view, _| {
            (view.generation, view.entries.clone(), view.index_revision)
        });
        assert_ne!(current_generation, stale_generation);
        assert!(
            entries
                .iter()
                .any(|entry| entry.entry().source().as_str() == "src/posts/c.md"),
            "the incoming project owns the published index"
        );

        view.update(cx, |view, cx| {
            view.apply_watcher_scan(
                stale_generation,
                1,
                Ok(WatcherScan {
                    index: Ok((stale_manifest, (*stale_entries).clone())),
                    fingerprints: Vec::new(),
                }),
                cx,
            );
        });
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.generation, current_generation,
                "the outgoing loop cannot replace the project generation"
            );
            assert_eq!(
                view.index_revision, revision,
                "a superseded watcher generation must not publish"
            );
            assert!(
                view.entries
                    .iter()
                    .all(|entry| entry.entry().source().as_str() != "src/posts/queued.md"),
                "the outgoing project's queued entry never appears"
            );
        });

        // The same publication path stays live for the owning generation.
        let (manifest, entries) = view.read_with(cx, |view, _| {
            (
                view.manifest
                    .as_ref()
                    .expect("the second project is open")
                    .as_ref()
                    .clone(),
                view.entries.clone(),
            )
        });
        view.update(cx, |view, cx| {
            view.apply_watcher_scan(
                current_generation,
                2,
                Ok(WatcherScan {
                    index: Ok((manifest, (*entries).clone())),
                    fingerprints: Vec::new(),
                }),
                cx,
            );
        });
        assert!(
            view.read_with(cx, |view, _| view.index_revision) > revision,
            "the current generation still publishes watcher scans"
        );
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
        let (stale_generation, stale_token) =
            view.update(cx, |view, _| (view.generation, view.begin_index_request()));

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
                stale_generation,
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
                stale_generation,
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
}
