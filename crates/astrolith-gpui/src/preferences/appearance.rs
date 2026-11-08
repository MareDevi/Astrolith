//! Appearance pane: theme, interface and document typography, heading colours.

use super::*;

impl PreferencesSheet {
    pub(super) fn appearance_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let theme_label = match self.theme {
            ThemePreference::Light => tr!("preferences.option.light"),
            ThemePreference::Dark => tr!("preferences.option.dark"),
            ThemePreference::System => tr!("preferences.option.follow_system"),
        };

        let theme_weak = weak.clone();
        let theme = Button::new("preferences-theme")
            .outline()
            .label(theme_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, preference) in [
                    (
                        tr!("preferences.option.follow_system"),
                        ThemePreference::System,
                    ),
                    (tr!("preferences.option.light"), ThemePreference::Light),
                    (tr!("preferences.option.dark"), ThemePreference::Dark),
                ] {
                    let weak = theme_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.theme = preference;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let appearance = vec![self.field(
            "preferences-field-theme",
            tr!("preferences.appearance.theme"),
            theme,
            FieldExtras {
                field: None,
                hint: Some(tr!("preferences.appearance.theme_hint").into()),
                trailing: None,
            },
            cx,
        )];
        let language_label = match self.language.as_str() {
            "zh-CN" => "简体中文",
            "ja" => "日本語",
            _ => "English",
        };
        let selected_language = self.language.clone();
        let language_weak = weak.clone();
        let language_picker = Button::new("preferences-language")
            .outline()
            .label(language_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, language) in
                    [("简体中文", "zh-CN"), ("English", "en"), ("日本語", "ja")]
                {
                    let weak = language_weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .checked(language == selected_language)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |sheet, cx| {
                                    sheet.language = language.to_owned();
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element();
        let mut interface = vec![self.field(
            "preferences-field-language",
            tr!("app.language"),
            language_picker,
            FieldExtras {
                field: None,
                hint: Some(tr!("app.choose_language").into()),
                trailing: None,
            },
            cx,
        )];
        interface.extend([
            self.field(
                "preferences-field-ui-font-family",
                tr!("preferences.appearance.interface_font"),
                self.control(FieldId::UiFontFamily),
                FieldExtras {
                    field: Some(FieldId::UiFontFamily),
                    hint: Some(tr!("preferences.appearance.interface_font_hint").into()),
                    trailing: Some(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .child(self.font_picker(FieldId::UiFontFamily, "ui-font-picker", cx))
                            .child(self.reset_button(
                                FieldId::UiFontFamily,
                                "preferences-reset-ui-font-family",
                                cx,
                            ))
                            .into_any_element(),
                    ),
                },
                cx,
            ),
            self.field(
                "preferences-field-ui-font-size",
                tr!("preferences.appearance.interface_size"),
                self.control(FieldId::UiFontSize),
                FieldExtras {
                    field: Some(FieldId::UiFontSize),
                    hint: Some(tr!("preferences.appearance.interface_size_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::UiFontSize,
                        "preferences-reset-ui-font-size",
                        cx,
                    )),
                },
                cx,
            ),
        ]);
        let panel = vec![self.field(
            "preferences-field-panel-radius",
            tr!("preferences.appearance.panel_radius"),
            self.control(FieldId::PanelRadius),
            FieldExtras {
                field: Some(FieldId::PanelRadius),
                hint: Some(tr!("preferences.appearance.panel_radius_hint").into()),
                trailing: Some(self.reset_button(
                    FieldId::PanelRadius,
                    "preferences-reset-panel-radius",
                    cx,
                )),
            },
            cx,
        )];
        let document = vec![
            self.field(
                "preferences-field-prose-font-family",
                tr!("preferences.appearance.document_font"),
                self.control(FieldId::ProseFontFamily),
                FieldExtras {
                    field: Some(FieldId::ProseFontFamily),
                    hint: Some(tr!("preferences.appearance.document_font_hint").into()),
                    trailing: Some(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .child(self.font_picker(
                                FieldId::ProseFontFamily,
                                "prose-font-picker",
                                cx,
                            ))
                            .child(self.reset_button(
                                FieldId::ProseFontFamily,
                                "preferences-reset-prose-font-family",
                                cx,
                            ))
                            .into_any_element(),
                    ),
                },
                cx,
            ),
            self.field(
                "preferences-field-font-size",
                tr!("preferences.appearance.document_font_size"),
                self.control(FieldId::FontSize),
                FieldExtras {
                    field: Some(FieldId::FontSize),
                    hint: Some(tr!("preferences.appearance.document_font_size_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::FontSize,
                        "preferences-reset-font-size",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-line-height",
                tr!("preferences.appearance.line_height"),
                self.control(FieldId::LineHeight),
                FieldExtras {
                    field: Some(FieldId::LineHeight),
                    hint: Some(tr!("preferences.appearance.line_height_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::LineHeight,
                        "preferences-reset-line-height",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-editor-width",
                tr!("preferences.appearance.page_width"),
                self.control(FieldId::PageWidth),
                FieldExtras {
                    field: Some(FieldId::PageWidth),
                    hint: Some(tr!("preferences.appearance.page_width_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::PageWidth,
                        "preferences-reset-editor-width",
                        cx,
                    )),
                },
                cx,
            ),
        ];
        let build = vec![
            self.field(
                "preferences-field-build-font-family",
                tr!("preferences.appearance.build_font"),
                self.control(FieldId::BuildFontFamily),
                FieldExtras {
                    field: Some(FieldId::BuildFontFamily),
                    hint: Some(tr!("preferences.appearance.build_font_hint").into()),
                    trailing: Some(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .child(self.font_picker(
                                FieldId::BuildFontFamily,
                                "build-font-picker",
                                cx,
                            ))
                            .child(self.reset_button(
                                FieldId::BuildFontFamily,
                                "preferences-reset-build-font-family",
                                cx,
                            ))
                            .into_any_element(),
                    ),
                },
                cx,
            ),
            self.field(
                "preferences-field-build-font-size",
                tr!("preferences.appearance.build_font_size"),
                self.control(FieldId::BuildFontSize),
                FieldExtras {
                    field: Some(FieldId::BuildFontSize),
                    hint: Some(tr!("preferences.appearance.build_font_size_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::BuildFontSize,
                        "preferences-reset-build-font-size",
                        cx,
                    )),
                },
                cx,
            ),
        ];
        let preview = vec![
            self.field(
                "preferences-field-preview-font-family",
                tr!("preferences.appearance.preview_font"),
                self.control(FieldId::PreviewFontFamily),
                FieldExtras {
                    field: Some(FieldId::PreviewFontFamily),
                    hint: Some(tr!("preferences.appearance.preview_font_hint").into()),
                    trailing: Some(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .child(self.font_picker(
                                FieldId::PreviewFontFamily,
                                "preview-font-picker",
                                cx,
                            ))
                            .child(self.reset_button(
                                FieldId::PreviewFontFamily,
                                "preferences-reset-preview-font-family",
                                cx,
                            ))
                            .into_any_element(),
                    ),
                },
                cx,
            ),
            self.field(
                "preferences-field-preview-font-size",
                tr!("preferences.appearance.preview_font_size"),
                self.control(FieldId::PreviewFontSize),
                FieldExtras {
                    field: Some(FieldId::PreviewFontSize),
                    hint: Some(tr!("preferences.appearance.preview_font_size_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::PreviewFontSize,
                        "preferences-reset-preview-font-size",
                        cx,
                    )),
                },
                cx,
            ),
        ];
        let headings = vec![
            self.field(
                "preferences-field-heading-light",
                tr!("preferences.appearance.heading_light"),
                self.control(FieldId::HeadingLight),
                FieldExtras {
                    field: Some(FieldId::HeadingLight),
                    hint: Some(tr!("preferences.appearance.heading_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::HeadingLight,
                        "preferences-reset-heading-light",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-heading-dark",
                tr!("preferences.appearance.heading_dark"),
                self.control(FieldId::HeadingDark),
                FieldExtras {
                    field: Some(FieldId::HeadingDark),
                    hint: Some(tr!("preferences.appearance.heading_hint").into()),
                    trailing: Some(self.reset_button(
                        FieldId::HeadingDark,
                        "preferences-reset-heading-dark",
                        cx,
                    )),
                },
                cx,
            ),
        ];
        div()
            .v_flex()
            .w_full()
            .gap_4()
            .child(self.section(
                tr!("preferences.appearance.theme_section"),
                Some(tr!("preferences.appearance.theme_section_hint").into()),
                appearance,
                cx,
            ))
            .child(self.section(
                tr!("preferences.appearance.interface_section"),
                Some(tr!("preferences.appearance.interface_section_hint").into()),
                interface,
                cx,
            ))
            .child(self.section(
                tr!("preferences.appearance.panel_section"),
                Some(tr!("preferences.appearance.panel_section_hint").into()),
                panel,
                cx,
            ))
            .child(self.section(
                tr!("preferences.appearance.document_section"),
                Some(tr!("preferences.appearance.document_section_hint").into()),
                {
                    let mut fields = document;
                    fields.extend(headings);
                    fields
                },
                cx,
            ))
            .child(self.section(
                tr!("preferences.appearance.build_section"),
                Some(tr!("preferences.appearance.build_section_hint").into()),
                build,
                cx,
            ))
            .child(self.section(
                tr!("preferences.appearance.preview_section"),
                Some(tr!("preferences.appearance.preview_section_hint").into()),
                preview,
                cx,
            ))
            .into_any_element()
    }

    fn font_picker(
        &self,
        field: FieldId,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let families = theme::installed_font_names(cx);
        let owner = cx.weak_entity();
        Button::new(id)
            .small()
            .outline()
            .label(tr!("preferences.appearance.browse"))
            .dropdown_menu(move |menu, _, _| {
                let default_owner = owner.clone();
                let mut menu = menu.item(
                    PopupMenuItem::new(tr!("preferences.option.system_default")).on_click(
                        move |_, window, cx| {
                            let _ = default_owner.update(cx, |sheet, cx| {
                                sheet.set_text(field, "", window, cx);
                                sheet.refresh_errors(cx);
                            });
                        },
                    ),
                );
                for family in families {
                    let owner = owner.clone();
                    let family: &'static str = family;
                    let label = family.to_owned();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |sheet, cx| {
                            sheet.set_text(field, family, window, cx);
                            sheet.refresh_errors(cx);
                        });
                    }));
                }
                menu.scrollable(true).max_h(px(450.))
            })
            .into_any_element()
    }
}
