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
        let weak = cx.weak_entity();
        let extension_label = match self.global_extension {
            FileExtension::Md => tr!("preferences.option.markdown"),
            FileExtension::Mdx => tr!("preferences.option.mdx"),
        };
        let autosave_label = tr!(
            "preferences.workspace.seconds",
            seconds = self.autosave.seconds()
        );

        let mode_weak = weak.clone();
        let default_mode = Button::new("preferences-default-mode")
            .outline()
            .label(mode_label(self.default_mode))
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for mode in WorkspaceMode::ALL {
                    let weak = mode_weak.clone();
                    menu = menu.item(PopupMenuItem::new(mode_label(mode)).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |sheet, cx| {
                                sheet.default_mode = mode;
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            })
            .into_any_element();

        let extension_weak = weak.clone();
        let extension = Button::new("preferences-default-extension")
            .outline()
            .label(extension_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, extension) in [
                    (tr!("preferences.option.markdown"), FileExtension::Md),
                    (tr!("preferences.option.mdx"), FileExtension::Mdx),
                ] {
                    let weak = extension_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.global_extension = extension;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let autosave_weak = weak.clone();
        let autosave = Button::new("preferences-autosave")
            .outline()
            .label(autosave_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for seconds in [1, 2, 5, 10] {
                    let Some(delay) = AutosaveDelay::from_seconds(seconds) else {
                        continue;
                    };
                    let weak = autosave_weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(tr!("preferences.workspace.seconds", seconds = seconds))
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |sheet, cx| {
                                    sheet.autosave = delay;
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element();

        let modes = vec![
            self.field(
                "preferences-field-default-mode",
                tr!("preferences.workspace.default_mode"),
                default_mode,
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
                div().into_any_element(),
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
                    div().into_any_element(),
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
                        div().into_any_element(),
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
                                    .xsmall()
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
                autosave,
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
                extension,
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
                self.control(FieldId::IdeExecutable),
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
                self.control(FieldId::IdeArguments),
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
            .gap_4()
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
            div().into_any_element(),
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
                ]
                .map(|(id, field, label)| {
                    self.field(
                        id,
                        tr!(label),
                        self.control(field),
                        FieldExtras {
                            field: Some(field),
                            hint: None,
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
            .xsmall()
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
