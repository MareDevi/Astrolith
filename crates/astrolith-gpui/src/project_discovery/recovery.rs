//! Recovery persistence and restore of unsaved sessions.

use super::*;

const RECOVERY_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
const RECOVERY_RETRY_MIN: std::time::Duration = std::time::Duration::from_secs(1);
const RECOVERY_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) enum RecoveryOperation {
    Save(Arc<RecoveryRecord>),
    Clear,
    ClearAfterSave(Arc<str>),
}

impl RecoveryOperation {
    fn apply(
        &self,
        store: &RecoveryStore,
        identity: &DocumentIdentity,
        ticket: RecoveryTicket,
    ) -> Result<(), astrolith_document::DocumentError> {
        match self {
            Self::Save(record) => store.save_at(record, ticket),
            Self::Clear => store.clear_at(identity, ticket).map(drop),
            Self::ClearAfterSave(saved_content) => store
                .clear_after_save_at(identity, saved_content, ticket)
                .map(drop),
        }
    }
}

#[derive(Clone)]
pub(super) enum RecoveryWork {
    Debounced,
    Fixed(Arc<RecoveryOperation>),
}

impl ProjectWorkspace {
    pub(super) fn schedule_recovery_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(store) = self.recovery.clone() else {
            self.recovery_tasks.clear();
            return;
        };
        let Some(session) = self.sessions.get(path).cloned() else {
            return;
        };
        let (revision, dirty, identity) = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            (
                session.buffer.revision().get(),
                session.buffer.is_dirty(),
                session.buffer.identity().clone(),
            )
        });
        if self
            .recovery_tasks
            .get(path)
            .is_some_and(|task| task.job.revision == revision && task.job.dirty == dirty)
        {
            return;
        }
        let ticket = store.next_ticket();
        let job = RecoveryJob {
            path: path.to_path_buf(),
            identity,
            revision,
            dirty,
            work: RecoveryWork::Debounced,
            ticket,
        };
        let task = Self::spawn_recovery_task(job.clone(), RECOVERY_DEBOUNCE, store, cx);
        self.recovery_tasks
            .insert(path.to_path_buf(), RecoveryTask { job, _task: task });
    }

    fn spawn_recovery_task(
        job: RecoveryJob,
        initial_delay: std::time::Duration,
        store: RecoveryStore,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let RecoveryJob {
            path,
            identity,
            revision,
            dirty,
            work,
            ticket,
        } = job;
        cx.spawn(async move |this, cx| {
            let mut delay = initial_delay;
            let mut cached = match work {
                RecoveryWork::Debounced => None,
                RecoveryWork::Fixed(operation) => Some((identity.clone(), operation)),
            };
            loop {
                cx.background_executor().timer(delay).await;
                let current = this
                    .update(cx, |this, _| {
                        this.recovery_tasks
                            .get(&path)
                            .is_some_and(|task| task.job.ticket == ticket)
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
                if cached.is_none() {
                    let Some(captured) = this
                        .update(cx, |this, cx| {
                            let session = this.sessions.get(&path)?.clone();
                            let identity = session.read(cx).buffer.identity().clone();
                            let dirty = session.update(cx, |session, cx| {
                                session.sync_buffer(cx);
                                session.buffer.is_dirty()
                            });
                            let operation = if dirty {
                                Arc::new(RecoveryOperation::Save(Arc::new(
                                    RecoveryRecord::capture(&session.read(cx).buffer, now_secs()),
                                )))
                            } else {
                                Arc::new(RecoveryOperation::Clear)
                            };
                            Some((identity, operation))
                        })
                        .ok()
                        .flatten()
                    else {
                        return;
                    };
                    cached = Some(captured);
                }
                let Some((write_identity, write_operation)) = cached.clone() else {
                    return;
                };
                let write_store = store.clone();
                let result = runtime::blocking(cx, move || {
                    write_operation.apply(&write_store, &write_identity, ticket)
                })
                .await;
                match result {
                    Ok(Ok(())) => {
                        tracing::debug!(
                            path = %path.display(),
                            source = %identity.canonical_source().as_str(),
                            revision,
                            dirty,
                            "recovery task completed"
                        );
                        return;
                    }
                    Ok(Err(error)) => tracing::warn!(
                        path = %path.display(),
                        source = %identity.canonical_source().as_str(),
                        %error,
                        "recovery task failed; retrying"
                    ),
                    Err(error) => tracing::warn!(
                        path = %path.display(),
                        source = %identity.canonical_source().as_str(),
                        %error,
                        "recovery service task failed; retrying"
                    ),
                }
                #[cfg(test)]
                let _ = this.update(cx, |this, _| this.recovery_failures += 1);
                let superseded = this
                    .update(cx, |this, _| {
                        !this
                            .recovery_tasks
                            .get(&path)
                            .is_some_and(|task| task.job.ticket == ticket)
                    })
                    .unwrap_or(true);
                if superseded {
                    return;
                }
                delay = (delay * 2).max(RECOVERY_RETRY_MIN).min(RECOVERY_RETRY_MAX);
            }
        })
    }

    fn install_recovery_clear(
        &mut self,
        path: &Path,
        identity: DocumentIdentity,
        revision: u64,
        operation: Arc<RecoveryOperation>,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.recovery.clone() else {
            self.recovery_tasks.remove(path);
            return;
        };
        let ticket = store.next_ticket();
        let job = RecoveryJob {
            path: path.to_path_buf(),
            identity,
            revision,
            dirty: false,
            work: RecoveryWork::Fixed(operation),
            ticket,
        };
        let task = Self::spawn_recovery_task(job.clone(), std::time::Duration::ZERO, store, cx);
        self.recovery_tasks
            .insert(path.to_path_buf(), RecoveryTask { job, _task: task });
    }

    pub(super) fn clear_recovery_record(
        &mut self,
        path: &Path,
        identity: DocumentIdentity,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        self.install_recovery_clear(
            path,
            identity,
            revision,
            Arc::new(RecoveryOperation::Clear),
            cx,
        );
    }

    pub(super) fn clear_recovery_after_save(
        &mut self,
        path: &Path,
        identity: DocumentIdentity,
        saved_content: String,
        revision: u64,
        ticket: RecoveryTicket,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.recovery.clone() else {
            self.recovery_tasks.remove(path);
            return;
        };
        let work = RecoveryWork::Fixed(Arc::new(RecoveryOperation::ClearAfterSave(Arc::from(
            saved_content,
        ))));
        let job = RecoveryJob {
            path: path.to_path_buf(),
            identity,
            revision,
            dirty: false,
            work,
            ticket,
        };
        let task = Self::spawn_recovery_task(job.clone(), std::time::Duration::ZERO, store, cx);
        self.recovery_tasks
            .insert(path.to_path_buf(), RecoveryTask { job, _task: task });
    }

    fn capture_dirty_recovery_on_quit(
        &mut self,
        store: &RecoveryStore,
        cx: &mut Context<Self>,
    ) -> Vec<(RecoveryRecord, RecoveryTicket)> {
        let now = now_secs();
        let mut captures = Vec::new();
        for session in self.sessions.values().cloned().collect::<Vec<_>>() {
            let dirty = session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                session.buffer.is_dirty()
            });
            if dirty {
                let record = RecoveryRecord::capture(&session.read(cx).buffer, now);
                captures.push((record, store.next_ticket()));
            }
        }
        captures
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
    /// when recovery is disabled and no recovery work is pending.
    pub fn flush_recovery_on_quit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<(), String>>> {
        let store = self.recovery.clone()?;
        let mut cleanups = self
            .recovery_tasks
            .values()
            .filter_map(|task| match &task.job.work {
                RecoveryWork::Debounced => None,
                RecoveryWork::Fixed(operation) => Some((
                    task.job.ticket,
                    task.job.identity.clone(),
                    operation.clone(),
                )),
            })
            .collect::<Vec<_>>();
        cleanups.sort_by_key(|(ticket, _, _)| *ticket);
        self.recovery_tasks.clear();
        let captures = self.capture_dirty_recovery_on_quit(&store, cx);
        if cleanups.is_empty() && captures.is_empty() {
            return None;
        }
        tracing::debug!(
            cleanups = cleanups.len(),
            documents = captures.len(),
            "flushing recovery work on exit"
        );
        Some(runtime::flush_on_quit(cx, move || {
            for (ticket, identity, operation) in cleanups {
                if let Err(error) = operation.apply(&store, &identity, ticket) {
                    tracing::error!(
                        source = %identity.canonical_source().as_str(),
                        %error,
                        "could not finish a recovery cleanup on exit"
                    );
                }
            }
            for (record, ticket) in captures {
                if let Err(error) = store.save_at(&record, ticket) {
                    tracing::error!(
                        source = %record.identity().canonical_source().as_str(),
                        %error,
                        "could not flush a recovery record on exit"
                    );
                }
            }
        }))
    }

    pub(super) fn load_pending_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                this.status = tr!("browser.recovery.restoring", count = count);
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
            self.status = tr!("browser.recovery.none");
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
                let Ok(disk) = read_bounded(&actual, MAX_DOCUMENT_BYTES) else {
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
                    record.baseline().map(str::to_owned),
                    record.revision(),
                ));
            }
            loaded
        });
        self.restore_task = Some(cx.spawn_in(window, async move |this, cx| {
            let loaded = reads.await.unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                let mut restored = 0;
                let mut conflicts = 0;
                for (path, identity, disk, content, baseline, revision) in loaded {
                    if this.sessions.contains_key(&path) {
                        this.activate_session(&path, window, cx);
                        restored += 1;
                        continue;
                    }
                    // A record that predates baseline capture falls back to the
                    // current disk, which is the pre-DOC-03 behaviour. A record
                    // whose baseline differs from the disk loads as a conflict
                    // so its first save cannot silently overwrite that edit.
                    let original = baseline.unwrap_or_else(|| disk.clone());
                    let session = cx.new(|cx| {
                        DocumentSession::from_recovered_at(
                            identity, original, content, disk, revision, window, cx,
                        )
                    });
                    if session.read(cx).externally_changed {
                        conflicts += 1;
                    }
                    let copyedit = this.copyedit.clone();
                    session.update(cx, |session, cx| session.set_copyedit_config(copyedit, cx));
                    let key = path.clone();
                    this.watch_session(key.clone(), session.clone(), window, cx);
                    this.sessions.insert(key.clone(), session.clone());
                    this.active = Some(key.clone());
                    this.install(&key, session.clone(), cx);
                    restored += 1;
                }
                // Only the document left active gets the Build presentation and
                // its language server; the others switch when activated.
                if let Some(path) = this.active.clone()
                    && let Some(session) = this.sessions.get(&path).cloned()
                {
                    let source_view = this.source_view;
                    session.update(cx, |session, cx| {
                        session.set_source_view(source_view, window, cx)
                    });
                    this.sync_lsp(cx);
                }
                tracing::info!(restored, conflicts, "recovered unsaved documents");
                this.status = if conflicts == 0 {
                    tr!("browser.recovery.restored", restored = restored)
                } else {
                    tr!(
                        "browser.recovery.restored_conflicts",
                        restored = restored,
                        conflicts = conflicts
                    )
                };
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use gpui_kit::TestAppContext;

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
    async fn recovery_detects_a_newer_disk_and_its_first_save_conflicts(cx: &mut TestAppContext) {
        let dir = fixture();
        let path = dir.path().join("src/posts/a.md");
        // The disk moved on after the record captured the "# A\n" baseline.
        std::fs::write(&path, "# External\n").unwrap();
        let recovery_dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery_dir.path());
        let identity = DocumentIdentity::new(
            ProjectId::new("workspace-1").unwrap(),
            1,
            SourcePath::new("src/posts/a.md").unwrap(),
        );
        store
            .save(&RecoveryRecord::new_with_baseline(
                identity,
                DocumentRevision::new(4),
                "# Recovered\n",
                Some("# A\n".to_owned()),
                1,
            ))
            .unwrap();

        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        open_project_and_wait(cx, handle, &view, dir.path()).await;
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
            let session = view.sessions.get(&path).expect("the record must restore");
            let session = session.read(cx);
            assert_eq!(session.editor.read(cx).value(), "# Recovered\n");
            assert_eq!(
                session.buffer.original_text(),
                "# A\n",
                "the record's baseline must be restored, not the current disk"
            );
            assert_eq!(session.buffer.revision(), DocumentRevision::new(4));
            assert!(session.buffer.is_dirty());
            assert!(
                session.externally_changed,
                "a newer disk edit is an unresolved conflict"
            );
            assert_eq!(
                session.disk_fingerprint,
                crate::editor::session::fingerprint("# External\n")
            );
            assert!(view.status.contains("changed on disk"), "{}", view.status);
        });

        // The first save must not overwrite the newer disk bytes.
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
            assert!(view.sessions.get(&path).unwrap().read(cx).buffer.is_dirty());
            assert_eq!(
                view.save_coordinator.status(&path),
                Some(SaveStatus::Conflict)
            );
        });
    }

    #[gpui_kit::test]
    async fn recovery_with_a_matching_baseline_saves_without_a_false_conflict(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let path = dir.path().join("src/posts/a.md");
        let recovery_dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery_dir.path());
        let identity = DocumentIdentity::new(
            ProjectId::new("workspace-1").unwrap(),
            1,
            SourcePath::new("src/posts/a.md").unwrap(),
        );
        store
            .save(&RecoveryRecord::new_with_baseline(
                identity,
                DocumentRevision::new(2),
                "# Recovered\n",
                Some("# A\n".to_owned()),
                1,
            ))
            .unwrap();

        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.condition(&view, |view, _| view.sessions.contains_key(&path))
            .await;
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(session.buffer.original_text(), "# A\n");
            assert_eq!(session.buffer.revision(), DocumentRevision::new(2));
            assert!(session.buffer.is_dirty());
            assert!(
                !session.externally_changed,
                "an unchanged disk is not a conflict"
            );
        });

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, cx| {
            view.sessions
                .get(&path)
                .is_some_and(|session| !session.read(cx).buffer.is_dirty())
        })
        .await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Recovered\n");
        view.read_with(cx, |view, cx| {
            assert!(!view.status.contains("conflict"), "{}", view.status);
            assert!(!view.sessions.get(&path).unwrap().read(cx).buffer.is_dirty());
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
    async fn failed_recovery_capture_is_retried(cx: &mut TestAppContext) {
        let dir = fixture();
        let parent = tempfile::tempdir().unwrap();
        let recovery_path = parent.path().join("recovery");
        std::fs::write(&recovery_path, "blocks create_dir_all").unwrap();
        let store = RecoveryStore::new(&recovery_path);
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        open_fixture_entry(cx, handle, &view, &dir).await;

        edit_active(cx, handle, &view, "# Retry recovery\n").await;
        wait_for(cx, |cx| {
            view.read_with(cx, |view, _| view.recovery_failures > 0)
        })
        .await;
        assert!(recovery_path.is_file(), "the first write must have failed");

        std::fs::remove_file(&recovery_path).unwrap();
        wait_for(cx, |_| {
            store.load().is_ok_and(|load| {
                load.records()
                    .iter()
                    .any(|record| record.content() == "# Retry recovery\n")
            })
        })
        .await;
    }

    #[gpui_kit::test]
    async fn quitting_finishes_a_discarded_documents_pending_clear(cx: &mut TestAppContext) {
        let dir = fixture();
        let recovery_dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery_dir.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        let path = open_fixture_entry(cx, handle, &view, &dir).await;

        edit_active(cx, handle, &view, "# Discard me\n").await;
        wait_for(cx, |_| !store.load().unwrap().is_empty()).await;

        view.update(cx, |view, cx| view.remove_session(&path, cx));
        let flush = view
            .update(cx, |view, cx| view.flush_recovery_on_quit(cx))
            .expect("the pending discard cleanup must be flushed on exit");
        flush.await.unwrap();

        assert!(
            store.load().unwrap().is_empty(),
            "quit must finish the clear for a discarded closed document"
        );
    }
}
