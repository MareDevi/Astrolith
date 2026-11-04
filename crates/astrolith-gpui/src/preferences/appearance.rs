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
        let interface = vec![
            self.field(
                "preferences-field-ui-font-family",
                "Interface font",
                self.control(FieldId::UiFontFamily),
                FieldExtras {
                    field: Some(FieldId::UiFontFamily),
                    hint: Some("An installed font family. Empty uses the system UI font."),
                    trailing: Some(self.reset_button(
                        FieldId::UiFontFamily,
                        "preferences-reset-ui-font-family",
                        cx,
                    )),
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
        ];
        let document = vec![
            self.field(
                "preferences-field-prose-font-family",
                "Document font",
                self.control(FieldId::ProseFontFamily),
                FieldExtras {
                    field: Some(FieldId::ProseFontFamily),
                    hint: Some("An installed font family. Empty uses the mock's sans face (Inter or the platform UI font)."),
                    trailing: Some(self.reset_button(
                        FieldId::ProseFontFamily,
                        "preferences-reset-prose-font-family",
                        cx,
                    )),
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
                "Document",
                Some("Body typography, page measure and heading colours."),
                {
                    let mut fields = document;
                    fields.extend(headings);
                    fields
                },
                cx,
            ))
            .into_any_element()
    }
}
