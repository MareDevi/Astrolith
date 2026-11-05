# Build Mode audit — LSP, terminal, and their integration

Audit date: 2026-11-05, `dev` @ `326eeec`. Build Mode is **not** a greenfield subsystem:
it exists, it is tested (LSP 24 tests, terminal 4, shell Build tests, preview 85 + 9 unix
server tests), and it works for the documented feature set. This audit measures what it
actually does against what it claims, and where the coupling costs.

Full per-subsystem evidence: `evidence/lsp.md`, `evidence/terminal-build-mode.md`,
`evidence/zed-reference.md` (sections L\*, T\*).

## 1. Build Mode composition

Build Mode is a presentation state, not a separate document stack:

| Aspect | Implementation | Evidence |
|---|---|---|
| Mode enum | `WorkspaceMode::{Writing, Build, Preview}` in `astrolith-core` | `core/preferences.rs` |
| Activation | `Shell::set_mode` → `capture_layout` → close inspector → `session.set_source_view(true)` → `apply_layout` → `sync_center` → persist `last_mode` | `shell.rs:1875-1916` |
| Document identity | same `DocumentSession`, same `DocumentBuffer`, same `SourceHistory` | `session.rs:1414-1468` |
| Editor presentation | line numbers, indent guides, folding, code presentation, full-width, scrollbar | `layout/panels.rs:155-174` |
| Navigation | `FileBrowser` (bounded real project tree) replaces the collection library | `project_discovery.rs:1116-1123` |
| Tab strip | `build_tab_strip` over `project.open_tabs()` | `shell.rs:1747-1838` |
| Bottom dock | terminal panel (or the Astro preview in Preview mode) | `shell.rs:1684-1716` |
| Services | LSP starts on entry, stops on exit; terminal is already running | §3, §5 |

**BM-01 (P2, Strong).** `set_source_view` calls `editor.set_value(full source)`, which
clears the pinned editor's internal undo stack (`gpui-base/state.rs:1245`) and rebuilds its
layout cache. History is preserved because `SourceHistory` is the owner, but every mode
switch and every tab switch pays a full document replacement (measured `set_value`
= 42 ms at 1 MiB, `EDITOR_WORKLOAD`).

### 1.1 Retained vs destroyed

| Transition | PTY | Terminal view | Editor state | LSP | Build tree | Preview server | WebView |
|---|---|---|---|---|---|---|---|
| Open project | **created (eagerly)** | retained | — | not running | created | retained, not started | not mounted |
| Writing → Build | retained | mounted if layout says so | source presentation, history kept | **spawned** | **full walk** | retained | removed |
| Build → Writing | retained | unmounted | projection rebuilt, history kept | **killed** | retained | retained | removed |
| Build → Preview | retained | retained | unchanged | killed | retained | **auto-started if embedded** | mounted |
| Build tab switch | retained | retained | replaced (`set_value`) | **stop + start** | retained | — | — |
| Dock hidden (Ctrl-`) | retained | unmounted | unchanged | unchanged | unchanged | unchanged | removed |
| Project switch | **destroyed (blocking kill)** | retained | sessions cleared | destroyed | re-scanned | not restarted | reloaded |
| Quit | destroyed in `Drop` (blocking) | dropped | flushed first | `kill_on_drop` | — | group killed | destroyed |

## 2. Language server — lifecycle

**BM-02 (P1, Confirmed).** **There is one server process per open Build document, not per
language or per project.** `DocumentSession::attach_lsp` constructs an `LspState` that
owns the whole `LspClient` (`session.rs:1530-1600`); `suspend_lsp`/`stop_lsp` drop it
(`session.rs:1612-1622`); the tab switch path stops the outgoing file's server
(`project_discovery.rs:3666-3676`). The module comment in `astrolith-lsp/src/language.rs:23-25`
describes "one-server-per-language reuse" — the code does not implement it. Opening five
`.ts` tabs in one project costs five `typescript-language-server` processes and five
`node_modules` scans, serialised by the 2-permit blocking lane.

**BM-03 (P1, Strong).** **Graceful shutdown is unreachable in production.**
`ClientHandle::shutdown()` has exactly one caller — a test (`client.rs:1031`). `stop_lsp`
sends `didClose`, drops the state, and the actor's `kill_on_drop` kills the child
(`client.rs:255, 339-345`). The 2 s grace window in `shutdown` (`client.rs:479`) is dead
code in the app. A server that is mid-write when a tab is switched is `SIGKILL`ed.

**BM-04 (P1, Strong).** **No restart, no crash recovery, no user-visible failure state.**
`LspEvent::Failed` / `Exited` are logged (`session.rs:1587-1594`) and discarded. The
`lsp_unavailable` latch resets only on `stop_lsp` (`session.rs:1619`), so a server installed
after a failed discovery is picked up only by leaving and re-entering Build.

**BM-05 (P2, Strong).** **Document URIs are built from an uncanonicalized path**
(`editor/lsp.rs` via `uri_for`), while saves and watch events use canonical paths. Two
sessions for the same file via different spellings (symlinked project root, `..` segment)
produce two server-side documents.

## 3. Language server — protocol correctness

| ID | Finding | Evidence | Level |
|---|---|---|---|
| BM-06 | **No request timeout.** `request()` waits on a oneshot that is only completed by a response (`client.rs:211-229`). A wedged server hangs the provider task forever; no retry, no user feedback. | `client.rs:211-229, 425-440` | Confirmed |
| BM-07 | **`$/cancelRequest` is never sent.** Dropping a provider task abandons the local wait but the server keeps computing and its reply is discarded. | grep: no `cancelRequest` in the workspace | Confirmed |
| BM-08 | **`pending` grows monotonically** for ids that never get a reply (server crash, dropped request); removed only on response (`client.rs:389, 431`). | same | Confirmed |
| BM-09 | **Diagnostics ignore both `uri` and `version`.** `LspEvent::Diagnostics { diagnostics, .. }` (`session.rs:1577`) then `apply_diagnostics(editor, &diagnostics)` (`editor/lsp.rs:87-101`), converted against the *current* text. Publishes for other files in the project are painted onto the active editor; version N can overwrite N+1. | `session.rs:1577`, `lsp.rs:87-101` | Confirmed |
| BM-10 | **Cross-file definition columns are wrong.** Ranges are converted UTF-16→char using the *source* document (`lsp.rs:206-208`); for a target line absent from the source, the column collapses to 0 (`position.rs:72-79, 96-101`). | same | Strong |
| BM-11 | **Capability negotiation is one-way.** `initialize` declares only sync + completion + hover + definition + codeAction + publishDiagnostics (`client.rs:518-590`); `Event::Ready(capabilities)` is discarded (`session.rs:1601`), so providers are never gated and semantic tokens/document colors (supported by the pinned editor) are never negotiated. | same | Confirmed |
| BM-12 | **Server-initiated requests get placeholder answers** (`client.rs` request handling): `workspace/configuration` returns an empty array regardless of the section asked; `window/workDoneProgress/create` is acknowledged; `client/registerCapability` is ignored. A server that needs a setting silently gets defaults. | `client.rs` server-request arm | Strong |
| BM-13 | **Framing** is correct on the happy path (`Content-Length`, split reads, oversize rejection — `framing.rs` tests) but the body is copied three times per frame and drained from the front of the buffer (`client.rs`), so a large completion payload is O(n²)-ish in copies. | `framing.rs`, `client.rs` | Strong |
| BM-14 | **Full-text `didChange` on every edit**, ignoring the server's declared sync kind (`client.rs:717-725`), debounced 150 ms by task-drop cancellation (`session.rs:1652-1665`). The debounce copies the complete source *before* the timer starts. | same | Confirmed |
| BM-15 | **Completion is requested per typed character with a UI-thread full-document `to_string()`** (`lsp.rs:129`), while `didChange` is debounced. This is the dominant per-keystroke LSP cost. | `lsp.rs:129` | Confirmed |

### 3.1 Feature coverage matrix

"Implemented" = reachable from the UI and covered by a test; "partial" = reachable but
incomplete; "missing" = not present; "unverified" = code exists, no runtime proof.

| LSP feature | Status | Evidence |
|---|---|---|
| `initialize` / capabilities | Implemented (request-only) | `client.rs:518-590` |
| `didOpen` | Implemented | `session.rs:1560` |
| `didChange` | Implemented (full text, no versioning) | `client.rs:717-725` |
| `didSave` | **Missing** | no `didSave` in workspace |
| `didClose` | Implemented | `session.rs:1620` |
| Completion | Implemented (per keystroke, no prefix filtering) | `lsp.rs:129` |
| Hover | Implemented | `lsp.rs` provider |
| Definition | Partial (cross-file column bug) | BM-10 |
| References | **Missing** | no request in workspace |
| Rename | **Missing** | — |
| Code actions | Partial (resolve and `Command` no-op; `documentChanges` dropped) | `lsp.rs:283-317` |
| Diagnostics (push) | **Defective** (uri/version ignored) | BM-09 |
| Diagnostics (pull) | **Missing** | — |
| Semantic tokens | **Missing** (editor supports it) | BM-11 |
| Formatting | **Missing** | — |
| Workspace symbols | **Missing** | — |
| Signature help / inlay hints | **Missing** | — |
| Cancellation / timeouts | **Missing** | BM-06/07 |
| Server restart / recovery | **Missing** | BM-04 |

This is consistent with `docs/refactor/ARCHITECTURE.md:125` ("no rename/symbol/signature …
cannot claim IDE parity"), but **not** with `README.md`'s "Language server" framing alone;
the gap list must stay visible in user documentation.

## 4. LSP ownership decision

Options evaluated:

| Option | Cost | Benefit | Verdict |
|---|---|---|---|
| Per active document (today) | server restart per tab switch; N processes for N tabs; diagnostics only for the visible file | simplest code | **Replace** |
| Per language per project, refcounted open documents | one registry entity on `ProjectWorkspace`; must route diagnostics by URI | removes restart thrash, enables project-wide diagnostics, matches every reference implementation | **Adopt (Zed's pattern, simplified)** |
| Global per language across projects | cross-project contamination of `rootUri`; servers are root-scoped | none for Astrolith's single-project window | **Reject** |

Concrete minimum: a `LspRegistry` (path → client handle) keyed by
`(project_generation, LanguageId)`, with refcounted open documents, the existing
`astrolith-lsp` client underneath, and **URI-scoped** diagnostic routing. This is the one
place where adopting a reference design is justified by a measured cost (BM-02) rather than
by preference.

## 5. Terminal — PTY and process lifecycle

| ID | Finding | Evidence | Level |
|---|---|---|---|
| BM-16 | **PTY kill signals only the shell pid.** `portable-pty`'s `kill` sends `SIGHUP` to the pid, waits 5 × 50 ms, then `Child::kill`. No `killpg`, no reap after `SIGKILL`. A foreground job that ignores `SIGHUP`, a `setsid` grandchild, or an exec'd supervisor survives project switch and quit. | `astrolith-terminal/src/lib.rs:277-293`, `portable-pty/src/lib.rs:340-379` | Strong |
| BM-17 | **`kill()` joins the reader thread on the UI thread.** Called from `attach → stop` on `ProjectOpened`, from Restart, and from `Drop`. The reader blocks in `read()` until every slave fd closes, so a surviving descendant freezes the window. | `lib.rs:277-293`, `terminal/mod.rs:130-138, 175-179`, `shell.rs:498-506` | Strong (freeze is Hypothesis) |
| BM-18 | **The shell is spawned eagerly on project open**, before Build is ever visited — including the auto-reopened last project at startup. One forked shell tree the user did not ask for, and a new hang candidate on every project switch. | `shell.rs:498-506`, asserted by `shell.rs:4481-4523` | Confirmed (test) |
| BM-19 | **No quit hook and no project-close hook.** The only cleanup is `Drop` during window teardown; `SIGKILL` of the app leaves the tree. | grep: no `ProjectClosed`, no terminal shutdown in `shell.rs:362-369` | Strong |
| BM-20 | **Shell is spawned without `-l`/`-i`**, so login profiles are never read; `$SHELL` only. | `lib.rs:76-92` | Strong |
| BM-21 | **Exit status fabricates a code for signal death** (`status.code().unwrap_or(1)` → `SIGINT` shows "code 1 (Interrupt)"), and `running = session.is_some()` keeps a green dot after exit. | `lib.rs:38-44, 244-265`, `terminal/mod.rs:428` | Strong |
| BM-22 | **Resize is a pixel estimate** (`bounds/7.8`, `bounds/17`, clamped 20–500 × 3–200), not a font measurement; every dock drag issues `SIGWINCH` at up to 62.5 Hz. | `terminal/mod.rs:277-287` | Strong |

What is right: the child is a session leader with a controlling tty (`portable-pty`
`setsid` + `TIOCSCTTY`), so `Ctrl+C` works through the line discipline; the PTY is retained
across mode switches and dock hides (tested); scrollback is bounded at 2,000 rows.

## 6. Terminal — emulation and rendering

| ID | Finding | Evidence | Level |
|---|---|---|---|
| BM-23 | **The cell grid uses the literal family `"monospace"`**, which is a fontconfig *generic*, not an installed family; gpui resolves families by exact name and remaps only `.SystemUIFont`/`.ZedMono`/`.ZedSans`. The theme's resolved mono face (`cx.theme().mono_font_family`) is ignored. Result: proportional fallback glyphs inside fixed 7.8 px cells. | `terminal/mod.rs:29-31, 425, 536`; pinned `gpui-pre-wgpu/cosmic_text_system.rs:386-393` | Strong (visual confirmation pending) |
| BM-24 | **No font measurement anywhere**; `CELL_WIDTH`/`LINE_HEIGHT` are constants, so the PTY grid and the drawn grid disagree whenever the font advance differs from 7.8 px. Cursor and selection math inherit the error. | same | Strong |
| BM-25 | **Whole visible grid rebuilt per frame**: one `div` and one `SharedString` per cell, up to 100,000 elements at the clamp. | `terminal/mod.rs:345-401, 607-628` | Strong |
| BM-26 | **Parser mutex == render mutex**, no backpressure, no flood guard. | `lib.rs:125-141`, `terminal/mod.rs:353-398` | Strong |
| BM-27 | **DECCKM ignored**: arrows/Home/End always send normal-mode sequences, so `vim`, `less`, `fzf`, `htop` misbehave. One-line fix (`vt100::Screen::application_cursor()`). | `terminal/mod.rs:652-741` | Strong |
| BM-28 | **No bracketed paste, no mouse reporting**, `Ctrl+V` never reaches the child, `Ctrl+Shift+C` claims copy (so no `SIGINT` on that chord). | `terminal/mod.rs:321-341, 544-591` | Strong |
| BM-29 | Selection is viewport-only (`GridPoint` has no scrollback coordinate) and is cleared by scroll or input; multi-screen copy of build output is impossible. | `terminal/mod.rs:60-79, 581-590` | Strong |

## 7. Terminal/output performance

Measured? **No** (needs a window). Source-derived budget, per live terminal:

- 62.5 foreground wakeups/s + 62.5 `waitpid(WNOHANG)`/s while the session exists — including
  when hidden (`terminal/mod.rs:236-266`).
- ≤ 62.5 repaints/s, each rebuilding the full grid (BM-25).
- Reader thread: one parse lock per 16 KiB chunk, unbounded loop (no budget, no yield).
- One extra full grid rebuild of *element* allocation per repaint; no damage tracking.

Measurement plan (E2E, see `e2e-verification-plan.md` scenario `B-E2E-03`): run `yes`,
record `perf top` split by thread (`astrolith-terminal-reader` vs UI), `strace -c` futex
count on the UI thread, and p99 of the `with_screen` closure. Expected: UI thread blocked
on the parse mutex dominates p99.

## 8. Terminal/view lifecycle defects

**BM-30 (P2, Strong).** `ToggleTerminal` from Writing/Preview enters Build and then toggles
the bottom dock — which, if the Build layout remembered it visible, closes it
(`shell.rs:2188-2194` + `apply_layout` `1657-1716`). From the user's view: nothing appears.

**BM-31 (P2, Strong).** `TogglePreview` and `ToggleTerminal` both toggle
`PanelKind::Preview`, whose panel is mode-selected (`shell.rs:1684-1712`). In Build,
"Toggle preview" opens the terminal and the menu check-state reports terminal visibility
(`commands.rs:1071`).

**BM-32 (P2, Strong).** **Focus is never moved off the terminal when it leaves the screen.**
The keystroke interceptor checks only `focus.is_focused(window)` (`terminal/mod.rs:100-113`);
gpui focus is pure handle equality (`gpui-pre/src/window.rs:514-516`); `remove_dock` performs
no handoff (`gpui-base/dock/dock_area.rs:271-284`). Leaving Build with terminal focus can
route every keystroke into a hidden shell.

## 9. Editor ↔ LSP ↔ terminal ↔ preview integration

Traced end-to-end (16-step workflow). Findings:

**BM-33 (P1, Strong).** **Two independent Astro dev servers, no bridge.** The terminal's
only observability is `revision` + `with_screen`, consumed solely by `TerminalPanel`
(`lib.rs:169-185`); the preview spawns its own `astro dev --port <free>`
(`preview/src/server.rs:359-431`). The canonical Build loop — run the dev server in the
terminal, look at Preview — cannot work: the preview loads *its own* server on a different
port. Running both means two compilers, two watchers, two HMR sockets.

**BM-34 (P2, Strong).** **The preview server is not stopped when preview leaves the
screen** (`preview/mod.rs:325-333`), so a server started in Writing keeps running in Build
— exactly when a second one is likely to be started.

**BM-35 (P2, Strong).** **Each saved edit costs two independent filesystem scans while in
Build**: index rescan (content) and a full `FileBrowser` walk (navigation)
(`project_discovery.rs:4516` → `:1116-1123`). The watcher and the scanner also disagree
about ignored directories (`files.rs:15-28` vs `watcher.rs:540-543`), so `dist/` and
`build/` writes trigger rescans whose result cannot change.

**BM-36 (P2, Strong).** **Diagnostics exist only while Build is visible and only for the
active file**, because stopping the server clears the diagnostic set. The "edit, run, look"
loop therefore loses its error information exactly when the user switches away to run
something.

Verified-good integration behaviour: preview attachment does not restart a running server
and re-syncs only when `(index_revision, active_path, mapping)` changes
(`shell.rs:1969-1998`); mode switching never re-reads a file or recreates a session; the
terminal survives mode changes (tested).

## 10. Reference comparison (Build Mode specifics)

Source: `/tmp/zed` @ `c7bea34`, read by `evidence/zed-reference.md`.

| Concern | Zed does | Astrolith does | Verdict |
|---|---|---|---|
| Server identity | one server per (worktree, server name, toolchain, settings), deduped by `LanguageServerSeed`, refcounted `OpenLspBufferHandle` (`crates/project/src/lsp_store.rs:283-289, 5374-5460`) | one per open document | **Adopt (simplified to per-project)** |
| Cancellation | cancel-on-drop sends `$/cancelRequest` and removes the timeout handler (`crates/lsp/src/lsp.rs:1594-1622`) | none | **Adopt** |
| Stale responses | retains the last 10 buffer snapshots per (buffer, server, version) so a late response can be mapped to the right text | converts against current text, ignores version | **Adopt (version check only; skip snapshot retention)** |
| Sync | `didChange` built from `edits_since`, FULL fallback (`lsp_store.rs:9710-9778`) | always FULL | **Adapt** (incremental only if a real server shows a cost) |
| Diagnostics | merged per (worktree, path, server), replaced only for that key (`lsp_store.rs:10285`) | per session, URI ignored | **Adopt** |
| Terminal output | 4 ms coalescing drain, wakeup collapse, 100-event cap (`crates/terminal/src/terminal.rs:1420-1500`) | 16 ms revision poll | **Adopt (batching shape)** |
| Terminal render | viewport cell extraction from the terminal model, painted cells (`alacritty.rs:904-955`) | one `div` per cell per frame | **Adapt** (keep `vt100`, change the paint path) |
| Terminal PTY teardown | alacritty `tty` + dedicated event loop, resize through one channel (`terminal.rs:1741-1765`) | `portable-pty` + pid-only kill | **Adopt** (group kill + non-blocking teardown) |
| Scrollback | 10,000 default / 100,000 max | 2,000 fixed | **Defer** (config, not correctness) |

Not adopted: Zed's MultiBuffer/Project/rope stack, its per-server task-permission system,
and its crates split. Astrolith's document model is smaller and measurably fine
(`§2.1` of the performance audit).

## 11. Build Mode verdict

The **structure** is right: one authoritative document, mode as presentation, retained
services. The **implementations** under it are thin in the places that matter:

1. LSP is a per-document, uncancellable, untimed client with URI-blind diagnostics —
   correct enough to look working, wrong enough to show other files' errors.
2. The terminal is a correct PTY wrapper with a rendering path that cannot scale and a
   teardown path that can block the UI.
3. The Build loop the mode exists for (edit → dev server → preview) is not connected.

Recommended order (see roadmap phase 4): diagnostics routing and one-server-per-project
first (they are wrong-data bugs), then cancellation/timeouts, then terminal output batching
and teardown, then the terminal↔preview bridge, then the emulation gaps (DECCKM, paste,
mouse) which are cheap and high-visibility.
