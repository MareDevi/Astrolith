# Pinned rendering patches

Astrolith still depends on `gpui-kit = 0.8.1`. Cargo patches its transitive
`gpui-base = 0.8.1` and `gpui-pre = 0.3.10` implementations with the published
crate sources in this directory. The original Apache-2.0 licenses are retained.
Do not edit the user's Cargo registry or depend on an unpinned upstream branch.

Changes from the published sources:

- `gpui-base/src/text/state.rs`: measure visible blocks instead of synchronously
  measuring the whole Markdown document.
- `gpui-base/src/text/document.rs`: give unmeasured blocks height estimates so
  the entire document remains reachable before every block has been visited.
- `gpui-pre/src/elements/list.rs`: retain old measurements as estimates when
  width changes; visible items are still remeasured before being painted.
- `gpui-base/src/text/inline_flow.rs`: index item/font-size boundaries, advance
  through borrowed wrap fragments, and index shaped-span widths. Visual-line
  layout no longer repeatedly copies or scans the remaining hard line.

- `gpui-base/src/text/inline_flow.rs`, `text/editor_seam.rs`,
  `text/editor_host.rs`, and `gpui-pre/src/text_system/line.rs`: prepaint only
  visible text fragments. Offscreen fragments share immutable shaped glyphs
  for complete hit-testing, caret and scroll geometry without native elements.

- `gpui-base/src/input/base/state.rs`: expose shared command binding without
  mounting the stock text element, plus text revision and caret visibility.
  Astrolith supplies native input geometry from its visible writing surface.

- `gpui-base/src/text/node.rs`: combine overlapping marks within each child
  and append disjoint child ranges; never rebuild every preceding child's
  highlights when rendering a styled paragraph.
- `gpui-base/src/text/inline_flow.rs`: index overlapping style and link ranges
  once per item for repeated visual-line queries, preserving input order.
- `gpui-base/src/input/editor/display_map/{text_wrapper,inline_line}.rs`:
  bound unwrapped display rows at Unicode grapheme boundaries, without
  inserting source newlines, and scan whitespace positions with a glyph cursor.
- `gpui-base/src/input/base/{element,state}.rs`: shape visible display rows
  and selection/IME endpoints; retain at most 256 shaped rows per input model,
  keyed by text, full runs, font size and tab width. Bound longest-line
  measurement to display rows. Keep a resolved focused caret visible on both
  axes after a viewport shrinks; deliberate scrolling keeps ownership.

The facade, public package versions, image/link handling and text source mapping
remain unchanged. Application UI integration tests exercise first-frame and
resize windowing plus reaching the final paragraph. Native development-profile
measurements cover the long styled paragraph that blocked Preview for seconds.
