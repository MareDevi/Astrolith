//! General pane: appearance, IDE command, new-file type, autosave and typography.

use super::*;

impl PreferencesSheet {
    pub(super) fn general_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let theme_label = match self.theme {
            ThemePreference::Light => "Light",
            ThemePreference::Dark => "Dark",
            ThemePreference::System => "Follow system",
        };
        let light_theme_weak = weak.clone();
        let current_light = self.light_theme;
        let light_theme = Button::new("preferences-light-theme")
            .small()
            .outline()
            .label(self.light_theme.label())
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for scheme in LightColorScheme::ALL {
                    let weak = light_theme_weak.clone();
                    let selected = scheme == current_light;
                    menu = menu.item(
                        PopupMenuItem::new(scheme.label())
                            .checked(selected)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |sheet, cx| {
                                    sheet.light_theme = scheme;
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element();

        let dark_theme_weak = weak.clone();
        let current_dark = self.dark_theme;
        let dark_theme = Button::new("preferences-dark-theme")
            .small()
            .outline()
            .label(self.dark_theme.label())
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for scheme in DarkColorScheme::ALL {
                    let weak = dark_theme_weak.clone();
                    let selected = scheme == current_dark;
                    menu = menu.item(
                        PopupMenuItem::new(scheme.label())
                            .checked(selected)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |sheet, cx| {
                                    sheet.dark_theme = scheme;
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element();

        let extension_label = match self.global_extension {
            FileExtension::Md => "Markdown (.md)",
            FileExtension::Mdx => "MDX (.mdx)",
        };
        let autosave_label = format!("{} seconds", self.autosave.seconds());

        let theme_weak = weak.clone();
        let theme = Button::new("preferences-theme")
            .small()
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

        let extension_weak = weak.clone();
        let extension = Button::new("preferences-default-extension")
            .small()
            .outline()
            .label(extension_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, extension) in [
                    ("Markdown (.md)", FileExtension::Md),
                    ("MDX (.mdx)", FileExtension::Mdx),
                ] {
                    let weak = extension_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.global_extension = extension;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let autosave_weak = weak.clone();
        let autosave = Button::new("preferences-autosave")
            .small()
            .outline()
            .label(autosave_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for seconds in [1, 2, 5, 10] {
                    let Some(delay) = AutosaveDelay::from_seconds(seconds) else {
                        continue;
                    };
                    let weak = autosave_weak.clone();
                    menu = menu.item(PopupMenuItem::new(format!("{seconds} seconds")).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |sheet, cx| {
                                sheet.autosave = delay;
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            })
            .into_any_element();

        let appearance = vec![
            self.field(
                "preferences-field-theme",
                "Appearance",
                theme,
                FieldExtras {
                    field: None,
                    hint: Some(
                        "Follow system keeps the light and dark palettes in sync with the desktop.",
                    ),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-light-theme",
                "Light palette",
                light_theme,
                FieldExtras {
                    field: None,
                    hint: Some("Used when the appearance resolves to light."),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-dark-theme",
                "Dark palette",
                dark_theme,
                FieldExtras {
                    field: None,
                    hint: Some("Used when the appearance resolves to dark."),
                    trailing: None,
                },
                cx,
            ),
        ];
        let interface = vec![
            self.field(
                "preferences-field-ui-font-family",
                "Interface font",
                self.control(FieldId::UiFontFamily),
                FieldExtras {
                    field: Some(FieldId::UiFontFamily),
                    hint: Some(
                        "An installed font family. Empty uses the system UI font; the page keeps its book face.",
                    ),
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
                    hint: Some("8–28 logical pixels; the default is 14."),
                    trailing: Some(self.reset_button(
                        FieldId::UiFontSize,
                        "preferences-reset-ui-font-size",
                        cx,
                    )),
                },
                cx,
            ),
        ];
        let writing = vec![
            self.field(
                "preferences-field-font-size",
                "Editor font size",
                self.control(FieldId::FontSize),
                FieldExtras {
                    field: Some(FieldId::FontSize),
                    hint: Some("1–30 logical pixels; the default is 18."),
                    trailing: Some(self.reset_button(
                        FieldId::FontSize,
                        "preferences-reset-font-size",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-editor-width",
                "Editor width",
                self.control(FieldId::EditorWidth),
                FieldExtras {
                    field: Some(FieldId::EditorWidth),
                    hint: Some("480–1200 logical pixels; the default is 720."),
                    trailing: Some(self.reset_button(
                        FieldId::EditorWidth,
                        "preferences-reset-editor-width",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-autosave",
                "Idle autosave",
                autosave,
                FieldExtras {
                    field: None,
                    hint: Some("Dirty documents save after this much idle time."),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-default-extension",
                "New files",
                extension,
                FieldExtras {
                    field: None,
                    hint: Some("Extension used for new entries unless a project or collection overrides it."),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-heading-light",
                "Light heading colour",
                self.control(FieldId::HeadingLight),
                FieldExtras {
                    field: Some(FieldId::HeadingLight),
                    hint: Some("Optional #rrggbb or #rrggbbaa. Empty keeps the palette heading colour."),
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
                    hint: Some("Optional #rrggbb or #rrggbbaa. Empty keeps the palette heading colour."),
                    trailing: Some(self.reset_button(
                        FieldId::HeadingDark,
                        "preferences-reset-heading-dark",
                        cx,
                    )),
                },
                cx,
            ),
        ];
        let external = vec![
            self.field(
                "preferences-field-ide-executable",
                "IDE command",
                self.control(FieldId::IdeExecutable),
                FieldExtras {
                    field: Some(FieldId::IdeExecutable),
                    hint: Some("Launched directly as argv; no shell interprets it."),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-ide-arguments",
                "IDE arguments",
                self.control(FieldId::IdeArguments),
                FieldExtras {
                    field: Some(FieldId::IdeArguments),
                    hint: Some("One argument per line. {project}, {file}, {line} and {column} are substituted."),
                    trailing: None,
                },
                cx,
            ),
        ];
        div()
            .v_flex()
            .w_full()
            .gap_4()
            .child(self.section(
                "Appearance",
                Some("Colour schemes shared by every project."),
                appearance,
                cx,
            ))
            .child(self.section(
                "Interface",
                Some(
                    "Typography for panels, controls and menus. The page keeps its own book face.",
                ),
                interface,
                cx,
            ))
            .child(self.section(
                "Writing",
                Some("Editor typography and document defaults."),
                writing,
                cx,
            ))
            .child(self.section(
                "External editor",
                Some("How Astrolith opens files in another program."),
                external,
                cx,
            ))
            .into_any_element()
    }
}
