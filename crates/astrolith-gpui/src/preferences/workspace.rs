//! Workspace pane: startup mode, per-mode layouts and workspace behaviour.

use super::*;
use gpui_kit::component::switch::Switch;

/// Localized label for a workspace mode; the core label stays English.
fn mode_label(mode: WorkspaceMode) -> String {
    match mode {
        WorkspaceMode::Writing => tr!("preferences.workspace.mode_writing"),
        WorkspaceMode::Build => tr!("preferences.workspace.mode_build"),
        WorkspaceMode::Preview => tr!("preferences.workspace.mode_preview"),
    }
}

impl PreferencesSheet {
    pub(super) fn workspace_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let extension_label = match self.global_extension {
            FileExtension::Md => tr!("preferences.option.markdown"),
            FileExtension::Mdx => tr!("preferences.option.mdx"),
        };
        let autosave_label = tr!(
            "preferences.workspace.seconds",
            seconds = self.autosave.seconds()
        );

        let mode_options = WorkspaceMode::ALL
            .into_iter()
            .map(|mode| ui::PickerOption::new(mode_label(mode)).selected(mode == self.default_mode))
            .collect();
        let default_mode = self.picker(
            "preferences-default-mode",
            mode_label(self.default_mode),
            mode_options,
            None,
            cx,
            |sheet, index, _, cx| {
                if let Some(mode) = WorkspaceMode::ALL.get(index) {
                    sheet.default_mode = *mode;
                }
                cx.notify();
            },
        );

        let extension_options = vec![
            ui::PickerOption::new(tr!("preferences.option.markdown"))
                .selected(self.global_extension == FileExtension::Md),
            ui::PickerOption::new(tr!("preferences.option.mdx"))
                .selected(self.global_extension == FileExtension::Mdx),
        ];
        let extension = self.picker(
            "preferences-default-extension",
            extension_label,
            extension_options,
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.global_extension = if index == 0 {
                    FileExtension::Md
                } else {
                    FileExtension::Mdx
                };
                cx.notify();
            },
        );

        let delays: Vec<AutosaveDelay> = [1, 2, 5, 10]
            .into_iter()
            .filter_map(AutosaveDelay::from_seconds)
            .collect();
        let autosave = self.picker(
            "preferences-autosave",
            autosave_label,
            delays
                .iter()
                .map(|delay| {
                    ui::PickerOption::new(tr!(
                        "preferences.workspace.seconds",
                        seconds = delay.seconds()
                    ))
                    .selected(delay.seconds() == self.autosave.seconds())
                })
                .collect(),
            None,
            cx,
            move |sheet, index, _, cx| {
                if let Some(delay) = delays.get(index) {
                    sheet.autosave = *delay;
                }
                cx.notify();
            },
        );

        let modes = vec![
            self.field(
                "preferences-field-default-mode",
                tr!("preferences.workspace.default_mode"),
                FieldBody::Control(Some(default_mode)),
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.workspace.default_mode_hint").into()),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-writing-wysiwyg",
                tr!("preferences.workspace.writing_wysiwyg"),
                FieldBody::Control(None),
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.workspace.writing_wysiwyg_hint").into()),
                    trailing: Some(
                        Switch::new("preferences-writing-wysiwyg")
                            .accessibility_label(tr!("preferences.workspace.writing_wysiwyg"))
                            .checked(self.writing_wysiwyg)
                            .on_change(cx.listener(|this, checked, _, cx| {
                                this.writing_wysiwyg = *checked;
                                cx.notify();
                            }))
                            .into_any_element(),
                    ),
                },
                cx,
            ),
        ];

        let mut layouts = Vec::new();
        if let Some(context) = self.context.as_ref() {
            for mode in WorkspaceMode::ALL {
                let layout = context.global.layouts.get(mode);
                let pending = self.layout_resets.global.contains(&mode);
                layouts.push(self.field(
                    match mode {
                        WorkspaceMode::Writing => "preferences-field-layout-writing",
                        WorkspaceMode::Build => "preferences-field-layout-build",
                        WorkspaceMode::Preview => "preferences-field-layout-preview",
                    },
                    match mode {
                        WorkspaceMode::Writing => tr!("preferences.workspace.layout_writing"),
                        WorkspaceMode::Build => tr!("preferences.workspace.layout_build"),
                        WorkspaceMode::Preview => tr!("preferences.workspace.layout_preview"),
                    },
                    FieldBody::Control(None),
                    FieldExtras {
                        field: None,
                        hint: Some(if pending {
                            tr!("preferences.workspace.layout_pending_hint").into()
                        } else {
                            layout_hint(layout).into()
                        }),
                        trailing: Some(self.reset_layout_button(mode, pending, cx)),
                    },
                    cx,
                ));
            }
            if let Some(project) = context.project.as_ref() {
                let overridden = WorkspaceMode::ALL
                    .iter()
                    .any(|mode| project.layout_overrides.get(*mode).is_some());
                let pending = self.layout_resets.project;
                layouts.push(
                    self.field(
                        "preferences-field-project-layout-overrides",
                        tr!("preferences.workspace.project_overrides"),
                        FieldBody::Control(None),
                        FieldExtras {
                            field: None,
                            hint: Some(if pending {
                                tr!("preferences.workspace.project_overrides_pending").into()
                            } else if overridden {
                                tr!("preferences.workspace.project_overrides_set").into()
                            } else {
                                tr!("preferences.workspace.project_overrides_none").into()
                            }),
                            trailing: Some(
                                Button::new("preferences-reset-project-layouts")
                                    .small()
                                    .ghost()
                                    .label(if pending {
                                        tr!("preferences.workspace.reset_pending")
                                    } else {
                                        tr!("preferences.workspace.reset_overrides")
                                    })
                                    .selected(pending)
                                    .disabled(!overridden && !pending)
                                    .accessibility_label(tr!(
                                        "preferences.workspace.reset_project_overrides_accessibility"
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.layout_resets.project = !this.layout_resets.project;
                                        cx.notify();
                                    }))
                                    .into_any_element(),
                            ),
                        },
                        cx,
                    ),
                );
            }
        }

        let writing = vec![
            self.field(
                "preferences-field-autosave",
                tr!("preferences.workspace.autosave"),
                FieldBody::Control(Some(autosave)),
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.workspace.autosave_hint").into()),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-default-extension",
                tr!("preferences.workspace.new_files"),
                FieldBody::Control(Some(extension)),
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.workspace.new_files_hint").into()),
                    trailing: None,
                },
                cx,
            ),
        ];
        let external = vec![
            self.field(
                "preferences-field-ide-executable",
                tr!("preferences.workspace.ide_command"),
                FieldBody::Text(FieldId::IdeExecutable),
                FieldExtras {
                    field: Some(FieldId::IdeExecutable),
                    hint: Some(tr!("preferences.workspace.ide_command_hint").into()),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-ide-arguments",
                tr!("preferences.workspace.ide_arguments"),
                FieldBody::Text(FieldId::IdeArguments),
                FieldExtras {
                    field: Some(FieldId::IdeArguments),
                    hint: Some(tr!("preferences.workspace.ide_arguments_hint").into()),
                    trailing: None,
                },
                cx,
            ),
        ];
        div()
            .v_flex()
            .w_full()
            .gap_6()
            .child(self.section(tr!("preferences.workspace.modes"), None, modes, cx))
            .child(self.section(
                tr!("preferences.workspace.layouts"),
                Some(tr!("preferences.workspace.layouts_hint").into()),
                layouts,
                cx,
            ))
            .child(self.section(
                tr!("preferences.workspace.documents"),
                Some(tr!("preferences.workspace.documents_hint").into()),
                writing,
                cx,
            ))
            .child(self.image_upload_section(cx))
            .child(self.section(
                tr!("preferences.workspace.external_editor"),
                Some(tr!("preferences.workspace.external_editor_hint").into()),
                external,
                cx,
            ))
            .into_any_element()
    }

    fn image_upload_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = tr!("preferences.upload.enable");
        let enabled = self.field(
            "preferences-field-image-upload",
            label.clone(),
            FieldBody::Control(None),
            FieldExtras {
                field: None,
                hint: None,
                trailing: Some(
                    Switch::new("preferences-image-upload")
                        .accessibility_label(label)
                        .checked(self.image_upload_enabled)
                        .on_change(cx.listener(|this, checked, _, cx| {
                            this.image_upload_enabled = *checked;
                            this.refresh_errors(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            },
            cx,
        );
        let mut fields = vec![enabled];
        if self.image_upload_enabled {
            fields.extend(
                [
                    (
                        "preferences-field-image-upload-endpoint",
                        FieldId::ImageUploadEndpoint,
                        "preferences.upload.endpoint",
                    ),
                    (
                        "preferences-field-image-upload-bucket",
                        FieldId::ImageUploadBucket,
                        "preferences.upload.bucket",
                    ),
                    (
                        "preferences-field-image-upload-region",
                        FieldId::ImageUploadRegion,
                        "preferences.upload.region",
                    ),
                    (
                        "preferences-field-image-upload-public-url",
                        FieldId::ImageUploadPublicUrl,
                        "preferences.upload.public_url",
                    ),
                    (
                        "preferences-field-image-upload-access-key",
                        FieldId::ImageUploadAccessKey,
                        "preferences.upload.access_key",
                    ),
                    (
                        "preferences-field-image-upload-secret-key",
                        FieldId::ImageUploadSecretKey,
                        "preferences.upload.secret_key",
                    ),
                ]
                .map(|(id, field, label)| {
                    self.field(
                        id,
                        tr!(label),
                        FieldBody::Text(field),
                        FieldExtras {
                            field: Some(field),
                            hint: (field == FieldId::ImageUploadSecretKey)
                                .then(|| tr!("preferences.upload.credentials_hint").into()),
                            trailing: None,
                        },
                        cx,
                    )
                }),
            );
        }
        self.section(
            tr!("preferences.upload.title"),
            Some(tr!("preferences.upload.description").into()),
            fields,
            cx,
        )
    }

    fn reset_layout_button(
        &self,
        mode: WorkspaceMode,
        pending: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match mode {
            WorkspaceMode::Writing => "preferences-reset-layout-writing",
            WorkspaceMode::Build => "preferences-reset-layout-build",
            WorkspaceMode::Preview => "preferences-reset-layout-preview",
        };
        Button::new(id)
            .small()
            .ghost()
            .label(if pending {
                tr!("preferences.workspace.reset_pending")
            } else {
                tr!("preferences.sheet.reset")
            })
            .selected(pending)
            .accessibility_label(tr!(
                "preferences.workspace.reset_layout_accessibility",
                layout = mode_label(mode)
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                if pending {
                    this.layout_resets.global.remove(&mode);
                } else {
                    this.layout_resets.global.insert(mode);
                }
                cx.notify();
            }))
            .into_any_element()
    }
}

fn layout_hint(layout: &astrolith_core::preferences::PanelLayout) -> String {
    match (layout.sidebar_visible, layout.preview_visible) {
        (true, true) => tr!("preferences.workspace.layout_both_open"),
        (true, false) => tr!("preferences.workspace.layout_library_open"),
        (false, true) => tr!("preferences.workspace.layout_tool_open"),
        (false, false) => tr!("preferences.workspace.layout_both_closed"),
    }
}
