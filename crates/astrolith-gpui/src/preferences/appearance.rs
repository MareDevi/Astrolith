//! Appearance pane: theme, interface and document typography, heading colours.

use super::*;

impl PreferencesSheet {
    pub(super) fn appearance_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let theme_label = match self.theme {
            ThemePreference::Light => "Light",
            ThemePreference::Dark => "Dark",
            ThemePreference::System => "Follow system",
        };

        let theme_weak = weak.clone();
        let theme = Button::new("preferences-theme")
            .outline()
            .label(theme_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, preference) in [
                    ("Follow system", ThemePreference::System),
                    ("Light", ThemePreference::Light),
                    ("Dark", ThemePreference::Dark),
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
            "Colour mode",
            theme,
            FieldExtras {
                field: None,
                hint: Some(
                    "Astrolith uses one neutral light and dark palette; Follow system chooses between them from the desktop appearance.",
                ),
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
            rust_i18n::t!("app.language").to_string(),
            language_picker,
            FieldExtras {
                field: None,
                hint: Some("Choose the interface language."),
                trailing: None,
            },
            cx,
        )];
        interface.extend([
            self.field(
                "preferences-field-ui-font-family",
                "Interface font",
                self.control(FieldId::UiFontFamily),
                FieldExtras {
                    field: Some(FieldId::UiFontFamily),
                    hint: Some("An installed font family. Empty uses the system UI font."),
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
                "Interface size",
                self.control(FieldId::UiFontSize),
                FieldExtras {
                    field: Some(FieldId::UiFontSize),
                    hint: Some("8–28 logical pixels; the default is 18."),
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
            "Panel and component corner radius",
            self.control(FieldId::PanelRadius),
            FieldExtras {
                field: Some(FieldId::PanelRadius),
                hint: Some(
                    "0–24 logical pixels; applies to all panels and components. The default is 10.",
                ),
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
                "Document font",
                self.control(FieldId::ProseFontFamily),
                FieldExtras {
                    field: Some(FieldId::ProseFontFamily),
                    hint: Some("An installed font family. Empty uses the mock's sans face (Inter or the platform UI font)."),
                    trailing: Some(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .child(self.font_picker(FieldId::ProseFontFamily, "prose-font-picker", cx))
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
                "Document font size",
                self.control(FieldId::FontSize),
                FieldExtras {
                    field: Some(FieldId::FontSize),
                    hint: Some("1–30 logical pixels; the default is 16."),
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
                "Line height",
                self.control(FieldId::LineHeight),
                FieldExtras {
                    field: Some(FieldId::LineHeight),
                    hint: Some("1.2–3.0 × the font size; the default is 2.0 (16/32)."),
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
                "Page width",
                self.control(FieldId::PageWidth),
                FieldExtras {
                    field: Some(FieldId::PageWidth),
                    hint: Some("30–100% of the editor pane; the default is 64%."),
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
                "Build font",
                self.control(FieldId::BuildFontFamily),
                FieldExtras {
                    field: Some(FieldId::BuildFontFamily),
                    hint: Some(
                        "Installed family for source code. Empty uses the system monospace font.",
                    ),
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
                "Build font size",
                self.control(FieldId::BuildFontSize),
                FieldExtras {
                    field: Some(FieldId::BuildFontSize),
                    hint: Some("1–30 logical pixels; default 14."),
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
                "Preview font",
                self.control(FieldId::PreviewFontFamily),
                FieldExtras {
                    field: Some(FieldId::PreviewFontFamily),
                    hint: Some("Installed family for rendered documents. Empty uses the preferred sans font."),
                    trailing: Some(
                        div().h_flex().items_center().gap_1()
                            .child(self.font_picker(FieldId::PreviewFontFamily, "preview-font-picker", cx))
                            .child(self.reset_button(FieldId::PreviewFontFamily, "preferences-reset-preview-font-family", cx))
                            .into_any_element(),
                    ),
                },
                cx,
            ),
            self.field(
                "preferences-field-preview-font-size",
                "Preview font size",
                self.control(FieldId::PreviewFontSize),
                FieldExtras {
                    field: Some(FieldId::PreviewFontSize),
                    hint: Some("1–30 logical pixels; default 16."),
                    trailing: Some(self.reset_button(FieldId::PreviewFontSize, "preferences-reset-preview-font-size", cx)),
                },
                cx,
            ),
        ];
        let headings = vec![
            self.field(
                "preferences-field-heading-light",
                "Light heading colour",
                self.control(FieldId::HeadingLight),
                FieldExtras {
                    field: Some(FieldId::HeadingLight),
                    hint: Some(
                        "Optional #rrggbb or #rrggbbaa. Empty keeps the palette heading colour.",
                    ),
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
                "Dark heading colour",
                self.control(FieldId::HeadingDark),
                FieldExtras {
                    field: Some(FieldId::HeadingDark),
                    hint: Some(
                        "Optional #rrggbb or #rrggbbaa. Empty keeps the palette heading colour.",
                    ),
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
                "Theme",
                Some("Colour schemes shared by every project."),
                appearance,
                cx,
            ))
            .child(self.section(
                "Interface",
                Some("Typography for panels, controls and menus."),
                interface,
                cx,
            ))
            .child(self.section(
                "Panel shape",
                Some("One corner radius for panels and components across every project."),
                panel,
                cx,
            ))
            .child(self.section(
                "Document",
                Some("Body typography, page measure and heading colours."),
                {
                    let mut fields = document;
                    fields.extend(headings);
                    fields
                },
                cx,
            ))
            .child(self.section("Build", Some("Source editor typography."), build, cx))
            .child(self.section(
                "Preview",
                Some("Rendered document typography."),
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
            .label("Browse")
            .dropdown_menu(move |menu, _, _| {
                let default_owner = owner.clone();
                let mut menu = menu.item(PopupMenuItem::new("System default").on_click(
                    move |_, window, cx| {
                        let _ = default_owner.update(cx, |sheet, cx| {
                            sheet.set_text(field, "", window, cx);
                            sheet.refresh_errors(cx);
                        });
                    },
                ));
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
