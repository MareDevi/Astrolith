//! Recovery persistence and restore of unsaved sessions.

use super::*;

impl ProjectWorkspace {
    pub(super) fn schedule_recovery(&mut self, cx: &mut Context<Self>) {
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
                    format!("Restored {restored} unsaved document(s) from recovery.")
                } else {
                    format!(
                        "Restored {restored} unsaved document(s) from recovery. {conflicts} changed on disk since the record; the recovery content is kept and a save will conflict until you resolve it."
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
}
