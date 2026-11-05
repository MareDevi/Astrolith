# Performance audit

Audit date: 2026-11-05, repository `dev` @ `326eeec`. This document separates
**measured** numbers from **source-derived** costs and from **hypotheses** that still
need a native measurement. Nothing here is extrapolated into a claimed speed-up.

## 1. Method

- Build: `release` (`opt-level = 3`), `--locked`, via the repository's own mise tasks.
  `dev`-profile numbers are **not** comparable to anything in another project and are
  reported separately only where both were captured.
- Harness: the repository's existing `#[ignore]` workload tests
  (`mise run test:projection-perf:release`, `test:editor-perf`, `test:shell-perf:release`,
  `test:resize-perf:release`). They run a headless GPUI test window
  (`TestAppContext`), so they measure **CPU work inside layout/render and the document
  pipeline**, not GPU presentation, vsync, input-device latency, or window-manager cost.
- Samples: 60 (diff), 100 (writer, input frames), 90 frames (resize). Percentiles are the
  harness's own (`p50`/`p95`); `max` is reported where it is unstable.
- Raw log: `artifacts/e2e/audit-baseline-326eeec/perf.log`.
  Test suite baseline: `artifacts/e2e/audit-baseline-326eeec/check.log`
  (**915 passed, 0 failed, 8 ignored**).

### Environment

| Item | Value |
|---|---|
| CPU | Intel Core Ultra 9 185H (22 threads) |
| RAM | 30 GiB |
| OS | CachyOS, kernel 7.3.1-1-cachyos |
| Session | Wayland (`WAYLAND_DISPLAY=wayland-1`), XWayland `:1` |
| Rust | rustc 1.99.1 (58cf8f9c3 2026-10-13) |
| GPUI | `gpui-kit 0.8.1` / `gpui-pre 0.3.10` |
| Display / DPI | **not applicable — headless harness** |
| GPU | not used by these measurements |

### Limitations that must be stated with any citation of these numbers

1. Headless: no compositor, no GPU upload, no vsync. Absolute frame times are **not**
   representative of the desktop; the **scaling** with document size is.
2. The harness runs one frame at a time with `window.render_frame(cx)`; a real window
   coalesces input into frames. Typing numbers are therefore per-frame CPU cost, not
   end-to-end input latency.
3. No RSS instrumentation exists; memory claims are structural (**Hypothesis** for exact
   values).
4. Nothing was measured on macOS or Windows.

## 2. Measured results

### 2.1 Projection diff (document pipeline only)

`mise run test:projection-perf:release` → `PROJECTION_DIFF`, 60 samples:

| Input | p50 | p95 | max |
|---|---:|---:|---:|
| 1,105 B (36 lines) | 0.0002 ms | 0.0003 ms | 0.0013 ms |
| 102,476 B (3,453 lines) | 0.0077 ms | 0.0079 ms | 0.0346 ms |
| 1,048,635 B (35,346 lines) | 0.0725 ms | 0.1199 ms | 0.6109 ms |
| 102,457 B (1 line) | 0.0049 ms | 0.0050 ms | 0.0054 ms |

**Reading:** the projection diff/rebuild itself is *cheap* — 0.12 ms at 1 MiB. It is not
the typing bottleneck. This matters because the code *looks* expensive (it allocates a
new source string per edit, `projection.rs:363`); the measurements say the diff is not
where the time goes.

### 2.2 Projected writer pipeline (absorb + history + decorations)

`PROJECTED_WRITER`, 100 samples, per-sample = one simulated edit through
`DocumentSession` (the same work a keystroke triggers before paint):

| Input | p50 | p95 | max |
|---|---:|---:|---:|
| 1,088 B (14 lines) | 0.851 ms | 1.251 ms | 1.517 ms |
| 103,035 B (1,372 lines) | 2.433 ms | 2.869 ms | 3.281 ms |
| 1,048,935 B (13,972 lines) | **16.045 ms** | **17.465 ms** | 19.828 ms |
| 102,457 B (1 line) | **31.061 ms** | **33.638 ms** | 39.548 ms |

**Reading:** per-edit cost is linear in document size and already exceeds the 16.7 ms
frame budget at 1 MiB. A single 100 KB line costs 31 ms — 5× the multi-line 100 KB case —
because whole-line/sentence work dominates when there are no line breaks. Extrapolating
linearly to the accepted 8 MiB cap gives ~125 ms per edit; that is an extrapolation, not
a measurement (**Hypothesis**).

### 2.3 Editor spike (code editor, same content)

`test:editor-perf` → `EDITOR_WORKLOAD` (desktop `--spike=editor`):

| Input | `set_value` | first frame | input frame p50 | p95 | max |
|---|---:|---:|---:|---:|---:|
| 1,051 B | 0.796 ms | 7.479 ms | 3.158 ms | 3.418 ms | 4.025 ms |
| 102,998 B | 6.120 ms | 12.904 ms | 4.406 ms | 4.765 ms | 6.249 ms |
| 1,048,898 B | 42.282 ms | **49.007 ms** | 2.750 ms | 3.813 ms | 4.733 ms |
| 102,420 B (1 line) | 7.671 ms | 13.628 ms | 10.374 ms | 11.346 ms | 15.430 ms |

**Reading:** the pinned editor's *input* path is row-windowed (2.8–4.8 ms even at 1 MiB),
but `set_value` and the resulting frame are O(N) (49 ms at 1 MiB). Every mode switch,
every `set_source_view`, and every programmatic refresh pays that.

### 2.4 Shell open (read → install → first frame)

`test:shell-perf:release` → `SHELL_OPEN`:

| Input | read | install | first frame |
|---|---:|---:|---:|
| 1,160 B (16 lines) | 0.014 ms | 0.019 ms | 0.767 ms |
| 102,515 B (1,414 lines) | 0.078 ms | 0.065 ms | 5.481 ms |
| 1,048,640 B (14,464 lines) | 0.774 ms | 0.741 ms | **56.580 ms** |
| 102,410 B (1 line) | 0.076 ms | 0.058 ms | 7.146 ms |

**Reading:** I/O is negligible. The first frame is O(N) and reaches **56.6 ms at 1 MiB** —
3.4× the 60 Hz budget. Crossing ~300 KB is enough to break the frame budget on this CPU;
that threshold is derived from the two points, not measured.

### 2.5 Window resize with a 1.3 MB entry and an 80-field schema

`mise run test:resize-perf:release` runs 90 full window resizes over an entry built from
`paragraph.repeat(8000)` (≈1.28 MB) with an 80-field collection schema
(`shell.rs:5874-5972`). It prints one `SHELL_RESIZE` line at the end.

**Observed (Confirmed): the workload had not produced its result after 33 minutes of wall time,
having consumed 32 minutes 31 seconds of CPU on one saturated core at a stable 1.48 GiB RSS**
(`ps -o etime,time,pcpu,rss` on `astrolith_gpui-… window_resize_workload`, sampled repeatedly).
It was terminated by the audit (SIGTERM); `perf.log` records the run and the absence of a
`SHELL_RESIZE` line. With ~90 frames and a few seconds of setup, that is a **lower bound of
~22 s per resize frame** for this document.

What this is and is not:

- It is a **direct observation** of the repository's own measured workload failing to complete in
  a time budget that any user would call unusable.
- It is **not** an isolated per-frame measurement: the frame count reached at 33 minutes is not
  reported (the test only prints at the end), and the workload includes the inspector form with
  80 fields, which the open-workload does not.
- The prime suspect is the non-windowed writer surface (§PERF-01 / `writer_surface.rs:466-536`):
  8,000 blocks rebuilt, shaped and laid out on every frame, inside a plain
  `overflow_y_scroll` container. This is consistent with, but not proven by, the observation.
- It also explains why the `SHELL_OPEN` numbers (56.6 ms at 1 MiB) *understate* the writer path:
  `SHELL_OPEN` exercises `Shell::show_document` (the code editor), not the WYSIWYG writer.

Isolation plan: instrument `WriterSurface::render` with a per-phase timer (parse, block build,
shape, layout, paint) and re-run the workload at 1/8 scale (1,000 blocks) to get a per-frame
number and a scaling exponent. Recorded as `PERF-E2E-04`.

Sampling the running test was attempted and **blocked by the host's ptrace policy**
(`kernel.yama.ptrace_scope = 1`, `perf_event_paranoid = 2`); no stack samples were obtained, so
the hot function is *not* identified. This is exactly the kind of gap Phase 0 instrumentation is
meant to remove.

### 2.6 Summary table (measured, release, headless)

| Metric | 1 KB | 100 KB | 1 MB | 100 KB single line |
|---|---:|---:|---:|---:|
| Projection diff p50 | 0.0002 ms | 0.008 ms | 0.073 ms | 0.005 ms |
| Writer edit p50 | 0.85 ms | 2.43 ms | 16.0 ms | 31.1 ms |
| Editor `set_value` | 0.80 ms | 6.1 ms | 42.3 ms | 7.7 ms |
| Shell open first frame | 0.77 ms | 5.5 ms | **56.6 ms** | 7.1 ms |
| Input frame p50 (code editor) | 3.2 ms | 4.4 ms | 2.8 ms | 10.4 ms |
| Resize frame, WYSIWYG writer (1.3 MB) | — | — | **> 22 s (lower bound)** | — |

### 2.7 Dev-profile spot check (partial, non-comparable)

The dev-profile runs (`mise run test:projection-perf`) were started and interrupted after two
data points. They are recorded only to show the debug/release gap and must not be compared with
another project's release numbers:

| Metric (dev profile) | 1 KB | 100 KB |
|---|---:|---:|
| Projection diff p50 | 0.0006 ms | not reached |
| Writer edit pipeline p50 | 1.077 ms | 12.394 ms |

Release is ~5× faster on the writer pipeline at 100 KB (2.43 ms). Debug numbers are not used
anywhere else in this document.

## 3. Findings — rendering

All source citations below are from the pinned GPUI sources
(`gpui-pre-0.3.10`, `gpui-component-0.8.1`) or the repository.

**PERF-01 (P1, Strong + measured).** **The writer surface is not viewport-windowed.**
`WriterSurface::render` iterates `document.blocks` and builds an element for every block
every frame (`writer_surface.rs:466-536`), inside a plain `overflow_y_scroll` container
(`:555-559`); `RichFlow::prepaint` collects and clones geometry for all leaves
(`:793-796`). The measured `SHELL_OPEN` first frame (56.6 ms at 1 MiB) is consistent with
this and with `set_value`'s 42 ms. Sibling lists (`EntryList`, `FileBrowser`) *are*
virtualized (`gpui-component/src/list/list.rs:564-600`), so the fix is local.

**PERF-02 (P1, Strong + measured).** **`Shell::render` runs on every frame of anything,
and does per-frame work proportional to project/document size.** The root view is laid out
every frame (`gpui-pre/src/window.rs:3720-3721`) and `Shell` is the window root
(`shell.rs:164`); only dock panels are wrapped in `.cached(...)`
(`gpui-component/src/dock/tab_panel.rs:789-807`). Per frame `Shell::render`:
`refresh_menus` → `availability` + `collection_items()` (allocates a `Vec` of every
collection, `project_discovery.rs:2578-2599`), `active_frontmatter_key()` → `resolved_for`
(clones preference strings), builds a `ProseStyle` (linear scan over installed font names,
`theme/typography.rs:61-113`), and two `Entity::update` calls (`shell.rs:3116-3193`).
Consequence: a terminal repaint at 62.5 Hz or a palette keystroke pays the same
project-sized work as a document edit.

**PERF-03 (P1, Strong).** **Preview mode clones the whole visible document on every
`Shell` frame** (`shell.rs:3173-3190` → `projection.visible.clone()`), then `set_document`
compares it and early-returns (`rendered.rs:82-90`). With a 1 MiB document, every
unrelated frame — terminal output, splitter drag, palette — copies and compares 1 MiB.

**PERF-04 (P1, Strong + measured).** **Per-edit work is O(document): 6 full-size
allocations and ≥10 full-length scans** (`session.rs:1351, 1357, 1368`,
`projection.rs:363`, `writer_surface.rs:437-449`, `session.rs:1135-1150`), plus a
`Vec<Range>` of 16 bytes per word (`tokens.rs:52-68`) that is discarded after counting.
Measured at 16 ms/1 MiB (§2.2). The individual components are cheap (§2.1); the
aggregate is not, because none of them is incremental.

**PERF-05 (P2, Strong).** **`ProjectWorkspace::render` calls `EntryList::entry_count()`,
which walks every entry** (`project_discovery.rs:6230-6234` → `:381-388`), and rebuilds
the collection list, on every project notify (111 sites). Library search re-filters and
re-sorts the entire collection per typed character with no debounce
(`project_discovery.rs:609-620`, `:221-300`), synchronously from `ListState`
(`gpui-component/src/list/list.rs:290-305`).

**PERF-06 (P2, Strong).** **The writer paints the document twice**: WYSIWYG blocks plus a
hidden full-size `Editor` at `opacity(0.)` (`writer_surface.rs:590-597`), whose own
viewport shapes and paints every frame.

**PERF-07 (P2, Strong).** **Typography resolution per frame**: `configured_prose_font`
does a case-insensitive linear scan over all installed font names and allocates a `String`
(`theme/typography.rs:61-108`), called from `Shell::render` (`shell.rs:3160-3165`).
Bounded by font count; on macOS with a large font list this is the largest fixed per-frame
cost. The theme itself is *not* re-resolved per frame (clean, `theme/mod.rs:86`).

**PERF-08 (P2, Strong).** **File explorer flattens every visible row on every
expand/collapse** (`file_browser.rs:311-390`) — the list is virtualized, the row model is
not.

## 4. Findings — terminal (see `build-mode-audit.md` §5 for the full list)

**PERF-09 (P1, Strong).** **62.5 foreground wakeups/s per live terminal, forever, even
when hidden**: a 16 ms poll timer runs `try_wait` (a `waitpid` syscall) and a revision
check each tick (`terminal/mod.rs:236-266`). The PTY survives mode switches and dock
hides by design (`shell.rs:247-249`, test `terminal_is_project_bound_and_survives_mode_and_dock_changes`),
so this runs for the whole session once a project is open.

**PERF-10 (P1, Strong).** **The whole visible grid is rebuilt as one `div` per cell per
frame** (`terminal/mod.rs:345-401, 607-628`) with a `SharedString` per cell, up to
500 × 200 = 100,000 elements per frame by the clamp at `:277-287`. At the default 80 × 24
that is ~1,920 cells per repaint, ~120k element builds/s at 62.5 fps.

**PERF-11 (P1, Strong).** **Parser mutex and renderer mutex are the same lock**: the
reader thread holds it across `vt100::Parser::process` for a 16 KiB chunk
(`astrolith-terminal/src/lib.rs:125-141`); the UI thread holds it across the entire grid
walk (`terminal/mod.rs:353-398`). Under a flood the UI thread blocks on `std::sync::Mutex`
once per frame. This is the one place where the terminal can stall the editor.

## 5. Findings — filesystem, index, preview

**PERF-12 (P1, Strong).** **Every watcher batch costs 2–3 full tree walks**: `inspect_project`
+ `scan_collections` (`project_discovery.rs:3237-3263`), a second walk inside
`watcher.refresh` (`:3267` → `watcher.rs:292-383`), plus a component walk (`:2112-2138`),
on a 2-permit blocking lane (`runtime.rs:47`). In Build, each save additionally triggers a
file-browser walk (`project_discovery.rs:1116-1123`).

**PERF-13 (P2, Strong).** **Per-entry index cost**: 2 × `canonicalize` + 2 `stat` +
2 YAML parses per entry (`index.rs:499, 540, 641, 947, 634-638`), no cache, no incremental
refresh (`index.rs:299`). Structural memory estimate 0.4–0.6 KB/entry → 40–60 MB at the
100k entry cap (**Hypothesis**, no RSS measurement).

**PERF-14 (P2, Strong).** **Every watcher batch re-reads and re-hashes every open
document** (`project_discovery.rs:3248-3256`): 32 MiB of reads + hashing for four 8 MiB
documents, per burst — and the app itself creates a burst on every save (`DOC-01`).

**PERF-15 (P2, Strong).** **Preview readiness polls by re-parsing captured output every
25 ms**, up to ~4 MiB of allocation per poll (`preview/src/server.rs:437-505`), with no TCP
probe.

**PERF-16 (P2, Strong).** **Schema freshness polls every 750 ms forever** and re-reads all
schema files (`project_discovery.rs:1837`).

## 6. Findings — memory

**PERF-17 (P2, Strong; exact RSS Hypothesis).** A session holds **5–6 × N** plus
O(words): `original_text`, `current_text`, `projection.visible`, the editor rope,
`WriterSurface.source`, `WysiwygDocument`, token ranges, decoration ranges, and history
(≤500 × removed+inserted). The admission guard sums only `original_text`
(`project_discovery.rs:3528-3536`), so its "32 MiB" budget corresponds to roughly
150–250 MiB of real occupant memory at the cap. Two load paths exist with **different**
policies: `open_entry_inner` enforces the budget, `open_project_source` (definition
targets, file dialog) does not (`:2786-2795`).

**PERF-18 (Hypothesis).** Long-session growth was not measured. Structural candidates:
retained sessions for 32 documents, `lsp.pending` growing monotonically for unanswered
request ids (`astrolith-lsp/src/client.rs:389`), and per-field entity churn in the
frontmatter form.

## 7. What is *not* a problem (measured or verified)

- Projection diff/rebuild: 0.12 ms at 1 MiB (§2.1).
- Document read from disk: 0.77 ms at 1 MiB (§2.4).
- Theme application: once per appearance/preference change, not per frame
  (`theme/mod.rs:86`, `shell.rs:323, 473-477`).
- Color conversion: OKLCH→HSLA once per palette build (`theme/palettes.rs:17-45`).
- Entry list and file tree rendering: virtualized
  (`gpui-component/src/list/list.rs:564-600`, `file_browser.rs:491`).
- Service work is off the UI thread: JSON parse/serialize, saves, indexing all run via
  `runtime::spawn`/`blocking` (`runtime.rs:77-127`), and cancellation is correct
  (`runtime::tests`).
- Test-suite health: 915 tests in ~5 s of test time; the gate is not a bottleneck.

## 8. Provisional targets (engineering goals, not current state)

| Scenario | Target | Status at baseline |
|---|---|---|
| Cold start → interactive | ≤ 500 ms | **Not measured** (needs a native run + trace) |
| File open → editable, ≤ 100 KB | ≤ 100 ms | ~5.5 ms first frame + ~0.1 ms I/O — plausibly met |
| File open → editable, 1 MB | ≤ 100 ms | 56.6 ms frame: met in release headless; unverified with a real window |
| Typing p95, ≤ 100 KB | ≤ 16 ms | 2.4 ms pipeline + render: likely met |
| Typing p95, 1 MB | ≤ 16 ms | 16.0 ms pipeline **before paint**: not met |
| Idle CPU with project open, no terminal use | ~0 | Not met: ~10 watcher dispatches/s + PTY poll if a terminal exists |
| Shutdown: no orphaned process | — | LSP/preview ok; terminal children not guaranteed (`BM-12`) |

## 9. Not measured (explicit gaps)

| Metric | Why it is missing | How to obtain it |
|---|---|---|
| Cold/warm start to first frame | Requires a native window and a timestamped trace | `perf record -g` + a `tracing` span from `main` to first `render_frame`; compare `--release` |
| Peak startup RSS | No instrumentation | `heaptrack` or `/proc/self/status` sampling at first frame |
| Real frame time / vsync | Headless harness | `WAYLAND_DEBUG`, frame callbacks, or a compositor-side frame counter |
| Input latency (key → pixel) | Headless harness | injected evdev events + high-speed capture, or GPU timer queries on the frame |
| RSS per open document | No instrumentation | sample `VmRSS` after opening N × 1 MiB documents |
| Terminal flood behaviour | Requires a window | `yes` in the terminal, `perf top` split by thread, `strace -c` for `futex` |
| LSP cold start / completion P50 | Requires a real server | spawn `typescript-language-server`, timestamp `initialize` → `Ready`, then N completions |
| HMR edit→paint latency | Requires the Astro stack | `astro dev` + timestamped save → page reload |
| Window resize with 1.3 MB entry | Workload did not complete in 33 min; lower bound recorded in §2.5 | Per-phase instrumentation + 1/8-scale run (`PERF-E2E-04`) |
| macOS/Windows anything | Not available | CI matrix on real hardware |

## 10. Optimization opportunities, ranked by measured leverage

1. **Window the writer surface** (PERF-01): the worst measured symptom in the codebase — the
   repository's own resize workload does not complete 90 frames on a 1.3 MB entry in 33 minutes
   (§2.5). Expected to change frame cost from O(blocks) to O(viewport).
2. **Stop cloning the document in `Shell::render`** (PERF-03): a one-line signature change
   (borrow instead of clone) removes O(N) from every unrelated frame.
3. **Make the per-edit pipeline incremental** (PERF-04): keep `visible`, update token
   counts incrementally, and stop re-parsing the whole WYSIWYG document per keystroke.
   Measured 16 ms/1 MiB is the ceiling to attack.
4. **Batch terminal output and separate parse from render state** (PERF-09/10/11).
5. **Move `Shell::render`'s project-sized work into change handlers** (PERF-02).
6. **One filesystem walk per watcher batch, shared by index and tree** (PERF-12).
7. **Replace the 750 ms schema poll and 25 ms output poll with events** (PERF-15/16).

None of these has a measured *after* value yet; that is what phase 7 of the roadmap is for.
