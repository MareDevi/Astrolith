# Feature completeness and reliability analysis

Audit date: 2026-11-05, `dev` @ `326eeec`. Baseline gate: `mise run check` exit 0,
**915 passed / 0 failed / 8 ignored**. Evidence: `evidence/feature-debt.md`,
`evidence/document-session-save.md`, `evidence/terminal-build-mode.md`,
`evidence/workspace-astro-preview.md`.

Classification used throughout: **Complete** (implemented, reachable, tested),
**Partial** (reachable but incomplete), **Missing**, **Defective** (reachable and wrong),
**Unverified** (code exists, no runtime/native proof), **Intentionally unsupported**.

## 1. User-facing feature matrix

| # | Capability | Status | Evidence | Note |
|---|---|---|---|---|
| F1 | Recent-project registry, reopen, Astro 5/6/7 discovery, generated schema, nested collections, static diagnostics | Complete | `recent_projects.rs:24,64`; `astro/src/{discovery,config,pattern,schema}`; fixture oracle PASS | |
| F2 | Entry create / duplicate / rename / trash / reveal / copy path | Complete | `workspace/src/operations.rs`; `tests/operations.rs` (13) | |
| F3 | Collection counts, breadcrumbs, draft filter, title/filename filter, sort modes, Content Finder | Complete | `project_discovery.rs`, `workspace/src/{list,files}.rs` | Search rebuilds per keystroke (PERF-05) but works |
| F4 | Writer: lossless projection, find/replace, go-to-line, focus/typewriter, WYSIWYG, Copyedit, formatting, counts | Complete | `document/src/{projection,wysiwyg,formatting,tokens}`, `writer_surface.rs` | Cost scales with document (PERF-01/04) |
| F5 | Frontmatter form: typed controls, references, images, raw fallback, lossless patch, schema refresh | Complete (one evidence gap) | `frontmatter/*`, `schema_revision` | `ACCEPTANCE.md` §13 admits no dedicated schema-refresh form integration test |
| F6 | MDX component builder: discovery, typed props, tracked placeholders, Starlight, opt-in imports | Complete | `astro/src/components/*`, `editor/component_builder/*` | |
| F7 | Atomic conflict-aware saves, autosave 1/2/5/10 s (default 2), 10 s checkpoint, dirty guards, recovery | **Defective** | `save.rs`, `save_coordinator.rs` | Mechanics are sound; two data-path defects: conflict dead-end (DOC-02), recovery baseline (DOC-03) |
| F8 | Preview: dev server start/stop/restart/retry, route tracking, external browser | Complete for external; **Unverified** for embedded | `preview/*` | Embedded surface exists but is never built by any task/CI/package (FEAT-02) |
| F9 | Preferences: global/project/collection, Apply/Cancel, scoped reset, advanced versions | Complete | `core/preferences.rs`, `settings/*`, `preferences/*` | README's "seven palettes" is stale (FEAT-06) |
| F10 | Desktop integration: menus, shortcuts, palette, IDE argv, `astrolith://`, MIME, single instance | **Defective on Linux** | `commands.rs` (73 commands), `url_scheme.rs`, `single_instance.rs` | **No menu bar is rendered on Linux** (FEAT-01) |
| F11 | Privacy: no telemetry; one documented network request | Complete feature, **Defective docs** | `help.rs`, `privacy.md`, `http.rs` | Remote document images are a second network path not in README |
| F12 | Signed update check | Implemented, **not deliverable** | `update.rs` | Capability requires build env that nothing sets; command hidden and unexplained |
| F13 | Build source editing, LSP, terminal | **Partial** | `astrolith-lsp`, `astrolith-terminal`, `editor/lsp.rs` | See `build-mode-audit.md`; diagnostics routing wrong (BM-09), terminal teardown unsafe (BM-16/17) |
| F14 | Unsaved-change protection (close/switch/quit, discard prompt) | Complete | `guard_window_close`, `flush_dirty_buffers`, tests | |
| F15 | Undo/Redo across body + hidden frontmatter | Complete | `SourceHistory`, `session.rs:1848-1936` | Undo of a frontmatter-only edit changes nothing visible (U-01) — semantic, documented |
| F16 | Multi-tab editing (Writing rail + Build tab strip) | Complete | `build_tab_strip`, 32-session cap | |
| F17 | External file modification detection | **Defective** | `disk_fingerprint` | Every own save flags the document as changed on disk (DOC-01) |
| F18 | Reload-from-disk | **Missing** | no `refresh_from_load` caller | `DocumentBuffer::refresh_from_load` is test-only |
| F19 | Autosave / checkpoint | Complete | `save_coordinator.rs` | |
| F20 | Workspace switching | Complete | `start`/`flush_dirty_buffers`/`advance_switch` | Terminal kill can block (BM-17) |
| F21 | File rename/move/delete from the UI | Complete | `perform_rename`/`perform_trash` | Watcher reconciliation tested |
| F22 | Error reporting to the user | **Partial** | `Shell.file_status` never rendered (FEAT-05); `status` line used for some paths | Silent failures on file open / scan / builder |
| F23 | Crash recovery | **Partial** | `recovery.rs`, `recover_pending` | Recovery restores text but adopts current disk as baseline (DOC-03) |
| F24 | Keyboard shortcuts | Complete with gaps | 39 commands keyed, `keyboard-shortcuts.md` generated from the registry | `Ctrl+F`/`Ctrl+H` routing unverified (Hypothesis) |
| F25 | Accessibility (names, tab order, contrast) | Partial / Unverified | `preferences_sheet_names_controls_and_fits_the_minimum_window`; theme contrast test | No screen-reader or native a11y evidence |
| F26 | Logging / diagnostics | Complete | `astrolith-observability`, redacted save-failure artifacts | Build identity never injected (FEAT-08) |
| F27 | Cross-platform behaviour | **Unverified / broken in CI** | `image_preview.rs:477`, `server_tests.rs` un-gated POSIX | Windows CI cannot compile tests (FEAT-07) |
| F28 | Packaging | Implemented, unvalidated | `packaging/linux/*` | No clean-environment install/uninstall run has been recorded |

## 2. Defects that make a documented workflow unreachable

**FEAT-01 (P1, Strong).** **No application menu is displayed on Linux.** `install_menus`
registers 7 menus and 77 placements (`commands.rs:727-944`), but the pinned Linux platform
implementation only *stores* them (`gpui-pre-linux-0.3.10/src/linux/platform.rs:1003-1011:
fn set_menus(&self, menus: Vec<Menu>, _keymap: &Keymap)`), and the app never instantiates a
menu bar widget (`AppMenuBar` has zero references in `crates/`). Every doc sentence of the
form "File → Open project…", "Writer menu", "Help menu → …" describes unreachable UI.
Palette, shortcuts and the toolbar dropdown still work, so the capability exists without
the documented path.

**FEAT-02 (P1, Confirmed).** **`embedded-webview` is built by nothing.** No mise task, no
CI job, no packaging recipe enables it (`packaging/linux/common.sh:37` builds the default
feature set). The shipped Linux binary therefore cannot offer the embedded preview that
`guide.md` and `ACCEPTANCE.md` §14 describe. Either the feature becomes part of a
documented build task with a gated release, or the docs must state it is developer-only.

**FEAT-03 (P2, Strong).** **`TogglePreview` and `ToggleTerminal` share
`PanelKind::Preview`**, whose content is mode-selected. In Build, "Toggle preview" opens
the terminal; the palette, menu item and shortcut reference keep the wrong label
(`commands.rs:399`, `keyboard-shortcuts.md:39`).

**FEAT-04 (P2, Strong).** **`ToggleTerminal` from Writing/Preview enters Build and may
immediately close the dock** (`shell.rs:2188-2194` + `apply_layout`): if the last Build
layout had the dock visible, the toggle hides it. The user sees nothing happen.

**FEAT-05 (P2, Confirmed).** **`Shell.file_status` is never rendered.** Nine assignment
sites (`shell.rs:2676-3014`) carry open/scan/builder errors; only tests read the field
(`shell.rs:6276-6278`). A failed "Open file…" or a scan error is silent.

**FEAT-06 (P2, Strong).** **README advertises seven palettes that do not exist.** The M1
theme migration replaced them with one neutral light/dark palette
(`ARCHITECTURE.md` theme row, `theme/palettes.rs`); `README.md:43-45` still lists One,
Catppuccin, Nord, Gruvbox, Tokyo Night, Solarized, Rosé Pine.

**FEAT-07 (P2, Confirmed).** **Windows CI cannot compile the test suite**: un-gated
`std::os::unix::fs::symlink` in `editor/image_preview.rs:477-493`, and
`astrolith-preview/src/server_tests.rs:16,57-383` calling a `#[cfg(unix)]` helper;
`astrolith-terminal` tests spawn a literal `sh` (`lib.rs:342,383`). The workflow exists
(`ci.yml` matrix includes `windows-2025`) but the platform is not actually verified.

**FEAT-08 (P3, Confirmed).** **Build identity is never injected** (`ASTROLITH_BUILD_ID`
has zero hits in CI, packaging, mise) and is defined twice with different fallbacks
(`"development build"` in `commands.rs:1044`, `"dev"` in `save_failure.rs:27`). About and
save-failure artifacts cannot distinguish a release build.

**FEAT-09 (P3, Confirmed).** **Release manifest signs artifacts the client ignores**
(`release.yml:110-113` signs deb/rpm; `update.rs:21,432` matches only
`linux-x86_64-appimage`).

**FEAT-10 (P3, Strong).** **M00 spikes ship in the release binary** (43 KB, runtime-gated
by `--spike=…`, `apps/desktop/src/main.rs:1-4`).

## 3. Silent-failure and state-consistency inventory

| Path | What goes wrong | Evidence | Level |
|---|---|---|---|
| Save → watcher | Own save sets `externally_changed = true` (stale `disk_fingerprint`); tab shows "Changed on disk" and status says so | `session.rs:171`, `project_discovery.rs:3325` | Strong |
| Conflict | No resolution path; the documented remedy (close+reopen) deletes the recovery record | `save_coordinator.rs:462-468`, `project_discovery.rs:3842-3850` | Strong |
| Recovery restore | Adopts current disk as baseline, discarding the record's revision/mtime | `project_discovery.rs:4284` | Strong |
| Two queued lifecycle actions | One `pending_after_save` slot; a second request silently discards the first | `project_discovery.rs:777, 5553, 5693, 5823` | Strong |
| Save of an oversized document | Saves fine, then cannot be reopened (8 MiB read cap) and reads as "changed on disk" forever | `save.rs:186-235` vs `project_discovery.rs:3560` | Strong |
| Definition into a file | `open_project_source` bypasses the memory budget that `open_entry_inner` enforces | `project_discovery.rs:2786-2795` vs `3528-3536` | Strong |
| LSP diagnostics | Published for other URIs / stale versions are painted on the active document | `session.rs:1577`, `lsp.rs:87-101` | Confirmed |
| LSP failure | `Failed`/`Exited` events logged and dropped; no UI state | `session.rs:1587-1594` | Strong |
| Terminal after shell exit | Header keeps "running" (green) with `running = session.is_some()` | `terminal/mod.rs:428` | Strong |
| Terminal focus | Leaving Build with terminal focus keeps sending keys to the hidden shell | `terminal/mod.rs:100-113`, `shell.rs:1907-1913` | Strong (observable = Hypothesis) |
| Index scan | One symlink, missing collection dir, duplicate slug, TOML `+++`, or inotify ENOSPC → whole project fails to open or navigator empties | `index.rs:349-350, 389-393, 969-977`; `watcher.rs:198-202` | Strong |
| Preview | No HMR observation; server keeps running after preview hides | `preview/mod.rs:325-333` | Strong |
| Command availability | Keyboard dispatch bypasses `is_available`; uneven self-guards | `shell.rs:3197-3200`, `project_discovery.rs:4830,4840,4787` | Strong |

## 4. Reliability of code-level invariants

- **0 `unwrap()`, 0 `todo!()`, 0 `unimplemented!()`, 0 `dbg!()`** in production code
  (Confirmed by scripted scan). 19 `expect`/`unreachable` sites; 18 are locally provable.
- The one genuine panic risk is `writer_surface.rs:778`
  `self.child.take().expect("flow child is laid out once")` inside
  `Element::request_layout` — a framework contract, not a local invariant (Hypothesis).
- `unsafe` is forbidden workspace-wide and relaxed to `deny` only in
  `astrolith-preview` for the Windows Job Object module.
- Panics in service tasks surface as `Err("task panicked …")` through the runtime bridge
  (`runtime.rs:112-126`), so a worker panic does not abort the UI.

## 5. Documentation contradictions (must be fixed regardless of code changes)

| # | Doc says | Code does |
|---|---|---|
| 1 | `README.md:43-45` — seven named palettes | one neutral palette (FEAT-06) |
| 2 | `README.md:41-42` — "external-browser presentation (the only Linux surface)" | embedded `wry` surface exists, X11-only, never built (FEAT-02) |
| 3 | `README.md:49-50` — "the only network request is the update manifest" | remote document images are fetched via `http.rs:56` and `image_preview.rs`; `privacy.md` documents it |
| 4 | `docs/technical/astro-editor-parity.md:7` — ".astro editing, terminal and embedded not delivered" | all three exist (ACCEPTANCE §8/§10/§14) |
| 5 | `docs/technical/editor-capabilities.md:3` — frontmatter history not integrated | M4a delivered one complete-source history |
| 6 | `docs/technical/editor-capabilities.md:15` — "Open file…" is read-only | in-project sources open editable (`shell.rs:2869-2875`) |
| 7 | `docs/technical/editor-capabilities.md:17` — no remote HTTP client | `http.rs` installs one at startup |
| 8 | `guide.md:206`, ACCEPTANCE — "Panels menu" | a toolbar dropdown; no native menu bar on Linux |
| 9 | `guide.md:504` — arbitrary Open file is read-only | in-project source files are editable |
| 10 | `guide.md:37` — no WebKit packages needed | true only for the default build; the embedded feature links `lb-wry` |
| 11 | `ARCHITECTURE.md:130` — Help link test fails at baseline | it passes (the stub docs it links now exist) |

## 6. E2E coverage gaps

Existing coverage is **unit/entity-heavy**: 917 tests, of which the genuinely end-to-end
paths are (a) the Astro fixture oracle runner (`mise run test:fixtures`, 6 Node tests + a
Rust example), (b) the opt-in preview gate (`preview_gate.rs`, ignored by default), and
(c) ignored GPUI workflow tests. There is **no** scripted native E2E: no window automation,
no protocol/MIME handler run, no file-dialog run, no packaged-artifact install run.

Missing, in priority order (scenarios defined in `e2e-verification-plan.md`):

1. Save → reopen → byte-identical, including frontmatter-only edits and CRLF/Unicode.
2. External modification → conflict → resolution (the path that currently has no exit).
3. Crash → recovery → first autosave does not clobber a newer on-disk edit.
4. Build: edit `.ts`/`.astro` → real language server → diagnostics land on the right file.
5. Build: terminal `Ctrl+C`, resize, 2-minute `yes` flood, project switch (no orphans).
6. Preview: edit → HMR visible in the browser; port conflict; server crash.
7. Mode cycles × 50: no session, watcher, server or PTY recreation.
8. Settings: clean install, v2 upgrade, corrupt file, future version, Reset All.
9. Quit with a 100 MB document open and a running dev server: no orphan, no hang.
10. Native menu/palette equivalence for every menu-only action.

## 7. Priority

| Priority | Items |
|---|---|
| **Now (P0/P1)** | DOC-01 fingerprint loop, DOC-02 conflict dead-end, DOC-03 recovery baseline, BM-09 diagnostics routing, FEAT-01 no menu, BM-02/03/06/07 LSP lifecycle, BM-16/17 terminal teardown, PERF-01/02/03 render scaling |
| Next (P2) | FEAT-02/03/04/05, WS-03 hard-fail policy, BM-23/24 terminal font, CFG-01/03/08, docs table §5 |
| Later (P3) | FEAT-07/08/09/10, dependency dedup, `[profile.release]`, artifacts convention |

Nothing in this list requires a new subsystem. The most expensive item (LSP registry) is
one new entity with an existing client underneath; the rest are corrections.
