use gpui_kit::{App, KeyBinding};

/// Stable command identity shared by keys, toolbar, and palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) enum CommandId {
    OpenFile,
    OpenProject,
    Save,
    RecoverUnsaved,
    OpenPalette,
    ToggleSidebar,
    ToggleInspector,
    TogglePreview,
    StartPreview,
    StopPreview,
    OpenPreview,
    FocusDocument,
    LightTheme,
    DarkTheme,
    SystemTheme,
    ResetLayout,
    Quit,
}

pub(crate) const COMMANDS: &[CommandId] = &[
    CommandId::OpenFile,
    CommandId::OpenProject,
    CommandId::Save,
    CommandId::RecoverUnsaved,
    CommandId::OpenPalette,
    CommandId::ToggleSidebar,
    CommandId::ToggleInspector,
    CommandId::TogglePreview,
    CommandId::StartPreview,
    CommandId::StopPreview,
    CommandId::OpenPreview,
    CommandId::FocusDocument,
    CommandId::LightTheme,
    CommandId::DarkTheme,
    CommandId::SystemTheme,
    CommandId::ResetLayout,
    CommandId::Quit,
];

impl CommandId {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenFile => "Open file…",
            Self::OpenProject => "Open project…",
            Self::Save => "Save",
            Self::RecoverUnsaved => "Recover unsaved changes…",
            Self::OpenPalette => "Commands…",
            Self::ToggleSidebar => "Toggle files",
            Self::ToggleInspector => "Toggle inspector",
            Self::TogglePreview => "Toggle preview",
            Self::StartPreview => "Start preview server",
            Self::StopPreview => "Stop preview server",
            Self::OpenPreview => "Open preview in browser",
            Self::FocusDocument => "Focus document",
            Self::LightTheme => "Light theme",
            Self::DarkTheme => "Dark theme",
            Self::SystemTheme => "System theme",
            Self::ResetLayout => "Reset layout",
            Self::Quit => "Quit Astrolith",
        }
    }

    pub fn shortcut(self) -> Option<&'static str> {
        let mac = cfg!(target_os = "macos");
        Some(match self {
            Self::OpenFile => {
                if mac {
                    "cmd-o"
                } else {
                    "ctrl-o"
                }
            }
            Self::OpenProject => {
                if mac {
                    "cmd-shift-o"
                } else {
                    "ctrl-shift-o"
                }
            }
            Self::Save => {
                if mac {
                    "cmd-s"
                } else {
                    "ctrl-s"
                }
            }
            Self::OpenPalette => {
                if mac {
                    "cmd-shift-p"
                } else {
                    "ctrl-shift-p"
                }
            }
            Self::ToggleSidebar => {
                if mac {
                    "cmd-b"
                } else {
                    "ctrl-b"
                }
            }
            Self::ToggleInspector => {
                if mac {
                    "cmd-alt-i"
                } else {
                    "ctrl-alt-i"
                }
            }
            Self::TogglePreview => {
                if mac {
                    "cmd-alt-p"
                } else {
                    "ctrl-alt-p"
                }
            }
            Self::FocusDocument => {
                if mac {
                    "cmd-1"
                } else {
                    "ctrl-1"
                }
            }
            Self::Quit => {
                if mac {
                    "cmd-q"
                } else {
                    "ctrl-q"
                }
            }
            _ => return None,
        })
    }
}

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys(COMMANDS.iter().filter_map(|command| {
        command
            .shortcut()
            .map(|key| KeyBinding::new(key, *command, Some("Astrolith")))
    }));
}

/// Install the application menu bar.
///
/// Every item dispatches the same [`CommandId`] the toolbar, palette and key
/// bindings use, so the OS menu cannot drift from the command registry. Linux
/// platforms without a native menu bar simply retain the registered menus;
/// macOS and Windows present them natively.
pub(crate) fn install_menus(cx: &mut App) {
    let file = gpui_kit::Menu::new("File").items({
        let mut items = items_for(&[
            CommandId::OpenFile,
            CommandId::OpenProject,
            CommandId::Save,
            CommandId::RecoverUnsaved,
        ]);
        items.push(gpui_kit::MenuItem::separator());
        items.push(gpui_kit::MenuItem::action(
            CommandId::Quit.label(),
            CommandId::Quit,
        ));
        items
    });
    let view = gpui_kit::Menu::new("View").items({
        let mut items = items_for(&[
            CommandId::OpenPalette,
            CommandId::ToggleSidebar,
            CommandId::ToggleInspector,
            CommandId::TogglePreview,
            CommandId::FocusDocument,
            CommandId::ResetLayout,
        ]);
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(&[
            CommandId::LightTheme,
            CommandId::DarkTheme,
            CommandId::SystemTheme,
        ]));
        items
    });
    let preview = gpui_kit::Menu::new("Preview").items(items_for(&[
        CommandId::StartPreview,
        CommandId::StopPreview,
        CommandId::OpenPreview,
    ]));
    cx.set_menus([file, view, preview]);
}

fn items_for(commands: &[CommandId]) -> Vec<gpui_kit::MenuItem> {
    commands
        .iter()
        .map(|command| gpui_kit::MenuItem::action(command.label(), *command))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registered_commands_have_unique_identity_and_shortcuts() {
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!(ids.insert(*command));
            if let Some(key) = command.shortcut() {
                assert!(keys.insert(key));
            }
        }
    }

    #[gpui_kit::test]
    fn application_menus_cover_every_registered_command_once(cx: &mut gpui_kit::TestAppContext) {
        let menus = cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
            install_menus(cx);
            cx.get_menus().expect("the menu bar is registered")
        });
        assert_eq!(
            menus
                .iter()
                .map(|menu| menu.name.to_string())
                .collect::<Vec<_>>(),
            ["File", "View", "Preview"]
        );
        let mut labels = Vec::new();
        for menu in &menus {
            for item in &menu.items {
                if let gpui_kit::OwnedMenuItem::Action { name, .. } = item {
                    labels.push(name.clone());
                }
            }
        }
        for command in COMMANDS {
            assert!(
                labels.iter().any(|label| label == command.label()),
                "`{}` is missing from the menu bar",
                command.label()
            );
        }
        assert_eq!(
            labels.len(),
            COMMANDS.len(),
            "every command must appear exactly once: {labels:?}"
        );
    }
}
