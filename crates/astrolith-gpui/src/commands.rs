use gpui_kit::{App, KeyBinding};

/// Stable command identity shared by keys, toolbar, and palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) enum CommandId {
    OpenFile,
    OpenPalette,
    ToggleSidebar,
    ToggleInspector,
    TogglePreview,
    FocusDocument,
    LightTheme,
    DarkTheme,
    SystemTheme,
    ResetLayout,
}

pub(crate) const COMMANDS: &[CommandId] = &[
    CommandId::OpenFile,
    CommandId::OpenPalette,
    CommandId::ToggleSidebar,
    CommandId::ToggleInspector,
    CommandId::TogglePreview,
    CommandId::FocusDocument,
    CommandId::LightTheme,
    CommandId::DarkTheme,
    CommandId::SystemTheme,
    CommandId::ResetLayout,
];

impl CommandId {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenFile => "Open file…",
            Self::OpenPalette => "Commands…",
            Self::ToggleSidebar => "Toggle files",
            Self::ToggleInspector => "Toggle inspector",
            Self::TogglePreview => "Toggle preview",
            Self::FocusDocument => "Focus document",
            Self::LightTheme => "Light theme",
            Self::DarkTheme => "Dark theme",
            Self::SystemTheme => "System theme",
            Self::ResetLayout => "Reset layout",
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
}
