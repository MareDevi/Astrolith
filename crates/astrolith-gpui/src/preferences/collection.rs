//! Collection pane: per-collection overrides, mappings and the URL pattern.

use super::*;

impl PreferencesSheet {
    pub(super) fn collection_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(context) = self.context.clone() else {
            return div().into_any_element();
        };
        if context.collections.is_empty() {
            return ui::empty_state(
                "preferences-collection-empty",
                gpui_kit::assets::IconName::FolderOpen,
                tr!("preferences.collection.no_collections"),
                tr!("preferences.collection.no_collections_hint"),
                None,
                cx,
            )
            .into_any_element();
        }
        let current = self
            .collection_index
            .and_then(|index| context.collections.get(index))
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| context.collections[0].name.clone());
        let selector_options = context
            .collections
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                ui::PickerOption::new(entry.name.clone())
                    .selected(self.collection_index == Some(index))
            })
            .collect();
        let selector = self.picker(
            "preferences-collection-select",
            current,
            selector_options,
            None,
            cx,
            |sheet, index, window, cx| sheet.select_collection(Some(index), window, cx),
        );

        let image_label = match self.collection_image_mode {
            None => tr!("preferences.option.inherit_project"),
            Some(ImagePathMode::Relative) => tr!("preferences.option.relative"),
            Some(ImagePathMode::Absolute) => tr!("preferences.option.absolute"),
        };
        let image_options = vec![
            ui::PickerOption::new(tr!("preferences.option.inherit_project"))
                .selected(self.collection_image_mode.is_none()),
            ui::PickerOption::new(tr!("preferences.option.relative"))
                .selected(self.collection_image_mode == Some(ImagePathMode::Relative)),
            ui::PickerOption::new(tr!("preferences.option.absolute"))
                .selected(self.collection_image_mode == Some(ImagePathMode::Absolute)),
        ];
        let image_mode = self.picker(
            "preferences-collection-image-mode",
            image_label,
            image_options,
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.collection_image_mode = match index {
                    1 => Some(ImagePathMode::Relative),
                    2 => Some(ImagePathMode::Absolute),
                    _ => None,
                };
                cx.notify();
            },
        );

        let extension_label = match self.collection_extension {
            None => tr!("preferences.option.inherit_project"),
            Some(FileExtension::Md) => tr!("preferences.option.markdown"),
            Some(FileExtension::Mdx) => tr!("preferences.option.mdx"),
        };
        let extension_options = vec![
            ui::PickerOption::new(tr!("preferences.option.inherit_project"))
                .selected(self.collection_extension.is_none()),
            ui::PickerOption::new(tr!("preferences.option.markdown"))
                .selected(self.collection_extension == Some(FileExtension::Md)),
            ui::PickerOption::new(tr!("preferences.option.mdx"))
                .selected(self.collection_extension == Some(FileExtension::Mdx)),
        ];
        let extension = self.picker(
            "preferences-collection-extension",
            extension_label,
            extension_options,
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.collection_extension = match index {
                    1 => Some(FileExtension::Md),
                    2 => Some(FileExtension::Mdx),
                    _ => None,
                };
                cx.notify();
            },
        );

        let directories = vec![
            self.field(
                "preferences-field-collection-content",
                tr!("preferences.collection.content_directory"),
                FieldBody::Text(FieldId::CollectionContent),
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
                FieldBody::Text(FieldId::CollectionAssets),
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
                FieldBody::Control(Some(image_mode)),
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
                FieldBody::Control(Some(extension)),
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
                FieldBody::Text(FieldId::CollectionTitle),
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
                FieldBody::Text(FieldId::CollectionDate),
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
                FieldBody::Text(FieldId::CollectionDescription),
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
                FieldBody::Text(FieldId::CollectionDraft),
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
            FieldBody::Text(FieldId::CollectionUrlPattern),
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
            .child(self.field(
                "preferences-field-collection-select",
                tr!("preferences.collection.collection"),
                FieldBody::Control(Some(selector)),
                FieldExtras {
                    field: None,
                    hint: None,
                    trailing: None,
                },
                cx,
            ))
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
