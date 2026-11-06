# Published GPUI dependencies — 2026-11-06

At commit `b85b34e`, Astrolith stopped vendoring or patching GPUI packages.
That measurement snapshot pinned `gpui-kit = 0.8.1`, with published `gpui-base = 0.8.1` and `gpui-pre = 0.3.10` restored to
registry sources and checksums in Cargo.lock. No Cargo registry files were edited.

## Application adapters

The visible Writing surface keeps its native input geometry and retained
EditorState. Its command adapter uses published selection, replacement, clipboard,
input-handler and focus APIs. Source history still owns undo. Extended-grapheme
boundaries use the already-resolved unicode-segmentation 1.13.3 directly, including
chunk-crossing combining marks, emoji modifiers, ZWJ sequences, flags and CRLF.
The surface owns caret blinking and derives text generations from shared Rope
snapshots. No hidden Editor is remounted for Writing.

The source view uses a pane wrapper to adjust public scroll offsets after a
focused, previously visible caret is pushed out by a shrinking viewport. The
adjustment runs on the next UI turn; manual scroll ownership is preserved.
The stock source editor still owns source shaping and display wrapping.

Preview no longer mounts the published TextView renderer or a source-page list.
It reuses the writer's accepted `WysiwygDocument` revision when available and
lays the whole body out with the application-owned viewport text layout:
visible-row wrapping, bounded shaping, table-cell grids, footnote and raw
HTML/MDX blocks and standalone images. Read-only selection, keyboard navigation,
Select All/Copy, link activation and a scroll anchor live in the pane; source
selection is still what Select All copies.

Inline images remain alt text (the same rule as Writing); standalone image
paragraphs paint the picture. Raw HTML/MDX flow is shown verbatim rather than
interpreted. Page splitting and its list height repair are gone.

## Limits

Removing the patches also removes the framework's inline-fragment culling,
indexed wrap/style queries, bounded long-source-row shaping and row-layout cache.
Application Rope snapshots, local source splices/count updates, coalesced worker
parsing, retained mode models and Writing block windowing remain in place.

The page byte budget is deliberately soft: a single huge paragraph, list, code
block or MDX component stays intact. Such a block can still enter expensive stock
TextView layout. This migration does not claim performance parity with patched
GPUI or a universal 16 ms frame bound. Earlier reports explicitly describe the
patched dependency graph and cannot be reused as current performance evidence.

## Validation

Regression tests retain visible native input and IME geometry, hidden-source
undo/save behavior, source caret visibility after resize, unmeasured Preview tails
and reaching the end after resize. New coverage checks extended grapheme deletion
through actual keyboard actions, AST page boundaries, shared reference definitions
and whole-document Select All/Copy across pages.

## Native measurements without patches

Used actual `mise run dev` on Hyprland 0.57.1/Wayland, scale 1, tiled
1263×1384 and fullscreen 2560×1440. Isolated fixtures/preferences/recovery and
an Astro discovery stub were used; this does not measure a running Astro server.
Both native windows completed their mode cycle and exited normally.

| CPU workload | 1,112,000-byte multi-block source | 102,420-byte styled CJK/emoji single paragraph |
| --- | --- | --- |
| Largest Writing request-layout through paint sample | 29.534 ms | 837.569 ms |
| Largest Preview request-layout through paint sample | 19.452 ms | 7,817.355 ms |
| First Build request-layout through paint | 59.258 ms | 2,399.334 ms |
| All six directed mode handlers | 0.272–3.794 ms | 0.228–0.994 ms |

The common multi-block case retains the earlier application optimizations.
The long single paragraph regresses substantially without framework patches.
Preview paging cannot split that paragraph without changing Markdown semantics;
the stock parser/inline renderer still handles it as one unit. A cheap mode
handler can queue behind one of these expensive frames. These measurements
therefore do not establish responsive long-single-block switching.

Eight programmatic operations per window committed `中🦀` through the real native
Writer EntityInputHandler. Source Change handler work in the 1.1 MB run was
1.098–2.454 ms. This excludes frame time and is not end-to-end input latency.
The rich parser still ran on Worker threads and accepted the latest revision 8.
Both runs included native Hyprland fullscreen/tiled dispatches. Probes log CPU
samples above 8 ms; they do not measure GPU presentation/compositor latency.
Physical keyboard input and actual fcitx5 candidate-window placement are not
established by these programmatic adapter calls.

Raw probes, resize timestamps, dependency metadata and the summary are retained
locally under `/home/maredevi/Documents/Codex/2026-11-06/astrolith-unvendor/`.
Temporary instrumentation was removed before the final `mise run check`.
