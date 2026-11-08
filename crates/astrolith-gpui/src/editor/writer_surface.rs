//! WYSIWYG writing surface.
//!
//! The retained [`EditorState`] remains the input model: it owns the cursor,
//! selection, IME composition and undo integration. This view renders the same
//! writer projection through the application-owned viewport text layout, paints
//! the caret and selection itself, and maps pointer positions back into the
//! editor's byte offsets. Markdown syntax (heading hashes, emphasis markers,
//! list bullets, link destinations and fences) is not painted; the source
//! stays byte-exact.
//!
//! Only the visible visual rows and their overscan are wrapped and shaped.
//! Long blocks advance across frames under a bounded budget, and geometry for
//! hit-testing, the caret, selection and IME comes from the same row snapshot
//! that paints.

mod commands;
mod input;

use super::perf;
use super::text_change::TextChange;
use super::text_layout::{
    Caret, Decoration, FocusSentence, Hit, LayoutStyle, Selection, TextLayout, Viewport,
};
use gpui_kit::component::input::Rope;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use astrolith_document::{WysiwygDocument, sentences};
use gpui_kit::base::AutoScroll;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    App, Bounds, Context, Element, ElementId, Focusable, GlobalElementId, Hsla, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, ScrollHandle,
    TestSupportExt, Window, div, fill, point, prelude::*, px, relative, size,
};

use crate::editor::session::DocumentSession;
use crate::editor::writing_modes::ProseStyle;

/// One frame's content placement and clipped viewport in window coordinates.
#[derive(Clone, Copy)]
struct FrameGeometry {
    origin: Point<Pixels>,
    viewport: Bounds<Pixels>,
    /// Width of the laid-out content column.
    width: f32,
    /// Scroll distance used when this frame's origin was measured.
    scroll_y: f32,
}

impl Default for FrameGeometry {
    fn default() -> Self {
        Self {
            origin: point(px(0.), px(0.)),
            viewport: Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.))),
            width: 0.,
            scroll_y: 0.,
        }
    }
}

/// The writing surface for one retained document session.
pub(crate) struct WriterSurface {
    session: gpui_kit::WeakEntity<DocumentSession>,
    caret_state: Option<(bool, usize, Range<usize>)>,
    /// Caret painting and blinking live in this child entity, so a blink never
    /// invalidates the text layout.
    caret: gpui_kit::Entity<CaretOverlay>,
    /// Application-owned layout for the accepted document revision. Rebuilt
    /// only when the width or font metrics change.
    layout: Rc<RefCell<Option<TextLayout>>>,
    layout_style: Option<Arc<LayoutStyle>>,
    /// Content width the current layout was built for.
    laid_out_width: Option<f32>,
    /// Last frame's measured writer work, for the opt-in counters.
    frame_stats: perf::Frame,
    document: Arc<WysiwygDocument>,
    source: Rope,
    parsed_source: Rope,
    /// Exact text the accepted document was parsed from, shared with tasks.
    parsed_text: Arc<String>,
    requested_revision: Option<u64>,
    pending_edit: Option<TextChange>,
    pending_snapshot: Option<(u64, Rope)>,
    /// When the latest snapshot was requested, to report queue wait.
    parse_queued_at: Option<Instant>,
    /// When the accepted snapshot was requested, for document convergence.
    snapshot_started_at: Option<Instant>,
    parse_task: Option<gpui_kit::Task<()>>,
    parse_cancel: Arc<AtomicBool>,
    style: ProseStyle,
    geometry: Rc<RefCell<FrameGeometry>>,
    scroll: ScrollHandle,
    /// Source offset where the current drag began.
    drag_anchor: Option<usize>,
    auto_scroll: AutoScroll,
    /// Buffer revision the parsed document was built from.
    document_revision: Option<u64>,
    last_cursor: Option<usize>,
    /// Keep following the caret until its row is visible; a user scroll after
    /// that leaves the viewport alone.
    follow_caret: bool,
    /// Sentence boundaries for the active rendered block of this revision.
    focus_sentences: Option<(usize, Vec<Range<usize>>)>,
    #[cfg(test)]
    last_focus_sentence: Option<FocusSentence>,
    /// Layout rebuilds since construction, for the tests.
    #[cfg(test)]
    layout_builds: usize,
}

impl Drop for WriterSurface {
    fn drop(&mut self) {
        self.parse_cancel.store(true, Ordering::Relaxed);
    }
}

impl WriterSurface {
    pub(crate) fn new(
        session: gpui_kit::WeakEntity<DocumentSession>,
        style: ProseStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let geometry = Rc::new(RefCell::new(FrameGeometry::default()));
        let layout = Rc::new(RefCell::new(None));
        let caret = cx.new(|cx| CaretOverlay::new(layout.clone(), geometry.clone(), cx));
        Self {
            session,
            caret_state: None,
            caret,
            layout,
            layout_style: None,
            laid_out_width: None,
            frame_stats: perf::Frame::default(),
            document: Arc::new(WysiwygDocument::default()),
            geometry,
            source: Rope::new(),
            parsed_source: Rope::new(),
            parsed_text: Arc::new(String::new()),
            requested_revision: None,
            pending_edit: None,
            pending_snapshot: None,
            parse_queued_at: None,
            snapshot_started_at: None,
            parse_task: None,
            parse_cancel: Arc::new(AtomicBool::new(false)),
            style,
            scroll: ScrollHandle::new(),
            drag_anchor: None,
            auto_scroll: AutoScroll::default(),
            document_revision: None,
            last_cursor: None,
            follow_caret: false,
            focus_sentences: None,
            #[cfg(test)]
            last_focus_sentence: None,
            #[cfg(test)]
            layout_builds: 0,
        }
    }

    pub(crate) fn request_caret_follow(&mut self) {
        self.last_cursor = None;
        self.follow_caret = true;
    }

    /// Stop the caret blink while the writing surface is not on screen.
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if !visible {
            self.caret.update(cx, |caret, cx| caret.clear(cx));
        }
    }

    /// Move the viewport to an absolute translation. The native harness uses
    /// this instead of synthesising wheel input, which Wayland cannot inject.
    pub(crate) fn set_scroll_offset(&mut self, y: f32, cx: &mut Context<Self>) {
        let max = f32::from(self.scroll.max_offset().y);
        let target = px(clamp_scroll_offset(y, max));
        let offset = self.scroll.offset();
        if target != offset.y {
            self.scroll.set_offset(point(offset.x, target));
        }
        cx.notify();
    }

    /// Install the configured document typography.
    pub(crate) fn set_style(&mut self, style: ProseStyle, cx: &mut Context<Self>) {
        if self.style == style {
            return;
        }
        self.style = style;
        // The next render compares font metrics; a colour-only change keeps
        // the wrapped rows and only re-shapes the visible ones.
        self.last_cursor = None;
        cx.notify();
    }

    pub(crate) fn observe_text(&mut self, source: Rope, cx: &mut Context<Self>) {
        if self.requested_revision.is_some() && self.source == source {
            return;
        }
        self.caret_state = None;
        let revision = self
            .requested_revision
            .map_or(0, |revision| revision.wrapping_add(1));
        self.set_snapshot(revision, source, cx);
    }

    /// Snapshot input changes outside rendering. At most one parse runs and
    /// one latest snapshot waits; intermediate preedit revisions never queue.
    pub(crate) fn set_snapshot(&mut self, revision: u64, source: Rope, cx: &mut Context<Self>) {
        if self.requested_revision == Some(revision) {
            return;
        }
        self.requested_revision = Some(revision);
        self.parse_cancel.store(true, Ordering::Relaxed);
        self.pending_edit = TextChange::between(&self.parsed_source, &source);
        self.source = source.clone();
        self.pending_snapshot = Some((revision, source));
        self.parse_queued_at = perf::start();
        self.snapshot_started_at = Some(Instant::now());
        self.start_parse(cx);
    }

    fn start_parse(&mut self, cx: &mut Context<Self>) {
        if self.parse_task.is_some() {
            return;
        }
        let Some((revision, source)) = self.pending_snapshot.take() else {
            return;
        };
        if let Some(queued) = self.parse_queued_at.take() {
            perf::span("parse_queue", "wait", queued.elapsed());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.parse_cancel = cancel.clone();
        // Pending hit-test geometry already holds the diff from the accepted
        // document to this snapshot. Obsolete results cannot change that base.
        let previous_text = self.parsed_text.clone();
        let previous_document = self.document.clone();
        let change = self.pending_edit.clone();
        let work = cx.background_spawn(async move {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let started = std::time::Instant::now();
            let text = source.to_string();
            let parsed = parse_snapshot(&text, &previous_text, &previous_document, change, &cancel)?;
            let elapsed = started.elapsed();
            tracing::debug!(revision, bytes = source.len(), parse_ms = elapsed.as_secs_f64() * 1_000.,
                reused_blocks = parsed.1,
                thread = ?std::thread::current().id(), "parsed writer snapshot off thread");
            Some((source, text, parsed.0, parsed.1, elapsed))
        });
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.parse_task = None;
                if let Some((source, text, document, reused_blocks, parsed)) = result {
                    perf::parse_run(source.len(), reused_blocks, parsed);
                    if this.requested_revision == Some(revision) {
                        this.install_document(revision, source, text, document, cx);
                        cx.notify();
                    }
                }
                this.start_parse(cx);
            });
        }));
    }

    fn install_document(
        &mut self,
        revision: u64,
        source: Rope,
        text: String,
        document: WysiwygDocument,
        cx: &mut Context<Self>,
    ) {
        let first_install = self.document_revision.is_none();
        self.document_revision = Some(revision);
        self.parsed_source = source;
        self.parsed_text = Arc::new(text);
        self.pending_edit = None;
        self.document = Arc::new(document);
        self.focus_sentences = None;
        // Unchanged blocks keep their rows and shaped lines across the edit;
        // the edited block keeps the safe row prefix.
        let reconcile = Instant::now();
        if let Some(layout) = self.layout.borrow_mut().as_mut() {
            let outcome = layout.update(self.document.clone());
            perf::text_layout_edit(outcome);
        }
        if let Some(started) = self.snapshot_started_at.take() {
            perf::span("document_ready", "layout_reconcile", reconcile.elapsed());
            let event = if first_install {
                "initial_install"
            } else {
                "edit_install"
            };
            perf::span("document_ready", event, started.elapsed());
        }
        self.last_cursor = None;
        self.caret.update(cx, |caret, cx| caret.clear(cx));
    }

    /// Reuse the current layout for one accepted document revision.
    ///
    /// The layout is rebuilt only when the column width or the shaping font
    /// metrics change. Colour-only changes restyle the resolved runs.
    fn sync_layout(&mut self, width: f32, cx: &App) {
        let style = Arc::new(LayoutStyle::from_app(&self.style, cx));
        let width = width.max(1.);
        let mut slot = self.layout.borrow_mut();
        let rebuild = match slot.as_ref() {
            None => true,
            Some(layout) => {
                !layout.style().metrics_eq(&style)
                    || self
                        .laid_out_width
                        .is_some_and(|previous| (previous - width).abs() > 0.5)
            }
        };
        if rebuild {
            *slot = Some(TextLayout::new(
                self.document.clone(),
                style.clone(),
                px(width),
                &super::text_layout::ImageMetrics::default(),
            ));
            self.laid_out_width = Some(width);
            self.layout_style = Some(style);
            #[cfg(test)]
            {
                self.layout_builds += 1;
            }
            return;
        }
        if let Some(layout) = slot.as_mut()
            && *layout.style() != *style
        {
            layout.set_style(style.clone());
        }
        self.layout_style = Some(style);
        self.laid_out_width = Some(width);
    }

    /// The parsed hit for a source offset in the accepted document.
    fn hit_for_source(&self, source: usize) -> Option<Hit> {
        let (block, rendered) = self.document.caret_location(source)?;
        Some(Hit { block, rendered })
    }

    /// Source offset under a window position.
    pub(crate) fn source_at(&self, position: Point<Pixels>) -> Option<usize> {
        let (block, rendered) = self.location_at(position)?;
        let source = self.document.source_at(block, rendered)?;
        self.pending_edit
            .as_ref()
            .map_or(Some(source), |edit| edit.to_current(source))
    }

    /// The accepted parsed document and the exact text it was built from, so
    /// the read-only preview can reuse it instead of parsing the revision
    /// again.
    pub(crate) fn shared_document(&self) -> Option<(Arc<String>, Arc<WysiwygDocument>)> {
        self.document_revision
            .map(|_| (self.parsed_text.clone(), self.document.clone()))
    }

    pub(super) fn parsed_offset(&self, offset: usize) -> usize {
        self.pending_edit
            .as_ref()
            .map_or(offset, |edit| edit.to_parsed(offset))
    }

    /// Link destination under a window position.
    pub(crate) fn url_at(&self, position: Point<Pixels>) -> Option<String> {
        if self.pending_edit.is_some() {
            return None;
        }
        let (block, rendered) = self.location_at(position)?;
        self.document.link_at(block, rendered).map(str::to_owned)
    }

    /// Image destination under a window position.
    pub(crate) fn image_at(&self, position: Point<Pixels>) -> Option<String> {
        if self.pending_edit.is_some() {
            return None;
        }
        let (block, rendered) = self.location_at(position)?;
        self.document.image_at(block, rendered).map(str::to_owned)
    }

    /// The rendered selection rectangle and viewport from the last frame.
    pub(crate) fn selection_geometry(
        &self,
        selection: &Range<usize>,
    ) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        if self.pending_edit.is_some() {
            return None;
        }
        let geometry = *self.geometry.borrow();
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let anchor = self.hit_for_source(selection.start)?;
        let focus = self.hit_for_source(selection.end)?;
        let bounds = layout.bounds_for_range(anchor, focus)?;
        Some((
            Bounds::new(
                point(
                    geometry.origin.x + bounds.origin.x,
                    geometry.origin.y + bounds.origin.y,
                ),
                bounds.size,
            ),
            geometry.viewport,
        ))
    }

    /// Move the editor selection along the rendered layout.
    ///
    /// Left/right skip Markdown syntax that is not painted; up/down follow the
    /// rendered line boxes rather than source lines, so headings and wrapped
    /// paragraphs behave like the text the reader sees.
    pub(crate) fn navigate(
        &mut self,
        key: &str,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.session.upgrade() else {
            return false;
        };
        let (focus, selection) = {
            let session = session.read(cx);
            let editor = session.editor.read(cx);
            (editor.cursor(), editor.selected_range())
        };
        if self
            .pending_edit
            .as_ref()
            .is_some_and(|edit| edit.contains_current(focus))
        {
            return false;
        }
        let document = self.document.clone();
        let Some((block, rendered)) = document.caret_location(self.parsed_offset(focus)) else {
            return false;
        };
        let anchor = if shift {
            if focus == selection.start {
                selection.end
            } else {
                selection.start
            }
        } else {
            focus
        };
        let destination = match key {
            "left" => previous_rendered(&document, block, rendered),
            "right" => following_rendered(&document, block, rendered),
            "home" => Some((block, 0)),
            "end" => document
                .blocks
                .get(block)
                .map(|block| (block, block.text.text.len()))
                .map(|(_, len)| (block, len)),
            "up" | "down" => {
                let hit = Hit { block, rendered };
                let layout = self.layout.borrow();
                layout
                    .as_ref()
                    .and_then(|layout| layout.vertical_target(hit, key == "up"))
                    .map(|hit| (hit.block, hit.rendered))
            }
            _ => return false,
        };
        let Some((next_block, next_rendered)) = destination else {
            return false;
        };
        let Some(source) = document.source_at(next_block, next_rendered) else {
            return false;
        };
        let Some(source) = self
            .pending_edit
            .as_ref()
            .map_or(Some(source), |edit| edit.to_current(source))
        else {
            return false;
        };
        let anchor = if shift { anchor } else { source };
        session.update(cx, |session, cx| {
            session.set_writer_selection(anchor, source, window, cx);
        });
        cx.notify();
        true
    }

    fn location_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let geometry = *self.geometry.borrow();
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let hit = layout.hit_test(point(
            position.x - geometry.origin.x,
            position.y - geometry.origin.y
                + px((-f32::from(self.scroll.offset().y)).max(0.) - geometry.scroll_y),
        ))?;
        Some((hit.block, hit.rendered))
    }

    /// The viewport height the visible window is derived from.
    fn effective_viewport_height(&self, window: &Window) -> f32 {
        let geometry = f32::from(self.geometry.borrow().viewport.size.height);
        let window_height = f32::from(window.viewport_size().height);
        if geometry > 0. {
            geometry.min(window_height)
        } else {
            window_height
        }
    }

    /// Map a pointer position to a source offset and update the selection.
    fn select(
        &mut self,
        position: Point<Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let offset = match self.source_at(position) {
            Some(offset) => offset,
            None => {
                // An unshaped row or a pointer outside the layout is not EOF.
                // Keep the current selection until hit testing resolves a real row.
                let focus = session.read(cx).editor.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                return;
            }
        };
        let anchor = if extend {
            self.drag_anchor.or_else(|| {
                Some(session.read_with(cx, |session, cx| {
                    let editor = session.editor.read(cx);
                    let selection = editor.selected_range();
                    let cursor = editor.cursor();
                    if cursor == selection.start {
                        selection.end
                    } else {
                        selection.start
                    }
                }))
            })
        } else {
            None
        }
        .unwrap_or(offset);
        session.update(cx, |session, cx| {
            session.set_writer_selection(anchor, offset, window, cx);
        });
        if !extend {
            self.drag_anchor = Some(offset);
        }
        cx.notify();
    }

    fn update_drag_autoscroll(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.geometry.borrow().viewport;
        self.auto_scroll.last_drag_position = Some(position);
        let delta = AutoScroll::compute_delta(position.y, viewport).map(|delta| -delta);
        let window = window.window_handle();
        let writer = cx.weak_entity();
        self.auto_scroll.set(delta, cx, move |delta, state, cx| {
            let offset = state.scroll.offset();
            let max_y = f32::from(state.scroll.max_offset().y).max(0.);
            let y = (f32::from(offset.y) + f32::from(delta)).clamp(-max_y, 0.);
            if px(y) == offset.y {
                state.auto_scroll.stop();
                return;
            }
            state.scroll.set_offset(point(offset.x, px(y)));
            let window = window;
            let writer = writer.clone();
            cx.defer(move |cx| {
                _ = window.update(cx, |_, window, cx| {
                    _ = writer.update(cx, |writer, cx| {
                        if writer.drag_anchor.is_some()
                            && writer.auto_scroll.is_active()
                            && let Some(position) = writer.auto_scroll.last_drag_position
                        {
                            writer.select(position, true, window, cx);
                        }
                    });
                });
            });
        });
    }

    /// Keep the caret in view; typewriter mode centres it.
    fn adjust_scroll(&mut self, caret: Option<Caret>, typewriter: bool, viewport_height: f32) {
        let Some(caret) = caret else {
            return;
        };
        if viewport_height <= 0. {
            return;
        }
        let offset = self.scroll.offset();
        let scroll = (-f32::from(offset.y)).max(0.);
        let content_y = f32::from(caret.origin.y) - scroll;
        let mut target = f32::from(offset.y);
        if typewriter {
            let centre = (viewport_height - f32::from(caret.height)) / 2.;
            target += centre - content_y;
        } else {
            let top = 20.;
            let bottom = viewport_height - 28.;
            if content_y < top {
                target += top - content_y;
            } else if content_y + f32::from(caret.height) > bottom {
                target -= content_y + f32::from(caret.height) - bottom;
            }
        }
        let max = f32::from(self.scroll.max_offset().y);
        let target = px(clamp_scroll_offset(target, max));
        if target != offset.y {
            self.scroll.set_offset(point(offset.x, target));
        }
    }

    /// Keep the scroll offset inside a content height that changed.
    fn clamp_scroll(&mut self, total_height: f32, viewport_height: f32) {
        let max = (total_height - viewport_height).max(0.);
        let offset = self.scroll.offset();
        if f32::from(offset.y) < -max {
            self.scroll.set_offset(point(offset.x, px(-max)));
        }
    }
}
/// Clamp a scroll translation to GPUI's convention: 0 at the top, negative
/// while scrolling down, bounded by the positive `max` distance.
fn clamp_scroll_offset(target: f32, max: f32) -> f32 {
    target.clamp(-max.max(0.), 0.)
}

/// The previous rendered character, crossing block boundaries.
fn previous_rendered(
    document: &WysiwygDocument,
    block: usize,
    rendered: usize,
) -> Option<(usize, usize)> {
    if rendered > 0 {
        let text = &document.blocks.get(block)?.text.text;
        let previous = text[..rendered]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        return Some((block, previous));
    }
    for index in (0..block).rev() {
        let text = &document.blocks.get(index)?.text.text;
        if !text.is_empty() {
            return Some((index, text.len()));
        }
    }
    None
}

/// The next rendered character, crossing block boundaries.
fn following_rendered(
    document: &WysiwygDocument,
    block: usize,
    rendered: usize,
) -> Option<(usize, usize)> {
    let text = &document.blocks.get(block)?.text.text;
    if rendered < text.len() {
        let next = text[rendered..]
            .chars()
            .next()
            .map_or(rendered, |character| rendered + character.len_utf8());
        return Some((block, next));
    }
    for index in block + 1..document.blocks.len() {
        let text = &document.blocks[index].text.text;
        if !text.is_empty() {
            return Some((index, 0));
        }
    }
    None
}

impl WriterSurface {
    fn focused_sentence(&mut self, hit: Hit) -> FocusSentence {
        if self
            .focus_sentences
            .as_ref()
            .is_none_or(|(block, _)| *block != hit.block)
        {
            self.focus_sentences = Some((
                hit.block,
                sentences(&self.document.blocks[hit.block].text.text),
            ));
        }
        let (_, ranges) = self
            .focus_sentences
            .as_ref()
            .expect("the active block is indexed");
        let index = ranges.partition_point(|range| range.end <= hit.rendered);
        let range = ranges
            .get(index)
            .or_else(|| ranges.last())
            .cloned()
            .unwrap_or(0..0);
        FocusSentence {
            block: hit.block,
            range,
        }
    }
}

impl Render for WriterSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let render_started = Instant::now();
        let Some(session) = self.session.upgrade() else {
            return div().size_full().into_any_element();
        };
        let (cursor, selection, focused, modes, placeholders) = {
            let session = session.read(cx);
            let editor = session.editor.read(cx);
            (
                editor.cursor().min(editor.text().len()),
                editor.selected_range(),
                editor.focus_handle(cx).is_focused(window),
                session.writing_modes,
                session.placeholder_ranges().to_vec(),
            )
        };
        let copyedit = if self.pending_edit.is_none() {
            session.read(cx).copyedit.render_ranges(cursor)
        } else {
            Vec::new()
        };
        let cursor = self.parsed_offset(cursor);
        let selection = self.parsed_offset(selection.start)..self.parsed_offset(selection.end);
        let placeholders = if self.pending_edit.is_none() {
            placeholders
        } else {
            Vec::new()
        };
        let document = self.document.clone();
        let viewport_height = self.effective_viewport_height(window);
        let content_width = self.geometry.borrow().width;
        let width = if content_width > 0. {
            content_width
        } else {
            (f32::from(window.viewport_size().width) * 0.7).max(240.)
        };
        self.sync_layout(width, cx);
        let scroll = (-f32::from(self.scroll.offset().y)).max(0.);
        let caret_hit = document
            .caret_location(cursor)
            .map(|(block, rendered)| Hit { block, rendered });
        let selection_hits = if selection.is_empty() {
            None
        } else {
            self.hit_for_source(selection.start)
                .zip(self.hit_for_source(selection.end))
                .map(|(anchor, focus)| Selection { anchor, focus })
        };
        let prepare_started = Instant::now();
        let (prepared, layout_stats, rows, caret, total_height, pending_bytes) = {
            let mut slot = self.layout.borrow_mut();
            let layout = slot
                .as_mut()
                .expect("the layout is created before it is read");
            let prepared = layout.prepare(
                window,
                Viewport {
                    scroll,
                    height: viewport_height,
                },
                caret_hit,
                viewport_height * 0.5,
            );
            (
                prepared,
                layout.stats(),
                layout.measured_rows(),
                caret_hit.and_then(|hit| layout.caret(hit)),
                layout.total_height(),
                caret_hit.map_or(0, |hit| layout.pending_bytes(hit.block)),
            )
        };
        let prepare_elapsed = prepare_started.elapsed();
        perf::text_layout(
            "writer",
            layout_stats,
            prepare_elapsed,
            rows,
            document.blocks.len(),
            total_height,
            pending_bytes,
            width,
            scroll,
        );

        let follow_cursor = self.last_cursor != Some(cursor);
        self.last_cursor = Some(cursor);
        if follow_cursor {
            self.follow_caret = true;
        }
        if self.follow_caret {
            self.adjust_scroll(caret, focused && modes.typewriter, viewport_height);
            if let Some(caret) = caret {
                let scroll = (-f32::from(self.scroll.offset().y)).max(0.);
                let top = f32::from(caret.origin.y);
                let bottom = top + f32::from(caret.height);
                if top >= scroll && bottom <= scroll + viewport_height {
                    self.follow_caret = false;
                }
            }
        }
        self.clamp_scroll(total_height, viewport_height);
        if prepared.pending || follow_cursor || self.follow_caret {
            window.request_animation_frame();
        }

        let mut decorations = Vec::new();
        if self.pending_edit.is_none() {
            let placeholder_color = cx.theme().primary.opacity(0.18);
            for (range, color) in &copyedit {
                push_source_decoration(&mut decorations, &document, range.clone(), *color, false);
            }
            for range in &placeholders {
                push_source_decoration(
                    &mut decorations,
                    &document,
                    range.clone(),
                    placeholder_color,
                    true,
                );
            }
        }
        let focus_sentence = if modes.focus {
            caret_hit.map(|hit| self.focused_sentence(hit))
        } else {
            None
        };
        #[cfg(test)]
        {
            self.last_focus_sentence = focus_sentence.clone();
        }

        let caret_target = if focused && selection.is_empty() {
            caret_hit.map(|hit| CaretTarget {
                hit,
                color: cx.theme().caret,
            })
        } else {
            None
        };
        let caret_blinks = self.caret.read(cx).blinks;
        self.caret
            .update(cx, |caret, cx| caret.set_target(caret_target, cx));
        self.frame_stats = perf::Frame {
            blocks_total: document.blocks.len(),
            blocks_rendered: prepared.blocks.len(),
            shaping_bytes: layout_stats.shaped_bytes,
            decorations: decorations.len(),
            caret_blinks,
            ..perf::Frame::default()
        };
        let canvas = LayoutCanvas {
            layout: self.layout.clone(),
            geometry: self.geometry.clone(),
            scroll_y: scroll,
            selection: selection_hits,
            caret: caret_hit,
            decorations,
            focus_sentence,
            input: cx.entity(),
            stats: self.frame_stats,
            render_elapsed: render_started.elapsed(),
            painted: false,
        };
        let content = div()
            .id("writer-wysiwyg-content")
            .test_support()
            .cursor_text()
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(canvas)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.auto_scroll.stop();
                    this.drag_anchor = None;
                    let extend = event.modifiers.shift;
                    this.select(event.position, extend, window, cx);
                    // The writer chose its input model; ancestors must not steal focus.
                    window.prevent_default();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                this.select(event.position, true, window, cx);
                this.update_drag_autoscroll(event.position, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, cx| {
                    this.auto_scroll.stop();
                    this.drag_anchor = None;
                    cx.notify();
                }),
            );

        let editor = session.read(cx).editor.clone();
        let root = div()
            .id(("input", editor.entity_id()))
            .size_full()
            .child(content)
            .child(self.caret.clone());
        self.bind_commands(root.track_focus(&editor.read(cx).focus_handle(cx)), cx)
            .test_support()
            .into_any_element()
    }
}

/// Clip a source range to every block it covers and record the rendered
/// range as a paint decoration.
fn push_source_decoration(
    decorations: &mut Vec<Decoration>,
    document: &WysiwygDocument,
    source: Range<usize>,
    color: Hsla,
    anchor_empty: bool,
) {
    for (index, block) in document.blocks.iter().enumerate() {
        if source.end < block.source.start || source.start > block.source.end {
            continue;
        }
        let start = if source.start <= block.content.start {
            0
        } else {
            block.text.nearest_rendered_offset(source.start)
        };
        let end = if source.end >= block.content.end {
            block.text.text.len()
        } else {
            block.text.nearest_rendered_offset(source.end)
        };
        let range = if start < end {
            start..end
        } else if anchor_empty && start < block.text.text.len() {
            start..(start + 1)
        } else {
            continue;
        };
        decorations.push(Decoration {
            block: index,
            range,
            color,
        });
    }
}

/// The viewport-height canvas that paints the prepared rows.
struct LayoutCanvas {
    layout: Rc<RefCell<Option<TextLayout>>>,
    geometry: Rc<RefCell<FrameGeometry>>,
    scroll_y: f32,
    selection: Option<Selection>,
    caret: Option<Hit>,
    decorations: Vec<Decoration>,
    focus_sentence: Option<FocusSentence>,
    input: gpui_kit::Entity<WriterSurface>,
    stats: perf::Frame,
    render_elapsed: std::time::Duration,
    painted: bool,
}

impl IntoElement for LayoutCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LayoutCanvas {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        let height = self
            .layout
            .borrow()
            .as_ref()
            .map_or(1., |layout| layout.total_height().max(1.));
        let style = gpui_kit::Style {
            size: size(
                gpui_kit::Length::Definite(relative(1.0)),
                gpui_kit::Length::Definite(px(height).into()),
            ),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let mut geometry = self.geometry.borrow_mut();
        if (geometry.width - f32::from(bounds.size.width)).abs() > 0.5 {
            // Render used the previous frame's measurement. Reconcile on the
            // next frame even when short content has no pending layout work.
            window.request_animation_frame();
        }
        geometry.origin = bounds.origin;
        geometry.width = f32::from(bounds.size.width);
        geometry.viewport = bounds.intersect(&window.content_mask().bounds);
        geometry.scroll_y = self.scroll_y;
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let viewport = self.geometry.borrow().viewport;
        let input = self.input.clone();
        input.update(cx, |input, cx| input.register_input(viewport, window, cx));
        self.layout.borrow_mut().as_mut().map(|layout| {
            layout.paint(
                window,
                cx,
                bounds.origin,
                self.selection,
                self.caret,
                &self.decorations,
                self.focus_sentence.as_ref(),
                None,
            )
        });
        if std::mem::replace(&mut self.painted, true) {
            perf::frame_repaint(self.stats, started.elapsed());
        } else {
            perf::frame(
                self.stats,
                self.render_elapsed,
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
                started.elapsed(),
            );
        }
    }
}

/// Where the caret is drawn for the current layout epoch.
#[derive(Clone, Copy, PartialEq)]
struct CaretTarget {
    hit: Hit,
    color: Hsla,
}

/// Caret paint layer.
///
/// Blinking toggles `visible` and invalidates this entity only. The text
/// layout, parsed document and prepared rows are not touched.
struct CaretOverlay {
    layout: Rc<RefCell<Option<TextLayout>>>,
    geometry: Rc<RefCell<FrameGeometry>>,
    target: Option<CaretTarget>,
    visible: bool,
    blinks: u64,
    task: Option<gpui_kit::Task<()>>,
}

impl CaretOverlay {
    fn new(
        layout: Rc<RefCell<Option<TextLayout>>>,
        geometry: Rc<RefCell<FrameGeometry>>,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            layout,
            geometry,
            target: None,
            visible: true,
            blinks: 0,
            task: None,
        }
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if self.target.take().is_some() {
            self.task = None;
            cx.notify();
        }
    }

    /// Install the caret target. An unchanged target keeps the running blink
    /// phase; a new target restarts it visible.
    fn set_target(&mut self, target: Option<CaretTarget>, cx: &mut Context<Self>) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.visible = true;
        self.task = None;
        if target.is_some() {
            self.task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            this.visible = !this.visible;
                            this.blinks += 1;
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
}

impl Render for CaretOverlay {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        CaretLayer {
            layout: self.layout.clone(),
            geometry: self.geometry.clone(),
            target: if self.visible { self.target } else { None },
        }
        .into_any_element()
    }
}

struct CaretLayer {
    layout: Rc<RefCell<Option<TextLayout>>>,
    geometry: Rc<RefCell<FrameGeometry>>,
    target: Option<CaretTarget>,
}

impl IntoElement for CaretLayer {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CaretLayer {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        let style = gpui_kit::Style {
            position: gpui_kit::Position::Absolute,
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let Some(target) = self.target else {
            return;
        };
        let geometry = *self.geometry.borrow();
        let caret = self
            .layout
            .borrow()
            .as_ref()
            .and_then(|layout| layout.caret(target.hit));
        let Some(caret) = caret else {
            return;
        };
        let bounds = Bounds::new(
            point(
                geometry.origin.x + caret.origin.x,
                geometry.origin.y + caret.origin.y,
            ),
            size(px(1.5), caret.height.max(px(2.))),
        );
        // Clipped by the flow's own viewport instead of painting over the
        // chrome when the caret has scrolled away.
        if !geometry.viewport.intersects(&bounds) {
            return;
        }
        window.paint_quad(fill(bounds, target.color));
    }
}

/// Parse one snapshot off the UI thread.
///
/// A local edit reuses unchanged blocks through the incremental path. Edits the
/// incremental parser declines (a touched reference definition, or a missing
/// diff) take the whole-document path, so this never exposes a partial tree.
fn parse_snapshot(
    text: &str,
    previous_text: &str,
    previous: &WysiwygDocument,
    change: Option<TextChange>,
    cancel: &AtomicBool,
) -> Option<(WysiwygDocument, usize)> {
    if let Some(change) = change {
        if let Some(parsed) = WysiwygDocument::parse_incremental_cancellable(
            text,
            previous_text,
            previous,
            change.old,
            change.new,
            || cancel.load(Ordering::Relaxed),
        ) {
            return Some((parsed.document, parsed.reused_blocks));
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
    }
    WysiwygDocument::parse_cancellable(text, || cancel.load(Ordering::Relaxed))
        .map(|document| (document, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::{ProjectId, SourcePath};
    use astrolith_document::DocumentIdentity;
    use gpui_kit::{
        Entity, TestAppContext, WindowHandle, WindowOptions, base::Root, test::TestWindowExt,
    };

    struct WriterHost {
        writer: Entity<WriterSurface>,
        focus: gpui_kit::FocusHandle,
    }

    impl Render for WriterHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .child(self.writer.clone())
        }
    }

    fn mount_writer(
        cx: &mut TestAppContext,
        source: &str,
    ) -> (WindowHandle<Root>, Entity<DocumentSession>) {
        let mounted = cx.update(|cx| {
            gpui_kit::init(cx);
            let mut session = None;
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let identity = DocumentIdentity::new(
                    ProjectId::new("window").unwrap(),
                    1,
                    SourcePath::new("src/posts/entry.md").unwrap(),
                );
                let created =
                    cx.new(|cx| DocumentSession::new(identity, source.to_owned(), window, cx));
                let writer = created.read(cx).writer.clone();
                session = Some(created);
                cx.new(|cx| WriterHost {
                    writer,
                    focus: cx.focus_handle(),
                })
            })
            .unwrap();
            (
                handle.downcast::<Root>().unwrap(),
                session.expect("the writer session must exist"),
            )
        });
        cx.run_until_parked();
        mounted
    }

    #[gpui_kit::test]
    fn parsing_coalesces_revisions_and_never_installs_an_obsolete_snapshot(
        cx: &mut TestAppContext,
    ) {
        let (_, session) = mount_writer(cx, "# Original\n");
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        let seen = Rc::new(RefCell::new(Vec::new()));
        let observed = seen.clone();
        let _subscription = cx.update(|cx| {
            cx.observe(&writer, move |writer, cx| {
                observed
                    .borrow_mut()
                    .push(writer.read(cx).document_revision);
            })
        });
        writer.update(cx, |writer, cx| {
            writer.set_snapshot(100, Rope::from_str(&"Old paragraph.\n\n".repeat(2_000)), cx);
            writer.set_snapshot(101, Rope::from_str("intermediate に"), cx);
            writer.set_snapshot(102, Rope::from_str("# New 日本語🦀\n"), cx);
            assert_eq!(
                writer.parsed_text.as_str(),
                "# Original\n",
                "input does not synchronously parse"
            );
        });
        cx.run_until_parked();
        writer.read_with(cx, |writer, _| {
            assert_eq!(writer.document_revision, Some(102));
            assert_eq!(writer.document.blocks[0].text.text, "New 日本語🦀");
            assert!(writer.parse_task.is_none() && writer.pending_snapshot.is_none());
        });
        assert_eq!(*seen.borrow(), [Some(102)]);
    }

    #[gpui_kit::test]
    fn writer_click_keeps_native_input_focus_inside_a_focusable_panel(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, "# Heading\n\nEditable paragraph.\n");
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        let before = editor.read_with(cx, |editor, _| editor.value().len());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("writer-wysiwyg-content", cx);
            assert!(editor.read(cx).focus_handle(cx).is_focused(window));
            window.input("中", cx);
        })
        .unwrap();
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.value().len()),
            before + "中".len()
        );
        session.read_with(cx, |session, _| {
            assert!(session.buffer.current_text().contains('中'));
        });
    }

    #[gpui_kit::test]
    fn published_command_adapter_deletes_one_grapheme_and_undo_preserves_source(
        cx: &mut TestAppContext,
    ) {
        let source = "---\ntitle: Exact\n---\nHello 👩🏽‍💻";
        let (handle, session) = mount_writer(cx, source);
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                let end = session.projection.visible.len();
                session.set_writer_selection(end, end, window, cx);
            });
            window.render_frame(cx);
            window.press("backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.buffer.current_text(),
                "---\ntitle: Exact\n---\nHello "
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-z"
                } else {
                    "ctrl-z"
                },
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), source)
        });
    }

    #[gpui_kit::test]
    fn writer_native_composition_uses_visible_geometry_and_keeps_hidden_source(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{ElementInputHandler, EntityInputHandler, test::TestInput};
        let source = "---\ntitle: Exact\n---\n# Hello\n";
        let (handle, session) = mount_writer(cx, source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            session.update(cx, |session, cx| {
                session.set_writer_selection(2, 2, window, cx)
            });
            window.render_frame(cx);
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let caret = writer.update(cx, |writer, cx| {
                writer.bounds_for_range(2..2, bounds, window, cx)
            });
            assert!(
                caret.is_some(),
                "IME candidate geometry comes from the visible heading"
            );
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, writer.clone()), window);
            input.compose_text(None, "に🦀", Some(3..3), cx);
            assert_eq!(input.marked_text_range(cx), Some(2..5));
            drop(input);
            session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                assert_eq!(
                    session.buffer.current_text(),
                    source,
                    "save and recovery see committed bytes only"
                );
            });
        })
        .unwrap();
        cx.run_until_parked();
        writer.read_with(cx, |writer, _| {
            assert!(
                writer.parsed_text.contains("に🦀"),
                "preedit is rendered before commit"
            )
        });
        cx.update_window(handle.into(), |_, window, cx| {
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, writer.clone()), window);
            input.compose_text(None, "日本", Some(2..2), cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, writer.clone()), window);
            input.commit_text("日本語", cx);
            assert_eq!(input.marked_text_range(cx), None);
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.buffer.current_text(),
                "---\ntitle: Exact\n---\n# 日本語Hello\n"
            )
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.undo_source(window, cx);
            });
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.buffer.current_text(),
                source,
                "one undo removes the whole composition"
            )
        });
    }

    #[gpui_kit::test]
    fn unmarking_commits_preedit_without_a_replacement(cx: &mut TestAppContext) {
        use gpui_kit::{ElementInputHandler, test::TestInput};

        let (handle, session) = mount_writer(cx, "Hello");
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let mut input = TestInput::new(ElementInputHandler::new(bounds, writer), window);
            input.compose_text(None, "你", Some(1..1), cx);
            input.unmark_text(cx);
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), "你Hello");
        });
    }

    #[gpui_kit::test]
    fn cancelling_preedit_restores_the_replaced_selection(cx: &mut TestAppContext) {
        use gpui_kit::{ElementInputHandler, test::TestInput};

        let source = "# Hello\n";
        let (handle, session) = mount_writer(cx, source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.set_writer_selection(2, 7, window, cx)
            });
            window.render_frame(cx);
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, writer.clone()), window);
            input.compose_text(None, "に🦀", Some(3..3), cx);
            input.compose_text(None, "", None, cx);
            assert_eq!(input.marked_text_range(cx), None);
            assert_eq!(input.selected_text_range(cx).unwrap().range, 2..7);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.sync_buffer(cx);
                assert_eq!(session.buffer.current_text(), source);
                assert_eq!(session.editor.read(cx).value().as_str(), source);
                assert!(
                    !session.undo_source(window, cx),
                    "cancel does not add source history"
                );
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn short_writer_reconciles_width_on_the_next_frame(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, "Short paragraph.");
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        for width in [900., 1500., 900.] {
            cx.simulate_window_resize(handle.into(), size(px(width), px(700.)));
            cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                assert!(
                    window.simulate_next_frame(cx) > 0,
                    "measured width changes must schedule reconciliation"
                );
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
            writer.read_with(cx, |writer, _| {
                assert!(
                    (writer.laid_out_width.unwrap() - writer.geometry.borrow().width).abs() <= 0.5,
                    "short content must resize without waiting for typing or caret blink"
                );
            });
        }
    }

    #[gpui_kit::test]
    fn long_paragraph_keeps_offscreen_caret_geometry_after_resize(cx: &mut TestAppContext) {
        let source = "中文日本語🦀 **bold** `code` ".repeat(1_000);
        let (handle, session) = mount_writer(cx, &source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        let end = source.len();
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.set_writer_selection(end, end, window, cx)
            });
            for _ in 0..80 {
                window.render_frame(cx);
            }
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert!(writer.scroll.offset().y < px(-1_000.));
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            let (block, rendered) = writer.document.caret_location(end).unwrap();
            let caret = layout.caret(Hit { block, rendered }).unwrap();
            let geometry = writer.geometry.borrow();
            assert!(layout.total_height() > px(1_000.).into());
            assert!(geometry.viewport.contains(&point(
                geometry.origin.x + caret.origin.x,
                geometry.origin.y + caret.origin.y
            )));
        });
        cx.simulate_window_resize(handle.into(), size(px(900.), px(700.)));
        cx.update_window(handle.into(), |_, window, cx| {
            for _ in 0..80 {
                window.render_frame(cx);
            }
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            let (block, rendered) = writer.document.caret_location(end).unwrap();
            assert!(
                layout.caret(Hit { block, rendered }).is_some(),
                "the end caret keeps geometry after a resize"
            );
        });
    }

    #[gpui_kit::test]
    fn focus_mode_follows_sentences_within_the_same_block(cx: &mut TestAppContext) {
        let source = "**第一句。**第二句！\n\nThird sentence.";
        let (handle, session) = mount_writer(cx, source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.set_writing_modes(
                    crate::editor::writing_modes::WritingModes {
                        focus: true,
                        typewriter: false,
                    },
                    window,
                    cx,
                )
            });
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert_eq!(
                writer.last_focus_sentence,
                Some(FocusSentence {
                    block: 0,
                    range: 0.."第一句。".len()
                })
            );
        });
        let second = source.find("第二句").unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.set_writer_selection(second, second, window, cx)
            });
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert_eq!(
                writer.last_focus_sentence,
                Some(FocusSentence {
                    block: 0,
                    range: "第一句。".len().."第一句。第二句！".len()
                })
            );
        });
    }

    #[gpui_kit::test]
    fn enabling_typewriter_centres_the_visible_caret_without_moving_it(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(100));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        let offset = writer.read_with(cx, |writer, _| {
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            let scroll = (-f32::from(writer.scroll.offset().y)).max(0.);
            let viewport = f32::from(writer.geometry.borrow().viewport.size.height);
            let block = layout
                .block_at_y(scroll + viewport * 0.6)
                .expect("a block below the centre");
            writer.document.blocks[block].content.start
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session
                    .editor
                    .update(cx, |editor, cx| window.focus(&editor.focus_handle(cx), cx));
                session.set_writer_selection(offset, offset, window, cx);
            });
            window.render_frame(cx);
            session.update(cx, |session, cx| {
                session.set_writing_modes(
                    crate::editor::writing_modes::WritingModes {
                        focus: false,
                        typewriter: true,
                    },
                    window,
                    cx,
                )
            });
            for _ in 0..4 {
                window.render_frame(cx);
            }
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            let (block, rendered) = writer.document.caret_location(offset).unwrap();
            let caret = layout.caret(Hit { block, rendered }).unwrap();
            let scroll = (-f32::from(writer.scroll.offset().y)).max(0.);
            let viewport = f32::from(writer.geometry.borrow().viewport.size.height);
            let centre = f32::from(caret.origin.y) - scroll + f32::from(caret.height) / 2.;
            assert!((centre - viewport / 2.).abs() < 3.);
        });
        assert_eq!(
            session.read_with(cx, |session, cx| session.editor.read(cx).cursor()),
            offset
        );
    }

    #[gpui_kit::test]
    fn writer_scrolling_does_not_snap_back_to_an_unchanged_caret(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(2_000));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        writer.update(cx, |writer, cx| {
            writer.scroll.set_offset(point(px(0.), px(-20_000.)));
            cx.notify();
        });
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        writer.read_with(cx, |writer, _| {
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            let scroll = (-f32::from(writer.scroll.offset().y)).max(0.);
            assert!(layout.block_at_y(scroll).unwrap() > 0);
            assert!(writer.scroll.offset().y < px(-1_000.));
        });
    }

    fn window_source(paragraphs: usize) -> String {
        let mut source = String::new();
        for index in 0..paragraphs {
            source.push_str(&format!(
                "Paragraph {index} with a handful of ordinary words for the window test.\n\n"
            ));
        }
        source
    }

    #[gpui_kit::test]
    fn writer_windows_from_the_first_layout_and_keeps_the_caret_block(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(2_000));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        let total = writer.read_with(cx, |writer, _| {
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            assert!(
                layout.measured_rows() < writer.document.blocks.len(),
                "the first frames wrap a viewport, not every block"
            );
            assert_eq!(layout.block_at_y(0.), Some(0));
            writer.document.blocks.len()
        });
        assert!(total > 1_000);

        // A local edit reuses the untouched blocks and keeps the frame
        // windowed instead of wrapping the whole document.
        cx.update_window(handle.into(), |_, window, cx| {
            let editor = session.read_with(cx, |session, _| session.editor.clone());
            editor.update(cx, |editor, cx| {
                let mut value = editor.value().to_string();
                value.push_str("A trailing sentence after the edit.\n\n");
                editor.set_value(value, window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            assert!(
                layout.measured_rows() < writer.document.blocks.len(),
                "an edit must not fall back to a full wrap"
            );
        });
    }

    #[gpui_kit::test]
    fn scrolling_keeps_the_caret_block_and_hit_testing_aligned(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(2_000));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        let target = 1_000;
        let offset = writer.read_with(cx, |writer, _| writer.document.blocks[target].content.start);
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.set_writer_selection(offset, offset, window, cx);
            });
            for _ in 0..12 {
                window.render_frame(cx);
            }
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert!(writer.scroll.offset().y < px(-1_000.));
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            let (block, rendered) = writer.document.caret_location(offset).unwrap();
            let hit = Hit { block, rendered };
            let caret = layout.caret(hit).expect("the caret row is prepared");
            let geometry = writer.geometry.borrow();
            let position = point(
                geometry.origin.x + caret.origin.x,
                geometry.origin.y + caret.origin.y,
            );
            assert!(geometry.viewport.contains(&position));
            assert!(
                layout
                    .hit_test(point(caret.origin.x, caret.origin.y))
                    .is_some()
            );
            let source = writer
                .source_at(position)
                .expect("hit testing maps the caret row");
            let block = writer.document.blocks[block].source.clone();
            assert!(
                block.start <= source && source <= block.end,
                "hit {source} must land in the caret block {block:?}"
            );
        });
    }

    #[gpui_kit::test]
    fn unresolved_drag_hit_does_not_extend_selection_to_eof(cx: &mut TestAppContext) {
        let source = window_source(2_000);
        let (handle, session) = mount_writer(cx, &source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let (position, viewport) = writer.read_with(cx, |writer, _| {
                let geometry = *writer.geometry.borrow();
                (
                    point(
                        geometry.origin.x + px(16.),
                        geometry.viewport.bottom() + px(500.),
                    ),
                    geometry.viewport,
                )
            });
            assert!(viewport.size.height > px(0.));
            assert_eq!(
                writer.read_with(cx, |writer, _| writer.source_at(position)),
                None
            );

            writer.update(cx, |writer, cx| {
                writer.select(position, false, window, cx);
            });
            assert_eq!(
                session.read(cx).editor.read(cx).selected_range(),
                0..0,
                "a missed mouse-down must not place the caret at EOF"
            );

            writer.update(cx, |writer, cx| {
                writer.drag_anchor = Some(0);
                writer.select(position, true, window, cx);
            });
            assert_eq!(
                session.read(cx).editor.read(cx).selected_range(),
                0..0,
                "an unresolved drag position must leave the selection unchanged"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn dragging_to_the_viewport_edge_scrolls_and_extends_selection(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(2_000));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        let (start, edge, before) = writer.read_with(cx, |writer, _| {
            let geometry = *writer.geometry.borrow();
            let layout = writer.layout.borrow();
            let caret = layout
                .as_ref()
                .unwrap()
                .caret(Hit {
                    block: 0,
                    rendered: 0,
                })
                .unwrap();
            (
                point(
                    geometry.origin.x + caret.origin.x + px(1.),
                    geometry.origin.y + caret.origin.y + caret.height * 0.5,
                ),
                point(
                    geometry.origin.x + caret.origin.x + px(140.),
                    geometry.viewport.bottom() - px(2.),
                ),
                writer.scroll.offset(),
            )
        });

        let mut visual = gpui_kit::VisualTestContext::from_window(handle.into(), cx);
        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(edge, MouseButton::Left, Default::default());
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(64));
        visual.run_until_parked();
        visual.simulate_mouse_up(edge, MouseButton::Left, Default::default());

        let (after, selection) = writer.read_with(cx, |writer, cx| {
            (
                writer.scroll.offset(),
                session.read(cx).editor.read(cx).selected_range(),
            )
        });
        assert!(after.y < before.y, "dragging at the edge scrolls down");
        assert!(selection.end > 0, "selection extends beyond its start");
    }

    #[test]
    fn scroll_offsets_stay_between_the_top_and_the_positive_max_distance() {
        assert_eq!(clamp_scroll_offset(120., 3219.), 0.);
        assert_eq!(clamp_scroll_offset(-500., 3219.), -500.);
        assert_eq!(clamp_scroll_offset(-9000., 3219.), -3219.);
        assert_eq!(clamp_scroll_offset(12., 0.), 0.);
    }

    #[gpui_kit::test]
    fn caret_blink_only_repaints_the_overlay_after_the_first_layout(cx: &mut TestAppContext) {
        let source = "A **bold** and `code` paragraph with a link.\n\n".repeat(40);
        let (handle, session) = mount_writer(cx, &source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("writer-wysiwyg-content", cx);
            window.render_frame(cx);
        })
        .unwrap();
        let (builds, rows) = writer.read_with(cx, |writer, cx| {
            assert!(writer.caret.read(cx).blinks == 0);
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            (writer.layout_builds, layout.measured_rows())
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1_100));
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        writer.read_with(cx, |writer, cx| {
            assert!(
                writer.caret.read(cx).blinks >= 2,
                "the caret timer toggled the overlay"
            );
            let layout = writer.layout.borrow();
            let layout = layout.as_ref().unwrap();
            assert_eq!(
                (writer.layout_builds, layout.measured_rows()),
                (builds, rows),
                "blinking must not rebuild or re-wrap the layout"
            );
        });
    }
}
