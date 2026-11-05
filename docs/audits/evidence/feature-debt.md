# Feature inventory, command registry and delivery debt

Baseline: branch `dev`, HEAD `326eeec5bcc69d90ea6af6fd50ab8f214b97e4b8`.
Evidence levels: **Confirmed** = measured/reproduced in this audit (counts, greps, `cargo metadata`);
**Strong** = direct source evidence; **Hypothesis** = needs a runtime/native check.

## Scope inspected

| File | Lines | Read |
| --- | --- | --- |
| `crates/astrolith-gpui/src/commands.rs` | 1489 | full (registry, labels, availability, keys, menus, tests) |
| `crates/astrolith-gpui/src/desktop_actions.rs` | 194 | 1-113, test names |
| `crates/astrolith-gpui/src/help.rs` | 463 | full |
| `crates/astrolith-gpui/src/update.rs` | 806 | full |
| `crates/astrolith-gpui/src/single_instance.rs` | 129 | full |
| `crates/astrolith-gpui/src/url_scheme.rs` | 363 | 1-300 |
| `crates/astrolith-gpui/src/recent_projects.rs` | 118 | 1-83 |
| `crates/astrolith-gpui/src/http.rs` | 204 | 1-128 |
| `crates/astrolith-gpui/src/shell.rs` | 7363 | launch path, menus, dispatch, panels, palette, toolbar, listener |
| `crates/astrolith-gpui/src/project_discovery.rs` | 13451 | dispatch targets, availability, guards |
| `crates/astrolith-gpui/src/runtime.rs` | 353 | 1-93 |
| `crates/astrolith-gpui/src/editor/{save_coordinator,writer_surface,rendered,image_preview,lsp}.rs` | 1041/863/226/575/317 | targeted |
| `crates/astrolith-gpui/src/frontmatter/mod.rs` | 1540 | panic context |
| `crates/astrolith-core/src/preferences.rs` | 1604 | autosave/delay/typing |
| `crates/astrolith-{terminal,lsp,preview,workspace,astro}` | 435/1145+291+229+193+274/―/29/8 | lib roots, discovery |
| docs | `README.md` 133, `docs/user/{guide,keyboard-shortcuts,privacy,README}.md` 515/121/94/36, `docs/refactor/ACCEPTANCE.md` 400 (full), `ARCHITECTURE.md` 130, `docs/technical/*` 5-19 each | full |
| meta | root `Cargo.toml` 55, all 11 crate manifests, `mise.toml` 133, `Cargo.lock` (974 packages), `.github/workflows/{ci,release}.yml` 31/125, `scripts/*` 790, `fixtures/` (4 projects), `packaging/linux/*` | full |

Counting shell: `cargo metadata --locked --format-version 1` (exit 0, no compile).

## 1. Feature matrix vs README bullets

| ID | README bullet (`README.md`) | Implementing modules | Class | Evidence |
| --- | --- | --- | --- | --- |
| F1 | Projects/collections: recent registry, Astro 5/6/7 discovery, generated schema, nested collections, static diagnostics, bounded watcher | `recent_projects.rs:24,64`; `astrolith-astro/src/{discovery,config,pattern,schema}.rs`; `astrolith-workspace/src/{index,watcher}.rs` | complete | `fixtures/astro{5,6,7}` + `workspace/tests/{index,watcher,fixture_oracle}.rs`; watcher bounded in `watcher.rs` |
| F2 | Entries: create/duplicate/rename/move/trash/reveal/copy path | `astrolith-workspace/src/operations.rs`; dispatch `shell.rs:2137-2141` | complete | `workspace/tests/operations.rs` (create, rename, trash, symlink escape, byte-exact duplicate) |
| F3 | Navigation: counts, breadcrumbs, draft filter, case-insensitive filter, sort, Content Finder | `project_discovery.rs` (palette/finder), `workspace/src/{list,files}.rs` | complete | ACCEPTANCE §16 regression list; F3 has no e2e beyond GPUI headless |
| F4 | Writer: lossless projection, find/replace, go-to-line, focus/typewriter, presentation, Copyedit, formatting, counts | `astrolith-document/src/{projection,wysiwyg,formatting,tokens}.rs`; `editor/writer_surface.rs` | complete | ACCEPTANCE §19/§21; `project_discovery.rs` regression names |
| F5 | Frontmatter: typed controls, refs, images, raw fallback, lossless, refresh on schema change | `frontmatter/{mod,fields}.rs`, `project_discovery.rs` (`schema_revision`, fingerprint) | complete (one evidence gap) | refresh path exists (`project_discovery.rs:1671,1704`); ACCEPTANCE §13 admits “未新增专门的 schema-refresh 表单集成测试” |
| F6 | MDX components: discovery, typed props, placeholders, Starlight, opt-in imports | `astrolith-astro/src/components/*`; `editor/component_builder/*` | complete | `components/tests.rs` (19 tests), ACCEPTANCE §16 |
| F7 | Safe persistence: atomic saves, autosave 1/2/5/10 default 2, 10 s checkpoint, guards, recovery | `editor/save_coordinator.rs:34-35,532`; `astrolith-document/src/{save,recovery}.rs` | complete | default `AutosaveDelay::Two` (`preferences.rs:127-128`), `MAX_CHECKPOINT = 10 s` |
| F8 | Preview: dev server start/stop/restart/retry, route tracking, external browser “(the only Linux surface)” | `astrolith-preview/*`, `preview/{mod,embedded}.rs` | **partial / doc-defective** | embedded surface exists (`preview/embedded.rs:83-85`) but is never built (§6 D9); README statement stale (D11) |
| F9 | Preferences: three-tier overrides, scoped Reset All, seven named palettes | `astrolith-core/src/preferences.rs`, `settings/{store,migrate}.rs`, `preferences/*` | **defective doc; feature complete minus palettes** | palette list removed in M1 (`ARCHITECTURE.md` §4 theme row); `theme/palettes.rs` has no Catppuccin/Nord/… (grep 0) |
| F10 | Desktop integration: menus from one registry, shortcuts, palette, IDE argv, scheme, MIME, single instance | `commands.rs`, `desktop_actions.rs:26,90`, `url_scheme.rs:65,111,167`, `single_instance.rs:69,82`, `packaging/linux/templates/astrolith.desktop.in:13` | **partial on Linux** | menus are registered but never rendered (D1); everything else implemented + tested |
| F11 | Privacy: “The only Astrolith network request is the signed update manifest” | `http.rs:56` installs a real HTTP client; `editor/image_preview.rs:67,135` | **defective doc, feature correct** | `privacy.md` documents remote document images; `README.md:49` contradicts it (D11-3) |
| F12 | Update check: signed manifest, capability gated | `update.rs:393` (`check`), `:21` (`PLATFORM`) | implemented, **not deliverable** | no signed manifest, no `ASTROLITH_UPDATE_*` set in repo, build identity never injected (D8) |
| F13 | (beyond README) Build source editing, LSP, terminal, WYSIWYG | `editor/{session,lsp}.rs`, `astrolith-lsp`, `astrolith-terminal` | complete per ACCEPTANCE §19-§21 | only partially reflected in README/technical docs (D11) |

README claims that are accurate as written: Astro 5/6/7 discovery, entries, navigation, writer,
frontmatter (except the refresh evidence gap), MDX components, persistence, preferences
inheritance (minus palettes), IDE argv, URL scheme, MIME associations, single instance, privacy
(no telemetry), `mise` toolchain pins and `--locked`.

## 2. Command registry

Shape (`commands.rs`):

- Enum `CommandId` with 73 variants at `commands.rs:12-86`; `COMMANDS` array re-lists all 73 at
  `commands.rs:88-161`. **Confirmed** by parsing the enum: 73 variants, 73 `COMMANDS` entries,
  no duplicates.
- `label()` covers 73 arms (`:334`); `shortcut()` (`:422`) has 39 arms — 38 return a key and reach
  39 commands (`ToggleComment | OpenComponentBuilder` share one arm), `FocusSelectionToolbar`
  returns `None`.
- `is_available()` (`:278`) names 39 commands; 34 fall through to `_ => true` (`OpenFile`,
  `OpenProject`, `OpenRecentProjects`, `Save`, `CloseDocument`, `RecoverUnsaved`,
  `ToggleFocusMode`, `ToggleTypewriterMode`, the 6 copyedit toggles, the 4 mode commands,
  `OpenPalette`, `OpenPreferences`, `ToggleSidebar`, `TogglePreview`, `ToggleTerminal`,
  `FocusDocument`, the 3 theme commands, `ResetLayout`, `ToggleFullscreen`, the 3 Help surfaces,
  `About`, `Quit`). **Confirmed** by parsing the `match` arms.
- Menus: `install_menus` (`:727`) builds File/View/Edit/Writer/Preview/Help/Astrolith and calls
  `cx.set_menus` at `:943`. **Confirmed** 77 placements, 73 unique commands — every registered
  command appears at least once; 4 appear twice (`OpenRecentProjects`, `Quit`, `OpenPreferences`,
  `About`).
- Palette: `shell.rs:2331-2405` re-derives groups from `command_group` (`commands.rs:965`); order
  covers every group value, so no command is dropped for a missing group.
- Keys: `bind_keys` (`:669`) binds 37 command chords (39 with a key minus the two skipped `Ctrl+/` entries) + `ModSlash` + `ContentFinderInsert`
  (`ctrl-enter`, context `ContentFinder`) + `ctrl-space`/`f12` in context `Input`.

Counts (Confirmed):

| Metric | Count |
| --- | --- |
| Registered commands | 73 |
| Commands with a key | 39 |
| Distinct chords for those | 38 (`Ctrl+/` is deliberately shared by `ToggleComment` + `OpenComponentBuilder`, `:672-680`) |
| Commands with no key | 34 |
| Menu placements / unique | 77 / 73 |
| Unreachable by menu, palette or key | 0 in the abstract; 1 in practice (`CheckForUpdates`, see below) |

Findings:

- **D1 (below, registry-level): on Linux no menu is ever rendered.** That removes “menus” as a
  user path; palette + shortcuts remain.
- **Unreachable in the default build:** `CheckForUpdates` `is_available` requires
  `updates_configured` (`commands.rs:326`), filled from `update::CapabilityState::detect()`
  (`shell.rs:931`), which is `Disabled` without compile-time env (`update.rs:52-62`). The palette
  filters unavailable commands out (`shell.rs:2357-2359`), and the toolbar has no entry, so on a
  stock build the command is invisible **and** unexplained. **Strong.**
- **Bindings with overlapping chords:** `Ctrl+Enter` = `OpenPreview` (context `Astrolith`,
  `commands.rs:641-648`) and `ContentFinderInsert` (context `ContentFinder`, `shell.rs:2766`).
  Dispatch precedence is deepest-context-first (`gpui-pre-0.3.10/src/key_dispatch.rs:483`, listener
  loop `window.rs:6196-6212`), so the dialog wins while open. **Strong** (no defect, but the
  shortcut doc lists both under different sections without saying one is dialog-scoped).
- **Framework-shadowed chords:** gpui-base binds `ctrl-f`/`ctrl-h` to `Search`/`Replace` in the
  `Input` context (`gpui-base-0.8.1/src/input/base/state.rs:310-316`). No listener for those
  actions exists in this repo (`grep on_action` in `editor/` → none) and gpui dispatch continues
  when no listener consumes the action, so `FindInDocument`/`ReplaceInDocument` still receive the
  chord. **Hypothesis** — worth one native check; a future listener would silently steal `Ctrl+F`.
- **Non-exhaustive self-guards for keys:** the root action handler calls `execute` directly with no
  availability check (`shell.rs:3197-3200`), although the doc comment claims palette, toolbar and
  context menu share one value (`commands.rs:166-171`). Some targets re-check
  (`project_discovery.rs:5025-5034` for entry commands, `:4767-4773` for formatting) and some do
  not: `select_next_occurrence` (`:4830`), `cursors_at_line_ends` (`:4840`), `open_find` (`:4787`)
  mutate the active session while `is_available` would have disabled them outside editor focus.
  **Strong**; impact is bounded (selection/cursor state, an invisible find bar), not data loss.
- **`ToggleTerminal` forces a mode switch** (`shell.rs:2188-2193`): pressing `Ctrl+\`` in
  Writing/Preview switches to Build before toggling the bottom dock. Deliberate but undocumented.

## 3. Panic / error handling outside `#[cfg(test)]`

Method: scripted scan of every `crates/**`/`apps/**` `.rs` file, excluding `*/tests/*`, `*tests*.rs`
and `#[cfg(test)]` module spans.

Result (Confirmed): **0 `unwrap()`, 0 `todo!()`, 0 `unimplemented!()`, 0 `dbg!()` in production
code.** 19 `expect`/`unreachable` sites (`shell.rs:1102`+`:1142` and `:2380`+`:2400` share an
invariant each):

| Site | Invariant | Judgement |
| --- | --- | --- |
| `project_discovery.rs:1622` | `pending_after_save.take()` after `let Some(..) = ..as_ref()` + path equality | locally provable (same fn, `:1614-1621`) |
| `project_discovery.rs:1737` | `u64 checked_add` on reference revision | provable modulo 2^64 publishes |
| `project_discovery.rs:3356` | same for index revision | same |
| `project_discovery.rs:4480` | same for index request sequence | same |
| `shell.rs:1102,1142` | `env!("CARGO_PKG_VERSION")` parses as `x.y.z` | statically true for `0.1.0`; breaks on a pre-release version string |
| `shell.rs:1733` | `project_scope` present after root equality check | provable (`:1729-1732`) |
| `shell.rs:1941` | `unreachable!("handled above")` for `Inspector|Preview` | provable (`:1919-1938`) |
| `shell.rs:2380,2400` | palette group lookup for “Recent projects”/“Collections” | provable: `order` array at `:2346-2350` |
| `editor/save_coordinator.rs:394` | entry inserted 3 lines above | provable (`:389-394`) |
| `editor/component_builder/mod.rs:205` | `self.context` set 4 lines above | provable (`:203`) |
| `editor/writer_surface.rs:778` | `self.child.take()` — “flow child is laid out once” | **not locally provable**: framework contract on `Element::request_layout`; a second layout request panics. **Hypothesis** (only real panic risk) |
| `frontmatter/mod.rs:317` | `as_object_mut` after `*entry = Value::Object(..)` | provable |
| `astrolith-astro/src/components/imports.rs:382` | `.named.as_ref()` after find predicate on `.named.is_some()` | provable |
| `astrolith-document/src/serialize.rs:120` | `chars().next()` after `is_empty()` early return | provable |
| `astrolith-document/src/parse.rs:580,632` | `unreachable!` for `Opaque` after `sequence_info` filter | provable within module |
| `astrolith-terminal/src/lib.rs:296` | `NonZeroU16::new(value.max(2))` | provable |

Verification of the “zero unwrap” claim matters for the error-handling grade: user-facing failures
go through `Result`/status strings (`update.rs:211-292`, `desktop_actions.rs:26-88`) and
`self.status` messages, not panics.

`unsafe` is forbidden workspace-wide (`Cargo.toml` `unsafe_code = "forbid"`), relaxed to `deny` only
in `crates/astrolith-preview/Cargo.toml` for the Windows job-object module.

## 4. Dead code

- **Modules not reachable from a `lib.rs`/`main.rs`: none.** Scripted check over
  `crates/**`+`apps/**` found only `crates/astrolith-workspace/examples/fixture-index.rs` (an
  `examples/` target, invoked by `scripts/test-astro-fixtures.mjs:683`) and
  `apps/desktop/src/spikes/{editor,layout}.rs`, which are reachable through the inline
  `mod spikes` at `apps/desktop/src/main.rs:1-4`.
- **Spike code ships in the production binary.** `main.rs:1-4` compiles `editor.rs` (28.9 KB) and
  `layout.rs` (14.4 KB) unconditionally; they are only gated at runtime by `--spike=editor` /
  `--spike=layout` (`main.rs:38-43,56-82`) and by the `dev:editor`/`dev:layout` mise tasks. These
  are M00.x spikes still present at v0.2.0 (**Strong**; dead weight, not dead code).
- **Unused direct dependencies** (zero non-test references): `astrolith-lsp` → `serde`
  (only `serde_json` and `lsp_types` are used); `astrolith-terminal` → `tracing` (0 uses).
  **Confirmed** by identifier grep over non-test source.
- **Two implementations of one concept:** `commands.rs:1044` `build_identity()` →
  `"development build"`; `editor/save_failure.rs:27` `build_identity()` → `"dev"`. Neither value is
  ever produced because no build sets `ASTROLITH_BUILD_ID` (grep over `.github/`, `packaging/`,
  `mise.toml` → 0 hits). **Confirmed.** Consequence: About and save-failure artifacts always
  report the placeholder.
- **Two HTTP clients:** `http.rs:38` builds a `ureq::Agent` for GPUI's asset client;
  `update.rs:311` builds a second `ureq::Agent`; additionally the default build links
  `gpui-pre-reqwest` via `gpui-kit-assets` (dependency graph, §5). Three independent stacks for
  one feature area (**Strong**).
- **Feature never enabled:** `embedded-webview` (`crates/astrolith-gpui/Cargo.toml` features;
  forwarded by `apps/desktop/Cargo.toml`) is referenced in **no** mise task (grep `mise.toml` → 0),
  **no** CI workflow, and **not** in the packaging build
  (`packaging/linux/common.sh:37`, plain `cargo build --release --locked -p astrolith-desktop`).
  ACCEPTANCE §14 exercised it manually with `--features embedded-webview` only. **Confirmed.**
  The release channel therefore ships a binary with the feature compiled out while
  README/guide describe the setting.

## 5. Dependencies

`cargo metadata --locked --format-version 1` succeeded (4.4 MB JSON, exit 0, no build).
`Cargo.lock`: **974 packages**, **97 crate names with more than one version** (Confirmed).

Direct deps per crate (declared / notable):

| Crate | Direct | Notes |
| --- | --- | --- |
| desktop | 5 | `astrolith-gpui`, `astrolith-observability`, `gpui-kit`, `tracing`, dev `gpui-kit/test-support` |
| astrolith-gpui | 26 | 8 path crates + `gpui-kit` (11 grammar features), `lsp-types`, `anyhow`, `tokio`, `futures`, `serde`, `serde_json`, `url`, `chrono`, `image`, `ureq`, `ring`, `base64`, `tempfile`, opt `gpui-webview`/`wry` |
| astrolith-astro | 9 | `tree-sitter` + `tree-sitter-typescript`, `regex`, `semver`, `serde_json`, `tempfile`, `tokio` (all used in production: `discovery.rs:540`, `schema/generation.rs:562`, `schema/validation_tooling.rs:251`) |
| astrolith-document | 8 | `tree-sitter`/`tree-sitter-typescript` used in `projection.rs:24,509`; `yaml-rust2`; `tokio` |
| astrolith-lsp | 7 | `serde` unused |
| astrolith-preview | 9 | `nix`/`windows-sys` split by target |
| astrolith-terminal | 3 | `tracing` unused |
| astrolith-workspace | 8 | `notify =8.2.1`, `trash`, `yaml-rust2` |
| astrolith-core | 2 | `serde` only |
| astrolith-observability | 4 | tracing stack |

Duplicate functionality / heavy transitives (Confirmed from the resolved graph, Linux default
features):

- **HTTP**: `ureq` 3.5.1 (app) *and* `gpui-pre-reqwest`+`hyper`+`rustls` (via `gpui-kit-assets`).
  Two TLS stacks and two connection pools for one product need.
- **Images**: direct `image` 0.25.10 (bmp/gif/jpeg/png/webp, `editor/image_preview.rs`) plus
  `resvg`/`usvg` in **two versions each** (0.45.1 and 0.46.0 via `gpui-component` and
  `gpui-pre`) and `image-webp`; `png` 0.17.16 + 0.18.1. Three decode paths.
- **Other duplicated majors**: `nix` 0.28/0.29, `sha2` 0.10/0.11, `getrandom` 0.1/0.2/0.3/0.4,
  `rand` 0.7/0.8/0.9/0.10, `windows-sys` 0.45/0.48/0.52/0.61, `syn` 1/2/3, `thiserror` 1/2,
  `toml` 0.8/1.1, `bitflags` 1/2. Most are forced by the pinned GPUI Kit; none are actionable
  without an upstream bump, but they explain build time and 97 duplicate compilations.
- **Tree-sitter grammars**: 12 distinct grammar crates in the default graph
  (`astro-next`, `css`, `html`, `javascript`, `json`, `md`, `tsx`, `typescript`, `svelte`, `toml`,
  `yaml`, core) — the app enables 11 explicitly at `crates/astrolith-gpui/Cargo.toml`.
- Rough default-graph size: ~863 nodes (crude traversal of the resolved graph), versus 974 in the
  lockfile (the delta is target-specific and the optional WebView subtree).

Root `Cargo.toml` profile settings: `[profile.dev] debug = 1`, plus `opt-level = 3` overrides for
`gpui-base`, `gpui-component`, `gpui-pre`, `ropey`, `gpui-pre-sum-tree`, `tree-sitter`, `taffy`.
There is **no `[profile.release]` at all** — packaged builds get Cargo defaults (no LTO, no
`codegen-units=1`, no `strip`), while `packaging/linux/common.sh:37` ships that binary. **Strong**;
this is the cheapest remaining size/startup lever.

## 6. Test inventory

Counted `#[test]`/`#[gpui_kit::test]` annotations per crate (Confirmed, grep):

| Crate | Tests |
| --- | --- |
| astrolith-gpui | 389 (`project_discovery.rs` 88, `shell.rs` 59, `frontmatter/tests.rs` 26, `preview/tests.rs` 22, `save_coordinator.rs` 21) |
| astrolith-document | 152 |
| astrolith-astro | 144 |
| astrolith-preview | 86 |
| astrolith-workspace | 78 (incl. 3 integration files) |
| astrolith-core | 24 |
| desktop (spikes) | 19 |
| astrolith-lsp | 18 |
| astrolith-terminal | 4 |
| astrolith-observability | 3 |
| **total** | **917** |

Target files: `commands.rs` 11, `update.rs` 9, `help.rs` 6, `url_scheme.rs` 6,
`desktop_actions.rs` 3, `single_instance.rs` 2, `recent_projects.rs` 2, `http.rs` 2.

`#[ignore]`d workloads (8, Confirmed):

| Site | Purpose |
| --- | --- |
| `astrolith-astro/src/schema/generation_tests.rs:354` | needs `ASTROLITH_FIXTURE_PROJECT` |
| `astrolith-document/src/tests.rs:1490` | `projection_diff_workload` |
| `astrolith-gpui/src/project_discovery.rs:7860` | installed-Astro editing workflow |
| `astrolith-gpui/src/project_discovery.rs:10446` | `projected_writer_workload` |
| `astrolith-gpui/src/shell.rs:5873` | window resize workload |
| `astrolith-gpui/src/shell.rs:5981` | readonly file-to-frame workload |
| `astrolith-preview/tests/preview_gate.rs:162` | installs fixture and runs real `astro dev` |
| `apps/desktop/src/spikes/editor.rs:654` | large-document workload |

Every ignored case has a mise entry (`test:*:perf`, `test:fixtures`) except
`generation_tests.rs:354` (env-driven) — the workload story is coherent.

Unit-heavy vs E2E: the suite is dominated by unit/entity tests; the only end-to-end paths are
(a) the Astro fixture runner (`mise run test:fixtures` → `scripts/test-astro-fixtures.mjs`, 727
lines, which shells into `cargo test --package astrolith-gpui installed_astro_fixture_editing_workflow`
at `:690` and `cargo run --example fixture-index` at `:683`), (b) the opt-in preview gate
(`preview_gate.rs`), and (c) the ignored GPUI workflow. There is **no** scripted native E2E
(no window automation, no protocol/MIME/file-dialog runs) — guide §14 admits exactly this.
`artifacts/e2e/<run-id>/{check.log,perf.log}` is a manual log convention only: two run directories
exist, nothing in the repo references them, `.gitignore` does not exclude them, and no test reads
them. **Confirmed.**

CI: `.github/workflows/ci.yml` runs `mise run ci` (= `check`: doctor, fmt, clippy `-D warnings`,
build, workspace tests, fixture verification) on `ubuntu-24.04`, `macos-15`, `windows-2025`.
Release: `.github/workflows/release.yml` packages deb/appimage/rpm and signs a manifest, but never
runs tests and never enables a feature flag.

## 7. Findings

Severity is relative to “Linux alpha with headless evidence”.

| ID | Finding | Evidence | Level | Impact |
| --- | --- | --- | --- | --- |
| **D1** | **No application menu is ever displayed on Linux.** `install_menus` registers File/Edit/View/Writer/Preview/Help/Astrolith, but gpui-pre-linux only stores them (`gpui-pre-linux-0.3.10/src/linux/platform.rs:1003-1011`) and nothing draws them: the app never instantiates `AppMenuBar` (`grep` over `crates/` → 0), and `set_app_menus`, the only writer `AppMenuBar` reads, has **no callers** in `gpui-component`/`gpui-kit`/the app. Users get only the toolbar “Panels” dropdown, the palette and keys. | `commands.rs:727-944`; app toolbar `shell.rs:3330-3384`; platform impl above | Strong | README “application menus”, guide “File → Open project…”, “Writer menu”, “Help menu contains …”, and ACCEPTANCE §11/§16/§19 menu evidence describe unreachable UI. The 73-command registry is still exercised by palette/keys, so the *commands* are reachable; the *menus* are not. |
| **D2** | **Mode-ambiguous `Toggle preview`:** `CommandId::TogglePreview` (`Ctrl+Alt+P`) and `ToggleTerminal` (Ctrl+backtick) both toggle `PanelKind::Preview`, whose panel is chosen by mode. In Build, “Toggle preview” shows/hides the **terminal**; the toolbar dropdown relabels it (“Terminal”) but the label, palette entry, menu item and shortcut doc do not. | `shell.rs:1684-1712` (`panel` = terminal in Build), `:2187-2193`, `commands.rs:399` label, `shell.rs:3364-3379` relabel | Strong | Misleading command semantics; documented as “Toggle preview” in `keyboard-shortcuts.md:39` with no Build caveat. |
| **D3** | **Keyboard dispatch bypasses `is_available`.** Root `on_action` calls `execute` unconditionally; guard coverage is uneven (entry commands and formatting self-check, find/occurrence/cursor handlers do not). | `shell.rs:3197-3200` vs `commands.rs:166-171`, `project_discovery.rs:4830,4840,4787` | Strong | Disabled commands can run from keys; observable as silent no-ops/invisible find bars, not corruption. |
| **D4** | **Only genuine panic risk:** `writer_surface.rs:778` `self.child.take().expect("flow child is laid out once")` inside `Element::request_layout`. | `editor/writer_surface.rs:771-780` | Hypothesis | A double layout request (framework retry/measure path) panics the UI. All 18 other `expect`/`unreachable` sites are locally provable (§3). |
| **D5** | **Unused direct dependencies.** | `crates/astrolith-lsp/Cargo.toml` (`serde`), `crates/astrolith-terminal/Cargo.toml` (`tracing`); grep 0 | Confirmed | Build graph noise; trivial removal. |
| **D6** | **Three HTTP stacks**: app `ureq` asset client, app `ureq` update client, framework `gpui-pre-reqwest`. | `http.rs:38`, `update.rs:311`, dependency graph (`gpui-pre-reqwest` parents: `gpui-kit-assets`) | Strong | Duplicate TLS/connection code, binary size, two timeout/redirect policies to keep in sync (`http.rs:21-22` vs `update.rs:26`). |
| **D7** | **Duplicate image-decode stacks**: direct `image` 0.25.10 for previews; `resvg`/`usvg` 0.45+0.46 and `image-webp` via GPUI; `png` 0.17+0.18. | `crates/astrolith-gpui/Cargo.toml`; lockfile duplicates | Strong | Larger binary and two decode policies; our own limits (`image_preview.rs`) do not constrain the SVG path. |
| **D8** | **Build identity is never injected** and exists twice with different fallbacks (`"development build"` vs `"dev"`). | `commands.rs:1044-1047`, `editor/save_failure.rs:27-29`, 0 hits for `ASTROLITH_BUILD_ID` in `.github/`, `packaging/`, `mise.toml` | Confirmed | About dialog and save-failure artifacts cannot distinguish a packaged release from a dev build, contradicting `privacy.md` (“build identity”) and the update/`About` docs. |
| **D9** | **`embedded-webview` is never built** by any task, CI job or package recipe. | `mise.toml` (0 hits), `.github/workflows/*` (0 hits), `packaging/linux/common.sh:37` | Confirmed | The advertised “external or embedded presentation” (`guide.md:418`, ACCEPTANCE §14) cannot be exercised by any produced artifact; the XWayland evidence in ACCEPTANCE §14 is unreproducible from repo tasks. |
| **D10** | **Windows CI cannot compile the test suite.** At least two un-gated POSIX-only test sites: `image_preview.rs:477` uses `std::os::unix::fs::symlink`, and `astrolith-preview/src/server_tests.rs:16,57-383` calls `test_support::write_script`, which is itself `#[cfg(unix)]` (`test_support.rs:23`). Every other `std::os::unix` use in the workspace is gated (`single_instance.rs:113`, `url_scheme.rs:243`, `project_discovery.rs:8564`, `runtime.rs:208`, `settings/store.rs:1219`, `document/save.rs:268`, `lsp/discovery.rs:184,282`). `astrolith-terminal` tests also spawn a literal `sh` (`astrolith-terminal/src/lib.rs:342,383`), which would compile but fail at runtime on Windows. | `crates/astrolith-gpui/src/editor/image_preview.rs:477-493`, `crates/astrolith-preview/src/server_tests.rs:16`, `crates/astrolith-terminal/src/lib.rs:342` vs `ci.yml` matrix | Strong (compile/runtime level; not executed on Windows here) | `ubuntu`/`macos` can be green while the `windows-2025` job fails at `cargo clippy --all-targets`, so the 3-OS matrix cannot serve as the release gate it appears to be. |
| **D11** | **Doc/code contradictions** (full table in §8). | see §8 | Strong | Users and future audits are told about UI/features that do not exist (palettes, menus, “only network request”, read-only Open file). |
| **D12** | **Release manifest signs artifacts the client ignores**: workflow signs `linux-x86_64-deb` and `-rpm`, but the client matches only `linux-x86_64-appimage`. | `release.yml:110-113` vs `update.rs:21,432` | Strong | Harmless today (appimage entry is present), but the deb/rpm rows are dead payload and invite a future mismatch if the AppImage is dropped. |
| **D13** | **M00 spikes ship in the release binary** (43 KB, runtime-gated only). | `apps/desktop/src/main.rs:1-4`; `mise.toml` `dev:editor`/`dev:layout` | Strong | Dead weight and a second, divergent editor implementation in every binary. |
| **D14** | **No `[profile.release]`** while packaging ships a release build. | `Cargo.toml:47-55`; `packaging/linux/common.sh:37` | Strong | Missing LTO/`strip`/`codegen-units` tuning; artifact size and startup left on defaults. |
| **D15** | **`artifacts/e2e/` is undocumented and unignored** (two run dirs with 82 KB logs). | `artifacts/e2e/audit-*/{check,perf}.log`; no reference in docs, absent from `.gitignore` | Confirmed | Evidence convention is tribal; risk of committing machine logs, or of losing the only E2E trace. |

Ranked debt (highest first): **D1 → D9 → D2 → D11 → D8 → D10 → D3 → D6/D7 → D14 → D13 → D5 → D4 → D12 → D15**.

## 8. Docs consistency (quote both sides)

| ID | Doc statement | Code reality |
| --- | --- | --- |
| D11-1 | `README.md:43-45`: “independent light and dark palettes (One, Catppuccin, Nord, Gruvbox, Tokyo Night, Solarized, Rosé Pine)” | `ARCHITECTURE.md` theme row: “M1 已改为单一中性 light/dark，删除 palette 选择”; `theme/palettes.rs` contains no such names (grep 0); `guide.md:134` correctly says “one neutral palette” |
| D11-2 | `README.md:41-42`: “external-browser presentation (the only Linux surface)” | `preview/embedded.rs:83-85` builds a real `wry` WebView on X11; `guide.md:418` allows “embedded build … on a supported display backend”; ACCEPTANCE §14 records XWayland screenshots |
| D11-3 | `README.md:49-50`: “The only Astrolith network request is the signed update manifest” | `privacy.md` lists a second case (“Remote document images … Preview and the Alt-hover image preview fetch that URL”); `http.rs:56` installs the client; `rendered.rs:144-146` falls back to `ImageSource::from(uri)` for remote URIs |
| D11-4 | `docs/technical/astro-editor-parity.md:7`: “`.astro` 开发文件编辑、交互 Terminal 与真实 Embedded surface 当前尚未交付；完整 LSP 为单独扩展” | `.astro`/TS editing (`project_discovery.rs` source sessions, ACCEPTANCE §10/§18), terminal crate (`astrolith-terminal`, §8), embedded surface (§14), LSP crate (§20) all exist; ACCEPTANCE §11 records user acceptance of A14/A24/A25 |
| D11-5 | `docs/technical/editor-capabilities.md:3`: “隐藏 Frontmatter-only 补丁尚缺统一 history 集成证据” | ACCEPTANCE §9 (M4a) documents the unified complete-source transaction stack + `HistoryChanged`; `project_discovery.rs` has the interleaved undo/redo regression |
| D11-6 | `docs/technical/editor-capabilities.md:15`: “任意 Open file 入口仍 readonly” | `shell.rs:2869-2879`: `open_file` routes in-project editable sources to `open_project_source` (editable session) |
| D11-7 | `docs/technical/editor-capabilities.md:17`: “app 不提供远程图片 HTTP client” | `http.rs:31-56` installs `UreqHttpClient`; `runtime.rs:57` calls it at startup; ACCEPTANCE §19 limitation (“当前 Linux 构建没有 HttpClient”) is stale |
| D11-8 | `guide.md:206` / ACCEPTANCE `:349-350`: “Panels menu” shows “Astro preview” / “Details” | There is no “Panels” *menu*; it is a toolbar dropdown (`shell.rs:3330`), and the native menu bar containing `TogglePreview`/`ToggleInspector` is never rendered on Linux (D1) |
| D11-9 | `guide.md:504-505`: “arbitrary local files opened with **Open file…** are read-only … Editing applies to project entries” | In-project `.astro`/TS/JS/JSON/… open editable via the same dialog (`shell.rs:2869-2875`); only non-source or out-of-project paths are read-only |
| D11-10 | `guide.md:37-38` (“Native GTK/WebKit packages are not needed … opens previews in your normal browser”) | True for the default build, false for `--features embedded-webview` (links `lb-wry`/WebKitGTK, ACCEPTANCE §14 requires `WEBKIT_DISABLE_DMABUF_RENDERER=1` on NVIDIA) — the doc should scope the claim |
| D11-11 | `ARCHITECTURE.md:130`: “现有 Help 链接测试在未改源码前失败” | The M0 note is historical and now false: `help.rs:406-441` passes because the referenced `docs/technical/*` stubs exist (5-19 lines each); README/`docs/user/README.md:19-31` still promise “architecture boundaries, performance workloads, test strategy and the real-platform gate list” from a 19-line stub |

`ARCHITECTURE.md` claims that **do** hold: runtime sizing (2 workers / 4 blocking threads / 2
slots) matches `runtime.rs:39-52`; LSP scope/discovery order matches `astrolith-lsp/src/discovery.rs`;
terminal ownership matches `astrolith-terminal/src/lib.rs`; `embedded-webview` is a real feature but
default-off.

## 9. Open questions / not inspected

- Native check: whether `Ctrl+F`/`Ctrl+H` reach `FindInDocument` while the editor is focused (the
  §2 framework-shadowed-chord item, pinned `Search`/`Replace` actions) — needs a window or a GPUI test that asserts the
  app's find bar appears.
- Whether the framework ever calls `request_layout` twice for one `FlowDocument` element (D4).
- Real menu rendering on macOS/Windows (`gpui-pre-apple` may render `Menu` natively); not needed for
  the Linux delivery but would change the severity framing of D1 for the other platforms.
- Windows/macOS compile status of the whole workspace: only `image_preview.rs` was verified as
  un-gated; other unix-only APIs (`nix`, `PermissionsExt`, `UnixListener`) were checked and are
  gated.
- I did not read `project_discovery.rs` end to end (13.4 k lines) or `astrolith-preview/src/server.rs`
  beyond its public surface; findings there are limited to the dispatched handlers and the
  ACCEPTANCE-listed regressions.
- Performance numbers from ACCEPTANCE §16 were not re-measured (build cache in use); only the
  `cargo metadata`/lockfile analysis above was executed.
