# Executive summary

Astrolith at `326eeec` (branch `dev`), audited 2026-11-05. Full reasoning and citations in the
eight sibling documents; raw subsystem evidence in `docs/audits/evidence/`.

Baseline gate: `mise run check` → exit 0, **915 passed / 0 failed / 8 ignored** (all 8 are the
declared workload/gate tests), Astro fixture oracle `PASS`.

---

## 1. What is Astrolith's current architectural quality?

**A coherent service layer under an over-centralised UI layer, with sound data invariants but
three defects that break them.**

- The six domain/service crates (`core`, `document`, `astro`, `workspace`, `preview`, `lsp`,
  `terminal`) have clear ownership, typed errors, and genuinely good invariants: complete-source
  authority, revision-tokened stale-result rejection, atomic baseline-compared saves, one undo
  stack covering hidden frontmatter, process-group dev-server ownership, and a cancelling
  async bridge (`runtime.rs:77-134`).
- The GPUI crate holds 51 % of the code (50.7k of 92.6k lines). `project_discovery.rs` is 13,451
  lines with a 78-field entity and seven unrelated concerns; `shell.rs` owns 18 entities and
  ~70 fields; the centre document panel has two writers; layout state has three authorities.
  The highest-severity defects cluster exactly in the extracted-but-not-separated save/watcher/
  recovery block.
- Rendering is not windowed on the writing path, so **cost is a function of document size, not of
  what changed**. This is the single architectural property that most limits the product.

Grade: **service design A-, UI composition C, rendering architecture D, data-safety paths C-.**
Not a rewrite candidate; a target-extraction and correction candidate.

## 2. The five most serious problems

| # | Problem | Why it is serious | Evidence |
|---|---|---|---|
| 1 | **The WYSIWYG writer re-renders every block every frame** (`writer_surface.rs:466-536`) | The repository's own resize workload on a 1.3 MB entry **did not complete 90 frames in 33 minutes** at 100 % CPU (lower bound ≈22 s/frame). Even the code-editor path is 56.6 ms for one 1 MiB frame. Typing and scrolling degrade with article length. | §2.5/PERF-01, Confirmed |
| 2 | **A save conflict has no exit, and the documented remedy deletes the recovery record** (`DOC-02`); **recovery restore adopts the current disk as baseline** (`DOC-03`) | Two data-loss-shaped paths: a conflicted user who follows the UI loses in-memory edits, and a post-crash autosave can overwrite a disk edit made after the crash. | Strong |
| 3 | **LSP diagnostics ignore `uri` and `version`** (`session.rs:1577` → `lsp.rs:87-116`), and **each Build document owns its own server** (`session.rs:1530-1600`) | Wrong errors shown on the wrong file (silent wrong data), N servers for N tabs, no timeout and no `$/cancelRequest`, so a wedged server leaks requests forever. | Confirmed |
| 4 | **Terminal teardown blocks the UI thread and kills only the shell pid** (`lib.rs:277-293`) | `reader.join()` on the UI thread during project switch / restart / quit can freeze the window; without `killpg` the shell's children can survive as orphans. | Strong (freeze = Hypothesis) |
| 5 | **No application menu is rendered on Linux** (`gpui-pre-linux` only stores `set_menus`) | A large share of documented UI paths ("File → Open project…", "Writer menu", "Help menu") do not exist in the shipped product. 73 registered commands remain reachable by palette/shortcuts, so the capability exists without its documented path. | Strong |

Adjacent, and the *trigger* for #2: `DOC-01` — every own save marks the open document as
"Changed on disk" because `disk_fingerprint` is captured once at load and never updated
(`session.rs:171`, `project_discovery.rs:3325`). Users are pushed into the conflict path.

## 3. The five highest-value optimization opportunities

1. **Window the writer surface** (PERF-01): render only the visible block range plus the caret
   block, derived from scroll offset ÷ measured heights. Cost 2–3 days; risk concentrated in
   caret/selection geometry. This is the only change that removes the 22 s/frame class of failure.
2. **Make `Shell::render` cheap and non-copying** (PERF-02/03): move menu/availability/font work
   into change handlers and stop `projection.visible.clone()` per frame. Cost <1 day. Removes
   O(document) cost from every unrelated frame (terminal output, palette, splitter drag).
3. **Incremental edit pipeline** (PERF-04): splice the editor's change range through the
   projection into the source instead of 6 full copies + 2 full diffs; keep the whole-document
   path as the fallback and the `refused` latch as the correctness guard. Measured 16 ms/1 MiB
   today, target ≤4 ms.
4. **One LSP server per (project, language) with URI-scoped, version-checked diagnostics, plus
   timeouts and cancellation** (BM-02/06/07/09). Cost ~1.5 days. Removes the per-tab process
   churn and the wrong-file diagnostics.
5. **Terminal output and teardown** (PERF-09/10/11, BM-16/17/22/23): event-driven output instead
   of a permanent 16 ms poll, run-coalesced cells instead of one `div` per cell, a
   double-buffered screen instead of a shared mutex, process-group kill, measured cell metrics
   from the theme's mono face. Cost 2–3 days. Fixes responsiveness, orphans and the visibly wrong
   proportional glyphs.

Not in the top five but cheap and high-visibility: the ~500-line configuration-migration
deletion (§6), the watcher path-carrying batch (one save no longer re-reads every entry), and the
`no menu` decision.

## 4. Which Zed designs should be adopted or adapted?

Source: `crates/*` at `c7bea34`; full table in `reference-comparison.md`.

**Adopt**
- One language server per (worktree, server name, toolchain), refcounted per open buffer
  (`project/src/lsp_store.rs:283-289, 5374-5460`) → simplify to (project root, language).
- Cancel-on-drop that emits `$/cancelRequest` and removes the timeout handler
  (`lsp/src/lsp.rs:1594-1622`).
- Diagnostics merged/replaced per (path, server) with version awareness
  (`lsp_store.rs:10285-10375`) — the version half now.
- Watcher batches that carry paths and trigger targeted refresh
  (`worktree.rs:2121-2150`) instead of a whole-project rescan.

**Adapt and simplify**
- Visible-range rendering (`editor/src/element.rs:8740-8774`) *without* `ListState`/`MultiBuffer`:
  the writer needs one continuous selection and a shared geometry map.
- Bounded synchronous parse then background, re-parsed if the buffer moved
  (`language/src/buffer.rs:1177-1204, 1943-2020`) — keyed on Astrolith's existing `DocumentRevision`.
- Terminal output coalescing (`terminal/src/terminal.rs:1420-1500`) — but keep Astrolith's
  16 ms revision poll, which is simpler and avoids cross-thread notification; fix the *paint*
  path instead.

**Reject**
- MultiBuffer, Project, rope, `Anchor`s, DisplayMap; edit-stable anchors are unnecessary at 8 MiB.
- Dynamic LSP registration, workspace folders, pull diagnostics, 18.8k-line `lsp_store`.
- Per-directory gitignore stacks and lazy directory loading — Astrolith indexes Astro content,
  not a repository. (A `.gitignore`-aware skip *inside* collection directories is worth its own
  small change.)
- Zed's `dirty_views` reuse machinery: not exposed by `gpui-base 0.8.1`.

## 5. Which Astro Editor designs should be adopted or adapted?

Source: `/tmp/astro-editor` at `3af8e9b`; full table in `reference-comparison.md`.

**Adopt**
- The watcher **generation guard** (`src-tauri/src/commands/watcher.rs:32,38,41,205-260`): a
  superseded loop exits instead of resurrecting after a project switch. ~1 hour.
- The debug control bridge as a testability seam — token-guarded, `cfg(debug_assertions)`, local
  socket. It is the only realistic way to script a GPUI window for L2 E2E.

**Reject**
- Its save model (plain `fs::write`, dirty-wins, 3 s echo window) — Astrolith's baseline-compared
  atomic save is strictly better.
- Its discovery fallback (regex + brace counting; on parse failure, treat every directory as a
  collection) and its refusal to run `astro sync`. Astrolith's tree-sitter parse + self-run sync +
  content fingerprint is strictly better and is a feature to protect.
- Its `Result<T, String>` IPC, which erases error kinds.
- Its stringly-typed Rust→JS event bus.

**Note:** Astro Editor has no dev server and no preview *by product decision*, so it has nothing
to teach about `astrolith-preview`'s hard parts; the only transferable lesson there is scope
discipline.

## 6. What configuration migration code should be removed?

Seven one-shot migrations plus one legacy file import target formats that existed only between
2026-11-01 and 2026-11-04, with **no tag, no release and no installer** in the repository:

`settings/migrate.rs` (v1 import), `migrate_neutral_palette`, `migrate_page_width`,
`migrate_writing_typography`, `migrate_interface_scale`, `migrate_mode_layouts`,
`migrate_project_layout` — ≈500 production lines, 5 backup filename families, 7 tests.

Also remove: six provably unreachable `!writable` guards inside those migrations, the
`interface_scale` tombstone, and the dead schema fields `telemetry_enabled`, project-file
`display_name`/`last_opened_at`.

Replace with **one versioned `upgrade.rs`** (`version: 3`), one backup, run once, applying the
final defaults only where the stored value equals the retired default. Keep — do not touch:
corruption quarantine, future-version read-only, unknown-envelope preservation, atomic writes,
recovery records, per-collection overrides, the project registry and its stable IDs.

Deleting this code loses **no document, project, collection or preference value**; it only stops
re-baselining four old defaults for a user who never opened Preferences. Full deletion list, data
safety table and ten E2E scenarios: `config-cleanup.md`.

## 7. What should explicitly remain unchanged?

1. Complete source as the single authority; editor text as a projection.
2. One undo stack for body **and** hidden frontmatter, with the pinned editor's stack suppressed.
3. Lossless frontmatter patching (comments, key order, unknown keys, CRLF preserved).
4. Static Astro discovery that refuses to guess, plus a self-run `astro sync` with
   content-fingerprint freshness.
5. Atomic, baseline-compared, per-canonical-path-serialized saves.
6. `runtime.rs`: cancelling bridge, bounded blocking lane, quit-time flush that survives the
   200 ms observer budget.
7. Preview process-group ownership, generation-reconciled lifecycle, capability matrix.
8. Typed preference envelopes with unknown-field preservation, explicit `false`/`0`/`null`
   inheritance, corruption quarantine, future-version read-only.
9. The terminal's PTY model (session leader + controlling tty, bounded scrollback, revision
   counter) — fix paint and teardown, do not replace the design.
10. The 73-command registry with generated labels/availability/menus and the shortcut reference.

## 8. What are the most significant Build Mode risks?

| Risk | Consequence | Evidence |
|---|---|---|
| Diagnostics are URI- and version-blind | Wrong errors shown on the active file; stale publishes overwrite new ones | Confirmed |
| One server per document, killed on tab switch, no timeout/cancel | N node processes for N tabs; wedged servers hang providers; request map grows | Confirmed |
| Terminal teardown joins the reader on the UI thread; pid-only kill | Window freeze on project switch/quit; orphaned shells and zombies | Strong |
| Terminal paints `"monospace"` (a fontconfig alias, not a family) into fixed 7.8 px cells | Proportional glyphs in a fixed grid; cursor and selection math wrong for every session on this host | Strong |
| Terminal poll + full-grid rebuild + shared parser mutex | 62.5 wakeups/s per hidden terminal; flood stalls the UI | Strong |
| Two independent Astro dev servers, no terminal↔preview bridge | The mode's core loop ("run it, look at it") does not work; running both duplicates compilers and HMR | Strong |
| Focus stays on a hidden terminal after leaving Build | Keystrokes go to an invisible shell | Strong (observable = Hypothesis) |
| Diagnostics disappear when Build is left | The error information is gone exactly when the user switches away to run something | Strong |

Build Mode's **structure** (mode as presentation, retained services, one document) is right and
should not be rebuilt. Its **implementations** need the eight corrections above.

## 9. What performance claims have actually been measured?

Release build, headless GPUI harness, Intel Core Ultra 9 185H, CachyOS kernel 7.3.1, Wayland.
`mise run test:projection-perf:release`, `test:editor-perf`, `test:shell-perf:release`,
`test:resize-perf:release`. Raw: `artifacts/e2e/audit-baseline-326eeec/perf.log`.

| Metric | 1 KB | 100 KB | 1 MB | 100 KB single line |
|---|---:|---:|---:|---:|
| Projection diff p50 | 0.0002 ms | 0.008 ms | 0.073 ms | 0.005 ms |
| Writer edit pipeline p50 | 0.85 ms | 2.43 ms | **16.0 ms** | **31.1 ms** |
| Editor `set_value` | 0.80 ms | 6.1 ms | 42.3 ms | 7.7 ms |
| Shell open first frame | 0.77 ms | 5.5 ms | **56.6 ms** | 7.1 ms |
| Input frame p50 (code editor) | 3.2 ms | 4.4 ms | 2.8 ms | 10.4 ms |
| Resize frame, WYSIWYG writer (1.3 MB) | — | — | **> 22 s lower bound** | — |

Also measured: document read 0.77 ms at 1 MiB; 915 passing tests in ~5 s of test time; the resize
workload terminated after 33 min / 100 % CPU / 1.48 GiB RSS.

**Not measured** (and not claimed): cold/warm startup to first frame, peak startup RSS, real
GPU frame time, key→pixel latency, RSS per open document, terminal flood frame times, LSP cold
start and completion P50/P95, HMR latency, macOS/Windows behaviour. Each has a stated method in
`performance-audit.md` §9.

## 10. Recommended implementation order

1. **Phase 0 — harness**: E2E runner + manifest schema, native startup/latency instrumentation,
   CI perf job, record the nine missing baselines.
2. **Phase 2a — P0 correctness** (before any cleanup): `DOC-02` conflict resolution, `DOC-03`
   recovery revision, `DOC-01` fingerprint update, `DOC-04` per-path pending action, `DOC-05`
   save-size refusal. Each a separate commit with an L2 E2E case.
3. **Phase 1 — configuration and debt cleanup**: the `config-cleanup.md` deletion list, one
   versioned `upgrade.rs`, dead fields, then the doc corrections.
4. **Phase 2b — low-risk performance**: borrow-instead-of-clone, cached counts, font resolution,
   watcher hash short-circuit, readiness probe, schema-poll replacement, `[profile.release]`.
5. **Phase 3 — GPUI and concurrency**: writer windowing, shell render diet, incremental edit,
   terminal output pipeline, one walk per batch.
6. **Phase 4 — Build Mode**: diagnostics correctness first, then LSP registry + request hygiene,
   then terminal process life and metrics, then view semantics, then the terminal↔preview bridge.
7. **Phase 5 — architecture seams**: extract `project_discovery.rs` (recovery → load admission →
   save lifecycle → watcher/index → operations), single document-panel writer, one layout
   authority, path-carrying watch batches and per-collection diagnostics.
8. **Phase 6 — features and reliability**: menu strategy decision, `embedded-webview` build or
   doc scoping, command semantics, status bar surfacing, Windows CI gating, build identity.
9. **Phase 7 — verification**: re-run every baseline, publish before/after, full E2E matrix,
   process and memory checks, update architecture and user docs.

Every phase ships independently, ends with `mise run check` plus its E2E manifests, and has a
stated rollback. Details: `optimization-roadmap.md`.

---

## Audit completeness and limits

- **Inspected:** all 10 workspace crates; every file named in each evidence document's scope
  table; `project_discovery.rs` structural inventory (187 production items) with line-by-line
  reading of the save/watcher/recovery/load clusters; `shell.rs` render/entity/mode paths;
  the full LSP client, terminal crate, preview process/server, settings store, and command
  registry; docs, CI, packaging and test inventory.
- **Not deeply read:** copyedit tokenizer internals, component-builder form generation, update
  signature verification internals, macOS/Windows platform modules, the React mock
  (`astrolith-web-mock/`, untracked prototype) and the CLI-level packaging scripts.
- **Runtime evidence:** the repository's own release workloads; the test suite; `ps` observation
  of the resize workload. **No native window was opened**, so IME, focus, scaling, GPU frame time,
  menus and browser launch remain unverified at the native layer, exactly as `AGENTS.md` requires
  them to be labelled.
- **Reference trees** were read at pinned SHAs; `crates/terminal_view` and `crates/fs` were
  outside the sparse checkout and their claims are marked accordingly.
- No production code, dependency, or configuration was modified by this audit. The only files
  added are `docs/audits/**` and the run artifacts under
  `artifacts/e2e/audit-baseline-326eeec/`.
