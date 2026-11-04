# Keyboard shortcuts

These are the shortcuts the Linux build binds. Every entry is registered in the
same command registry used by menus, the palette and the toolbar, so a shortcut
shown here is the one the application dispatches. On macOS builds the same
registry substitutes `Cmd` for `Ctrl`; macOS is not a supported delivery target
yet.

## Files and application

| Command | Shortcut |
| --- | --- |
| Open file… | Ctrl+O |
| Open project… | Ctrl+Shift+O |
| New file | Ctrl+N |
| Save | Ctrl+S |
| Close file | Ctrl+W |
| Commands… | Ctrl+P |
| Preferences… | Ctrl+, |
| Quit Astrolith | Ctrl+Q |

## Workspace modes

| Command | Shortcut |
| --- | --- |
| Writing workspace | Ctrl+Shift+1 |
| Build workspace | Ctrl+Shift+2 |
| Preview workspace | Ctrl+Shift+3 |
| Cycle workspace mode | Ctrl+Shift+M |

Switching modes keeps the open document, its caret, selection and history.

## View

| Command | Shortcut |
| --- | --- |
| Toggle files | Ctrl+1 |
| Toggle inspector | Ctrl+2 |
| Toggle preview | Ctrl+Alt+P |
| Toggle terminal | Ctrl+` |
| Focus document | Ctrl+0 |
| Enter full screen | F11 |

Full screen is also available from the View menu and the command palette.

## Writer and search

| Command | Shortcut |
| --- | --- |
| Find… | Ctrl+F |
| Find and replace… | Ctrl+H |
| Find next | Ctrl+G |
| Find previous | Ctrl+Shift+G |
| Go to line… | Ctrl+Alt+G |
| Content Finder… | Ctrl+Shift+K |
| Select next occurrence | Ctrl+D |
| Add cursors at line ends | Ctrl+Shift+L |
| Focus mode | Ctrl+Shift+F |
| Typewriter mode | Ctrl+Shift+T |

Select next occurrence works from the active selection only. Placing cursors
with Ctrl-click, extending select-next across every user cursor, and applying
distinct replacement text per cursor in one undo step are not available with
the pinned editor API. See [Linux limits](guide.md#14-linux-limits-and-known-gaps).

## Formatting

| Command | Shortcut |
| --- | --- |
| Bold | Ctrl+B |
| Italic | Ctrl+I |
| Link… | Ctrl+K |
| Heading 1 | Ctrl+Alt+1 |
| Heading 2 | Ctrl+Alt+2 |
| Heading 3 | Ctrl+Alt+3 |
| Heading 4 | Ctrl+Alt+4 |
| Paragraph | Ctrl+Alt+0 |
| Toggle Markdown comment (`.md`) | Ctrl+/ |
| Insert MDX component… (`.mdx`) | Ctrl+/ |

Each formatting command is one undo step over visible text and refuses to edit
hidden frontmatter or imports. `Ctrl+/` is document-kind dependent: a comment
toggle in Markdown, the component builder in MDX.

## Preview

| Command | Shortcut |
| --- | --- |
| Open preview in browser | Ctrl+Enter |

## Not bound

The following commands are available from menus, the palette, the toolbar or
context menus but have no default key binding:

- Open recent projects, Reload collections
- Duplicate file, Rename or move file…, Move file to Trash…, Reveal in file
  manager, Copy absolute path, Copy project-relative path
- Recover unsaved changes…
- Open project/collection/file in IDE
- Start, stop and restart preview server; Copy preview URL
- Focus selection toolbar
- Light, dark and system theme; Reset layout
- About Astrolith; Check for updates… (when configured)
- Copyedit category toggles

Component-builder placeholders use `Tab`, `Shift+Tab` and `Escape`. Content
Finder uses `Enter` to open and `Ctrl+Enter` to insert a link.
