//! Project pane: directories, image paths, extension, imports and field mappings.

use super::*;

impl PreferencesSheet {
    pub(super) fn project_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let image_label = match self.project_image_mode {
            None => tr!("preferences.option.inherit_relative"),
            Some(ImagePathMode::Relative) => tr!("preferences.option.relative"),
            Some(ImagePathMode::Absolute) => tr!("preferences.option.absolute"),
        };
        let extension_label = match self.project_extension {
            None => tr!("preferences.option.inherit_global"),
            Some(FileExtension::Md) => tr!("preferences.option.markdown"),
            Some(FileExtension::Mdx) => tr!("preferences.option.mdx"),
        };
        let imports_label = match self.project_insert_imports {
            None => tr!("preferences.option.inherit"),
            Some(true) => tr!("preferences.option.on"),
            Some(false) => tr!("preferences.option.off"),
        };

        let image_weak = weak.clone();
        let image_mode = Button::new("preferences-project-image-mode")
            .outline()
            .label(image_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, mode) in [
                    (tr!("preferences.option.inherit_relative"), None),
                    (
                        tr!("preferences.option.relative"),
                        Some(ImagePathMode::Relative),
                    ),
                    (
                        tr!("preferences.option.absolute"),
                        Some(ImagePathMode::Absolute),
                    ),
                ] {
                    let weak = image_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.project_image_mode = mode;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let extension_weak = weak.clone();
        let extension = Button::new("preferences-project-extension")
            .outline()
            .label(extension_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [
                    (tr!("preferences.option.inherit_global"), None),
                    (tr!("preferences.option.markdown"), Some(FileExtension::Md)),
                    (tr!("preferences.option.mdx"), Some(FileExtension::Mdx)),
                ] {
                    let weak = extension_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.project_extension = value;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let imports_weak = weak.clone();
        let imports = Button::new("preferences-project-imports")
            .outline()
            .label(imports_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [
                    (tr!("preferences.option.inherit"), None),
                    (tr!("preferences.option.on"), Some(true)),
                    (tr!("preferences.option.off"), Some(false)),
                ] {
                    let weak = imports_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.project_insert_imports = value;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let auto_import = self
            .context
            .as_ref()
            .is_some_and(|context| context.auto_import);
        let mut directories = vec![
            self.field(
                "preferences-field-project-content",
                tr!("preferences.project.content_directory"),
                self.control(FieldId::ProjectContent),
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
                self.control(FieldId::ProjectAssets),
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
                self.control(FieldId::ProjectMdx),
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
            image_mode,
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
            extension,
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
                    .h_flex()
                    .w_full()
                    .gap_2()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().warning.opacity(0.12))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(tr!("preferences.project.auto_import_notice")),
                    )
                    .into_any_element(),
            );
        }
        imports_section.push(self.field(
            "preferences-field-project-imports",
            tr!("preferences.project.insert_imports"),
            imports,
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
                self.control(FieldId::ProjectTitle),
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
                self.control(FieldId::ProjectDate),
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
                self.control(FieldId::ProjectDescription),
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
                self.control(FieldId::ProjectDraft),
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
