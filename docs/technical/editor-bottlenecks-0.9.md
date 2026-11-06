# Editor bottlenecks on GPUI Kit 0.9.0 — 2026-11-06

## Dependency and measurement boundary

The application now uses published gpui-kit/base/component 0.9.0 and gpui-pre
0.3.11, with no vendor or registry edits. See [the dependency refresh](dependency-refresh.md)
for the complete update and tree-sitter's native-link compatibility constraint.

Three separate native `mise run dev` processes opened the same 102,420-byte styled
CJK/emoji single paragraph in Writing, Preview and Build. Each used isolated
preferences/recovery, performed a Hyprland fullscreen/tiled pair, and exited
normally. The Astro fixture is a discovery stub; this does not profile a server.

Linux perf recorded user-only `task-clock` samples at 199 Hz with DWARF stacks.
The analysis below filters the main thread, uses exclusive self samples and
disables inline symbol expansion for report generation. Some libc symbols and
unwound callers remain unresolved. No kernel/compositor time is included.
Percentages describe this workload's sampled CPU time, not universal bounds.
Source mapping of the largest Writing instruction was checked separately with
`perf annotate` and `addr2line`, including inlined source frames.

Temporary frame probes measured CPU request-layout through paint above 8 ms.
They observed maxima of 610.066 ms in Writing, 6,176.236 ms in Preview and
2,548.843 ms in Build. These runs include profiler overhead and fresh process
startup. They are not controlled before/after speedups, release benchmarks,
end-to-end keystroke latency or proof about physical IME candidate windows.

## Writing: inline layout retention and range scans

Writing mounts the application's `WriterSurface`, not the Kit Editor widget.
EditorState remains its text/selection/composition input model. The visible body
still renders through published `base::text::editor_host` Paragraph/InlineFlow.

61.11% of main-thread self samples resolved to `Inline::retain_styled_text`;
another 7.14% resolved to a `slice_ranges` scan in mixed inline layout. Within the
first symbol, the largest sampled instruction represented 86.88% of that symbol's
local sampled period. Its inlined source trace resolves to
`inline.rs:481`: `sweep_retained_layouts` calling `HashMap::retain`, reached from
`retain_layout` and `retain_styled_text`. This directly identifies the table sweep
as substantial CPU work, rather than merely naming its high-level caller.

The published implementation calls the sweep during each layout hand-over. It
scans the table if dropped-state accounting changed **or** the table has at least
4,096 entries. When many live fragments keep the table above that threshold,
repeated hand-overs can repeatedly scan the live table. The fixture contains
2,845 inline-code spans plus surrounding prose and wrapped fragments. Source and
sampling establish the scan hotspot; the trace does not independently record the
live table length or distinguish every reason a sweep was triggered.

Application code amplifies this work. `WriterSurface::render` constructs new
Paragraph/BlockNode values for visible blocks every frame; `paragraph_for` starts
with Paragraph::default. Each receives fresh interaction/layout state. Caret
blinks notify the surface, and one enormous visible paragraph includes thousands
of offscreen inline fragments. Block windowing cannot bound work inside that
single block. Retaining node identity would help cache reuse, but alone would not
remove the unbounded live-table sweep or full-block fragment work.

The rich Markdown snapshot parser already runs on a worker. Changing it or
removing EditorState alone does not remove this measured render path.

## Preview: repeated highlight merging and memory traffic

Preview uses TextView inside the application's source-page list. It does not
mount the Kit Editor widget. A huge paragraph intentionally remains one source
page so a split cannot alter Markdown structure.

20.93% of main-thread self samples resolved to `combine_highlights`. There are
also significant style-clone samples and unresolved libc memory-operation
samples; no precise allocation/copy percentage is claimed for those unresolved
symbols.

In published `node.rs`, `Paragraph::inline_flow_items` loops over inline children
and assigns `highlights = combine_highlights(highlights, node_highlights)` after
each text child (line 3201). The merge rebuilds/sorts endpoints from accumulated
input. A paragraph with many styled children repeatedly processes its preceding
ranges. The plain-text render cache is reached only after the mixed-flow branch
in `Paragraph::render`; it does not generally cover this inline-code workload.
The same merge pattern also exists in the plain preparation path.

This mode alone disproves the claim that the Editor widget is the sole cause.

## Build: shaping an entire visible source line

Build mounts the stock Editor widget. `TextElement::visible_buffer_lines` culls
by buffer line, while `layout_lines` extracts and shapes each relevant line through
`WindowTextSystem::shape_line`. With soft wrapping disabled, the fixture is one
large visible source line. Vertical line virtualization cannot bound the amount
of text shaped inside that line.

About 47.19% of main-thread self samples can be grouped into resolved font parsing,
font lookup and shaping symbols (font_types, read_fonts, harfrust, skrifa, fontdb
and related modules). The group excludes unresolved and generic helper symbols,
so it is not a complete attribution of source-line shaping cost. Background
Tree-sitter startup also consumes CPU, but its worker cost is not a main-thread
layout percentage.

The current dev profile optimizes gpui-base/pre and several editor dependencies
at level 3, while the font-stack crates still use the default dev optimization
level. This can amplify cold source shaping. No release or alternate-profile A/B
was performed, so the benefit of changing those profile settings is unquantified.

## Replacing the high-level editor is possible

GPUI itself does not require either TextView or EditorState. Keep Kit's shell,
docks, menus and ordinary controls while owning document editing and rendering:

1. Reuse authoritative DocumentBuffer Rope snapshots, source history and projection
   models. Those already live outside the Kit widget.
2. Use one application text-layout core for Writing and read-only Preview. Cache
   shaped runs by document/style/width revisions, shape and paint visible visual
   rows, and bound retained geometry. Preserve source/UTF-16/grapheme mappings.
3. Register EntityInputHandler directly on the visible surface. It already owns
   native input geometry; replacing EditorState means also owning composition,
   selection direction, keyboard commands, clipboard and accessibility state.
4. For Build, either retain the stock editor initially or move source rows onto
   that core with separate syntax/LSP presentation. Long-line shaping needs its
   own bounded strategy; removing a type does not provide it automatically.

The first target should be the shared **rich rendering path**, its node identity
and within-block virtualization. Replacing only Build's Editor widget leaves the
measured Writing and Preview hotspots intact. Removing Writing's EditorState can
later eliminate redundant display/highlighting/history state, but is a separate
change with real-platform IME/focus and editing acceptance requirements.

## Evidence

Raw perf recordings, main-thread reports, annotated Writing instructions, source
line resolution, native CPU logs, resize actions, dependency inventory and checks
are retained locally at:

`/home/maredevi/Documents/Codex/2026-11-06/astrolith-gpui-0.9-profile/`

Temporary probes were removed before the clean workspace check. The upgrade
passes `mise run check` with 1,011 Rust tests (8 ignored), fixture loaders and the
optional embedded-webview type check. The Windows job adapter separately passed
an isolated Windows-target compilation and Send/Sync assertion; native Windows
execution was not tested.

## Implementation plan

See the [detailed optimization implementation plan](editor-performance-implementation-plan.md)
for milestones, layout/input contracts, acceptance targets and rollback boundaries.
