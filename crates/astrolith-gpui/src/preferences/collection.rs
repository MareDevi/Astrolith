//! Collection pane: per-collection overrides, mappings and the URL pattern.

use super::*;

impl PreferencesSheet {
    pub(super) fn collection_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let Some(context) = self.context.clone() else {
            return div().into_any_element();
        };
        if context.collections.is_empty() {
            return div()
                .v_flex()
                .gap_2()
                .child(div().text_sm().child("No collections were discovered"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Open a project whose content configuration declares collections."),
                )
                .into_any_element();
        }
        let current = self
            .collection_index
            .and_then(|index| context.collections.get(index))
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| context.collections[0].name.clone());
        let selector_weak = weak.clone();
        let collections: Vec<String> = context
            .collections
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        let selector = Button::new("preferences-collection-select")
            .outline()
            .label(current)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (index, name) in collections.iter().enumerate() {
                    let weak = selector_weak.clone();
                    let name = name.clone();
                    menu = menu.item(PopupMenuItem::new(name).on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.select_collection(Some(index), window, cx);
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let image_label = match self.collection_image_mode {
            None => "Inherit project",
            Some(ImagePathMode::Relative) => "Relative",
            Some(ImagePathMode::Absolute) => "Absolute",
        };
        let extension_label = match self.collection_extension {
            None => "Inherit project",
            Some(FileExtension::Md) => "Markdown (.md)",
            Some(FileExtension::Mdx) => "MDX (.mdx)",
        };

        let image_weak = weak.clone();
        let image_mode = Button::new("preferences-collection-image-mode")
            .outline()
            .label(image_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, mode) in [
                    ("Inherit project", None),
                    ("Relative", Some(ImagePathMode::Relative)),
                    ("Absolute", Some(ImagePathMode::Absolute)),
                ] {
                    let weak = image_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.collection_image_mode = mode;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let extension_weak = weak.clone();
        let extension = Button::new("preferences-collection-extension")
            .outline()
            .label(extension_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [
                    ("Inherit project", None),
                    ("Markdown (.md)", Some(FileExtension::Md)),
                    ("MDX (.mdx)", Some(FileExtension::Mdx)),
                ] {
                    let weak = extension_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.collection_extension = value;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let directories = vec![
            self.field(
                "preferences-field-collection-content",
                "Content directory",
                self.control(FieldId::CollectionContent),
                FieldExtras {
                    field: Some(FieldId::CollectionContent),
                    hint: Some("Project-relative directory that replaces this collection's base."),
                    trailing: Some(self.reset_button(
                        FieldId::CollectionContent,
                        "preferences-reset-collection-content",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-collection-assets",
                "Assets directory",
                self.control(FieldId::CollectionAssets),
                FieldExtras {
                    field: Some(FieldId::CollectionAssets),
                    hint: Some("Project-relative directory for this collection's images."),
                    trailing: Some(self.reset_button(
                        FieldId::CollectionAssets,
                        "preferences-reset-collection-assets",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-collection-image-mode",
                "Image paths",
                image_mode,
                FieldExtras {
                    field: None,
                    hint: Some("Overrides the project image path mode."),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-collection-extension",
                "New files",
                extension,
                FieldExtras {
                    field: None,
                    hint: Some("Overrides the project new-file type."),
                    trailing: None,
                },
                cx,
            ),
        ];

        let mappings = vec![
            self.field(
                "preferences-field-collection-title",
                "Title field",
                self.control(FieldId::CollectionTitle),
                FieldExtras {
                    field: Some(FieldId::CollectionTitle),
                    hint: Some("Checked against this collection's schema kinds."),
                    trailing: Some(self.reset_button(
                        FieldId::CollectionTitle,
                        "preferences-reset-collection-title",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-collection-date",
                "Date field",
                self.control(FieldId::CollectionDate),
                FieldExtras {
                    field: Some(FieldId::CollectionDate),
                    hint: Some("Checked against this collection's schema kinds."),
                    trailing: Some(self.reset_button(
                        FieldId::CollectionDate,
                        "preferences-reset-collection-date",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-collection-description",
                "Description field",
                self.control(FieldId::CollectionDescription),
                FieldExtras {
                    field: Some(FieldId::CollectionDescription),
                    hint: Some("Checked against this collection's schema kinds."),
                    trailing: Some(self.reset_button(
                        FieldId::CollectionDescription,
                        "preferences-reset-collection-description",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-collection-draft",
                "Draft field",
                self.control(FieldId::CollectionDraft),
                FieldExtras {
                    field: Some(FieldId::CollectionDraft),
                    hint: Some("Must be a boolean field in this collection's schema."),
                    trailing: Some(self.reset_button(
                        FieldId::CollectionDraft,
                        "preferences-reset-collection-draft",
                        cx,
                    )),
                },
                cx,
            ),
        ];

        let url = vec![self.field(
            "preferences-field-collection-url",
            "Content Finder URL pattern",
            self.control(FieldId::CollectionUrlPattern),
            FieldExtras {
                field: Some(FieldId::CollectionUrlPattern),
                hint: Some("Empty links to the entry file. {slug} is URL-encoded."),
                trailing: Some(self.reset_button(
                    FieldId::CollectionUrlPattern,
                    "preferences-reset-collection-url",
                    cx,
                )),
            },
            cx,
        )];

        div()
            .v_flex()
            .w_full()
            .gap_6()
            .child(
                div()
                    .v_flex()
                    .w_full()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child("Collection"))
                    .child(selector),
            )
            .child(self.section(
                "Directories",
                Some("Empty fields inherit the project setting."),
                directories,
                cx,
            ))
            .child(self.section(
                "Frontmatter fields",
                Some("Collection mappings win over project mappings."),
                mappings,
                cx,
            ))
            .child(self.section(
                "Links",
                Some("Used when inserting a Content Finder entry as a Markdown link."),
                url,
                cx,
            ))
            .into_any_element()
    }
}
