//! Visible complete-source editing surface.
//!
//! The retained [`EditorState`] stays the input model: it owns the cursor,
//! selection, IME composition, highlighter and undo integration. This view
//! renders the same source text through the application-owned source layout,
//! paints line numbers, selection and caret itself, and maps pointer positions
//! back into editor byte offsets.
//!
//! Vertical virtualization selects lines; horizontal virtualization shapes only
//! the window the viewport needs. A 1 MiB single source line therefore does not
//! shape or paint the whole line. Geometry for hit-testing, the caret and IME
//! comes from the same shaped window that paints; unresolved windows fail
//! closed.

use super::input_boundaries::{grapheme_boundary, word_boundary};
use super::perf;
use super::session::DocumentSession;
use super::text_layout::source::{
    PlainSourceRuns, SourceLayout, SourceRuns, SourceStyle, SourceViewport,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_kit::component::input::{self, EditorState, InputEvent, Rope, RopeExt};
use gpui_kit::{
    AnyElement, App, Bounds, ClipboardItem, Context, Div, Element, ElementId, Entity,
    EntityInputHandler, Focusable, GlobalElementId, HighlightStyle, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, ScrollHandle, SharedString,
    Stateful, TestSupportExt, TextAlign, TextRun, UTF16Selection, WeakEntity, Window, div, fill,
    point, prelude::*, px, size,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// Select the source surface for Build in development builds.
///
/// The stock Kit editor remains the default until the source surface reaches
/// the functional gate in the M6 decision. This switch is internal to the
/// native harness and A/B runs, not a user setting.
pub(crate) fn enabled() -> bool {
    static ENABLED: LazyLock<bool> = LazyLock::new(|| {
        cfg!(debug_assertions)
            && std::env::var_os("ASTROLITH_SOURCE_SURFACE").is_some_and(|value| value == "1")
    });
    *ENABLED
}

const HIGHLIGHT_SYNC_TIMEOUT: Duration = Duration::from_millis(2);
const HIGHLIGHT_BACKGROUND_DEBOUNCE: Duration = Duration::from_millis(120);
const GUTTER_PADDING: f32 = 10.;
const GUTTER_GAP: f32 = 10.;
/// Rows the completion menu paints at once; the window follows the selection.
const COMPLETION_ROWS: usize = 12;
const COMPLETION_ROW_HEIGHT: f32 = 22.;
const COMPLETION_PADDING: f32 = 4.;
const COMPLETION_MAX_WIDTH: f32 = 420.;
const COMPLETION_MIN_WIDTH: f32 = 160.;
const POPUP_GAP: f32 = 4.;

#[derive(Clone, Copy, Default)]
struct FrameGeometry {
    origin: Point<Pixels>,
    viewport: Bounds<Pixels>,
    width: f32,
}

/// Window-space placement for a caret-anchored overlay.
#[derive(Clone, Copy)]
struct PopupPlacement {
    left: f32,
    top: f32,
    width: f32,
    row_height: f32,
    font_size: f32,
}

struct RenderState {
    layout: Option<SourceLayout>,
    geometry: FrameGeometry,
    gutter: f32,
    selection: Range<usize>,
    caret: Option<usize>,
    active_line: Option<usize>,
    caret_visible: bool,
    render_elapsed: Duration,
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            layout: None,
            geometry: FrameGeometry::default(),
            gutter: 0.,
            selection: 0..0,
            caret: None,
            active_line: None,
            caret_visible: true,
            render_elapsed: Duration::ZERO,
        }
    }
}

/// The complete-source editing surface for one retained document session.
pub(crate) struct SourceSurface {
    session: WeakEntity<DocumentSession>,
    editor: Entity<EditorState>,
    language: SharedString,
    state: Rc<RefCell<RenderState>>,
    scroll: ScrollHandle,
    text: Rope,
    styled: Option<SourceStyle>,
    highlighter: Option<SyntaxHighlighter>,
    highlighted: Rope,
    highlight_generation: u64,
    highlight_task: Option<gpui_kit::Task<()>>,
    highlight_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    last_cursor: Option<usize>,
    follow_caret: bool,
    blink_task: Option<gpui_kit::Task<()>>,
    blink_visible: bool,
    focused: bool,
    /// Menu revision the selected completion row belongs to.
    completion_revision: u64,
    completion_selected: usize,
    _observe: gpui_kit::Subscription,
    _change: gpui_kit::Subscription,
}

impl Drop for SourceSurface {
    fn drop(&mut self) {
        if let Some(cancel) = self.highlight_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl SourceSurface {
    pub(crate) fn enabled() -> bool {
        enabled()
    }

    pub(crate) fn new(
        session: WeakEntity<DocumentSession>,
        editor: Entity<EditorState>,
        language: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&editor, |_, _, cx| cx.notify());
        let change = cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = this.editor.read(cx).text().clone();
                this.sync_text(&text);
                this.schedule_highlight(text, cx);
            }
        });
        Self {
            session,
            editor,
            language,
            state: Rc::new(RefCell::new(RenderState::default())),
            scroll: ScrollHandle::new(),
            text: Rope::new(),
            styled: None,
            highlighter: None,
            highlighted: Rope::new(),
            highlight_generation: 0,
            highlight_task: None,
            highlight_cancel: None,
            last_cursor: None,
            follow_caret: false,
            blink_task: None,
            blink_visible: true,
            focused: false,
            completion_revision: 0,
            completion_selected: 0,
            _observe: observe,
            _change: change,
        }
    }

    fn sync_text(&mut self, text: &Rope) {
        if &self.text == text {
            return;
        }
        self.text = text.clone();
        let mut state = self.state.borrow_mut();
        if let Some(layout) = state.layout.as_mut() {
            layout.set_text(SharedString::from(text.to_string()));
        }
    }

    fn schedule_highlight(&mut self, text: Rope, cx: &mut Context<Self>) {
        if self.highlighted == text && self.highlighter.is_some() {
            return;
        }
        self.highlighted = text.clone();
        self.highlight_generation = self.highlight_generation.wrapping_add(1);
        let language = self.language.clone();
        if self.highlighter.is_none() {
            self.highlighter = Some(SyntaxHighlighter::new(&language));
        }
        let complete = self.highlighter.as_mut().is_some_and(|highlighter| {
            highlighter.update(None, &text, Some(HIGHLIGHT_SYNC_TIMEOUT))
        });
        if complete {
            cx.notify();
            return;
        }
        let generation = self.highlight_generation;
        if self.highlight_task.is_some() {
            return;
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.highlight_cancel = Some(cancel.clone());
        let executor = cx.background_executor().clone();
        let work = cx.background_spawn(async move {
            executor.timer(HIGHLIGHT_BACKGROUND_DEBOUNCE).await;
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            let mut highlighter = SyntaxHighlighter::new(&language);
            highlighter.update(None, &text, None);
            Some(highlighter)
        });
        self.highlight_task = Some(cx.spawn(async move |this, cx| {
            let parsed = work.await;
            let _ = this.update(cx, |this, cx| {
                this.highlight_task = None;
                this.highlight_cancel = None;
                if this.highlight_generation == generation
                    && let Some(highlighter) = parsed
                {
                    this.highlighter = Some(highlighter);
                    cx.notify();
                }
            });
        }));
    }

    fn update_style(&mut self, cx: &App) -> SourceStyle {
        let style = SourceStyle::from_theme(cx);
        if self.styled.as_ref() != Some(&style) {
            self.styled = Some(style.clone());
            if let Some(layout) = self.state.borrow_mut().layout.as_mut() {
                layout.set_style(std::sync::Arc::new(style.clone()));
            }
        }
        style
    }

    fn viewport_metrics(&self, window: &Window) -> (f32, f32, f32, f32) {
        let geometry = self.state.borrow().geometry;
        let fallback = window.viewport_size();
        let width = if geometry.width > 0. {
            geometry.width
        } else {
            (f32::from(fallback.width) * 0.75).max(240.)
        };
        let height = if geometry.viewport.size.height > px(0.) {
            f32::from(geometry.viewport.size.height)
        } else {
            (f32::from(fallback.height) * 0.75).max(160.)
        };
        let offset = self.scroll.offset();
        let scroll_x = (-f32::from(offset.x)).max(0.);
        let scroll_y = (-f32::from(offset.y)).max(0.);
        (width, height, scroll_x, scroll_y)
    }

    fn sync_blink(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        self.blink_visible = true;
        self.blink_task = None;
        if focused {
            self.blink_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            this.blink_visible = !this.blink_visible;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    /// Move the viewport to an absolute translation. The native harness uses
    /// this instead of synthesising wheel input, which Wayland cannot inject.
    pub(crate) fn set_scroll_offset(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let max = self.scroll.max_offset();
        let target = point(
            px((-x).clamp(-f32::from(max.x), 0.)),
            px((-y).clamp(-f32::from(max.y), 0.)),
        );
        if target != self.scroll.offset() {
            self.scroll.set_offset(target);
        }
        self.follow_caret = false;
        cx.notify();
    }

    fn select(
        &mut self,
        position: Point<Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (geometry, selection, reversed) = {
            let state = self.state.borrow();
            (
                state.geometry,
                self.editor.read(cx).selected_range(),
                self.editor.read(cx).is_selection_reversed(),
            )
        };
        let offset = self.state.borrow().layout.as_ref().and_then(|layout| {
            let gutter = self.state.borrow().gutter;
            let x = position.x - geometry.origin.x - px(gutter);
            let y = position.y - geometry.origin.y;
            layout.hit_test(point(x, y))
        });
        let Some(offset) = offset else {
            return;
        };
        let anchor = if extend {
            if reversed {
                selection.end
            } else {
                selection.start
            }
        } else {
            offset
        };
        self.editor.update(cx, |editor, cx| {
            let range = anchor.min(offset)..anchor.max(offset);
            editor.set_selected_range_with_direction(range, offset < anchor, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    fn navigate(
        &mut self,
        direction: &str,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (cursor, selection, reversed) = {
            let editor = self.editor.read(cx);
            (
                editor.cursor(),
                editor.selected_range(),
                editor.is_selection_reversed(),
            )
        };
        let anchor = if reversed {
            selection.end
        } else {
            selection.start
        };
        let line = self
            .state
            .borrow()
            .layout
            .as_ref()
            .map(|layout| layout.line_of(cursor));
        let target = match direction {
            "up" | "down" => {
                let Some(line) = line else {
                    return false;
                };
                let layout = self.state.borrow();
                let Some(layout) = layout.layout.as_ref() else {
                    return false;
                };
                let row = layout.row_height();
                if let Some((caret, _)) = layout.caret(cursor) {
                    let y = caret.y + if direction == "up" { -px(row) } else { px(row) };
                    if let Some(hit) = layout.hit_test(point(caret.x, y)) {
                        hit
                    } else {
                        let next = if direction == "up" {
                            line.checked_sub(1)
                        } else if line + 1 < layout.line_count() {
                            Some(line + 1)
                        } else {
                            None
                        };
                        let Some(next) = next else {
                            return false;
                        };
                        let start = layout.line_start(next).unwrap_or(cursor);
                        let end = layout.line_end(next).unwrap_or(start);
                        let column =
                            cursor.saturating_sub(layout.line_start(line).unwrap_or(cursor));
                        start + column.min(end.saturating_sub(start))
                    }
                } else {
                    let next = if direction == "up" {
                        line.checked_sub(1)
                    } else if line + 1 < layout.line_count() {
                        Some(line + 1)
                    } else {
                        None
                    };
                    let Some(next) = next else {
                        return false;
                    };
                    let start = layout.line_start(next).unwrap_or(cursor);
                    let end = layout.line_end(next).unwrap_or(start);
                    let column = cursor.saturating_sub(layout.line_start(line).unwrap_or(cursor));
                    start + column.min(end.saturating_sub(start))
                }
            }
            "home" => line
                .and_then(|line| self.state.borrow().layout.as_ref()?.line_start(line))
                .unwrap_or(0),
            "end" => line
                .and_then(|line| self.state.borrow().layout.as_ref()?.line_end(line))
                .map(|end| {
                    end.saturating_sub(usize::from(
                        end > 0 && &self.text.slice(end - 1..end) == "\n",
                    ))
                })
                .unwrap_or(0),
            _ => return false,
        };
        let (range, reversed) = if extend {
            (anchor.min(target)..anchor.max(target), target < anchor)
        } else {
            (target..target, false)
        };
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range_with_direction(range, reversed, cx);
            if !extend {
                editor.focus_handle(cx).focus(window, cx);
            }
        });
        true
    }

    fn move_or_delete(
        &mut self,
        backward: bool,
        unit: Boundary,
        extend: bool,
        delete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (selection, cursor, editable, text) = {
            let editor = self.editor.read(cx);
            (
                editor.selected_range(),
                editor.cursor(),
                editor.is_editable(),
                editor.text().clone(),
            )
        };
        if delete && !editable {
            return;
        }
        let target = match unit {
            Boundary::Grapheme if !extend && !delete && !selection.is_empty() => {
                if backward {
                    selection.start
                } else {
                    selection.end
                }
            }
            Boundary::Grapheme => grapheme_boundary(&text, cursor, backward),
            Boundary::Word => word_boundary(&text, cursor, backward),
            Boundary::Line => {
                let row = text.offset_to_point(cursor).row;
                if backward {
                    text.line_start_offset(row)
                } else {
                    text.line_start_offset(row) + text.slice_line(row).len()
                        - usize::from(text.slice_line(row).chars().reversed().next() == Some('\r'))
                }
            }
            Boundary::Document => {
                if backward {
                    0
                } else {
                    text.len()
                }
            }
        };
        if delete {
            let range = if !selection.is_empty() {
                selection.clone()
            } else {
                cursor.min(target)..cursor.max(target)
            };
            self.editor.update(cx, |editor, cx| {
                editor.set_selected_range(range, cx);
                editor.replace("", window, cx);
            });
            return;
        }
        let (range, reversed) = if extend {
            let anchor = if self.editor.read(cx).is_selection_reversed() {
                selection.end
            } else {
                selection.start
            };
            (anchor.min(target)..anchor.max(target), target < anchor)
        } else {
            (target..target, false)
        };
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range_with_direction(range, reversed, cx);
            if !extend {
                editor.focus_handle(cx).focus(window, cx);
            }
        });
    }

    fn insert(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editable = self.editor.read(cx).is_editable();
        if editable {
            self.editor
                .update(cx, |editor, cx| editor.replace(text.to_owned(), window, cx));
        }
    }

    fn bind_commands(&self, root: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        macro_rules! movement {
            ($root:expr, $action:ty, $back:expr, $unit:ident, $extend:expr, $delete:expr) => {
                $root.on_action(cx.listener(|this, _: &$action, window, cx| {
                    this.move_or_delete($back, Boundary::$unit, $extend, $delete, window, cx);
                }))
            };
        }
        let root = root.key_context("Input");
        let root = movement!(root, input::Backspace, true, Grapheme, false, true);
        let root = movement!(root, input::Delete, false, Grapheme, false, true);
        let root = movement!(
            root,
            input::DeleteToPreviousWordStart,
            true,
            Word,
            false,
            true
        );
        let root = movement!(root, input::DeleteToNextWordEnd, false, Word, false, true);
        let root = movement!(
            root,
            input::DeleteToBeginningOfLine,
            true,
            Line,
            false,
            true
        );
        let root = movement!(root, input::DeleteToEndOfLine, false, Line, false, true);
        let root = movement!(root, input::MoveLeft, true, Grapheme, false, false);
        let root = movement!(root, input::MoveRight, false, Grapheme, false, false);
        let root = movement!(
            root,
            gpui_kit::base::actions::SelectLeft,
            true,
            Grapheme,
            true,
            false
        );
        let root = movement!(
            root,
            gpui_kit::base::actions::SelectRight,
            false,
            Grapheme,
            true,
            false
        );
        let root = movement!(root, input::MoveToPreviousWord, true, Word, false, false);
        let root = movement!(root, input::MoveToNextWord, false, Word, false, false);
        let root = movement!(
            root,
            input::SelectToPreviousWordStart,
            true,
            Word,
            true,
            false
        );
        let root = movement!(root, input::SelectToNextWordEnd, false, Word, true, false);
        let root = movement!(root, input::MoveToStartOfLine, true, Line, false, false);
        let root = movement!(root, input::MoveToEndOfLine, false, Line, false, false);
        let root = movement!(root, input::MoveToStart, true, Document, false, false);
        let root = movement!(root, input::MoveToEnd, false, Document, false, false);
        let root = movement!(root, input::SelectToStart, true, Document, true, false);
        let root = movement!(root, input::SelectToEnd, false, Document, true, false);
        macro_rules! navigation {
            ($root:expr, $action:ty, $key:expr, $extend:expr, $paged:expr, $menu:expr) => {
                $root.on_action(cx.listener(|this, _: &$action, window, cx| {
                    if !$extend
                        && !$paged
                        && $menu.is_some_and(|delta| this.move_completion(delta, cx))
                    {
                        return;
                    }
                    let steps = if $paged { page_rows(this, window) } else { 1 };
                    for _ in 0..steps {
                        if !this.navigate($key, $extend, window, cx) {
                            break;
                        }
                    }
                }))
            };
        }
        let root = navigation!(root, input::MoveUp, "up", false, false, Some(-1_isize));
        let root = navigation!(root, input::MoveDown, "down", false, false, Some(1_isize));
        let root = navigation!(root, input::MoveHome, "home", false, false, None::<isize>);
        let root = navigation!(root, input::MoveEnd, "end", false, false, None::<isize>);
        let root = navigation!(
            root,
            gpui_kit::base::actions::SelectUp,
            "up",
            true,
            false,
            None::<isize>
        );
        let root = navigation!(
            root,
            gpui_kit::base::actions::SelectDown,
            "down",
            true,
            false,
            None::<isize>
        );
        let root = navigation!(root, input::MovePageUp, "up", false, true, None::<isize>);
        let root = navigation!(
            root,
            input::MovePageDown,
            "down",
            false,
            true,
            None::<isize>
        );
        root.on_action(cx.listener(|this, _: &input::ShowCompletions, window, cx| {
            this.editor
                .update(cx, |editor, cx| editor.show_completions(window, cx));
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &input::Enter, window, cx| {
            if !this.accept_completion(window, cx) {
                this.insert("\n", window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &input::SelectAll, window, cx| {
            this.editor
                .update(cx, |editor, cx| editor.select_all(window, cx));
        }))
        .on_action(cx.listener(|this, _: &input::Copy, _, cx| this.copy(cx)))
        .on_action(cx.listener(|this, _: &input::Cut, window, cx| {
            this.copy(cx);
            if !this.editor.read(cx).selected_range().is_empty() {
                this.insert("", window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &input::Paste, window, cx| {
            if let Some(item) = cx.read_from_clipboard() {
                this.editor
                    .update(cx, |editor, cx| editor.paste(item, window, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &input::Undo, window, cx| {
            if let Some(session) = this.session.upgrade() {
                session.update(cx, |session, cx| session.undo_source(window, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &input::Redo, window, cx| {
            if let Some(session) = this.session.upgrade() {
                session.update(cx, |session, cx| session.redo_source(window, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
            if this.dismiss_completion(cx) {
                return;
            }
            this.editor
                .update(cx, |editor, cx| editor.unmark_text(window, cx));
        }))
    }

    fn copy(&self, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        if !editor.selected_range().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                editor.selected_text().to_string(),
            ));
        }
    }

    fn register_input(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.editor.read(cx).focus_handle(cx);
        window.handle_input(
            &focus,
            gpui_kit::ElementInputHandler::new(bounds, cx.entity()),
            cx,
        );
    }

    fn source_at(&self, position: Point<Pixels>) -> Option<usize> {
        let state = self.state.borrow();
        let geometry = state.geometry;
        let layout = state.layout.as_ref()?;
        layout.hit_test(point(
            position.x - geometry.origin.x - px(state.gutter),
            position.y - geometry.origin.y,
        ))
    }

    /// Paint the completion menu the retained editor model is presenting.
    ///
    /// The Kit editor element renders its own menu, so a host that owns the
    /// visible text must mirror the same public overlay state: the items and
    /// revisions come from `EditorState`, and the row geometry comes from this
    /// surface's own shaped window.
    fn completion_popup(
        &mut self,
        placement: PopupPlacement,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (open, revision, len) = {
            let editor = self.editor.read(cx);
            let menu = editor.completion_menu_state();
            (menu.open, menu.revision(), menu.items.len())
        };
        if !open || len == 0 {
            self.completion_selected = 0;
            return None;
        }
        if revision != self.completion_revision {
            self.completion_revision = revision;
            self.completion_selected = 0;
        }
        self.completion_selected = self.completion_selected.min(len - 1);
        let selected = self.completion_selected;
        let start = (selected + 1).saturating_sub(COMPLETION_ROWS);
        let end = (start + COMPLETION_ROWS).min(len);
        let items = {
            let editor = self.editor.read(cx);
            editor.completion_menu_state().items[start..end].to_vec()
        };
        let theme = cx.theme();
        let (popover, foreground, muted, active, border) = (
            theme.popover,
            theme.popover_foreground,
            theme.muted_foreground,
            theme.list_active,
            theme.border,
        );
        let mut rows = Vec::with_capacity(items.len());
        for (offset, item) in items.iter().enumerate() {
            let index = start + offset;
            let mut row = div()
                .id(("completion-row", index as u64))
                .h(px(placement.row_height))
                .w_full()
                .px(px(6.))
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(4.))
                .when(index == selected, |row| row.bg(active))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(placement.font_size))
                        .text_color(foreground)
                        .child(SharedString::from(item.label.clone())),
                );
            if let Some(detail) = item.detail.clone() {
                row = row.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_size(px((placement.font_size - 2.).max(8.)))
                        .text_color(muted)
                        .child(SharedString::from(detail)),
                );
            }
            let row = row.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.commit_completion(index, window, cx);
                    cx.stop_propagation();
                }),
            );
            rows.push(row.into_any_element());
        }
        Some(
            div()
                .id("source-completion-menu")
                .absolute()
                .left(px(placement.left))
                .top(px(placement.top))
                .w(px(placement.width))
                .py(px(COMPLETION_PADDING))
                .rounded(px(6.))
                .border_1()
                .border_color(border)
                .bg(popover)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(rows)
                .into_any_element(),
        )
    }

    /// Move the menu selection; `false` leaves the action to the caret.
    fn move_completion(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let len = {
            let editor = self.editor.read(cx);
            let menu = editor.completion_menu_state();
            if !menu.open {
                return false;
            }
            menu.items.len()
        };
        if len == 0 {
            return false;
        }
        let next = (self.completion_selected as isize + delta).clamp(0, len as isize - 1);
        self.completion_selected = next as usize;
        cx.notify();
        true
    }

    /// Insert the selected completion, replacing the text that opened the menu.
    fn accept_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let open = {
            let editor = self.editor.read(cx);
            let menu = editor.completion_menu_state();
            menu.open && !menu.items.is_empty()
        };
        if !open {
            return false;
        }
        self.commit_completion(self.completion_selected, window, cx);
        true
    }

    fn commit_completion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((item, range)) = ({
            let editor = self.editor.read(cx);
            let menu = editor.completion_menu_state();
            let end = editor.cursor();
            menu.items
                .get(index)
                .cloned()
                .map(|item| (item, menu.trigger_start_offset.unwrap_or(end)..end))
        }) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.insert_completion(&item, range, window, cx);
            editor.dismiss_completion_overlay(cx);
        });
        self.completion_selected = 0;
        cx.notify();
    }

    /// Close the completion menu; `false` when none was open.
    fn dismiss_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.editor.read(cx).completion_menu_state().open;
        if !open {
            return false;
        }
        self.editor
            .update(cx, |editor, cx| editor.dismiss_completion_overlay(cx));
        self.completion_selected = 0;
        true
    }
}

fn page_rows(surface: &SourceSurface, window: &Window) -> usize {
    let (_, height, _, _) = surface.viewport_metrics(window);
    let row = surface
        .state
        .borrow()
        .layout
        .as_ref()
        .map_or(20., |layout| layout.row_height());
    (height / row).max(1.).floor() as usize
}

#[derive(Clone, Copy)]
enum Boundary {
    Grapheme,
    Word,
    Line,
    Document,
}

/// Syntax runs for the shaped source window.
struct ThemedSourceRuns<'a> {
    highlighter: Option<&'a SyntaxHighlighter>,
    theme: &'a HighlightTheme,
    style: Arc<SourceStyle>,
}

impl SourceRuns for ThemedSourceRuns<'_> {
    fn runs(&self, range: Range<usize>) -> Vec<TextRun> {
        let Some(highlighter) = self.highlighter else {
            return PlainSourceRuns {
                style: self.style.clone(),
            }
            .runs(range);
        };
        let styles = highlighter.styles(&range, self.theme);
        let mut runs = Vec::with_capacity(styles.len().max(1));
        let mut cursor = range.start;
        for (span, style) in styles {
            let start = span.start.max(range.start).min(range.end);
            let end = span.end.max(range.start).min(range.end);
            if start > cursor {
                runs.push(plain_run(&self.style, start - cursor));
                cursor = start;
            }
            if end <= cursor {
                continue;
            }
            runs.push(styled_run(&self.style, end - cursor, &style));
            cursor = end;
        }
        if cursor < range.end {
            runs.push(plain_run(&self.style, range.end - cursor));
        }
        if runs.is_empty() {
            runs.push(plain_run(&self.style, range.len()));
        }
        runs
    }
}

fn plain_run(style: &SourceStyle, len: usize) -> TextRun {
    TextRun {
        len,
        font: style.font.clone(),
        color: style.foreground,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

fn styled_run(style: &SourceStyle, len: usize, highlight: &HighlightStyle) -> TextRun {
    let mut font = style.font.clone();
    if let Some(weight) = highlight.font_weight {
        font.weight = weight;
    }
    if let Some(font_style) = highlight.font_style {
        font.style = font_style;
    }
    TextRun {
        len,
        font,
        color: highlight.color.unwrap_or(style.foreground),
        background_color: highlight.background_color,
        underline: highlight.underline,
        strikethrough: highlight.strikethrough,
    }
}

impl Render for SourceSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let render_started = Instant::now();
        let Some(session) = self.session.upgrade() else {
            return div().size_full().into_any_element();
        };
        let _ = session;
        let (text, selection, cursor, focused) = {
            let editor = self.editor.read(cx);
            (
                editor.text().clone(),
                editor.selected_range(),
                editor.cursor(),
                editor.focus_handle(cx).is_focused(window),
            )
        };
        let style = self.update_style(cx);
        self.sync_text(&text);
        if self.highlighter.is_none() {
            self.highlighter = Some(SyntaxHighlighter::new(&self.language));
        }
        if self.highlighted != text {
            self.schedule_highlight(text.clone(), cx);
        }
        self.sync_blink(focused, cx);

        let (width, height, scroll_x, scroll_y) = self.viewport_metrics(window);
        let gutter = gutter_width(self.state.borrow().layout.as_ref(), &style);
        let content_width = (width - gutter).max(80.);
        let highlight_theme = cx.theme().highlight_theme.clone();
        let prepare_started = Instant::now();
        {
            let mut state = self.state.borrow_mut();
            if state.layout.is_none() {
                state.layout = Some(SourceLayout::new(
                    SharedString::from(text.to_string()),
                    Arc::new(style.clone()),
                    content_width,
                ));
            }
            state.selection = selection.clone();
            state.caret = Some(cursor);
            state.gutter = gutter;
            state.caret_visible = self.blink_visible && focused && selection.is_empty();
            let layout = state
                .layout
                .as_mut()
                .expect("the layout is created before it is read");
            layout.set_width(content_width);
            layout.set_style(Arc::new(style.clone()));
            let runs = ThemedSourceRuns {
                highlighter: self.highlighter.as_ref(),
                theme: &highlight_theme,
                style: Arc::new(style.clone()),
            };
            let _prepared = layout.prepare(
                window.text_system(),
                SourceViewport {
                    scroll_x,
                    scroll_y,
                    width: content_width,
                    height,
                },
                Some(cursor),
                &runs,
            );
            state.active_line = Some(layout.line_of(cursor));
        }
        let prepare_elapsed = prepare_started.elapsed();
        let (stats, total_height, total_width, row_height) = {
            let state = self.state.borrow();
            let layout = state.layout.as_ref().expect("layout exists");
            (
                layout.stats(),
                layout.total_height(),
                layout.total_width() + gutter,
                layout.row_height(),
            )
        };
        let follow = self.last_cursor != Some(cursor);
        self.last_cursor = Some(cursor);
        if follow {
            self.follow_caret = true;
        }
        if self.follow_caret {
            let caret = self
                .state
                .borrow()
                .layout
                .as_ref()
                .and_then(|layout| layout.caret(cursor));
            let line = self
                .state
                .borrow()
                .layout
                .as_ref()
                .map_or(0, |layout| layout.line_of(cursor));
            let top = line as f32 * row_height;
            let mut next_x = scroll_x;
            let mut next_y = scroll_y;
            if let Some((caret, _)) = caret {
                let x = f32::from(caret.x);
                if x < scroll_x {
                    next_x = x;
                } else if x > scroll_x + content_width {
                    next_x = x - content_width;
                }
            }
            if top < scroll_y {
                next_y = top;
            } else if top + row_height > scroll_y + height {
                next_y = top + row_height - height;
            }
            if next_x != scroll_x || next_y != scroll_y {
                let max = self.scroll.max_offset();
                self.scroll.set_offset(point(
                    px((-next_x).clamp(-f32::from(max.x), 0.)),
                    px((-next_y).clamp(-f32::from(max.y), 0.)),
                ));
            }
            if caret.is_some()
                && top >= next_y
                && top + row_height <= next_y + height
                && next_y == scroll_y
            {
                self.follow_caret = false;
            }
        }
        let clamped_x = scroll_x.min((total_width - content_width).max(0.));
        let clamped_y = scroll_y.min((total_height - height).max(0.));
        if clamped_x != scroll_x || clamped_y != scroll_y {
            let max = self.scroll.max_offset();
            self.scroll.set_offset(point(
                px((-clamped_x).clamp(-f32::from(max.x), 0.)),
                px((-clamped_y).clamp(-f32::from(max.y), 0.)),
            ));
        }
        if stats.pending || self.follow_caret || follow {
            window.request_animation_frame();
        }
        let render_rest = {
            let mut state = self.state.borrow_mut();
            state.render_elapsed = render_started.elapsed() - prepare_elapsed;
            state.render_elapsed
        };
        perf::source_layout(stats, prepare_elapsed, total_width, total_height);

        let canvas = SourceCanvas {
            state: self.state.clone(),
            input: cx.entity(),
            prepare_elapsed,
            rendered: render_rest,
        };
        let completion = {
            let editor = self.editor.read(cx);
            let menu = editor.completion_menu_state();
            (focused && menu.open && !menu.items.is_empty()).then_some(menu.items.len())
        };
        let popup = completion.and_then(|len| {
            let caret = self
                .state
                .borrow()
                .layout
                .as_ref()
                .and_then(|layout| layout.caret(cursor));
            let (caret, caret_height) = caret?;
            let rows = len.min(COMPLETION_ROWS);
            let popup_width = COMPLETION_MAX_WIDTH
                .min(width)
                .max(COMPLETION_MIN_WIDTH.min(width));
            let popup_height = rows as f32 * COMPLETION_ROW_HEIGHT + COMPLETION_PADDING * 2.;
            let left =
                (gutter + f32::from(caret.x) - scroll_x).clamp(0., (width - popup_width).max(0.));
            let below = f32::from(caret.y) - scroll_y + f32::from(caret_height) + POPUP_GAP;
            let top = if below + popup_height <= height {
                below
            } else {
                (f32::from(caret.y) - scroll_y - popup_height - POPUP_GAP).max(0.)
            };
            self.completion_popup(
                PopupPlacement {
                    left,
                    top,
                    width: popup_width,
                    row_height: COMPLETION_ROW_HEIGHT,
                    font_size: f32::from(style.font_size),
                },
                cx,
            )
        });
        let scroll_area = div()
            .id(("source-surface", self.editor.entity_id()))
            .size_full()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .child(canvas)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.dismiss_completion(cx);
                    this.select(event.position, event.modifiers.shift, window, cx);
                    window.prevent_default();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                this.select(event.position, true, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _: &gpui_kit::MouseUpEvent, _, cx| {
                    cx.notify();
                }),
            );
        let mut root = div()
            .id(("source-surface-root", self.editor.entity_id()))
            .size_full()
            .relative()
            .child(scroll_area);
        if let Some(popup) = popup {
            root = root.child(popup);
        }
        let focus = self.editor.read(cx).focus_handle(cx);
        self.bind_commands(root.track_focus(&focus), cx)
            .test_support()
            .into_any_element()
    }
}

fn gutter_width(layout: Option<&SourceLayout>, style: &SourceStyle) -> f32 {
    let digits = layout.map_or(3, |layout| layout.line_count().max(1).ilog10() + 1);
    digits as f32 * f32::from(style.font_size) * 0.62 + GUTTER_PADDING * 2. + GUTTER_GAP
}

impl EntityInputHandler for SourceSurface {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.editor.update(cx, |editor, cx| {
            editor.text_for_range(range, adjusted, window, cx)
        })
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.editor.update(cx, |editor, cx| {
            editor.selected_text_range(ignore_disabled, window, cx)
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.editor
            .update(cx, |editor, cx| editor.marked_text_range(window, cx))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.unmark_text(window, cx));
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.paste(item, window, cx));
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(range, text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(range, text, selected, window, cx)
        });
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let text = self.editor.read(cx).text().clone();
        let offset = text.offset_utf16_to_offset(range.start);
        let state = self.state.borrow();
        let layout = state.layout.as_ref()?;
        let (position, height) = layout.caret(offset)?;
        let geometry = state.geometry;
        let gutter = state.gutter;
        Some(Bounds::new(
            point(
                geometry.origin.x + px(gutter) + position.x,
                geometry.origin.y + position.y,
            ),
            size(px(1.5), height.max(px(2.))),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.source_at(position)?;
        let text = self.editor.read(cx).text().clone();
        Some(text.offset_to_offset_utf16(offset))
    }

    fn text_length_utf16(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        let text = self.editor.read(cx).text().clone();
        Some(text.offset_to_offset_utf16(text.len()))
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            let text = editor.text().clone();
            let range =
                text.offset_utf16_to_offset(range.start)..text.offset_utf16_to_offset(range.end);
            editor.set_selected_range(range, cx);
        });
    }
}

struct SourceCanvas {
    state: Rc<RefCell<RenderState>>,
    input: Entity<SourceSurface>,
    prepare_elapsed: Duration,
    rendered: Duration,
}

impl IntoElement for SourceCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SourceCanvas {
    type RequestLayoutState = ();
    type PrepaintState = Duration;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let started = Instant::now();
        let state = self.state.borrow();
        let (height, width) = state.layout.as_ref().map_or((1., 1.), |layout| {
            (
                layout.total_height().max(1.),
                layout.total_width() + state.gutter,
            )
        });
        let style = gpui_kit::Style {
            size: size(
                gpui_kit::Length::Definite(px(width).into()),
                gpui_kit::Length::Definite(px(height).into()),
            ),
            ..Default::default()
        };
        let layout = window.request_layout(style, [], cx);
        let _ = started;
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Duration {
        let started = Instant::now();
        let mut state = self.state.borrow_mut();
        state.geometry.origin = bounds.origin;
        state.geometry.viewport = bounds.intersect(&window.content_mask().bounds);
        state.geometry.width = f32::from(state.geometry.viewport.size.width);
        started.elapsed()
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Duration,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let input = self.input.clone();
        input.update(cx, |input, cx| {
            let viewport = input.state.borrow().geometry.viewport;
            input.register_input(viewport, window, cx);
        });
        let mut state = self.state.borrow_mut();
        let (origin, viewport, gutter) = {
            let geometry = state.geometry;
            let scroll = input.read(cx).scroll.offset();
            (
                geometry.origin,
                SourceViewport {
                    scroll_x: (-f32::from(scroll.x)).max(0.),
                    scroll_y: (-f32::from(scroll.y)).max(0.),
                    width: (geometry.viewport.size.width - px(state.gutter))
                        .max(px(80.))
                        .into(),
                    height: f32::from(geometry.viewport.size.height),
                },
                state.gutter,
            )
        };
        let selection = state.selection.clone();
        let caret = state.caret.filter(|_| state.caret_visible);
        let active_line = state.active_line;
        let style = state
            .layout
            .as_ref()
            .map(|layout| layout.style().clone())
            .unwrap_or_else(|| Arc::new(SourceStyle::from_theme(cx)));
        if let Some(layout) = state.layout.as_mut() {
            let content_origin = point(origin.x + px(gutter), origin.y);
            layout.paint(
                window,
                cx,
                content_origin,
                viewport,
                selection,
                caret,
                active_line,
            );
            paint_gutter(
                window,
                cx,
                layout,
                origin,
                viewport,
                gutter,
                &style,
                active_line,
            );
        }
        drop(state);
        let paint = started.elapsed();
        let rendered = std::mem::take(&mut self.rendered);
        let prepare = std::mem::take(&mut self.prepare_elapsed);
        let _ = prepaint;
        perf::source_frame(prepare, rendered, Duration::ZERO, paint);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_gutter(
    window: &mut Window,
    cx: &mut App,
    layout: &SourceLayout,
    origin: Point<Pixels>,
    viewport: SourceViewport,
    gutter: f32,
    style: &SourceStyle,
    active_line: Option<usize>,
) {
    let row_height = layout.row_height();
    let first = ((viewport.scroll_y / row_height).floor() as usize).min(layout.line_count());
    let last = (((viewport.scroll_y + viewport.height) / row_height).ceil() as usize + 1)
        .min(layout.line_count());
    let theme = cx.theme();
    let gutter_background = theme
        .highlight_theme
        .style
        .editor_gutter_background
        .unwrap_or(theme.background);
    let line_number_color = theme
        .highlight_theme
        .style
        .editor_line_number
        .unwrap_or(theme.muted_foreground);
    let active_number_color = theme
        .highlight_theme
        .style
        .editor_active_line_number
        .unwrap_or(theme.foreground);
    let clip = window.content_mask().bounds;
    let band = Bounds::new(
        point(origin.x, clip.top()),
        size(px(gutter), clip.size.height),
    );
    window.paint_quad(fill(band.intersect(&clip), gutter_background));
    for line in first..last {
        let y = origin.y + px(line as f32 * row_height);
        let label = format!("{}", line + 1);
        let color = if Some(line) == active_line {
            active_number_color
        } else {
            line_number_color
        };
        let font = style.font.clone();
        let run = TextRun {
            len: label.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            SharedString::from(label),
            style.font_size,
            &[run],
            None,
        );
        let x = origin.x + px(gutter - GUTTER_GAP - f32::from(shaped.width()));
        let _ = shaped.paint(
            point(x, y),
            px(row_height),
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::{ProjectId, SourcePath};
    use astrolith_document::DocumentIdentity;
    use gpui_kit::{
        ElementInputHandler, EntityInputHandler, FocusHandle, TestAppContext, WindowHandle,
        WindowOptions, base::Root, test::TestInput, test::TestWindowExt,
    };

    struct SourceHost {
        surface: Entity<SourceSurface>,
        focus: FocusHandle,
    }

    impl Render for SourceHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .child(self.surface.clone())
        }
    }

    fn mount_source(
        cx: &mut TestAppContext,
        source: &str,
    ) -> (WindowHandle<Root>, Entity<SourceSurface>) {
        let mounted = cx.update(|cx| {
            gpui_kit::init(cx);
            let mut mounted = None;
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let identity = DocumentIdentity::new(
                    ProjectId::new("window").unwrap(),
                    1,
                    SourcePath::new("src/pages/index.astro").unwrap(),
                );
                let session =
                    cx.new(|cx| DocumentSession::new(identity, source.to_owned(), window, cx));
                session.update(cx, |session, cx| {
                    session.set_source_view(true, window, cx);
                });
                let editor = session.read(cx).editor.clone();
                let weak = session.downgrade();
                let surface =
                    cx.new(|cx| SourceSurface::new(weak, editor, SharedString::from("astro"), cx));
                mounted = Some(surface.clone());
                cx.new(|cx| SourceHost {
                    surface,
                    focus: cx.focus_handle(),
                })
            })
            .unwrap();
            (handle.downcast::<Root>().unwrap(), mounted.unwrap())
        });
        cx.run_until_parked();
        mounted
    }

    #[gpui_kit::test]
    fn surface_renders_lines_and_exposes_input_geometry(cx: &mut TestAppContext) {
        let (handle, surface) = mount_source(cx, "alpha\nbeta\n");
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let state = surface.read(cx).state.borrow();
            let layout = state.layout.as_ref().expect("layout exists");
            assert_eq!(layout.line_count(), 3);
            assert!(state.gutter > 0., "the line-number gutter is reserved");
            assert!(
                state.geometry.viewport.size.width > px(0.),
                "the visible viewport is known"
            );
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            let bounds = surface.read(cx).state.borrow().geometry.viewport;
            let geometry = surface.update(cx, |surface, cx| {
                surface.bounds_for_range(0..0, bounds, window, cx)
            });
            assert!(geometry.is_some(), "IME bounds come from the shaped line");
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, surface.clone()), window);
            input.compose_text(None, "に", Some(1..1), cx);
            assert!(
                input.marked_text_range(cx).is_some(),
                "composition state stays on the retained editor model"
            );
            drop(input);
            let text = surface.update(cx, |surface, cx| {
                surface.text_for_range(0..4, &mut None, window, cx)
            });
            assert!(
                text.is_some_and(|text| text.contains('に')),
                "text_for_range reads the composed input model"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn long_line_frame_shapes_only_the_viewport(cx: &mut TestAppContext) {
        let source = "x".repeat(1024 * 1024);
        let (handle, surface) = mount_source(cx, &source);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        surface.read_with(cx, |surface, _| {
            let state = surface.state.borrow();
            let stats = state.layout.as_ref().expect("layout exists").stats();
            assert!(stats.shaped_lines >= 1);
            assert!(
                stats.shaped_bytes < 64 * 1024,
                "one frame shapes only the visible window: {} bytes",
                stats.shaped_bytes
            );
        });
    }

    #[gpui_kit::test]
    fn completion_menu_mirrors_the_retained_overlay(cx: &mut TestAppContext) {
        let (handle, surface) = mount_source(cx, "alp\n");
        surface.update(cx, |surface, cx| {
            let editor = surface.editor.clone();
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(3..3, cx);
                editor.present_completion_items(
                    0,
                    "alp",
                    vec![
                        lsp_types::CompletionItem {
                            label: "alpha".into(),
                            ..Default::default()
                        },
                        lsp_types::CompletionItem {
                            label: "alpine".into(),
                            ..Default::default()
                        },
                    ],
                    cx,
                );
            });
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        surface.read_with(cx, |surface, _| assert_eq!(surface.completion_selected, 0));
        surface.update(cx, |surface, cx| {
            assert!(
                surface.move_completion(1, cx),
                "the open menu owns MoveDown"
            );
        });
        surface.read_with(cx, |surface, _| assert_eq!(surface.completion_selected, 1));
        cx.update_window(handle.into(), |_, window, cx| {
            surface.update(cx, |surface, cx| {
                assert!(surface.accept_completion(window, cx));
            });
        })
        .unwrap();
        surface.read_with(cx, |surface, cx| {
            assert_eq!(surface.editor.read(cx).text().to_string(), "alpine\n");
            assert!(!surface.editor.read(cx).completion_menu_state().open);
        });
    }
}
