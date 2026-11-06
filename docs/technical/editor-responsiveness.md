# Editor responsiveness repair — 2026-11-05

## Causes and ownership

Writing no longer mounts a transparent source editor behind the rich document.
The visible surface owns native input, caret/selection/IME geometry and scrolling;
the retained `EditorState` owns text, selection, composition and input actions.
Clicking text or empty space keeps that model focused instead of allowing the
parent panel to take focus. Composition snapshots include preedit text.

A session retains separate Writing and Build input models and one source history.
Mode changes reuse those models. Only a stale model receives a minimal text diff;
unchanged mode transitions do not reinstall source, restart highlighting or scan
an unchanged rope. Old source-editor prose decorations and typewriter geometry
have been removed: they duplicated the visible rich surface's work.

Both Writing and read-only Preview lay out a window of blocks from the first
frame, with estimates for unmeasured heights. A width change invalidates exact
measurements while retaining estimates. A mixed paragraph also creates native
inline elements only for visible fragments. Offscreen glyph layouts remain
available for source mapping, hit-testing, selection, IME and reaching the end.

Styled-paragraph work no longer repeatedly merges every preceding inline child,
copies the remaining hard line or scans every style range for every visual row.
Child ranges append after local overlap resolution; item, range and width indices
bound repeated queries.

Build adds display continuations around 1,024 UTF-8 bytes on exceptionally long
unwrapped source lines, without splitting graphemes or inserting source newlines.
It shapes visible rows and selection/composition endpoints, measures bounded
rows for horizontal extent, and retains at most 256 shaped rows per input model.
Cache keys include text, full styling runs, font size and tab width. A resolved
focused caret follows a shrinking viewport on both axes. Manual scrolling away
from an unchanged caret remains allowed.

Framework changes use published, pinned sources in `vendor/`, not edits to the
user's Cargo registry. `gpui-kit` remains 0.8.1. See `vendor/PATCHES.md` for the
transitive package versions, modified files and maintenance boundary.

## Native development-profile evidence

Actual `mise run dev` runs on Hyprland 0.57.1/Wayland, NVIDIA RTX 4060 Vulkan,
scale 1, tiled 1263×1384 and fullscreen 2560×1440. Fixtures and settings were
isolated under `/tmp`; the real project and user preferences were not modified.
The fixture Astro package was a discovery stub, not a running Astro server.
These measurements concern native Markdown Writing, Build and Preview.

Temporary probes timed mode handlers and native element request-layout through
paint. Hyprland dispatchers performed fullscreen/tiled changes in every mode.
Mode commands ran through the actual shell command path on the native event loop.
The probes were removed after measurement. Values below are CPU work, not GPU
presentation latency or a guarantee of 60 FPS. Frame logging threshold was 8 ms;
`<8 ms` means no frame above that threshold was recorded in the measured window.

| Workload | Before | After |
| --- | --- | --- |
| 1,112,000-byte multi-block document, first Writing layout through paint | 10.95–11.29 s | 133.5 ms |
| Same document, Hyprland fullscreen/tiled reflow after initial layout | Writing 91–120 ms; Preview 135–154 ms | mostly <8 ms; highest recorded 15.2 ms |
| 102,420-byte styled CJK/emoji single paragraph, Preview resize | 16.1–16.4 s | 17.2–31.6 ms |
| Same single paragraph, Writing resize | — | 27.2–41.5 ms |
| Same single paragraph, Build layout through paint | about 2.35 s per remount | 322.1 ms first entry; repeat/resize <8 ms |
| Mode handler, all six directions in the final native runs | exit could queue behind a 15.4 s Preview frame | 0.38–2.41 ms handler work |

First entry into pathological long lines still has measurable cold shaping work.
At this measurement point, the first large Writing frame still parsed its rich
snapshot synchronously. The subsequent [Rope snapshot repair](rope-snapshot-performance.md)
moves that parse to background workers and updates the input measurements.
These results do not establish constant-time first-open or a 16 ms worst-case
bound for arbitrary document sizes. Warm transitions and resize no longer repeat
whole-document layout or pathological quadratic work.

Raw probes, action timestamps and summaries are retained locally in
`/home/maredevi/Documents/Codex/2026-11-05/astrolith-native-performance/after/final/`.
Baseline evidence is in the parent `astrolith-native-performance/` directory.

## Regression coverage

Headless UI tests exercise first-frame block windowing, resize and reaching the
last paragraph; offscreen long-paragraph caret geometry; scrolling without an
unchanged caret snapping back; text/empty-space click focus; visible native input
and IME preedit/commit with one undo; exact hidden-source preservation; retained
mode models, cross-view edits and undo; Unicode source-line end editing after
resize; visible Focus dimming and Typewriter centering.

Native Hyprland switching and resize were exercised as above. Actual fcitx5
candidate-window placement and physical-keyboard composition were not established
by the headless IME test. `mise run check` is the final automated gate.
