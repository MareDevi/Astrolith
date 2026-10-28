# M00.2 — Editor feasibility spike

Date: 2026-10-28. Recommendation: **retain GPUI Kit as a viable candidate, keep the M00 editor gate open and do not start M01.** Automated editing contracts and a native Wayland launch work. Physical Chinese/Japanese IME, visual highlighting, inter-app clipboard, and native input-to-present latency still require acceptance. This disposable probe does not establish a product document architecture or an approved replacement strategy.

## Reproduce

Install the pinned tools with `mise install`, then:

```sh
mise run dev:editor
mise run test
mise run test:editor-perf
mise run check
```

`mise run dev` still opens the original bootstrap. The editor is selected with `--spike=editor`; it remains under `apps/desktop/src/spikes/editor.rs`. There are no files to open or save. Embedded/generated multilingual Markdown supplies all content. The toolbar loads approximately 1 KiB, 100 KiB, 1 MiB, or a 100 KiB single line. These replacements are undoable. The footer displays UTF-8 byte cursor/selection offsets and the result of the last refresh. Refresh uses Ctrl+Alt+R on Linux/Windows and Command+Option+R on macOS. Ordinary component shortcuts handle selection, clipboard and undo/redo.

The performance task builds in release mode and runs one explicitly ignored diagnostic test, with four finite workloads and 100 typing samples per workload. It is separate from normal CI; no machine-specific timing assertions are used.

## Exact environment

- GPUI Kit/Base/Component/assets 0.8.1; GPUI snapshot `gpui-pre` 0.3.10, locked in Cargo.lock. The application depends only on the Kit facade.
- Runtime features: default `component`/`assets`, `tree-sitter-markdown`, `tree-sitter-markdown-inline` (and their transitive tree-sitter support). `test-support` is enabled only by the dev dependency. `gpui-fast` is present in the resolved optional lock graph but absent from the normal runtime dependency tree.
- Mise 2026.10.19; Rust 1.99.1 (`58cf8f9c3`, 2026-10-13); Node 26.12.1; Bun 1.4.4. Rust components remain exactly `rust-analyzer`, `rust-src`, `clippy`, `rustfmt`.
- CachyOS Linux 7.3.0-4-cachyos, x86_64; Intel Core Ultra 9 185H, 30 GiB RAM; Intel Arc integrated and NVIDIA RTX 4060 Laptop adapters present. The active native rendering adapter was not independently identified.
- Native session: Wayland, Hyprland 0.56.2 (`efb50993780079460b0cbed1363e2166a2de1d9f`). Both observed windows were on HDMI-A-1, 2560×1440, scale 1. The internal display was configured at scale 1.3333334; no window interaction or scaling acceptance was performed there.
- Headless measurements: GPUI test platform, 1000×700 logical viewport, default simulated scale 1, release optimization. This is CPU dispatch/layout/frame generation, without compositor/GPU presentation. No OS IME is involved.

Initial feature resolution failed against stale sparse registry metadata with `gpui-fast ^0.1.12` unavailable. A temporary 0.8.0 resolution refreshed the index; updating explicitly back to the original 0.8.1 succeeded. No framework downgrade, vendor patch, or fabricated dependency remains. The optional graph's exact requirements select taffy 0.15.0 and unicode-properties 0.1.3, replacing the bootstrap's patch versions; these are upstream dependency constraints, not application API overrides.

## Capability findings

| Capability | Evidence and limitation |
| --- | --- |
| Editor window | The ordinary styled `Editor` retains one `Entity<EditorState>` inside the Kit window Root. Bounded native launches observed both the editor and bootstrap mapped, visible, `xwayland=false`, 1263×686. Processes were terminated after observation, with empty stderr. This proves native window mapping, not pixels or physical interaction. |
| Markdown | `.language("markdown")` and both grammar features are enabled. A headless test reads the actual editor's language/buffer, checks registered block/inline grammars, and runs the published `SyntaxHighlighter` against that buffer: parse tree and heading style ranges are present. The component installs its highlighter factory in its render path (verified in pinned source). The retained internal highlighter is not publicly inspectable; the test does not assert its private state or rendered colors. Visual highlighting acceptance is pending. Fenced Rust grammar and MDX-aware highlighting are outside this spike. |
| Selection and cursor | Actual typing and Shift+Left select `中文` in `A🦀中文`: state reports UTF-8 bytes `5..11`, cursor 5; the public handler reports reversed UTF-16 `3..5`. Replacement and keyboard undo/redo preserve text and selection. Do not interchange these coordinate systems. Public `set_selected_range` takes UTF-8 bytes and clips/expands to scalar boundaries; `cursor_position`/`set_cursor_position` also exist in pinned source. |
| Programmatic updates | `replace_all` preserves undo history, emits normal changes and resets multiline selection to `0..0`. The production refresh is tested through its keyboard action and pointer controls. `set_value` clears history and emits no Change event; use it only for deliberate initialization/reset, as in the isolated workload setup. It is unsuitable for a live refresh. |
| Undo grouping | Simulated multi-character typing replaces an existing selection with an atomic first-character edit and then a coalesced subsequent typing group. The test observes `Unsaved 中文` → refresh → undo back to `Unsaved 中文`, with earlier undo steps `U` then the initial sample. Do not assume every multi-character typing helper call is one undo transaction. |
| Composition | Real `ElementInputHandler` methods on the mounted Editor exercise UTF-16 replacement of an astral character, Japanese marked text/update/commit, Chinese preedit update/cancel, selection preservation during refresh, and retry after cancellation. This is protocol simulation, not native candidate-window or OS routing acceptance. |
| Clipboard | Component copy/cut/paste shortcuts round-trip combining marks, ZWJ emoji and Chinese text through the test-platform clipboard; cut/paste undo restores content. Native clipboard interchange remains pending. |

Exact signatures and behavior were checked in published 0.8.1 sources: `gpui-base/src/input/base/state.rs`, `input/editor/mod.rs`, `gpui-component/src/input/input.rs`, and `highlighter/{mod,highlighter,registry}.rs`. Upstream Kit integration tests in `tests/integration/input/{composition,lifecycle,editor,history}.rs` supplied the public production-view/input-dispatch pattern. Context7 was resolved to `/longbridge/gpui-kit` before querying editor language/grammar and state usage; current main-branch documentation was verified against the pinned sources.

## Composition-safe refresh

Calling `replace_all` directly during preedit erased `A日本Z` and its marked range. The regression test first reproduced that loss with the actual Editor and refresh key. The spike now constructs a public handler from the same retained entity/input bounds and checks `marked_text_range` before any fixture replacement. While composition is active it preserves the buffer, marked text and selection and displays “Refresh blocked during composition; commit or cancel, then retry.” It does not queue or automatically apply a stale replacement later. Keyboard and pointer entry points share this guard.

This is a narrow safe strategy for the spike. Future document integration must also preserve unsaved edits, validate document identity/revision, and resolve external-change conflicts. Those services belong to later tasks. If native acceptance shows that the OS commits/cancels preedit before the guard can observe it, stop at the gate and investigate GPUI's platform input bridge. Do not promote the simulated result to an OS guarantee or fall back to `set_value`.

## Workload measurements

One optimized run after finalizing the view; milliseconds, lower is better. Generated fixtures contain CJK, emoji, combining marks, headings, lists, inline markup and fenced code; the long-line fixture repeats CJK/emoji/inline markup without newlines. Fixture construction is outside the measured interval.

| Actual bytes / lines | `set_value` CPU | Next headless frame CPU | 100 input+frame samples p50 / p95 / max |
| --- | ---: | ---: | ---: |
| 1,051 / 14 | 0.809 | 7.552 | 1.944 / 2.001 / 2.085 |
| 102,998 / 1,372 | 6.313 | 12.997 | 3.523 / 3.638 / 3.669 |
| 1,048,898 / 13,972 | 44.792 | 48.966 | 1.373 / 1.738 / 1.880 |
| 102,420 / 1 long line | 7.724 | 13.588 | 8.688 / 9.137 / 9.294 |

Each sample calls `window.input("中")` at the start of the loaded document. The helper delivers input to the focused real component and completes headless frames. The test verifies exactly 300 additional UTF-8 bytes after 100 samples. p95 is the 95th ordered sample (nearest rank). It includes helper overhead and CPU frame work; it excludes hardware input, OS event delivery, candidate windows, GPU execution and presentation. It is **not** native end-to-end typing latency.

The representative release load's combined measured CPU intervals total 93.758 ms for 1 MiB. Neither interval waits for all deferred/background syntax work or a presented frame, so this does not establish a completed native load below the provisional 500 ms budget. Actual native load/presentation and visible typing below 50 ms p95 are still unmeasured; the gate cannot pass on these surrogate measurements.

Earlier debug diagnostics used a shorter repeated sample: 1,110 bytes/72 lines (load 4.815 ms, next frame 29.184 ms, input/frame p95 18.327 ms), 102,490/6,648 (123.024, 143.372, 13.437 ms), and 1,048,580/68,016 (1,682.446, 1,555.371, 17.664 ms). The 102,420-byte single line measured 75.680 ms load, 75.270 ms frame and 70.422 ms input/frame p95. The 68,016-line case exceeds upstream Editor's stated approximate 50K-line guidance; its slow debug result is retained as an observed limit, not hidden or compared as equivalent to the optimized 13,972-line case. The final status reads the rope's O(1) byte length instead of materializing the whole buffer each render. No library performance implementation was changed.

## Native acceptance still required

A person with working Chinese and Japanese input methods should record compositor/backend, IME framework and engine versions, keyboard layout, display/scale, document size, observed result, and failures for each step:

1. Run `mise run dev:editor`; inspect headings, emphasis, inline code, lists, fences, selection, caret and CJK glyphs. Capture actual visual evidence before marking highlighting accepted.
2. With a Chinese IME, update preedit several times, choose a candidate, commit, then repeat and cancel. Include CJK/emoji neighbors and selection replacement. Verify undo/redo and buffer contents.
3. With a Japanese IME, convert kana, change/select candidates, commit and cancel. Confirm candidate placement and selection/caret after each operation.
4. While preedit is active, invoke Ctrl+Alt+R without moving focus. The preedit/selection must remain intact and refresh must report blocked. Commit/cancel, explicitly retry, then undo. Repeat with toolbar load/refresh controls and record whether focus transfer changes OS composition first.
5. Exchange multilingual text in both directions with a separate native app, using copy/cut/paste. Verify emoji/combining marks, multiline text, selection replacement, and undo/redo.
6. Repeat meaningful editing and focus traversal on native scale settings required by the M00 layout gate. Native scale acceptance remains a separate uncompleted item.
7. Measure at least 100 real native visible typing samples on the representative 1 MiB document with an input-to-presentation instrument (or frame-labelled capture with a documented clock). Report p95 and compare with 50 ms. Measure native load to first presented frame against 500 ms. No such instrumentation was available in this session.

If these fail, keep M01 blocked. First investigate the pinned component/platform bridge and obtain an upstream fix or a reviewed narrow adapter; re-run this matrix against any proposed version. If the contracts remain unsatisfied, request approval for a replacement editor strategy backed by the same workloads and native acceptance. No replacement has been approved by this report.
