//! Document operations: create, rename, duplicate, trash, reveal and copy.
use super::*;

/// Everything a create dialog needs after its required-field inputs are filled.
pub(super) struct PendingCreate {
    pub(super) collection: CollectionId,
    pub(super) directory: Option<SourcePath>,
    pub(super) extension: FileExtension,
    pub(super) fields: Vec<CreationField>,
    pub(super) today: String,
}

impl ProjectWorkspace {
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

    pub(super) fn report_entry_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.status = message;
        cx.notify();
    }

    pub(super) fn operation_scope(&self) -> Option<ProjectScope> {
        self.manifest
            .as_ref()
            .map(|manifest| ProjectScope::new(manifest.root().to_path_buf(), self.generation))
    }

    pub(super) fn source_path(&self, path: &Path) -> Option<SourcePath> {
        let root = self.manifest.as_ref()?.root();
        relative_source(root, path)
    }

    pub(super) fn collection_of(&self, path: &Path) -> Option<String> {
        self.session_collections.get(path).cloned().or_else(|| {
            self.entries
                .iter()
                .find(|entry| entry.absolute_source() == path)
                .map(|entry| entry.entry().id().collection().as_str().to_owned())
        })
    }

    pub(super) fn needs_save(&self, path: &Path, cx: &App) -> bool {
        self.sessions
            .get(path)
            .is_some_and(|session| session.read(cx).is_dirty())
    }

    pub(super) fn start_new_entry(
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
        let explicit_entry = target.and_then(|path| {
            self.entries
                .iter()
                .find(|entry| entry.absolute_source() == path)
        });
        let active_entry = self.active.as_ref().and_then(|active| {
            self.entries
                .iter()
                .find(|entry| entry.absolute_source() == active)
        });
        let descriptor_of = |name: &str| {
            manifest
                .collections()
                .iter()
                .find(|descriptor| descriptor.id().as_str() == name)
        };
        let selected_descriptor = self.selected_collection.as_deref().and_then(descriptor_of);
        let Some((collection, descriptor)) = explicit_entry
            .and_then(|entry| {
                let name = entry.entry().id().collection().as_str();
                descriptor_of(name).map(|descriptor| (descriptor.id().clone(), descriptor))
            })
            .or_else(|| selected_descriptor.map(|descriptor| (descriptor.id().clone(), descriptor)))
            .or_else(|| {
                active_entry.and_then(|entry| {
                    let name = entry.entry().id().collection().as_str();
                    descriptor_of(name).map(|descriptor| (descriptor.id().clone(), descriptor))
                })
            })
            .or_else(|| {
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
        // The browsed directory wins over the active document so "New entry"
        // follows the folder the library is showing.
        let browsed_directory = (self.selected_collection.as_deref() == Some(collection.as_str()))
            .then(|| {
                let mut relative = PathBuf::new();
                for segment in &self.selected_directory {
                    relative.push(segment);
                }
                descriptor.base().join(relative)
            });
        let directory_absolute = explicit_entry
            .and_then(|entry| entry.absolute_source().parent())
            .filter(|parent| parent.starts_with(descriptor.base()))
            .or(browsed_directory.as_deref())
            .or_else(|| {
                active_entry
                    .and_then(|entry| entry.absolute_source().parent())
                    .filter(|parent| parent.starts_with(descriptor.base()))
            })
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
    pub(super) fn creation_hint(&self, input: &CreationInput) -> Option<SharedString> {
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
    pub(super) fn reference_hint(&self, collections: &[String]) -> SharedString {
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

    pub(super) fn open_create_dialog(
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
    pub(super) fn submit_create(
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

    pub(super) fn open_rename_dialog(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn submit_rename(
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
            self.pending_after_save.insert(
                path.clone(),
                PendingAfterSave::Rename {
                    from: path.clone(),
                    to: destination,
                },
            );
            self.save_path(path, SaveKind::Manual, window, cx);
        } else {
            self.perform_rename(path, destination, window, cx);
        }
    }

    pub(super) fn perform_rename(
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
    pub(super) fn retarget_session(
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
        self.save_coordinator.retarget(from, to);
        // The old subscriptions capture the previous path, so they would feed
        // changes, blur, status and recovery to a session that no longer exists
        // under that key. Drop them and subscribe the same session to its new
        // source; `observe` is idempotent for the retargeted revision.
        self.session_subscriptions.remove(from);
        if let Some(collection) = self.session_collections.remove(from) {
            self.session_collections
                .insert(to.to_path_buf(), collection);
        }
        self.sessions.insert(to.to_path_buf(), session.clone());
        self.watch_session(to.to_path_buf(), session.clone(), window, cx);
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

    pub(super) fn duplicate_entry(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.needs_save(&path, cx) {
            self.pending_after_save.insert(
                path.clone(),
                PendingAfterSave::Duplicate { path: path.clone() },
            );
            self.save_path(path, SaveKind::Manual, window, cx);
        } else {
            self.perform_duplicate(path, window, cx);
        }
    }

    pub(super) fn perform_duplicate(
        &mut self,
        path: PathBuf,
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

    pub(super) fn request_trash(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn save_and_trash(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.needs_save(&path, cx) {
            self.pending_after_save
                .insert(path.clone(), PendingAfterSave::Trash { path: path.clone() });
            self.save_path(path, SaveKind::Manual, window, cx);
        } else {
            self.perform_trash(path, window, cx);
        }
    }

    pub(super) fn discard_and_trash(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remove_session(&path, cx);
        self.perform_trash(path, window, cx);
    }

    pub(super) fn perform_trash(
        &mut self,
        path: PathBuf,
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

    pub(super) fn reveal_entry(
        &mut self,
        path: PathBuf,
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

    pub(super) fn copy_entry_path(
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

    pub(super) fn run_after_save(
        &mut self,
        action: PendingAfterSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PendingAfterSave::Rename { from, to } => self.perform_rename(from, to, window, cx),
            PendingAfterSave::Duplicate { path } => self.perform_duplicate(path, window, cx),
            PendingAfterSave::Trash { path } => self.perform_trash(path, window, cx),
            PendingAfterSave::Close { path } => self.remove_session(&path, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use astrolith_core::preferences::CollectionPreferencesV1;
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;
    use std::sync::Mutex;

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
            let request = save_request(&view, &path, SaveKind::Manual, cx);
            view.update(cx, |view, cx| {
                view.pending_after_save
                    .insert(path.clone(), PendingAfterSave::Trash { path: path.clone() });
                // The completion seam: the snapshot's revision is already outdated.
                view.finish_save(
                    request,
                    Ok(SaveAttempt::Saved {
                        revision: saved_revision,
                        persisted: "# Saved revision\n".to_owned(),
                    }),
                    window,
                    cx,
                );
            })
        })
        .unwrap();
        assert!(
            view.read_with(cx, |view, _| view.pending_after_save.is_empty()),
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
            let request = save_request(&view, &path, SaveKind::Manual, cx);
            view.update(cx, |view, cx| {
                view.pending_after_save.insert(
                    path.clone(),
                    PendingAfterSave::Rename {
                        from: path.clone(),
                        to: destination.clone(),
                    },
                );
                view.finish_save(
                    request,
                    Ok(SaveAttempt::Saved {
                        revision: saved_revision,
                        persisted: "# Saved revision\n".to_owned(),
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
