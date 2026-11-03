use astrolith_core::preferences::{CopyeditCategory, CopyeditPreferences};
use gpui_kit::{App, KeyBinding};

/// Stable command identity shared by keys, toolbar, and palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) enum CommandId {
    OpenFile,
    OpenProject,
    NewEntry,
    DuplicateEntry,
    RenameEntry,
    DeleteEntry,
    RevealEntry,
    CopyPathAbsolute,
    CopyPathRelative,
    Save,
    CloseDocument,
    RecoverUnsaved,
    FindInDocument,
    ReplaceInDocument,
    FindNextMatch,
    FindPreviousMatch,
    CloseFind,
    GoToLine,
    OpenContentFinder,
    SelectNextOccurrence,
    CursorsAtLineEnds,
    Bold,
    Italic,
    InsertLink,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Paragraph,
    ToggleComment,
    OpenComponentBuilder,
    ToggleFocusMode,
    ToggleTypewriterMode,
    ToggleCopyeditNouns,
    ToggleCopyeditVerbs,
    ToggleCopyeditAdjectives,
    ToggleCopyeditAdverbs,
    ToggleCopyeditConjunctions,
    ToggleAllCopyedit,
    OpenPalette,
    ToggleSidebar,
    ToggleInspector,
    TogglePreview,
    StartPreview,
    StopPreview,
    OpenPreview,
    FocusDocument,
    FocusSelectionToolbar,
    LightTheme,
    DarkTheme,
    SystemTheme,
    ResetLayout,
    Quit,
}

pub(crate) const COMMANDS: &[CommandId] = &[
    CommandId::OpenFile,
    CommandId::OpenProject,
    CommandId::NewEntry,
    CommandId::DuplicateEntry,
    CommandId::RenameEntry,
    CommandId::DeleteEntry,
    CommandId::RevealEntry,
    CommandId::CopyPathAbsolute,
    CommandId::CopyPathRelative,
    CommandId::Save,
    CommandId::CloseDocument,
    CommandId::RecoverUnsaved,
    CommandId::FindInDocument,
    CommandId::ReplaceInDocument,
    CommandId::FindNextMatch,
    CommandId::FindPreviousMatch,
    CommandId::CloseFind,
    CommandId::GoToLine,
    CommandId::OpenContentFinder,
    CommandId::SelectNextOccurrence,
    CommandId::CursorsAtLineEnds,
    CommandId::Bold,
    CommandId::Italic,
    CommandId::InsertLink,
    CommandId::Heading1,
    CommandId::Heading2,
    CommandId::Heading3,
    CommandId::Heading4,
    CommandId::Paragraph,
    CommandId::ToggleComment,
    CommandId::OpenComponentBuilder,
    CommandId::ToggleFocusMode,
    CommandId::ToggleTypewriterMode,
    CommandId::ToggleCopyeditNouns,
    CommandId::ToggleCopyeditVerbs,
    CommandId::ToggleCopyeditAdjectives,
    CommandId::ToggleCopyeditAdverbs,
    CommandId::ToggleCopyeditConjunctions,
    CommandId::ToggleAllCopyedit,
    CommandId::OpenPalette,
    CommandId::ToggleSidebar,
    CommandId::ToggleInspector,
    CommandId::TogglePreview,
    CommandId::StartPreview,
    CommandId::StopPreview,
    CommandId::OpenPreview,
    CommandId::FocusDocument,
    CommandId::FocusSelectionToolbar,
    CommandId::LightTheme,
    CommandId::DarkTheme,
    CommandId::SystemTheme,
    CommandId::ResetLayout,
    CommandId::Quit,
];

/// Live application state that decides whether a command can run.
///
/// The palette, toolbar and file context menu all read this one value, so the
/// same [`CommandId`] is enabled or disabled identically everywhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CommandAvailability {
    /// A project with a published index is open.
    pub project_open: bool,
    /// An entry is selected and can be operated on.
    pub active_entry: bool,
    /// The initial project scan is running; entry files must not change yet.
    pub busy: bool,
    /// The active document's editor owns keyboard focus.
    pub editor_focused: bool,
    /// The active document is Markdown, so `Mod+/` toggles a comment rather
    /// than opening the MDX builder.
    pub markdown_document: bool,
    /// The active document is MDX, so `Mod+/` opens the component builder.
    pub mdx_document: bool,
    /// The active editor has a non-empty selection.
    pub selection_nonempty: bool,
}

impl CommandId {
    /// The Copyedit category this command toggles, if any.
    pub fn copyedit_category(self) -> Option<CopyeditCategory> {
        match self {
            Self::ToggleCopyeditNouns => Some(CopyeditCategory::Noun),
            Self::ToggleCopyeditVerbs => Some(CopyeditCategory::Verb),
            Self::ToggleCopyeditAdjectives => Some(CopyeditCategory::Adjective),
            Self::ToggleCopyeditAdverbs => Some(CopyeditCategory::Adverb),
            Self::ToggleCopyeditConjunctions => Some(CopyeditCategory::Conjunction),
            _ => None,
        }
    }

    /// The label for one command under the live Copyedit state.
    ///
    /// Copyedit labels come from [`CopyeditPreferences`] itself, so the
    /// command registry, menus, tooltips and the settings pane cannot drift
    /// from the persisted state.
    pub fn label_with(self, copyedit: &CopyeditPreferences) -> &'static str {
        match self.copyedit_category() {
            Some(category) => copyedit.toggle_label(category),
            None if self == Self::ToggleAllCopyedit => copyedit.all_toggle_label(),
            None => self.label(),
        }
    }

    /// Whether this command can run in the given application state.
    pub fn is_available(self, state: CommandAvailability) -> bool {
        match self {
            Self::NewEntry | Self::OpenContentFinder => state.project_open && !state.busy,
            Self::DuplicateEntry
            | Self::RenameEntry
            | Self::DeleteEntry
            | Self::RevealEntry
            | Self::CopyPathAbsolute
            | Self::CopyPathRelative => state.active_entry && !state.busy,
            // Formatting follows editor focus, never a stale document flag, and
            // the Markdown comment toggle is reserved for `.md`: `.mdx` spends
            // `Mod+/` on the component builder.
            Self::Bold
            | Self::Italic
            | Self::InsertLink
            | Self::Heading1
            | Self::Heading2
            | Self::Heading3
            | Self::Heading4
            | Self::Paragraph
            | Self::FindInDocument
            | Self::ReplaceInDocument
            | Self::FindNextMatch
            | Self::FindPreviousMatch
            | Self::CloseFind
            | Self::GoToLine
            | Self::SelectNextOccurrence
            | Self::CursorsAtLineEnds => state.editor_focused,
            Self::FocusSelectionToolbar => state.editor_focused && state.selection_nonempty,
            Self::ToggleComment => state.editor_focused && state.markdown_document,
            Self::OpenComponentBuilder => state.editor_focused && state.mdx_document,
            _ => true,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenFile => "Open file…",
            Self::OpenProject => "Open project…",
            Self::NewEntry => "New file",
            Self::DuplicateEntry => "Duplicate file",
            Self::RenameEntry => "Rename or move file…",
            Self::DeleteEntry => "Move file to Trash…",
            Self::RevealEntry => "Reveal in file manager",
            Self::CopyPathAbsolute => "Copy absolute path",
            Self::CopyPathRelative => "Copy project-relative path",
            Self::Save => "Save",
            Self::CloseDocument => "Close file",
            Self::RecoverUnsaved => "Recover unsaved changes…",
            Self::FindInDocument => "Find…",
            Self::ReplaceInDocument => "Find and replace…",
            Self::FindNextMatch => "Find next",
            Self::FindPreviousMatch => "Find previous",
            Self::CloseFind => "Close find",
            Self::GoToLine => "Go to line…",
            Self::OpenContentFinder => "Content Finder…",
            Self::SelectNextOccurrence => "Select next occurrence",
            Self::CursorsAtLineEnds => "Add cursors at line ends",
            Self::Bold => "Bold",
            Self::Italic => "Italic",
            Self::InsertLink => "Link…",
            Self::Heading1 => "Heading 1",
            Self::Heading2 => "Heading 2",
            Self::Heading3 => "Heading 3",
            Self::Heading4 => "Heading 4",
            Self::Paragraph => "Paragraph",
            Self::ToggleComment => "Toggle Markdown comment",
            Self::OpenComponentBuilder => "Insert MDX component…",
            Self::ToggleFocusMode => "Focus mode",
            Self::ToggleTypewriterMode => "Typewriter mode",
            Self::ToggleCopyeditNouns => {
                CopyeditPreferences::default().toggle_label(CopyeditCategory::Noun)
            }
            Self::ToggleCopyeditVerbs => {
                CopyeditPreferences::default().toggle_label(CopyeditCategory::Verb)
            }
            Self::ToggleCopyeditAdjectives => {
                CopyeditPreferences::default().toggle_label(CopyeditCategory::Adjective)
            }
            Self::ToggleCopyeditAdverbs => {
                CopyeditPreferences::default().toggle_label(CopyeditCategory::Adverb)
            }
            Self::ToggleCopyeditConjunctions => {
                CopyeditPreferences::default().toggle_label(CopyeditCategory::Conjunction)
            }
            Self::ToggleAllCopyedit => CopyeditPreferences::default().all_toggle_label(),
            Self::OpenPalette => "Commands…",
            Self::ToggleSidebar => "Toggle files",
            Self::ToggleInspector => "Toggle inspector",
            Self::TogglePreview => "Toggle preview",
            Self::StartPreview => "Start preview server",
            Self::StopPreview => "Stop preview server",
            Self::OpenPreview => "Open preview in browser",
            Self::FocusDocument => "Focus document",
            Self::FocusSelectionToolbar => "Focus selection toolbar",
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
            Self::NewEntry => {
                if mac {
                    "cmd-n"
                } else {
                    "ctrl-n"
                }
            }
            Self::Save => {
                if mac {
                    "cmd-s"
                } else {
                    "ctrl-s"
                }
            }
            Self::CloseDocument => {
                if mac {
                    "cmd-w"
                } else {
                    "ctrl-w"
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
                    "cmd-1"
                } else {
                    "ctrl-1"
                }
            }
            Self::ToggleInspector => {
                if mac {
                    "cmd-2"
                } else {
                    "ctrl-2"
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
                    "cmd-0"
                } else {
                    "ctrl-0"
                }
            }
            Self::FocusSelectionToolbar => return None,
            Self::FindInDocument => {
                if mac {
                    "cmd-f"
                } else {
                    "ctrl-f"
                }
            }
            Self::ReplaceInDocument => {
                if mac {
                    "cmd-h"
                } else {
                    "ctrl-h"
                }
            }
            Self::FindNextMatch => {
                if mac {
                    "cmd-g"
                } else {
                    "ctrl-g"
                }
            }
            Self::FindPreviousMatch => {
                if mac {
                    "cmd-shift-g"
                } else {
                    "ctrl-shift-g"
                }
            }
            Self::GoToLine => {
                if mac {
                    "cmd-alt-g"
                } else {
                    "ctrl-alt-g"
                }
            }
            Self::OpenContentFinder => {
                if mac {
                    "cmd-shift-k"
                } else {
                    "ctrl-shift-k"
                }
            }
            Self::SelectNextOccurrence => {
                if mac {
                    "cmd-d"
                } else {
                    "ctrl-d"
                }
            }
            Self::CursorsAtLineEnds => {
                if mac {
                    "cmd-shift-l"
                } else {
                    "ctrl-shift-l"
                }
            }
            Self::Bold => {
                if mac {
                    "cmd-b"
                } else {
                    "ctrl-b"
                }
            }
            Self::Italic => {
                if mac {
                    "cmd-i"
                } else {
                    "ctrl-i"
                }
            }
            Self::InsertLink => {
                if mac {
                    "cmd-k"
                } else {
                    "ctrl-k"
                }
            }
            Self::Heading1 => {
                if mac {
                    "cmd-alt-1"
                } else {
                    "ctrl-alt-1"
                }
            }
            Self::Heading2 => {
                if mac {
                    "cmd-alt-2"
                } else {
                    "ctrl-alt-2"
                }
            }
            Self::Heading3 => {
                if mac {
                    "cmd-alt-3"
                } else {
                    "ctrl-alt-3"
                }
            }
            Self::Heading4 => {
                if mac {
                    "cmd-alt-4"
                } else {
                    "ctrl-alt-4"
                }
            }
            Self::Paragraph => {
                if mac {
                    "cmd-alt-0"
                } else {
                    "ctrl-alt-0"
                }
            }
            Self::ToggleComment | Self::OpenComponentBuilder => {
                if mac {
                    "cmd-/"
                } else {
                    "ctrl-/"
                }
            }
            Self::ToggleFocusMode => {
                if mac {
                    "cmd-shift-f"
                } else {
                    "ctrl-shift-f"
                }
            }
            Self::ToggleTypewriterMode => {
                if mac {
                    "cmd-shift-t"
                } else {
                    "ctrl-shift-t"
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
        // `Mod+/` is document-kind dependent: one physical key dispatches the
        // Markdown comment toggle or the MDX builder. The two registry entries
        // advertise the same key, so the generic binding skips both.
        if matches!(
            command,
            CommandId::ToggleComment | CommandId::OpenComponentBuilder
        ) {
            return None;
        }
        command
            .shortcut()
            .map(|key| KeyBinding::new(key, *command, Some("Astrolith")))
    }));
    let mod_slash = if cfg!(target_os = "macos") {
        "cmd-/"
    } else {
        "ctrl-/"
    };
    cx.bind_keys([KeyBinding::new(mod_slash, ModSlash, Some("Astrolith"))]);
    // The Content Finder's secondary Enter inserts a link instead of opening
    // the entry. It is scoped to the dialog, so ordinary Enter keeps working.
    let insert = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    cx.bind_keys([KeyBinding::new(
        insert,
        ContentFinderInsert,
        Some("ContentFinder"),
    )]);
}

/// The kind-dependent `Mod+/` key: a comment in `.md`, the builder in `.mdx`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) struct ModSlash;

/// Insert the selected Content Finder entry as a Markdown link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) struct ContentFinderInsert;

/// Install the application menu bar.
///
/// Every item dispatches the same [`CommandId`] the toolbar, palette and key
/// bindings use, so the OS menu cannot drift from the command registry. Linux
/// platforms without a native menu bar simply retain the registered menus;
/// macOS and Windows present them natively.
pub(crate) fn install_menus(cx: &mut App, copyedit: &CopyeditPreferences) {
    let file = gpui_kit::Menu::new("File").items({
        let mut items = items_for(
            &[
                CommandId::OpenFile,
                CommandId::OpenProject,
                CommandId::NewEntry,
                CommandId::Save,
                CommandId::CloseDocument,
                CommandId::DuplicateEntry,
                CommandId::RenameEntry,
                CommandId::DeleteEntry,
            ],
            copyedit,
        );
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(
            &[
                CommandId::RevealEntry,
                CommandId::CopyPathAbsolute,
                CommandId::CopyPathRelative,
                CommandId::RecoverUnsaved,
            ],
            copyedit,
        ));
        items.push(gpui_kit::MenuItem::separator());
        items.push(gpui_kit::MenuItem::action(
            CommandId::Quit.label(),
            CommandId::Quit,
        ));
        items
    });
    let view = gpui_kit::Menu::new("View").items({
        let mut items = items_for(
            &[
                CommandId::OpenPalette,
                CommandId::ToggleSidebar,
                CommandId::ToggleInspector,
                CommandId::TogglePreview,
                CommandId::FocusDocument,
                CommandId::ResetLayout,
            ],
            copyedit,
        );
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(
            &[
                CommandId::LightTheme,
                CommandId::DarkTheme,
                CommandId::SystemTheme,
            ],
            copyedit,
        ));
        items
    });
    let format = gpui_kit::Menu::new("Format").items({
        let mut items = items_for(
            &[
                CommandId::Bold,
                CommandId::Italic,
                CommandId::InsertLink,
                CommandId::ToggleComment,
                CommandId::OpenComponentBuilder,
            ],
            copyedit,
        );
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(
            &[
                CommandId::Heading1,
                CommandId::Heading2,
                CommandId::Heading3,
                CommandId::Heading4,
                CommandId::Paragraph,
            ],
            copyedit,
        ));
        items
    });
    let writer = gpui_kit::Menu::new("Writer").items({
        let mut items = items_for(
            &[
                CommandId::FindInDocument,
                CommandId::ReplaceInDocument,
                CommandId::FindNextMatch,
                CommandId::FindPreviousMatch,
                CommandId::CloseFind,
                CommandId::GoToLine,
                CommandId::OpenContentFinder,
                CommandId::SelectNextOccurrence,
                CommandId::CursorsAtLineEnds,
                CommandId::FocusSelectionToolbar,
                CommandId::ToggleFocusMode,
                CommandId::ToggleTypewriterMode,
            ],
            copyedit,
        );
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(
            &[
                CommandId::ToggleCopyeditNouns,
                CommandId::ToggleCopyeditVerbs,
                CommandId::ToggleCopyeditAdjectives,
                CommandId::ToggleCopyeditAdverbs,
                CommandId::ToggleCopyeditConjunctions,
                CommandId::ToggleAllCopyedit,
            ],
            copyedit,
        ));
        items
    });
    let preview = gpui_kit::Menu::new("Preview").items(items_for(
        &[
            CommandId::StartPreview,
            CommandId::StopPreview,
            CommandId::OpenPreview,
        ],
        copyedit,
    ));
    cx.set_menus([file, view, format, writer, preview]);
}

fn items_for(commands: &[CommandId], copyedit: &CopyeditPreferences) -> Vec<gpui_kit::MenuItem> {
    commands
        .iter()
        .map(|command| gpui_kit::MenuItem::action(command.label_with(copyedit), *command))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formatting_follows_editor_focus_and_the_comment_belongs_to_markdown() {
        let blurred = CommandAvailability::default();
        let focused = CommandAvailability {
            editor_focused: true,
            markdown_document: true,
            ..CommandAvailability::default()
        };
        for command in [
            CommandId::Bold,
            CommandId::Italic,
            CommandId::InsertLink,
            CommandId::Heading1,
            CommandId::Heading2,
            CommandId::Heading3,
            CommandId::Heading4,
            CommandId::Paragraph,
            CommandId::FindInDocument,
            CommandId::GoToLine,
            CommandId::SelectNextOccurrence,
            CommandId::CursorsAtLineEnds,
        ] {
            assert!(!command.is_available(blurred), "{}", command.label());
            assert!(command.is_available(focused), "{}", command.label());
        }
        assert!(!CommandId::ToggleComment.is_available(blurred));
        assert!(CommandId::ToggleComment.is_available(focused));
        // `.mdx` reserves Mod+/ for the component builder.
        assert!(!CommandId::ToggleComment.is_available(CommandAvailability {
            markdown_document: false,
            mdx_document: true,
            ..focused
        }));
        assert!(
            CommandId::OpenComponentBuilder.is_available(CommandAvailability {
                markdown_document: false,
                mdx_document: true,
                ..focused
            })
        );
        assert!(!CommandId::OpenComponentBuilder.is_available(focused));
        assert!(!CommandId::OpenComponentBuilder.is_available(blurred));
        // Writer modes are global view toggles, not document commands.
        assert!(CommandId::ToggleFocusMode.is_available(blurred));
        assert!(CommandId::ToggleTypewriterMode.is_available(blurred));
    }

    #[test]
    fn registered_commands_have_unique_identity_and_shortcuts() {
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!(ids.insert(*command));
            if let Some(key) = command.shortcut() {
                // `Mod+/` is one physical key with two document-kind commands,
                // dispatched by `ModSlash`; every other shortcut is unique.
                if matches!(
                    command,
                    CommandId::ToggleComment | CommandId::OpenComponentBuilder
                ) {
                    continue;
                }
                assert!(keys.insert(key));
            }
        }
        assert_eq!(
            CommandId::ToggleComment.shortcut(),
            CommandId::OpenComponentBuilder.shortcut()
        );
    }

    #[test]
    fn entry_commands_share_one_availability_rule() {
        let empty = CommandAvailability::default();
        let ready = CommandAvailability {
            project_open: true,
            active_entry: true,
            ..CommandAvailability::default()
        };
        assert!(!CommandId::NewEntry.is_available(empty));
        assert!(CommandId::NewEntry.is_available(ready));
        assert!(!CommandId::OpenContentFinder.is_available(empty));
        assert!(CommandId::OpenContentFinder.is_available(ready));
        assert!(
            !CommandId::OpenContentFinder.is_available(CommandAvailability {
                busy: true,
                ..ready
            })
        );
        for command in [
            CommandId::DuplicateEntry,
            CommandId::RenameEntry,
            CommandId::DeleteEntry,
            CommandId::RevealEntry,
            CommandId::CopyPathAbsolute,
            CommandId::CopyPathRelative,
        ] {
            assert!(!command.is_available(empty), "{}", command.label());
            assert!(
                !command.is_available(CommandAvailability {
                    project_open: true,
                    active_entry: false,
                    ..CommandAvailability::default()
                }),
                "{} must require an entry",
                command.label()
            );
            assert!(command.is_available(ready), "{}", command.label());
            assert!(
                !command.is_available(CommandAvailability {
                    busy: true,
                    ..ready
                }),
                "{} must be disabled while the index is busy",
                command.label()
            );
        }
        // Existing commands stay available; their handlers report their own
        // preconditions instead of disappearing from the palette.
        assert!(CommandId::Save.is_available(empty));
        assert!(CommandId::OpenProject.is_available(empty));
    }

    #[test]
    fn copyedit_toggles_share_one_label_source_and_availability() {
        let mut copyedit = CopyeditPreferences::default();
        let commands = [
            CommandId::ToggleCopyeditNouns,
            CommandId::ToggleCopyeditVerbs,
            CommandId::ToggleCopyeditAdjectives,
            CommandId::ToggleCopyeditAdverbs,
            CommandId::ToggleCopyeditConjunctions,
            CommandId::ToggleAllCopyedit,
        ];
        for command in commands {
            assert!(
                command.copyedit_category().is_some() || command == CommandId::ToggleAllCopyedit
            );
            assert_eq!(command.label_with(&copyedit), command.label());
        }
        assert_eq!(
            CommandId::ToggleCopyeditNouns.label_with(&copyedit),
            copyedit.toggle_label(CopyeditCategory::Noun)
        );
        assert_eq!(
            CommandId::ToggleAllCopyedit.label_with(&copyedit),
            copyedit.all_toggle_label()
        );

        let states = [
            CommandAvailability::default(),
            CommandAvailability {
                project_open: true,
                active_entry: true,
                busy: true,
                editor_focused: true,
                markdown_document: true,
                mdx_document: false,
                selection_nonempty: true,
            },
        ];
        for state in states {
            let available = CommandId::ToggleCopyeditNouns.is_available(state);
            for command in commands {
                assert_eq!(
                    command.is_available(state),
                    available,
                    "{} must share Copyedit availability",
                    command.label_with(&copyedit)
                );
            }
            assert!(available, "Copyedit toggles are global view commands");
        }

        copyedit.set_all(true);
        assert_eq!(
            CommandId::ToggleCopyeditVerbs.label_with(&copyedit),
            "Hide verbs"
        );
        assert_eq!(
            CommandId::ToggleAllCopyedit.label_with(&copyedit),
            "Hide all copyedit"
        );
    }

    #[gpui_kit::test]
    fn application_menus_cover_every_registered_command_once(cx: &mut gpui_kit::TestAppContext) {
        let copyedit = CopyeditPreferences::default();
        let menus = cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
            install_menus(cx, &copyedit);
            cx.get_menus().expect("the menu bar is registered")
        });
        assert_eq!(
            menus
                .iter()
                .map(|menu| menu.name.to_string())
                .collect::<Vec<_>>(),
            ["File", "View", "Format", "Writer", "Preview"]
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
            let label = command.label_with(&copyedit);
            assert!(
                labels.iter().any(|candidate| candidate == label),
                "`{label}` is missing from the menu bar"
            );
        }
        assert_eq!(
            labels.len(),
            COMMANDS.len(),
            "every command must appear exactly once: {labels:?}"
        );
    }
}
