use astrolith_core::preferences::{CopyeditCategory, CopyeditPreferences, WorkspaceMode};
use gpui_kit::{App, KeyBinding};

use crate::recent_projects::RecentEntry;

/// Stable command identity shared by keys, toolbar, and palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) enum CommandId {
    OpenFile,
    OpenProject,
    OpenRecentProjects,
    ReloadCollections,
    OpenProjectInIde,
    OpenCollectionInIde,
    OpenEntryInIde,
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
    WorkspaceWriting,
    WorkspaceBuild,
    WorkspacePreview,
    CycleWorkspaceMode,
    OpenPalette,
    OpenPreferences,
    ToggleSidebar,
    ToggleInspector,
    TogglePreview,
    StartPreview,
    StopPreview,
    OpenPreview,
    RestartPreview,
    CopyPreviewUrl,
    FocusDocument,
    FocusSelectionToolbar,
    LightTheme,
    DarkTheme,
    SystemTheme,
    ResetLayout,
    ToggleFullscreen,
    OpenUserGuide,
    OpenKeyboardShortcuts,
    OpenLocalData,
    About,
    CheckForUpdates,
    Quit,
}

pub(crate) const COMMANDS: &[CommandId] = &[
    CommandId::OpenFile,
    CommandId::OpenProject,
    CommandId::OpenRecentProjects,
    CommandId::ReloadCollections,
    CommandId::OpenProjectInIde,
    CommandId::OpenCollectionInIde,
    CommandId::OpenEntryInIde,
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
    CommandId::WorkspaceWriting,
    CommandId::WorkspaceBuild,
    CommandId::WorkspacePreview,
    CommandId::CycleWorkspaceMode,
    CommandId::OpenPalette,
    CommandId::OpenPreferences,
    CommandId::ToggleSidebar,
    CommandId::ToggleInspector,
    CommandId::TogglePreview,
    CommandId::StartPreview,
    CommandId::StopPreview,
    CommandId::OpenPreview,
    CommandId::RestartPreview,
    CommandId::CopyPreviewUrl,
    CommandId::FocusDocument,
    CommandId::FocusSelectionToolbar,
    CommandId::LightTheme,
    CommandId::DarkTheme,
    CommandId::SystemTheme,
    CommandId::ResetLayout,
    CommandId::ToggleFullscreen,
    CommandId::OpenUserGuide,
    CommandId::OpenKeyboardShortcuts,
    CommandId::OpenLocalData,
    CommandId::About,
    CommandId::CheckForUpdates,
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
    /// An IDE command is configured in preferences.
    pub ide_configured: bool,
    /// The left dock is open.
    pub sidebar_visible: bool,
    /// The right dock is open.
    pub inspector_visible: bool,
    /// The bottom preview dock is open.
    pub preview_visible: bool,
    /// A preview backend is attached to an open project.
    pub preview_attached: bool,
    /// The preview dev server is starting, ready or stopping.
    pub preview_running: bool,
    /// The preview dev server is ready to serve a route.
    pub preview_ready: bool,
    /// Focus mode is active.
    pub focus_mode: bool,
    /// Typewriter mode is active.
    pub typewriter_mode: bool,
    /// The window is fullscreen.
    pub fullscreen: bool,
    /// Active workspace mode, used for checked menu and palette state.
    pub workspace_mode: WorkspaceMode,
    /// This build carries a manifest URL and public key for update checks.
    pub updates_configured: bool,
}

/// One collection offered as a switch command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollectionItem {
    /// Collection identity from the live index.
    pub name: String,
    /// Whether the active document already belongs to this collection.
    pub active: bool,
}

/// The shared label for one generated collection command.
pub(crate) fn collection_command_label(name: &str) -> String {
    format!("Open Collection: {name}")
}

/// Open one recent project from the registry.
#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) struct OpenRecentProject {
    /// Stable registry identity.
    pub id: String,
}

/// Remove one project from the recent list.
#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) struct RemoveRecentProject {
    /// Stable registry identity.
    pub id: String,
}

/// Switch the file list to one collection.
#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = astrolith, no_json)]
pub(crate) struct OpenCollection {
    /// Collection identity from the live index.
    pub id: String,
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
            Self::NewEntry | Self::OpenContentFinder | Self::ReloadCollections => {
                state.project_open && !state.busy
            }
            Self::StartPreview => {
                state.project_open
                    && state.preview_attached
                    && !state.preview_running
                    && !state.busy
            }
            Self::StopPreview => state.preview_running && !state.busy,
            Self::RestartPreview => state.preview_attached && !state.busy,
            Self::OpenPreview => state.preview_ready && state.active_entry && !state.busy,
            Self::CopyPreviewUrl => state.preview_ready && state.active_entry,
            Self::OpenProjectInIde | Self::OpenCollectionInIde => {
                state.project_open && state.ide_configured && !state.busy
            }
            Self::OpenEntryInIde => state.active_entry && state.ide_configured && !state.busy,
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
            Self::CheckForUpdates => state.updates_configured,
            _ => true,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenFile => "Open file…",
            Self::OpenProject => "Open project…",
            Self::OpenRecentProjects => "Manage Recent Projects…",
            Self::ReloadCollections => "Reload collections",
            Self::OpenProjectInIde => "Open project in IDE",
            Self::OpenCollectionInIde => "Open collection in IDE",
            Self::OpenEntryInIde => "Open file in IDE",
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
            Self::WorkspaceWriting => "Writing workspace",
            Self::WorkspaceBuild => "Build workspace",
            Self::WorkspacePreview => "Preview workspace",
            Self::CycleWorkspaceMode => "Cycle workspace mode",
            Self::OpenPalette => "Commands…",
            Self::OpenPreferences => "Preferences…",
            Self::ToggleSidebar => "Toggle files",
            Self::ToggleInspector => "Toggle inspector",
            Self::TogglePreview => "Toggle preview",
            Self::StartPreview => "Start preview server",
            Self::StopPreview => "Stop preview server",
            Self::OpenPreview => "Open preview in browser",
            Self::RestartPreview => "Restart preview server",
            Self::CopyPreviewUrl => "Copy preview URL",
            Self::FocusDocument => "Focus document",
            Self::FocusSelectionToolbar => "Focus selection toolbar",
            Self::LightTheme => "Light theme",
            Self::DarkTheme => "Dark theme",
            Self::SystemTheme => "System theme",
            Self::ResetLayout => "Reset layout",
            Self::ToggleFullscreen => "Enter full screen",
            Self::OpenUserGuide => "User guide",
            Self::OpenKeyboardShortcuts => "Keyboard shortcuts",
            Self::OpenLocalData => "Privacy and local data",
            Self::About => "About Astrolith",
            Self::CheckForUpdates => "Check for updates…",
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
            Self::WorkspaceWriting => "ctrl-shift-1",
            Self::WorkspaceBuild => "ctrl-shift-2",
            Self::WorkspacePreview => "ctrl-shift-3",
            Self::CycleWorkspaceMode => "ctrl-shift-m",
            Self::OpenPalette => {
                if mac {
                    "cmd-p"
                } else {
                    "ctrl-p"
                }
            }
            Self::OpenPreferences => {
                if mac {
                    "cmd-,"
                } else {
                    "ctrl-,"
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
            Self::OpenPreview => {
                if mac {
                    "cmd-enter"
                } else {
                    "ctrl-enter"
                }
            }
            Self::ToggleFullscreen => {
                // F11 is an explicit Linux decision; other platforms reach
                // fullscreen through the menu and command alone.
                if cfg!(target_os = "linux") {
                    "f11"
                } else {
                    return None;
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
pub(crate) fn install_menus(
    cx: &mut App,
    copyedit: &CopyeditPreferences,
    availability: CommandAvailability,
    recent: &[RecentEntry],
    collections: &[CollectionItem],
) {
    let file = gpui_kit::Menu::new("File").items({
        let mut items = items_for(
            &[
                CommandId::OpenFile,
                CommandId::OpenProject,
                CommandId::ReloadCollections,
                CommandId::NewEntry,
                CommandId::Save,
                CommandId::CloseDocument,
                CommandId::DuplicateEntry,
                CommandId::RenameEntry,
                CommandId::DeleteEntry,
            ],
            copyedit,
            availability,
        );
        items.push(gpui_kit::MenuItem::separator());
        let mut recent_items: Vec<gpui_kit::MenuItem> = recent
            .iter()
            .map(|entry| {
                gpui_kit::MenuItem::action(
                    entry.label.clone(),
                    OpenRecentProject {
                        id: entry.id.clone(),
                    },
                )
            })
            .collect();
        if recent_items.is_empty() {
            recent_items.push(
                gpui_kit::MenuItem::action("No recent projects", CommandId::OpenRecentProjects)
                    .disabled(true),
            );
        }
        let mut recent_menu = recent_items;
        recent_menu.push(gpui_kit::MenuItem::separator());
        recent_menu.push(menu_item(
            CommandId::OpenRecentProjects,
            copyedit,
            availability,
        ));
        items.push(gpui_kit::MenuItem::submenu(
            gpui_kit::Menu::new("Open Recent").items(recent_menu),
        ));
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(
            &[
                CommandId::RevealEntry,
                CommandId::CopyPathAbsolute,
                CommandId::CopyPathRelative,
                CommandId::RecoverUnsaved,
                CommandId::OpenProjectInIde,
                CommandId::OpenEntryInIde,
            ],
            copyedit,
            availability,
        ));
        items.push(gpui_kit::MenuItem::separator());
        items.push(menu_item(CommandId::Quit, copyedit, availability));
        items
    });
    let view = gpui_kit::Menu::new("View").items({
        let mut items = items_for(
            &[
                CommandId::WorkspaceWriting,
                CommandId::WorkspaceBuild,
                CommandId::WorkspacePreview,
                CommandId::CycleWorkspaceMode,
                CommandId::OpenPalette,
                CommandId::OpenPreferences,
                CommandId::ToggleSidebar,
                CommandId::ToggleInspector,
                CommandId::TogglePreview,
                CommandId::FocusDocument,
                CommandId::ResetLayout,
                CommandId::ToggleFullscreen,
            ],
            copyedit,
            availability,
        );
        if !collections.is_empty() {
            items.push(gpui_kit::MenuItem::separator());
            items.extend(collections.iter().map(|collection| {
                gpui_kit::MenuItem::action(
                    collection_command_label(&collection.name),
                    OpenCollection {
                        id: collection.name.clone(),
                    },
                )
                .disabled(collection.active)
            }));
        }
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(
            &[
                CommandId::LightTheme,
                CommandId::DarkTheme,
                CommandId::SystemTheme,
            ],
            copyedit,
            availability,
        ));
        items
    });
    let edit = gpui_kit::Menu::new("Edit").items({
        let mut items = items_for(
            &[
                CommandId::Bold,
                CommandId::Italic,
                CommandId::InsertLink,
                CommandId::ToggleComment,
                CommandId::OpenComponentBuilder,
            ],
            copyedit,
            availability,
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
            availability,
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
            availability,
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
            availability,
        ));
        items
    });
    let preview = gpui_kit::Menu::new("Preview").items(items_for(
        &[
            CommandId::StartPreview,
            CommandId::StopPreview,
            CommandId::RestartPreview,
            CommandId::OpenPreview,
            CommandId::CopyPreviewUrl,
        ],
        copyedit,
        availability,
    ));
    let help = gpui_kit::Menu::new("Help").items({
        let mut items = items_for(
            &[CommandId::OpenUserGuide, CommandId::OpenKeyboardShortcuts],
            copyedit,
            availability,
        );
        items.push(gpui_kit::MenuItem::separator());
        items.extend(items_for(
            &[
                CommandId::OpenLocalData,
                CommandId::About,
                CommandId::CheckForUpdates,
            ],
            copyedit,
            availability,
        ));
        items
    });
    let application = gpui_kit::Menu::new("Astrolith").items({
        let mut items = items_for(
            &[
                CommandId::About,
                CommandId::OpenPreferences,
                CommandId::OpenCollectionInIde,
            ],
            copyedit,
            availability,
        );
        items.push(gpui_kit::MenuItem::separator());
        items.push(menu_item(CommandId::Quit, copyedit, availability));
        items
    });
    cx.set_menus([file, edit, view, writer, preview, help, application]);
}

fn items_for(
    commands: &[CommandId],
    copyedit: &CopyeditPreferences,
    availability: CommandAvailability,
) -> Vec<gpui_kit::MenuItem> {
    commands
        .iter()
        .map(|command| menu_item(*command, copyedit, availability))
        .collect()
}

/// The IDE commands present in the file-row context menu.
pub(crate) const ENTRY_CONTEXT_IDE_COMMANDS: &[CommandId] = &[
    CommandId::OpenProjectInIde,
    CommandId::OpenCollectionInIde,
    CommandId::OpenEntryInIde,
];

/// The palette group a command appears under, aligned with its menu.
pub(crate) fn command_group(command: CommandId) -> &'static str {
    match command {
        CommandId::OpenFile
        | CommandId::OpenProject
        | CommandId::OpenRecentProjects
        | CommandId::ReloadCollections
        | CommandId::NewEntry
        | CommandId::DuplicateEntry
        | CommandId::RenameEntry
        | CommandId::DeleteEntry
        | CommandId::RevealEntry
        | CommandId::CopyPathAbsolute
        | CommandId::CopyPathRelative
        | CommandId::Save
        | CommandId::CloseDocument
        | CommandId::RecoverUnsaved
        | CommandId::OpenProjectInIde
        | CommandId::OpenCollectionInIde
        | CommandId::OpenEntryInIde => "File",
        CommandId::WorkspaceWriting
        | CommandId::WorkspaceBuild
        | CommandId::WorkspacePreview
        | CommandId::CycleWorkspaceMode
        | CommandId::OpenPalette
        | CommandId::OpenPreferences
        | CommandId::ToggleSidebar
        | CommandId::ToggleInspector
        | CommandId::TogglePreview
        | CommandId::FocusDocument
        | CommandId::FocusSelectionToolbar
        | CommandId::ResetLayout
        | CommandId::ToggleFullscreen
        | CommandId::LightTheme
        | CommandId::DarkTheme
        | CommandId::SystemTheme => "View",
        CommandId::Bold
        | CommandId::Italic
        | CommandId::InsertLink
        | CommandId::ToggleComment
        | CommandId::OpenComponentBuilder
        | CommandId::Heading1
        | CommandId::Heading2
        | CommandId::Heading3
        | CommandId::Heading4
        | CommandId::Paragraph => "Format",
        CommandId::FindInDocument
        | CommandId::ReplaceInDocument
        | CommandId::FindNextMatch
        | CommandId::FindPreviousMatch
        | CommandId::CloseFind
        | CommandId::GoToLine
        | CommandId::OpenContentFinder
        | CommandId::SelectNextOccurrence
        | CommandId::CursorsAtLineEnds
        | CommandId::ToggleFocusMode
        | CommandId::ToggleTypewriterMode
        | CommandId::ToggleCopyeditNouns
        | CommandId::ToggleCopyeditVerbs
        | CommandId::ToggleCopyeditAdjectives
        | CommandId::ToggleCopyeditAdverbs
        | CommandId::ToggleCopyeditConjunctions
        | CommandId::ToggleAllCopyedit => "Writer",
        CommandId::StartPreview
        | CommandId::StopPreview
        | CommandId::OpenPreview
        | CommandId::RestartPreview
        | CommandId::CopyPreviewUrl => "Preview",
        CommandId::OpenUserGuide | CommandId::OpenKeyboardShortcuts | CommandId::OpenLocalData => {
            "Help"
        }
        CommandId::About => "Help",
        CommandId::CheckForUpdates => "Help",
        CommandId::Quit => "Application",
    }
}

/// Build identity shown in About; P14 supplies a packaged identifier.
pub(crate) fn build_identity() -> &'static str {
    option_env!("ASTROLITH_BUILD_ID").unwrap_or("development build")
}

/// About content: version and build identity, with no update action.
pub(crate) fn about_text() -> String {
    format!(
        "Astrolith {}\nBuild: {}\n{} · {}",
        env!("CARGO_PKG_VERSION"),
        build_identity(),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// One menu item carrying the shared label, availability and check state.
fn menu_item(
    command: CommandId,
    copyedit: &CopyeditPreferences,
    availability: CommandAvailability,
) -> gpui_kit::MenuItem {
    let checked = match command {
        CommandId::WorkspaceWriting => availability.workspace_mode == WorkspaceMode::Writing,
        CommandId::WorkspaceBuild => availability.workspace_mode == WorkspaceMode::Build,
        CommandId::WorkspacePreview => availability.workspace_mode == WorkspaceMode::Preview,
        CommandId::ToggleSidebar => availability.sidebar_visible,
        CommandId::ToggleInspector => availability.inspector_visible,
        CommandId::TogglePreview => availability.preview_visible,
        CommandId::ToggleFocusMode => availability.focus_mode,
        CommandId::ToggleTypewriterMode => availability.typewriter_mode,
        CommandId::ToggleFullscreen => availability.fullscreen,
        _ => false,
    };
    gpui_kit::MenuItem::action(command.label_with(copyedit), command)
        .checked(checked)
        .disabled(!command.is_available(availability))
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
    fn preferences_is_a_global_command_with_the_standard_shortcut() {
        let expected = if cfg!(target_os = "macos") {
            "cmd-,"
        } else {
            "ctrl-,"
        };
        assert_eq!(CommandId::OpenPreferences.shortcut(), Some(expected));
        assert_eq!(CommandId::OpenPreferences.label(), "Preferences…");
        assert!(CommandId::OpenPreferences.is_available(CommandAvailability::default()));
        assert!(COMMANDS.contains(&CommandId::OpenPreferences));
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
        assert!(!CommandId::CheckForUpdates.is_available(empty));
        assert!(
            CommandId::CheckForUpdates.is_available(CommandAvailability {
                updates_configured: true,
                ..CommandAvailability::default()
            })
        );
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
                ..CommandAvailability::default()
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
    fn application_menus_cover_every_registered_command(cx: &mut gpui_kit::TestAppContext) {
        let copyedit = CopyeditPreferences::default();
        let recent = vec![RecentEntry {
            id: "site".into(),
            path: std::path::PathBuf::from("/projects/site"),
            label: "Open site".into(),
        }];
        let collections = vec![
            CollectionItem {
                name: "blog".into(),
                active: false,
            },
            CollectionItem {
                name: "docs".into(),
                active: true,
            },
        ];
        let menus = cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
            install_menus(
                cx,
                &copyedit,
                CommandAvailability::default(),
                &recent,
                &collections,
            );
            cx.get_menus().expect("the menu bar is registered")
        });
        assert_eq!(
            menus
                .iter()
                .map(|menu| menu.name.to_string())
                .collect::<Vec<_>>(),
            [
                "File",
                "Edit",
                "View",
                "Writer",
                "Preview",
                "Help",
                "Astrolith"
            ]
        );
        let mut labels = Vec::new();
        for menu in &menus {
            for item in &menu.items {
                match item {
                    gpui_kit::OwnedMenuItem::Action { name, .. } => labels.push(name.clone()),
                    gpui_kit::OwnedMenuItem::Submenu(submenu) => {
                        for item in &submenu.items {
                            if let gpui_kit::OwnedMenuItem::Action { name, .. } = item {
                                labels.push(name.clone());
                            }
                        }
                    }
                    _ => {}
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
        assert!(labels.iter().any(|label| label == "Open site"));
        assert!(labels.iter().any(|label| label == "Open Collection: blog"));
        assert!(labels.iter().any(|label| label == "Open Collection: docs"));
    }

    #[test]
    fn generated_commands_share_labels_and_availability_rules() {
        assert_eq!(collection_command_label("blog"), "Open Collection: blog");
        let idle = CommandAvailability::default();
        for command in [CommandId::ReloadCollections, CommandId::OpenProjectInIde] {
            assert!(!command.is_available(idle));
        }
        let open = CommandAvailability {
            project_open: true,
            ..CommandAvailability::default()
        };
        assert!(CommandId::ReloadCollections.is_available(open));
        assert!(!CommandId::OpenProjectInIde.is_available(open));
        let configured = CommandAvailability {
            ide_configured: true,
            ..open
        };
        assert!(CommandId::OpenProjectInIde.is_available(configured));
        assert!(CommandId::OpenCollectionInIde.is_available(configured));
        assert!(!CommandId::OpenEntryInIde.is_available(configured));
        assert!(CommandId::OpenEntryInIde.is_available(CommandAvailability {
            active_entry: true,
            ..configured
        }));
        assert!(CommandId::About.is_available(idle));
        assert!(CommandId::OpenRecentProjects.is_available(idle));
    }

    #[test]
    fn entry_context_ide_commands_follow_shared_availability() {
        let empty = CommandAvailability::default();
        let project = CommandAvailability {
            project_open: true,
            active_entry: true,
            ..CommandAvailability::default()
        };
        let configured = CommandAvailability {
            ide_configured: true,
            ..project
        };
        for command in ENTRY_CONTEXT_IDE_COMMANDS {
            assert!(!command.is_available(empty));
            assert!(!command.is_available(project), "{}", command.label());
            assert!(command.is_available(configured), "{}", command.label());
        }
        assert!(CommandId::OpenEntryInIde.is_available(configured));
        assert!(
            !CommandId::OpenEntryInIde.is_available(CommandAvailability {
                active_entry: false,
                ..configured
            })
        );
    }

    #[test]
    fn help_surfaces_are_available_from_the_help_group() {
        for command in [
            CommandId::OpenUserGuide,
            CommandId::OpenKeyboardShortcuts,
            CommandId::OpenLocalData,
        ] {
            assert!(COMMANDS.contains(&command));
            assert_eq!(command_group(command), "Help");
            assert!(command.is_available(CommandAvailability::default()));
            assert!(command.shortcut().is_none());
        }
    }

    #[test]
    fn about_reports_the_crate_version_and_build_identity() {
        let text = about_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains(build_identity()));
    }

    #[test]
    fn preview_commands_follow_the_server_phase() {
        let attached = CommandAvailability {
            project_open: true,
            preview_attached: true,
            ..CommandAvailability::default()
        };
        assert!(CommandId::StartPreview.is_available(attached));
        assert!(CommandId::RestartPreview.is_available(attached));
        assert!(!CommandId::StopPreview.is_available(attached));
        assert!(!CommandId::OpenPreview.is_available(attached));
        assert!(!CommandId::CopyPreviewUrl.is_available(attached));

        let ready = CommandAvailability {
            preview_running: true,
            preview_ready: true,
            active_entry: true,
            ..attached
        };
        assert!(!CommandId::StartPreview.is_available(ready));
        assert!(CommandId::StopPreview.is_available(ready));
        assert!(CommandId::RestartPreview.is_available(ready));
        assert!(CommandId::OpenPreview.is_available(ready));
        assert!(CommandId::CopyPreviewUrl.is_available(ready));

        let no_entry = CommandAvailability {
            active_entry: false,
            ..ready
        };
        assert!(!CommandId::OpenPreview.is_available(no_entry));
        assert!(!CommandId::CopyPreviewUrl.is_available(no_entry));

        let stopped = CommandAvailability {
            preview_running: false,
            preview_ready: false,
            ..ready
        };
        assert!(!CommandId::CopyPreviewUrl.is_available(stopped));
        assert!(!CommandId::OpenPreview.is_available(stopped));
    }

    #[test]
    fn ctrl_enter_opens_the_preview() {
        let expected = if cfg!(target_os = "macos") {
            "cmd-enter"
        } else {
            "ctrl-enter"
        };
        assert_eq!(CommandId::OpenPreview.shortcut(), Some(expected));
    }
}
