# Editor capability contract — pinned GPUI Kit 0.8.1

Date: 2026-11-01. Status: **P0 deliverable, frozen evidence.** This document is the single
authority for whether a specific writer command is proven, degraded or unavailable on the pinned
GPUI Kit facade. [`astro-editor-parity.md`](astro-editor-parity.md) fixes the user-facing scenarios
and defers to this file for capability status (`astro-editor-parity.md` §3, editor-command note).

Two questions are answered here, and they are not the same:

1. **Is the behavior reachable through public API in the exact installed versions?** Answered by
   source inspection of the installed crates plus headless tests on the GPUI test platform.
2. **Has the behavior been observed in a real window, with a real compositor, keyboard and IME?**
   Almost nothing in this contract has. Where it has not, this document says so and no row claims
   otherwise.

Nothing in this document authorizes vendoring, forking, `#[doc(hidden)]` presentation installers, or
any reach into private state. A capability that requires one of those is recorded as **Blocked** and
becomes an upstream request.

## 1. Environment and pinned versions

| Item | Pinned value | Source of truth |
|---|---|---|
| `gpui-kit` | `=0.8.1` | `Cargo.toml` `[workspace.dependencies]` |
| `gpui-base` | 0.8.1 | resolved lock graph / installed registry |
| `gpui-component` | 0.8.1 | resolved lock graph / installed registry |
| `gpui-kit-assets` | 0.8.1 | resolved lock graph / installed registry |
| GPUI snapshot (`gpui-pre`) | 0.3.10 | resolved lock graph |
| Linux platform backend (`gpui-pre-linux`) | 0.3.10 | resolved lock graph |
| Rust | 1.99.1 (`58cf8f9c3`, 2026-10-13), edition 2024 | `mise.toml` / workspace `rust-version` |
| Mise | 2026.10.19 | `mise --version` on the evidence host |
| Host | CachyOS Linux 7.3.0-4-cachyos, x86_64 | host |
| Native session used for prior evidence | Wayland, Hyprland 0.56.2, scale 1 | `editor-spike.md` |

Application code depends on the `gpui-kit` facade only. `gpui_kit::*` is GPUI
(`gpui-kit-0.8.1/src/lib.rs:101`), `gpui_kit::base` is `gpui-base` (`:130`), `gpui_kit::component`
is `gpui-component` (`:170`), `gpui_kit::assets` is `gpui-kit-assets` (`:173`), and
`gpui_kit::test` (`:128`) is the only test harness used here. Do not add direct `gpui-base` or
`gpui-pre` dependencies to read "just one more field": the facade is the contract, and a facade
omission is exactly what §10 is for.

All source paths below are relative to the installed registry root
`$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/` and carry line numbers from the exact
0.8.1/0.3.10 files read on 2026-11-01. **Line numbers are version-specific; symbol names are the
durable part.** Re-read the symbol on any version bump (§12).

Context7 resolves `gpui-kit` to `/longbridge/gpui-kit`, which publishes **only v0.8.0** documentation
at the time of writing (verified by `resolve-library-id`; no v0.8.1 doc set is served). Those docs are
a secondary source with a known horizon: a query about multicursor editing returns the Alt-click and
Alt+Shift interactions and no programmatic selection API at all, while the installed 0.8.1 source has
a public `add_selection`. Absence from a doc query is not evidence of absence in the crate, and
presence in a doc query is not evidence of presence in the pinned version. Where docs and installed
source disagree, the installed source wins; the `add_selection` discrepancy is recorded in §5.

## 2. Evidence classes

Every status in §4 cites the classes it actually has. A row with only E1 is a source-inspection
claim; it is not a behavior report.

| Class | Meaning | What it can establish | What it cannot |
|---|---|---|---|
| **E1 — source inspection** | Direct reading of the installed 0.8.1 / 0.3.10 crate sources at the pinned versions. | That a symbol is `pub`, its exact signature and visibility, its call sites and its mode gating. | That the code path works when rendered, or that the platform delivers the input. |
| **E2 — headless automated test** | `#[gpui_kit::test]` on the GPUI test platform (`gpui-kit-0.8.1/src/test.rs:33` `TestWindowExt`; `test/events.rs:14-25` `TestEventExt`; `gpui-pre-0.3.10/src/app/test_context.rs:1068` `simulate_event`). | Dispatch, state transitions, undo grouping, layout/frame computation, geometry values. | Native latency, compositor behavior, OS IME routing, pixels. |
| **E3 — native window/backend** | A real window on the real backend. On 2026-11-01 `mise run dev:editor` mapped a visible 1263×1384 `xwayland=false` window under Wayland/Hyprland; the surface was visually inspected and compositor-delivered image paste, pointer clicks, Alt-click cursor creation, typing and undo were exercised. | That the named launch/render/input path reached the real Wayland backend. | OS IME behavior or file-manager drag unless separately recorded. |
| **E4 — user native interaction** | A person exercising the real window with native input. The user accepted IME, scaling and latency on 2026-10-29 and a Nautilus→Astrolith image-file drop on 2026-11-01. | The exact accepted native scenarios in those sessions. | P4's future complete-source projection/rebuild behavior; it does not exist yet. |
| **E5 — unverified platform behavior** | Platform code paths read but never exercised on a real compositor/file manager. | Nothing on its own; recorded to prevent a later reader from assuming the opposite. | Anything. |

`editor-spike.md` records that the M00.2 suite (preedit refresh guard, selection/undo, clipboard
round-trip, programmatic refresh) passed when that milestone was accepted. That is historical E2.
The implementation plan separately records user acceptance of native IME, scaling and latency on
2026-10-29; that is historical E4 and is not downgraded here. On 2026-11-01 the integration owner ran
`cargo test --locked --package astrolith-desktop public_`: all six current public-API probes passed
(6 passed, 13 filtered out). They establish the single-region projection prerequisite with byte-exact
hidden CRLF bytes and undo, search/current-match/replacement, go-to-line, range/caret geometry,
deferred negative scroll offset, main-selection line-end cursors, public secondary-selection creation,
uniform multi-cursor replacement, image-paste interception and external drop-path delivery on the GPUI
test platform. Native results are recorded separately in §9.3.

## 3. Public surface anchors

### 3.1 Text, projection and programmatic edits

| Symbol | Signature | Anchor |
|---|---|---|
| `text` | `pub fn text(&self) -> &Rope` | `gpui-base-0.8.1/src/input/base/state.rs:1623` |
| `value` | `pub fn value(&self) -> SharedString` | `state.rs:1593` |
| `set_value` | `pub fn set_value(&mut self, value: impl Into<InputContent>, window, cx)` — clears undo history, emits no `Change` | `state.rs:1226` |
| `replace_all` | `pub fn replace_all(&mut self, text: impl Into<SharedString>, window, cx)` — keeps history, emits `Change`, resets selection to `0..0` and drops secondary cursors | `state.rs:1258` (`reset_selection` at `:1335`) |
| `insert` | `pub fn insert(&mut self, text, window, cx)` — inserts at the active caret only; passes `Some(range)` so the multi-cursor branch is **not** taken | `state.rs:1286` |
| `replace` | `pub fn replace(&mut self, text, window, cx)` — passes `None`, so with >1 selection it replaces every selection with the same text in one transaction | `state.rs:1305`; multi-cursor branch `state.rs:4545-4565` |
| `replace_text_in_ranges` | `pub(crate) fn replace_text_in_ranges(&mut self, edits: &[(Range<usize>, String)], window, cx)` | `state.rs:4186` |
| `selected_value` / `selected_text` | `pub fn selected_value(&self) -> SharedString` / `pub fn selected_text(&self) -> RopeSlice<'_>` | `state.rs:1602`, `:4133` |
| `InputEvent` | `pub enum InputEvent { Change, PressEnter { secondary, shift }, Focus, Blur }` — `Change` carries **no** range and no text | `state.rs:132-137` |
| `RopeExt` | `line_start_offset`, `line_end_offset`, `slice_line`, `position_to_offset`, `clip_offset` | `gpui-base-0.8.1/src/input/base/rope_ext.rs:93,105,117,193,242` |

`Rope`, `RopeExt`, `InputEvent`, `InputEdit` are all re-exported through the component facade
(`gpui-component-0.8.1/src/input/mod.rs:25-41`), as are `EditorState`, `EditorMode`, `InputModeKind`
(`:41`) and `lsp_types::Position` (`:63`).

### 3.2 Selections and cursors

| Symbol | Signature / visibility | Anchor |
|---|---|---|
| `cursor` | `pub fn cursor(&self) -> usize` — active caret, UTF-8 bytes | `state.rs:3412` |
| `cursor_position` | `pub fn cursor_position(&self) -> Position` | `state.rs:1628` |
| `set_cursor_position` | `pub fn set_cursor_position(&mut self, position: impl Into<Position>, window, cx)` | `state.rs:1636` |
| `selected_range` | `pub fn selected_range(&self) -> Range<usize>` — active only | `state.rs:3480` |
| `selected_text` | `pub fn selected_text(&self) -> RopeSlice<'_>` — active only | `state.rs:4133` |
| `set_selected_range` | `pub fn set_selected_range(&mut self, range, cx)` — documented to remove additional cursors | `state.rs:3495` |
| `add_selection` | `pub fn add_selection(&mut self, range, cx)` — **public**, documented as the programmatic Alt-click | `state.rs:3521` |
| `select_all` | `pub fn select_all(&mut self, _: &mut Window, cx)` — truncates to one selection | `state.rs:3484` |
| selections storage | `pub(super) selections: Selections` | `state.rs:458` |
| selection types | `pub(super) struct CursorSelection` / `pub(super) struct Selections` | `input/base/cursor.rs:60`, `:134` |
| `AddCursorAbove` / `AddCursorBelow` | public actions, Linux bindings `cmd-alt-up/down`, `ctrl-alt-up/down`, `shift-alt-up/down` | `state.rs:101-102`, `:216-226`; handlers `:2512`, `:2521` |
| `add_cursor_at` | `pub(super) fn add_cursor_at(&mut self, offset, cx)` | `state.rs:2582` |
| `index_for_mouse_position` | `pub(crate) fn index_for_mouse_position(&self, position) -> (usize, bool)` | `state.rs:3565` |
| pointer handling | `pub(super) fn on_mouse_down`; the multi-cursor branch fires on Alt-click (adds a cursor) and Alt+Shift / Linux Ctrl+Alt (block anchor); there is **no** Ctrl-click branch | `state.rs:2659`; multi-cursor branch `:2748-2768` |
| undo restore | `fn restore_selections(&mut self, selections: Option<Vec<CursorSelection>>)` — private | `state.rs:3391` |

### 3.3 Search, replace and navigation

| Symbol | Signature | Anchor |
|---|---|---|
| `open_search` | `pub fn open_search(&mut self, replace_mode: bool, cx)` | `input/editor/search.rs:104` |
| `search_session` | `pub fn search_session(&self) -> &SearchSession` (`#[non_exhaustive]`, public fields incl. `matcher`) | `search.rs:26`, `:136` |
| `search_activation_revision` | `pub fn search_activation_revision(&self) -> u64` | `search.rs:146` |
| `is_replaceable` | `pub fn is_replaceable(&self) -> bool` | `search.rs:160` |
| `set_search_query` | `pub fn set_search_query(&mut self, query: impl Into<String>, case_insensitive: bool, cx)` | `search.rs:175` |
| `close_search` | `pub fn close_search(&mut self, cx)` | `search.rs:202` |
| `next_search_match` | `pub fn ...(&mut self, cx) -> Option<Range<usize>>` | `search.rs:207` |
| `previous_search_match` | same, backwards | `search.rs:216` |
| `replace_current_search_match` | `pub fn ...(&mut self, replacement: &str, window, cx) -> bool` | `search.rs:227` |
| `replace_all_search_matches` | `pub fn ...(&mut self, replacement: &str, window, cx) -> usize` | `search.rs:260` |
| `SearchMatcher` | public `new`, `update(&Rope)`, `update_query(&str, bool)`, `matched_ranges`, `current`, `current_match_index`, `update_cursor_by_offset`, `label`, and `Iterator<Item = Range<usize>>` | `search.rs:13`, `:331-400` |
| go-to-line | `set_cursor_position(Position::new(line, column), …)`; `position_to_offset` clamps an out-of-range row to end-of-document and an out-of-range column to the line end; `Position` is 0-based in **chars** within the line, not bytes or UTF-16; `move_to` → `move_to_with_affinity` → `scroll_to` reveals the caret | `state.rs:1636`; `rope_ext.rs:325-338,316-320`; `input/base/movement.rs:62,80-95` |

Search highlighting and reveal for `EditorMode` are automatic: `EditorState::new` sets
`searchable = true` (`state.rs:12262-12266`), and `open_search` is a no-op when it is false
(`search.rs:105`). Linux bindings are `ctrl-f` → `Search` and `ctrl-h` → `Replace`
(`state.rs:310-316`); the app may rebind but must not assume the panel is the only entry point,
since `SearchMatcher` and the session methods work without it.

### 3.4 Geometry, scroll and decorations

| Symbol | Signature | Anchor |
|---|---|---|
| `range_to_bounds` | `pub fn range_to_bounds(&self, range: &Range<usize>) -> Option<Bounds<Pixels>>` — supported, documented as the selection-popover anchor | `state.rs:4142` |
| `cursor_layout` | `pub fn cursor_layout(&self) -> Option<(Bounds<Pixels>, Pixels)>` — `#[doc(hidden)]`, public | `state.rs:721` |
| `prepaint_caret_geometry` | `pub fn prepaint_caret_geometry(&self) -> Option<PrepaintCaretGeometry>` — `#[doc(hidden)]`, public; exposes `cursor_bounds`, `line_height`, `input_bounds`, `scroll_offset` | `state.rs:731`; struct accessors `state.rs:384-400` |
| `scroll_offset` | `pub fn scroll_offset(&self) -> Point<Pixels>` | `state.rs:3456` |
| `set_scroll_offset` | `pub fn set_scroll_offset(&mut self, offset, cx)` — deferred to next layout, clamped | `state.rs:3463` |
| `visible_row_range` / `line_height` | `pub fn visible_row_range(&self) -> Option<Range<usize>>` / `pub fn line_height(&self) -> Option<Pixels>` | `state.rs:3451`, `:3469` |
| `input_bounds` / `text_bounds` | `pub fn input_bounds(&self) -> Bounds<Pixels>` / `pub fn text_bounds(&self) -> Option<Bounds<Pixels>>` | `state.rs:735`, `:739` |
| `scroll_beyond_last_line` / `cursor_surrounding_lines` | public builder settings for typewriter room and reveal margin | `state.rs:1525`, `:1557` |
| `RangeDecoration` | `pub fn new(range)` / `.with_style(RangeDecorationStyle)` / `.with_color(Hsla)` | `input/editor/decorations.rs:23-45` |
| `TextDecoration` | `pub fn new(range, HighlightStyle)` | `decorations.rs:50-70` |
| `create_range_decorations_collection` | `pub fn ...(&mut self, decorations: Vec<RangeDecoration>, cx) -> RangeDecorationCollection` — impl block is `impl InputBaseState<EditorMode>` | `decorations.rs:510` (impl at `:496`) |
| `create_decorations_collection` | `pub fn ...(&mut self, decorations: Vec<TextDecoration>, cx) -> TextDecorationCollection` | `decorations.rs:542` |
| collection control | `set` / `append` / `clear` / `dispose` / `get_ranges(&App)` | `decorations.rs:102,115,128,136,165,175,185,190,205` |

Decorations track edits exactly as documented upstream: insertion at an edge does not expand the
range, insertion inside does, replacement clips anchors, deletion drops empty ranges, undo does not
resurrect deleted decorations, and `get_ranges` reads back the tracked spans. This is the intended
substrate for copyedit and for snippet placeholders.

### 3.5 Tokens (negative evidence)

| Fact | Anchor |
|---|---|
| `token_api!` is implemented for `InputMode` and `TextareaMode` only — not `EditorMode` | `gpui-base-0.8.1/src/input/base/inline_tokens.rs:425-426` |
| `check_token_mode` rejects `M::CODE_EDITOR` with `InlineTokenError::UnsupportedMode`; `EditorMode::CODE_EDITOR = true` | `inline_tokens.rs:329-330`; `input/editor/mod.rs:15` |
| `install_tokens` drops tokens when `M::CODE_EDITOR` | `inline_tokens.rs:379-388` |
| Presentation installers exist but are `#[doc(hidden)]` and explicitly documented as not part of the supported API | `input/base/token_presentation.rs:188-215` |
| The component `Editor` exposes no `.token`, `.on_token_click` or `.on_token_hover`; only `Input` and `Textarea` do | `gpui-component-0.8.1/src/input/editor.rs` (no matches); `input/input.rs:177-196` |

### 3.6 Clipboard, drop and IME

| Capability | Public path | Anchor |
|---|---|---|
| clipboard read | `App::read_from_clipboard(&self) -> Option<ClipboardItem>`, `read_from_clipboard_async` | `gpui-pre-0.3.10/src/app.rs:1823`, `:1834` |
| clipboard write | `App::write_to_clipboard(&self, item: ClipboardItem)` | `app.rs:1851` |
| payload | `ClipboardItem { entries: Vec<ClipboardEntry> }`; `ClipboardEntry::{String, Image, ExternalPaths}` | `platform.rs:3064`, `:3105-3111` |
| paste interception | `Editor::on_paste(handler: impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool)` — capture phase, before default text paste | `gpui-component-0.8.1/src/input/editor.rs:123` |
| drop types | `ExternalPaths(pub SmallVec<[PathBuf; 2]>)`; `FileDropEvent::{Entered{position,paths}, Pending, Submit{position}, Exited, Ended}` | `gpui-pre-0.3.10/src/interactive.rs:715`, `:758` |
| drop delivery | `InteractiveElement::on_drop::<T>`; delivered on mouse-up while the drop hitbox is hovered, from `cx.active_drag` | `gpui-pre-0.3.10/src/elements/div.rs:592`, `:2962` |
| cross-platform drop mapping | `FileDropEvent::Entered` installs `ExternalPaths` as the active drag; `Submit` becomes a mouse-up | `gpui-pre-0.3.10/src/window.rs:5698-5745` |
| IME | `EntityInputHandler` + `ElementInputHandler::new(bounds, entity)`; `InputHandler::{selected_text_range, marked_text_range, replace_and_mark_text_in_range, unmark_text}` (UTF-16) | `gpui-pre-0.3.10/src/input.rs:37,45,52,152` |
| Linux clipboard reads | Wayland: `read_text().or_else(read_image)` only. X11: image atoms and text atoms only. | `gpui-pre-linux-0.3.10/src/linux/wayland/clipboard.rs:193-207`; `x11/clipboard.rs:1026-1075` |
| Linux clipboard writes | Offers `TEXT_MIME_TYPES` + the self MIME; no `text/uri-list` | `wayland/client.rs:1475-1479` |
| `text/uri-list` on Linux | Declared (`wayland/clipboard.rs:19`), used only for **drag sources** (`client.rs:688`) and DnD payload reads (`:2904,3018`); X11's URI list is commented out (`x11/clipboard.rs:81`) | as cited |
| Linux file drops | Wayland and X11 emit `FileDropEvent::Entered { paths }` from URI lists / XDND | `wayland/client.rs:492-506,2955-2961`; `x11/client.rs:1026-1030` |

## 4. Capability matrix

Status vocabulary: **Proven** = reachable through public API and (where stated) observed; **Blocked**
= not reachable without private state, an upstream change or a platform read that does not exist;
**NA** = not applicable to the Astrolith writer design. "Scope" names the exact limitation, which is
binding: a Proven row with a scope is **not** a promise of full parity.

Mapping to the vocabulary the parity ledger uses: **Proven** covers ledger "proven"; **Proven
(degraded)** covers ledger "degraded" and passes only the narrower behavior in its Scope cell, never
the row as written; **Blocked** covers ledger "unavailable".

| # | Capability | Status | Scope / limit | Evidence |
|---|---|---|---|---|
| 1 | Display projected text; swap the visible buffer | Proven | Whole-buffer replacement only | E1 (`text`, `set_value`, `replace_all`) |
| 2 | Read the complete visible text for save | Proven | `text()` returns `&Rope`; do not materialise per render | E1 |
| 3 | Incremental visible→source edit mapping | **Blocked** | No edit delta exists; must reconstruct by whole-visible diff (§8) | E1 (`InputEvent::Change` unit, `state.rs:132-137`) |
| 4 | Restore the **active** caret after a projection rebuild | Proven | Collapses to one cursor | E1 (`cursor`, `set_selected_range`) |
| 5 | Preserve **secondary** cursors across a projection rebuild | **Blocked** | No enumeration; `replace_all` resets and truncates to one | E1, §5 |
| 6 | Find / replace / next / previous / replace-all | Proven | Replace-all rebuilds the visible text in **one** edit (`search.rs:260-281`), so it collapses secondary cursors and scrolls the view; next/previous return the match range | E1; E2 for find/current replacement + undo (2026-11-01) |
| 7 | Search state owned by the app rather than the panel | Proven | `SearchMatcher` is constructible and independent | E1 |
| 8 | Go to line | Proven | 0-based char column; out-of-range rows clamp to end-of-document (no error), so validate the line against the document before reporting success | E1, E2 (2026-11-01) |
| 9 | Create multiple selections programmatically | Proven | `add_selection`, `AddCursorAbove/Below`, Alt-click, Alt+Shift block | E1, E2 for `add_selection`; E3 for native Alt-click + typing + undo (2026-11-01) |
| 10 | Enumerate existing selections | **Blocked** | `Selections`/`CursorSelection` are `pub(super)` | E1, §3.2 |
| 11 | Select next occurrence | Proven (degraded) | **Active selection only**; per-cursor repetition blocked | E1 |
| 12 | Add cursors at selected line ends | Proven | Upstream reads only the main selection; `selected_range` plus `add_selection` reproduces its exact line-end set | E1 (pinned upstream command + facade) |
| 13 | Ctrl-click cursors | **Blocked** | Pointer→offset resolver is `pub(crate)`; `on_mouse_down` handles Alt only | E1 |
| 14 | Distinct per-range atomic edit (one undo) | **Blocked** | `replace_text_in_ranges` is `pub(crate)` | E1 |
| 15 | Uniform replacement across all cursors, one undo | Proven | Only via `replace`/typing; `insert` collapses to the active caret | E1 (`state.rs:1286`, `:1305`, `:4545`) |
| 16 | Caret/selection geometry for popovers | Proven | `range_to_bounds` supported; `cursor_layout` is doc-hidden but public | E1, E2 (2026-11-01) |
| 17 | Scroll offset read/write, deferred and clamped | Proven | Apply after `render_frame` sees the new layout | E1, E2 (2026-11-01) |
| 18 | Typewriter centering | Proven | Compose geometry + `set_scroll_offset`; defer the scroll | E1 |
| 19 | Tracked decorations for copyedit/focus/placeholders | Proven | `EditorMode`-specific constructors; decorations are not undo history | E1 |
| 20 | Inline tokens (activation, hover, click) in the writer | **Blocked** | `EditorMode` is explicitly rejected; `Editor` forwards no token hooks | E1, §7 |
| 21 | IME composition guard | Proven (protocol) / historical OS acceptance | Simulated preedit plus 2026-10-29 user acceptance; P0 projection-refresh interaction remains unobserved | E2, historical E4 |
| 22 | Undo / redo | Proven | Active selection observable; secondary-selection restore is internal | E1 |
| 23 | Image paste from the clipboard | Proven | `Editor::on_paste` captures the image before text fallback; native Wayland delivery from `wl-copy image/png` observed | E1, E2, E3 (2026-11-01) |
| 24 | File-list paste (copy files in a file manager, paste into the app) | **Blocked on Linux** | Neither backend reads `text/uri-list`; writes never offer it | E1, §6 B4 |
| 25 | File drop with paths | Proven | `ExternalPaths` reached the editor from a real Nautilus→Wayland drag | E1, E2, E4 (2026-11-01) |
| 26 | Drag files **out** of the app | Proven (API) / **platform unverified** | Wayland drag source offers `text/uri-list`; no parity row requires this direction | E1, E5 |
| 27 | Hit-test a pointer position against a **range** | Proven (workaround) | Invert `range_to_bounds` over known candidate ranges; cost is per-range | E1 |
| 28 | Map a pointer position to a **caret offset** | **Blocked** | No public inverse of the layout map | E1, §6 B3 |
| 29 | Protect hidden regions from edits | NA | Hidden bytes are absent from the visible rope; boundary rejection belongs to `apply_visible_edit` | E1 |

## 5. Correction: multiple selections are creatable in 0.8.1

The first draft of the implementing plan (§3.4) stated that multiple-selection creation was not
exposed. The approved plan now carries this correction: creation is public; observation of the full
selection set is not. That distinction is load-bearing for P4–P10 scope.

**Public and documented:** `InputBaseState::add_selection` (`state.rs:3521`) is a supported method
whose doc comment calls it "the programmatic equivalent of Alt-clicking another cursor into the
text". It clips and character-expands the range, merges overlaps, keeps the active selection, and
scrolls to the new one. It is single-line-guarded (`!is_multi_line()` falls back to
`set_selected_range`). `AddCursorAbove`/`AddCursorBelow` are public actions that operate on **every**
existing selection (`state.rs:2512-2576`). Alt-click adds a cursor and Alt+Shift (or Linux Ctrl+Alt)
starts a block selection (`state.rs:2748-2768`). Typing with several cursors applies one atomic
transaction (`state.rs:4545-4565`), which is how the spike's `public_add_selection_supports_…`
probe asserts `X\nX\n` and a single undo.

**Not public:** reading the set back. The field is `pub(super) selections: Selections`
(`state.rs:458`); `CursorSelection` and `Selections` are `pub(super)` (`cursor.rs:60,134`); no public
method returns a count or a list. `selected_range`, `cursor`, `selected_value` and `selected_text`
all report the **active** selection only. Undo restores secondary cursors through the private
`restore_selections`.

**Consequence.** "Add a cursor" is shipped behavior. "Reason about every cursor" is not. Commands
whose next step depends on the *set* — select-next per cursor, restoring user-placed cursors after a
projection refresh, and per-cursor formatting — are blocked. The upstream line-end command is not in
that class: it deliberately replaces the cursor set from the main selection only, which
`selected_range` plus `add_selection` can reproduce. Commands the app drives from ranges it created
itself can work today; commands that must discover arbitrary user-created cursors cannot.

## 6. Residual blockers

These four are the complete set. Everything else in §4 is reachable through public API, possibly
with a stated degradation.

### B1 — Selection enumeration

No public accessor for the selection set (anchors in §3.2). Blocks:

- select-next-occurrence applied to every cursor;
- saving and restoring secondary cursors when a projection rebuild calls `replace_all`;
- any app-side diagnostic or command that must reflect user-placed cursors;
- revision-ordered rebasing of per-cursor positions after a frontmatter edit.

The app may track the selections it created itself, but Alt-click and `AddCursorAbove/Below` produce
selections it never observes, so a tracked shadow set is not a faithful substitute and must not be
presented as one.

### B2 — Distinct-range atomic edits

`replace_text_in_ranges` is `pub(crate)` (`state.rs:4186`). It is the only API that applies
`(range, distinct_text)` pairs in one history transaction with disjoint-range assertions. Public
alternatives cover less:

- `replace` applies the **same** string to every selection, in one transaction (uniform only);
- `insert` applies to the active caret only and collapses secondary cursors;
- `set_selected_range` + repeated `replace` calls produce N separate edits in history rather than one transaction.

Blocks: per-cursor formatting that needs different replacement text per range; applying a set of
mapped edits from `apply_visible_edit` as one transaction; filling several snippet placeholders in
one step; any "apply this plan atomically" path.

Note that *identical* text at several disjoint ranges can be faked by setting empty selections at
each boundary via `add_selection` and calling `replace` once — that is legitimate and stays public,
but it needs the boundary ranges, hence B1, and it cannot express distinct strings.

### B3 — Ctrl-click position mapping

`on_mouse_down` branches on `event.modifiers.alt` only (`state.rs:2748-2768`); Ctrl+click falls
through to an ordinary caret placement. The resolver that would make a custom mapping possible is
`pub(crate) fn index_for_mouse_position` (`state.rs:3565`) and the insertion helper is
`pub(super) fn add_cursor_at` (`state.rs:2582`). There is no public `offset_for_position`, and
`range_to_bounds` is the wrong direction: it answers "where is this range", not "which byte is under
this point", and a caret cannot be recovered from a bounding box across proportional glyphs.

Blocks: a Ctrl-click cursor command; also any app-side "place a caret at this pointer position".

Not blocked by B3, but adjacent: Alt+hover image preview and Alt+click URL opening need only a
**range** under the pointer. The app can invert `range_to_bounds` over its own candidate ranges
(URLs, image links) and test containment. That is supported today (row 27) and must be implemented
as an app-side inverse hit test, never as a caret query.

### B4 — File-list clipboard on Linux

Wayland's clipboard reader is `read_text().or_else(read_image)` and X11's reads text and image atoms
only (anchors in §3.6). Neither requests `text/uri-list`; the constant exists only for drag sources
and DnD payload reads, and X11's URI list line is commented out. The writer offers text MIME types
plus the app self MIME, never a file list.

Consequences, which P8 must not promise:

- copying files in a file manager and pasting them into Astrolith does nothing (no paths arrive);
- copying a file entry out of Astrolith to a file manager is not available;
- **drag-and-drop of files works in both directions** and is the supported path.

This blocks no ledger row: row 17 (copy path) is text, and rows 42/43 use drops and hover.

### Constraints that are not blockers but bind the implementation

- **C1 — no edit delta.** `InputEvent::Change` is a unit variant; there is no revision counter, no
  last-edit range and no suppression flag. Projection mapping must diff (§8).
- **C2 — refresh collapses cursors.** `replace_all`, `set_value`, `select_all` and
  `set_selected_range` all drop secondary cursors; `replace_all` also resets the selection to
  `0..0`. Any refresh path must re-apply a mapped active range, and cannot restore extras until B1
  closes.
- **C3 — `set_value` clears undo history** and emits no `Change`. It is acceptable for deliberate
  external reload (current use: `crates/astrolith-gpui/src/project_discovery.rs:630-650`), never for
  a live projection refresh.
- **C4 — self-inflicted change.** Because there is no suppression flag, a projection refresh that
  writes through the editor emits `Change` like user typing. The session must set an origin flag
  around its own write and ignore that one event, or the document marks itself dirty.

## 7. Tokens versus decorations: decision

**Decision: the writer uses tracked decorations exclusively. Tokens are not used anywhere in
Astrolith, and the doc-hidden presentation installers are not used at all.**

Reasoning from §3.5: `EditorMode` sets `CODE_EDITOR = true`, `token_api!` is not implemented for it,
`check_token_mode` returns `InlineTokenError::UnsupportedMode`, `install_tokens` drops tokens, and
the component `Editor` forwards no token hook. The only path that paints tokens in this mode is a
`#[doc(hidden)]` installer the crate documents as unsupported — using it would be a private-API
dependency in all but name.

Concrete mapping for the dependent tasks:

| Need | Mechanism |
|---|---|
| Copyedit category colouring (P7) | `create_range_decorations_collection` with `RangeDecoration::new(range).with_style(...)/.with_color(...)`; one collection per category layer, replaced atomically with `set` |
| Active-token suppression | Simply omit the range from the next `set`; no editor feature needed |
| Focus-mode dimming (P5) | One range-decoration collection over every sentence except the cursor sentence |
| Syntax-marker dimming and heading presentation (P5) | Text decorations from the app's own Markdown analysis, or the component highlighter where it already applies |
| Snippet placeholder highlight and navigation (P10) | `RangeDecoration` for the visible field, plus an app-owned ordered placeholder list keyed to `get_ranges(&cx)` read-back |
| Placeholder **click** targeting | Not available. Placeholders are keyboard-navigated (Tab / Shift+Tab / Esc), which is what ledger row 57 requires. If a later slice needs pointer targeting, use the range hit test (row 27), not tokens. |

Two consequences to respect: decorations are not undo history (a deleted placeholder does not come
back on undo — the builder must re-derive its placeholder list from the tracked collection after
every edit), and decoration ranges are UTF-8 bytes like everything else.

## 8. Projection constraint: whole-visible diff

No public API reports *what* changed. The only signal is `InputEvent::Change` with no payload
(`state.rs:132-137`), plus the current `text()`. Therefore the P4 mapping is a **whole-visible-text
diff at explicit boundaries**, not an incremental edit stream. This is a design constraint, not a
temporary hack, and P4 must be written and reviewed as such.

Binding rules:

1. `DocumentSession` keeps a shadow `String` of the last accepted visible text and the projection
   that produced it.
2. A single-region diff may be computed with common-prefix / common-suffix trimming. Its worst case
   is **O(document) on every `Change`**, including ordinary typing near either end; it is not
   O(changed region). A multi-region diff requires an actual diff algorithm with the same or higher
   worst-case cost.
3. The diff is accepted only when it resolves to a single contiguous replacement inside visible
   segments, or a set of non-overlapping replacements that each lie wholly inside one visible
   segment. Any replacement that starts or ends inside a hidden segment, or that straddles a
   visible/hidden boundary, is **rejected**: rebuild the projection from the authoritative complete
   source, keep the source unchanged, and surface a diagnostic. Never write a guessed source.
   A multi-region diff (for example after replace-all, `search.rs:260-281`) is applied right-to-left
   into one new source string; atomicity comes from replacing the whole `DocumentBuffer` source
   once, not from an editor transaction.
4. Boundaries at which a diff must be taken: every `Change` while the session is live; immediately
   before save; before a frontmatter patch is applied; after undo/redo (which also emits `Change`);
   and after any refresh, before re-arming the origin flag (C4).
5. Ambiguity that the diff cannot distinguish (two identical edits in one frame) is resolved by the
   same fail-closed rebuild, never by ordering guesses.
6. Diffing is event-driven, never per-render, but it may still scan the whole visible document per
   keystroke. P4 must measure 1 KiB, 100 KiB, 1 MiB and pathological long-line editing on the pinned
   debug and release profiles. If input responsiveness misses the accepted editor baseline, P4 must
   stop at its gate and seek an edit-delta API or another bounded design; it must not disguise the
   cost with render-path work, stale buffering or delayed source updates.
7. The projection itself is rebuilt with `replace_all` (history-preserving) and the active caret
   restored with `set_selected_range` (C2). Secondary cursors are lost until B1 closes; the gate in
   §9.3 must record that as an observed limitation rather than a silent one.

Hidden-region byte identity is the acceptance invariant: after any body edit, save must reproduce the
hidden frontmatter/import bytes exactly. That is a P4 test requirement, not something this contract
can assert.

## 9. Spike: commands and procedures

The commands below were verified to exist. On 2026-11-01 the integration owner executed the two
`public_*` probes; their exact result is recorded in §9.3. Native procedures and all other listed
probes remain unexecuted unless §9.3 says otherwise.

The spike is disposable: `apps/desktop/src/spikes/editor.rs`, selected by `--spike=editor`
(`apps/desktop/src/main.rs:31`), mounted through `gpui_kit::init` → `gpui_kit::open_window` →
`Editor::new(&editor).size_full()` at 1000×700, tests in `mod tests` using `TestAppContext` and
`TestWindowExt`. Do not promote any of it into `crates/`.

### 9.1 Commands

```sh
mise install                                            # pinned tools

mise run dev:editor                                     # native window; visual pass
mise run test:editor-perf                               # ignored, release; CPU-only workload data

cargo test --locked -p astrolith-desktop public_add_selection_supports_multicursor_replacement_and_undo
cargo test --locked -p astrolith-desktop public_search_geometry_and_scroll_contract_is_available
cargo test --locked -p astrolith-desktop refresh_during_preedit
cargo test --locked -p astrolith-desktop -- --nocapture  # every desktop test

mise run test                                           # whole workspace, headless
mise run check                                          # doctor, fmt, lint, build, test, fixtures
```

The two `public_*` filters target the current P0 probes. Both passed on 2026-11-01. `mise run
dev:editor` opens the window with "Probe search" and "Probe multicursor" buttons
(`editor.rs:19,143-152`) for native pointer verification, which remains outstanding.

### 9.2 Probes to execute

Each probe uses only public API. Where a probe establishes a **Blocked** status, it is a negative
proof: it demonstrates the behavior is absent (compile-time for the API, behavioral for the input
path), not that a workaround exists.

| # | Probe | Procedure (public calls) | Expected assertion |
|---|---|---|---|
| P1 | Projection round-trip | `set_value(visible_without_frontmatter)` → type via `window.input` → read `text()` → diff against the shadow → apply the app's `apply_visible_edit` → assert source equals expected and the hidden frontmatter bytes are untouched → `replace_all(new_visible)` → `set_selected_range(mapped)` → undo/redo | Byte-exact hidden region; active caret restored; one undo step per interaction |
| P2 | Go to line | `set_cursor_position(Position::new(line, 0), …)`, then `render_frame` | `cursor_position().line == line` and `scroll_offset().y` moved |
| P3 | Select next occurrence (active) | `SearchMatcher::new` → `update(text())` → `update_query(selected_text, false)` → `update_cursor_by_offset(sel.start)` → `next()` → `add_selection` → `window.input("X")` | Both occurrences changed; one undo restores both |
| P4 | Line-end cursors (active) | Active multi-line selection → `line_start_offset`/`line_end_offset` per row → `add_selection(end..end)` → type | One character per line end; one undo |
| P5 | Ctrl-click negative | `window.click_with_options(id, ClickOptions::new().with_modifiers(Modifiers { control: true, .. }).with_offset(point(..)), cx)` at a second position, then type; repeat with `alt: true` as the positive control | Ctrl-click leaves one caret; Alt-click adds a cursor. Documents B3 |
| P6 | Distinct batch negative | Source-level: the only `(range, String)` batch API is `pub(crate)`; runtime: `replace("**")` applies one identical string to all selections | No public call accepts distinct strings for distinct ranges. Documents B2 |
| P7 | Token negative | `set_value(InputContent { text, tokens })`, render; source-level: `replace_with_token` is not implemented for `EditorMode` | No token rendering; `UnsupportedMode`. Documents row 20 |
| P8 | Typewriter offset | `render_frame` → `range_to_bounds(&(cursor..cursor))` + `line_height()` + `input_bounds()` → `set_scroll_offset(point(0, target_y), cx)` → `render_frame` | Caret bounds centre ≈ viewport centre |
| P9 | Image paste | `Editor::new(&editor).on_paste(handler)` capturing `ClipboardEntry::Image`; `cx.write_to_clipboard(ClipboardItem::new_image(..))`; `window.press("secondary-v")` | Handler sees an `Image` entry and returns `true` |
| P10 | File drop | Wrap the editor in `div().on_drop(|paths: &ExternalPaths, ..| …)`; dispatch `FileDropEvent::Entered { position, paths }` with `cx.simulate_event(..)` (or `VisualTestContext::simulate_event`) and then a pointer-up over the editor | Paths reach the handler. `Submit` alone carries no paths — the payload lives in the active drag (`window.rs:5698-5745`) |
| P11 | Refresh self-change | Force a projection refresh, observe `InputEvent::Change` from the app's own `replace_all` | Event arrives with no payload; the session's origin flag must suppress the dirty transition (C4) |

Native additions (`mise run dev:editor`), which must be recorded before any E3/E4 claim:

1. visual pass over headings, emphasis, inline code, lists, fences, caret, CJK glyphs;
2. Chinese IME preedit update → candidate → commit, then cancel, with CJK/emoji neighbours;
3. Japanese kana conversion, candidate change, commit and cancel;
4. refresh during active preedit must block and preserve marked text;
5. inter-app multilingual copy/cut/paste in both directions;
6. real drag-and-drop of an image file from a file manager onto the editor;
7. real Alt-click cursor placement and one undo over two cursors.

Items 2–4 require a person with working input methods; no automated substitute counts. Items 5–7
may use compositor delivery, but the result must still be observed in the native window.

### 9.3 Result recording (integration owner)

Only actually exercised probes are marked passed. A source-inspected row remains E1.

| Probe | Run date | Environment | Result | Classes earned |
|---|---|---|---|---|
| Public search/replacement/go-to-line/geometry/scroll probe | 2026-11-01 | GPUI test platform, Rust 1.99.1, pinned facade | PASS; `public_search_geometry_and_scroll_contract_is_available` | E1, E2 |
| Public `add_selection`/uniform replacement/undo probe | 2026-11-01 | GPUI test platform, Rust 1.99.1, pinned facade | PASS; `public_add_selection_supports_multicursor_replacement_and_undo` | E1, E2 |
| P1 projection prerequisite | 2026-11-01 | GPUI test platform | PASS; visible edit mapped into complete source, hidden CRLF bytes exact, one undo | E1, E2 |
| P2 go-to-line | 2026-11-01 | GPUI test platform | PASS within search/geometry probe | E1, E2 |
| P3 select-next algorithm | not run | — | — | E1 |
| P4 line-end cursors | 2026-11-01 | GPUI test platform | PASS; main selection replaced by one cursor per selected line end, one undo | E1, E2 |
| P5 Ctrl-click | source proof only | installed 0.8.1 source | Ctrl-click branch absent; Alt-click remains built in | E1 |
| P6 distinct batch | source proof only | installed 0.8.1 source | no public call accepts distinct strings | E1 |
| P7 tokens | source proof only | installed 0.8.1 source | `EditorMode` token API absent | E1 |
| P8 typewriter offset | 2026-11-01 | GPUI test platform | PASS for geometry + deferred/clamped negative scroll; centering formula remains P5 | E1, E2 |
| P9 image paste capture | 2026-11-01 | GPUI test platform + native Wayland window | PASS; image reaches `Editor::on_paste`; `wl-copy image/png` + compositor Ctrl+V changed the footer to `Received clipboard image` | E1, E2, E3 |
| P10 external file drop | 2026-11-01 | GPUI test platform + Nautilus/Wayland | PASS; simulated `Entered` + `Submit` delivers `ExternalPaths`; user-confirmed physical Nautilus drag changed footer to `Received 1 dropped path(s)` | E1, E2, E4 |
| P11 self-change | source proof only | installed 0.8.1 source | `replace_all` emits payload-free `Change`; session origin guard remains P4 | E1 |
| Native launch/static visual surface | 2026-11-01 | Wayland, Hyprland 0.56.2, scale 1 | PASS; mapped 1263×1384, `xwayland=false`; sample, Unicode, Markdown styling and P0 controls visible | E1, E3 |
| Native Alt-click/multicursor/undo | 2026-11-01 | Wayland, Hyprland 0.56.2, scale 1 | PASS; compositor Alt-click produced two insertion points; uniform typing changed both; one Ctrl+Z restored both | E1, E3 |
| Native IME/scaling/latency | 2026-10-29 | recorded prior milestone session | PASS; manual user acceptance retained | historical E4 |

The pre-existing M00.2 tests (preedit refresh guard, selection/undo, clipboard round-trip) are
recorded as passing in `editor-spike.md` at that milestone. This contract neither re-ran nor
invalidated them; they are historical E2 and are listed here so a reader can tell them apart from
the unverified probes above.

## 10. Minimal upstream facade API proposal

Three additions to `gpui-base`, `impl<M: InputModeKind> InputBaseState<M>`, shipped in one release
and then pinned. No private state needs to be exposed; each is a thin wrapper over an existing
internal path.

```rust
/// Read-only view of the cursor set. Active selection first; UTF-8 byte ranges.
pub fn selection_ranges(&self) -> Vec<Range<usize>>;

/// Apply a batch of edits as one history transaction.
/// Same contract as the crate-internal batch edit: disjoint ranges, one undo step,
/// per-edit resulting cursors preserved.
pub fn apply_edits(
    &mut self,
    edits: &[(Range<usize>, String)],
    window: &mut Window,
    cx: &mut Context<Self>,
);

/// Map a pointer position to a caret offset (UTF-8 bytes).
pub fn offset_for_position(&self, position: Point<Pixels>) -> Option<usize>;
```

Mapping to the blockers:

| Addition | Closes | Implementation note |
|---|---|---|
| `selection_ranges` | B1 | Iterate the existing `Selections`, active first. Read-only; no mutation. |
| `apply_edits` | B2 | Public wrapper over `replace_text_in_ranges`, which already debug-asserts disjoint ranges and commits one transaction. |
| `offset_for_position` | B3 | Wrapper over `index_for_mouse_position`, which already returns `(offset, line_end_affinity)`; discard the affinity or return it alongside. |

Optional, only if a later slice wants real inline tokens in the writer (for example to make snippet
placeholders clickable): implement `token_api!(EditorMode)` and remove the `CODE_EDITOR` rejection
in `check_token_mode`/`install_tokens`, **or** forward `token`/`on_token_click`/`on_token_hover`
from the component `Editor`. Not required for parity: §7 places placeholders on decorations.

Explicitly **not** needed, because they are already public: search/replace/navigation, go-to-line,
uniform multi-cursor replacement, cursor creation, geometry and scroll, decorations, IME, undo,
image paste and file drops.

Upstream request framing: two of the three are pure visibility changes to existing correct
implementations; the third is a read-only accessor. That is the smallest possible diff and is the
argument to make upstream. Do not vendor, patch, or reach through `pub(super)` fields while waiting.

## 11. Consequences for the implementation plan

### 11.1 What may proceed now (source-inspected, public API only)

| Task | May proceed | Condition |
|---|---|---|
| **P4** lossless projection | **Yes** | Whole-visible diff (§8), fail-closed ambiguity, `replace_all` + mapped active caret. The P4 real-window gate must record that secondary cursors are not preserved, and hidden-byte identity must be asserted. P4's own gateway still applies: if the real-window gate fails, P5–P10 stop and return for approval. |
| **P5** writer commands and chrome | **Yes, except row 36's blocked operations** | Single-range formatting, line-end cursors from the main selection, find/replace panel wiring, go-to-line, focus-by-sentence, typewriter centering, contextual toolbar and counts are Proven. Select-next across arbitrary cursors, Ctrl-click placement and distinct batch edits remain blocked. |
| **P7** copyedit | **Yes** | Decorations only (§7); no tokens. Classification, revision tickets and suppression are app-side. |
| **P8** assets, links, image preview | **Yes** | Drops + `on_paste` for images; range hit test for Alt-hover and Alt-click URLs (row 27). **Do not** implement or promise file-list clipboard paste (B4). Platform behavior stays E5 until native evidence. `avif`, `tif`/`tiff`, `ico` and `svg` insert image Markdown but fail preview silently, because the pinned decoder enables only `bmp`, `gif`, `jpeg`, `png` and `webp`. |
| **P10** MDX builder and snippets | **Yes** | Placeholders are decoration-tracked and keyboard-navigated; no token API, no pointer-targeted placeholders. Import planning edits complete source, not the projection. |

Nothing in P4/P5/P7/P8/P10 may add a direct `gpui-base`/`gpui-pre` dependency, use the doc-hidden
presentation installers, or introspect private fields.

### 11.2 Final-parity status

Ledger row numbers refer to [`astro-editor-parity.md`](astro-editor-parity.md).

**Exactly one ledger row remains partially blocked: #36.** Its line-end command is reachable; three
operations still require missing facade APIs and cannot be represented as parity-complete degraded
versions:

| Ledger row | Status / behavior | Dependency |
|---|---|---|
| #36 (a) | **Blocked:** select next occurrence across all cursors | B1 `selection_ranges` — the app cannot see the cursor set it must extend |
| #36 (b) | **Proven:** replace the selection set with cursors at each line end of the main selection | Public `selected_range` + `add_selection`; P5 implementation and smoke remain |
| #36 (c) | **Blocked:** modified (Ctrl) click cursors | B3 `offset_for_position` |
| #36 (d) | **Blocked:** distinct batch replacement across all cursors in one transaction | B2 `apply_edits` for distinct text per range, B1 for arbitrary user-created ranges |

Typing the *same* text into several cursors already works and is one undo step (row 15 of §4); that
is not what (d) needs. (d) is the command path that must compute a replacement set from the cursor
set the user created — which is precisely what B1 hides and B2 forbids applying atomically.

Rows that a reader might expect to be blocked and are **not**:

| Ledger row | Reachable via | Remaining obligation |
|---|---|---|
| #28 projection | `replace_all` + mapped active caret, whole-visible diff (§8) | P4 real-window gate; hidden-byte identity |
| #32 formatting | single-range public edits | one undo step per transform |
| #37, #57, #58 snippet fields, placeholder navigation, wrapping scaffolds | tracked decorations, keyboard-navigated | none beyond P10 smoke |
| #81 copyedit decorations | `create_range_decorations_collection` | none beyond P7 smoke |
| #41 modified-click URL open, #43 modified-hover image preview | app-side inverse `range_to_bounds` hit test over known candidate ranges | record the mechanism as a range hit test, never a caret query; verify natively |
| #42 drop → assets | `ExternalPaths` + `on_drop` | native verification (E5 today) |

Consequently, **the release gate in plan §4 ("no known silent data loss; every scoped ledger row
Pass") cannot pass while row #36 is blocked.** Every other writer row is implementable against the
pinned facade, subject to the real-window gates in P4 and the evidence still outstanding in §9.3.
Row #36 does not need to block P5's other work: formatting, search, navigation, focus and typewriter
modes ship independently and should be committed separately so the blocked row is a single, visible
exception rather than a stalled milestone.

## 12. Re-verification on any version bump

The facade is the contract, and it moves. On any change to `gpui-kit`/`gpui-base`/`gpui-component`/
`gpui-pre`:

1. Diff the symbol table in §3 against the new sources; a removed or re-signatured symbol invalidates
   the rows that cite it.
2. Re-check the four blockers first: `selections` visibility, `replace_text_in_ranges` visibility,
   `index_for_mouse_position` visibility and the `on_mouse_down` modifier branches, and the Linux
   clipboard read/write MIME lists. Those four determine whether the plan's gate reopens.
3. Re-run `mise run check`, then the §9.2 probes in a real window, and refill §9.3. A version bump
   without a fresh §9.3 row is not evidence.
4. Keep this document's statuses in step with `astro-editor-parity.md` §3: the ledger owns scenarios,
   this file owns capability. If they disagree, this file is wrong until it is re-read against the
   new source.
