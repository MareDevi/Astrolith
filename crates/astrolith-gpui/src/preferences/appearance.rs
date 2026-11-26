//! Appearance pane: theme, interface and document typography, heading colours.

use super::*;

impl PreferencesSheet {
    pub(super) fn appearance_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_label = match self.theme {
            ThemePreference::Light => tr!("preferences.option.light"),
            ThemePreference::Dark => tr!("preferences.option.dark"),
            ThemePreference::System => tr!("preferences.option.follow_system"),
        };
        let theme = self.picker(
            "preferences-theme",
            theme_label,
            vec![
                ui::PickerOption::new(tr!("preferences.option.follow_system"))
                    .selected(self.theme == ThemePreference::System),
                ui::PickerOption::new(tr!("preferences.option.light"))
                    .selected(self.theme == ThemePreference::Light),
                ui::PickerOption::new(tr!("preferences.option.dark"))
                    .selected(self.theme == ThemePreference::Dark),
            ],
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.theme = match index {
                    1 => ThemePreference::Light,
                    2 => ThemePreference::Dark,
                    _ => ThemePreference::System,
                };
                cx.notify();
            },
        );
        let light_scheme = self.scheme_picker(
            "preferences-light-color-scheme",
            true,
            self.light_color_scheme,
            cx,
        );
        let dark_scheme = self.scheme_picker(
            "preferences-dark-color-scheme",
            false,
            self.dark_color_scheme,
            cx,
        );
        let appearance = vec![
            self.field(
                "preferences-field-theme",
                tr!("preferences.appearance.theme"),
                FieldBody::Control(Some(theme)),
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.appearance.theme_hint").into()),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-light-color-scheme",
                tr!("preferences.appearance.light_color_scheme"),
                FieldBody::Control(Some(light_scheme)),
                FieldExtras {
                    field: None,
                    hint: None,
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-dark-color-scheme",
                tr!("preferences.appearance.dark_color_scheme"),
                FieldBody::Control(Some(dark_scheme)),
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.appearance.color_scheme_hint").into()),
                    trailing: None,
                },
                cx,
            ),
        ];
        let language_label = match self.language.as_str() {
            "zh-CN" => "简体中文",
            "ja" => "日本語",
            _ => "English",
        };
        let language_picker = self.picker(
            "preferences-language",
            language_label,
            vec![
                ui::PickerOption::new("简体中文").selected(self.language == "zh-CN"),
                ui::PickerOption::new("English").selected(self.language == "en"),
                ui::PickerOption::new("日本語").selected(self.language == "ja"),
            ],
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.language = match index {
                    0 => "zh-CN",
                    2 => "ja",
                    _ => "en",
                }
                .to_owned();
                cx.notify();
            },
        );
        let mut interface = vec![self.field(
            "preferences-field-language",
            tr!("app.language"),
            FieldBody::Control(Some(language_picker)),
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
                FieldBody::Text(FieldId::UiFontFamily),
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
                FieldBody::Text(FieldId::UiFontSize),
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
            FieldBody::Text(FieldId::PanelRadius),
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
                FieldBody::Text(FieldId::ProseFontFamily),
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
                FieldBody::Text(FieldId::FontSize),
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
                FieldBody::Text(FieldId::LineHeight),
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
                FieldBody::Text(FieldId::PageWidth),
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
                FieldBody::Text(FieldId::BuildFontFamily),
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
                FieldBody::Text(FieldId::BuildFontSize),
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
                FieldBody::Text(FieldId::PreviewFontFamily),
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
                FieldBody::Text(FieldId::PreviewFontSize),
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
                FieldBody::Text(FieldId::HeadingLight),
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
                FieldBody::Text(FieldId::HeadingDark),
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
            .gap_6()
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

    fn scheme_picker(
        &self,
        id: &'static str,
        light: bool,
        selected: ColorScheme,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let schemes: Vec<(&'static str, ColorScheme)> = SCHEMES
            .into_iter()
            .filter(|(_, value)| value.is_dark() == !light)
            .collect();
        let label = schemes
            .iter()
            .find(|(_, value)| *value == selected)
            .map_or("", |(label, _)| *label);
        let options = schemes
            .iter()
            .map(|(label, value)| ui::PickerOption::new(*label).selected(*value == selected))
            .collect();
        self.picker(
            id,
            label,
            options,
            Some(28.0),
            cx,
            move |sheet, index, _, cx| {
                if let Some((_, value)) = schemes.get(index) {
                    if light {
                        sheet.light_color_scheme = *value;
                    } else {
                        sheet.dark_color_scheme = *value;
                    }
                }
                cx.notify();
            },
        )
    }

    fn font_picker(
        &self,
        field: FieldId,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let families = theme::installed_font_names(cx);
        let current = self
            .input_entity(field)
            .map(|input| input.read(cx).value())
            .unwrap_or_default();
        let mut options = vec![
            ui::PickerOption::new(tr!("preferences.option.system_default"))
                .selected(current.is_empty()),
        ];
        options.extend(families.iter().map(|family| {
            ui::PickerOption::new(family.clone()).selected(family.as_str() == current.as_ref())
        }));
        self.picker(
            id,
            tr!("preferences.appearance.browse"),
            options,
            Some(28.0),
            cx,
            move |sheet, index, window, cx| {
                if index == 0 {
                    sheet.set_text(field, "", window, cx);
                } else if let Some(family) = families.get(index - 1) {
                    sheet.set_text(field, family, window, cx);
                }
                sheet.refresh_errors(cx);
            },
        )
    }
}

const SCHEMES: [(&str, ColorScheme); 14] = [
    ("Astrolith Light", ColorScheme::AstrolithLight),
    ("Astrolith Dark", ColorScheme::AstrolithDark),
    ("Catppuccin Latte", ColorScheme::CatppuccinLatte),
    ("Catppuccin Frappé", ColorScheme::CatppuccinFrappe),
    ("Catppuccin Macchiato", ColorScheme::CatppuccinMacchiato),
    ("Catppuccin Mocha", ColorScheme::CatppuccinMocha),
    ("Nord", ColorScheme::Nord),
    ("Nord Light", ColorScheme::NordLight),
    ("Tokyo Night", ColorScheme::TokyoNight),
    ("Tokyo Night Storm", ColorScheme::TokyoNightStorm),
    ("Tokyo Night Moon", ColorScheme::TokyoNightMoon),
    ("Tokyo Day", ColorScheme::TokyoDay),
    ("Gruvbox Light", ColorScheme::GruvboxLight),
    ("Gruvbox Dark", ColorScheme::GruvboxDark),
];
