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
                .child(
                    div()
                        .text_sm()
                        .child(tr!("preferences.collection.no_collections")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr!("preferences.collection.no_collections_hint")),
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
            None => tr!("preferences.option.inherit_project"),
            Some(ImagePathMode::Relative) => tr!("preferences.option.relative"),
            Some(ImagePathMode::Absolute) => tr!("preferences.option.absolute"),
        };
        let extension_label = match self.collection_extension {
            None => tr!("preferences.option.inherit_project"),
            Some(FileExtension::Md) => tr!("preferences.option.markdown"),
            Some(FileExtension::Mdx) => tr!("preferences.option.mdx"),
        };

        let image_weak = weak.clone();
        let image_mode = Button::new("preferences-collection-image-mode")
            .outline()
            .label(image_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, mode) in [
                    (tr!("preferences.option.inherit_project"), None),
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
                    (tr!("preferences.option.inherit_project"), None),
                    (tr!("preferences.option.markdown"), Some(FileExtension::Md)),
                    (tr!("preferences.option.mdx"), Some(FileExtension::Mdx)),
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
                tr!("preferences.collection.content_directory"),
                self.control(FieldId::CollectionContent),
                FieldExtras {
                    field: Some(FieldId::CollectionContent),
                    hint: Some(tr!("preferences.collection.content_directory_hint").into()),
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
                tr!("preferences.collection.assets_directory"),
                self.control(FieldId::CollectionAssets),
                FieldExtras {
                    field: Some(FieldId::CollectionAssets),
                    hint: Some(tr!("preferences.collection.assets_directory_hint").into()),
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
                tr!("preferences.collection.image_paths"),
                image_mode,
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.collection.image_paths_hint").into()),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-collection-extension",
                tr!("preferences.collection.new_files"),
                extension,
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.collection.new_files_hint").into()),
                    trailing: None,
                },
                cx,
            ),
        ];

        let mappings = vec![
            self.field(
                "preferences-field-collection-title",
                tr!("preferences.collection.title_field"),
                self.control(FieldId::CollectionTitle),
                FieldExtras {
                    field: Some(FieldId::CollectionTitle),
                    hint: Some(tr!("preferences.collection.mapping_hint").into()),
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
                tr!("preferences.collection.date_field"),
                self.control(FieldId::CollectionDate),
                FieldExtras {
                    field: Some(FieldId::CollectionDate),
                    hint: Some(tr!("preferences.collection.mapping_hint").into()),
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
                tr!("preferences.collection.description_field"),
                self.control(FieldId::CollectionDescription),
                FieldExtras {
                    field: Some(FieldId::CollectionDescription),
                    hint: Some(tr!("preferences.collection.mapping_hint").into()),
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
                tr!("preferences.collection.draft_field"),
                self.control(FieldId::CollectionDraft),
                FieldExtras {
                    field: Some(FieldId::CollectionDraft),
                    hint: Some(tr!("preferences.collection.draft_hint").into()),
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
            tr!("preferences.collection.url_pattern"),
            self.control(FieldId::CollectionUrlPattern),
            FieldExtras {
                field: Some(FieldId::CollectionUrlPattern),
                hint: Some(tr!("preferences.collection.url_pattern_hint").into()),
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
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(tr!("preferences.collection.collection")),
                    )
                    .child(selector),
            )
            .child(self.section(
                tr!("preferences.collection.directories"),
                Some(tr!("preferences.collection.directories_hint").into()),
                directories,
                cx,
            ))
            .child(self.section(
                tr!("preferences.collection.frontmatter_fields"),
                Some(tr!("preferences.collection.frontmatter_fields_hint").into()),
                mappings,
                cx,
            ))
            .child(self.section(
                tr!("preferences.collection.links"),
                Some(tr!("preferences.collection.links_hint").into()),
                url,
                cx,
            ))
            .into_any_element()
    }
}
