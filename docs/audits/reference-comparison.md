# Reference comparison: Zed and Astro Editor

Source-level comparison at pinned commits. Both reference trees were cloned read-only and
read directly; no claim here comes from marketing pages or memory.

| Reference | Commit | Tree | Read |
|---|---|---|---|
| Zed | `c7bea343838f42b4c14151393e41f0bbe033c95e` | `/tmp/zed` (sparse: `gpui`, `editor`, `project`, `language`, `lsp`, `terminal`, `workspace`, `rope`, `text`, `multi_buffer`, `language_tools`, `project_symbols`, `docs`) | `evidence/zed-reference.md` (705 lines, 34 findings) |
| Astro Editor | `3af8e9bf24b03fe42b3dad9bf632aa6109f17294` | `/tmp/astro-editor` (Tauri 2 + React 19 + TypeScript 7 + `src-tauri`) | `evidence/astro-editor-reference.md` (468 lines, 30 findings) |
| Astrolith | `326eeec5bcc69d90ea6af6fd50ab8f214b97e4b8` | this repository | all evidence files |

Scale honesty: Zed is ~1,000× Astrolith's Rust LOC and solves multi-worktree collaboration, remote
editing, GPUI itself and an extension system. Astro Editor is a Tauri/React app with no dev
server and no preview. Neither is a template; only mechanisms that address a *measured*
Astrolith problem are proposed for adoption.

## 1. Process and architecture models

| | Astrolith | Astro Editor | Zed |
|---|---|---|---|
| Runtime | one native GPUI process, Tokio service runtime (2 workers, 2 blocking slots) | Tauri: one Rust process + one WebView; all editing state in JS | one native GPUI process, foreground + background executors |
| Authority over document text | `DocumentBuffer.current_text` (Rust) | Zustand store in the WebView; Rust only reads/writes bytes | `Buffer` rope (Rust) |
| IPC | none | 42 specta commands (`Result<T, String>`), 24 untyped events | none |
| Astro discovery | tree-sitter parse of `content.config.ts` + self-run `astro sync` | regex/brace-matching parse, silent directory-scan fallback, **never runs sync** | n/a |
| Preview | own supervised dev server + external browser / optional WebView | **none by product decision** | n/a |

**A-1 (confirms Astrolith's choices).** Astro Editor's split — preference *resolution* in
TypeScript, preference *bytes* in Rust — is the inverse of Astrolith's typed
`GlobalPreferencesV2`/`ProjectPreferencesV1` envelopes, and it loses error kinds: every command
returns `Result<T, String>` (`src-tauri/src/bindings.rs:16-84`), so the UI cannot branch on
"not found" vs "permission denied" vs "conflict". Astrolith's typed domain errors are the
better design; **do not adopt** the string-error IPC.

**A-2 (adopt).** Astro Editor's **watcher generation guard** is the one mechanism worth copying.
`watcher.rs:32,38,41,205-260`: a monotonic generation per start, a 500 ms quiet window, a 300 s
safety rescan, and a re-check of generation ownership under the registry lock before a loop
continues — so a stopped watcher cannot resurrect itself after a project switch. Astrolith has
bounded registration and a 100 ms merge window (`astrolith-workspace/src/watcher.rs:25`) and
token ordering for index results, but **no generation check inside the watcher loop**; the
failure mode (two loops emitting after a switch) is the exact bug the reference had and fixed.
Cost: ~1 h. Risk: low.

**A-3 (reject).** Astro Editor's **save model**: `std::fs::write` with no read-back
(`commands/files.rs:313`), dirty-wins over disk (`src/hooks/useFileChangeHandler.ts:61`), and a
3 s "own save" echo window (`:14,61`). An external edit to a dirty file is silently discarded
on the next autosave. Astrolith's baseline-compared `SaveOutcome::Conflict` with atomic
temp+fsync+rename (`astrolith-document/src/save.rs:137-235`) is strictly better — keep it, and
fix Astrolith's *own* conflict dead-end (`DOC-02`) rather than importing this.

**A-4 (adopt, small).** Astro Editor's **dev-only agent bridge**
(`agent_bridge/mod.rs:29`: token-guarded local HTTP server that evaluates JS and snapshots the
webview) is a testability multiplier Astrolith lacks. The transferable half — a token-guarded
debug endpoint that can trigger commands and read state — is adaptable to GPUI and would give
the E2E plan a scriptable surface without a full UI-automation framework. Cost: ~1 day for a
debug-only `single_instance`-shaped socket command channel. Risk: must be `cfg(debug_assertions)`
+ token, exactly as the reference does.

## 2. Rendering and the writer

**Z-1 (adopt as a rule, no code).** Zed's invalidation is per entity per window
(`crates/gpui/src/app.rs:3166-3190`, `window.rs:171-192`), and a clean view's `render` is
skipped entirely, replaying the previous frame's prepaint ranges
(`crates/gpui/src/view.rs:533-570`, `window.rs:4244-4310`). Astrolith's entity layout matches the
first half (`Shell` owns 18 separate entities), but `Shell` **is the window root**, so it
re-renders on every frame of anything (verified in the pinned
`gpui-pre/src/window.rs:3720-3721`), and only dock panels are `.cached(...)`
(`gpui-component/src/dock/tab_panel.rs:789-807`). Consequence and fix: move project-sized work
out of `Shell::render` (`PERF-02`), do not import Zed's dirty-view machinery — `gpui-base 0.8.1`
does not expose it.

**Z-2 (adapt, deferred).** Virtualization: `uniform_list` renders the visible index range only
(`crates/gpui/src/elements/uniform_list.rs:473-500`); `List` adds measured heights, `splice`,
overdraw and follow-tail (`elements/list.rs:490, 310-321, 1081-1210`). Astrolith's lists already
use `ListState` over `v_virtual_list` (`gpui-component-0.8.1/src/list/list.rs:564-600`); the
writer surface does not (it builds every block, `writer_surface.rs:466-536`). Adopting
`ListState` for prose is **not** free: the writer needs one continuous selection/caret and a
shared geometry map across blocks, which `ListState` does not provide. Verdict: adopt
**windowed block iteration** (compute the visible range from `ScrollHandle` ÷ measured block
heights, as `crates/editor/src/element.rs:8740-8774` does) without adopting `ListState` itself.
Cost 2–3 days; risk: caret/selection mapping (`collect_leaf_geometry`, `location_at`,
`source_at`). Measured justification exists (`SHELL_OPEN` 56.6 ms at 1 MiB), so this is
**Adopt**, not Defer.

**Z-3 (adapt).** Incremental edit instead of full-document passes. Zed edits a rope and
publishes only changed regions (`crates/text/src/text.rs:870-901`). Astrolith does 6 full copies
+ 2 full diffs per keystroke (`session.rs:1351-1368`), measured at 16 ms/1 MiB. The measured
projection diff is cheap (0.12 ms), so the target is the *copy* pattern, not the diff algorithm:
replace `EditorState::text().to_string()` + full-source rebuild with a `replace_range` driven by
the editor's change range mapped through `WriterProjection.segments`. Keep the current
whole-document path as the fallback for unmappable edits and the `refused` latch as the safety
net. Cost ~1 day; risk: the refused-edit invariant must stay byte-exact (it has tests).

**Z-4 (adapt).** Bounded synchronous parse then background. Zed tries a 1 ms sync parse, falls
back to `background_spawn`, and re-parses if the buffer moved (`crates/language/src/buffer.rs:1177-1204, 1943-2020`).
Astrolith parses the whole WYSIWYG document inside `render` (`writer_surface.rs:94`). Adopt the
shape, but key the background result on `DocumentRevision` (Astrolith already has the token
pattern in `accepts_index_result`). Cost ~1 day; risk: one frame of stale blocks after an edit.

**Z-5 (reject).** `MultiBuffer`, `DisplayMap`, `rope`, and `Anchor` machinery. Astrolith has one
buffer per document and a projection that already plays the display-map role
(`astrolith-document/src/projection.rs:76-81`). Nothing measured justifies a rope conversion;
`String` is correct at the 8 MiB cap.

**Z-6 (adopt, invariant only).** Zed stores selections as 16-byte anchors so edits never rewrite
them (`crates/text/src/anchor.rs:9-32`, `selections_collection.rs:28-38`). Astrolith keeps byte
offsets and converts between visible and source space in **two** helpers
(`session.rs:1667-1680`). Do not adopt anchors; collapse the two conversions into one function
with a round-trip test. Cost <1 h.

## 3. LSP

| Concern | Zed | Astrolith | Verdict |
|---|---|---|---|
| Server identity | `LanguageServerSeed{worktree,name,toolchain,settings}` + refcounted `OpenLspBufferHandle` (`crates/project/src/lsp_store.rs:283-289, 5374-5460`) | one per document (`session.rs:1530-1600`) | **Adopt** (key = project root + language) |
| Request cancellation | drop-of-future sends `$/cancelRequest`, handler removed on timeout (`crates/lsp/src/lsp.rs:1594-1622`) | no timeout, no cancel, `pending` grows (`client.rs:211-229, 389`) | **Adopt** (~½ day) |
| Stale responses | 10 snapshots retained per (buffer, server, version) (`lsp_store.rs:3500-3540`) | converts against current text; `version` ignored | **Adapt**: revision guard, not history |
| Diagnostics | merged per (worktree, path, server) (`lsp_store.rs:10285-10375`) | replaces the whole set, ignores `uri` and `version` (`lsp.rs:104-116`) | **Adopt** version + URI check now |
| Sync | `edits_since` with FULL fallback (`lsp_store.rs:9710-9778`) | always FULL after 150 ms (`client.rs:716-724`) | **Defer** (bandwidth is not the cost; the UI-thread clone is) |
| Lifecycle | rate-limited start, explicit stop, restart task list (`lsp_store.rs:400, 13340-1352`) | start/stop per mode, restart per tab switch | **Adopt** the registry; keep Astrolith's `lsp_unavailable` latch |
| Scope | 18,808-line store with workspace folders, dynamic registration, pull diagnostics | 1,145-line client | **Reject** the extra surface |

**Z-7 (adopt, highest value).** One server per (project root, language) with refcounted open
documents is the single highest-value reference mechanism for Build Mode: it removes the
per-tab process churn (BM-02) and makes URI-scoped diagnostics tractable. Astrolith's client
already accepts multiple documents (`ClientHandle::open(uri, language, text)`,
`client.rs:184-208`), so the change is a `HashMap<(generation, LanguageId), LspClient>` plus a
refcount, not a new protocol stack. Cost ~1 day; risk: process lifetime — the map owner must
outlive sessions, and `stop_lsp` becomes a decrement. Test with the existing fake-server harness
(`client.rs:951-1030`): open two documents, assert one spawn.

**Z-8 (reject).** Dynamic capability registration, workspace folders, pull diagnostics, upstream
LSP proxying. Astrolith has one root, one language per document, push diagnostics. Not a
scaling problem at this scope.

## 4. Terminal

| Concern | Zed | Astrolith | Verdict |
|---|---|---|---|
| Output delivery | first event immediate, then a 4 ms drain, wakeup collapse, 100-event cap (`crates/terminal/src/terminal.rs:1420-1500`) | reader thread + `AtomicU64` revision polled every 16 ms (`astrolith-terminal/src/lib.rs:122-142`, `terminal/mod.rs:231-275`) | **Keep** Astrolith's pull model (simpler, no cross-thread notify) |
| Paint | viewport cells from the terminal model → text runs (`alacritty.rs:904-955`) | one `div` + `SharedString` per cell, rebuilt per frame (`terminal/mod.rs:345-414, 606-628`) | **Adopt**: one custom element, or at minimum run coalescing |
| PTY | alacritty `tty`, dedicated event-loop thread, one resize channel (`alacritty.rs:180-238`, `terminal.rs:1741-1765`) | `portable-pty`, pid-only kill, blocking join (`lib.rs:277-293`) | **Adapt** teardown (group kill + non-blocking), keep the thread model |
| Scrollback | 10,000 default / 100,000 max (`terminal.rs:891-892`) | 2,000 fixed (`lib.rs:27`) | **Adapt** (10,000 + preference) |
| Input | table-driven `mappings/keys.rs` (459 lines) | ~70-line `match`, no DECCKM/paste/mouse | **Adapt** DECCKM + bracketed paste only |

**Z-9 (adopt, step 1 cheap).** Before any custom element: skip trailing blank cells with the
default background and coalesce runs with identical style. That alone typically removes 60–80 %
of the elements on an 80-column line, in ~15 lines, with no behavioural risk. Then, if still
measured slow, one custom `Element` that shapes each visible row once per revision.

**Z-10 (adapt).** Scrollback of 2,000 lines truncates most `npm run build` output; Zed's 10,000
is the practical floor. Memory is bounded by `vt100`'s per-cell struct (needs a check before
committing, estimated 10–25 MB at 10k × 120).

## 5. Concurrency, cancellation, shutdown

**Z-11 (keep, and enforce).** Astrolith's `runtime::spawn` bridge
(`AbortOnDrop` constructed before polling, `runtime.rs:77-82`) fixes a real bug Zed does not
face (tokio tasks are not cancelled by dropping a GPUI task). Recommendation: add a Clippy
`disallowed_methods` entry for raw `tokio::spawn` in `astrolith-gpui`, otherwise the invariant
rots.

**Z-12 (keep).** `flush_on_quit` bypasses GPUI's 200 ms quit-observer budget
(`runtime.rs:125-134`) — the correct workaround, documented and tested. Add a log line when the
2 s runtime shutdown timeout is exceeded (currently silent).

**Z-13 (keep).** The 2-worker/2-blocking-slot service runtime is right-sized for this workload.
Raising it is not justified by any measurement in this audit; a Node-spawning workload that
awaits two subprocess pipes can exhaust it (Hypothesis) — measure `PERF` gaps before changing.

## 6. Filesystem and scanning

| Concern | Zed | Astrolith | Verdict |
|---|---|---|---|
| Event batching | drain all ready batches, one `process_events`, 100 ms latency (`worktree.rs:4640-4645, 84`) | 64-slot channel, 100 ms quiet window, `needs_rescan` on overflow (`watcher.rs:180-200, 226-272`) | **Keep** |
| Reaction to a batch | per-path targeted refresh (`worktree.rs:2121-2150, 5687`) | whole-project rescan: `inspect_project` + `scan_collections` (`project_discovery.rs:3237-3268`) | **Adapt**: batch carries paths |
| Ignore rules | per-directory ignore stack + lazy `UnloadedDir` (`worktree.rs:3090, 6555`) | name-based skips + hard caps | **Reject** gitignore machinery; **adopt** a `.gitignore`-aware skip inside collection dirs and diagnostic text naming the offending path |
| Scan convergence | `scan_id` + `wait_for_snapshot` (`worktree.rs:823-915`) | index request token protocol (`project_discovery.rs:3300-3310`) | **Keep** (Astrolith's half is the important one) |
| Root liveness | 5 s canonicalise + moved/deleted report (`worktree.rs:90, 4615-4630`) | none | **Defer** |

**Z-14 (adapt, highest-value workspace change).** `WatchBatch` is three booleans
(`watcher.rs:35-43`) with no paths, so one saved file re-reads the frontmatter of every entry in
every collection (`index.rs:344-400`). Make the batch carry a deduped path list (falling back to
`needs_rescan` above ~1,000 paths) and re-index only those entries. Cheapest first step: a
*path-filtered* `scan_collections` (reuses the walker, no index surgery), ~1 day; full index
splice ~2–3 days.

**Z-15 (adopt, small).** After a successful save, feed the path into the targeted reindex
instead of waiting for the watcher batch (≥100 ms quiet + 100 ms poll). Removes a visible
lag and one rescan per save.

## 7. Summary of verdicts

| Verdict | Items | Measured/observed justification |
|---|---|---|
| **Adopt** | LSP registry per (root, language); LSP timeout+cancel+pending cleanup; diagnostics URI+version check; watcher generation guard; watcher paths → targeted reindex; terminal run coalescing (then custom element); post-save direct reindex; single selection-conversion helper | LSP-01/07/09/26, PERF-10, PERF-12, doc E-01 |
| **Adapt and simplify** | writer visible-range windowing (without `ListState`); incremental splice (without rope); bounded sync parse keyed by revision; terminal teardown via group kill; scrollback size; DECCKM + bracketed paste; debug command socket | PERF-01/04/09, BM-16/17/27 |
| **Defer** | incremental `didChange`; per-block entities; settings virtualization | no measured cost |
| **Reject** | MultiBuffer/Project/rope/anchors; dynamic LSP registration/workspace folders/pull diagnostics; gitignore ignore-stack; Zed's whole-document dirty-view machinery (not exposed in `gpui-base 0.8.1`); Tauri string-error IPC; Astro Editor's last-writer-wins save; Astro Editor's regex config parser; Astro Editor's "don't run `astro sync`" | no problem to solve at Astrolith's scale, or strictly worse than what exists |

**What Astrolith does better than both references and must not regress:** baseline-compared
atomic saves; one complete-source undo stack covering hidden frontmatter; refusal to guess at
unparseable Astro config; self-run `astro sync` with content-fingerprint freshness; typed
preference envelopes with unknown-field preservation; process-group dev-server ownership;
`AbortOnDrop` service bridging.

## 8. Limitations of this comparison

- `crates/terminal_view` was outside the Zed sparse checkout, so "Zed paints the terminal as one
  element" is inferred from `make_content` returning viewport cells
  (`alacritty.rs:904-955`) and is marked **Strong**, not Confirmed.
- `crates/fs` and the concrete `Scheduler` implementation were outside the checkout; thread-count
  claims rely on executor signatures only.
- Astro Editor was read at the level of its Rust commands, stores, hooks and docs; its React
  component tree was sampled, not exhaustively read.
- No reference behaviour was executed; every reference claim is static source reading.
