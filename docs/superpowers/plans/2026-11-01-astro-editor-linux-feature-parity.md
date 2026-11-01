# Astro Editor Linux feature-parity implementation plan

**Status:** Approved on 2026-11-01; P0 evidence, fixture and editor capability gate complete; P1 scoped preferences and project registry implemented; P2 entry lifecycle, filters and sort implemented; P3 schema-driven frontmatter parity implemented (real-window field-family gate still manual); P4 next.

**Date:** 2026-11-01

**Baseline:** Upstream `dannysmith/astro-editor` commit `9ec2343b0bd3f4424f92a20000231a66f1930a34` (Astro Editor 1.0.20) against Astrolith commit `5c90091d8be8de63d8d45b856e6d73eee28447df`.

**Scope:** Match every shipped Linux end-user feature in upstream, clean-room in native Rust/GPUI Kit. Preserve Astrolith's retained tabs, conflict-safe atomic save, durable recovery, preview workflow and cross-platform-safe architecture. Exclude the marketing/docs website, release signing, macOS/Windows delivery and platform-only behavior until Linux passes. Telemetry remains local opt-in state only; no endpoint or events ship.

## 1. Outcome and invariants

A Linux user can register/open recent Astro projects, browse and mutate collection entries, edit Markdown/MDX with the upstream writer controls, configure global/project/collection behavior, preview the active route, and install/open/update-check the packaged app without upstream code or a web UI runtime.

1. Complete source is authoritative. Hidden frontmatter/imports are never deleted, normalized or reconstructed from lossy state.
2. Save/recovery/conflict detection keep operating on complete-source `DocumentBuffer`. Autosave only schedules the existing guarded save path.
3. File I/O stays off the GPUI render path, is root/symlink constrained, and reconciles tabs, index, watcher, recovery, preview and selection transactionally.
4. Unsupported pinned GPUI Kit 0.8.1 behavior fails an early capability gate; no private API patch or second UI framework.
5. Preference resolution is `collection override > project override > global default`; absence means inheritance.
6. Formatting/copyedit use pure byte ranges over a writer projection, preserve selection/undo, and cannot touch hidden source segments.
7. Astrolith safety wins where upstream is weaker: desktop trash, dirty-close decisions, conflict blocking and visible autosave failure.
8. `commands.rs` remains the sole command registry consumed by menu, palette, shortcuts, toolbar and context menus.

## 2. Parity ledger

| Surface | Upstream behavior | Existing strength retained | Owner |
| --- | --- | --- | --- |
| Projects | recent registry, open/remove/reopen, package metadata | discovery/generation guards | preferences + shell |
| Entries | create, duplicate, rename, delete, reveal, copy path, extension choice | index/watch/safe sessions | workspace operations |
| Navigation | collections, draft/title filter, schema-aware sort, Content Finder | tree/list/tabs | project workspace |
| Writer | MD/MDX, hidden metadata/imports, formatting, search, multicursor, focus/typewriter | IME, retained undo/cursor, highlighting | projection + editor |
| Assets/links | dropped images/files, managed asset paths, image hover, URL/internal links | safe workspace boundary | asset + link services |
| MDX builder | Astro/React/Vue/Svelte discovery, typed prop form, imports/snippets | Astro schema/static parsing seams | component service + UI |
| Save | 2 s idle autosave, 10 s continuous-write checkpoint, manual save | atomic conflict save/recovery | save coordinator |
| Frontmatter | typed schema fields and special-field mappings | lossless patches/validation | existing form + prefs |
| Copyedit | noun/verb/adjective/adverb/conjunction toggles and exclusions | native decorations | copyedit service |
| Preview | active route/server controls | stronger local service/fallback | preview surface |
| Preferences | global/project/collection/advanced settings | versioned global theme/layout | settings subsystem |
| Desktop | menus, shortcuts, IDE/reveal, association, URL scheme, package/update | native commands/menu | desktop + packaging |
| Help/privacy | help/full-screen/devtools/log path, telemetry controls | observability | shell + docs |

Parity means behavior, not pixel duplication. Keep the native four-panel shell and Astrolith tokens; do not reproduce upstream branding or its WebView/iA Writer appearance.

## 3. Architecture decisions

### 3.1 Scoped preferences

Create domain types in `astrolith-core` and persistence in `astrolith-gpui`:

- `GlobalPreferencesV2`: theme, panel visibility/width layout, light/dark heading colors, editor base font size, autosave delay (1/2/5/10 seconds), default new-file extension, IDE executable/arguments, five Copyedit category toggles/colors and telemetry consent.
- `ProjectPreferencesV1`: stable ID/current path/display name/last-open, project layout/last entry, content/assets/MDX-component directories, image path mode, default extension, title/date/description/draft field mappings and experimental MDX import insertion.
- `CollectionPreferencesV1`: content/assets directory overrides, image path mode, extension, title/date/description/draft mapping overrides and Content Finder URL pattern.
- `ResolvedPreferences`: immutable result of one resolver; consumers do not merge scopes.

Persist atomically under:

```text
astrolith/preferences/global.json
astrolith/preferences/projects.json
astrolith/preferences/projects/<project-id>.json
```

The registry stores stable IDs, canonical paths, package names and last-open timestamps. Rebind a moved project only when the old path no longer exists and package identity matches; simultaneous same-name projects stay distinct. Migrate current `preferences.json` once into V2, retain it as `preferences.v1.migrated.json`, and stop reading it afterward. Preserve unknown JSON envelope fields. Quarantine invalid data using current corruption handling and surface it in Preferences.

### 3.2 Lossless writer projection

Add `astrolith-document::projection`:

```rust
pub struct WriterProjection {
    pub visible: String,
    pub segments: Vec<ProjectionSegment>,
}
pub enum ProjectionSegment {
    Hidden { source: Range<usize> },
    Visible { source: Range<usize>, visible: Range<usize> },
}
pub fn project_writer(source: &str, kind: DocumentKind) -> Result<WriterProjection, ProjectionError>;
pub fn apply_visible_edit(source: &str, projection: &WriterProjection, edit: TextEdit) -> Result<String, ProjectionError>;
```

Markdown hides only exact leading frontmatter. MDX also hides the contiguous top-level ESM import/export prelude proven by Tree-sitter; ambiguous syntax remains visible. Mapping is UTF-8-byte based, monotonic and round-trips unchanged source exactly. `DocumentSession` shows `visible` in `EditorState` while `DocumentBuffer` retains complete source. Editor edits map back before save/recovery/form patch. Frontmatter patches and reloads rebuild projection while preserving mapped cursor offset.

A disposable real-window gate must prove IME, undo/redo, cursor restoration, body edits interleaved with frontmatter edits, and byte-exact save/reload before formatting/autosave/copyedit starts.

### 3.3 File operations

Extend `astrolith-workspace::operations` with typed create, duplicate, rename/move, trash, reveal-target and copy-path requests/results. Every request carries project generation and canonical source ID. Validate names, extension, collision, containment and symlinks; execute blocking I/O away from render; return identity deltas for shell reconciliation. Pin `trash = 5.2.9`; no permanent-delete fallback.

Create uses resolved collection directory/extension. Write minimal schema-valid frontmatter only when required fields have representable defaults; otherwise show creation fields before writing. Duplicate preserves bytes and derives `name-copy`, `name-copy-2`, etc. Rename does not rewrite content. Dirty sessions save through the guarded path before rename/duplicate; trash requires Save or Discard. The file list additionally owns exact draft-only filtering, case-insensitive filename/title filtering, and stable sorting by default publication date, title, filename, modified time, every schema date field, and numeric `order` when present; missing dates sort first for the default mode.

### 3.4 Editor capability contract and formatting

The P0 probe must prove or expose public GPUI Kit APIs for search/replace, go-to-line, select-next-occurrence, add-cursors-at-selected-line-ends, modified-click cursors, mapped batch replacement, cursor geometry and scroll offset. The pinned 0.8.1 facade publicly exposes search/replacement, active selection, multiple-selection creation, tracked decorations, geometry and scroll offset. The residual gaps are enumerating all selections, applying distinct text per range as one edit, and resolving a modified pointer click to a byte offset; Ctrl-click cursor placement and commands requiring those operations remain blocked. Astrolith will not reach through private fields or vendor a fork. All independent editor work may proceed, but the final parity gate cannot pass until the pinned facade exposes the required operations. `docs/technical/editor-capabilities.md` is the sole authority for exact status and the minimal upstream proposal.

Add pure `astrolith-document::formatting` transforms only for upstream commands: bold, italic, link insertion/edit, headings 1–4 and plain paragraph. Preserve line endings and indentation and reject hidden-boundary edits. Indent/outdent, Markdown list continuation, inline/fenced code and blockquote display use the editor's native Markdown behavior; they are not new formatting commands. Apply each transform as one undo transaction.

Writer chrome also includes native find/replace, next/previous match, go-to-line, select next occurrence, add cursors to selected line ends, Ctrl-click cursors, syntax-marker dimming, hanging heading markers where geometry permits, responsive prose typography, and toolbar auto-hide only when both sidebars are closed. Focus mode dims every sentence except the cursor sentence. Typewriter mode keeps the cursor line vertically centered by combining public cursor geometry and deferred scroll offset. The contextual selection toolbar uses `range_to_bounds`, clamps inside the pane, hides for empty selection/IME and remains keyboard reachable.

### 3.5 Autosave

App-owned `SaveCoordinator`, keyed by canonical source:

```text
Clean -> Dirty(idle deadline) -> Saving(ticket) -> Clean
              |                       |
              +-> 10 s max deadline --+
              +-> Manual -------------+
Dirty + conflict -> Blocked
Saving + newer revision -> Dirty(new deadlines)
Saving + failure -> Dirty(error)
```

The effective delay is exactly 1, 2, 5 or 10 seconds (default 2). Every dirty run also has a 10-second maximum checkpoint during uninterrupted typing. One in-flight save per document; a newer revision schedules a follow-up. Close/rename/trash/project switch/quit flush or prompt under the existing dirty contract. Status distinguishes Unsaved, Saving…, Saved, Conflict and Save failed.

### 3.6 Copyedit and writing modes

Add deterministic offline English classification in `astrolith-document::copyedit`. Exclude fenced/inline code, link destinations, HTML/MDX tags, frontmatter and hidden imports. Match upstream's five categories only: nouns, verbs, adjectives, adverbs and conjunctions. A compact lexicon plus ordered suffix/context rules handles classification; unknown tokens are undecorated. This matches the feature contract, not Compromise's exact linguistic output.

Run debounced analysis in background with revision/generation tickets; drop stale results. Use tracked decorations, omit the active token ±2 bytes, and replace one category layer atomically. Preferences control the five toggles and semantic colors. Footer word/character counts share the token snapshot; focus-mode sentence spans use a separate deterministic sentence detector.

### 3.7 Content links, dropped assets and image preview

Create `astrolith-workspace::assets` to copy dropped images/files into resolved assets directories. Sanitize to lower-kebab, strip unsafe characters, prepend local `YYYY-MM-DD`, collision-suffix atomically, and preserve bytes. Insert image Markdown for supported raster/SVG formats and normal Markdown links otherwise. Relative paths resolve from the document; absolute mode emits project-root paths. Pasting a valid URL over selected text wraps it as a link. Alt-click opens only validated `http`/`https` URLs.

Content Finder searches every indexed entry by configured title/id, Enter opens the item, Ctrl+Enter inserts a link. A collection URL pattern substitutes URL-encoded `{slug}` from frontmatter then ID; absent pattern emits a relative file link. Alt-hover image preview resolves remote HTTP(S), project-root and document-relative sources, loads/decodes off the render path with size/time limits, and anchors a dismissible preview bottom-right. Invalid/untrusted paths show no content and never escape the project root.

### 3.8 MDX component builder

Create `astrolith-astro::components` as a static, non-executing index. Discover `.astro`, `.tsx`, `.jsx`, `.vue`, `.svelte` recursively in the resolved component directory; extract framework script/frontmatter blocks and analyze local/relative TypeScript declarations with the existing Tree-sitter stack. Model required/optional props, defaults, doc comments, string unions/enums, number, boolean, date, string/number arrays, `ImageMetadata`, `CollectionEntry<'name'>`, client directives and content/slot support. Unknown types become strings; parse failures still list the component with no props. Add a versioned built-in Starlight 0.42 catalog only when `@astrojs/starlight` is declared.

The GPUI builder palette fuzzy-searches user then package components, shows framework/prop/slot metadata, and opens a typed form. Ctrl+Enter inserts a generated tag; required blanks/content become tracked snippet placeholders navigated by Tab/Shift+Tab/Escape. Optional props are omitted. Image and collection-entry props use existing asset/index services.

When project opt-in import insertion is enabled, parse complete source at insertion time, refuse same-name collisions, reuse existing/corroborated imports, otherwise add a relative/package import while preserving quote/semicolon/multiline style; named imports join compatible declarations. Add `getEntry` loads and image imports with collision-safe identifiers. Keep import edits in complete-source state and component insertion in writer projection; undo removes the component but intentionally leaves the harmless import, matching upstream. Auto-import package detection shows a warning but does not change the user's setting.

### 3.9 Desktop/platform surfaces

- IDE command is argv with `{project}`, `{file}`, `{line}`, `{column}` placeholders; no shell interpolation.
- Reveal opens the containing directory; file selection only where a verified Linux portal/API supports it.
- Editor file drops feed the asset service; shell drop accepts one directory to open a project or one in-project `.md`/`.mdx` item. Reject mixed/multiple drops visibly.
- Package MIME handlers for Markdown/MDX and an `astrolith://open?path=` URL scheme. Both route through the project registry and validated content-entry open flow. Document the name difference from upstream's `astro-editor://` clean-room identity.
- Publish AppImage, `.deb`, `.rpm`. Update check consumes signed HTTPS manifest, verifies Ed25519 metadata + SHA-256 artifact, then opens release download; no self-replacement.
- Updater UI stays capability-disabled until manifest URL/public key exist.
- Telemetry is off and local-only; no endpoint, ID, queue or network request.

## 4. Ordered implementation tasks

Every task: meaningful failing test → minimal implementation → focused test → real smoke → `mise run check` → independent review → one Conventional Commit. Cargo uses `--locked`; dependency commits include `Cargo.lock`.

### P0 — Freeze evidence, fixtures and editor API gates

**Files:** `docs/technical/astro-editor-parity.md`, `docs/technical/editor-capabilities.md`, new `fixtures/parity-project/`, disposable desktop spike.

Record every shipped Linux behavior from upstream user docs, command registry, menus and current source with one Astrolith acceptance scenario. The fixture includes MD/MDX imports, all supported frontmatter field types, local Astro/React/Vue/Svelte components, Starlight dependency metadata, nested collections, aliases, assets, required fields, CRLF, Unicode, code/link content, duplicate slugs and external changes.

Probe pinned GPUI Kit in a real window for projected editing, search/replace, batch changes, cursor geometry/typewriter scrolling, token activation/hover, native image/file clipboard/drop, and all multicursor commands. For every missing public operation, prepare the minimal upstream facade API proposal and block only the dependent shortcuts; do not patch private state.

**Gate:** no unowned upstream feature, fixture passes current discovery/schema/preview, projection prerequisites pass, and every editor command is Proven or explicitly Blocked on an identified upstream API. Commit evidence/fixture separately from any dependency update.

### P1 — Scoped preferences and project registry

**Files:** create `crates/astrolith-core/src/preferences.rs`; split `crates/astrolith-gpui/src/settings.rs` into `settings/{model,resolve,store,migrate}.rs`; update exports/mounting.

Implement exact global/project/collection fields from §3.1, version envelopes, one resolver, atomic stores, corruption recovery, v1 migration and recent/moved project registry. Migration must preserve current theme and panel visibility/widths. Tests cover explicit false/zero inheritance, collection→project mapping inheritance, Copyedit persistence, layout restart, path normalization, migration, corruption, moved paths, same-name collision and stale completion. Smoke: overrides, restart, move fixture, reopen with settings/layout retained.

### P2 — Safe entry lifecycle, filters and sort

**Files:** create `crates/astrolith-workspace/src/operations.rs`; update workspace exports/index/watcher and GPUI project/commands/dialogs/context menus.

Implement create, duplicate, rename/move, trash, reveal and absolute/relative copy path; schema-aware creation and MD/MDX choice; transactionally reconcile index, selection, tabs, sessions, recovery keys, preview route and watcher batches. Implement draft-only, case-insensitive filename/title filter, and exact sort modes/directions from §3.3.

Tests: traversal/symlink refusal, collisions, nested collections, dirty guards, duplicate bytes, trash error, watcher race, recovery retarget, mapped draft field, undated ordering and stable equal keys. Smoke every operation/filter/sort and inspect disk/tree/tabs/route/trash.

### P3 — Complete frontmatter parity gate

**Files:** extend `astrolith-astro/src/schema/{model,normalize,generation}.rs`, `astrolith-document/src/{parse,serialize}.rs` plus a focused `yaml/` module, `astrolith-gpui/src/frontmatter/{fields,mod}.rs`, metadata index/schema watcher and data-collection discovery.

Implement schema description/constraints/defaults and declaration order; typed number/integer/date/enum/reference/image/description controls; single and array references; lossless insert/remove of optional fields; block scalar-sequence and simple-mapping-sequence edits; validated raw-YAML fallback for unsupported containers; unknown-field and no-schema controls; schema freshness/reload; JSON `file()` collection reference targets. Preserve comments, unknown fields, quoting and CRLF; never coerce unsupported values. Existing lossless refusal remains for anchors, aliases, tags, complex keys and unsafe flow forms.

Tests cover both schema dialects, order/metadata, every control family, constraints, reference labels/data entries, sequence insertion/removal, comments/CRLF, raw-YAML validation, absent-field insertion, unknown/no-schema and stale schema results. Real-window gate edits/saves/reopens every supported field family, including image via P8 service when available; image control may initially choose an in-project path, but frontmatter cannot pass final parity until external copy works.

### P4 — Lossless writer projection gate

**Files:** create `astrolith-document/src/projection.rs`; update parser/exports/tests and GPUI editor session/project/frontmatter; temporary writer-projection spike removed after acceptance.

Implement mapping/MDX prelude, projected editor, complete-source buffer and revision-ordered body/form rebasing. Tests: boundaries, Unicode/CRLF, malformed frontmatter, ambiguous MDX, body/form interleave, save/recovery round-trip and conflict. Real Linux smoke: IME, undo/redo, every P3 control while body-dirty, save/restart/recover and hidden-region byte identity. Failure blocks P5–P10 and returns to approval.

### P5 — Core writer commands and chrome

**Files:** create `astrolith-document/src/{formatting,sentences}.rs`; create `astrolith-gpui/src/editor/{formatting,selection_toolbar,writer_chrome,writing_modes}.rs`; update editor/commands/shell/theme.

Implement exact formatting and editor contract from §3.4: shortcuts, find/replace navigation, go-to-line, multicursor actions when API gate permits, Markdown presentation, focus-by-sentence, typewriter centering, responsive typography, hanging headings, toolbar auto-hide, contextual toolbar and counts/status footer. Add URL-over-selection paste and Alt-click HTTP(S) here; asset/image behavior remains P8.

Tests: toggles, heading/plain transforms, URL paste, sentence boundaries, Unicode/CRLF, visible mapping and typewriter offset calculation. Smoke all pointer/keyboard commands, one-step undo, search, scroll, IME, themes, narrow window, sidebars and reduced motion.

### P6 — Exact autosave and lifecycle flush

**Files:** create `astrolith-gpui/src/editor/save_coordinator.rs`; update session/project/shell/settings/status.

Implement idle + 10-second maximum deadlines and route automatic/manual saves through guarded atomic save. Tests: 1/2/5/10-second policy, continuous typing checkpoint, edit during save, conflict, failure/retry, switch ordering, close/quit flush and stale generation. Smoke rapid continuous typing, switching, read-only failure, external conflict and recovery.

### P7 — Copyedit mode

**Files:** create `astrolith-document/src/copyedit/{mod,tokenize,classify}.rs`; create `astrolith-gpui/src/editor/copyedit.rs`; update settings/editor/theme.

Implement five exact categories, Markdown/MDX exclusions, revisioned background classification, active-token suppression and tracked decorations. Tests: representative fixed corpus, exclusions, cursor proximity, stale results, Unicode and no overlap. Smoke large-document typing, each toggle/all toggle/colors/focus interaction/scroll with no drift or input lag.

### P8 — Content Finder, dropped assets and image preview

**Files:** create `astrolith-workspace/src/assets.rs`, `astrolith-gpui/src/editor/{content_finder,assets,image_preview}.rs`; update project/editor/commands/settings.

Implement service and UX from §3.7, including date/name/collision rules, image-vs-file Markdown, relative/absolute modes, external-image copy, internal-link pattern/fallback, Enter open vs Ctrl+Enter insert, Alt-hover preview and path/network limits. Integrate schema image frontmatter fields with the same picker/copy service.

Tests: sanitization, collision, path traversal/symlink, relative calculation, URL pattern encoding/fallback, image detection, preview resolution and stale async result. Smoke local/remote image previews, image/PDF drops, clipboard entries, Content Finder open/insert and undo.

### P9 — Static MDX component index and import planner

**Files:** create `crates/astrolith-astro/src/components/{mod,discover,props,starlight,imports,snippet}.rs`; add frozen component fixtures and a versioned Starlight 0.42 catalog asset.

Implement all component/index/import semantics from §3.8 without running project code. Keep parsing bounded and generation-ticketed. Tests use real Astro/React/Vue/Svelte source for required/optional/default/docs/unions/enums/arrays/images/entries/client directives/slots/relative types, malformed fallback, duplicate names, quote/semicolon/multiline import style, named-import merge, reused imports, auto-import detection and identifier collisions.

**Gate:** generated tags/import plans match frozen expected complete-source bytes and never modify unrelated import formatting.

### P10 — MDX component builder and snippets

**Files:** create `astrolith-gpui/src/editor/component_builder/{mod,palette,form,snippets}.rs`; update editor/project/settings/commands.

Implement searchable framework-grouped palette, metadata/help, typed prop controls, optional-prop toggle, client directives, image and collection-entry pickers, tag preview, Ctrl+Enter insertion and tracked Tab/Shift+Tab/Escape placeholders. Apply planned imports to complete source and tag to projection with stale-plan revalidation; undo behavior matches §3.8.

Smoke every framework, keyboard-only flow, required blank placeholders, wrapping content, asset import, `getEntry`, existing import reuse, add-import off/on, auto-import warning, dirty body and concurrent external edit.

### P11 — Complete preferences UI

**Files:** create `astrolith-gpui/src/preferences/{mod,general,project,collection,advanced}.rs`; update shell/commands/settings/theme.

One modal/sheet with panes exactly matching upstream scope. Inline validate directories, mappings against schema kinds, URL pattern, IDE argv, font range and autosave choices. Apply atomically; Cancel mutates nothing; reset controls remove only their value. Advanced shows app/preferences versions, opens preferences folder, and Reset All requires object-specific destructive confirmation, deletes only preferences/registry (not recovery/logs), then restarts.

Smoke keyboard/focus/Escape, invalid/dirty state, restart, project switching, inherited values, themes/minimum window; run full GPUI Kit design checklist.

### P12 — Navigation, registry commands and desktop protocols

**Files:** update commands/shell/project/layout; create `recent_projects`, `drag_drop`, `desktop_actions`, `url_scheme` modules.

Add recent-project welcome/open/remove, collection/file context menus, breadcrumbs, draft/filter/sort controls, copy path, reveal, project/collection/file IDE open, reload collections, Help/logs/full-screen and conditional debug/devtools. Wire shell drop, startup files, MIME and `astrolith://open?path=` through one validator. Menus/palette/controls share command IDs, labels, shortcuts and availability; include every upstream reference shortcut plus retained Astrolith commands.

Tests cover command availability, IDE argv, URL decoding/registry/root/content checks and startup routing—not copied menu wiring. Smoke every action by mouse/keyboard, protocol cold/warm launch, invalid paths and empty/loading/error states.

### P13 — Preview lifecycle integration

**Files:** update GPUI preview/project/commands/status; alter `astrolith-preview` only for missing behavior.

Retarget after create/rename/trash/switch and Content Finder open; persist pane open/width per project; preserve external-browser default and recoverable errors. Test route removal, stale generation, occupied port and project switch. Smoke preview while mutating entries.

### P14 — Linux packaging, associations and signed update check

**Files:** `packaging/linux/{astrolith.desktop,astrolith.metainfo.xml,astrolith.xml}`, package config/scripts, approved icons, `astrolith-gpui/src/update.rs`, CI/release workflows and mise tasks.

Build AppImage/deb/rpm from one metadata source; register Markdown/MDX MIME and `astrolith` scheme; implement signed manifest/version check and external release opening; capability-disable until URL/key configured. Gate: install/launch/association/protocol/uninstall each artifact in clean Linux environments and reject tampered metadata/artifacts.

### P15 — Help, privacy, docs and final audit

**Files:** README, developer/technical/user docs, privacy/help content, original implementation-plan status after acceptance.

Document install, settings/overrides, operations/filter/sort, writer/search/modes, assets/links, component builder, autosave/conflict/recovery, preview, protocol, privacy, packaging and Linux limits. Expose local preferences/log/recovery paths without content leakage. Walk every frozen ledger scenario. Remove spikes, stale commands, aliases and superseded docs.

**Release gate:** `mise run check`; full real Wayland workflow; hidden-source/import byte snapshots; no known silent data loss; packaged artifacts launch; every scoped ledger row Pass or has explicit user-approved removal. The final gate remains blocked while required GPUI public multicursor APIs are unavailable.

## 5. Subagent execution map

One integration owner controls shared manifests/locks, `commands.rs`, `shell.rs`, settings/editor module roots and milestone commits. Agents never concurrently edit those files.

- **Wave A:** P1 preference domain/store, P2 pure workspace operation/filter-sort services, P3 pure schema/document extensions and P4 pure projection mapping in parallel; owner integrates exports/manifests.
- **Integration Gate A (serial):** one owner lands P2 file lifecycle, P3 frontmatter/schema freshness and P4 projected sessions into `project_discovery.rs`, `editor/session.rs` and shared tests, then runs both real-window gates.
- **Wave B after Gate A:** P5 pure formatting/sentences, P6 save coordinator, P7 Copyedit classifier, P8 asset/link pure services in parallel; one owner integrates editor/session/render.
- **Wave C:** P9 component index/import planner while P8 GPUI integration completes; P10 starts only after P9 gate.
- **Wave D:** P11 panes by scope, P12 desktop-action/protocol services, P13 preview reconciliation in parallel; one owner composes shell.
- **Wave E:** P14 packaging/update and P15 docs/evidence in parallel; final release integration serial.

Each slice: implementer → independent reviewer → implementer fixes verified findings → owner focused check + real smoke → atomic commit. Agents receive exact API/invariants and skip workspace-wide checks mid-flight; owner runs `mise run check` at each green commit boundary.

## 6. Explicit non-goals

Upstream branding/pixel appearance/React-Tauri architecture; upstream website; macOS/Windows delivery; embedded preview as Linux requirement; sync/accounts/collaboration/remote/plugin marketplace; network telemetry; full IDE/LSP; arbitrary YAML editor; WYSIWYG; exact Compromise linguistic output.

## 7. Approval checkpoint

Approval authorizes P0→P15 and these subagent waves. Frontmatter/projection failure, missing GPUI public API, package-signing prerequisite or any weakening of save/recovery returns here with evidence; no silent workaround.
