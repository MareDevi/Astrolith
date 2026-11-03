//! Project pane: directories, image paths, extension, imports and field mappings.

use super::*;

/// Shown when the project declares `astro-auto-import`; never changes a setting.
const AUTO_IMPORT_REMINDER: &str = "This project declares astro-auto-import. Components may already \
be available without imports; Astrolith never enables import insertion for you.";

impl PreferencesSheet {
    pub(super) fn project_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let image_label = match self.project_image_mode {
            None => "Inherit (relative)",
            Some(ImagePathMode::Relative) => "Relative",
            Some(ImagePathMode::Absolute) => "Absolute",
        };
        let extension_label = match self.project_extension {
            None => "Inherit global",
            Some(FileExtension::Md) => "Markdown (.md)",
            Some(FileExtension::Mdx) => "MDX (.mdx)",
        };
        let imports_label = match self.project_insert_imports {
            None => "Inherit",
            Some(true) => "On",
            Some(false) => "Off",
        };

        let image_weak = weak.clone();
        let image_mode = Button::new("preferences-project-image-mode")
            .small()
            .outline()
            .label(image_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, mode) in [
                    ("Inherit (relative)", None),
                    ("Relative", Some(ImagePathMode::Relative)),
                    ("Absolute", Some(ImagePathMode::Absolute)),
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
            .small()
            .outline()
            .label(extension_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [
                    ("Inherit global", None),
                    ("Markdown (.md)", Some(FileExtension::Md)),
                    ("MDX (.mdx)", Some(FileExtension::Mdx)),
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
            .small()
            .outline()
            .label(imports_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [("Inherit", None), ("On", Some(true)), ("Off", Some(false))]
                {
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
                "Content directory",
                self.control(FieldId::ProjectContent),
                FieldExtras {
                    field: Some(FieldId::ProjectContent),
                    hint: Some("Project-relative root that contains the collection directories."),
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
                "Assets directory",
                self.control(FieldId::ProjectAssets),
                FieldExtras {
                    field: Some(FieldId::ProjectAssets),
                    hint: Some("Project-relative directory for dropped and picked images."),
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
                "MDX component directory",
                self.control(FieldId::ProjectMdx),
                FieldExtras {
                    field: Some(FieldId::ProjectMdx),
                    hint: Some("Only projects have an MDX component directory."),
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
            "Image paths",
            image_mode,
            FieldExtras {
                field: None,
                hint: Some("Absolute paths start at the project root; relative paths start at the document."),
                trailing: None,
            },
            cx,
        ));
        directories.push(self.field(
            "preferences-field-project-extension",
            "New files",
            extension,
            FieldExtras {
                field: None,
                hint: Some("Overrides the General new-file type for this project."),
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
                    .rounded_md()
                    .bg(cx.theme().warning.opacity(0.12))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(AUTO_IMPORT_REMINDER),
                    )
                    .into_any_element(),
            );
        }
        imports_section.push(self.field(
            "preferences-field-project-imports",
            "Insert MDX imports",
            imports,
            FieldExtras {
                field: None,
                hint: Some("Experimental. Inherit leaves the choice to publisher defaults (off)."),
                trailing: None,
            },
            cx,
        ));

        let mappings = vec![
            self.field(
                "preferences-field-project-title",
                "Title field",
                self.control(FieldId::ProjectTitle),
                FieldExtras {
                    field: Some(FieldId::ProjectTitle),
                    hint: Some("Schema field mapped to the entry title; empty uses “title”."),
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
                "Date field",
                self.control(FieldId::ProjectDate),
                FieldExtras {
                    field: Some(FieldId::ProjectDate),
                    hint: Some("Schema field mapped to the publication date; empty uses “date”."),
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
                "Description field",
                self.control(FieldId::ProjectDescription),
                FieldExtras {
                    field: Some(FieldId::ProjectDescription),
                    hint: Some("Schema field mapped to the description; empty uses “description”."),
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
                "Draft field",
                self.control(FieldId::ProjectDraft),
                FieldExtras {
                    field: Some(FieldId::ProjectDraft),
                    hint: Some(
                        "Boolean schema field mapped to the draft flag; empty uses “draft”.",
                    ),
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
                "Directories and paths",
                Some("Empty fields inherit the Astro configuration or the global default."),
                directories,
                cx,
            ))
            .child(self.section("MDX imports", None, imports_section, cx))
            .child(self.section(
                "Frontmatter fields",
                Some("Mappings are checked against the generated collection schemas."),
                mappings,
                cx,
            ))
            .into_any_element()
    }
}
