# Zed Reference: Rendering, Editing, LSP, Terminal, Concurrency, Scanning

Reference tree: Zed `c7bea343838f42b4c14151393e41f0bbe033c95e` sparse checkout at `/tmp/zed`.
Note: this Zed is **newer than the GPUI that `gpui-kit 0.8.1` pins** — `crates/gpui/src/app/context.rs`,
`Effect::Notify`, `WindowInvalidator`, `dirty_views` and the `arena_scope` element arena do not exist in
gpui-base 0.8.1. Every Zed mechanism below is therefore a *design* reference, not a drop-in port.
`crates/worktree` and `crates/terminal_view` are absent from the sparse checkout; where a claim depends on
them I read the blob from the git object store (`git cat-file -p HEAD:crates/worktree/src/worktree.rs`)
and cite it as `crates/worktree/src/worktree.rs:LINE` (git-derived, not on-disk). All such lines are marked
`[git]` in the evidence column.

## Scope inspected

| Area | Files read (lines) |
|---|---|
| GPUI invalidation + elements | `gpui/src/app.rs` (3163-3205, 1455-1500), `gpui/src/app/context.rs` (63-110, 180-260, 735-755), `gpui/src/window.rs` (110-233, 1250-1295, 2449-2500, 2585-2605, 3770-3830, 4244-4330), `gpui/src/view.rs` (469-675), `gpui/src/subscription.rs` (1-320), `gpui/src/element.rs` (1-70, 640-665), `gpui/src/elements/{uniform_list.rs 1-600, list.rs index}` |
| Editor / text / LSP | `text/src/{text.rs 59-130,860-940,1300-1420; anchor.rs}`, `editor/src/{editor.rs 979-1040; selections_collection.rs 28-60; element.rs 8700-8790,9590-9620}`, `rope/src/rope.rs` (147-260), `language/src/buffer.rs` (1170-1200, 1943-2050), `multi_buffer/src/multi_buffer.rs` (1403-1450), `project/src/buffer_store.rs` (285-400), `project/src/lsp_store.rs` (278-300, 349-430, 514-640, 3480-3560, 5250-5470, 6250-6330, 9700-9800, 10285-10380, 13340-13460), `lsp/src/lsp.rs` (125-150, 1505-1660) |
| Terminal / concurrency / scan | `terminal/src/terminal.rs` (491-560, 760-810, 880-900, 1205-1230, 1400-1520, 1735-1770), `terminal/src/alacritty.rs` (110-330, 730-960), `gpui/src/executor.rs` (1-300), `scheduler/src/executor.rs` [git] (256-300, 430-470, 596-640), `worktree/src/worktree.rs` [git] (84-95, 143-180, 1091-1120, 2121-2150, 4367-4665, 4852-4960, 5290-5320, 6555-6560), `worktree/src/ignore.rs` [git] (1-151) |

Astrolith files inspected: `astrolith-gpui/src/{shell.rs 219-330, runtime.rs, terminal/mod.rs, project_discovery.rs 3130-3320, editor/{session.rs, writer_surface.rs, lsp.rs, rendered.rs, save_coordinator.rs}}`,
`astrolith-document/src/{buffer.rs, save.rs, projection.rs}`, `astrolith-lsp/src/{client.rs, discovery.rs, language.rs}`, `astrolith-workspace/src/{watcher.rs, index.rs}`, `astrolith-terminal/src/lib.rs`, plus `gpui-component-0.8.1/src/list/*` and `gpui-base-0.8.1/src/text/text_view.rs` from the cargo registry.

Evidence levels: **Strong** = read at source:line; **Confirmed** = arithmetic or count reproduced from cited constants; **Hypothesis** = needs a runtime measurement.

---

## 1. Rendering: per-entity invalidation and virtualization

### R1 — `notify()` does not touch the tree; it dirties a view, and the dirty set is checked at prepaint

Zed does **X**: `Context::notify` (`crates/gpui/src/app/context.rs:216`) forwards the entity id to
`App::notify` (`crates/gpui/src/app.rs:3166`), which looks up `window_invalidators_by_entity` and calls
`WindowInvalidator::invalidate_view` (`crates/gpui/src/window.rs:171`) only for windows whose
`tracked_entities` set contains that entity (`app.rs:3178-3184`). If no window currently displays it, the
notification is queued as an `Effect::Notify` instead (`app.rs:3185-3190`).
`invalidate_view` inserts the entity into `dirty_views` (`window.rs:174`) and, once per frame, asks the
platform waker to draw (`window.rs:178-192`).

**Because** Y: invalidation is *per entity per window*, so changing the terminal panel cannot dirty the
editor subtree, and a background window's notifies do not schedule frames.
**Astrolith** Z: same shape one level up — `Shell` owns every panel as a separate `Entity`
(`crates/astrolith-gpui/src/shell.rs:219-306`), so `notify()` on `TerminalPanel` cannot dirty
`WriterSurface`. `Shell` also caches its menu model by signature to avoid rebuilding menus every frame
(`shell.rs:299-300` comment + `menu_signature`).
**Recommendation: Adopt (already matching).** No change. Risk: none. Cost: none.

### R2 — The per-window entity set is rebuilt from *what was actually read* each frame

Zed does **X**: while a view prepaints, `App::detect_accessed_entities` opens an access scope
(`crates/gpui/src/app.rs:1455-1461`); `ViewElement::prepaint_view` wraps the child `render` +
`layout + prepaint` in it and stores the resulting `accessed_entities` on `ViewElementState`
(`crates/gpui/src/view.rs:571-590`). At end of draw, `Window::record_entities_accessed`
(`crates/gpui/src/window.rs:3818`) replaces the window's tracked set wholesale
(`app.rs:1474-1494`), removing stale `(entity → window)` registrations first.

**Because** Y: the mapping is derived from real reads, not declared; an entity that stops being rendered
automatically stops invalidating the window (no leak, no over-invalidation).
**Astrolith** Z: no equivalent; panels are statically owned by `Shell` and never re-tracked. For a fixed
dock layout that is correct and cheaper.
**Recommendation: Reject for now / Defer.** Only worth building if Astrolith gains dynamic out-of-tree
view attachment (floating panes, per-tab overlays). Cost to build: ~1 day of GPUI-internals work that the
pinned `gpui-kit` does not expose (`dirty_views`/`tracked_entities` are not public in gpui-base 0.8.1).
Risk: none while the dock is static.

### R3 — A clean view's `render()` is skipped entirely; its prepaint/paint ranges are replayed

Zed does **X**: `prepaint_view` (`crates/gpui/src/view.rs:533-570`) reuses the previous frame's
`ViewElementState` when `bounds`, `content_mask`, `text_style` and `opacity` all match **and**
`!window.dirty_views.contains(&entity_id)` **and** `!window.refreshing`; the hitboxes, tooltips, text
layout ranges, `dispatch_tree` subtree, deferred draws and cursor styles are copied by *index range* in
`Window::reuse_prepaint` (`window.rs:4244-4300`) and `reuse_paint` (`window.rs:4310`). The render closure
is never called on that path.
Dirty marking is ancestor-aware: `mark_view_dirty` walks `view_path_reversed` and stops as soon as it hits
an id already in the set (`window.rs:2476-2490`) — one dirty leaf cannot dirty N ancestors repeatedly.

**Because** Y: repaint work is proportional to *changed* views, not to the tree.
**Astrolith** Z: `WriterSurface::render` (`crates/astrolith-gpui/src/editor/writer_surface.rs:431-537`)
always re-renders every block, and it is the *only* view for the whole document, so a keystroke dirties
100% of the document subtree (see E1). `Shell::render` (`shell.rs:3115`) re-runs per notify too, though
its children are entity views.
**Recommendation: Adapt, but not by importing R3.** The R3 machinery is unavailable in gpui-base 0.8.1;
the actionable half is keeping `WriterSurface`'s element tree small (render visible blocks only, R5). Do not
split into entity-per-block: it fights the single-pass `block_key(index)` geometry map
(`writer_surface.rs:469-537`), which is a simple, stable identity, and costs hundreds of entities plus focus
tracking per document.

### R4 — `AnyElement` is an arena allocation, not a heap allocation

Zed does **X**: `AnyElement(ArenaBox<dyn ElementObject>)` (`crates/gpui/src/element.rs:650-661`) allocates
from the per-frame element arena (`with_element_arena`), cleared at end of draw (`window.rs:3806-3815`).
**Because** Y: the cost model is "elements constructed per frame", not "heap allocations".
**Astrolith** Z: same allocator via `gpui_kit::AnyElement` (`writer_surface.rs:526-537`).
**Recommendation: Adopt the cost model as a review rule** — count elements per frame (blocks × inline runs
× cells), not boxes. Confirm gpui-base 0.8.1 uses the same arena before quoting.

### R5 — Virtualization: `uniform_list` renders only the visible index range; `List` adds measured heights and overdraw

Zed does **X**: `uniform_list` computes `first/last_visible_element_ix` from `scroll_top`, item height and
padded viewport height, then calls the caller's `render_items(visible_range)` and lays out **only** those
items (`crates/gpui/src/elements/uniform_list.rs:473-500`). `List` generalizes this to variable heights with
`ListState::splice(old_range, count)` (`elements/list.rs:490`) to record item-count changes without a full
reset, an explicit `overdraw` band measured above/below the viewport (`list.rs:310-321`, consumed at
`list.rs:1081-1210`), `measure_all()` / `with_uniform_item_height()` escape hatches (`list.rs:336-352`), and
`ListAlignment::{Top,Bottom}` with follow-tail semantics (`list.rs:605-660`).
**Because** Y: O(viewport) layout with O(1) splice for streaming content (terminal, logs, search results).
**Astrolith** Z: lists already go through gpui-component, which itself wraps `v_virtual_list` over
`rows_cache.entries_sizes` and only maps `visible_range`
(`gpui-component-0.8.1/src/list/list.rs:568-600`, registry): `project_discovery.rs:972`
`ListState::new(EntryList…)`, `file_browser.rs:174`. **Not** virtualized: the writer surface
(`writer_surface.rs:485-537`), the terminal grid (`terminal/mod.rs:606-628`), settings/preferences scroll
panes (plain `v_flex`, no list).
**Recommendation: Adapt for the writer surface, Reject for settings.** The writer is "one row per block,
unknown height" — `ListState` + `splice` + overdraw, not `uniform_list`. The blocker is that the writer needs
one continuous selection/caret across blocks plus a shared geometry map; `ListState` owns scroll and yields
`visible_range` but not a shared text coordinate space, so selection must be re-plumbed. Cost: ~2-3 days;
risk: regressing caret/selection mapping (`collect_leaf_geometry` at `writer_surface.rs:22`, `location_at`,
`source_at`). Only after measurement shows >~4 ms/frame on a 1000-block file.

### R6 — Large-list correctness details worth copying whatever the container

Zed does **X**: (a) clip via `window.with_content_mask` instead of filtering (`uniform_list.rs:493`);
(b) `logical_scroll_top_index` survives splices so re-render does not jump (`uniform_list.rs:222-229`);
(c) `scroll_to_item` is non-strict by default (`uniform_list.rs:145-160`); (d) a focused off-screen item is
still rendered so keyboard interaction works (`list.rs:1239-1250`).
**Astrolith** Z: `publish_list_state` re-publishes the whole `ListState` on each index change
(`project_discovery.rs:6065`); terminal scroll is one `i32` counter (`astrolith-terminal/src/lib.rs:212-225`).
**Recommendation: Adopt (a), (b), (d).** Cheap, container-independent.

---

## 2. Editor and buffers

### E1 — Astrolith rebuilds the whole document model per keystroke; Zed does not

Zed does **X**: an edit is `Buffer::edit(ranges, texts)` (`crates/text/src/text.rs:870-892`):
`start_transaction` → `Operation::Edit(apply_local_edit)` → `history.push` → `end_transaction`. The edit is
spliced into the rope, an `Operation` is published to topic subscribers
(`text.rs:901`, `self.subscriptions.publish_mut(&edits_patch)`), and only *changed regions* are reparsed.
The `BufferSnapshot` keeps `visible_text: Rope`, `deleted_text: Rope` plus `fragments`/`insertions` sum trees
(`text.rs:113-124`) so undo/redo and (in collab) remote ops are O(edit), not O(document).
**Astrolith** Z: one keystroke runs, synchronously inside the `InputEvent::Change` handler
(`astrolith-gpui/src/editor/session.rs:200-237` → `absorb_with_intent` `session.rs:1335-1378`):
1. `self.editor.read(cx).text().to_string()` — full copy (`session.rs:1352`);
2. `self.buffer.current_text().to_owned()` — full copy (`session.rs:1356`);
3. `diff_visible(...)` — full-string diff (`session.rs:1360`);
4. `apply_visible_edit(&before, &projection, edit)` — allocates a new full source `String` (`session.rs:1361`);
5. `self.buffer.apply_text(source)` — replaces `current_text` wholesale (`astrolith-document/src/buffer.rs:225-234`);
6. `after = current_text().to_owned()` — full copy again (`session.rs:1367`);
7. `history.record(&before, &after, …)` → `source_diff(before, after)` scans both full strings for the
   common prefix/suffix (`session.rs:1938-1960`);
8. `projection.renew_visible(visible)` — projection rebuild;
9. `this.dirty = this.buffer.is_dirty()` — full `original_text != current_text` compare (`buffer.rs:215`).
Then `WriterSurface::render` calls `sync_document` (`writer_surface.rs:88-95`), which compares the whole
source again (`self.source == source`) and, on mismatch, `WysiwygDocument::parse(source)` — a full Markdown
parse — and re-renders every block (`writer_surface.rs:485-537`).
**Because** of the "complete source is authoritative, editor text is a projection" design, every step is
correct but each is O(document); at 100 KB the cost is ~5 full-string passes + 1 parse per keystroke;
at 1 MB it is a visible typing stall. **[Hypothesis]** — needs a 1 MB-document typing measurement.
**Recommendation: Adapt and simplify.** Replace steps 1–9 with an incremental splice: the editor's own
`Rope` (gpui-component `EditorState`, already used — see `editor/lsp.rs:119 fn completions(… text: &Rope …)`)
reports a `Change` with a range; map *that range* through `WriterProjection.segments`
(`astrolith-document/src/projection.rs:76-81`) and splice it into `current_text` (a `String::replace_range`
is O(edit + tail-copy) and needs no new dependency). Keep the whole-document `source_diff` only as the
undo-record fallback when a range cannot be mapped. Cost: ~1 day in `session.rs` + `projection.rs`; risk:
the "fail closed" path (`session.rs:1370-1375`, `refused` latch) must keep the same guarantee — a refused
edit must leave the source byte-identical.

### E2 — Selections live in the view and are stored as anchors, so edits never rewrite them

Zed does **X**: `Editor.selections: SelectionsCollection` (`crates/editor/src/editor.rs:988`) holds
`disjoint: Arc<[Selection<Anchor>]>` plus at most one `pending` selection during a drag
(`crates/editor/src/selections_collection.rs:28-38`). `Anchor` is
`{timestamp_replica_id, timestamp_value, offset: u32, bias: Bias, buffer_id}` — 16 bytes, no position
recomputation on edit (`crates/text/src/anchor.rs:9-32`). Selection sets are resolved to offsets on read
against the current snapshot.
**Because** Y: one place owns selection, and it is edit-stable by construction; the editor never walks its
selections to "fix them up" after a buffer change.
**Astrolith** Z: selection lives in the editor (`EditorState::selected_range`, e.g. `session.rs:441`, read as
`last_selection: Range<usize>` in *source* bytes, `session.rs:53,1364`) and is re-derived from the projection
on every mode switch (`current_source_selection`, `session.rs:1667-1675`). There are two coordinate spaces
(visible/projection and source) and two conversion helpers (`projection_offset_to_source`,
`source_range_to_projection`), with `map_visible_offset` used to fix the caret after a rewrite
(`session.rs:1773`).
**Recommendation: Defer anchor machinery (Reject), Adopt the invariant.** Byte offsets are fine for a
single-buffer writer. The invariant to copy: *exactly one* place converts selection between visible and
source space, and the conversion is exercised by a round-trip test. Today the conversion lives in two
places (`current_source_selection` and `projection_range_to_source`, `session.rs:1667-1680`) — collapse
them into one function. Cost: <1 hour; risk: low.

### E3 — Undo: one stack, in the buffer, transaction-grouped; Astrolith keeps it in the session with full-string entries

Zed does **X**: undo lives in the text buffer, not the editor: `Buffer.history: History` +
`undo_map: UndoMap` (`crates/text/src/text.rs:59-68,119`); `Buffer::undo` pops a `HistoryEntry`, replays the
inverse `Transaction` and returns the resulting `Operation` so collab can broadcast it
(`text.rs:1352-1360`). Grouping is time-based and explicit: `start_transaction`/`end_transaction_at(now)`
(`text.rs:1313-1334`), `finalize_last_transaction`, `group_until_transaction`, `merge_transactions`
(`text.rs:1340-1400`). Remote edits and undo are the same `Operation` stream.
**Because** Y: one authority for text history; an editor can be destroyed and recreated without losing
undo, and undo is expressible as a network operation.
**Astrolith** Z: `SourceHistory { entries: Vec<SourceTransaction>, cursor }` in the **session**
(`session.rs:1848-1860`), each entry `{range, removed: String, inserted: String, selection_before/after,
intent, at}` (`session.rs:1802-1820`), capped at 500 entries (`session.rs:1888`), typing merged within 900 ms
(`session.rs:1904-1920`). `undo`/`redo` clone the whole document and `replace_range`
(`session.rs:1817-1836`, called from `session.rs:1192-1230`).
**Because** the document is a `String` and there is no transaction/operation log, this is the simplest
correct design at this scale.
**Recommendation: Adopt (keep), with one fix.** Strip `clone()` of the full text from `undo`/`redo` by
applying the inverse in place (`String::replace_range`) and handing the result to
`buffer.apply_text` — currently two full copies per undo. Keep the 500-entry cap; note it is currently
expressed as `if self.entries.len() > 500 { self.entries.remove(0) }` (`session.rs:1888`) which is O(n) per
overflow — use `VecDeque` or accept it. Cost: ~30 min; risk: low (tests at `session.rs:1817-1840` cover
inverse round-trips).

### E4 — Large files: try a bounded synchronous parse, then go background, then re-parse if the text moved

Zed does **X**: `Buffer::reparse` (`crates/language/src/buffer.rs:1943-2020`) first attempts
`syntax_snapshot.reparse_with_timeout(…, sync_parse_timeout)`; the default timeout is **1 ms**
(10 ms in tests) (`buffer.rs:1177-1183`). If the sync attempt fails, the parse moves to
`cx.background_spawn`. On completion it re-checks `self.version.changed_since(&parsed_version)` and, if the
text moved, calls `self.reparse(cx, false)` again — a self-terminating loop; a second guard
(`if self.reparse.is_some() { return; }`, `buffer.rs:1947-1949`) prevents concurrent parses.
`parse_status` is a `watch` channel so UI can show "parsing" (`buffer.rs:2038-2058`).
**Because** Y: keystroke latency is bounded by 1 ms of parse work; correctness is bounded by "parse until
caught up"; no debounce timer to tune.
**Astrolith** Z: `WysiwygDocument::parse(source)` runs synchronously in `render`
(`writer_surface.rs:94`), and `token_snapshot(&projection.visible)` runs synchronously in
`refresh_writer` (`session.rs:1135-1136`), which the `Change` handler calls (`session.rs:234`).
**Recommendation: Adopt the shape (bounded sync attempt + background follow-up + re-parse-if-changed),
but with a *generation counter* rather than a version clock.** Astrolith already has
`DocumentRevision` (`astrolith-document/src/buffer.rs:8-35`) — pass it into the parse task and drop the
result if `revision != task_revision`, exactly like `apply_watcher_scan` already does for index results
(`project_discovery.rs:3300-3310`, `accepts_index_result`). Cost: ~1 day; async plumbing already exists
(`runtime::blocking`, `runtime.rs:99-122`). Risk: the writer surface currently parses inside `render`;
moving that out means the surface must render stale blocks for one frame — acceptable if the stale frame
keeps the caret, but it is a visible behavior change and needs a test.

### E5 — The editor lays out and paints only the visible display rows, derived from scroll position

Zed does **X**: `EditorElement::prepaint` derives the window from scroll state, not from the document:
`start_row = min(DisplayRow((scroll_position.y + clipped_top_in_lines).floor()), max_row)` and
`end_row = min(ceil(scroll_position.y + clipped_top_in_lines + visible_height_in_lines), max_row)` computed
from `window.content_mask()` and `line_height` (`crates/editor/src/element.rs:8740-8768`); the rest of the
frame uses only `snapshot.row_infos(start_row).take((start_row..end_row).len())`
(`element.rs:8771-8774`) and an `Anchor` range clamped to those rows for highlights, diagnostics and
selections (`element.rs:8780-8795`). `EditorLayout.visible_display_row_range` then limits painting
(`element.rs:9608`, `:10777`) and cursor layout skips off-screen cursors by comparing to that range
(`element.rs:9609-9618`).
**Because** Y: an editor over a 1 M-line buffer costs the same per frame as one over 100 lines.
**Astrolith** Z: `WriterSurface::render` builds every block of the document
(`writer_surface.rs:485-537`) and has no visible-range concept; the caret/selection geometry map is rebuilt
from all leaves each frame (`FrameGeometry`, `writer_surface.rs:38-42`, filled via
`collect_leaf_geometry`). There is no scroll-derived row range anywhere in the surface.
**Recommendation: Adopt the invariant when adopting R5.** Any writer virtualization must compute its range
from `ScrollHandle` offset ÷ measured block heights, exactly as `element.rs:8740-8768` does, and must keep
the caret block in range even when off-screen (Zed's `non_visible_cursors` path, `element.rs:9609-9618`).
Risk: the typewriter/focus-mode scroll logic (`adjust_scroll`, `writer_surface.rs:455`) depends on the full
block list today; it must switch to an index-based calculation first.

### E6 — What is reusable at small scale vs what needs MultiBuffer/Project

Zed does **X**: `MultiBuffer` splices *excerpts of several buffers* (search results, inline assist) into one
editable coordinate space; `MultiBuffer::edit` (`crates/multi_buffer/src/multi_buffer.rs:1403-1420`) fans out
to backing buffers and returns a `ProjectTransaction`. `DisplayMap` adds soft wrap and folds
(`editor.rs:985-987` comment).
**Astrolith** Z: one buffer per document, no excerpts, no folds; the display-map analogue already exists as
`WriterProjection { visible: String, segments }` (`astrolith-document/src/projection.rs:76-81`).
**Recommendation:**
- **Reject** MultiBuffer / Project / DisplayMap — Astrolith never needs multi-buffer excerpts.
- **Keep** the projection-as-display-map split: the editor holds projection bytes only, never source bytes
  (`session.rs:40-46` comment). This is already the right architecture.
- **Defer** `text::Anchor` — only needed when positions must survive edits across save/reload.
- **Reject** `rope` at current document sizes; `String` is right, and the migration buys nothing measured.

---

## 3. LSP

### L1 — Server identity: one server per (worktree, server name, toolchain, settings), refcounted per buffer

Zed does **X**: `LanguageServerSeed { worktree_id, name, toolchain, settings }` is the dedupe key
(`crates/project/src/lsp_store.rs:283-289`) into `language_server_ids: HashMap<LanguageServerSeed, …>`
(`lsp_store.rs:359`). `start_language_server` (`lsp_store.rs:514-640`) allocates a fresh
`LanguageServerId` only on a new seed, and waits for worktree *trust* before starting
(`lsp_store.rs:537-575`). Buffers are registered by refcount: `OpenLspBufferHandle` +
`cx.observe_release`, unregistering only when `refcount == 0`
(`lsp_store.rs:5374-5460`).
**Because** Y: N open files in one language cost one server process; a settings change or a second
worktree with a different toolchain gets its own server, which is exactly the LSP workspace-folder model.
**Astrolith** Z: **per document session**. `DocumentSession::start_lsp`
(`crates/astrolith-gpui/src/editor/session.rs:1480-1526`) calls `astrolith_lsp::discover(root, language)`
+ `Client::spawn_process` for *this* document, and stores it in `self.lsp`
(`session.rs:1527-1560`). `LspState`'s doc comment says so explicitly: "One running server bound to one
open build-mode document" (`crates/astrolith-gpui/src/editor/lsp.rs:25-27`). Consequently: N `.astro`
tabs in Build mode ⇒ N `astro-ls` node processes, each indexing the whole project.
**Recommendation: Adopt the seed+refcount model (highest-value LSP change).** Key = `(project_root,
LanguageId)`; store `HashMap<LspKey, LspClient>` on the shell or a `LanguageServers` global, and have each
session acquire a handle. Cost: ~1 day (the client already supports multiple documents: `ClientHandle::open`
takes a `uri` and a shared `version` counter, `crates/astrolith-lsp/src/client.rs:184-208`); risk: shutdown
ordering — the map owner must outlive sessions, and `stop_lsp` currently drops the client process
(`session.rs:1612-1625`) which must become a refcount decrement. Rust's type system will not catch a leaked
process; add a test that opens two documents and asserts one spawn (use the existing fake-server harness at
`client.rs:951-1030`).

### L2 — Request cancellation is "drop the future", and it emits `$/cancelRequest`

Zed does **X**: `request_internal_with_timer` (`crates/lsp/src/lsp.rs:1529-1640`) allocates the id from an
`AtomicI32`, inserts a `ResponseHandler` keyed by `RequestId` **before** sending, and returns an
`LspRequest` future. If that future is dropped before completion, `gpui_util::defer` fires and sends
`notification::Cancel` with the id (`lsp.rs:1594-1610`). On timeout the handler entry is **removed**, so a
late reply hits `resolve` → `pending.remove(&id)` → `None` → logged and discarded
(`lsp.rs:1601-1622` and `Connection::resolve`, `lsp.rs:426-445`).
**Because** Y: cancellation is RAII; there is no explicit registry of "things to cancel on next keystroke",
and the id map cannot grow when a server never answers.
**Astrolith** Z: `ClientHandle::request` returns `async move { … receiver.await … }`
(`crates/astrolith-lsp/src/client.rs:210-229`) with **no timeout, no `$/cancelRequest`, and no
cleanup of `Connection::pending` when the caller drops the future** (`client.rs:355-360` `pending:
HashMap<i64, Pending>`; `resolve` at `client.rs:426-444`). A completion request abandoned by the user
stays in `pending` until the server replies; a server that never replies leaks one entry per keystroke.
**Recommendation: Adopt, minimally.** Three changes: (1) wrap each request in a `tokio::time::timeout`
with a configurable budget; (2) `Connection::resolve` already removes on reply — add removal on
timeout/abandon (the oneshot `sender.send` failing is an existing signal: check `sender.is_closed()` in a
sweep, or keep an `Instant` per entry and reap in `serve`'s select loop); (3) send `$/cancelRequest` for
abandoned ids. Cost: ~half a day in `client.rs`; risk: the actor loop (`client.rs:642-690`) is a single
`select!` — the reap must not hold `pending` across an await. Not doing this is a steady memory/fd-adjacent
leak in any completion-heavy session **[Hypothesis: bounded by responses, unmeasured]**.

### L3 — Stale-response handling: snapshot retention per (buffer, server, version)

Zed does **X**: `buffer_snapshots: HashMap<BufferId, HashMap<LanguageServerId, Vec<LspBufferSnapshot>>>`
(`lsp_store.rs:390`) keeps each server's own version→snapshot history;
`buffer_snapshot_for_lsp_version` binary-searches the requested version and **retains the last 10 versions**
(`const OLD_VERSIONS_TO_RETAIN: i32 = 10`, `lsp_store.rs:3500-3540`), special-casing `version == 0` (some
servers send 0 before open) as "use current snapshot" (`lsp_store.rs:3520-3524`). A missing snapshot is a
hard error, not a silent fallback to current text.
**Because** Y: a `textDocument/codeAction` computed on version 41 must resolve ranges against version 41,
not against 43. Per-server versioning is required because each server is told a different version sequence.
**Astrolith** Z: exactly **one** version counter per client, shared by all documents
(`client.rs:186-187` comment "Document version shared by every provider for this open file"), and **no
snapshot history at all** — `range_offsets(text, range)` converts LSP ranges against the *current* text
(`client.rs:877-890`). Editing while a code action is in flight therefore maps ranges onto shifted text.
**Recommendation: Adopt (defer the history, add the guard).** Minimum viable: capture
`(DocumentRevision, text len)` at request time and drop any response whose revision no longer matches,
instead of converting ranges onto new text. Full snapshot retention is overkill for one buffer — a single
previous snapshot would cover the common "one keystroke behind" case. Cost: ~2 hours for the guard; risk:
dropping responses makes some provider results disappear during fast typing (acceptable: the provider
re-requests).

### L4 — Document sync: incremental changes per server, from a diff, with full-sync fallback on line-ending change

Zed does **X**: `buffer.edits_since::<Dimensions<PointUtf16, usize>>(previous_snapshot.version())` produces
`TextDocumentContentChangeEvent`s with ranges (`lsp_store.rs:9710-9735`); the sync kind comes from the
server's capability: `TextDocumentSyncKind::FULL` ⇒ one whole-document change, `INCREMENTAL` ⇒ the diff
(`lsp_store.rs:9740-9760`). If the line ending changed, it forces a full change. Version is
`previous + 1` per server, pushed as `VersionedTextDocumentIdentifier` (`lsp_store.rs:9762-9778`).
**Because** Y: incremental sync keeps typing O(edit) on the wire; per-server counters respect the spec.
**Astrolith** Z: `sync_lsp_change` (`session.rs:1652-1664`) debounces **150 ms** on the background
executor, then `handle.change(uri, text)` sends **the complete document** as a single
`contentChanges: [{"text": text}]` (`client.rs:716-724`). The whole text was cloned on the UI thread before
the task was spawned (`session.rs:1659`), and the client never reads `textDocumentSync` from
`ServerCapabilities` (no such field is stored; `initialize_params` only advertises client capabilities,
`client.rs:518-580`).
**Recommendation: Defer.** Full sync at 150 ms is correct and the code is 10 lines. Cost of incremental:
a diff engine plus per-server version bookkeeping plus capability handling — several days for a bandwidth
win that does not matter for one file. Do fix the clone: move it inside the spawned task by handing over
`Arc<str>` built once at change time, or send `(revision, offset-range)` and let the task read the buffer.
Risk of the current form: 100 KB × 6.6 keystrokes/s of allocation churn (Hypothesis, unmeasured).

### L5 — Diagnostics: merged per (worktree, path, server), replaced only for the publishing server

Zed does **X**: storage is `diagnostics: HashMap<WorktreeId, HashMap<Arc<RelPath>,
Vec<(LanguageServerId, Vec<DiagnosticEntry<…>>)>>>` (`lsp_store.rs:378-388`);
`merge_diagnostic_entries` (`lsp_store.rs:10285-10375`) finds the worktree, picks the buffer if open,
**reuses** the previous diagnostics of other servers that pass the caller's `merge` predicate, and replaces
only this server's slice. Sort order is `binary_search_by_key(&server_id, …)` (`lsp_store.rs:10355-10362`).
`lsp_to_document_diagnostics` also sorts by severity so primary diagnostics stay most-severe
(`lsp_store.rs:13420-13425`).
**Because** Y: multiple servers legitimately describe one file (ts + astro + spell); a later publisher must
not erase an earlier one.
**Astrolith** Z: `apply_diagnostics` **replaces the entire set** (`*current = set`,
`crates/astrolith-gpui/src/editor/lsp.rs:104-116`) and **ignores the `version` field** even though the
client carries it (`client.rs:110-112`, event matched with `..` at `session.rs:1565-1575`). Combined with
L1 (one server per document) the merge problem does not exist yet, but the version problem does: a stale
`publishDiagnostics` from an older version overwrites a newer one, and `set` is rebuilt from
`editor.text().clone()` + `to_string()` on every publish (two full copies per publish,
`editor/lsp.rs:106-108`).
**Recommendation: Adopt the version check now (2 lines), Defer the merge map.**
Check `event.version` against the version this client last sent for that URI and drop older publishes.
Cost: ~1 hour with a test using the fake server (`client.rs:951-1030` already asserts
`version == Some(1)`).

### L6 — Server lifecycle: rate-limited start, explicit stop, restart task tracking

Zed does **X**: `language_server_restart_tasks: Vec<Task<()>>` (`lsp_store.rs:400`) keeps restarts
observable; `stop_local_language_server` returns a future joined through
`futures::future::join_all` (`lsp_store.rs:13340-1352`); `LanguageServerState::{Starting, Running, Error}`
distinguishes a starting server from a running one so requests do not queue against a half-initialised
process (`running_language_server_for_id` at `lsp_store.rs:429-433`).
**Astrolith** Z: `lsp_start: Option<Task<()>>` + `lsp_unavailable: bool` latch so a failed discovery is not
retried on every mode switch (`session.rs:58-63` comment, `session.rs:1487`, error path
`session.rs:1508-1522`). No "starting" state visible to providers: `install` happens only on success
(`session.rs:1553`), so providers simply do not exist while starting.
**Recommendation: Adopt (already adequate).** The `lsp_unavailable` latch is a genuinely good call — keep it
and document why (`session.rs:58-63` already does).

### L7 — What not to copy

- **Workspace folders, dynamic registration, pull diagnostics** (`lsp_store/dynamic_registration.rs`,
  `insert_synced_server_capabilities` at `lsp_store.rs:5979`) — Astrolith pushes one root, one server per
  (root, language), push diagnostics. **Reject.**
- **Remote/upstream LSP proxying** (`upstream_client`, `lsp_store.rs:4932`) — **Reject** (no collab).
- `lsp_store.rs` is 18 808 lines: a warning, not a target. Astrolith's 1 145-line `client.rs` is the right
  order of magnitude.

---

## 4. Terminal

### T1 — Output coalescing: first event immediate, then a 4 ms drain with a wakeup collapse and a 100-event cap

Zed does **X**: `Terminal::subscribe` (`crates/terminal/src/terminal.rs:1420-1500`) processes the **first**
PTY event immediately for latency, then enters a `futures::select_biased!` loop that drains more events for
up to **4 ms**, folding every `TerminalBackendEvent::Wakeup` into a single `wakeup = Some(generation)`, caps
a batch at 100 events, applies the batch inside one `terminal.update(...)`, and `yield_now().await`s so the
foreground executor is not starved (`terminal.rs:1428-1490`).
**Because** Y: a `cargo build` flood becomes ~250 window updates/s instead of thousands, and cursor work is
done once per batch rather than once per write.
**Astrolith** Z: the reader thread parses under the screen lock and bumps `revision: AtomicU64`
(`astrolith-terminal/src/lib.rs:122-142`); the view polls every **16 ms** and notifies only when the revision
moved (`crates/astrolith-gpui/src/terminal/mod.rs:231-275`). This is a *pull* equivalent of Zed's batching —
arguably better: no cross-thread notification at all, and the 16 ms poll is a natural coalescer.
**Recommendation: Adopt (keep the poll), fix the render cost (T2).** The revision-poll design is sound and
simpler than Zed's; it is the *rendering* that is expensive.

### T2 — Rendering only visible rows: Zed extracts viewport cells; Astrolith builds every cell as an element

Zed does **X**: `make_content` (`crates/terminal/src/alacritty.rs:904-955`) walks
`term.renderable_content().display_iter` — the **viewport only** — into `Vec<IndexedCell>`; the `Content`
carries `cells`, `columns`, `screen_lines`, `display_offset`, `total_lines` for the view to reason about
scroll state without materialising scrollback (`terminal.rs:491-512`). Row count comes from
`TerminalBounds::num_lines()` derived from the *pixel* height and line height
(`terminal.rs:766-800`), i.e. the terminal's grid is exactly the visible grid, and scrollback is handled by
`display_offset` (`scroll_up_by`/`scroll_down_by`, `terminal.rs:2131-2150`).
**Astrolith** Z: `render_rows` (`terminal/mod.rs:345-414`) loops `for row in 0..rows` × `while col < cols`
and builds a `CellView` per cell; `render` then maps them to nested `div()`s —
one element per cell, `.w(px(CELL_WIDTH * width_cells))` (`terminal/mod.rs:606-628`). Grid size is clamped to
`cols 20..500`, `rows 3..200` (`terminal/mod.rs:283`). At the default 80×24 that is ~1 920 elements per
frame; at the clamp it is up to **100 000** elements per frame (Confirmed by arithmetic on
`terminal/mod.rs:283`), rebuilt on every revision change.
**Because** Zed paints text runs/cells in a custom element (`terminal_view`, not in this checkout — see Open
questions), not as a div tree.
**Recommendation: Adapt (highest-value terminal change).** Keep `vt100` and the poll; replace the div grid
with **one custom `Element`** that (a) shapes each visible row once per revision into a `TextRun`-per-style
list (max 16 runs/row by attribute change), and (b) paints them under one `content_mask`. Element count
drops from `rows × cols` to 1. Cost: ~1-2 days (hit-testing for selection already exists via
`point_at`, `mod.rs:286-302`, and would move into the element). Risk: IME/caret drawing and the
selection highlight currently ride on per-cell `.bg()` — they must become paint-time rectangles; keep the
current div grid behind a fallback for one release. Intermediate cheap win: clamp to the *actual visible*
rows and skip wide-continuation cells (already skipped, `mod.rs:367-370`) and skip trailing blank cells with
default background per row (typically 60-80% of an 80-column line) — ~15 lines, zero risk, immediately
cuts elements several-fold.

### T3 — PTY ownership: alacritty `tty` + a dedicated event-loop thread; resize goes through one channel

Zed does **X**: `open_pty` → `tty::new(options, window_size_from_terminal_bounds(bounds), window_id)`
(`alacritty.rs:180-190`); `spawn_event_loop` builds an `EventLoop` and `event_loop.spawn()`s a dedicated
thread, returning a `PtySender` whose only job is `resize`/write through the channel (`alacritty.rs:214-238`).
Resize is one message: `InternalEvent::Resize(new_bounds)` → `pty_tx.resize(new_bounds)` **and**
`resize(term, new_bounds)` **and** `reset_cwd_history()` if columns changed **and** a `Wakeup` if search
matches exist (`terminal.rs:1741-1765`).
**Astrolith** Z: `portable_pty` master + child behind `Mutex` (`astrolith-terminal/src/lib.rs:36-41`),
a reader thread per terminal (`lib.rs:122-142`), and `resize` that resizes the PTY then the parser and bumps
the revision (`lib.rs:193-209`). No resize coalescing: the view calls `session.resize(cols, rows)` whenever
its measured grid differs (`terminal/mod.rs:254-260`), and `resize` does a `master.resize` syscall on the
poll task's thread (a GPUI background task, not a dedicated thread).
**Because** Zed's resize path also invalidates search matches and cwd history, which Astrolith does not have.
**Recommendation: Adopt the "single resize path + invalidate derived state" shape, Reject the thread
topology.** Astrolith's is simpler and the syscall frequency is at most one per poll tick. Add: skip resize
while the process is exited (a resize on a dead master is an `Err` that currently sets a user-visible
failure string, `terminal/mod.rs:255-257`). Cost: ~15 min.

### T4 — Scrollback: bounded by config, default 10 000, max 100 000 lines

Zed does **X**: `DEFAULT_SCROLL_HISTORY_LINES = 10_000`, `MAX_SCROLL_HISTORY_LINES = 100_000`
(`terminal.rs:891-892`); task terminals get the max, interactive ones get
`settings.max_scroll_history_lines.unwrap_or(DEFAULT).min(MAX)` (`terminal.rs:1211-1220`), fed to
`term.set_options` via `Config.scrolling_history` (`alacritty.rs:123-145`).
**Astrolith** Z: `SCROLLBACK_CAPACITY = 2_000` fixed (`astrolith-terminal/src/lib.rs:27`), passed to
`vt100::Parser::new(rows, cols, SCROLLBACK_CAPACITY)` (`lib.rs:111-115`); asserted by test
`lib.rs:406`.
**Recommendation: Adapt.** 2 000 lines is too small for `npm run build` output (Zed's 10 000 is the
practical floor; `cargo build` of a large crate easily exceeds 2 000). Raise the default to 10 000 and add
a preference, clamped to a documented max. Memory: ~2 lines × chars × style per row, so 10 000 rows of 120
cols ≈ 10-25 MB with `vt100`'s per-cell structs **[Hypothesis: not measured; check `vt100` cell size before
committing]**. Risk: a bounded-capacity surprise is far cheaper than unbounded growth.

### T5 — Input translation and terminal-bound invariants

Zed does **X**: table-driven `mappings/keys.rs` (459 lines) + `mappings/mouse.rs` (318), and
`TerminalBounds::num_lines` rounds **down** so the first line cannot be pushed into invisible scrollback
(`terminal.rs:781-788`).
**Astrolith** Z: `translate_key` is a ~70-line `match` (`terminal/mod.rs:652-720`); no mouse reporting, no
bracketed paste or application-cursor-key modes. Its grid uses `floor()` on bounds ÷ metrics
(`terminal/mod.rs:281-283`), so the round-down invariant already holds.
**Recommendation: Defer.** Astrolith's terminal is a build-output viewer with an interactive shell; mouse
reporting and vi mode are not needed. Keep the round-down invariant.

---

## 5. Concurrency

### C1 — Two executors with a hard rule: `Send` work goes to the background pool, UI entities only on the foreground executor

Zed does **X**: `ForegroundExecutor` wraps a `LocalExecutor` and carries `PhantomData<Rc<()>>` so it cannot
cross threads (`crates/gpui/src/executor.rs:27-35`); `BackgroundExecutor::spawn` requires
`R: Send + 'static` (`executor.rs:120-145`); `SendForegroundExecutor` exists for the rare "from another
thread, run on the main thread" case (`executor.rs:43-54`). `Priority` (including `RealtimeAudio`, which
gets a dedicated thread) selects queue class: `spawn_with_priority` (`executor.rs:132-145`).
**Astrolith** Z: identical split, plus a hard boundary: a private tokio runtime
`Builder::new_multi_thread().worker_threads(2).max_blocking_threads(4)` named `astrolith-service`
(`crates/astrolith-gpui/src/runtime.rs:38-48`), and the documented invariant "UI entities never cross this
boundary" (`runtime.rs:1-2`).
**Recommendation: Adopt (already matching).** One tuning note: 2 worker threads for a Node-spawning,
SCXML-ish workload is thin if more than two service futures ever block on `await`-ing subprocess pipes.
Cost of raising: nothing but memory (~2 MB/thread); risk: none. Verify with a measurement before changing.

### C2 — Cancellation is dropping the `Task`; `detach()` opts out

Zed does **X**: `Task<T>` wraps `async_task::Task` and **cancels on drop**; `detach()` runs to completion
(`crates/scheduler/src/executor.rs:433-437` doc + `:610-618` `detach`) [git]. Production code relies on it
everywhere — e.g. the LSP cancel-on-drop `defer` (L2), and `Editor.serialize_selections: Task<()>` being
replaced on each change (`editor.rs:1202`).
**Because** Y: one cancellation idiom, no abort handles to track.
**Astrolith** Z: GPUI `Task` drops do cancel for the GPUI side, but a `tokio::spawn` joined into a GPUI task
would **not** be cancelled on drop (the tokio task keeps running detached). `runtime::spawn` fixes exactly
this with `AbortOnDrop(join.abort_handle())` constructed *before* the bridge is polled, with a comment
explaining why (`runtime.rs:77-82`) and a regression test
`dropping_unpolled_gpui_handle_aborts_its_service_future` (`runtime.rs:245-273`).
**Recommendation: Adopt/maintain — this is a real correctness win that Zed does not need.** Note the
documented limitation that a started `blocking` permit is not interruptible (`runtime.rs:99-122` comment).
Risk: any new `runtime::spawn` wrapper must be used instead of raw `tokio::spawn`; add a clippy
`disallowed_methods` entry for `tokio::spawn` if the team wants enforcement — otherwise this invariant will
rot.

### C3 — Blocking work never runs on the UI thread, and the blocking lane is bounded

Zed does **X**: `BackgroundExecutor::scoped` / `scoped_priority` run bounded fan-outs with
`self.executor.num_cpus()` tasks (worktree scan: `crates/worktree/src/worktree.rs:5306` [git]), and dropping
the scope future cancels its tasks and synchronously waits for their destruction
(`executor.rs:145-175`).
**Astrolith** Z: `runtime::blocking` acquires a semaphore permit (2) and calls `tokio::task::spawn_blocking`
(`runtime.rs:99-122`); the watcher poll, project scan and save all go through it
(`project_discovery.rs:3190-3200`, `astrolith-document/src/save.rs:132`).
**Recommendation: Adopt (already matching).** C5 (timers): both trees debounce with
`cx.background_executor().timer(d)` inside a spawned task, never `thread::sleep`
(Zed: `terminal.rs:1430-1435`, `lsp_store.rs:5365-5370`; Astrolith: `terminal/mod.rs:237-239`,
`session.rs:1660-1662`, `project_discovery.rs:3168-3170`; the only `thread::sleep` is a test helper,
`astrolith-terminal/src/lib.rs:322`). Keep.

### C4 — Shutdown ordering: quit observers get 200 ms, then the process dies

Zed does **X**: `App::shutdown` (`crates/gpui/src/app.rs:1274-1320`) removes quit observers, runs their
futures via `foreground_executor.block_with_timeout(SHUTDOWN_TIMEOUT, futures)` with
`SHUTDOWN_TIMEOUT = 200 ms` (`app.rs:80`), then clears windows and sets `quitting`.
**Astrolith** Z: `runtime.rs:125-134 flush_on_quit` deliberately bypasses the GPUI task and uses
`handle.spawn_blocking` because "GPUI only gives quit observers 200 ms" (`runtime.rs:30-31` comment), with a
test that a 250 ms blocking flush still completes (`runtime.rs:165-175`); the runtime itself does
`shutdown_timeout(2s)` on drop (`runtime.rs:30-32`).
**Recommendation: Adopt/maintain.** This is the right workaround and it is documented. Risk: the 2 s
`shutdown_timeout` runs on the *main thread* during `Drop` of the global, so a hung blocking task delays
exit up to 2 s. That is the intended trade; keep a log line when it is exceeded (currently silent).

### C5 — Timers

Folded into C3 above (both trees use `background_executor().timer`; verification lines listed there).

---

## 6. Worktree scanning and filesystem watching

### W1 — One background scanner owns all FS state; events are drained into a single batch before processing

Zed does **X**: `BackgroundScanner::run` (`worktree.rs:4381-4670` [git]) holds the only mutable snapshot.
The event arm drains **all immediately-ready batches** before acting:
`while let Poll::Ready(Some(more)) = futures::poll!(fs_events_rx.next()) { paths.extend(more) }` then one
`process_events(paths)` (`worktree.rs:4640-4645`). Watcher latency is `FS_WATCH_LATENCY = 100 ms`
(`worktree.rs:84`), passed to `fs.watch(path, FS_WATCH_LATENCY)` (`worktree.rs:4540`). A `select_biased!`
prioritises user scan requests above FS events (`worktree.rs:4597-4612`).
**Because** Y: hundreds of events from one `git checkout` become one state transition; user-initiated
refreshes are never starved by event storms.
**Astrolith** Z: `CollectionWatcher::drain` (`crates/astrolith-workspace/src/watcher.rs:226-272`) does the
equivalent well: `try_recv` up to `WATCH_CHANNEL_CAPACITY` (64), require a `WATCH_MERGE_WINDOW` (100 ms) of
quiet, and turn channel overflow/errors into `needs_rescan`
(`watcher.rs:180-200`, `:244-262`). The caller polls every 100 ms
(`project_discovery.rs:3168-3178`).
**Recommendation: Adopt (already matching).** Same latency constant, same overflow policy. Keep.

### W2 — But Astrolith's *reaction* to a batch is a full project rescan; Zed's is per-path

Zed does **X**: events carry paths. `process_events` (`worktree.rs:4852+`) classifies them: `.git`-internal
noise is filtered by name/dir allowlists (`worktree.rs:4884-4900`), bare `.git` events are deferred so they
cannot loop (`worktree.rs:4930-4945`), and the survivors go to `reload_entries_for_paths`
(`worktree.rs:5687`) or `forcibly_load_paths` (`worktree.rs:5249`). Targeted refresh API:
`refresh_entries_for_paths(Vec<RelPath>)` (`worktree.rs:2121-2132`), `add_path_prefix_to_scan`
(`worktree.rs:2140-2150`), both with `barrier` channels so callers can await completion.
**Astrolith** Z: `WatchBatch` is three booleans — `changed`, `config_changed`, `needs_rescan`
(`watcher.rs:35-43`) — with **no paths**. The consumer turns `changed || needs_rescan` into a whole-project
rescan: `inspect_project(&request)` + `scan_collections(&manifest, cancellation)`
(`project_discovery.rs:3183-3187` → `:3250-3268`). `scan_collections`
(`astrolith-workspace/src/index.rs:299-400`) walks every collection directory and re-reads **frontmatter of
every Markdown/MDX/YAML entry** (`index.rs:390-400` → `frontmatter_metadata`).
**Because** Y: after one keystroke-save, Astrolith re-reads every post's frontmatter. For a 500-post blog
that is 500 file opens per save (Confirmed arithmetically from the loop at `index.rs:344-400`; wall time
unmeasured).
**Recommendation: Adapt — make the batch carry paths (highest-value workspace change).** `drain()` already
sees individual `notify::Event`s; accumulate a deduped `Vec<PathBuf>`, falling back to
`needs_rescan = true` above ~1 000 paths. Then re-index only the changed entries, keeping
`scan_collections` as the fallback when `config_changed || needs_rescan || paths.is_empty()`.
Cost: ~2-3 days (index splice + duplicate-ID revalidation at `index.rs:380-390` + tests); risk: the index
feeds `publish_list_state`, which rebuilds the whole `ListState` per publish (`project_discovery.rs:6065`) —
a splice must preserve scroll/selection. Mitigation: land paths first plus a *targeted*
`scan_collections` variant taking a path filter (1 day, reuses the walker, no index surgery).

### W3 — Ignore rules are a per-directory stack, evaluated during the walk; plus a depth escape hatch

Zed does **X**: `Snapshot::ignore_stack_for_abs_path` (`worktree.rs:3090`) builds an `IgnoreStack` per
directory, consulted via `is_abs_path_ignored` while scanning (`worktree.rs:4493,5234,5757`); sources are
per-directory `.gitignore`, the global gitignore and `$GIT_DIR/info/exclude` (`worktree.rs:4451-4470`,
`ignore.rs` [git]); `is_beyond_scan_depth` (`worktree.rs:6555-6557`) marks deep directories
`EntryKind::UnloadedDir`, loaded lazily by `expand_entry` (`worktree.rs:1091-1110`).
**Because** Y: `node_modules`/`target` never enter the tree; a deep repo costs O(scan depth), not O(repo).
**Astrolith** Z: name-based skips (`watcher.rs:531-545`), a registration planner that watches only collection
bases + config parents (`watcher.rs:275-290`), and hard caps `MAX_PROJECT_WATCH_*` (`watcher.rs:27-34`),
`MAX_SCAN_*` (`index.rs:22-28`). Astrolith only indexes Astro content collections, so it never needs a
whole-repo tree.
**Recommendation: Reject gitignore machinery, Adopt two cheap pieces.** (1) A `.gitignore`-aware skip inside
collection directories is genuinely needed (`draft/`, generated output); the `ignore` crate costs one
dependency. (2) Keep failing closed on the caps, but surface the cap **and the offending path** as a
project-level diagnostic instead of an "could not open project" string (`index.rs:322-340`). Cost: ~2 hours.

### W4 — Snapshot/scan-id protocol: callers can await a specific scan

Zed does **X**: `Worktree::snapshot()` + monotonically increasing `scan_id` / `completed_scan_id`
(`worktree.rs:823-856`, incremented at `worktree.rs:4505-4510`); `wait_for_snapshot(scan_id)`
(`worktree.rs:905-915`) and `scan_complete()` (`worktree.rs:1616`) let callers await convergence; file
creation/writes go through the same scanner (`create_entry`, `write_file`,
`worktree.rs:964-1075`) so the watcher's echo of our own write cannot race the tree.
**Because** Y: "did the tree see my write yet?" needs an answer that is not a sleep.
**Astrolith** Z: the equivalent is the index-request token protocol — `begin_index_request()` returns a
`u64`, `apply_watcher_scan` drops results whose token is older than `published_index_request`
(`project_discovery.rs:3300-3310`), and `accepts_index_result`/`finish_index_request` implement the ordering.
Astrolith writes directly with `fs` on the blocking lane (`astrolith-document/src/save.rs:137-160`) and then
learns about it via the watcher.
**Recommendation: Adopt (already matching, and Astrolith's token protocol is the more important half).**
One gap: after a save, Astrolith *does not* update the index itself; it waits for the watcher batch
(≥100 ms quiet + 100 ms poll). Bound that: after a successful save, feed the path directly into the
targeted-reindex API from W2 rather than waiting for the watch. Cost: ~2 hours once W2 exists. Risk: none.

### W5 — Root liveness

Zed does **X**: a `ROOT_PATH_CHECK_INTERVAL = 5 s` timer canonicalises the worktree root and reports
moved/deleted once (`worktree.rs:90`, `:4615-4630`).
**Astrolith** Z: no periodic root check; a moved project root produces watcher errors (warned at
`watcher.rs:184-190`) and eventually `needs_rescan` on every batch.
**Recommendation: Defer.** Low value for a desktop editor the user is looking at. Revisit only if "project
folder deleted while open" generates log spam in the field.

---

## Priority summary

| # | Finding | Verdict | Cost | Why it matters |
|---|---|---|---|---|
| L1 | Server per (root, language) with refcount, not per document | **Adopt** | ~1 day | N open tabs ⇒ N node language servers today (`session.rs:1480-1560`, `lsp.rs:25-27`) |
| T2 | Terminal: one custom element instead of `rows × cols` divs | **Adapt** | 1-2 days (step 1: 15 min) | up to 100 000 elements/frame at the clamp (`terminal/mod.rs:283,606-628`) |
| E1 | Incremental splice instead of 5 full-document passes per keystroke | **Adapt** | ~1 day | typing cost is O(document) × 5 + full parse (`session.rs:1335-1378`) |
| W2 | Watch batch carries paths → targeted reindex | **Adapt** | 1-3 days | every save re-reads every entry's frontmatter (`index.rs:344-400`) |
| L5 | Check `publishDiagnostics.version` | **Adopt** | ~1 hour | stale publishes overwrite newer (`editor/lsp.rs:104-116`) |
| L2 | LSP request timeout + cancel + pending cleanup | **Adopt** | ~half day | unbounded `Connection::pending` growth (`client.rs:355-360`) |
| T4 | Scrollback 2 000 → 10 000 + preference | **Adapt** | ~1 hour | build output truncated today (`lib.rs:27`) |
| E4 | Bounded sync parse then background, keyed by revision | **Adapt** | ~1 day | render-time parse blocks the frame (`writer_surface.rs:94`) |
| R5 | Virtualize the writer surface | **Defer until measured** | 2-3 days | selection/caret plumbing risk exceeds current evidence |
| R1-R3, C1-C5, W1, W3-W5, E2, L6 | Already equivalent or intentionally simpler | **Keep / Reject** | 0 | do not import Zed machinery that gpui-base 0.8.1 does not expose |

**Single most important risk:** the writer's per-keystroke full-document pipeline (E1) combined with the
all-blocks re-render (`writer_surface.rs:485-537`) and the all-cells terminal grid (`terminal/mod.rs:606-628`)
means Astrolith's frame/keystroke cost is a function of *document and terminal size*, not of what changed.
Zed's whole architecture is an answer to exactly that; adopting three of its ideas (incremental splice,
visible-only list, single-element terminal paint) removes the size dependence without importing its
MultiBuffer/Project machinery.

## Open questions / not inspected

- `crates/terminal_view` is **not** in the sparse checkout, so "Zed paints the terminal as one element" is
  inferred from `make_content` returning a viewport-sized `Vec<IndexedCell>` plus a `TextRun`-oriented
  `Content` struct (`terminal.rs:491-512`) and from the absence of any per-cell element type in
  `crates/terminal`. Verify before quoting as fact.
- `crates/fs` (realfs thread pool) and `crates/scheduler` runtime (`Scheduler` trait impl for the platform)
  are outside the checkout; the thread-count and blocking-lane claims in C1/C3 are from `executor.rs`
  signatures and priority plumbing only.
- All Astrolith cost claims (keystroke latency, element counts at the terminal clamp, rescan wall time) are
  arithmetic or code-read, not measured. The three measurements worth taking before acting:
  1 MB document typing latency; frame time with a 200×500 terminal; `scan_collections` wall time on a
  500-post fixture.
- `gpui-component`'s `ListState` splice/reset cost for prose-replacement (R5 risk) was not profiled; the
  relevant code is `gpui-component-0.8.1/src/list/cache.rs` and `list.rs:490-540` (registry).
