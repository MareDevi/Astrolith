//! Workspace pane: startup mode, per-mode layouts and workspace behaviour.

use super::*;

impl PreferencesSheet {
    pub(super) fn workspace_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let extension_label = match self.global_extension {
            FileExtension::Md => "Markdown (.md)",
            FileExtension::Mdx => "MDX (.mdx)",
        };
        let autosave_label = format!("{} seconds", self.autosave.seconds());

        let mode_weak = weak.clone();
        let mode_label = self.default_mode.label();
        let default_mode = Button::new("preferences-default-mode")
            .small()
            .outline()
            .label(mode_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for mode in WorkspaceMode::ALL {
                    let weak = mode_weak.clone();
                    menu = menu.item(PopupMenuItem::new(mode.label()).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.default_mode = mode;
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

        let focus_weak = weak.clone();
        let focus_label = if self.focus_mode { "On" } else { "Off" };
        let focus_mode = Button::new("preferences-focus-mode")
            .small()
            .outline()
            .label(focus_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [("Off", false), ("On", true)] {
                    let weak = focus_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.focus_mode = value;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let typewriter_weak = weak.clone();
        let typewriter_label = if self.typewriter_mode { "On" } else { "Off" };
        let typewriter_mode = Button::new("preferences-typewriter-mode")
            .small()
            .outline()
            .label(typewriter_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [("Off", false), ("On", true)] {
                    let weak = typewriter_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.typewriter_mode = value;
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

        let startup = vec![self.field(
            "preferences-field-default-mode",
            "Default mode",
            default_mode,
            FieldExtras {
                field: None,
                hint: Some(
                    "Used at launch when the open project has not recorded its own last mode.",
                ),
                trailing: None,
            },
            cx,
        )];

        let mut layouts = Vec::new();
        if let Some(context) = self.context.as_ref() {
            for mode in WorkspaceMode::ALL {
                let layout = context.global_layouts.get(mode);
                let pending = self.layout_resets.global.contains(&mode);
                layouts.push(self.field(
                    match mode {
                        WorkspaceMode::Writing => "preferences-field-layout-writing",
                        WorkspaceMode::Build => "preferences-field-layout-build",
                        WorkspaceMode::Preview => "preferences-field-layout-preview",
                    },
                    match mode {
                        WorkspaceMode::Writing => "Writing layout",
                        WorkspaceMode::Build => "Build layout",
                        WorkspaceMode::Preview => "Preview layout",
                    },
                    div().into_any_element(),
                    FieldExtras {
                        field: None,
                        hint: Some(if pending {
                            "Reset to the design default on Apply; Cancel keeps the current layout."
                        } else {
                            layout_hint(layout)
                        }),
                        trailing: Some(self.reset_layout_button(mode, pending, cx)),
                    },
                    cx,
                ));
            }
            if let Some(project) = context.project.as_ref() {
                let overridden = WorkspaceMode::ALL
                    .iter()
                    .any(|mode| project.layout_overrides.get(*mode).is_some());
                let pending = self.layout_resets.project;
                layouts.push(
                    self.field(
                        "preferences-field-project-layout-overrides",
                        "Project layout overrides",
                        div().into_any_element(),
                        FieldExtras {
                            field: None,
                            hint: Some(if pending {
                                "The project falls back to the global layouts on Apply."
                            } else if overridden {
                                "This project records its own dock sizes and visibility."
                            } else {
                                "This project inherits every global layout."
                            }),
                            trailing: Some(
                                Button::new("preferences-reset-project-layouts")
                                    .xsmall()
                                    .ghost()
                                    .label(if pending {
                                        "Reset pending"
                                    } else {
                                        "Reset overrides"
                                    })
                                    .selected(pending)
                                    .disabled(!overridden && !pending)
                                    .accessibility_label("Reset the project's layout overrides")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.layout_resets.project = !this.layout_resets.project;
                                        cx.notify();
                                    }))
                                    .into_any_element(),
                            ),
                        },
                        cx,
                    ),
                );
            }
        }

        let writing = vec![
            self.field(
                "preferences-field-focus-mode",
                "Focus mode",
                focus_mode,
                FieldExtras {
                    field: None,
                    hint: Some("Dim every sentence except the one holding the cursor."),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-typewriter-mode",
                "Typewriter mode",
                typewriter_mode,
                FieldExtras {
                    field: None,
                    hint: Some("Keep the cursor line vertically centred."),
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
                "Startup",
                Some("Which workspace mode opens when there is no recorded project mode."),
                startup,
                cx,
            ))
            .child(self.section(
                "Layouts",
                Some("Drag a dock to size it; Reset restores the design default for that mode."),
                layouts,
                cx,
            ))
            .child(self.section(
                "Writing",
                Some("Writing presentation, saving and new-entry defaults."),
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

    fn reset_layout_button(
        &self,
        mode: WorkspaceMode,
        pending: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match mode {
            WorkspaceMode::Writing => "preferences-reset-layout-writing",
            WorkspaceMode::Build => "preferences-reset-layout-build",
            WorkspaceMode::Preview => "preferences-reset-layout-preview",
        };
        Button::new(id)
            .xsmall()
            .ghost()
            .label(if pending { "Reset pending" } else { "Reset" })
            .selected(pending)
            .accessibility_label(format!("Reset the {} layout", mode.label()))
            .on_click(cx.listener(move |this, _, _, cx| {
                if pending {
                    this.layout_resets.global.remove(&mode);
                } else {
                    this.layout_resets.global.insert(mode);
                }
                cx.notify();
            }))
            .into_any_element()
    }
}

fn layout_hint(layout: &astrolith_core::preferences::PanelLayout) -> &'static str {
    match (layout.sidebar_visible, layout.preview_visible) {
        (true, true) => "Library and bottom tool open.",
        (true, false) => "Library open, bottom tool closed.",
        (false, true) => "Library closed, bottom tool open.",
        (false, false) => "Library and bottom tool closed.",
    }
}
