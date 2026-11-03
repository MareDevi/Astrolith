//! General pane: appearance, IDE command, new-file type, autosave and typography.

use super::*;

impl PreferencesSheet {
    pub(super) fn general_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let theme_label = match self.theme {
            ThemePreference::Light => "Light",
            ThemePreference::Dark => "Dark",
            ThemePreference::System => "System",
        };
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
                    ("Light", ThemePreference::Light),
                    ("Dark", ThemePreference::Dark),
                    ("System", ThemePreference::System),
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

        let fields = vec![
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
            self.field(
                "preferences-field-theme",
                "Theme",
                theme,
                FieldExtras {
                    field: None,
                    hint: Some("System follows the desktop appearance and is the default."),
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
                "preferences-field-font-size",
                "Editor font size",
                self.control(FieldId::FontSize),
                FieldExtras {
                    field: Some(FieldId::FontSize),
                    hint: Some("1–30 logical pixels; the default is 18."),
                    trailing: Some(self.reset_button(FieldId::FontSize,
                    "preferences-reset-font-size",
                    cx,
                )),
                },
                cx,
            ),
            self.field(
                "preferences-field-heading-light",
                "Light heading colour",
                self.control(FieldId::HeadingLight),
                FieldExtras {
                    field: Some(FieldId::HeadingLight),
                    hint: Some("Optional #rrggbb or #rrggbbaa. Empty keeps the theme heading colour."),
                    trailing: Some(self.reset_button(FieldId::HeadingLight,
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
                    hint: Some("Optional #rrggbb or #rrggbbaa. Empty keeps the theme heading colour."),
                    trailing: Some(self.reset_button(FieldId::HeadingDark,
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
                "Editor",
                Some("These settings are shared by every project."),
                fields,
                cx,
            ))
            .into_any_element()
    }
}
