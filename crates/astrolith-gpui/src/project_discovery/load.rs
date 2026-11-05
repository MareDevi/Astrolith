//! Load admission, bounded reads and session installation.
use super::*;

impl ProjectWorkspace {
    /// Whether `path` is a project-owned text source this workspace may edit.
    ///
    /// Build mode edits the Astro stack in place: templates, scripts, styles,
    /// data files and configuration. Assets and files outside the project stay
    /// readonly.
    pub(crate) fn is_editable_project_source(&self, path: &std::path::Path) -> bool {
        let Some(root) = self.project_root() else {
            return false;
        };
        path.starts_with(&root)
            && !path
                .components()
                .any(|part| part.as_os_str() == "node_modules" || part.as_os_str() == ".git")
            && path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(is_editable_extension)
    }

    /// Open one project `.astro` source as a retained, editable document.
    pub(crate) fn open_project_source(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let root = self
            .project_root()
            .ok_or_else(|| "Open the project before opening a source file.".to_owned())?;
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let source = relative_source(&root, &canonical)
            .ok_or_else(|| format!("{} is outside the open project.", canonical.display()))?;
        if !self.is_editable_project_source(&canonical) {
            return Err(format!(
                "{} is not an editable project source file.",
                canonical.display()
            ));
        }
        if self.sessions.contains_key(&canonical) {
            self.activate_session(&canonical, window, cx);
            return Ok(());
        }
        if self.loads.contains_key(&canonical) {
            self.active = Some(canonical);
            self.document
                .update(cx, |document, cx| document.clear_editor(cx));
            return Ok(());
        }
        if self.loads.len() >= 2 {
            return Err("Two documents are loading; try again when one finishes.".into());
        }
        if self.sessions.len() + self.loads.len() >= 32 {
            return Err("Close an open document before opening more than 32 documents.".into());
        }
        let retained_bytes = self.retained_source_bytes(cx);
        if retained_bytes + (self.loads.len() + 1) * MAX_DOCUMENT_BYTES > 32 * 1024 * 1024 {
            return Err("Close documents before exceeding the 32 MiB loaded-source budget.".into());
        }
        let Some(project) = self.project_id.clone() else {
            return Err("The project is not ready; try again after it opens.".into());
        };
        let generation = self.generation;
        let source_string = source.as_str().to_owned();
        let identity = DocumentIdentity::new(project, generation, source);
        let ticket = DocumentLoadTicket::new(identity, DocumentRevision::INITIAL);
        let index_revision = self.index_revision;
        let expected = canonical.clone();
        let key = canonical.clone();
        let root = root.to_path_buf();
        self.document
            .update(cx, |document, cx| document.clear_editor(cx));
        let cancellation = self.cancellation.clone();
        let load = runtime::blocking(cx, move || {
            cancellation.check().map_err(|error| error.to_string())?;
            let actual = contained_path(&root, std::path::Path::new(&source_string))
                .map_err(|error| error.to_string())?;
            if actual != expected {
                return Err("Source mapping changed; reopen the project.".into());
            }
            let source =
                read_bounded(&actual, MAX_DOCUMENT_BYTES).map_err(|error| error.to_string())?;
            if source.lines().count() > 50_000 {
                return Err("Documents are limited to 50,000 lines.".into());
            }
            cancellation.check().map_err(|error| error.to_string())?;
            Ok(source)
        });
        self.loads.insert(
            key.clone(),
            cx.spawn_in(window, async move |this, cx| {
                let result = load.await.and_then(|result| result);
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_load(ticket, index_revision, key, result, window, cx);
                });
            }),
        );
        self.active = Some(canonical);
        self.status = "Loading document…".into();
        cx.notify();
        Ok(())
    }

    /// Bytes retained by every open session, counted once per representation.
    ///
    /// Load admission budgets the real footprint: the baseline and current source
    /// are distinct buffers, and the writer projection and editor rope are
    /// separate again. An in-flight load still reserves its maximum below.
    fn retained_source_bytes(&self, cx: &App) -> usize {
        self.sessions
            .values()
            .map(|session| session.read(cx).retained_bytes(cx))
            .sum()
    }

    pub(super) fn open_entry_inner(
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
        let retained_bytes = self.retained_source_bytes(cx);
        if retained_bytes + (self.loads.len() + 1) * MAX_DOCUMENT_BYTES > 32 * 1024 * 1024 {
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
                read_bounded(&actual, MAX_DOCUMENT_BYTES).map_err(|error| error.to_string())?;
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

    pub(super) fn finish_load(
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
                // Only the visible document presents complete source; other
                // retained buffers switch when they are activated.
                let source_view = self.source_view && self.active.as_ref() == Some(&key);
                session.update(cx, |session, cx| {
                    session.set_source_view(source_view, window, cx)
                });
                let copyedit = self.copyedit.clone();
                session.update(cx, |session, cx| session.set_copyedit_config(copyedit, cx));
                self.watch_session(key.clone(), session.clone(), window, cx);
                self.sessions.insert(key.clone(), session.clone());
                if !self.tab_order.contains(&key) {
                    self.tab_order.push(key.clone());
                }
                self.sync_tabs(cx);
                if self.active.as_ref() == Some(&key) {
                    self.focus_collection_for(&key, cx);
                    self.install(&key, session.clone(), cx);
                    self.reveal_pending(&key, &session, window, cx);
                    self.document.read(cx).focus_handle(cx).focus(window, cx);
                    // A Build document opened directly loads behind the mode,
                    // so its server attachment follows the load, not the click.
                    self.sync_lsp(cx);
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

    pub(super) fn install(
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
        let owner = cx.weak_entity();
        let wiring = EditorWiring::new(session.downgrade())
            .with_preview_root(self.project_root().unwrap_or_default())
            .with_drop(Rc::new(
                move |paths: &gpui_kit::ExternalPaths, window, cx| {
                    let sources = paths.paths().to_vec();
                    let _ =
                        owner.update(cx, |project, cx| project.drop_assets(sources, window, cx));
                },
            ));
        self.document.update(cx, |document, cx| {
            document.show_editor(title, editor, Some(wiring), cx)
        });
        self.file_browser.update(cx, |browser, cx| {
            browser.set_active(Some(path.to_path_buf()), cx)
        });
        self.status = if session.read(cx).externally_changed {
            "Document changed on disk. This buffer is retained; resolve it with Reload from Disk, Overwrite Disk with My Edits, or Save My Edits as a Copy."
        } else {
            ""
        }
        .into();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use gpui_kit::TestAppContext;

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

    #[gpui_kit::test]
    async fn load_admission_counts_every_retained_representation(cx: &mut TestAppContext) {
        let dir = fixture();
        // Large enough that four retained representations cross the 32 MiB
        // budget while the old original-text-only accounting would not. The
        // first line stays short so frontmatter sniffing does not read the body.
        let large_source = format!("# Large\n{}", "a".repeat(6_500_000));
        std::fs::write(dir.path().join("src/posts/large.md"), &large_source).unwrap();
        std::fs::create_dir_all(dir.path().join("src/pages")).unwrap();
        std::fs::write(
            dir.path().join("src/pages/index.astro"),
            "---\nconst title = 'x';\n---\n<h1>{title}</h1>\n",
        )
        .unwrap();

        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.start(dir.path().into(), window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| !view.loading).await;

        let large = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == "src/posts/large.md")
                .expect("the large document is indexed")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(large, window, cx))
        })
        .unwrap();
        cx.condition(&view, |view, _| view.loads.is_empty()).await;
        let large_path = view.read_with(cx, |view, cx| {
            let path = view.active.clone().expect("the large document is active");
            let session = view.sessions.get(&path).unwrap().read(cx);
            let retained = session.retained_bytes(cx);
            assert!(
                retained > 3 * large_source.len(),
                "baseline, projection and editor text all count: {retained}"
            );
            assert!(
                retained < 5 * large_source.len(),
                "no representation is counted twice: {retained}"
            );
            path
        });

        // The same budget now refuses a second, small entry.
        let small = view.read_with(cx, |view, _| {
            view.entries
                .iter()
                .position(|entry| entry.entry().source().as_str() == "src/posts/a.md")
                .expect("the small document is indexed")
        });
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_entry(small, window, cx))
        })
        .unwrap();
        view.read_with(cx, |view, _| {
            assert!(
                view.status.contains("32 MiB"),
                "the entry open must report the budget: {}",
                view.status
            );
            assert_eq!(view.sessions.len(), 1);
            assert_eq!(view.active.as_ref(), Some(&large_path));
        });

        // The project-source open path applies the same accounting.
        let source = dir.path().join("src/pages/index.astro");
        let result = cx
            .update_window(handle.into(), |_, window, cx| {
                view.update(cx, |view, cx| view.open_project_source(source, window, cx))
            })
            .unwrap();
        assert!(
            result
                .expect_err("the project source must be refused")
                .contains("32 MiB"),
            "the project-source open must report the same budget"
        );
        view.read_with(cx, |view, _| assert_eq!(view.sessions.len(), 1));
    }

    #[gpui_kit::test]
    async fn project_astro_source_opens_edits_saves_and_has_no_frontmatter(
        cx: &mut TestAppContext,
    ) {
        let dir = fixture();
        let source =
            "---\nimport Layout from '../layouts/Base.astro';\n---\n<Layout title=\"Home\" />\n";
        let path = dir.path().join("src/pages/index.astro");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, source).unwrap();
        let (handle, view) = mount(cx);
        open_project_and_wait(cx, handle, &view, dir.path()).await;
        let canonical = std::fs::canonicalize(&path).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_project_source(path.clone(), window, cx)
                    .expect("a project .astro source opens for editing");
            });
        })
        .unwrap();
        wait_for_session(cx, &view, &canonical).await;
        let session = view.read_with(cx, |view, cx| {
            let session = view.sessions.get(&canonical).unwrap().clone();
            let session_read = session.read(cx);
            assert_eq!(session_read.kind(), DocumentKind::Plain);
            assert_eq!(session_read.editor.read(cx).value(), source);
            assert_eq!(session_read.projection.visible, source);
            assert!(!session_read.is_dirty());
            assert!(
                view.active_frontmatter_key().is_none(),
                "an Astro development file has no content-entry frontmatter"
            );
            session
        });
        let edited =
            "---\nimport Layout from '../layouts/Base.astro';\n---\n<Layout title=\"Edited\" />\n";
        replace_visible(cx, handle, &session, edited);
        session.read_with(cx, |session, _| assert!(session.is_dirty()));
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.save_active(window, cx));
        })
        .unwrap();
        for _ in 0..300 {
            cx.run_until_parked();
            if std::fs::read_to_string(&canonical).unwrap() == edited {
                break;
            }
            runtime::blocking(cx, || {
                std::thread::sleep(std::time::Duration::from_millis(10))
            })
            .await
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&canonical).unwrap(), edited);
    }
}
