//! Project pane: directories, image paths, extension, imports and field mappings.

use super::*;

impl PreferencesSheet {
    pub(super) fn project_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let image_label = match self.project_image_mode {
            None => tr!("preferences.option.inherit_relative"),
            Some(ImagePathMode::Relative) => tr!("preferences.option.relative"),
            Some(ImagePathMode::Absolute) => tr!("preferences.option.absolute"),
        };
        let image_options = vec![
            ui::PickerOption::new(tr!("preferences.option.inherit_relative"))
                .selected(self.project_image_mode.is_none()),
            ui::PickerOption::new(tr!("preferences.option.relative"))
                .selected(self.project_image_mode == Some(ImagePathMode::Relative)),
            ui::PickerOption::new(tr!("preferences.option.absolute"))
                .selected(self.project_image_mode == Some(ImagePathMode::Absolute)),
        ];
        let image_mode = self.picker(
            "preferences-project-image-mode",
            image_label,
            image_options,
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.project_image_mode = match index {
                    1 => Some(ImagePathMode::Relative),
                    2 => Some(ImagePathMode::Absolute),
                    _ => None,
                };
                cx.notify();
            },
        );

        let extension_label = match self.project_extension {
            None => tr!("preferences.option.inherit_global"),
            Some(FileExtension::Md) => tr!("preferences.option.markdown"),
            Some(FileExtension::Mdx) => tr!("preferences.option.mdx"),
        };
        let extension_options = vec![
            ui::PickerOption::new(tr!("preferences.option.inherit_global"))
                .selected(self.project_extension.is_none()),
            ui::PickerOption::new(tr!("preferences.option.markdown"))
                .selected(self.project_extension == Some(FileExtension::Md)),
            ui::PickerOption::new(tr!("preferences.option.mdx"))
                .selected(self.project_extension == Some(FileExtension::Mdx)),
        ];
        let extension = self.picker(
            "preferences-project-extension",
            extension_label,
            extension_options,
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.project_extension = match index {
                    1 => Some(FileExtension::Md),
                    2 => Some(FileExtension::Mdx),
                    _ => None,
                };
                cx.notify();
            },
        );

        let imports_label = match self.project_insert_imports {
            None => tr!("preferences.option.inherit"),
            Some(true) => tr!("preferences.option.on"),
            Some(false) => tr!("preferences.option.off"),
        };
        let imports_options = vec![
            ui::PickerOption::new(tr!("preferences.option.inherit"))
                .selected(self.project_insert_imports.is_none()),
            ui::PickerOption::new(tr!("preferences.option.on"))
                .selected(self.project_insert_imports == Some(true)),
            ui::PickerOption::new(tr!("preferences.option.off"))
                .selected(self.project_insert_imports == Some(false)),
        ];
        let imports = self.picker(
            "preferences-project-imports",
            imports_label,
            imports_options,
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.project_insert_imports = match index {
                    1 => Some(true),
                    2 => Some(false),
                    _ => None,
                };
                cx.notify();
            },
        );

        let auto_import = self
            .context
            .as_ref()
            .is_some_and(|context| context.auto_import);
        let mut directories = vec![
            self.field(
                "preferences-field-project-content",
                tr!("preferences.project.content_directory"),
                FieldBody::Text(FieldId::ProjectContent),
                FieldExtras {
                    field: Some(FieldId::ProjectContent),
                    hint: Some(tr!("preferences.project.content_directory_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::ProjectContent,
                        "preferences-reset-project-content",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-project-assets",
                tr!("preferences.project.assets_directory"),
                FieldBody::Text(FieldId::ProjectAssets),
                FieldExtras {
                    field: Some(FieldId::ProjectAssets),
                    hint: Some(tr!("preferences.project.assets_directory_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::ProjectAssets,
                        "preferences-reset-project-assets",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-project-mdx",
                tr!("preferences.project.mdx_directory"),
                FieldBody::Text(FieldId::ProjectMdx),
                FieldExtras {
                    field: Some(FieldId::ProjectMdx),
                    hint: Some(tr!("preferences.project.mdx_directory_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::ProjectMdx,
                        "preferences-reset-project-mdx",
                        cx,
                    )),
                },
                cx,
            ),
        ];
        directories.push(self.field(
            "preferences-field-project-image-mode",
            tr!("preferences.project.image_paths"),
            FieldBody::Control(Some(image_mode)),
            FieldExtras {
                field: None,
                hint: Some(tr!("preferences.project.image_paths_hint").into()),
                trailing: None,
            },
            cx,
        ));
        directories.push(self.field(
            "preferences-field-project-extension",
            tr!("preferences.project.new_files"),
            FieldBody::Control(Some(extension)),
            FieldExtras {
                field: None,
                hint: Some(tr!("preferences.project.new_files_hint").into()),
                trailing: None,
            },
            cx,
        ));

        let mut imports_section = Vec::new();
        if auto_import {
            imports_section.push(
                div()
                    .id("preferences-auto-import-notice")
                    .test_support()
                    .w_full()
                    .child(ui::inline_notice(
                        "preferences-auto-import-alert",
                        NoticeTone::Info,
                        tr!("preferences.project.auto_import_notice"),
                    ))
                    .into_any_element(),
            );
        }
        imports_section.push(self.field(
            "preferences-field-project-imports",
            tr!("preferences.project.insert_imports"),
            FieldBody::Control(Some(imports)),
            FieldExtras {
                field: None,
                hint: Some(tr!("preferences.project.insert_imports_hint").into()),
                trailing: None,
            },
            cx,
        ));

        let mappings = vec![
            self.field(
                "preferences-field-project-title",
                tr!("preferences.project.title_field"),
                FieldBody::Text(FieldId::ProjectTitle),
                FieldExtras {
                    field: Some(FieldId::ProjectTitle),
                    hint: Some(tr!("preferences.project.title_field_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::ProjectTitle,
                        "preferences-reset-project-title",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-project-date",
                tr!("preferences.project.date_field"),
                FieldBody::Text(FieldId::ProjectDate),
                FieldExtras {
                    field: Some(FieldId::ProjectDate),
                    hint: Some(tr!("preferences.project.date_field_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::ProjectDate,
                        "preferences-reset-project-date",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-project-description",
                tr!("preferences.project.description_field"),
                FieldBody::Text(FieldId::ProjectDescription),
                FieldExtras {
                    field: Some(FieldId::ProjectDescription),
                    hint: Some(tr!("preferences.project.description_field_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::ProjectDescription,
                        "preferences-reset-project-description",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-project-draft",
                tr!("preferences.project.draft_field"),
                FieldBody::Text(FieldId::ProjectDraft),
                FieldExtras {
                    field: Some(FieldId::ProjectDraft),
                    hint: Some(tr!("preferences.project.draft_field_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::ProjectDraft,
                        "preferences-reset-project-draft",
                        cx,
                    )),
                },
                cx,
            ),
        ];

        div()
            .v_flex()
            .w_full()
            .gap_6()
            .child(self.section(
                tr!("preferences.project.directories"),
                Some(tr!("preferences.project.directories_hint").into()),
                directories,
                cx,
            ))
            .child(self.section(
                tr!("preferences.project.mdx_imports"),
                None,
                imports_section,
                cx,
            ))
            .child(self.section(
                tr!("preferences.project.frontmatter_fields"),
                Some(tr!("preferences.project.frontmatter_fields_hint").into()),
                mappings,
                cx,
            ))
            .into_any_element()
    }
}
