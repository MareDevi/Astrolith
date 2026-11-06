//! Viewport layout for the complete-source view.
//!
//! Source lines are laid out vertically and shaped only inside the visible
//! horizontal window. A line is measured in bounded grapheme-aligned chunks, so
//! one 1 MiB source line does not become one 1 MiB shaping call. The shaped
//! window keeps a short context prefix so ligatures and joining survive its
//! left edge, and painting offsets glyphs by the window start.
//!
//! Geometry comes from the same shaped window that paints: hit-test, caret,
//! selection and IME bounds are `None` until that window exists. Unresolved
//! ranges fail closed instead of estimating a caret position.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, Bounds, Font, Hsla, Pixels, Point, ShapedLine, SharedString, TextAlign, TextRun, Window,
    WindowTextSystem, fill, font, point, px, size,
};
use unicode_segmentation::GraphemeCursor;

/// Work allowed for one prepared source viewport.
pub(crate) const SOURCE_LAYOUT_BUDGET: Duration = Duration::from_millis(4);
/// Bytes measured per horizontal chunk. Small enough to keep one shaping call
/// well under a frame even with complex scripts.
const CHUNK_BYTES: usize = 4 * 1024;
/// Bytes of preceding text shaped as context across a window edge.
const WINDOW_CONTEXT_BYTES: usize = 24;
/// Lines kept measured outside the viewport before their chunks are dropped.
const LINE_RETENTION_SCREENS: usize = 2;

/// Presentation used by the source layout.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SourceStyle {
    pub(crate) font: Font,
    pub(crate) font_size: Pixels,
    pub(crate) line_height: f32,
    pub(crate) foreground: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) caret: Hsla,
}

impl SourceStyle {
    pub(crate) fn from_theme(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            font: font(theme.mono_font_family.clone()),
            font_size: theme.mono_font_size,
            line_height: 1.5,
            foreground: theme.foreground,
            selection: theme.selection,
            caret: theme.caret,
        }
    }

    pub(crate) fn row_height(&self) -> f32 {
        f32::from(self.font_size) * self.line_height
    }

    fn metrics_eq(&self, other: &Self) -> bool {
        self.font == other.font
            && self.font_size == other.font_size
            && self.line_height == other.line_height
    }
}

/// Resolves syntax runs for a source byte range.
pub(crate) trait SourceRuns {
    fn runs(&self, range: Range<usize>) -> Vec<TextRun>;
}

/// Plain, unhighlighted runs.
#[cfg(test)]
pub(crate) struct PlainSourceRuns {
    pub(crate) style: Arc<SourceStyle>,
}

#[cfg(test)]
impl SourceRuns for PlainSourceRuns {
    fn runs(&self, range: Range<usize>) -> Vec<TextRun> {
        if range.is_empty() {
            return Vec::new();
        }
        vec![plain_run(&self.style, range.len())]
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

/// A viewport in content pixels, excluding the line-number gutter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SourceViewport {
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// The result of preparing one source viewport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourcePrepared {
    pub(crate) lines: Range<usize>,
    pub(crate) pending: bool,
}

/// Per-preparation work, for the opt-in performance harness.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SourceStats {
    pub(crate) measured_chunks: usize,
    pub(crate) measured_bytes: usize,
    pub(crate) shaped_lines: usize,
    pub(crate) shaped_bytes: usize,
    pub(crate) reused_lines: usize,
    pub(crate) pending: bool,
}

#[derive(Clone, Debug)]
struct Chunk {
    start: usize,
    end: usize,
    width: f32,
}

#[derive(Clone, Debug)]
struct ShapedWindow {
    /// Absolute byte offset the painted slice starts at.
    start: usize,
    /// Absolute byte offset the shaped slice ends at.
    end: usize,
    /// Content x of `start`.
    x: f32,
    /// Shaped from `context_start` through `end`, split at `start`.
    line: ShapedLine,
}

#[derive(Clone, Debug)]
struct LineLayout {
    chunks: Vec<Chunk>,
    measured_bytes: usize,
    complete: bool,
    window: Option<ShapedWindow>,
}

impl LineLayout {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            measured_bytes: 0,
            complete: false,
            window: None,
        }
    }

    fn measured_width(&self) -> f32 {
        self.chunks.iter().map(|chunk| chunk.width).sum()
    }
}

/// Source text laid out into vertical lines and horizontal shaping windows.
pub(crate) struct SourceLayout {
    text: SharedString,
    /// Byte offset of every line start, with `text.len()` as the sentinel.
    line_starts: Vec<usize>,
    lines: HashMap<usize, LineLayout>,
    style: Arc<SourceStyle>,
    width: f32,
    stats: SourceStats,
    /// Estimated widest line, for the horizontal scroll range.
    total_width: f32,
}

impl SourceLayout {
    pub(crate) fn new(text: SharedString, style: Arc<SourceStyle>, width: f32) -> Self {
        let mut this = Self {
            text: SharedString::default(),
            line_starts: Vec::new(),
            lines: HashMap::new(),
            style,
            width,
            stats: SourceStats::default(),
            total_width: 0.,
        };
        this.set_text(text);
        this
    }

    pub(crate) fn style(&self) -> &Arc<SourceStyle> {
        &self.style
    }

    pub(crate) fn stats(&self) -> SourceStats {
        self.stats
    }

    pub(crate) fn line_count(&self) -> usize {
        self.line_starts.len().saturating_sub(1)
    }

    pub(crate) fn row_height(&self) -> f32 {
        self.style.row_height()
    }

    pub(crate) fn total_height(&self) -> f32 {
        self.line_count() as f32 * self.row_height()
    }

    pub(crate) fn total_width(&self) -> f32 {
        self.total_width.max(self.width)
    }

    pub(crate) fn set_width(&mut self, width: f32) {
        if (self.width - width).abs() < 0.5 {
            return;
        }
        self.width = width;
        // Chunk widths do not depend on the viewport width; shaped windows do.
        for line in self.lines.values_mut() {
            line.window = None;
        }
    }

    pub(crate) fn set_style(&mut self, style: Arc<SourceStyle>) {
        if *self.style == *style {
            return;
        }
        if !self.style.metrics_eq(&style) {
            // Metrics changed: measured chunk widths and shaped lines are stale.
            self.lines.clear();
            self.total_width = 0.;
        } else {
            for line in self.lines.values_mut() {
                line.window = None;
            }
        }
        self.style = style;
    }

    /// Replace the source text, keeping measurements for lines that did not
    /// change. Line starts are rebuilt in one scan; shaping is invalidated only
    /// for the edited line range.
    pub(crate) fn set_text(&mut self, text: SharedString) {
        if self.text == text {
            return;
        }
        let old = std::mem::take(&mut self.text);
        let old_starts = std::mem::take(&mut self.line_starts);
        self.line_starts = line_starts(&text);
        self.text = text;
        if old_starts.is_empty() {
            self.lines.clear();
            return;
        }
        let (prefix, suffix) = common_affixes(&old, &self.text);
        let first_old = old_starts
            .partition_point(|start| *start <= prefix)
            .saturating_sub(1);
        let last_old = old_starts
            .partition_point(|start| *start < old.len().saturating_sub(suffix))
            .saturating_sub(1);
        let last_new = self
            .line_starts
            .partition_point(|start| *start < self.text.len().saturating_sub(suffix))
            .saturating_sub(1);
        let first = first_old;
        let end = last_old.max(last_new);
        // Simple bound: when the edited region spans lines, drop every touched
        // line. Diffs far from the edit keep their measured chunks.
        for index in first..=end {
            self.lines.remove(&index);
        }
    }

    /// Prepare the visible lines and their horizontal shaping windows.
    pub(crate) fn prepare(
        &mut self,
        text_system: &WindowTextSystem,
        viewport: SourceViewport,
        caret: Option<usize>,
        runs: &dyn SourceRuns,
    ) -> SourcePrepared {
        self.stats = SourceStats::default();
        let started = Instant::now();
        let deadline = started + SOURCE_LAYOUT_BUDGET;
        let row_height = self.row_height().max(1.);
        let overscan = viewport.height * 0.5;
        let top = (viewport.scroll_y - overscan).max(0.);
        let bottom = viewport.scroll_y + viewport.height + overscan;
        let first = ((top / row_height).floor() as usize).min(self.line_count());
        let last = ((bottom / row_height).ceil() as usize + 1).min(self.line_count());
        let mut pending = false;

        if let Some(caret) = caret {
            let line = self.line_of(caret);
            if line < first || line >= last {
                self.prepare_caret_line(
                    line,
                    caret,
                    text_system,
                    viewport.width,
                    runs,
                    deadline,
                    &mut pending,
                );
            }
        }

        self.prune(first, last);
        for line in first..last {
            if Instant::now() >= deadline {
                pending = true;
                break;
            }
            self.prepare_line(
                line,
                text_system,
                viewport.scroll_x,
                viewport.width,
                runs,
                deadline,
                &mut pending,
            );
        }
        self.stats.pending = pending;
        self.update_total_width();
        SourcePrepared {
            lines: first..last.max(first),
            pending,
        }
    }

    /// Prepare the caret's line even when it is outside the viewport, so a
    /// jump to the end of a long line can reveal and shape that window.
    #[allow(clippy::too_many_arguments)]
    fn prepare_caret_line(
        &mut self,
        index: usize,
        caret: usize,
        text_system: &WindowTextSystem,
        width: f32,
        runs: &dyn SourceRuns,
        deadline: Instant,
        pending: &mut bool,
    ) {
        let Some(range) = self.display_range(index) else {
            *pending = true;
            return;
        };
        let style = self.style.clone();
        let text = self.text.clone();
        let state = self.lines.entry(index).or_insert_with(LineLayout::new);
        if state
            .window
            .as_ref()
            .is_some_and(|window| window.start <= caret && caret <= window.end)
        {
            return;
        }
        measure_chunks(
            &text,
            range.clone(),
            state,
            text_system,
            &style,
            f32::MAX,
            Some(caret),
            deadline,
            &mut self.stats,
        );
        let caret_chunk = state
            .chunks
            .iter()
            .position(|chunk| caret >= chunk.start && caret <= chunk.end);
        let Some(caret_chunk) = caret_chunk else {
            *pending = true;
            return;
        };
        if Instant::now() >= deadline {
            *pending = true;
            return;
        }
        let scroll_x = chunk_x(state, state.chunks[caret_chunk].start);
        if let Some(window) = shape_window(
            &text,
            range,
            state,
            text_system,
            &style,
            scroll_x,
            width,
            runs,
        ) {
            self.stats.shaped_lines += 1;
            self.stats.shaped_bytes += window.end - window.start;
            state.window = Some(window);
        } else {
            *pending = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_line(
        &mut self,
        index: usize,
        text_system: &WindowTextSystem,
        scroll_x: f32,
        width: f32,
        runs: &dyn SourceRuns,
        deadline: Instant,
        pending: &mut bool,
    ) {
        let Some(range) = self.display_range(index) else {
            *pending = true;
            return;
        };
        let target_x = scroll_x + width * 2.;
        let style = self.style.clone();
        let text = self.text.clone();
        let state = self.lines.entry(index).or_insert_with(LineLayout::new);
        if !state.complete && state.measured_width() < target_x {
            measure_chunks(
                &text,
                range.clone(),
                state,
                text_system,
                &style,
                target_x,
                None,
                deadline,
                &mut self.stats,
            );
            if !state.complete && state.measured_width() < target_x {
                *pending = true;
            }
        }
        if state
            .window
            .as_ref()
            .is_some_and(|window| window_covers(window, scroll_x, width))
        {
            self.stats.reused_lines += 1;
            return;
        }
        if Instant::now() >= deadline {
            // Keep any previously shaped window; without one the rows paint
            // nothing rather than a guessed slice, and the next frame retries.
            if state.window.is_none() {
                *pending = true;
            }
            return;
        }
        if let Some(window) = shape_window(
            &text,
            range,
            state,
            text_system,
            &style,
            scroll_x,
            width,
            runs,
        ) {
            self.stats.shaped_lines += 1;
            self.stats.shaped_bytes += window.end - window.start;
            state.window = Some(window);
        } else {
            state.window = None;
            *pending = true;
        }
    }

    fn prune(&mut self, first: usize, last: usize) {
        let margin = (last - first).max(1) * LINE_RETENTION_SCREENS;
        let keep = first.saturating_sub(margin)..last + margin;
        self.lines
            .retain(|index, _| *index >= keep.start && *index < keep.end);
    }

    fn update_total_width(&mut self) {
        let mut widest: f32 = self.width;
        for index in self.lines.keys().copied() {
            widest = widest.max(self.line_width(index));
        }
        self.total_width = widest;
    }

    /// Estimated width of a line: measured chunks plus an extrapolated tail.
    fn line_width(&self, index: usize) -> f32 {
        let Some(line) = self.lines.get(&index) else {
            return self.width;
        };
        let measured = line.measured_width();
        if line.complete || line.measured_bytes == 0 {
            return measured;
        }
        let Some(range) = self.display_range(index) else {
            return measured;
        };
        let remaining = range.len().saturating_sub(line.measured_bytes);
        let per_byte = measured / line.measured_bytes.max(1) as f32;
        measured + remaining as f32 * per_byte
    }

    pub(crate) fn line_range(&self, index: usize) -> Option<Range<usize>> {
        let start = *self.line_starts.get(index)?;
        let end = *self.line_starts.get(index + 1)?;
        Some(start..end)
    }

    /// Bytes of a line that are shaped and painted; the terminator is not.
    fn display_range(&self, index: usize) -> Option<Range<usize>> {
        let mut range = self.line_range(index)?;
        let bytes = self.text.as_bytes();
        if range.end > range.start && bytes.get(range.end - 1) == Some(&b'\n') {
            range.end -= 1;
            if range.end > range.start && bytes.get(range.end - 1) == Some(&b'\r') {
                range.end -= 1;
            }
        }
        Some(range)
    }

    pub(crate) fn line_of(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        self.line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
            .min(self.line_count().saturating_sub(1))
    }

    pub(crate) fn line_start(&self, index: usize) -> Option<usize> {
        self.line_starts.get(index).copied()
    }

    pub(crate) fn line_end(&self, index: usize) -> Option<usize> {
        self.line_starts.get(index + 1).copied()
    }

    /// Content position of a caret offset, if its window is shaped.
    pub(crate) fn caret(&self, offset: usize) -> Option<(Point<Pixels>, Pixels)> {
        let line = self.line_of(offset);
        let state = self.lines.get(&line)?;
        let window = state.window.as_ref()?;
        if !(window.start..=window.end).contains(&offset) {
            return None;
        }
        let x = window.x + f32::from(window.line.x_for_index(offset - window.start));
        let height = (window.line.ascent + window.line.descent).max(px(2.));
        Some((point(px(x), px(line as f32 * self.row_height())), height))
    }

    /// Source byte offset under a content-space point.
    pub(crate) fn hit_test(&self, position: Point<Pixels>) -> Option<usize> {
        let row_height = self.row_height();
        if row_height <= 0. {
            return None;
        }
        let line = (f32::from(position.y) / row_height).floor().max(0.) as usize;
        let state = self.lines.get(&line)?;
        let window = state.window.as_ref()?;
        let local_x = f32::from(position.x) - window.x;
        let index = window.line.closest_index_for_x(px(local_x));
        Some((window.start + index).min(window.end).min(self.text.len()))
    }

    /// Content-space rectangles covered by a source range.
    pub(crate) fn selection_quads(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut quads = Vec::new();
        if range.is_empty() {
            return quads;
        }
        let first_line = self.line_of(range.start);
        let last_line = self.line_of(range.end.saturating_sub(1));
        let row_height = self.row_height();
        for line in first_line..=last_line {
            let Some(state) = self.lines.get(&line) else {
                continue;
            };
            let Some(window) = state.window.as_ref() else {
                continue;
            };
            let Some(line_range) = self.line_range(line) else {
                continue;
            };
            let start = range.start.max(line_range.start).max(window.start);
            let end = range.end.min(line_range.end).min(window.end);
            if start >= end {
                continue;
            }
            let x0 = window.x + f32::from(window.line.x_for_index(start - window.start));
            let x1 = window.x + f32::from(window.line.x_for_index(end - window.start));
            quads.push(Bounds::new(
                point(px(x0.min(x1)), px(line as f32 * row_height)),
                size(px((x1 - x0).abs().max(1.)), px(row_height)),
            ));
        }
        quads
    }

    /// Paint the prepared source rows.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        origin: Point<Pixels>,
        viewport: SourceViewport,
        selection: Range<usize>,
        caret: Option<usize>,
        active_line: Option<usize>,
    ) -> usize {
        let clip = window.content_mask().bounds;
        let row_height = self.row_height();
        let content_top = viewport.scroll_y;
        let content_bottom = viewport.scroll_y + viewport.height;
        let first = ((content_top / row_height).floor() as usize).min(self.line_count());
        let last = ((content_bottom / row_height).ceil() as usize + 1).min(self.line_count());
        let quads = self.selection_quads(selection);
        for quad in &quads {
            let bounds = Bounds::new(
                point(origin.x + quad.origin.x, origin.y + quad.origin.y),
                quad.size,
            );
            let bounds = bounds.intersect(&clip);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                window.paint_quad(fill(bounds, self.style.selection));
            }
        }
        let mut painted = 0;
        for line in first..last {
            let y = origin.y + px(line as f32 * row_height);
            if active_line == Some(line) {
                let band =
                    Bounds::new(point(clip.left(), y), size(clip.size.width, px(row_height)));
                window.paint_quad(fill(band, self.style.selection.opacity(0.12)));
            }
            let Some(rendered) = self
                .lines
                .get(&line)
                .and_then(|state| state.window.as_ref())
            else {
                continue;
            };
            let x = origin.x + px(rendered.x);
            let _ = rendered.line.paint(
                point(x, y),
                px(row_height),
                TextAlign::Left,
                None,
                window,
                cx,
            );
            painted += 1;
        }
        if let Some(offset) = caret
            && let Some((position, height)) = self.caret(offset)
        {
            let bounds = Bounds::new(
                point(origin.x + position.x, origin.y + position.y),
                size(px(1.5), height),
            );
            let bounds = bounds.intersect(&clip);
            if bounds.size.height > px(0.) {
                window.paint_quad(fill(bounds, self.style.caret));
            }
        }
        painted
    }
}

fn window_covers(window: &ShapedWindow, scroll_x: f32, width: f32) -> bool {
    let left = window.x;
    let right = window.x + f32::from(window.line.width());
    left <= scroll_x + 1. && right >= scroll_x + width
}

#[allow(clippy::too_many_arguments)]
fn measure_chunks(
    text: &str,
    range: Range<usize>,
    line: &mut LineLayout,
    text_system: &WindowTextSystem,
    style: &SourceStyle,
    target_x: f32,
    stop_offset: Option<usize>,
    deadline: Instant,
    stats: &mut SourceStats,
) {
    if line.complete {
        return;
    }
    loop {
        if Instant::now() >= deadline && line.measured_bytes > 0 {
            return;
        }
        if stop_offset.is_some_and(|offset| {
            line.measured_bytes > 0 && range.start + line.measured_bytes >= offset
        }) {
            return;
        }
        if line.measured_width() >= target_x {
            return;
        }
        let start = range.start + line.measured_bytes;
        if start >= range.end {
            line.complete = true;
            return;
        }
        let mut end = (start + CHUNK_BYTES).min(range.end);
        end = grapheme_aligned_end(text, start, end);
        if end <= start {
            line.complete = true;
            return;
        }
        let width = measure_slice(text, start, end, range.start, text_system, style);
        stats.measured_chunks += 1;
        stats.measured_bytes += end - start;
        line.chunks.push(Chunk { start, end, width });
        line.measured_bytes = end - range.start;
    }
}

/// Move `candidate` to the grapheme boundary at or before it.
fn grapheme_aligned_end(text: &str, start: usize, mut candidate: usize) -> usize {
    if candidate >= text.len() {
        return text.len();
    }
    while candidate > start && !text.is_char_boundary(candidate) {
        candidate -= 1;
    }
    let mut cursor = GraphemeCursor::new(candidate, text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(true) => candidate,
        _ => match cursor.prev_boundary(text, 0) {
            Ok(Some(boundary)) if boundary >= start => boundary,
            _ => start,
        },
    }
}

/// Move a context offset back onto a grapheme boundary.
fn grapheme_aligned_start(text: &str, floor: usize, mut candidate: usize) -> usize {
    while candidate > floor && !text.is_char_boundary(candidate) {
        candidate -= 1;
    }
    let mut cursor = GraphemeCursor::new(candidate, text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(true) => candidate,
        _ => match cursor.prev_boundary(text, 0) {
            Ok(Some(boundary)) if boundary >= floor => boundary,
            _ => floor,
        },
    }
}

fn measure_slice(
    text: &str,
    start: usize,
    end: usize,
    line_start: usize,
    text_system: &WindowTextSystem,
    style: &SourceStyle,
) -> f32 {
    let context = grapheme_aligned_start(
        text,
        line_start,
        start.saturating_sub(WINDOW_CONTEXT_BYTES).max(line_start),
    );
    let slice = &text[context..end];
    let len = slice.len();
    let shaped = text_system.shape_line(
        SharedString::from(slice.to_owned()),
        style.font_size,
        &[plain_run(style, len)],
        None,
    );
    let x_end = f32::from(shaped.x_for_index(end - context));
    let x_start = f32::from(shaped.x_for_index(start - context));
    (x_end - x_start).max(0.)
}

#[allow(clippy::too_many_arguments)]
fn shape_window(
    text: &str,
    range: Range<usize>,
    line: &LineLayout,
    text_system: &WindowTextSystem,
    style: &SourceStyle,
    scroll_x: f32,
    width: f32,
    runs: &dyn SourceRuns,
) -> Option<ShapedWindow> {
    if line.chunks.is_empty() {
        if !line.complete {
            return None;
        }
        let shaped = text_system.shape_line(SharedString::default(), style.font_size, &[], None);
        return Some(ShapedWindow {
            start: range.start,
            end: range.start,
            x: 0.,
            line: shaped,
        });
    }
    let start_chunk = line
        .chunks
        .iter()
        .rposition(|chunk| chunk_x(line, chunk.start) <= scroll_x + 1.)
        .unwrap_or(0);
    let start = line.chunks.get(start_chunk)?.start;
    let needed = scroll_x + width;
    let end = line
        .chunks
        .iter()
        .find(|chunk| chunk_x(line, chunk.end) >= needed)
        .map(|chunk| chunk.end)
        .unwrap_or_else(|| line.chunks.last().map_or(start, |chunk| chunk.end));
    let context_start = grapheme_aligned_start(
        text,
        range.start,
        start.saturating_sub(WINDOW_CONTEXT_BYTES).max(range.start),
    );
    let shaped = text_system.shape_line(
        SharedString::from(text[context_start..end].to_owned()),
        style.font_size,
        &runs.runs(context_start..end),
        None,
    );
    let (prefix, suffix) = shaped.split_at(start - context_start);
    let x = f32::from(prefix.width());
    Some(ShapedWindow {
        start,
        end,
        x,
        line: suffix,
    })
}

fn chunk_x(line: &LineLayout, offset: usize) -> f32 {
    let mut x = 0.;
    for chunk in &line.chunks {
        if chunk.start >= offset {
            break;
        }
        x += chunk.width;
    }
    x
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(text.len() / 32 + 2);
    starts.push(0);
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(offset + 1);
        }
    }
    starts.push(text.len());
    starts
}

fn common_affixes(old: &str, new: &str) -> (usize, usize) {
    let mut prefix = 0;
    for (left, right) in old.bytes().zip(new.bytes()) {
        if left != right {
            break;
        }
        prefix += 1;
    }
    while prefix > 0 && (!old.is_char_boundary(prefix) || !new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let mut suffix = 0;
    let max = old
        .len()
        .saturating_sub(prefix)
        .min(new.len().saturating_sub(prefix));
    for (left, right) in old.bytes().rev().zip(new.bytes().rev()) {
        if left != right || suffix >= max {
            break;
        }
        suffix += 1;
    }
    while suffix > 0
        && (!old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }
    (prefix, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use std::sync::Arc;
    use unicode_segmentation::UnicodeSegmentation;

    fn test_style() -> Arc<SourceStyle> {
        Arc::new(SourceStyle {
            font: font(".SystemUIFont"),
            font_size: px(14.),
            line_height: 1.5,
            foreground: gpui_kit::black(),
            selection: gpui_kit::black(),
            caret: gpui_kit::black(),
        })
    }

    fn prepare(
        layout: &mut SourceLayout,
        text_system: &WindowTextSystem,
        viewport: SourceViewport,
        caret: Option<usize>,
    ) -> SourcePrepared {
        let runs = PlainSourceRuns {
            style: layout.style().clone(),
        };
        layout.prepare(text_system, viewport, caret, &runs)
    }

    #[gpui_kit::test]
    fn one_mib_line_shapes_only_the_visible_window(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let text_system = WindowTextSystem::new(cx.text_system().clone());
            let source = "x".repeat(1024 * 1024);
            let mut layout = SourceLayout::new(SharedString::from(source), test_style(), 800.);
            prepare(
                &mut layout,
                &text_system,
                SourceViewport {
                    scroll_x: 0.,
                    scroll_y: 0.,
                    width: 800.,
                    height: 600.,
                },
                None,
            );
            let stats = layout.stats();
            assert!(stats.shaped_lines >= 1, "a visible window is shaped");
            assert!(
                stats.shaped_bytes < 32 * 1024,
                "shaping stops at the viewport: {} bytes",
                stats.shaped_bytes
            );
            assert!(
                stats.measured_bytes < 64 * 1024,
                "measurement stops near the window: {} bytes",
                stats.measured_bytes
            );
            let (caret, _) = layout.caret(0).expect("visible caret is shaped");
            assert_eq!(caret.x, px(0.));
        });
    }

    #[gpui_kit::test]
    fn horizontal_scroll_shapes_a_later_window(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let text_system = WindowTextSystem::new(cx.text_system().clone());
            let source = "abcdefgh".repeat(1024 * 16);
            let mut layout =
                SourceLayout::new(SharedString::from(source.clone()), test_style(), 800.);
            prepare(
                &mut layout,
                &text_system,
                SourceViewport {
                    scroll_x: 0.,
                    scroll_y: 0.,
                    width: 800.,
                    height: 600.,
                },
                None,
            );
            let scroll_x = 20_000.;
            prepare(
                &mut layout,
                &text_system,
                SourceViewport {
                    scroll_x,
                    scroll_y: 0.,
                    width: 800.,
                    height: 600.,
                },
                None,
            );
            let stats = layout.stats();
            assert!(
                stats.shaped_bytes < 64 * 1024,
                "a far horizontal window stays bounded: {} bytes",
                stats.shaped_bytes
            );
            assert!(
                stats.measured_bytes < 128 * 1024,
                "measurement advances only toward the window: {} bytes",
                stats.measured_bytes
            );
            let hit = layout
                .hit_test(Point::new(px(10.), px(10.)))
                .expect("the scrolled window is shaped");
            assert!(hit > 0, "hit-test reads inside the scrolled window");
            assert!(source.is_char_boundary(hit));
        });
    }

    #[gpui_kit::test]
    fn chunk_and_window_edges_do_not_split_graphemes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let text_system = WindowTextSystem::new(cx.text_system().clone());
            // Combining marks, ZWJ emoji and flags repeat across chunk seams.
            let unit = "e\u{301}👩‍💻🇨🇳";
            let mut source = String::new();
            while source.len() < 64 * 1024 {
                source.push_str(unit);
            }
            let mut layout =
                SourceLayout::new(SharedString::from(source.clone()), test_style(), 160.);
            prepare(
                &mut layout,
                &text_system,
                SourceViewport {
                    scroll_x: 0.,
                    scroll_y: 0.,
                    width: 160.,
                    height: 400.,
                },
                None,
            );
            let line = layout.lines.get(&0).expect("line measured");
            let boundaries: std::collections::HashSet<usize> = source
                .grapheme_indices(true)
                .map(|(offset, _)| offset)
                .chain(std::iter::once(source.len()))
                .collect();
            for chunk in &line.chunks {
                assert!(source.is_char_boundary(chunk.start));
                assert!(source.is_char_boundary(chunk.end));
                assert!(
                    boundaries.contains(&chunk.end),
                    "a chunk ends on a grapheme boundary: start={} end={}",
                    chunk.start,
                    chunk.end
                );
            }
            let window = line.window.as_ref().expect("window shaped");
            assert!(source.is_char_boundary(window.start));
            assert!(source.is_char_boundary(window.end));
        });
    }

    #[test]
    fn line_index_keeps_the_trailing_empty_line() {
        let starts = line_starts("a\nb\n");
        assert_eq!(starts, vec![0, 2, 4, 4]);
        let layout = SourceLayout::new(SharedString::from("a\nb\n"), test_style(), 100.);
        assert_eq!(layout.line_count(), 3);
        assert_eq!(layout.line_of(0), 0);
        assert_eq!(layout.line_of(2), 1);
        assert_eq!(layout.line_of(4), 2);
        assert_eq!(layout.line_range(2), Some(4..4));
    }
}
