# Astrolith user guide

Astrolith is a native Linux editor for Astro content collections. It opens a local
Astro project, lists its collections and entries, edits Markdown, MDX and YAML
sources, validates and edits frontmatter from the project schema, and saves with
conflict detection and durable recovery.

This is an alpha build. Linux is the only delivery target; macOS and Windows
code paths exist but are not shipped or supported yet. See
[Linux limits and known gaps](#14-linux-limits-and-known-gaps) before relying on
a feature for production work.

Other pages:

- [Keyboard shortcuts](keyboard-shortcuts.md)
- [Privacy and local data](privacy.md)
- [Development and packaging](../technical/development.md)
- [Project README](../../README.md)

## 1. Install and first launch

Astrolith is built from this repository; there is no signed public download
channel yet. Packaging recipes exist for AppImage, `.deb` and `.rpm`, but no
artifact has completed a clean-environment install, launch, association,
protocol and uninstall run. Treat packaged builds as untested unless you ran
that check yourself.

Build from source:

```sh
mise install
mise run doctor
mise run build
mise run dev
```

The default build needs no native GTK/WebKit packages: Astrolith uses GPUI Kit
and opens previews in your normal browser. Only the optional, developer-only
`embedded-webview` build links WebKitGTK (X11/XWayland windows only) and needs
its development packages. A graphical Wayland or X11 session and a working
Vulkan driver are required. See the
[README](../../README.md#build-from-source) for the distribution packages.

At first launch Astrolith shows a welcome workspace. Use **Open project…**
(`Ctrl+Shift+O`), **File → Open project…**, or the command palette
(`Ctrl+P`) to choose an Astro project directory. Astrolith remembers recently
opened projects, reopens the last project on the next launch, and keeps one
window: a second launch forwards its arguments to the running window.

## 2. Projects and collections

Astrolith discovers content statically. It needs a working Astro project with
its dependencies installed; there is no separate project-creation wizard.
Configure collections exactly as you would for `astro dev`, then open the
project directory. Astrolith supports installed stable Astro majors 5, 6 and 7
and literal, local `glob()` collections over Markdown, MDX and YAML/YML data
sources. MDX patterns require the configured `@astrojs/mdx` integration whose
declared `astro` peer range includes the installed Astro release.

Discovery reads the Astro configuration; it does not execute project code.
Settings that cannot affect content discovery, such as adapters and
Markdown/Vite options, are ignored. Content-affecting settings that Astrolith
cannot prove statically (`root`, dynamic `srcDir`, legacy/experimental flags,
custom IDs, MDX options, imported schema constants) produce diagnostics instead
of a guessed index. Schema generation and helper resolution use the project's
own Astro tooling, so the project must have its dependencies installed.

YAML data entries are indexed and editable as raw text. They do not get the
Markdown frontmatter form, but they can be referenced from schema `reference()`
fields and used by the Content Finder.

Collections appear in the left sidebar with recursive entry counts. Nested
directories are browsable through breadcrumbs. **File → Reload collections**
re-scans the project without discarding open documents, selection, recovery
state or the preview route. Empty, loading and failed scans are reported
distinctly.

Astrolith refuses to open system and credential directories such as `/usr`,
`/etc`, `~/.ssh`, `~/.aws` and `~/.docker`. Such an open fails visibly, starts
no watcher and records nothing in the recent-project registry.

## 3. Entries

Create, duplicate, rename, move and delete entries from the file context menu,
the command palette, the application menus, or — where the
[keyboard reference](keyboard-shortcuts.md) lists one — a shortcut.

- **New file** (`Ctrl+N`) creates the resolved extension in the current
  collection or subdirectory with a collision-safe `YYYY-MM-DD.md` (or `.mdx`)
  name. When the collection schema has required fields with representable
  defaults, Astrolith writes minimal schema-valid frontmatter; otherwise it
  asks for the required values before writing anything.
- **Duplicate file** copies the entry bytes unchanged and names the copy
  `name-copy.ext`, then `name-copy-2.ext`, and so on.
- **Rename or move file…** validates the new path, refuses traversal, symlink
  escapes and collisions, never rewrites content, and retargets the open tab,
  index, watcher, recovery key and preview route. A dirty entry is saved
  through the guarded path first.
- **Move file to Trash…** uses the desktop trash; Astrolith has no
  permanent-delete fallback. A dirty entry must be saved or discarded first.
- **Reveal in file manager** opens the containing directory. Linux does not
  offer a verified portal for selecting the file itself, so the file is not
  highlighted.
- **Copy absolute path** and **Copy project-relative path** copy the entry
  location as text.

List rows show the mapped title, publication date, an MDX badge, a draft badge
and the filename. A missing title falls back to the filename. Counts refresh
after every create, duplicate, rename and trash.

## 4. Navigating, filtering and sorting

- The draft filter uses the mapped draft field and matches only the literal
  YAML boolean `true`.
- The text filter is case-insensitive over the filename and the mapped title.
- Sort modes are Default (publication date descending, undated entries first,
  title tiebreak), Title, Filename, Last Modified, every schema date field, and
  numeric `order` when present. Every mode except Default has a direction
  toggle. Ties keep a stable order.
- Filters, sort mode, direction and search text live for the current session
  only and reset when you switch projects. Pane layout, the last entry and the
  preview dock height persist per project.

**Content Finder** (`Ctrl+Shift+K`) searches every indexed entry by its mapped
title or Astro entry id. `Enter` opens the entry; `Ctrl+Enter` inserts a
Markdown link whose text is the title. The destination uses the collection URL
pattern with the frontmatter slug (percent-encoded) and falls back to the Astro
entry id. Without a pattern, Astrolith inserts a relative file link. Link text
and destinations escape brackets, parentheses and backslashes.

## 5. Preferences and three-tier overrides

Open **Preferences…** with `Ctrl+,` or **View → Preferences…**. The sheet has
six panes:

- **Appearance** — follow system, light or dark with one neutral palette,
  interface font family and size (8–28, default 18), heading colours,
  document font family, document font size (1–30, default 16), line height
  (1.2–3.0 × the font size, default 2.0) and page width (30–100% of the editor
  pane, default 64%).
- **Workspace** — default workspace mode, per-mode layout resets, Focus and
  Typewriter mode, autosave delay (1, 2, 5 or 10 seconds), default new-file
  extension, and the external editor command and arguments.
- **Preview** — external or embedded presentation, the preferred browser
  executable and arguments, and Open on Enter.
- **Project Settings** — content, assets and MDX component directories,
  absolute versus document-relative image paths, default extension override,
  title/date/description/draft field mappings, and the experimental
  MDX import-insertion setting. Project Settings exists only while a project is
  open.
- **Collections** — per-collection content and assets directory overrides,
  image path mode, default extension, the same four field mappings, and the
  Content Finder URL pattern.
- **Advanced** — application and preferences versions, the preferences folder,
  the documentation links and **Reset All**.

Values resolve in exactly this order: **collection override > project override
> global default**. An absent value inherits the next scope. Explicit
`false` and `0` are values, not absence. The MDX component directory is
project-only. Mapping values are validated against the collection schema kind,
directories must be project-relative, the IDE command accepts only
`{project}`, `{file}`, `{line}` and `{column}` placeholders, and the URL pattern
uses `{slug}`.

**Apply** validates every pane and replaces both scopes atomically. **Cancel**
and `Escape` discard the draft. Each reset control clears only its own value so
inheritance becomes visible again. Reset All deletes only preferences and the
project registry, never recovery snapshots, save-failure records or logs, and
then restarts.

Astrolith uses the system UI font until you name another installed family in
**Preferences → Interface font**; that family also sets interface headings.
The document uses the mock's sans face (Inter when installed, otherwise the
platform UI font); **Document font** names another installed family and falls
back to that sans face when it is missing. CJK text uses the platform's script
fallback because no bundled CJK face is shipped. **Interface size** controls
the UI scale and spacing. **Document font size** and **Line height** set the
page independently, and **Page width** sets the prose column as a percentage
of the editor pane (30–100%, default 64%), clamped to a readable 320–1200 px.
The column centres when the pane is wider. Focus and Typewriter toggles are
saved with these settings and restored on the next launch.

## 6. Writing

The workspace is a writing desk: the **Library** opens on the project's
collection index, and selecting a collection shows only that collection's
entries (the back arrow returns to the index). The page sits in the middle,
the frontmatter **Details** inspector floats above it, and the **Astro
preview** controls sit below when that panel is open. Both stay closed by
default so the page remains the primary surface. The title bar carries the
project, search and one view menu. The document rail carries the active
document, its collection, save state and save action; closing and switching
other open documents remain in its overflow menu.

The three modes present the same document differently. **Writing** is the
centred writing page described here, and it is what-you-see-is-what-you-get:
headings, bold and italic text, inline code, link text, lists, quotes, fenced
code and rules are rendered, while their Markdown punctuation is hidden.
Clicking anywhere on the page places the caret in the source at that position;
typing, selection, IME, undo and save continue to use the same retained editor.
Tabs, indentation, Setext headings, tables, HTML/MDX blocks and image
embeds keep their source form rather than being guessed into rich text.
**Build** turns the same retained editor into a source view with line numbers,
indent guides, folding and syntax highlighting, keeping the real file tabs,
project explorer and terminal. **Preview** replaces the editor with a
read-only rendering of the body, frontmatter and MDX imports excluded, like a
Markdown preview in a code editor; the Astro preview remains a separate bottom
panel opened from the toolbar **Panels** dropdown or its command. The
**Details** inspector exists only in Writing.

Astrolith registers one command registry and installs native application menus
from it, but the pinned Linux platform does not draw a menu bar: on Linux every
menu action is reachable from the toolbar buttons and dropdowns, the command
palette (`Ctrl+P`) and its keyboard shortcut. Paths written here as
**File → …**, **View → …**, **Writer → …** or **Help → …** describe the native
menu bar, which only macOS and Windows present.

Build also speaks the Astro toolchain's language servers when the project has
installed them: syntax highlighting for `.astro`, TypeScript/TSX,
JavaScript/JSX, JSON, CSS, HTML, Svelte, YAML, TOML and Markdown/MDX; inline
diagnostics; completions as you type or with `Ctrl+Space`; hover documentation;
`F12` (or the context menu) to go to a definition, including another project
file; and `Ctrl+.` for code actions. The application looks for servers in the
project's `node_modules/.bin` and on `PATH`; it never downloads them. Without a
server, the file still opens and highlights normally. Rename, workspace
symbols, signature help, a diagnostics panel and cross-file code-action edits
are not implemented; Markdown has no server.

Open **Details** with the sliders button at the right of the document rail or
`Ctrl+2` (`Cmd+2` on macOS). It is a non-modal popover: it never changes the
document's width or position. **Escape**, a click outside, or its close button
dismisses it and returns focus to the document. When a select or date picker
inside the inspector is open, the first Escape closes that control and the
next closes the inspector. The inspector is closed by default, is not part of
a saved layout, and closes when you switch workspace mode.

**New entry** creates a file in the collection you are viewing. Opening an
entry from another collection switches the Library to that collection, so the
list always matches the page beside it.

The writer shows the document body only. Leading frontmatter and, in MDX, the
leading contiguous ESM import/export prelude are hidden. The complete source is
retained and saved byte-identically while those hidden regions are untouched.
When Astrolith cannot safely delimit hidden text, it keeps the complete source
visible and tells you instead of hiding a region it cannot map.

Writer commands:

- **Find…** (`Ctrl+F`) and **Find and replace…** (`Ctrl+H`), with
  **Find next** (`Ctrl+G`), **Find previous** (`Ctrl+Shift+G`) and
  **Close find**. A replace run is one undo step.
- **Go to line…** (`Ctrl+Alt+G`) moves the caret to a one-based line.
- **Select next occurrence** (`Ctrl+D`) extends the active selection to the
  next matching occurrence. It works from the active selection only; it cannot
  extend every cursor.
- **Add cursors at line ends** (`Ctrl+Shift+L`) replaces the selection with one
  cursor at each line end of the main selection.
- **Focus mode** (`Ctrl+Shift+F`) dims every sentence except the cursor
  sentence.
- **Typewriter mode** (`Ctrl+Shift+T`) keeps the cursor line vertically
  centred while typing, navigating and clicking.
- The window chrome auto-hides after typing when the Library and Details are
  closed and returns on a top-edge hover. The selection toolbar appears for a
  non-empty selection and is keyboard reachable.
- The page is set in the neutral sans face at 16 px with 32 px lines and a
  64% page width (clamped to 320–1200 px) by default, without line numbers,
  indent guides, folding or automatic brackets. Font family, size, line height
  and page width are preferences; the page width follows the editor pane and
  the writing column no longer rescales its type with the pane.
  Markdown presentation includes dimmed syntax markers, tinted code spans and
  blocks, muted italic blockquotes, and uniform bold headings.
- Copyedit (`Writer` menu) highlights nouns, verbs, adjectives, adverbs or
  conjunctions using a deterministic offline English classifier. Code, link
  destinations, HTML/MDX tags, frontmatter and hidden imports are excluded.
  Each category has a colour and a show/hide label in the same preferences
  state; the default is all categories off.
- The document rail shows word and character counts when the window has room.

Formatting commands act on visible text, preserve line endings and indentation,
and are one undo step each: **Bold**, **Italic**, **Link…**,
**Heading 1–4**, **Paragraph**, and **Toggle Markdown comment**. In `.md`,
`Ctrl+/` toggles a Markdown comment. In `.mdx`, the same key opens the component
builder instead. The Link command edits the URL when the caret is inside an
existing link; an empty selection inserts `[text]()`.

Text-editor basics that come from the pinned editor rather than Astrolith
commands: undo/redo, select all, indentation, Markdown list continuation,
inline and fenced code, and blockquotes. `Tab` inserts a tab and keeps focus in
the writer; `Tab` and `Shift+Tab` navigate component-builder placeholders.

## 7. Assets, images and links

Drop files onto the writer. Astrolith copies them into the resolved assets
directory, sanitises the name to lower-case kebab case, strips unsafe
characters, prefixes the local date as `YYYY-MM-DD`, lower-cases the extension
and finds an atomic `-N` suffix when the name is taken. Traversal, symlink
escapes and files outside the project are refused.

Raster and SVG extensions insert image Markdown (`![name](path)`); PDFs and
other files insert a normal link (`[name](path)`). Paths are document-relative
by default or project-root absolute when that preference is enabled, so nested
documents survive save and reload. A failed copy is reported; Astrolith never
silently inserts a host path.

Paste a valid URL over selected text to wrap the selection as
`[selection](url)` in one undo step. `Alt`-click a validated `http` or `https`
URL to open it in your browser; Astrolith does not navigate in-app.

`Alt`-hover an image reference to see a preview anchored bottom-right. Remote
HTTP(S), project-root and document-relative sources are supported. Raster
previews are bounded in size and time and never escape the project root. The
built-in decoder handles BMP, GIF, JPEG, PNG and WebP; SVG, AVIF, TIFF and ICO
may insert correctly but show no preview. A late or superseded preview is
discarded.

The frontmatter image control copies an external image through the same asset
service, honours the image path mode, accepts a manually typed path verbatim
and can clear the value.

## 8. Frontmatter

The frontmatter panel is built from the collection schema plus validation
helpers:

- The mapped title comes first, then schema fields in declaration order, then
  non-schema fields alphabetically. With no schema, only existing fields
  appear.
- Controls are chosen from the schema kind: text, number, integer, toggle,
  date, enum, string arrays, email/URL hints, nested object groups, images,
  single and array references, and a validated raw-YAML control for containers
  Astrolith cannot represent safely.
- Defaults appear as hints and are prefilled only at creation. Opening and
  saving an existing file never materialises schema defaults.
- Descriptions, defaults and min/max/length/pattern hints render, and required
  fields are marked. Validation never silently coerces a value and never blocks
  the guarded save.
- The title control has **Copy as slug**, which slugifies the current title to
  the clipboard without touching the document or disk.
- Edits are lossless patches over the complete source: comments, quoting,
  unknown fields, CRLF, hidden imports and field order are preserved. Anchors,
  aliases, tags, complex keys and unsafe flow forms fall back to a visible raw
  text control with a validation message instead of being rewritten.
- Changing a mapping moves the list title/date/badges, filter, sort and panel
  together.
- Schema freshness: changes to the generated schema or content configuration
  reload the form without dropping dirty edits; stale results are dropped.
- File-based data collections over JSON feed reference dropdowns and never
  appear as editable collections.

## 9. MDX component builder

In an `.mdx` document, `Ctrl+/` opens the component builder. It indexes
`.astro`, `.tsx`, `.jsx`, `.vue` and `.svelte` components in the configured
directory without executing project code. Files whose stem is `index` are
skipped; sibling directories are still scanned.

The palette searches by name, lists project components before package groups,
shows the framework, required-prop summary and description, and lists duplicate
names separately. A Starlight catalog loads only when `@astrojs/starlight` is
declared as a dependency or dev-dependency.

The form shows a typed control per prop, pre-shows defaults, marks required
props, omits optional props unless enabled (with a toggle-all action), offers
client directives, and provides image and collection-entry pickers.
`Ctrl+Enter` inserts the generated tag; empty required props become tracked
placeholders navigated with `Tab`, `Shift+Tab` and `Escape`. Wrapping
components insert on their own lines with blank lines, and Starlight scaffolds
such as Tabs, CardGrid, Steps and FileTree are pre-filled.

When **Add imports for inserted components** is enabled (experimental, default
off), insertion keeps imports in complete-source state and the tag in the
writer projection. Existing or corroborated imports are reused; otherwise a
relative or package import is added without reformatting unrelated imports, and
named imports are merged into a compatible declaration. Entry props add an
entry-loader import and constant; image props add a sanitised identifier
import. `public/` assets are not imported. Undo removes the tag and
intentionally leaves the import, matching upstream. An `astro-auto-import`
dependency shows a reminder but never changes your setting.

## 10. Saving, conflicts and recovery

Astrolith autosaves dirty documents after 1, 2, 5 or 10 seconds of idle time
(default 2). Uninterrupted typing also checkpoints every 10 seconds. Saves are
atomic and conflict-aware; at most one save per document is in flight, and a
newer edit queues a follow-up save.

The status distinguishes **Unsaved**, **Saving…**, **Saved**, **Conflict** and
**Save failed**. Automatic saves are silent on success; a manual save reports
success. A clean buffer's `Ctrl+S` does nothing.

Dirty buffers also save on editor blur and on file switch through the same
guarded path. Switching projects flushes every dirty buffer; a conflict or
failure cancels the switch and keeps the project open. Closing or quitting
prompts to save, discard or keep editing, and a failed flush cancels the quit.

When the file changed on disk, the save is blocked with a visible **Conflict**
state and your buffer is never overwritten. Resolve the conflict by reopening
the entry or choosing your copy through the close prompt.

Every dirty edit also updates a recovery record. If Astrolith exits before a
successful save, **File → Recover unsaved changes…** lists pending records and
restores the chosen text into the editor. Recovery records are keyed by the
canonical source, survive restarts, and are cleared only after a successful
save.

A failed save writes one redacted local artifact per failure episode under the
save-failure directory. It contains the timestamp, error class, application
version, build identity and the project-relative source path. It never contains
buffer text, frontmatter values, snippets, environment variables or the full
project path. Artifacts are never transmitted, and nothing deletes them
automatically.

Exact locations are in [Privacy and local data](privacy.md).

## 11. Preview

**Preview mode** renders the active Markdown or MDX entry with a native
rich-text view. Frontmatter and MDX imports are not shown; headings, links,
lists, code blocks, tables, quotes, project-local images and absolute
`http`/`https` images are. The pane is
read-only and follows the document font, size, line height and page width
preferences. Typing in Preview does not modify the document. Other file types
show a short notice instead of a rendering.

The **Astro preview** is a separate bottom panel, not part of Preview mode. It
is an Astrolith addition; upstream Astro Editor does not ship one. On Linux the
only supported presentation is your **external default browser** unless the
embedded build runs on a supported display backend. Embedded preview is not
available on Wayland and is not built into the released dependency graph.

The Astro preview panel starts, stops, restarts and retries the project-local
Astro dev server. It shows `stopped`, `starting`, `ready`, `stopping` or
`failed`, and keeps the child's stderr excerpt for every spawn, exit, port and
timeout failure. **Copy preview URL** and **Open preview in browser** use the
same route resolution for the active entry; an unmapped entry reports that
instead of opening a URL. `Ctrl+Enter` runs the same command as the menu and
panel.

The preview route follows create, rename, trash, project switch and Content
Finder open. The panel remembers its open state and dock height per project.
Starting a preview for the same project root reuses the running server; a
different root replaces it. Restart the server after changing Astro
configuration or installing dependencies. The editor stays usable when the
server fails.

## 12. Desktop integration

- The URL scheme is `astrolith://open?path=<encoded path>`.
  Astrolith validates the scheme, the owning project root and content-entry
  membership before opening. Cold and warm launches both route through the
  project registry and the normal open path. This clean-room scheme differs
  from upstream's `astro-editor://`.
- Installed packages declare `text/markdown` and `text/mdx` handlers and the
  `x-scheme-handler/astrolith` scheme. Opening a `.md` or `.mdx` file from the
  desktop or a second launch routes through the same validator. This is a
  deliberate strengthening: upstream declares associations but ignores startup
  arguments.
- A second launch forwards its arguments to the single running window; no
  second window appears.
- **Open in IDE** commands expand a configured argv template with
  `{project}`, `{file}`, `{line}` and `{column}`. Arguments are never passed
  through a shell. The commands are available only when an IDE is configured.
- **Enter full screen** is available from the View menu and the command
  palette; `F11` is also bound on Linux as an explicit native decision.

File associations and the protocol handler take effect only after installing a
package, and those installs are not yet verified in a clean environment.

## 13. Help surfaces

The **Help** menu contains:

- **User guide** — this guide, bundled into the application.
- **Keyboard shortcuts** — the reference generated from the live command
  registry.
- **Privacy and local data** — the exact local directories Astrolith uses.
- **About Astrolith** — application version, build identity, operating system
  and architecture.
- **Check for updates…** — available only when the build was compiled with an
  update manifest URL and public key.

Preferences also links to the user guide and keyboard shortcuts. Nothing in
Help opens an external website.

## 14. Linux limits and known gaps

- **Preview:** external browser only on Wayland; embedded preview is not
  shipped.
- **Reveal:** opens the containing directory only; the file itself is not
  selected.
- **Clipboard assets:** the editor does not turn clipboard images or
  file-manager file lists into assets. Drop the file onto the writer, or use the
  frontmatter image picker. Pasting a URL over a selection still wraps the
  selection as a link.
- **Multicursor:** `Ctrl`-click cursor placement, selecting the next occurrence
  for every user cursor, and applying distinct text per range in one undo step
  are blocked by the pinned editor API. The `Alt`-click and line-end forms that
  the API exposes work.
- **Updates:** the Check for updates command is capability-disabled unless the
  build carries both an HTTPS manifest URL and an Ed25519 public key. No signed
  manifest has been published, and no packaged build has exercised a signed
  check.
- **Packaging:** AppImage, `.deb` and `.rpm` recipes exist. No artifact has
  passed clean-environment install, launch, association, protocol and uninstall
  checks. Icons are provisional, the AppStream homepage is a placeholder, and
  the repository has no license yet, so the RPM metadata says `UNLICENSED`.
- **Native evidence:** automated headless tests cover the logic and UI state,
  but some native interactions are not yet recorded for the current build:
  file dialogs, native menu rendering, drag and drop, image hover previews,
  external browser launch, single-instance forwarding, protocol cold/warm
  launch and packaged installs. Do not treat a successful build or headless
  test as proof of those behaviors.
- **Open file:** **Open file…** opens project sources with an editable text
  extension as editable documents. Files outside the project, binary and
  resource files open read-only (up to 8 MiB and 50,000 lines).

## 15. Getting help

Start with the in-app Help menu, then the
[keyboard reference](keyboard-shortcuts.md) and
[privacy and local data](privacy.md). Developer setup, test strategy,
architecture boundaries and packaging commands are documented in
[Development and packaging](../technical/development.md). The frozen parity
ledger and per-row final audit are in
[Astro Editor parity](../technical/astro-editor-parity.md).
