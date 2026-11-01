# Upstream parity ledger — Astro Editor (Linux)

Status: **P0 evidence, frozen.** This document records the shipped Linux end-user behavior of
upstream Astro Editor at a pinned commit, maps each behavior to one Astrolith acceptance scenario and
one owning implementation task, records intentional safety divergences, and closes the ownership of
every item that the source survey could not attribute to P1–P15.

It contains no upstream implementation code. Every description below is a behavioral statement
derived from the cited source, configuration or user documentation at the pinned commit; acceptance
scenarios are Astrolith-side, clean-room statements of observable behavior.

## 1. Baseline and method

| Item | Value |
|---|---|
| Upstream repository | `dannysmith/astro-editor` |
| Upstream commit | `9ec2343b0bd3f4424f92a20000231a66f1930a34` |
| Upstream shipped version | `1.0.20` (declared in `src-tauri/tauri.conf.json` at that commit) |
| Upstream platform scope read | Linux (plus cross-platform code paths explicitly noted as excluded) |
| Astrolith baseline | `5c90091d8be8de63d8d45b856e6d73eee28447df` |
| Feature-parity plan | [`2026-11-01-astro-editor-linux-feature-parity.md`](../superpowers/plans/2026-11-01-astro-editor-linux-feature-parity.md) (§2 ledger, §3 decisions, §4 tasks) |

Method:

1. The upstream tree was read **at the pinned commit only**. A local clone of the repository
   present on this workstation is a different fork/branch at version 1.0.17 and was **not** used as
   evidence for any row.
2. Inventory surfaces: the Rust command layer, the native menu tree, the TypeScript command
   registry and editor keymap, the stores/hooks that own behavioral policy, the React component tree
   (as a description of visible surfaces), Tauri configuration, and the user-facing documentation
   site shipped with the project.
3. Each discovered behavior was reduced to a single observable acceptance scenario targeted at an
   existing Astrolith module or a module created by a named plan task.
4. Behaviors were then partitioned into: (a) in-scope Linux parity, (b) intentional divergence, or
   (c) excluded (non-Linux platform, non-goal, or not end-user-facing).
5. P0's `fixtures/parity-project/` is the fixture every scenario in this document assumes. Its active
   configuration contains MD and MDX entries with imports, supported frontmatter field families,
   nested `image()`, local Astro/React/Vue/Svelte components, Starlight dependency metadata, nested
   collections, configured path aliases, assets, required fields, CRLF sources, Unicode, code and
   link content, duplicate slugs, and files intended to change externally. Companion oracle
   configurations under `oracles/` freeze later-task inputs that current discovery intentionally
   rejects, including JSON `file()` data collections and references targeting them.

## 2. Evidence-source classes

Rows cite evidence by class, not by transcription. Each class has a distinct reliability profile and
a distinct failure mode for this project:

| Class | What it is | Why it is admissible | Known limit |
|---|---|---|---|
| **U — shipped user documentation** | The documentation site bundled in the upstream repository (editor, file management, frontmatter, preferences, reference, privacy, installation, troubleshooting). | States the contract the upstream authors intend users to rely on. | Describes intent; may lag or omit behavior. Used to confirm, never alone to invent, a row. |
| **C — Rust command/config authority** | Command handlers, watcher, MDX/ESM and component scanners, menu construction, `tauri.conf.json` and `tauri.linux.conf.json`, capabilities file. | Owns filesystem, process, menu and packaging behavior; no UI can bypass it. | Some policy is enforced only in the UI layer; absence here is not absence of the behavior. |
| **R — command/shortcut registry** | The TypeScript application command registry, editor command registry, and keymap. | Enumerates user-invocable actions, labels, accelerators and availability rules. | Registry entries can be dead or conditionally registered; cross-checked against a consumer. |
| **S — store/hook policy** | Stores and hooks owning dirty state, autosave timing, switching, filters, sorting, external-change handling, URL/deep-link parsing, IDE launch guards, save failure handling. | Where timing and state transitions actually live. | Refactors can move policy; a row states the observable result only. |
| **V — visible surface** | React components describing panels, dialogs, forms, context menus, palettes and previews. | Establishes that a surface ships and what it exposes. | Describes presentation, not semantics; semantics are taken from C/R/S. |

A row is asserted as **shipped** only when at least one of C/R/S supports it and either U or V
corroborates that a user can reach it. Rows cite only the classes that actually establish them; a
row corroborated by V alone is presentation only and is not listed as behavior.

### 2.1 Evidence anchors

All paths are relative to the pinned upstream repository root. Each anchor was read at the pinned
commit; no anchor comes from the divergent local fork.

| Surface | Anchors |
|---|---|
| Application shell, menus, plugins, window | `src-tauri/src/lib.rs`; `src-tauri/tauri.conf.json`; `src-tauri/tauri.linux.conf.json`; `src-tauri/capabilities/default.json` |
| Rust command layer | `src-tauri/src/commands/{project,files,preferences,ide,updater,watcher,fonts,diagnostics,clipboard,menu}.rs` |
| MDX components | `src-tauri/src/mdx_components/{mod,prop_types,starlight}.rs`; `starlight-components.json` |
| MDX ESM imports | `src-tauri/src/mdx_esm/{block,imports,edit,plan}.rs` |
| Command registries | `src/lib/commands/app-commands.ts`; `src/lib/editor/commands/{CommandRegistry,editorCommands,insertSnippet}.ts` |
| Editor extensions and keymap | `src/lib/editor/extensions/{createExtensions,keymap,focus-mode,typewriter-mode,hanging-headers,syntax-mark-decorations,code-block-background,blockquote-style,copyedit-mode/*}.ts` |
| Editor formatting, selection, paste, URLs | `src/lib/editor/{markdown/formatting,markdown/headings,selection/addCursorsToLineEnds,paste,urls/*}.ts` |
| File list semantics | `src/lib/files/{sorting,filtering,search,fileProcessing,constants}.ts` |
| Content links | `src/lib/content-linker/link-builder.ts` |
| Settings model | `src/lib/project-registry/{persistence,defaults,effective-settings,collection-settings,migrations,default-file-type}.ts` |
| Platform services | `src/lib/{deep-link,ide,recovery,external-links,entry-id}.ts` |
| Session state | `src/store/{editorStore,projectStore,uiStore}.ts` |
| Behavioral hooks | `src/hooks/*` and `src/hooks/{editor,mutations,queries,settings}/*` |
| Visible surfaces | `src/components/**` (layout, toolbar, sidebar, frontmatter panel and field controls, component builder, preferences panes, update dialog, image preview, file context menu) |
| User contract | `website/src/content/docs/**` (editor, file management, frontmatter, preferences, reference, privacy, installation, troubleshooting) |

Astrolith-side scenario targets are the existing `crates/astrolith-{core,document,astro,workspace,gpui}`
modules named by plan §3 and §4, plus the modules those tasks create. No scenario targets an
upstream module boundary.

## 3. How to read the ledger

- Every row is one acceptance scenario with one owner. Owners are plan tasks P1–P15.
- `P#/P#` means a primary owner plus a named secondary obligation; the primary owner closes the row.
- Rows describe **behavior**, not appearance. Astrolith keeps its native four-panel shell and its
  own design tokens; upstream branding, the CodeMirror surface, the iA Writer typography and the
  React/Tauri structure are not reproduced.
- Editor-command capability status (search, replacement, cursor creation, geometry, decorations)
  is **not** asserted in this document. The single authority for whether a specific editor command
  is proven, degraded or unavailable on the pinned GPUI Kit facade is
  [`docs/technical/editor-capabilities.md`](editor-capabilities.md), produced by the P0 pinned-facade
  probe. This ledger fixes the *scenario*; the capability document fixes the *status*.
- Preview and the recent-projects browser are **not** parity rows in the strict sense — see §17 6.9 and §18.

## 4. Ledger — projects

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 1 | Open a project folder from a dialog, from the system tray, from `Ctrl+Shift+O`, and from the palette command `open-project` (C, R). | One project-open path reachable from dialog, palette and accelerator; it opens, scans collections and restores last pane state; all entry points dispatch the same command ID (P1/P12). |
| 2 | Refuse system and secret directories with a visible error (C; capabilities deny list). | Opening `/usr`, `/etc`, `~/.ssh`, `~/.aws`, `~/.docker` or equivalents shows a blocking error, starts no watcher and records nothing in the registry (P12; see §7). |
| 3 | Registry records every opened project with an identity derived from the package name plus a collision hash, its canonical path and last-opened time (C/S, U). | Two same-named fixture projects remain distinct; a moved folder rebinds on next open; registry writes are atomic and preserve unknown envelope fields (P1). |
| 4 | Last-opened project auto-loads on launch (S). | Restart reopens the last project with its settings and layout; a deep link at startup takes precedence (P1/P12). |
| 5 | Project scan parses the Astro config, falls back to a directory scan, and merges generated collection JSON schema with validation helpers (C). | Fixture with aliases, nested collections, unsupported config and a missing generated-schema directory still lists collections; diagnostics are surfaced, never a silent fallback (P3; existing discovery module). |
| 6 | `Reload Collections` command re-scans the project and reports completion (R). | Re-scan preserves selection, tabs, recovery keys and preview route, and reports completion (P12). |
| 7 | Switching projects or files flushes the outgoing dirty buffer (S). | Switching project with a dirty buffer saves through the guarded path or reports the failure; no silent loss (P6). |

## 5. Ledger — entries and file list

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 8 | Collections list shows recursively counted file totals (V/C). | Sidebar shows collection counts and refreshes after create, duplicate, rename and trash (P2/P12). |
| 9 | File rows show mapped title, publication date, an MDX badge, a draft badge and the filename (U/V). | Rows show mapped title and date plus both badges; a missing title falls back to the filename (P2/P12). |
| 10 | Subdirectory navigation with breadcrumb and a back action (V/S). | Nested browse, create and trash work at the collection root and inside subfolders; breadcrumb restores depth (P2/P12). |
| 11 | New file via `Ctrl+N`, toolbar and palette, created in the current collection or subdirectory, default name `YYYY-MM-DD.md[x]` with `-N` deduplication (S). | Create writes the resolved extension into the selected directory with a collision-safe date name and opens the result (P2). |
| 12 | New-file frontmatter: title `New <Singular>`, today for date-shaped fields, schema defaults for required fields, quoted YAML scalars (S). | Creation writes only schema-required values that have representable defaults; otherwise creation fields are shown before anything is written (P2). |
| 13 | Default extension resolves global → project → collection (S). | A collection overriding global `.md` to `.mdx` creates `.mdx` there; absence inherits the next scope (P1/P2). |
| 14 | Duplicate copies bytes and names the copy `name-1.ext` (V). | Duplicate preserves bytes; naming follows plan §3.3 (`name-copy`, `name-copy-2`) — recorded divergence (P2, §17 6.4). |
| 15 | Rename inline, updates the current file path, does not rewrite content (V/S). | Rename/move rejects traversal, symlink escape and collision; saves dirty sessions first; retargets tab, index, recovery key, preview route and watcher (P2). |
| 16 | Delete after a single confirmation dialog, then permanent removal (V/S). | Delete moves to the desktop trash, requires Save or Discard when dirty, and reconciles tabs, index, recovery and preview (P2, §17 6.3). |
| 17 | Reveal opens the containing directory; copy-path copies the absolute path (V/C). | Reveal opens the directory; copy path offers absolute and project-relative forms (P2/P12). |
| 18 | Draft filter toggle, per collection, matching literal `draft === true` only (V/S). | Draft-only filter uses the mapped draft field and only literal `true`; the header indicates an active filter (P2). |
| 19 | Case-insensitive filter over filename and title (S). | Filter is case-insensitive over filename and mapped title (P2). |
| 20 | Sort modes: Default (date descending, undated first, title tiebreak), Title, Filename, Last Modified, every schema date field, numeric `order`; direction toggle for all but Default (S). | Each mode and direction reproduced with stable keys; Default keeps undated-first ordering (P2). |
| 21 | Distinct loading and error states for the directory scan (V). | Empty, loading and error states are visually distinct and recoverable (P12). |

## 6. Ledger — navigation and chrome

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 22 | Command palette on `Ctrl+P`, fuzzy matching, grouped results, availability-filtered (R/V). | Palette lists exactly the command IDs, labels and shortcuts used by menus, shortcuts and context menus, hiding unavailable commands (P12). |
| 23 | Toggle sidebar `Ctrl+1`, frontmatter panel `Ctrl+2`, focus editor `Ctrl+0` (R). | All three dispatch registry commands and update menu check state (P12). |
| 24 | One `Open Collection: <name>` command per collection, unavailable for the active collection (R). | Collection-switch commands are generated from the live index and filtered identically in palette and menu (P12). |
| 25 | Toolbar: sidebar toggle, project name, new file, focus mode, typewriter mode, save, frontmatter toggle (V). | Toolbar controls mirror command IDs and disabled states (P5/P12). |
| 26 | Toolbar auto-hides after roughly four typed characters when both sidebars are closed; a top-edge hover restores it (V/S). | Chrome hides only when both panels are closed, restores on top-edge hover, and respects reduced motion (P5). |
| 27 | Window geometry persisted and restored, minimum 1000×700 (C). | Restart restores size and position; minimum size enforced (existing shell; P11 smoke). |

## 7. Ledger — writer surface

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 28 | The writer shows the body only; leading frontmatter and the leading MDX ESM import prelude are hidden (C/U). | Writer shows the projection; complete source is retained and saved byte-identically when hidden regions are untouched (P4). |
| 29 | Prose typography with responsive size and line height, dark adaptation, hanging heading marks when the pane is wide (V/S). | Typography scales with pane width; heading markers hang where geometry permits (P5). |
| 30 | Configurable heading colour per theme and a base editor font size of 1–30 (default 18) (V/S/U). | Changes apply live and after restart; reset restores defaults (P1/P11). |
| 31 | Markdown presentation: dimmed syntax markers, monospace code spans and blocks with background, grey italic blockquotes, uniform heading rendering, no token colouring inside code (V/S). | Same semantics through native decorations (P5). |
| 32 | Bold, italic, link, headings 1–4, paragraph, and `Mod+/` comment toggle in `.md` (R/S/U). | Each transform is one undo step over visible ranges and refuses hidden boundaries; `Mod+/` toggles a Markdown comment in `.md` only (P5; see §18, comment toggle). |
| 33 | Link command edits the URL when the caret is inside an existing link; an empty selection inserts `[text]()` (S). | Both behaviors reproduced; caret lands in the URL or the anchor as appropriate (P5). |
| 34 | Find/replace panel `Ctrl+F`, `Enter`/`Shift+Enter`, next/previous `Ctrl+G`/`Ctrl+Shift+G`, `Esc` closes (R). | Native find/replace with next/previous and close; a replacement run is one undo step (P5). |
| 35 | Go to line `Ctrl+Alt+G` (R). | Command and shortcut open a line prompt and move the caret (P5). |
| 36 | Select next occurrence `Ctrl+D`, add cursors to selected line ends `Ctrl+Shift+L`, add cursor by modified click (R/S). | Scenarios frozen: repeated occurrence selection, line-end cursors, modified-click cursors, and batch replacement across all cursors. Shipping status is set only by `editor-capabilities.md`; this ledger asserts neither Proven nor unavailable (P5). |
| 37 | Indent/dedent, list continuation, select all, undo/redo, `Tab` inserts a tab and traps focus, `Tab`/`Shift+Tab` move between snippet fields (R). | Native editor behaviors retained; snippet-field navigation works for builder insertions (P5/P10). |
| 38 | Focus mode `Ctrl+Shift+F` (and an eye control) dims everything except the cursor sentence (R/V/S). | Only the cursor sentence stays undimmed across cursor moves, Unicode and CRLF (P5). |
| 39 | Typewriter mode `Ctrl+Shift+T` keeps the cursor line vertically centred, including after pointer clicks (R/S). | Cursor line stays centred while typing, navigating and clicking, without selection drift (P5). |
| 40 | Pasting a valid URL over a selection wraps the selection as a link (S/U). | Paste-over-selection yields `[selection](url)` as one undo step (P5). |
| 41 | Modified-click on an http(s) URL opens the browser; modified hover decorates URLs (S). | Modified-click opens only validated `http`/`https` externally; no in-app navigation (P5). |
| 42 | Dropping a file on the editor copies it into assets and inserts Markdown; on failure it falls back to the original path (S). | Drop copies into the resolved assets directory, inserts image or link Markdown, and surfaces failure instead of inserting a raw path (P8). |
| 43 | Modified-hover image preview anchored bottom-right, resolving remote http(s) and project-relative paths, failing silently (V/S). | Preview loads off the render path with size and time limits, anchors bottom-right, dismisses on modifier release, and never escapes the project root (P8). |
| 44 | Save on editor blur, save on file switch, `Ctrl+S` that no-ops on a clean buffer (S). | Dirty buffers save on blur and on switch through the coordinator, and a blur or switch during an in-flight save queues as its follow-up; `Ctrl+S` on a clean buffer does nothing (P6; see §18, save on blur). |
| 45 | Project name and modified state visible in the toolbar/window chrome (V). | Project name and dirty indicator are visible without opening preferences (P5/P12). |

## 8. Ledger — assets, links, frontmatter images

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 46 | Asset copy: lower-case kebab, unsafe characters stripped, `YYYY-MM-DD-` date prefix, lower-cased extension, atomic `-N` collision suffix with a bounded retry count, containment check (C). | Same naming and collision semantics executed atomically; traversal and symlink escapes refused (P8). |
| 47 | Images insert `![name](path)`; other files insert `[name](path)`; a fixed raster/SVG extension set decides which (S). | Raster/SVG drop inserts image Markdown, PDFs and other files insert links (P8). |
| 48 | Paths are document-relative by default and project-root absolute when enabled (C/S). | Both modes emit correct paths for nested documents, surviving save and reload (P8). |
| 49 | Content Finder `Ctrl+Shift+K` searches all entries; `Enter` opens; `Ctrl+Enter` inserts a link whose text is the title (R/U). | Search over the configured title/id; Enter opens, `Ctrl+Enter` inserts a title-text link (P8). |
| 50 | Default link is the relative file path; a collection URL pattern replaces `{slug}` from frontmatter slug, else the Astro entry id; closing brackets and parentheses are escaped (S). | Pattern and fallback reproduced with URL-encoded slug per plan §3.7; escaping preserved (P8). |
| 51 | Image frontmatter field: thumbnail, pick/drop copies and renames, manual path edit used verbatim, clear action (V/U). | Image control copies external files through the asset service, accepts an unvalidated manual path, and clears cleanly (P3/P8). |
| 52 | An `image()` nested inside an object degrades to a text control (U — documented limitation). | Nested image renders a real image control (P3, §17 6.10: intentional improvement, not a regression). |

## 9. Ledger — MDX component builder

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 53 | `Ctrl+/` opens the builder in `.mdx` and toggles a comment in `.md` (S/R). | Shortcut opens the builder only for MDX; `.md` keeps the comment toggle (P10, with the `.md` path owned by P5). |
| 54 | Discovery of `.astro`, `.tsx`, `.jsx`, `.vue`, `.svelte` recursively in the configured directory, symlink-following, `index.*` skipped, containment check, no code execution (C). | Index lists the same frameworks from the resolved directory without executing project code; `index.*` files are skipped per §18, index files (P9). |
| 55 | Prop extraction for Astro props, React parameter/`FC<Props>`, Vue `defineProps`/`withDefaults`, Svelte `export let`/`$props`, including relative type imports; unknown types become strings; a parse failure still lists the component with no props (C/U). | Frozen fixtures produce identical prop models: required/optional, defaults, doc comments, unions/enums, number, boolean, date, arrays, `ImageMetadata`, `CollectionEntry`, client directives, slots (P9). |
| 56 | Palette: fuzzy search by name, project components first then package groups, framework badge, required-prop summary, description (V). | Searchable, framework-grouped palette with metadata and help text (P10). |
| 57 | Form: typed control per prop, defaults pre-shown, required marker, optional props omitted, a toggle-all-optional shortcut, back to list, `Esc` back, tag preview, `Ctrl+Enter` insert, empty required prop receives the caret (V/S). | Typed form with the same keyboard contract and tracked placeholder navigation (`Tab`/`Shift+Tab`/`Esc`) (P10). |
| 58 | Slot-bearing components insert on their own lines with blank lines; Starlight wrappers pre-fill children (`Tabs` with two `TabItem`, `CardGrid`, `Steps`, `FileTree`) (V/U). | Wrapping components insert multi-line scaffolds with tracked placeholders (P10). |
| 59 | Starlight catalog available when `@astrojs/starlight` is a dependency or dev-dependency; duplicate component names listed separately (C). | Catalog loads only when declared; project and package duplicates both listed (P9). |
| 60 | Import planning (per-project opt-in, default off): no import when the name is already bound; package imports for package components; reuse a corroborated import from the current collection then other collections; otherwise a relative path; named-import merge; original quote, semicolon and multiline style preserved; entry props add an entry-loader import and a constant; image props add an import with a sanitized identifier; `public/` assets not imported; non-capitalised filenames get no import (C/U). | Generated tags and import plans match frozen complete-source bytes, never reformat unrelated imports, revalidate at insert time, and undo removes the tag while intentionally leaving the import (P9/P10). |
| 61 | An `astro-auto-import` dependency shows a reminder but never enables the setting (U/V). | A warning appears without mutating the user's setting (P10/P11). |

## 10. Ledger — save, conflict, recovery, external changes

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 62 | Idle autosave defaulting to 2 s with 1/2/5/10 s options, plus a 10 s maximum interval during continuous typing (S/V). | Delay policy exact; continuous typing still checkpoints at 10 s (P6). |
| 63 | Manual save with a success toast; failure toast that mentions recovery; the file stays dirty; a recovery copy and a crash report are written (S/C/U). | Status distinguishes Unsaved, Saving…, Saved, Conflict and Save failed; automatic saves are silent on success; a failure keeps the buffer dirty and surfaces the error (P6; crash artifact P6/P15 per §18, crash-report artifact). |
| 64 | Untouched frontmatter is written through verbatim; once frontmatter is edited the whole block is regenerated from the object in schema order, then the body and imports are stitched back (C/U). | Body-only edits preserve frontmatter bytes; frontmatter edits are lossless patches preserving comments, quoting, unknown fields and CRLF (P3/P4, §17 6.2). |
| 65 | No conflict detection and no atomicity: a plain whole-file write (C). | An external change during save blocks with a visible Conflict state; saving is atomic and never clobbers (P6, §17 6.1). |
| 66 | Watcher with a short debounce, separate content and schema channels, a multi-minute rescan safety net, and automatic restart when the channel dies (C). | Index and watcher batches reconcile transactionally and recover from watcher death (P2/P4). |
| 67 | External-change reload is ignored while dirty and for a few seconds after the app's own save; otherwise the content is invalidated (S). | A dirty buffer wins; a clean buffer reconciles preview, tab and selection after an external edit (P4/P13). |
| 68 | No quit-time flush beyond autosave (C). | Close and quit prompt under the dirty contract, and the quit prompt's Save arm flushes every dirty buffer through the coordinator before the window closes (a conflict or failure cancels the quit); project switches flush before opening, and a rename re-subscribes the retargeted session (P6). |

## 11. Ledger — frontmatter form

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 69 | Form built from the collection schema plus validation helpers: the title field first, other schema fields in declaration order, non-schema fields alphabetically afterwards; with no schema only existing fields appear (V/U). | Exact ordering, with unknown fields preserved after schema fields (P3). |
| 70 | Defaults are shown as hints and prefilled only at creation, never injected into existing files (V/U). | Opening and saving an existing file never materialises schema defaults (P3). |
| 71 | Controls: text, number, integer, toggle, date picker, enum dropdown, string-array tags, email/URL hints, nested object groups (V/U). | One control per family, driven by the schema field kind (P3). |
| 72 | Image control; single and array reference controls labelled from title/name/slug else id; arrays of non-text values fall back to a raw YAML editor (V/U). | Reference pickers cover single and array including data collections; unsupported arrays use a validated raw-YAML control (P3). |
| 73 | Description text, defaults, min/max/length/pattern hints, required marker; validation never blocks saving (V/U). | Constraints and descriptions render; validation never silently coerces or blocks the guarded save (P3). |
| 74 | Data collections declared with a file-based loader over JSON, with required id/slug, read-only and absent from the sidebar (C/U). | File-based collections feed reference dropdowns and never appear as editable collections (P3). |
| 75 | Schema freshness: changes to the generated collection schema directory or the content config files invalidate and regenerate the form (C). | Schema regeneration refreshes the form without losing dirty edits; stale results are dropped (P3). |
| 76 | The title field is a larger auto-growing control at the top of the panel with a **copy as slug** affordance (V/U). | The mapped title renders first with a copy-as-slug affordance (P3; see §18, copy as slug). |
| 77 | Mapped description renders as a tall textarea; mapped draft drives badge and filter; mapped date drives list and sort (U). | A mapping change moves list, sort, filter and panel together (P1 resolution, P2 list, P3 panel). |
| 78 | Unsupported or ambiguous YAML values fall back to raw text controls instead of being coerced (U). | Lossless refusal retained for anchors, aliases, tags and complex keys, with a visible validation message (existing parsed-document layer plus P3). |

## 12. Ledger — copyedit

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 79 | Five categories (nouns, verbs, adjectives, adverbs, conjunctions) with individual toggles and an all-toggle, each category colour-coded and each label flipping between show and hide (R/U). | Five toggles plus an all-toggle with semantic colours and labels derived from the same state (P7). |
| 80 | Default inconsistency: persisted defaults are all off, while command labels and the all-toggle treat an absent value as on (S/R). | Astrolith defines exactly one persisted default and one label source; a test asserts settings and command labels cannot diverge (P7). |
| 81 | Debounced analysis, decorations cleared during active editing and re-applied after idle, the token under the cursor suppressed, decorations mapped through edits (S). | Revisioned background analysis, stale results dropped, the active token left undecorated over a small byte window, no overlapping decorations (P7). |
| 82 | No exclusion of code, links or frontmatter: the whole body is analysed by a third-party NLP library (S). | Astrolith excludes fenced/inline code, link destinations, HTML/MDX tags, frontmatter and hidden imports (P7, §17 6.8). |

## 13. Ledger — preview

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 83 | **Not shipped.** Upstream documents that there is no way to preview writing (U). | Retained Astrolith preview is made lifecycle-correct: it retargets after create, rename, trash, switch and Content-Finder open, persists pane width per project, and recovers from an occupied port (P13). This is an additive Astrolith strength, not an upstream parity row (§17 6.9, §18). |

## 14. Ledger — preferences

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 84 | One dialog with General, Project Settings, Collections and Advanced panes; project and collection panes only while a project is open; `Ctrl+,` (V/U). | One preferences surface with exactly these scopes (P11). |
| 85 | General: IDE command, theme light/dark/system, default new-file type, heading colours per theme with reset, editor font size 1–30 default 18 with reset, autosave delay (V). | Every field validates and applies atomically; Cancel mutates nothing; reset removes only that value (P11). |
| 86 | Project: content, assets and MDX directories; absolute image paths; default extension inherit/override; add-imports experimental setting with an auto-import reminder (V/U). | Three-tier inheritance with explicit false/zero treated as set; the MDX directory is project-only (P1/P11). |
| 87 | Collection: content and assets directory overrides, absolute image paths, default extension, four field mappings validated against schema kinds, link URL pattern (V/U). | Collection overrides resolve above project above global; mapping validity checked against schema kinds (P1/P11). |
| 88 | Advanced: app and preferences versions, open preferences folder through the desktop opener, Reset All that deletes the whole application-data directory and restarts (V/C/U). | Advanced shows versions and opens the preferences folder; Reset All requires object-specific confirmation, deletes only preferences and registry (never recovery or logs), then restarts (P11, §17 6.7). |
| 89 | Settings files: global settings, a project registry, and per-project files; a one-time v1→v2 migration; invalid JSON silently falls back to defaults (S). | Atomic stores, preserved unknown envelope fields, a single migration retaining the old file under a migrated name, and corrupt data quarantined and surfaced in Preferences (P1). |
| 90 | Theme `light/dark/system` with system as the resolved default (S/V). | Theme resolves before first paint and persists across restart (P1/P11). |
| 91 | Linux UI font detection from the desktop environment's font settings (C). | Astrolith renders with its own tokens; whether the system UI font is adopted is a capability decision recorded by P11 (§18, UI font detection). |
| 92 | Collection, sort, search and draft view state is per session only, not persisted (S). | Resolution: filters, sort and search are transient per session and reset on project switch; pane layout, last entry and preview width persist per project. Acceptance in P1 (persistence) and P12 (controls). |
| 107 | Window state restore and minimum size (C). | Same behavior via native shell persistence (P11/P12). |

## 15. Ledger — desktop and platform

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 93 | Native menu: File (open project, new file, save, close), Edit (undo/redo/cut/copy/paste/select all plus bold, italic, link, headings 1–4, paragraph), View (toggle sidebar, toggle frontmatter, enter full screen), Help (user guide, keyboard shortcuts), and an application menu (About, check for updates, preferences, quit) (C). | Menu and palette items share command IDs, labels and shortcuts; no menu-only or palette-only action (P12). |
| 94 | Formatting menu items start disabled and enable when the editor is focused (C). | Formatting availability follows editor focus, never stale state (P5/P12). |
| 95 | Menu events are emitted to the UI layer and dispatched there (C). | Menu dispatch goes through the command registry only (P12). |
| 96 | Fullscreen has a menu accelerator; F11 is bound only on other platforms (C/R). | Linux fullscreen reachable from menu and command; F11 also bound on Linux as an explicit native decision, documented (P12). |
| 97 | Single instance: a second launch focuses or unminimises the running window and forwards its arguments (C). | A cold or warm deep link focuses one window; no second window appears (P12). |
| 98 | File associations for `.md`/`.mdx` are declared as an editor, but startup arguments are not routed to opening the file (C). | MIME handlers open the file through one validator (P14), and startup-file routing is implemented — deliberately stronger than upstream (§17 6.11). |
| 99 | `astro-editor://open?path=` parsing with protocol and host checks and an owning-project match by longest ancestor, case-insensitive, waiting for a project switch before reusing the normal open path (C/S/U). | `astrolith://open?path=` validates scheme, project root and content-entry membership and routes both cold and warm launches (P12; registration in P14). |
| 100 | IDE command is a single executable with no arguments and no shell, a leading-home shorthand expanded, an augmented PATH, unsafe-character and UNC rejection, and argument-less process spawn (C). | IDE action is argv-based with `{project}`, `{file}`, `{line}`, `{column}` placeholders, no shell interpolation, and rejects untrusted paths (P12). |
| 101 | IDE commands for project, collection and file plus a context-menu item, only when an IDE is configured (R/V). | The same three commands gated on a valid configured IDE (P12). |
| 102 | Reveal opens the containing directory through the desktop opener (V/C). | Reveal opens the directory; selecting the file itself only where a verified Linux portal exists (P12). |
| 103 | Native right-click file menu positioned at the pointer (V). | Context menus at the pointer with identical items and availability (P12). |
| 104 | OS drag-and-drop into the editor is enabled window-wide (C). | Editor drop feeds the asset service; shell drop accepts exactly one directory to open a project or one in-project Markdown/MDX entry, and rejects mixed or multiple drops visibly (P8/P12). |
| 105 | Packaging targets AppImage, deb and rpm with updater artifacts (C/U). | AppImage, deb and rpm built from one metadata source with MIME and scheme registered (P14). |
| 106 | Update check shortly after launch and from the menu; skip-version; a dialog with release notes fetched from the host API; in-app download, install and relaunch (S/V/C). | Signed HTTPS manifest verified with an Ed25519 signature plus a SHA-256 artifact digest, then an external release download; no self-replacement; the UI stays capability-disabled until a manifest URL and public key exist (P14, §17 6.6). |

## 16. Ledger — help, logs, privacy

| # | Upstream behavior (evidence) | Astrolith acceptance scenario (owner) |
|---|---|---|
| 108 | Help menu opens the user guide and keyboard-shortcut documentation in the browser (R/C). | Help surfaces open Astrolith documentation (P12/P15). |
| 109 | Documentation links inside preference panes (V). | Same inline links (P11/P15). |
| 110 | Rolling file log with a size cap keeping two files, written to stdout and a per-application log directory under the user data directory (C/U). | Logs at a documented path with size rotation and no content leakage (existing observability; P15). |
| 111 | Launch telemetry: a persisted anonymous identifier plus version, platform and timestamp posted to a hosted endpoint, with no opt-out (C/U). | Telemetry is off and local-only, with an opt-in state and no endpoint, identifier or queue (P1 state, P15 docs, §17 6.5). |
| 112 | Recovery copies are written only when a save throws and are never restored in the app (C/U). | Durable recovery with in-app restore and conflict-safe keys (existing recovery store; P6/P15). |
| 113 | Save failures also write crash-report dumps to a dedicated directory (S/U). | Saved artifacts follow §18, crash-report artifact under a strict content-redaction constraint (P6/P15). |
| 114 | About dialog showing application version and platform (C). | About surface showing version and build identity, reachable from command palette and menu (P12 / §18, About surface). |
| 115 | Privacy documentation states the app is offline apart from two launch requests (telemetry and the update check) (U). | Astrolith privacy documentation states no network telemetry and update checks limited to the signed manifest (P15). |

## 17. Intentional divergences

Each divergence is a deliberate safety or correctness choice, is cited by the row that encodes it,
and must be covered by a test that would fail if the behavior silently regressed to upstream's.

| # | Divergence | Upstream | Astrolith | Rows |
|---|---|---|---|---|
| 6.1 | Save integrity | Plain whole-file write; no conflict state. | Atomic write; external change blocks save with a visible Conflict state; failure is visible. | 63, 65 |
| 6.2 | Frontmatter preservation | First frontmatter edit regenerates the block, dropping comments and reordering. | Lossless patches preserving comments, quoting, unknown fields, CRLF and hidden imports; refusal instead of coercion. | 64, 69, 78 |
| 6.3 | Deletion | Permanent removal after one confirmation. | Desktop trash with a Save/Discard guard; no permanent-delete fallback. | 16 |
| 6.4 | Duplicate naming | `name-1.ext`. | `name-copy.ext`, then `name-copy-2.ext`. | 14 |
| 6.5 | Telemetry | Unconditional launch POST with a persisted identifier. | Off; local opt-in state only; no endpoint, identifier or queue. | 111 |
| 6.6 | Updates | In-app download, install and relaunch. | Signed manifest verification, then an external download; no self-replacement. | 106 |
| 6.7 | Reset All | Deletes the whole application-data directory. | Deletes only preferences and registry, never recovery or logs, after object-specific confirmation. | 88 |
| 6.8 | Copyedit scope | Whole-body analysis with no exclusions; only the cursor token suppressed. | Code, links, tags, frontmatter and hidden imports excluded; a small window around the cursor suppressed. | 82 |
| 6.9 | Preview | Absent by design. | Retained and lifecycle-integrated. | 83 |
| 6.10 | Nested image control | Degrades to text. | Renders a real image control. | 52 |
| 6.11 | Startup file routing | Associations declared but arguments ignored. | Startup file and scheme arguments routed through one validator. | 98, 99 |

## 18. Ownership resolution — formerly unattributed behavior

Every item the survey could not attribute to P1–P15 is closed here. Where the plan's task text did
not already name the item, the task's acceptance scenario is extended by this document; the entry
below is the authority.

| Item | Evidence | Owner | Required acceptance |
|---|---|---|---|
| Save on editor blur | Save-on-blur hook | **P6** | Blur of a dirty buffer schedules an immediate guarded save through the coordinator (not the idle deadline); a conflict on blur never overwrites and leaves a visible Conflict state; a failure keeps the buffer dirty. |
| Project blocked-directory policy | Project command's blocked-directory predicate; capabilities deny list | **P12** (with a P1 registry guard) | Opening a system or secret directory (e.g. `/usr`, `/etc`, home credential directories) fails with a visible error before any watcher or scan starts, and P1 must not record a rejected path in the registry. |
| Markdown comment toggle (`Mod+/` in `.md`) | Editor keymap and editor fallback | **P5** | In `.md`, `Mod+/` toggles a Markdown comment over visible ranges as one undo step and refuses edits across hidden boundaries; in `.mdx` the same shortcut opens the component builder (P10), so the binding is document-kind dependent. Ledger-driven addition to P5's transform set; record it as such in the P5 commit. |
| Copy-as-slug on the title control | Title-field slug affordance | **P3** | The mapped title control exposes a copy-as-slug action that slugifies the current title into the clipboard only; no document mutation, no disk write, and no dependence on a saved file. |
| About surface | Application menu About handler | **P12** (version metadata supplied by P14) | A reachable About surface shows application version and build identity, is dispatchable from both menu and palette through one command ID, and contains no update action. Before P14 lands, the version is the crate version. |
| Save-failure crash-report artifact | Save-failure handler and recovery module | **P6** (writer) / **P15** (documentation) | A save failure writes a local artifact containing timestamp, error class, application version, build identity and the project-relative source path — **and no document content**: no buffer text, no frontmatter values, no snippets, no environment variables. One artifact is written per failure episode, not per retry. The path is documented in help/privacy text; artifacts are never transmitted and are deleted by nothing automatic. |
| Linux UI font detection | Linux font command | **P11** capability decision | P11 decides and records one outcome: adopt the system UI font when the pinned GPUI Kit facade exposes a public system-font query, otherwise keep Astrolith theme tokens and accept the native theme as the parity target. No private-API access and no second UI toolkit; the decision is written into the P11 commit and the preferences documentation. |
| Component `index.*` files skipped | Component scanner skip rule | **P9** | P9 skips files whose stem is `index` in the component tree, matching upstream, still scanning their sibling directories; the rule is stated in the P9 acceptance so a later "recursive discovery" change cannot silently start listing index files. |

Additional unattributed items from the survey are resolved as follows:

| Item | Resolution |
|---|---|
| Duplicate naming (`-1` vs `-copy`) | Closed as an intentional divergence (§17, 6.4); the plan's `name-copy` wins. |
| Recent-projects browser and removal | Not an upstream parity row. The registry exists upstream but no browsing or removal UI ships; Astrolith's recent-project list is an **additive strength**, owned by P12, and must be described as additive in user documentation (P15). |
| Startup file-association routing | Strengthening, not parity (§17, 6.11): upstream declares associations but ignores startup arguments. |
| F11 fullscreen on Linux | Bound on Linux as an explicit decision in P12; upstream binds it only off-Linux. Menu and command paths must work independently of F11. |
| Per-session view state (filter/sort/search) | Resolution in row 92: transient per session, reset on project switch; pane layout, last entry and preview width persist per project. |
| Preview | Additive strength, not parity (row 83, §17 6.9). P15 user documentation must state that preview is an Astrolith addition. |
| Copyedit default state | Row 80 requires one source of truth; P7 chooses the persisted default and asserts settings and command labels agree. |

## 19. Excluded behavior (not implemented)

| Upstream behavior | Evidence class | Reason for exclusion |
|---|---|---|
| macOS traffic lights, unified title bar, hide-on-close, dock reopen, platform-specific accelerators | V, C | Non-Linux platform behavior. |
| Windows window controls and overflow menu, Windows blocked-path list, Windows-only F11 binding | V, C, R | Non-Linux platform behavior. |
| Homebrew cask installation | U | Non-Linux delivery. |
| Marketing/documentation website, analytics, download buttons | whole subtree | Excluded by plan §Scope. |
| Telemetry worker, statistics tooling, schema files | whole subtree | No endpoint or events ship (row 111). |
| Platform release signing, notarization, universal builds, release workflow | C | Release delivery excluded; Astrolith uses its own signed manifest. |
| Debug agent bridge (HTTP control, UI tree) | C, R | Developer-only, not end-user-facing; outside the scoped ledger. |
| Tauri/WebView/React/CodeMirror architecture, upstream fonts and pixel appearance | whole tree | Clean-room native requirement (plan §Non-goals). |
| Embedded preview as a Linux requirement | U | Upstream ships no preview; Astrolith's preview is additive, not required parity. |

## 20. P0 exit state for this document

- Every shipped Linux end-user behavior found at the pinned commit has one row, one acceptance
  scenario and one owner; no row is unowned.
- The eight previously unattributed behaviors are closed in §18 with acceptance conditions.
- Editor-command status is deliberately **not** asserted here; it is fixed solely by
  [`docs/technical/editor-capabilities.md`](editor-capabilities.md) after the pinned facade probe.
  Rows 34–36 and 94 state the scenarios; the capability document states what is proven, degraded or
  unavailable, and the final parity gate follows that document.
- Scenarios assume `fixtures/parity-project/`; if the fixture cannot exercise a row, the row's
  evidence is incomplete and the row cannot be marked Pass at the release gate.
