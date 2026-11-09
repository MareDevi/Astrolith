//! Guarded save lifecycle: coordinator requests, completion, conflicts and failure artifacts.
use super::*;

/// Redacted class for a document error; the message never reaches an artifact.
pub(super) fn save_failure_class(error: &astrolith_document::DocumentError) -> &'static str {
    use astrolith_document::DocumentError;
    match error {
        DocumentError::Io { .. } => "io",
        DocumentError::TooLarge { .. } => "too-large",
        DocumentError::RevisionOverflow => "revision",
        DocumentError::InvalidEdit { .. } => "invalid-edit",
        DocumentError::InvalidFrontmatter
        | DocumentError::UnsupportedFrontmatter { .. }
        | DocumentError::UnknownField { .. }
        | DocumentError::UnsupportedPatch { .. } => "frontmatter",
    }
}

/// The file name used in status text for one document path.
pub(super) fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One completed guarded save, as the workspace applies it.
pub(super) enum SaveAttempt {
    /// The revision was persisted; `persisted` is the exact text written, so the
    /// buffer baseline can follow the file when newer edits already exist.
    Saved { revision: u64, persisted: String },
    /// The file changed on disk; nothing was written.
    Conflict,
}

/// What to do after a dirty session has been saved through the guarded path.
pub(super) enum PendingAfterSave {
    Rename { from: PathBuf, to: SourcePath },
    Duplicate { path: PathBuf },
    Trash { path: PathBuf },
    Close { path: PathBuf },
}

impl ProjectWorkspace {
    /// Save a dirty buffer because its editor lost focus (ledger row 44).
    pub(super) fn save_on_blur(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A modal decision may still end in Discard, which must not race a
        // background write; the dialog's own Save button starts the save.
        // A save already in flight keeps this request as its follow-up.
        if window.has_active_dialog(cx) {
            return;
        }
        let Some(session) = self.sessions.get(path).cloned() else {
            return;
        };
        let revision = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            session.buffer.revision().get()
        });
        if let Some(request) = self.save_coordinator.request_now(
            path,
            revision,
            cx.background_executor().now(),
            SaveKind::Automatic,
        ) {
            self.begin_save(request, window, cx);
        }
        self.arm_autosave(cx);
        cx.notify();
    }

    /// Arm the timer for the earliest autosave deadline.
    pub(super) fn arm_autosave(&mut self, cx: &mut Context<Self>) {
        self.autosave_task = None;
        let now = cx.background_executor().now();
        let Some(deadline) = self.save_coordinator.next_deadline(now) else {
            return;
        };
        let Some(handle) = self.window_handle else {
            return;
        };
        self.autosave_task = Some(cx.spawn(async move |this, cx| {
            let delay = deadline.saturating_duration_since(cx.background_executor().now());
            cx.background_executor().timer(delay).await;
            let _ = handle.update(cx, |_root, window, app| {
                let _ = this.update(app, |this, cx| this.pump_autosave(window, cx));
            });
        }));
    }

    /// Start every save whose deadline has passed.
    pub(crate) fn pump_autosave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let requests = self.save_coordinator.due(cx.background_executor().now());
        for request in requests {
            self.begin_save(request, window, cx);
        }
        self.arm_autosave(cx);
        self.advance_switch(window, cx);
        cx.notify();
    }

    /// Run one coordinator request through the guarded atomic save.
    pub(super) fn begin_save(
        &mut self,
        request: SaveRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = request.path.clone();
        let Some(session) = self.sessions.get(&path).cloned() else {
            self.save_coordinator.forget(&path);
            return;
        };
        // Capture the newest editor text so a queued `Change` cannot make the
        // snapshot stale.
        let (revision, dirty) = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            (session.buffer.revision().get(), session.buffer.is_dirty())
        });
        self.save_coordinator
            .observe(&path, revision, dirty, cx.background_executor().now());
        let snapshot = DocumentSnapshot::capture(&session.read(cx).buffer, path.clone());
        tracing::debug!(
            path = %path.display(),
            revision = snapshot.revision().get(),
            ticket = ?request.ticket,
            kind = ?request.kind,
            "starting guarded save"
        );
        if request.kind == SaveKind::Manual {
            self.status = tr!("browser.save.saving");
        }
        cx.notify();
        // Retain the bytes this attempt persists: a newer revision typed during
        // the save must rebase its baseline onto them, or its follow-up would
        // conflict with our own write.
        let persisted = snapshot.content().to_owned();
        let save = runtime::spawn(cx, async move {
            match save_snapshot(&snapshot).await {
                Ok(SaveOutcome::Saved { revision }) => Ok(SaveAttempt::Saved {
                    revision,
                    persisted,
                }),
                Ok(SaveOutcome::Conflict) => Ok(SaveAttempt::Conflict),
                Err(error) => Err((save_failure_class(&error), error.to_string())),
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = match save.await {
                Ok(result) => result,
                Err(message) => Err(("service", message)),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_save(request, result, window, cx);
            });
        });
        self.save_tasks.insert(path, task);
    }

    /// Apply one guarded save result, dropping stale tickets.
    pub(super) fn finish_save(
        &mut self,
        request: SaveRequest,
        result: Result<SaveAttempt, (&'static str, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = request.path.clone();
        self.save_tasks.remove(&path);
        let (outcome, persisted, class) = match result {
            Ok(SaveAttempt::Saved {
                revision,
                persisted,
            }) => (SaveResult::Saved { revision }, Some(persisted), None),
            Ok(SaveAttempt::Conflict) => (SaveResult::Conflict, None, None),
            Err((class, message)) => (SaveResult::Failed { message }, None, Some(class)),
        };
        let completion = self.save_coordinator.complete(
            &path,
            request.ticket,
            outcome,
            cx.background_executor().now(),
        );
        if completion == SaveCompletion::Stale {
            tracing::debug!(path = %path.display(), "dropped a stale save completion");
            return;
        }
        // One artifact per failure episode, never one per retry.
        if let (Some(class), SaveCompletion::Failed { first: true, .. }) = (class, &completion) {
            self.record_save_failure(&path, class, cx);
        }
        self.apply_save_completion(
            &path,
            &request,
            persisted.as_deref(),
            completion,
            window,
            cx,
        );
        self.arm_autosave(cx);
        self.advance_switch(window, cx);
        self.advance_quit(window, cx);
        cx.notify();
    }

    pub(super) fn apply_save_completion(
        &mut self,
        path: &std::path::Path,
        request: &SaveRequest,
        persisted: Option<&str>,
        completion: SaveCompletion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.sessions.get(path).cloned() else {
            self.save_coordinator.forget(path);
            self.pending_after_save.clear();
            self.status = tr!("browser.save.closed_before_finish");
            return;
        };
        match completion {
            SaveCompletion::Saved {
                revision,
                current,
                manual,
            } => {
                tracing::info!(path = %path.display(), revision, current, "document saved");
                let identity = session.read(cx).buffer.identity().clone();
                let persisted = persisted.unwrap_or_default();
                let (confirmed, content) = session.update(cx, |session, cx| {
                    // A newer edit after the snapshot may still be pending;
                    // materialize it so `confirm_saved` sees the newer revision.
                    session.sync_buffer(cx);
                    let content = session.buffer.current_text().to_owned();
                    let confirmed =
                        session.confirm_saved(DocumentRevision::new(revision), persisted);
                    if !confirmed {
                        // Newer edits exist: move the baseline onto what the disk
                        // now holds so their follow-up save does not conflict
                        // with our own write. The buffer stays dirty.
                        session.rebase_persisted(DocumentRevision::new(revision), persisted);
                    }
                    // Both `confirm_saved` and `rebase_persisted` already clear
                    // the external-change flag and adopt the persisted bytes.
                    session.dirty = session.buffer.is_dirty();
                    cx.notify();
                    (confirmed, content)
                });
                if !confirmed {
                    // The buffer is the authority on dirtiness, so the
                    // coordinator follows it instead of assuming newer content.
                    let dirty = session.read(cx).buffer.is_dirty();
                    let revision = session.read(cx).buffer.revision().get();
                    self.save_coordinator.observe(
                        path,
                        revision,
                        dirty,
                        cx.background_executor().now(),
                    );
                }
                if confirmed && let Some(store) = self.recovery.clone() {
                    let task = runtime::blocking(cx, move || {
                        let _ = store.clear_after_save(&identity, &content);
                    });
                    task.detach();
                }
                if request.kind == SaveKind::Manual || manual {
                    self.status = if current && confirmed {
                        tr!("browser.save.saved")
                    } else {
                        tr!("browser.save.saved_earlier")
                    };
                }
                self.update_tab_state(path, cx);
                cx.emit(SessionChanged);
                self.settle_pending_after_save(path, confirmed, window, cx);
            }
            SaveCompletion::Conflict => {
                tracing::warn!(
                    path = %path.display(),
                    "save conflict: the file changed on disk; edits retained"
                );
                session.update(cx, |session, cx| {
                    session.externally_changed = true;
                    cx.notify();
                });
                self.status = tr!("browser.save.conflict");
                self.update_tab_state(path, cx);
                cx.emit(SessionChanged);
                self.abandon_pending_after_save(path);
            }
            SaveCompletion::Failed { message, .. } => {
                tracing::error!(path = %path.display(), %message, "document save failed");
                self.status = tr!("browser.save.failed", error = message);
                self.update_tab_state(path, cx);
                cx.emit(SessionChanged);
                self.schedule_recovery(cx);
                self.abandon_pending_after_save(path);
            }
            SaveCompletion::Stale => {}
        }
    }

    /// Record a redacted artifact for a failed save.
    ///
    /// The artifact is written off the render path and contains no document
    /// content; recording never changes the save outcome.
    pub(super) fn record_save_failure(
        &mut self,
        path: &std::path::Path,
        error_class: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(directory) = self.save_failure_dir.clone() else {
            return;
        };
        let source = self
            .project_root()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(|relative| relative.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let artifact = SaveFailureArtifact::new(error_class, source);
        let task = runtime::blocking(cx, move || artifact.write(&directory));
        task.detach();
    }

    /// Run a lifecycle action that was waiting for this document's save.
    pub(super) fn settle_pending_after_save(
        &mut self,
        path: &std::path::Path,
        marked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = self.pending_after_save.remove(path) else {
            return;
        };
        if marked {
            self.run_after_save(action, window, cx);
        } else {
            self.run_after_saved_revision(action, window, cx);
        }
    }

    /// Drop a lifecycle action whose save conflicted or failed.
    ///
    /// A conflict or failure leaves the document dirty, so the action must wait
    /// for a later save: a rename is never applied over an unsaved revision and a
    /// close never destroys it. The status names the consequence so the user
    /// knows the operation did not happen.
    pub(super) fn abandon_pending_after_save(&mut self, path: &std::path::Path) {
        let Some(pending) = self.pending_after_save.remove(path) else {
            return;
        };
        let consequence = match pending {
            PendingAfterSave::Rename { .. } => tr!("browser.save.rename_not_applied"),
            PendingAfterSave::Duplicate { .. } => tr!("browser.save.not_duplicated"),
            PendingAfterSave::Trash { .. } => tr!("browser.save.not_trashed"),
            PendingAfterSave::Close { .. } => tr!("browser.save.document_stays_open"),
        };
        self.status = tr!(
            "browser.save.with_consequence",
            status = self.status.clone(),
            consequence = consequence
        );
    }

    /// Save the active document with conflict detection.
    pub fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active.clone() else {
            self.status = tr!("browser.save.no_document_to_save");
            cx.notify();
            return;
        };
        self.save_path(path, SaveKind::Manual, window, cx);
    }

    /// Discard the active document's edits and reload the current disk bytes.
    ///
    /// The explicit user command is the consent to lose in-memory text. The
    /// recovery record is cleared only after the disk bytes were adopted, and
    /// the session's baseline, fingerprint and clean state follow those bytes.
    pub(crate) fn resolve_conflict_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active.clone() else {
            self.status = tr!("browser.save.no_document_to_reload");
            cx.notify();
            return;
        };
        let Some(session) = self.sessions.get(&path).cloned() else {
            return;
        };
        let identity = session.read(cx).buffer.identity().clone();
        let read_path = path.clone();
        self.status = tr!("browser.save.reloading");
        cx.notify();
        let work = runtime::blocking(cx, move || {
            read_bounded(&read_path, MAX_DOCUMENT_BYTES).map_err(|error| error.to_string())
        });
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work.await.and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(source) => {
                        match session
                            .update(cx, |session, cx| session.force_reload(source, window, cx))
                        {
                            Ok(()) => {
                                let revision = session.read(cx).buffer.revision().get();
                                this.save_coordinator.observe(
                                    &path,
                                    revision,
                                    false,
                                    cx.background_executor().now(),
                                );
                                if let Some(store) = this.recovery.clone() {
                                    let task = runtime::blocking(cx, move || {
                                        let _ = store.clear(&identity);
                                    });
                                    task.detach();
                                }
                                this.status =
                                    tr!("browser.save.reloaded", file = file_label(&path));
                                this.update_tab_state(&path, cx);
                                cx.emit(SessionChanged);
                            }
                            Err(error) => {
                                this.status = tr!("browser.save.reload_failed", error = error);
                            }
                        }
                    }
                    Err(error) => {
                        this.status = tr!("browser.save.read_failed", error = error);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Write the active buffer over the file, bypassing the baseline comparison.
    ///
    /// This is the explicit "keep mine" resolution: the user chose to replace
    /// the external edit. A successful write is treated as a normal save so the
    /// baseline, fingerprint, recovery record, tab and status all follow it.
    pub(crate) fn resolve_conflict_overwrite(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.active.clone() else {
            self.status = tr!("browser.save.no_document_to_overwrite");
            cx.notify();
            return;
        };
        let Some(session) = self.sessions.get(&path).cloned() else {
            return;
        };
        let identity = session.read(cx).buffer.identity().clone();
        let (revision, content) = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            (
                session.buffer.revision(),
                session.buffer.current_text().to_owned(),
            )
        });
        let persisted = content.clone();
        let write_path = path.clone();
        self.status = tr!("browser.save.overwriting");
        cx.notify();
        let work = runtime::spawn(cx, async move {
            overwrite_document(&write_path, &content)
                .await
                .map_err(|error| error.to_string())
        });
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work.await.and_then(|result| result);
            let _ = this.update_in(cx, |this, _window, cx| {
                match result {
                    Ok(()) => {
                        let (confirmed, dirty) = session.update(cx, |session, cx| {
                            let confirmed = session.confirm_saved(revision, &persisted);
                            if !confirmed {
                                session.rebase_persisted(revision, &persisted);
                            }
                            session.dirty = session.buffer.is_dirty();
                            cx.notify();
                            (confirmed, session.buffer.is_dirty())
                        });
                        let revision = session.read(cx).buffer.revision().get();
                        this.save_coordinator.observe(
                            &path,
                            revision,
                            dirty,
                            cx.background_executor().now(),
                        );
                        if let Some(store) = this.recovery.clone() {
                            let saved = persisted.clone();
                            let task = runtime::blocking(cx, move || {
                                let _ = store.clear_after_save(&identity, &saved);
                            });
                            task.detach();
                        }
                        this.status = if confirmed {
                            tr!("browser.save.overwrote", file = file_label(&path))
                        } else {
                            tr!("browser.save.wrote_earlier")
                        };
                        this.update_tab_state(&path, cx);
                        cx.emit(SessionChanged);
                    }
                    Err(error) => {
                        this.status = tr!("browser.save.overwrite_failed", error = error);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Write the active buffer to a collision-free `.local` sibling.
    ///
    /// The original document keeps its unsaved edits and recovery record; only
    /// the copy is written, and the status names exactly where it landed.
    pub(crate) fn resolve_conflict_save_copy(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.active.clone() else {
            self.status = tr!("browser.save.no_document_to_copy");
            cx.notify();
            return;
        };
        let Some(session) = self.sessions.get(&path).cloned() else {
            return;
        };
        let content = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            session.buffer.current_text().to_owned()
        });
        let source = path.clone();
        self.status = tr!("browser.save.saving_copy");
        cx.notify();
        let work = runtime::spawn(cx, async move {
            save_document_copy(&source, &content)
                .await
                .map_err(|error| error.to_string())
        });
        self.retain_command_task(cx.spawn_in(window, async move |this, cx| {
            let result = work.await.and_then(|result| result);
            let _ = this.update_in(cx, |this, _window, cx| {
                match result {
                    Ok(copy) => {
                        let shown = this
                            .project_root()
                            .and_then(|root| copy.strip_prefix(root).ok())
                            .map(|relative| relative.display().to_string())
                            .unwrap_or_else(|| copy.display().to_string());
                        this.status = tr!(
                            "browser.save.saved_copy",
                            shown = shown,
                            file = file_label(&path)
                        );
                    }
                    Err(error) => {
                        this.status = tr!("browser.save.copy_failed", error = error);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Save one retained document through the guarded atomic path.
    ///
    /// A clean buffer is a no-op (`Ctrl/Cmd+S` on a saved document). The request
    /// goes through the coordinator, so an autosave already in flight finishes
    /// first and this save becomes its follow-up.
    pub(super) fn save_path(
        &mut self,
        path: PathBuf,
        kind: SaveKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.sessions.get(&path).cloned() else {
            return;
        };
        let (revision, dirty) = session.update(cx, |session, cx| {
            session.sync_buffer(cx);
            cx.notify();
            (session.buffer.revision().get(), session.buffer.is_dirty())
        });
        if !dirty {
            self.settle_pending_after_save(&path, true, window, cx);
            cx.notify();
            return;
        }
        let now = cx.background_executor().now();
        self.save_coordinator.observe(&path, revision, dirty, now);
        match self
            .save_coordinator
            .request_now(&path, revision, now, kind)
        {
            Some(request) => self.begin_save(request, window, cx),
            None => {
                if kind == SaveKind::Manual {
                    self.status = tr!("browser.save.saving");
                }
            }
        }
        self.arm_autosave(cx);
        cx.notify();
    }

    /// Handle a post-save action when the saved revision is already outdated.
    ///
    /// Trash and duplicate are abandoned: both would act on the persisted earlier
    /// revision while the newer edit is still only in memory. A rename still
    /// retargets the retained session and schedules recovery for the dirty buffer
    /// so the newer edits stay recoverable under the new source.
    pub(super) fn run_after_saved_revision(
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
                self.status = tr!("browser.save.trash_aborted_newer", file = name(&path));
                self.schedule_recovery(cx);
            }
            PendingAfterSave::Close { path } => {
                tracing::warn!(
                    path = %path.display(),
                    "keeping a document open: newer edits are still unsaved"
                );
                self.status = tr!("browser.save.close_aborted_newer", file = name(&path));
                self.schedule_recovery(cx);
            }
            PendingAfterSave::Duplicate { path } => {
                tracing::warn!(
                    path = %path.display(),
                    "aborting a pending duplicate: newer edits are still unsaved"
                );
                self.status = tr!("browser.save.duplicate_aborted_newer", file = name(&path));
                self.schedule_recovery(cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{TestAppContext, WindowHandle, base::Root};
    use std::sync::Mutex;

    #[test]
    fn an_oversized_save_reports_a_stable_failure_class() {
        let error = astrolith_document::DocumentError::TooLarge {
            bytes: MAX_DOCUMENT_BYTES + 1,
            limit: MAX_DOCUMENT_BYTES,
        };
        assert_eq!(save_failure_class(&error), "too-large");
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
    async fn our_own_save_does_not_report_an_external_change(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());

        edit_active(cx, handle, &view, "# Saved\n").await;
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

        // The watcher batch our own rename triggers must not flag the document.
        view.update(cx, |view, cx| {
            let token = view.begin_index_request();
            let manifest = view.manifest.as_ref().unwrap().as_ref().clone();
            let entries = view.entries.as_ref().clone();
            view.apply_watcher_scan(
                view.generation,
                token,
                Ok(WatcherScan {
                    index: Ok((manifest, entries)),
                    fingerprints: vec![(
                        path.clone(),
                        Some(crate::editor::session::fingerprint("# Saved\n")),
                    )],
                }),
                cx,
            );
        });
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(!session.externally_changed, "our own write is not external");
            assert_eq!(
                session.disk_fingerprint,
                crate::editor::session::fingerprint("# Saved\n")
            );
            let tab = view
                .tabs
                .iter()
                .find(|tab| tab.path == path)
                .expect("the saved document has a tab");
            assert!(!tab.externally_changed);
            assert!(!view.status.contains("changed on disk"));
        });

        // A genuinely external edit is still detected.
        std::fs::write(&path, "# External\n").unwrap();
        view.update(cx, |view, cx| {
            let token = view.begin_index_request();
            let manifest = view.manifest.as_ref().unwrap().as_ref().clone();
            let entries = view.entries.as_ref().clone();
            view.apply_watcher_scan(
                view.generation,
                token,
                Ok(WatcherScan {
                    index: Ok((manifest, entries)),
                    fingerprints: vec![(
                        path.clone(),
                        Some(crate::editor::session::fingerprint("# External\n")),
                    )],
                }),
                cx,
            );
        });
        view.read_with(cx, |view, cx| {
            assert!(
                view.sessions
                    .get(&path)
                    .unwrap()
                    .read(cx)
                    .externally_changed
            );
            let tab = view
                .tabs
                .iter()
                .find(|tab| tab.path == path)
                .expect("the document has a tab");
            assert!(tab.externally_changed);
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

    /// Turn the open fixture document into a live save conflict.
    async fn make_conflict(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
        path: &Path,
    ) {
        std::fs::write(path, "# External\n").unwrap();
        edit_active(cx, handle, view, "# Mine\n").await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| view.status.contains("Save conflict")) {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(view.read_with(cx, |view, _| view.status.contains("Save conflict")));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "# External\n");
    }

    fn conflict_availability(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
        view: &Entity<ProjectWorkspace>,
    ) -> CommandAvailability {
        cx.update_window(handle.into(), |_, window, cx| {
            view.read(cx).command_availability(cx, window)
        })
        .unwrap()
    }

    #[gpui_kit::test]
    async fn conflict_reload_discards_edits_and_clears_recovery(cx: &mut TestAppContext) {
        let dir = fixture();
        let recovery = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery.path());
        let identity = DocumentIdentity::new(
            ProjectId::new("workspace-1").unwrap(),
            1,
            SourcePath::new("src/posts/a.md").unwrap(),
        );
        store
            .save(&RecoveryRecord::new_with_baseline(
                identity,
                DocumentRevision::new(1),
                "# Mine\n",
                Some("# A\n".to_owned()),
                1,
            ))
            .unwrap();
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        let availability = conflict_availability(cx, handle, &view);
        assert!(
            !availability.save_conflict,
            "a clean document has no conflict to resolve"
        );
        assert!(!CommandId::ResolveConflictReload.is_available(availability));
        assert!(!CommandId::ResolveConflictOverwrite.is_available(availability));
        assert!(!CommandId::ResolveConflictSaveCopy.is_available(availability));

        make_conflict(cx, handle, &view, &path).await;
        let availability = conflict_availability(cx, handle, &view);
        assert!(availability.save_conflict);
        for command in [
            CommandId::ResolveConflictReload,
            CommandId::ResolveConflictOverwrite,
            CommandId::ResolveConflictSaveCopy,
        ] {
            assert!(command.is_available(availability), "{}", command.label());
        }

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.resolve_conflict_reload(window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, cx| {
            view.sessions.get(&path).is_some_and(|session| {
                let session = session.read(cx);
                session.buffer.current_text() == "# External\n" && !session.buffer.is_dirty()
            })
        })
        .await;
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert_eq!(session.buffer.current_text(), "# External\n");
            assert_eq!(session.buffer.original_text(), "# External\n");
            assert!(!session.buffer.is_dirty());
            assert!(!session.externally_changed);
            assert_eq!(
                session.disk_fingerprint,
                crate::editor::session::fingerprint("# External\n")
            );
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
            assert!(view.status.contains("Reloaded"), "{}", view.status);
        });
        for _ in 0..300 {
            cx.run_until_parked();
            if store.load().unwrap().records().is_empty() {
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
            "the deliberate discard clears the recovery record"
        );
    }

    #[gpui_kit::test]
    async fn conflict_overwrite_writes_buffer_bytes_and_clears_the_conflict(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        make_conflict(cx, handle, &view, &path).await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.resolve_conflict_overwrite(window, cx))
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if std::fs::read_to_string(&path).unwrap() == "# Mine\n"
                && view.read_with(cx, |view, cx| {
                    !view.sessions.get(&path).unwrap().read(cx).buffer.is_dirty()
                })
            {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Mine\n");
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(!session.buffer.is_dirty());
            assert!(!session.externally_changed);
            assert_eq!(
                session.disk_fingerprint,
                crate::editor::session::fingerprint("# Mine\n")
            );
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
            assert!(view.status.contains("Overwrote"), "{}", view.status);
        });
    }

    #[gpui_kit::test]
    async fn conflict_save_copy_writes_a_sibling_and_keeps_the_original_dirty(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let recovery = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        make_conflict(cx, handle, &view, &path).await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.resolve_conflict_save_copy(window, cx))
        })
        .unwrap();
        let copy = dir.path().join("src/posts/a.local.md");
        for _ in 0..300 {
            cx.run_until_parked();
            if copy.exists() {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "# Mine\n");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# External\n",
            "the original file is never overwritten by a copy"
        );
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(session.buffer.is_dirty(), "the original stays unsaved");
            assert!(session.externally_changed);
            assert_eq!(
                view.save_coordinator.status(&path),
                Some(SaveStatus::Conflict)
            );
            assert!(view.status.contains("a.local.md"), "{}", view.status);
        });

        // The original keeps its recovery record; the copy does not consume it.
        view.update(cx, |view, cx| view.schedule_recovery(cx));
        for _ in 0..300 {
            cx.run_until_parked();
            if store
                .load()
                .unwrap()
                .records()
                .iter()
                .any(|record| record.content() == "# Mine\n")
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
                .any(|record| record.content() == "# Mine\n"),
            "the original document keeps its recovery record"
        );
    }

    #[gpui_kit::test]
    async fn autosave_writes_through_the_guarded_path_and_is_silent_on_success(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let global = GlobalPreferences {
            autosave_delay: astrolith_core::preferences::AutosaveDelay::One,
            ..GlobalPreferences::default()
        };
        view.update(cx, |view, cx| view.set_preferences(global, None, cx));
        assert_eq!(
            view.read_with(cx, |view, _| view.save_coordinator.delay().seconds()),
            1,
            "the resolved preference drives the effective delay"
        );
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# Autosaved\n").await;
        assert_eq!(
            view.read_with(cx, |view, _| view.save_coordinator.status(&path)),
            Some(SaveStatus::Unsaved)
        );
        wait_for(cx, |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("# Autosaved\n")
        })
        .await;
        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(!session.is_dirty());
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
            assert!(
                !view.status.starts_with("Saved"),
                "an automatic save reports nothing in the status line"
            );
        });
    }

    #[gpui_kit::test]
    async fn autosave_conflict_is_visible_and_never_overwrites(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let global = GlobalPreferences {
            autosave_delay: astrolith_core::preferences::AutosaveDelay::One,
            ..GlobalPreferences::default()
        };
        view.update(cx, |view, cx| view.set_preferences(global, None, cx));
        let path = open_fixture_entry(cx, handle, &view, &dir).await;

        std::fs::write(&path, "# External\n").unwrap();
        edit_active(cx, handle, &view, "# Mine\n").await;
        wait_for(cx, |cx| {
            view.read_with(cx, |view, _| {
                view.save_coordinator.status(&path) == Some(SaveStatus::Conflict)
            })
        })
        .await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# External\n",
            "the guarded save never clobbers an external change"
        );
        view.read_with(cx, |view, cx| {
            assert!(view.status.contains("conflict"));
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(session.is_dirty());
            assert!(session.externally_changed);
            assert!(!view.save_tasks.contains_key(&path));
        });
    }

    #[gpui_kit::test]
    async fn manual_save_on_a_clean_buffer_does_nothing(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        let status = view.read_with(cx, |view, _| view.status.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        view.read_with(cx, |view, _| {
            assert_eq!(view.status, status);
            assert!(view.save_tasks.is_empty());
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# A\n");
    }

    #[gpui_kit::test]
    async fn saving_and_closing_flushes_the_buffer_then_closes_it(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# Closed with save\n").await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.close_active(window, cx));
            assert!(window.has_active_dialog(cx));
            window.click("save-and-close", cx);
        })
        .unwrap();
        wait_for(cx, |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("# Closed with save\n")
        })
        .await;
        wait_for(cx, |cx| {
            view.read_with(cx, |view, _| view.sessions.is_empty())
        })
        .await;
        assert_eq!(
            view.read_with(cx, |view, _| view.active.clone()),
            None,
            "the saved document is closed"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Closed with save\n"
        );
    }

    #[gpui_kit::test]
    async fn quitting_flushes_a_revision_typed_during_an_in_flight_save(cx: &mut TestAppContext) {
        let dir = fixture();
        let recovery_dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(recovery_dir.path());
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| view.recovery = Some(store.clone()));
        let path = open_fixture_entry(cx, handle, &view, &dir).await;

        edit_active(cx, handle, &view, "# First revision\n").await;
        // A save is in flight for the first revision when the next one is typed.
        cx.update_window(handle.into(), |_, _, cx| {
            save_request(&view, &path, SaveKind::Manual, cx);
        })
        .unwrap();
        edit_active(cx, handle, &view, "# Pending revision\n").await;
        assert!(view.read_with(cx, |view, _| view.save_coordinator.is_saving(&path)));

        let flush = view
            .update(cx, |view, cx| view.flush_recovery_on_quit(cx))
            .expect("a pending revision must produce an exit flush");
        flush.await.unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.records().len(), 1);
        assert_eq!(
            loaded.records()[0].content(),
            "# Pending revision\n",
            "the revision typed while saving is not lost at quit"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# A\n",
            "quit never writes the recovered text over the original"
        );
    }

    #[gpui_kit::test]
    async fn switching_files_saves_the_outgoing_buffer_before_the_new_one(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(1, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let other = view.read_with(cx, |view, _| view.active.clone().unwrap());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&path, window, cx))
        })
        .unwrap();
        edit_active(cx, handle, &view, "# Outgoing\n").await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&other, window, cx));
            view.read_with(cx, |view, _| {
                assert_eq!(
                    view.active.clone(),
                    Some(other.clone()),
                    "the new document is active"
                );
                assert!(
                    view.save_coordinator.is_saving(&path),
                    "the outgoing save starts before the switch completes"
                );
            });
        })
        .unwrap();
        wait_for(cx, |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("# Outgoing\n")
        })
        .await;
        view.read_with(cx, |view, cx| {
            assert!(!view.sessions.get(&path).unwrap().read(cx).is_dirty());
            assert_eq!(
                view.sessions
                    .get(&other)
                    .unwrap()
                    .read(cx)
                    .editor
                    .read(cx)
                    .value(),
                "<X>{42}</X>\n",
                "the incoming document keeps its own projection"
            );
        });
    }

    #[cfg(unix)]
    #[gpui_kit::test]
    async fn a_failed_save_keeps_the_buffer_dirty_and_writes_a_redacted_artifact(
        cx: &mut TestAppContext,
    ) {
        use std::os::unix::fs::PermissionsExt;

        let dir = fixture();
        let artifacts = tempfile::tempdir().unwrap();
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| {
            view.save_failure_dir = Some(artifacts.path().to_path_buf());
        });
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# Never written\n").await;

        let posts = dir.path().join("src/posts");
        let original = std::fs::metadata(&posts).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_mode(0o500);
        std::fs::set_permissions(&posts, read_only).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        wait_for(cx, |cx| {
            view.read_with(cx, |view, _| {
                matches!(
                    view.save_coordinator.status(&path),
                    Some(SaveStatus::Failed(_))
                )
            })
        })
        .await;
        std::fs::set_permissions(&posts, original).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# A\n",
            "a failed save leaves the original intact"
        );
        view.read_with(cx, |view, cx| {
            assert!(view.status.starts_with("Save failed"));
            assert!(view.sessions.get(&path).unwrap().read(cx).is_dirty());
        });
        let names: Vec<PathBuf> = std::fs::read_dir(artifacts.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(names.len(), 1, "one redacted artifact per failed save");
        let artifact = std::fs::read_to_string(&names[0]).unwrap();
        assert!(artifact.contains("\"error_class\": \"io\""));
        assert!(artifact.contains("\"source\": \"src/posts/a.md\""));
        assert!(
            !artifact.contains("Never written"),
            "artifacts never contain document content"
        );
    }

    #[gpui_kit::test]
    async fn an_edit_during_an_autosave_saves_the_newer_revision_without_a_conflict(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let global = GlobalPreferences {
            autosave_delay: astrolith_core::preferences::AutosaveDelay::One,
            ..GlobalPreferences::default()
        };
        view.update(cx, |view, cx| view.set_preferences(global, None, cx));
        let path = open_fixture_entry(cx, handle, &view, &dir).await;

        edit_active(cx, handle, &view, "# First\n").await;
        let revision = view.read_with(cx, |view, cx| {
            view.sessions
                .get(&path)
                .unwrap()
                .read(cx)
                .buffer
                .revision()
                .get()
        });
        // The autosave is in flight for the first revision when the next is typed.
        let request = cx
            .update_window(handle.into(), |_, _, cx| {
                save_request(&view, &path, SaveKind::Automatic, cx)
            })
            .unwrap();
        edit_active(cx, handle, &view, "# Second\n").await;
        assert!(view.read_with(cx, |view, _| view.save_coordinator.is_saving(&path)));

        // The save's own write lands before its completion is delivered.
        std::fs::write(&path, "# First\n").unwrap();
        let finish_view = view.clone();
        cx.update_window(handle.into(), move |_, window, cx| {
            finish_view.update(cx, |view, cx| {
                view.finish_save(
                    request,
                    Ok(SaveAttempt::Saved {
                        revision,
                        persisted: "# First\n".to_owned(),
                    }),
                    window,
                    cx,
                )
            })
        })
        .unwrap();

        view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(session.is_dirty(), "the newer revision stays unsaved");
            assert_eq!(
                session.buffer.original_text(),
                "# First\n",
                "the baseline follows the bytes the save persisted"
            );
            assert_ne!(
                view.save_coordinator.status(&path),
                Some(SaveStatus::Conflict),
                "our own write never conflicts with the follow-up"
            );
        });

        wait_for(cx, |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("# Second\n")
        })
        .await;
        view.read_with(cx, |view, cx| {
            assert!(!view.sessions.get(&path).unwrap().read(cx).is_dirty());
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
        });
    }

    #[gpui_kit::test]
    async fn a_blur_during_an_in_flight_save_queues_the_follow_up(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# First\n").await;
        let revision = view.read_with(cx, |view, cx| {
            view.sessions
                .get(&path)
                .unwrap()
                .read(cx)
                .buffer
                .revision()
                .get()
        });
        let request = cx
            .update_window(handle.into(), |_, _, cx| {
                save_request(&view, &path, SaveKind::Automatic, cx)
            })
            .unwrap();
        edit_active(cx, handle, &view, "# Second\n").await;

        let session = view.read_with(cx, |view, _| view.sessions.get(&path).unwrap().clone());
        cx.update_window(handle.into(), |_, _, cx| {
            session.update(cx, |_, cx| cx.emit(SaveOnBlur));
        })
        .unwrap();
        std::fs::write(&path, "# First\n").unwrap();
        let finish_view = view.clone();
        cx.update_window(handle.into(), move |_, window, cx| {
            finish_view.update(cx, |view, cx| {
                view.finish_save(
                    request,
                    Ok(SaveAttempt::Saved {
                        revision,
                        persisted: "# First\n".to_owned(),
                    }),
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        view.read_with(cx, |view, cx| {
            let now = cx.background_executor().now();
            assert_eq!(
                view.save_coordinator.next_deadline(now),
                Some(now),
                "a blur during a save becomes its follow-up instead of being dropped"
            );
        });
        wait_for(cx, |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("# Second\n")
        })
        .await;
        view.read_with(cx, |view, cx| {
            assert!(!view.sessions.get(&path).unwrap().read(cx).is_dirty());
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
        });
    }

    #[gpui_kit::test]
    async fn a_file_switch_during_an_in_flight_save_queues_the_outgoing_follow_up(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(1, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let other = view.read_with(cx, |view, _| view.active.clone().unwrap());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&path, window, cx))
        })
        .unwrap();

        edit_active(cx, handle, &view, "# First\n").await;
        let revision = view.read_with(cx, |view, cx| {
            view.sessions
                .get(&path)
                .unwrap()
                .read(cx)
                .buffer
                .revision()
                .get()
        });
        let request = cx
            .update_window(handle.into(), |_, _, cx| {
                save_request(&view, &path, SaveKind::Automatic, cx)
            })
            .unwrap();
        edit_active(cx, handle, &view, "# Second\n").await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.activate_session(&other, window, cx))
        })
        .unwrap();
        std::fs::write(&path, "# First\n").unwrap();
        let finish_view = view.clone();
        cx.update_window(handle.into(), move |_, window, cx| {
            finish_view.update(cx, |view, cx| {
                view.finish_save(
                    request,
                    Ok(SaveAttempt::Saved {
                        revision,
                        persisted: "# First\n".to_owned(),
                    }),
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        view.read_with(cx, |view, cx| {
            let now = cx.background_executor().now();
            assert_eq!(
                view.save_coordinator.next_deadline(now),
                Some(now),
                "the outgoing switch save waits as the in-flight save's follow-up"
            );
        });
        wait_for(cx, |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("# Second\n")
        })
        .await;
        view.read_with(cx, |view, cx| {
            assert!(!view.sessions.get(&path).unwrap().read(cx).is_dirty());
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
        });
    }

    #[gpui_kit::test]
    async fn a_manual_save_queued_behind_an_autosave_reports_saved(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# Queued\n").await;
        let revision = view.read_with(cx, |view, cx| {
            view.sessions
                .get(&path)
                .unwrap()
                .read(cx)
                .buffer
                .revision()
                .get()
        });
        let request = cx
            .update_window(handle.into(), |_, _, cx| {
                save_request(&view, &path, SaveKind::Automatic, cx)
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx))
        })
        .unwrap();
        assert_eq!(view.read_with(cx, |view, _| view.status.clone()), "Saving…");

        let finish_view = view.clone();
        cx.update_window(handle.into(), move |_, window, cx| {
            finish_view.update(cx, |view, cx| {
                view.finish_save(
                    request,
                    Ok(SaveAttempt::Saved {
                        revision,
                        persisted: "# Queued\n".to_owned(),
                    }),
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.status, "Saved.",
                "the queued manual request reports its own success"
            );
            assert_eq!(view.save_coordinator.status(&path), Some(SaveStatus::Saved));
        });
    }

    #[gpui_kit::test]
    async fn a_conflict_abandons_a_pending_rename_and_reports_it(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# Mine\n").await;

        let destination = SourcePath::new("src/posts/renamed.md").unwrap();
        let target = dir.path().join("src/posts/renamed.md");
        let request = cx
            .update_window(handle.into(), |_, _, cx| {
                save_request(&view, &path, SaveKind::Manual, cx)
            })
            .unwrap();
        let finish_view = view.clone();
        let from = path.clone();
        cx.update_window(handle.into(), move |_, window, cx| {
            finish_view.update(cx, |view, cx| {
                view.pending_after_save.insert(
                    from.clone(),
                    PendingAfterSave::Rename {
                        from,
                        to: destination,
                    },
                );
                view.finish_save(request, Ok(SaveAttempt::Conflict), window, cx)
            })
        })
        .unwrap();

        assert!(
            view.read_with(cx, |view, _| view.pending_after_save.is_empty()),
            "a conflict never leaves the rename queued"
        );
        assert!(!target.exists(), "a rename never runs after a conflict");
        assert!(path.exists(), "the original file is untouched");
        view.read_with(cx, |view, cx| {
            assert!(view.status.contains("Save conflict"));
            assert!(view.status.contains("rename was not applied"));
            let session = view.sessions.get(&path).unwrap().read(cx);
            assert!(session.is_dirty());
            assert_eq!(
                view.save_coordinator.status(&path),
                Some(SaveStatus::Conflict)
            );
        });
    }

    #[gpui_kit::test]
    async fn pending_after_save_actions_are_tracked_per_document(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let path_a = view.read_with(cx, |view, _| view.active.clone().unwrap());
        edit_active(cx, handle, &view, "# A edited\n").await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(1, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let path_b = view.read_with(cx, |view, _| view.active.clone().unwrap());
        assert_ne!(path_a, path_b);
        edit_active(cx, handle, &view, "# B edited\n").await;

        let revision = |view: &Entity<ProjectWorkspace>, path: &Path, cx: &mut TestAppContext| {
            let session = view.read_with(cx, |view, _| view.sessions.get(path).unwrap().clone());
            session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                session.buffer.revision().get()
            })
        };
        let rev_a = revision(&view, &path_a, cx);
        let rev_b = revision(&view, &path_b, cx);

        let backend = Arc::new(RecordingTrash {
            fail: false,
            saved: Mutex::new(None),
        });
        view.update(cx, |view, _| view.trash_backend = backend.clone());
        let destination = SourcePath::new("src/posts/renamed.md").unwrap();
        let target = dir.path().join("src/posts/renamed.md");
        let trash_path = path_b.clone();
        cx.update_window(handle.into(), |_, window, cx| {
            let request_a = save_request(&view, &path_a, SaveKind::Manual, cx);
            let request_b = save_request(&view, &path_b, SaveKind::Manual, cx);
            let rename_path = path_a.clone();
            let rename_destination = destination.clone();
            view.update(cx, |view, cx| {
                view.pending_after_save.insert(
                    rename_path.clone(),
                    PendingAfterSave::Rename {
                        from: rename_path,
                        to: rename_destination,
                    },
                );
                view.pending_after_save.insert(
                    trash_path.clone(),
                    PendingAfterSave::Trash {
                        path: trash_path.clone(),
                    },
                );

                // A's completion runs its rename and must leave B's trash queued.
                view.finish_save(
                    request_a,
                    Ok(SaveAttempt::Saved {
                        revision: rev_a,
                        persisted: "# A edited\n".to_owned(),
                    }),
                    window,
                    cx,
                );
                assert_eq!(
                    view.pending_after_save.len(),
                    1,
                    "A's save must not drop B's queued action"
                );
                assert!(view.pending_after_save.contains_key(&path_b));

                let (b_text, b_revision) = {
                    let session = view.sessions.get(&path_b).unwrap().read(cx);
                    (
                        session.buffer.current_text().to_owned(),
                        session.buffer.revision(),
                    )
                };
                assert_eq!(b_revision, DocumentRevision::new(rev_b), "B's revision");
                view.finish_save(
                    request_b,
                    Ok(SaveAttempt::Saved {
                        revision: rev_b,
                        persisted: b_text,
                    }),
                    window,
                    cx,
                );
                assert!(view.pending_after_save.is_empty());
                assert!(
                    view.status.contains("Trash"),
                    "status after B's completion: {}",
                    view.status
                );
            })
        })
        .unwrap();

        for _ in 0..300 {
            cx.run_until_parked();
            if target.exists() && !path_b.exists() && backend.saved.lock().unwrap().is_some() {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert!(target.exists(), "A's rename still runs");
        assert!(!path_a.exists());
        assert!(
            !path_b.exists(),
            "B's trash still runs; status={:?} saved={:?}",
            view.read_with(cx, |view, _| view.status.clone()),
            backend.saved.lock().unwrap().is_some()
        );
        assert!(
            backend.saved.lock().unwrap().is_some(),
            "B was moved through the trash backend"
        );
    }

    #[cfg(unix)]
    #[gpui_kit::test]
    async fn a_failure_episode_writes_one_artifact_and_rearms_the_idle_delay(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let artifacts = tempfile::tempdir().unwrap();
        let (handle, view) = mount(cx);
        view.update(cx, |view, _| {
            view.save_failure_dir = Some(artifacts.path().to_path_buf());
        });
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# Never written\n").await;

        let first = cx
            .update_window(handle.into(), |_, _, cx| {
                save_request(&view, &path, SaveKind::Automatic, cx)
            })
            .unwrap();
        let finish_view = view.clone();
        cx.update_window(handle.into(), move |_, window, cx| {
            finish_view.update(cx, |view, cx| {
                view.finish_save(first, Err(("io", "cannot write".to_owned())), window, cx)
            })
        })
        .unwrap();
        view.read_with(cx, |view, cx| {
            let now = cx.background_executor().now();
            assert!(
                view.save_coordinator
                    .next_deadline(now)
                    .is_some_and(|deadline| deadline > now),
                "a failed save never re-issues immediately"
            );
            assert_eq!(
                view.save_coordinator.status(&path),
                Some(SaveStatus::Failed("cannot write".into()))
            );
        });
        wait_for(cx, |_| {
            std::fs::read_dir(artifacts.path())
                .map(|entries| entries.count() == 1)
                .unwrap_or(false)
        })
        .await;

        // A retry of the same episode fails again: no second artifact.
        let retry = cx
            .update_window(handle.into(), |_, _, cx| {
                save_request(&view, &path, SaveKind::Automatic, cx)
            })
            .unwrap();
        let finish_view = view.clone();
        cx.update_window(handle.into(), move |_, window, cx| {
            finish_view.update(cx, |view, cx| {
                view.finish_save(retry, Err(("io", "cannot write".to_owned())), window, cx)
            })
        })
        .unwrap();
        runtime::blocking(cx, || {
            std::thread::sleep(std::time::Duration::from_millis(50))
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_dir(artifacts.path()).unwrap().count(),
            1,
            "one artifact per failure episode, not one per retry"
        );
        view.read_with(cx, |view, cx| {
            assert!(view.sessions.get(&path).unwrap().read(cx).is_dirty());
        });
    }

    #[gpui_kit::test]
    async fn a_renamed_document_keeps_autosaving_at_its_new_path(cx: &mut TestAppContext) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let global = GlobalPreferences {
            autosave_delay: astrolith_core::preferences::AutosaveDelay::One,
            ..GlobalPreferences::default()
        };
        view.update(cx, |view, cx| view.set_preferences(global, None, cx));
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(0, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let path = view.read_with(cx, |view, _| view.active.clone().unwrap());
        let renamed = dir.path().join("src/posts/renamed.md");

        // A dirty rename saves through the coordinator, moves the file and
        // retargets the retained session.
        edit_active(cx, handle, &view, "# Rename save\n").await;
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.submit_rename(path.clone(), "src/posts/renamed.md".to_owned(), window, cx)
            })
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if !path.exists() && view.read_with(cx, |view, _| view.sessions.contains_key(&renamed))
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
            view.read_with(cx, |view, _| view.sessions.contains_key(&renamed)),
            "the session is retargeted at the new source"
        );
        assert_eq!(
            std::fs::read_to_string(&renamed).unwrap(),
            "# Rename save\n"
        );

        // Autosave must still see edits after the retarget.
        edit_active(cx, handle, &view, "# Renamed edit\n").await;
        wait_for(cx, |_| {
            std::fs::read_to_string(&renamed).ok().as_deref() == Some("# Renamed edit\n")
        })
        .await;
        view.read_with(cx, |view, cx| {
            assert!(!view.sessions.get(&renamed).unwrap().read(cx).is_dirty());
            assert_eq!(
                view.save_coordinator.status(&renamed),
                Some(SaveStatus::Saved)
            );
        });
    }

    #[gpui_kit::test]
    async fn saving_and_quitting_flushes_every_dirty_buffer_before_the_window_closes(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let (handle, view) = mount(cx);
        let path = open_fixture_entry(cx, handle, &view, &dir).await;
        edit_active(cx, handle, &view, "# Quit with save\n").await;

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| assert!(!view.guard_window_close(window, cx)));
            assert!(window.has_active_dialog(cx));
            window.click("save-all-and-quit", cx);
        })
        .unwrap();

        wait_for(cx, |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("# Quit with save\n")
        })
        .await;
        wait_for(cx, |cx| cx.windows().is_empty()).await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Quit with save\n",
            "quitting through the Save arm writes the buffer"
        );
    }
}
