//! Application-owned viewport text layout.
//!
//! Writing and Preview render through this layer. It wraps the rendered writer
//! text into visual rows with GPUI's own text system, shapes only the rows that
//! intersect the viewport plus overscan, and answers hit-test, caret, selection
//! and IME geometry from the same row snapshot. It does not go through the Kit
//! Inline retained-layout table or the Kit `TextView`.
//!
//! Contracts:
//!
//! - Wrapping works on rendered bytes only; it never rewrites source or
//!   inserts hard line breaks.
//! - A row always starts and ends on a grapheme boundary at a legal line-break
//!   opportunity (or a mandatory newline), so shaping never starts mid-cluster.
//! - Wrapping is resumable: a block that is not fully wrapped stores the row in
//!   progress, so a long block is measured in bounded slices across frames.
//! - Rows outside the viewport are wrapped but not shaped; glyph geometry and
//!   painted lines exist only for the prepared viewport.
//! - Unresolved ranges fail closed: hit-test, caret and IME queries return
//!   `None` instead of inventing geometry from an estimate.
//! - Block positions come from a height index that is updated locally, never
//!   rebuilt from every block each frame.

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use astrolith_document::{
    BlockKind, ColumnAlign, InlineMark, InlineMarkKind, RichBlock, RichText, WysiwygDocument,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, Bounds, Font, FontStyle, FontWeight, Hsla, Pixels, Point, ShapedLine, SharedString,
    StrikethroughStyle, TextAlign, TextRun, Window, WindowTextSystem, fill, font, point, px, size,
};
use unicode_segmentation::UnicodeSegmentation;

pub(crate) mod source;
#[cfg(test)]
mod tests;

/// Work allowed for one prepared viewport.
pub(crate) const LAYOUT_BUDGET: Duration = Duration::from_millis(4);
/// Share of the budget spent discovering rows before glyphs get shaped.
const WRAP_BUDGET_SHARE: f32 = 0.5;
/// Graphemes measured between two budget checks.
const BUDGET_CHECK_GRAPHEMES: usize = 16;
/// Bytes of preceding text shaped as context so joining and ligatures survive
/// a row boundary.
const LINE_CONTEXT_BYTES: usize = 24;
/// Bytes of line-break opportunities computed at once.
const BREAK_CHUNK_BYTES: usize = 16 * 1024;
/// Shaped rows kept outside the viewport before their glyphs are dropped.
const SHAPE_RETENTION_SCREENS: f32 = 2.;
/// Multiplier converting the byte length of one visual row into a fallback
/// estimate before any row of a block has been measured.
const FALLBACK_BYTES_PER_GLYPH: f32 = 1.5;
/// Bytes before an edit prefix that a row may still reuse: line-break and
/// shaping context never reaches further back, so the trimmed prefix is safe.
const EDIT_PREFIX_MARGIN: usize = 8;
/// Horizontal padding inside one table cell.
const TABLE_CELL_PADDING: f32 = 8.;
/// Vertical padding above and below one table row's cells.
const TABLE_ROW_PADDING: f32 = 6.;
/// Height reserved for an image whose intrinsic size is not known yet.
const IMAGE_FALLBACK_ASPECT: f32 = 9. / 16.;
/// Largest height a picture may reserve in the reading column.
const IMAGE_MAX_HEIGHT: f32 = 900.;
/// Height of the alt-text strip when an image resource failed to load.
const IMAGE_FAILED_HEIGHT: f32 = 32.;

/// Intrinsic image sizes, keyed by destination, in device pixels.
pub(crate) type ImageMetrics = std::collections::HashMap<String, (f32, f32)>;

/// Resolved document presentation used by the layout core.
///
/// The values are plain data so a width/style/revision change can be detected
/// by equality and the layout rebuilt with the new epoch.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutStyle {
    pub(crate) font: Font,
    pub(crate) mono_font: Font,
    pub(crate) font_size: Pixels,
    pub(crate) line_height: f32,
    pub(crate) foreground: Hsla,
    pub(crate) muted_foreground: Hsla,
    pub(crate) link: Hsla,
    pub(crate) code_background: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) caret: Hsla,
    pub(crate) border: Hsla,
    pub(crate) background: Hsla,
    pub(crate) heading: Hsla,
    /// Paint standalone images instead of their alt text. Writing keeps the
    /// alt text so the source stays editable; the reading pane shows pictures.
    pub(crate) render_images: bool,
}

impl LayoutStyle {
    /// Resolve the writer's prose style against the active theme.
    pub(crate) fn from_app(prose: &crate::editor::writing_modes::ProseStyle, cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            font: font(prose.font_family.clone()),
            mono_font: font(theme.mono_font_family.clone()),
            font_size: px(prose.font_size),
            line_height: prose.line_height,
            foreground: theme.foreground,
            muted_foreground: theme.muted_foreground,
            link: theme.link,
            code_background: theme.muted,
            selection: theme.selection,
            caret: theme.caret,
            border: theme.border,
            background: theme.background,
            heading: crate::theme::prose_heading_color(cx),
            render_images: false,
        }
    }

    /// Paint standalone images, for the read-only preview.
    pub(crate) fn with_images(mut self, render_images: bool) -> Self {
        self.render_images = render_images;
        self
    }

    /// Font metrics that decide wrapping and row heights.
    pub(crate) fn metrics_eq(&self, other: &Self) -> bool {
        self.font == other.font
            && self.mono_font == other.mono_font
            && self.font_size == other.font_size
            && self.line_height == other.line_height
            && self.render_images == other.render_images
    }

    /// Font size used by a heading level, matching the current block chrome.
    fn block_font_size(&self, kind: &BlockKind) -> Pixels {
        match kind {
            BlockKind::Heading(level) => self.font_size * heading_scale(*level),
            BlockKind::TableRow { header: true, .. } => self.font_size,
            _ => self.font_size,
        }
    }
}

fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 2.0,
        2 => 1.6,
        3 => 1.4,
        4 => 1.2,
        5 => 1.1,
        _ => 1.05,
    }
}

/// One rendered byte position: an offset in a block's visible text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Hit {
    pub(crate) block: usize,
    pub(crate) rendered: usize,
}

/// A directional selection in rendered text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) anchor: Hit,
    pub(crate) focus: Hit,
}

impl Selection {
    fn ordered(self) -> (Hit, Hit) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    fn is_empty(self) -> bool {
        self.anchor == self.focus
    }
}

/// A caret box in content coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caret {
    pub(crate) origin: Point<Pixels>,
    pub(crate) height: Pixels,
    pub(crate) color: Hsla,
}

/// A painted range of one block's rendered text, such as a Copyedit or
/// placeholder highlight.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decoration {
    pub(crate) block: usize,
    pub(crate) range: Range<usize>,
    pub(crate) color: Hsla,
}

/// The rendered sentence that remains undimmed in Writing mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusSentence {
    pub(crate) block: usize,
    pub(crate) range: Range<usize>,
}

/// Paints image blocks into the layout's content coordinates.
pub(crate) trait LayoutImagePainter {
    /// Paint `url` into `bounds`; return whether the image was ready.
    fn paint(&self, url: &str, bounds: Bounds<Pixels>, window: &mut Window) -> bool;
}

/// A viewport in content pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Viewport {
    /// Content pixels scrolled above the viewport top.
    pub(crate) scroll: f32,
    pub(crate) height: f32,
}

/// Per-preparation work, for the opt-in performance harness.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LayoutStats {
    pub(crate) wrapped_rows: usize,
    pub(crate) wrapped_bytes: usize,
    pub(crate) shaped_rows: usize,
    pub(crate) shaped_bytes: usize,
    pub(crate) reused_rows: usize,
    pub(crate) pending: bool,
}

/// The result of preparing one viewport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Prepared {
    /// Block range that has geometry for the viewport.
    pub(crate) blocks: Range<usize>,
    /// Wrapping or shaping stopped before covering the viewport.
    pub(crate) pending: bool,
}

/// Why a block's wrapping is advanced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WrapTarget {
    /// Enough measured rows to cover a viewport on this block.
    Rows(usize),
    /// Rows covering a rendered offset.
    Rendered(usize),
}

/// Bounded work for one viewport's rows.
struct ViewportWork<'a> {
    text_system: &'a WindowTextSystem,
    scroll: f32,
    height: f32,
    overscan: f32,
    deadline: Instant,
}

#[derive(Clone, Debug)]
struct ResolvedRun {
    range: Range<usize>,
    font: Font,
    color: Hsla,
    background: Option<Hsla>,
    strikethrough: bool,
}

impl ResolvedRun {
    fn text_run(&self, len: usize) -> TextRun {
        TextRun {
            len,
            font: self.font.clone(),
            color: self.color,
            background_color: self.background,
            underline: None,
            strikethrough: self.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(self.color),
            }),
        }
    }
}

#[derive(Clone, Debug)]
struct Row {
    /// Rendered byte range covered by this visual row.
    range: Range<usize>,
    /// Offset from the block content top.
    y: f32,
    height: f32,
    /// Shaped glyphs, only for rows retained around the prepared viewport.
    shaped: Option<ShapedLine>,
}

/// A row whose graphemes are being measured.
#[derive(Clone, Debug)]
struct PartialRow {
    start: usize,
    scan: usize,
    width: f32,
    last_break: Option<(usize, f32)>,
    break_cursor: usize,
}

struct BlockLayout {
    kind: BlockKind,
    depth: usize,
    /// Rendered text this block was laid out from, for edit reuse.
    text: SharedString,
    marks: Vec<InlineMark>,
    fingerprint: u64,
    text_len: usize,
    font_size: f32,
    row_height: f32,
    runs: Vec<ResolvedRun>,
    base: ResolvedRun,
    rows: Vec<Row>,
    /// Rendered byte offset where the next row starts.
    scanned: usize,
    partial: Option<PartialRow>,
    complete: bool,
    /// Sorted byte offsets at which a line break is allowed.
    breaks: Vec<usize>,
    break_scanned: usize,
    /// Sum of measured row byte lengths, for the remaining-height estimate.
    row_bytes: usize,
    /// Bytes per row used before this block has any measured row.
    fallback_row_bytes: f32,
    /// Block content starts this far right of the column.
    x_offset: f32,
    chrome_top: f32,
    chrome_bottom: f32,
    gap: f32,
    marker_offset: f32,
    marker: Option<SharedString>,
    marker_line: Option<ShapedLine>,
    /// Cells of one table row, each laid out in its own column slice.
    cells: Vec<BlockLayout>,
    /// Rendered offset where each table cell starts in `text`.
    cell_starts: Vec<usize>,
    /// Width of this block's column slice when it is a table cell.
    column_width: f32,
    /// Horizontal alignment inside a table cell.
    text_align: TextAlign,
    /// Picture geometry when this block paints an image.
    image: Option<ImageSpec>,
}

#[derive(Clone, Debug)]
struct ImageSpec {
    url: String,
    width: f32,
    height: f32,
}

impl BlockLayout {
    fn new(block: &RichBlock, style: &LayoutStyle, width: Pixels, images: &ImageMetrics) -> Self {
        let mut this = Self::build(block.kind.clone(), block.depth, &block.text, style, width);
        if let BlockKind::TableRow { align, .. } = &block.kind {
            let columns = align.len().max(1);
            let cell_width =
                (f32::from(width) - TABLE_CELL_PADDING * 2. * columns as f32) / columns as f32;
            let cell_width = cell_width.max(32.);
            let mut start = 0;
            for (column, cell) in block.cells.iter().enumerate() {
                if column > 0 {
                    start += 1;
                }
                this.cell_starts.push(start);
                let mut layout = Self::build(
                    BlockKind::Paragraph,
                    block.depth,
                    cell,
                    style,
                    px(cell_width),
                );
                layout.column_width = cell_width;
                layout.text_align = match align.get(column).copied().unwrap_or(ColumnAlign::Left) {
                    ColumnAlign::Left => TextAlign::Left,
                    ColumnAlign::Center => TextAlign::Center,
                    ColumnAlign::Right => TextAlign::Right,
                };
                layout.x_offset =
                    column as f32 * (cell_width + TABLE_CELL_PADDING * 2.) + TABLE_CELL_PADDING;
                start += cell.text.len();
                this.cells.push(layout);
            }
            while this.cells.len() < columns {
                let mut layout = Self::build(
                    BlockKind::Paragraph,
                    block.depth,
                    &RichText::default(),
                    style,
                    px(cell_width),
                );
                layout.column_width = cell_width;
                layout.x_offset = this.cells.len() as f32 * (cell_width + TABLE_CELL_PADDING * 2.)
                    + TABLE_CELL_PADDING;
                layout.text_len = 0;
                this.cells.push(layout);
            }
        }
        if let BlockKind::Image { url } = &block.kind
            && style.render_images
        {
            let available = f32::from(width).max(1.);
            let (mut image_width, mut image_height) = match images.get(url) {
                Some((image_width, image_height)) if *image_width > 0. && *image_height > 0. => {
                    let scale = (available / image_width).min(1.);
                    (*image_width * scale, *image_height * scale)
                }
                Some((0., 0.)) => (available, IMAGE_FAILED_HEIGHT),
                _ => (available, available * IMAGE_FALLBACK_ASPECT),
            };
            image_width = image_width.min(available).max(1.);
            image_height = image_height.clamp(1., IMAGE_MAX_HEIGHT);
            this.image = Some(ImageSpec {
                url: url.clone(),
                width: image_width,
                height: image_height,
            });
            // The picture replaces its alt text; no glyph row is prepared.
            this.rows.clear();
            this.complete = true;
        }
        this
    }

    fn build(
        kind: BlockKind,
        depth: usize,
        text: &RichText,
        style: &LayoutStyle,
        width: Pixels,
    ) -> Self {
        let font_size = f32::from(style.block_font_size(&kind));
        let row_height = font_size * style.line_height;
        let mono_all = matches!(kind, BlockKind::Code { .. });
        let base = base_run(&kind, style);
        let runs = resolve_runs(&text.text, &text.marks, style, mono_all, &base);
        let gap = f32::from(style.font_size) * 0.75;
        let (x_offset, chrome_top, chrome_bottom, marker_offset, marker) = match &kind {
            BlockKind::ListItem { ordered, number } => {
                let marker = if *ordered {
                    format!("{number}.")
                } else {
                    "•".to_owned()
                };
                let indent = depth as f32 * 20.;
                (
                    indent + 18.,
                    0.,
                    0.,
                    indent,
                    Some(SharedString::from(marker)),
                )
            }
            BlockKind::Quote => (12., 0., 0., 0., None),
            BlockKind::Code { .. } => (0., 8., 8., 0., None),
            BlockKind::Rule => (0., 6., 7., 0., None),
            BlockKind::Footnote { label } => (
                28.,
                0.,
                0.,
                0.,
                Some(SharedString::from(format!("[{label}]"))),
            ),
            BlockKind::Raw => (12., 0., 0., 0., None),
            BlockKind::TableRow { .. } => (0., TABLE_ROW_PADDING, TABLE_ROW_PADDING, 0., None),
            BlockKind::Image { .. } => (0., 0., 0., 0., None),
            BlockKind::Heading(_) | BlockKind::Paragraph => (0., 0., 0., 0., None),
        };
        let fallback_row_bytes =
            (f32::from(width) / (font_size * 0.55)).max(8.) * FALLBACK_BYTES_PER_GLYPH;
        let marks = text.marks.clone();
        let text = SharedString::from(text.text.clone());
        let fingerprint = block_fingerprint(&kind, depth, &text, &marks);
        let mut this = Self {
            kind,
            depth,
            text,
            marks,
            fingerprint,
            text_len: 0,
            font_size,
            row_height,
            runs,
            base,
            rows: Vec::new(),
            scanned: 0,
            partial: None,
            complete: false,
            breaks: Vec::new(),
            break_scanned: 0,
            row_bytes: 0,
            fallback_row_bytes,
            x_offset,
            chrome_top,
            chrome_bottom,
            gap,
            marker_offset,
            marker,
            marker_line: None,
            cells: Vec::new(),
            cell_starts: Vec::new(),
            column_width: f32::from(width),
            text_align: TextAlign::Left,
            image: None,
        };
        this.text_len = this.text.len();
        if this.text.is_empty() {
            this.rows.push(Row {
                range: 0..0,
                y: 0.,
                height: row_height,
                shaped: None,
            });
            this.complete = true;
        }
        this
    }

    fn matches(&self, block: &RichBlock) -> bool {
        self.kind == block.kind
            && self.depth == block.depth
            && self.text.as_ref() == block.text.text
            && self.marks == block.text.marks
    }

    /// Re-resolve colours and fonts for a style whose metrics did not change.
    fn update_style(&mut self, style: &LayoutStyle) {
        let mono_all = matches!(self.kind, BlockKind::Code { .. });
        self.base = base_run(&self.kind, style);
        self.runs = resolve_runs(&self.text, &self.marks, style, mono_all, &self.base);
        for row in &mut self.rows {
            row.shaped = None;
        }
        self.marker_line = None;
        for cell in &mut self.cells {
            cell.update_style(style);
        }
    }

    /// Horizontal offset of a shaped row inside this block's column.
    fn align_offset(&self, width: Pixels) -> Pixels {
        let column = px(self.column_width);
        match self.text_align {
            TextAlign::Left => px(0.),
            TextAlign::Center => (column - width) * 0.5,
            TextAlign::Right => column - width,
        }
    }

    fn is_table(&self) -> bool {
        matches!(self.kind, BlockKind::TableRow { .. })
    }

    /// Offset range of one cell in the row's joined text.
    fn cell_range(&self, cell: usize) -> Range<usize> {
        let start = self.cell_starts.get(cell).copied().unwrap_or(0);
        let end = self
            .cells
            .get(cell)
            .map_or(start, |layout| start + layout.text_len);
        start..end
    }

    /// Cell and local offset for one rendered offset in a table row.
    fn cell_offset(&self, rendered: usize) -> (usize, usize) {
        let cell = self
            .cell_starts
            .partition_point(|start| *start <= rendered)
            .saturating_sub(1);
        let start = self.cell_starts.get(cell).copied().unwrap_or(0);
        (cell, rendered.saturating_sub(start))
    }

    /// Height of this block's text, without the trailing block gap.
    fn text_height(&self) -> f32 {
        let tail_rows = if self.complete {
            0.
        } else {
            let remaining = self.text_len.saturating_sub(self.scanned);
            let average = if self.rows.is_empty() {
                self.fallback_row_bytes
            } else {
                (self.row_bytes as f32 / self.rows.len() as f32).max(1.)
            };
            (remaining as f32 / average).ceil().max(1.)
        };
        self.chrome_top
            + self.rows.len() as f32 * self.row_height
            + tail_rows * self.row_height
            + self.chrome_bottom
    }

    /// Carry rows that end safely before the common prefix of an edit.
    fn carry_prefix(&mut self, old: &BlockLayout, block: &RichBlock) -> usize {
        let old_text = old.text.as_str();
        let new_text = block.text.text.as_str();
        // Compare unchanged source in chunks instead of visiting every byte
        // on the UI thread after an edit near the end of a huge paragraph.
        const CHUNK: usize = 64;
        let mut prefix = old_text
            .as_bytes()
            .as_chunks::<CHUNK>()
            .0
            .iter()
            .zip(new_text.as_bytes().as_chunks::<CHUNK>().0)
            .take_while(|(before, after)| before == after)
            .count()
            * CHUNK;
        let limit = old_text.len().min(new_text.len());
        while prefix < limit && old_text.as_bytes()[prefix] == new_text.as_bytes()[prefix] {
            prefix += 1;
        }
        while prefix > 0 && !old_text.is_char_boundary(prefix) {
            prefix -= 1;
        }
        // Rendered bytes alone do not identify cached geometry or glyphs:
        // adding Markdown delimiters can change runs without changing text.
        let mut cursor = 0;
        while cursor < prefix {
            let before = old.run_at(cursor);
            let after = self.run_at(cursor);
            if before.font != after.font
                || before.color != after.color
                || before.background != after.background
                || before.strikethrough != after.strikethrough
            {
                prefix = cursor;
                break;
            }
            cursor = before.range.end.min(after.range.end).min(prefix);
        }
        let safe = prefix.saturating_sub(EDIT_PREFIX_MARGIN);
        if safe == 0 {
            return 0;
        }
        let reuse_count = old
            .rows
            .iter()
            .take_while(|row| row.range.end <= safe)
            .count();
        if reuse_count == 0 {
            return 0;
        }
        self.rows = old.rows[..reuse_count].to_vec();
        self.row_bytes = self.rows.iter().map(|row| row.range.len()).sum();
        self.scanned = self.rows.last().map_or(0, |row| row.range.end);
        let boundary = self.rows.last().map_or(0, |row| row.range.end);
        self.breaks = old
            .breaks
            .iter()
            .copied()
            .take_while(|offset| *offset <= boundary)
            .collect();
        self.break_scanned = boundary;
        self.partial = None;
        self.complete = false;
        reuse_count
    }

    fn push_row(&mut self, range: Range<usize>) {
        let y = self.rows.last().map_or(0., |row| row.y + row.height);
        self.row_bytes += range.len();
        self.rows.push(Row {
            range,
            y,
            height: self.row_height,
            shaped: None,
        });
    }

    fn run_at(&self, offset: usize) -> &ResolvedRun {
        if self.runs.is_empty() {
            return &self.base;
        }
        let index = self
            .runs
            .partition_point(|run| run.range.end <= offset)
            .min(self.runs.len() - 1);
        &self.runs[index]
    }

    fn runs_for(&self, range: Range<usize>) -> Vec<TextRun> {
        let mut runs = Vec::new();
        let mut cursor = range.start;
        for run in &self.runs {
            if run.range.end <= range.start {
                continue;
            }
            if run.range.start >= range.end {
                break;
            }
            let start = run.range.start.max(range.start);
            let end = run.range.end.min(range.end);
            if start > cursor {
                runs.push(self.base.text_run(start - cursor));
            }
            runs.push(run.text_run(end - start));
            cursor = end;
        }
        if cursor < range.end {
            runs.push(self.base.text_run(range.end - cursor));
        }
        runs
    }

    /// Compute line-break opportunities up to `upto`.
    fn ensure_breaks(&mut self, text: &str, upto: usize) {
        while self.break_scanned < upto && self.break_scanned < text.len() {
            let rest = &text[self.break_scanned..];
            let mut end = rest.len().min(BREAK_CHUNK_BYTES);
            while end < rest.len() && !rest.is_char_boundary(end) {
                end += 1;
            }
            let (safe, _) = unicode_linebreak::split_at_safe(&rest[..end]);
            let boundary = if safe.is_empty() { end } else { safe.len() };
            for (offset, _) in unicode_linebreak::linebreaks(&rest[..boundary]) {
                // `linebreaks` always reports the end of its input; that is not
                // a real opportunity in the middle of the text.
                if offset == boundary && boundary < rest.len() {
                    continue;
                }
                self.breaks.push(self.break_scanned + offset);
            }
            self.break_scanned += boundary;
            if boundary == 0 {
                break;
            }
        }
    }

    /// Measured row height plus an estimate for the unwrapped tail.
    fn height(&self) -> f32 {
        if let Some(image) = &self.image {
            return image.height + self.gap;
        }
        if self.is_table() {
            let cells = self
                .cells
                .iter()
                .map(|cell| cell.text_height())
                .fold(0f32, f32::max);
            return self.chrome_top + cells + self.chrome_bottom + self.gap;
        }
        self.text_height() + self.gap
    }

    fn row_index_for_offset(&self, rendered: usize) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let index = self
            .rows
            .partition_point(|row| row.range.start <= rendered)
            .saturating_sub(1);
        let row = self.rows.get(index)?;
        if rendered > row.range.end {
            // The offset lies beyond the wrapped prefix; the row that will
            // hold it does not exist yet.
            return None;
        }
        Some(index)
    }

    /// Caret point and height for a rendered offset inside this block, with
    /// `top` as the block's content top in document coordinates.
    fn caret_at(&self, top: f32, rendered: usize) -> Option<(Point<Pixels>, Pixels)> {
        if self.is_table() {
            let (cell, local) = self.cell_offset(rendered);
            let cell = self.cells.get(cell)?;
            return cell.caret_at(top + self.chrome_top, local);
        }
        let row_index = self.row_index_for_offset(rendered)?;
        let row = self.rows.get(row_index)?;
        let shaped = row.shaped.as_ref()?;
        let local = rendered
            .saturating_sub(row.range.start)
            .min(row.range.len());
        let x = shaped.x_for_index(local) + self.align_offset(shaped.width());
        let padding = (row.height - f32::from(shaped.ascent) - f32::from(shaped.descent)) * 0.5;
        let y = top + self.chrome_top + row.y + padding;
        Some((
            point(px(self.x_offset) + x, px(y)),
            (shaped.ascent + shaped.descent).max(px(2.)),
        ))
    }

    /// Rendered offset under a point relative to the block's content top.
    fn hit_test_at(&self, x: f32, local_y: f32) -> Option<usize> {
        if self.is_table() {
            let cell_index = self
                .cells
                .iter()
                .rposition(|cell| x >= cell.x_offset - TABLE_CELL_PADDING)
                .unwrap_or(0);
            let cell = self.cells.get(cell_index)?;
            let local = cell.hit_test_at(x, local_y)?;
            return Some(self.cell_starts.get(cell_index).copied().unwrap_or(0) + local);
        }
        let row_index = row_index_at(self, local_y);
        let row = self.rows.get(row_index)?;
        let shaped = row.shaped.as_ref()?;
        let index = shaped
            .closest_index_for_x(px(x - self.x_offset) - self.align_offset(shaped.width()))
            .min(row.range.len());
        Some(row.range.start + index)
    }

    /// Selection rectangles covering `range` inside this block.
    fn range_quads(&self, top: f32, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        if self.is_table() {
            let mut quads = Vec::new();
            for (cell_index, cell) in self.cells.iter().enumerate() {
                let cell_range = self.cell_range(cell_index);
                let start = range.start.max(cell_range.start);
                let end = range.end.min(cell_range.end);
                if start >= end {
                    continue;
                }
                quads.extend(cell.range_quads(
                    top + self.chrome_top,
                    start - cell_range.start..end - cell_range.start,
                ));
            }
            return quads;
        }
        let mut quads = Vec::new();
        for row in &self.rows {
            let row_start = row.range.start.max(range.start);
            let row_end = row.range.end.min(range.end);
            if row_start >= row_end {
                continue;
            }
            let Some(shaped) = row.shaped.as_ref() else {
                continue;
            };
            let local_start = row_start - row.range.start;
            let local_end = row_end - row.range.start;
            let offset = self.align_offset(shaped.width());
            let left = shaped.x_for_index(local_start) + offset;
            let right = shaped.x_for_index(local_end) + offset;
            let y = top + self.chrome_top + row.y;
            quads.push(Bounds::new(
                point(px(self.x_offset) + left, px(y)),
                size((right - left).max(px(1.)), px(row.height)),
            ));
        }
        quads
    }

    fn cursor_at(&self, offset: usize) -> usize {
        self.breaks
            .partition_point(|candidate| *candidate <= offset)
    }

    /// Advance wrapping until `target` is met, the block ends or the budget
    /// expires. Returns whether the target is met.
    fn advance(
        &mut self,
        text: &str,
        width: Pixels,
        target: WrapTarget,
        text_system: &WindowTextSystem,
        deadline: Instant,
        examined: &mut usize,
    ) -> bool {
        if self.complete {
            return true;
        }
        match target {
            WrapTarget::Rows(rows) if self.rows.len() >= rows => return true,
            WrapTarget::Rendered(offset) if self.scanned > offset => return true,
            _ => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        {
            let mut partial = self.partial.take().unwrap_or_else(|| PartialRow {
                start: self.scanned,
                scan: self.scanned,
                width: 0.,
                last_break: None,
                break_cursor: self.cursor_at(self.scanned),
            });
            let mut checks = 0usize;
            loop {
                if let WrapTarget::Rows(rows) = target
                    && self.rows.len() >= rows
                {
                    self.scanned = partial.start;
                    self.partial = Some(partial);
                    return true;
                }
                if let WrapTarget::Rendered(offset) = target
                    && partial.start > offset
                {
                    self.scanned = partial.start;
                    self.partial = Some(partial);
                    return true;
                }
                if partial.scan >= text.len() {
                    if partial.scan > partial.start {
                        self.push_row(partial.start..partial.scan);
                    } else if text.ends_with('\n') {
                        self.push_row(partial.scan..partial.scan);
                    }
                    self.scanned = text.len();
                    self.complete = true;
                    self.partial = None;
                    return true;
                }
                if text.as_bytes()[partial.scan] == b'\n' {
                    self.push_row(partial.start..partial.scan);
                    let next = partial.scan + 1;
                    partial = PartialRow {
                        start: next,
                        scan: next,
                        width: 0.,
                        last_break: None,
                        break_cursor: self.cursor_at(next),
                    };
                } else {
                    let glyph_len = next_grapheme_len(text, partial.scan);
                    let grapheme = &text[partial.scan..partial.scan + glyph_len];
                    let next = partial.scan + glyph_len;
                    *examined += glyph_len;
                    let run = self.run_at(partial.scan);
                    let advance = grapheme_width(text_system, grapheme, run, self.font_size);
                    self.ensure_breaks(text, next);
                    while partial.break_cursor < self.breaks.len()
                        && self.breaks[partial.break_cursor] <= next
                    {
                        partial.break_cursor += 1;
                    }
                    partial.width += f32::from(advance);
                    if self.breaks.get(partial.break_cursor.saturating_sub(1)) == Some(&next) {
                        partial.last_break = Some((next, partial.width));
                    }
                    if partial.width > f32::from(width) && next > partial.start {
                        match partial.last_break {
                            Some((offset, _)) if offset > partial.start && offset < next => {
                                self.push_row(partial.start..offset);
                                partial = PartialRow {
                                    start: offset,
                                    scan: offset,
                                    width: 0.,
                                    last_break: None,
                                    break_cursor: self.cursor_at(offset),
                                };
                            }
                            Some((offset, _)) if offset == next => {
                                self.push_row(partial.start..next);
                                partial = PartialRow {
                                    start: next,
                                    scan: next,
                                    width: 0.,
                                    last_break: None,
                                    break_cursor: partial.break_cursor,
                                };
                            }
                            _ if partial.scan == partial.start => {
                                // One grapheme wider than the column still
                                // gets its own row instead of looping.
                                self.push_row(partial.start..next);
                                partial = PartialRow {
                                    start: next,
                                    scan: next,
                                    width: 0.,
                                    last_break: None,
                                    break_cursor: partial.break_cursor,
                                };
                            }
                            _ => {
                                self.push_row(partial.start..partial.scan);
                                partial = PartialRow {
                                    start: partial.scan,
                                    scan: partial.scan,
                                    width: 0.,
                                    last_break: None,
                                    break_cursor: self.cursor_at(partial.scan),
                                };
                            }
                        }
                    } else {
                        partial.scan = next;
                    }
                }
                checks += 1;
                if checks.is_multiple_of(BUDGET_CHECK_GRAPHEMES) && Instant::now() >= deadline {
                    self.scanned = partial.start;
                    self.partial = Some(partial);
                    return false;
                }
            }
        }
    }
}

fn next_grapheme_len(text: &str, offset: usize) -> usize {
    text[offset..].graphemes(true).next().map_or(1, str::len)
}

/// Shape a single grapheme for wrapping. Single-code-point graphemes use the
/// text system's cached per-character advance, which is also what GPUI's own
/// line wrapper measures; clusters (combining marks, emoji sequences) are
/// shaped whole so they keep their cluster advance.
fn grapheme_width(
    text_system: &WindowTextSystem,
    grapheme: &str,
    run: &ResolvedRun,
    font_size: f32,
) -> Pixels {
    let mut chars = grapheme.chars();
    if let (Some(character), None) = (chars.next(), chars.next()) {
        let font_id = text_system.resolve_font(&run.font);
        return text_system.layout_width(font_id, px(font_size), character);
    }
    text_system
        .shape_line(
            SharedString::from(grapheme.to_owned()),
            px(font_size),
            &[run.text_run(grapheme.len())],
            None,
        )
        .width()
}

/// The unmarked run of a block: font, colour and weight before inline marks.
fn base_run(kind: &BlockKind, style: &LayoutStyle) -> ResolvedRun {
    let mono_all = matches!(kind, BlockKind::Code { .. } | BlockKind::Raw);
    let mut font = if mono_all {
        style.mono_font.clone()
    } else {
        style.font.clone()
    };
    let color = match kind {
        BlockKind::Heading(_) => {
            font.weight = FontWeight::BOLD;
            style.heading
        }
        BlockKind::Quote | BlockKind::Footnote { .. } => style.muted_foreground,
        BlockKind::Raw => style.muted_foreground,
        _ => style.foreground,
    };
    ResolvedRun {
        range: 0..0,
        font,
        color,
        background: None,
        strikethrough: false,
    }
}

fn resolve_runs(
    text: &str,
    marks: &[InlineMark],
    style: &LayoutStyle,
    mono_all: bool,
    base: &ResolvedRun,
) -> Vec<ResolvedRun> {
    let len = text.len();
    if len == 0 {
        return Vec::new();
    }
    let mut cuts = vec![0, len];
    let mut events = Vec::with_capacity(marks.len() * 2);
    for (index, mark) in marks.iter().enumerate() {
        let start = mark.range.start.min(len);
        let end = mark.range.end.min(len);
        if start < end {
            cuts.extend([start, end]);
            events.extend([(start, true, index), (end, false, index)]);
        }
    }
    events.sort_unstable();
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::with_capacity(cuts.len().saturating_sub(1));
    let mut event_index = 0;
    let mut active = std::collections::BTreeSet::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end {
            continue;
        }
        while let Some(&(offset, entering, index)) = events.get(event_index) {
            if offset > start {
                break;
            }
            if entering {
                active.insert(index);
            } else {
                active.remove(&index);
            }
            event_index += 1;
        }
        let mut bold = false;
        let mut italic = false;
        let mut strikethrough = false;
        let mut code = false;
        let mut link = false;
        for index in &active {
            match &marks[*index].kind {
                InlineMarkKind::Bold => bold = true,
                InlineMarkKind::Italic => italic = true,
                InlineMarkKind::Strikethrough => strikethrough = true,
                InlineMarkKind::Code => code = true,
                InlineMarkKind::Link(_) => link = true,
                InlineMarkKind::Image(_) => italic = true,
            }
        }
        let mut font = if mono_all || code {
            style.mono_font.clone()
        } else {
            base.font.clone()
        };
        if bold {
            font.weight = FontWeight::BOLD;
        }
        if italic {
            font.style = FontStyle::Italic;
        }
        runs.push(ResolvedRun {
            range: start..end,
            font,
            color: if link { style.link } else { base.color },
            background: code.then_some(style.code_background),
            strikethrough,
        });
    }
    runs
}

/// Work reused by [`TextLayout::update`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UpdateOutcome {
    pub(crate) reused_blocks: usize,
    pub(crate) reused_rows: usize,
    pub(crate) rebuilt_blocks: usize,
}

fn block_fingerprint(kind: &BlockKind, depth: usize, text: &str, marks: &[InlineMark]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(kind).hash(&mut hasher);
    depth.hash(&mut hasher);
    text.hash(&mut hasher);
    for mark in marks {
        mark.range.hash(&mut hasher);
        std::mem::discriminant(&mark.kind).hash(&mut hasher);
        match &mark.kind {
            InlineMarkKind::Link(url) | InlineMarkKind::Image(url) => url.hash(&mut hasher),
            _ => {}
        }
    }
    hasher.finish()
}

/// Line breaking opportunities and heights for every block.
pub(crate) struct TextLayout {
    document: Arc<WysiwygDocument>,
    style: Arc<LayoutStyle>,
    width: Pixels,
    blocks: Vec<BlockLayout>,
    heights: HeightIndex,
    stats: LayoutStats,
    images: ImageMetrics,
}

impl TextLayout {
    /// Build a layout for one accepted document revision and width epoch.
    pub(crate) fn new(
        document: Arc<WysiwygDocument>,
        style: Arc<LayoutStyle>,
        width: Pixels,
        images: &ImageMetrics,
    ) -> Self {
        let width = width.max(px(1.));
        let blocks: Vec<BlockLayout> = document
            .blocks
            .iter()
            .map(|block| BlockLayout::new(block, &style, width, images))
            .collect();
        let mut heights = HeightIndex::new(blocks.len());
        for (index, block) in blocks.iter().enumerate() {
            heights.set(index, f64::from(block.height()));
        }
        Self {
            document,
            style,
            width,
            blocks,
            heights,
            stats: LayoutStats::default(),
            images: images.clone(),
        }
    }

    /// Adopt newly loaded intrinsic image sizes. Only picture blocks change
    /// height, so the rest of the measured rows stay valid.
    pub(crate) fn set_image_metrics(&mut self, images: &ImageMetrics) -> bool {
        if *images == self.images {
            return false;
        }
        self.images = images.clone();
        let mut changed = false;
        for (index, block) in self.blocks.iter_mut().enumerate() {
            let BlockKind::Image { url } = &block.kind else {
                continue;
            };
            let image = block.image.as_mut();
            let Some(image) = image else {
                continue;
            };
            let available = f32::from(self.width).max(1.);
            let (next_width, next_height) = match images.get(url) {
                Some((width, height)) if *width > 0. && *height > 0. => {
                    let scale = (available / width).min(1.);
                    (
                        (width * scale).min(available).max(1.),
                        (height * scale).clamp(1., IMAGE_MAX_HEIGHT),
                    )
                }
                Some((0., 0.)) => (available, IMAGE_FAILED_HEIGHT),
                _ => (available, available * IMAGE_FALLBACK_ASPECT),
            };
            if (image.width - next_width).abs() > 0.5 || (image.height - next_height).abs() > 0.5 {
                image.width = next_width;
                image.height = next_height;
                self.heights.set(index, f64::from(block.height()));
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn document(&self) -> &WysiwygDocument {
        &self.document
    }

    pub(crate) fn stats(&self) -> LayoutStats {
        self.stats
    }

    pub(crate) fn style(&self) -> &LayoutStyle {
        &self.style
    }

    /// Adopt a new presentation without discarding wrapping when the font
    /// metrics are unchanged. Shaped rows are dropped so the next viewport
    /// frame paints the new colours; the document is not re-parsed and rows
    /// are not re-wrapped.
    pub(crate) fn set_style(&mut self, style: Arc<LayoutStyle>) {
        if *self.style == *style {
            return;
        }
        if !self.style.metrics_eq(&style) {
            return;
        }
        self.style = style.clone();
        for block in &mut self.blocks {
            block.update_style(&style);
        }
    }

    /// Total content height in pixels.
    pub(crate) fn total_height(&self) -> f32 {
        self.heights.total() as f32
    }

    pub(crate) fn block_top(&self, index: usize) -> f32 {
        self.heights.prefix(index) as f32
    }

    pub(crate) fn block_at_y(&self, y: f32) -> Option<usize> {
        self.heights.block_at(f64::from(y))
    }

    /// Content position of `hit`, or `None` while its row is unresolved.
    pub(crate) fn caret(&self, hit: Hit) -> Option<Caret> {
        let block = self.blocks.get(hit.block)?;
        let (origin, height) = block.caret_at(self.block_top(hit.block), hit.rendered)?;
        Some(Caret {
            origin,
            height,
            color: self.style.caret,
        })
    }

    /// Bounds of a rendered range in content coordinates.
    ///
    /// A collapsed range yields the caret box, which is what native IME
    /// candidate placement consumes.
    pub(crate) fn bounds_for_range(&self, anchor: Hit, focus: Hit) -> Option<Bounds<Pixels>> {
        let selection = Selection { anchor, focus };
        if selection.is_empty() {
            let caret = self.caret(focus)?;
            return Some(Bounds::new(caret.origin, size(px(1.), caret.height)));
        }
        let quads = self.selection_quads(selection);
        quads.into_iter().reduce(|left, right| left.union(&right))
    }

    /// Selection rectangles in content coordinates, one per affected row.
    pub(crate) fn selection_quads(&self, selection: Selection) -> Vec<Bounds<Pixels>> {
        let (start, end) = selection.ordered();
        if self.blocks.is_empty() {
            return Vec::new();
        }
        let first = start.block.min(self.blocks.len() - 1);
        let last = end.block.min(self.blocks.len() - 1);
        let mut quads = Vec::new();
        for index in first..=last {
            let start_offset = if index == start.block {
                start.rendered
            } else {
                0
            };
            let end_offset = if index == end.block {
                end.rendered
            } else {
                self.blocks[index].text_len
            };
            if start_offset >= end_offset {
                continue;
            }
            quads.extend(self.block_range_quads(index, start_offset..end_offset));
        }
        quads
    }

    /// Rectangles one rendered range of one block covers, in content
    /// coordinates. Rows without glyphs contribute nothing.
    fn block_range_quads(&self, index: usize, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let Some(block) = self.blocks.get(index) else {
            return Vec::new();
        };
        block.range_quads(self.block_top(index), range)
    }

    /// Rendered byte position under a content-space point, if the row has
    /// been shaped.
    pub(crate) fn hit_test(&self, position: Point<Pixels>) -> Option<Hit> {
        let y = f32::from(position.y);
        let block_index = self.block_at_y(y)?;
        let block = self.blocks.get(block_index)?;
        let local = y - self.block_top(block_index);
        let rendered = block.hit_test_at(f32::from(position.x), local)?;
        Some(Hit {
            block: block_index,
            rendered,
        })
    }

    /// Nearest caret on the visual row above or below `hit`.
    pub(crate) fn vertical_target(&self, hit: Hit, up: bool) -> Option<Hit> {
        let caret = self.caret(hit)?;
        let step = self.blocks.get(hit.block)?.row_height;
        let target_y = f32::from(caret.origin.y) + if up { -step } else { step };
        self.hit_test(point(caret.origin.x, px(target_y)))
    }

    /// Reconcile this layout with a new accepted document revision.
    ///
    /// Unchanged blocks keep their rows, breaks and shaped lines even when an
    /// insertion shifts their index. The block that changed keeps the rows
    /// before a safe edit prefix, so a local edit does not re-wrap the whole
    /// document.
    pub(crate) fn update(&mut self, document: Arc<WysiwygDocument>) -> UpdateOutcome {
        let width = self.width;
        let style = &self.style;
        let mut old: Vec<Option<BlockLayout>> = std::mem::take(&mut self.blocks)
            .into_iter()
            .map(Some)
            .collect();
        let mut by_fingerprint: std::collections::HashMap<u64, Vec<usize>> =
            std::collections::HashMap::new();
        for (index, block) in old.iter().enumerate() {
            if let Some(block) = block {
                by_fingerprint
                    .entry(block.fingerprint)
                    .or_default()
                    .push(index);
            }
        }
        let mut outcome = UpdateOutcome::default();
        let mut blocks = Vec::with_capacity(document.blocks.len());
        for (index, block) in document.blocks.iter().enumerate() {
            let fingerprint = block_fingerprint(
                &block.kind,
                block.depth,
                &block.text.text,
                &block.text.marks,
            );
            let mut reused = None;
            if let Some(candidates) = by_fingerprint.get_mut(&fingerprint) {
                while let Some(candidate) = candidates.pop() {
                    if old[candidate]
                        .as_ref()
                        .is_some_and(|layout| layout.matches(block))
                    {
                        reused = old[candidate].take();
                        break;
                    }
                }
            }
            if let Some(layout) = reused {
                outcome.reused_blocks += 1;
                outcome.reused_rows += layout.rows.len();
                blocks.push(layout);
            } else {
                let mut rebuilt = BlockLayout::new(block, style, width, &self.images);
                if let Some(candidate) = old.get_mut(index).and_then(Option::take)
                    && candidate.kind == rebuilt.kind
                    && candidate.depth == rebuilt.depth
                {
                    let carried = rebuilt.carry_prefix(&candidate, block);
                    outcome.reused_rows += carried;
                }
                outcome.rebuilt_blocks += 1;
                blocks.push(rebuilt);
            }
        }
        self.blocks = blocks;
        self.document = document;
        self.heights = HeightIndex::new(self.blocks.len());
        for (index, block) in self.blocks.iter().enumerate() {
            self.heights.set(index, f64::from(block.height()));
        }
        self.stats = LayoutStats::default();
        outcome
    }

    /// Wrap and shape the viewport with a bounded work budget.
    ///
    /// Only the rows this viewport needs are wrapped; the rest of a long block
    /// keeps an estimated height until a later frame needs it.
    pub(crate) fn prepare(
        &mut self,
        window: &Window,
        viewport: Viewport,
        caret: Option<Hit>,
        overscan: f32,
    ) -> Prepared {
        self.prepare_with(window.text_system(), viewport, caret, overscan)
    }

    /// Prepare with an explicit text system, for headless layout tests.
    pub(crate) fn prepare_with(
        &mut self,
        text_system: &WindowTextSystem,
        viewport: Viewport,
        caret: Option<Hit>,
        overscan: f32,
    ) -> Prepared {
        self.stats = LayoutStats::default();
        let started = Instant::now();
        let deadline = started + LAYOUT_BUDGET;
        let wrap_deadline = started + LAYOUT_BUDGET.mul_f32(WRAP_BUDGET_SHARE);
        let scroll = viewport.scroll.max(0.);
        let top = (scroll - overscan).max(0.);
        let bottom = scroll + viewport.height + overscan;
        let start = self.block_at_y(top).unwrap_or(0);
        let end = self
            .block_at_y(bottom)
            .map_or(self.blocks.len(), |index| index + 1);
        let mut pending = false;

        if let Some(hit) = caret.filter(|hit| hit.block < self.blocks.len())
            && !self.advance(
                hit.block,
                WrapTarget::Rendered(hit.rendered),
                text_system,
                wrap_deadline,
            )
        {
            pending = true;
        }

        let work = ViewportWork {
            text_system,
            scroll,
            height: viewport.height,
            overscan,
            deadline,
        };
        for index in start..end {
            if Instant::now() < wrap_deadline {
                let block_top = self.block_top(index);
                let block = &self.blocks[index];
                let local_top = (top - block_top - block.chrome_top).max(0.);
                let rows_above = (local_top / block.row_height).ceil() as usize;
                let visible_rows =
                    ((viewport.height + 2. * overscan) / block.row_height).ceil() as usize + 2;
                let needed = rows_above + visible_rows;
                if !self.advance(index, WrapTarget::Rows(needed), text_system, wrap_deadline) {
                    pending = true;
                }
            } else {
                pending = true;
            }
            self.shape_visible(index, &work, &mut pending);
            if Instant::now() >= deadline {
                pending = true;
                break;
            }
        }
        if let Some(hit) = caret.filter(|hit| hit.block < self.blocks.len()) {
            self.shape_caret_row(hit, text_system, deadline, &mut pending);
        }

        self.stats.pending = pending;
        Prepared {
            blocks: start..end.max(start),
            pending,
        }
    }

    fn advance(
        &mut self,
        index: usize,
        target: WrapTarget,
        text_system: &WindowTextSystem,
        deadline: Instant,
    ) -> bool {
        let width = self.width;
        let document = &self.document;
        let block = &mut self.blocks[index];
        let before_rows = block.rows.len();
        let mut examined = 0usize;
        if block.is_table() {
            let mut met = true;
            for cell in &mut block.cells {
                let text = std::mem::take(&mut cell.text);
                let before = cell.rows.len();
                met &= cell.advance(
                    &text,
                    px(cell.column_width),
                    target,
                    text_system,
                    deadline,
                    &mut examined,
                );
                self.stats.wrapped_rows += cell.rows.len() - before;
                cell.text = text;
            }
            self.stats.wrapped_bytes += examined;
            if block.rows.len() != before_rows || !block.cells.is_empty() {
                self.heights.set(index, f64::from(block.height()));
            }
            return met;
        }
        let text = &document.blocks[index].text.text;
        let met = block.advance(text, width, target, text_system, deadline, &mut examined);
        self.stats.wrapped_rows += block.rows.len() - before_rows;
        self.stats.wrapped_bytes += examined;
        if block.rows.len() != before_rows || block.complete {
            self.heights.set(index, f64::from(block.height()));
        }
        met
    }

    /// Shape the row that holds the caret even when it is outside the viewport.
    ///
    /// Following the caret to a far position needs that row's geometry before
    /// the viewport can scroll to it; painting only the current viewport would
    /// leave the target unresolved.
    fn shape_caret_row(
        &mut self,
        hit: Hit,
        text_system: &WindowTextSystem,
        deadline: Instant,
        pending: &mut bool,
    ) {
        let Some(block) = self.blocks.get_mut(hit.block) else {
            *pending = true;
            return;
        };
        if block.is_table() {
            let (cell_index, local) = block.cell_offset(hit.rendered);
            let Some(cell) = block.cells.get_mut(cell_index) else {
                *pending = true;
                return;
            };
            let Some(row_index) = cell.row_index_for_offset(local) else {
                *pending = true;
                return;
            };
            if Instant::now() >= deadline {
                *pending = true;
                return;
            }
            let text = std::mem::take(&mut cell.text);
            let (did_shape, bytes) = shape_row_in(cell, &text, row_index, text_system);
            cell.text = text;
            if did_shape {
                self.stats.shaped_rows += 1;
                self.stats.shaped_bytes += bytes;
            }
            return;
        }
        let Some(row_index) = block.row_index_for_offset(hit.rendered) else {
            *pending = true;
            return;
        };
        if block.rows[row_index].shaped.is_some() {
            return;
        }
        if Instant::now() >= deadline {
            *pending = true;
            return;
        }
        let text = &self.document.blocks[hit.block].text.text;
        let (did_shape, bytes) = shape_row_in(block, text, row_index, text_system);
        if did_shape {
            self.stats.shaped_rows += 1;
            self.stats.shaped_bytes += bytes;
        }
    }

    fn shape_visible(&mut self, index: usize, work: &ViewportWork<'_>, pending: &mut bool) {
        let block_top = self.block_top(index);
        let content_top = work.scroll - work.overscan;
        let content_bottom = work.scroll + work.height + work.overscan;
        if self.blocks[index].is_table() {
            let mut rows = 0usize;
            let mut shaped = 0usize;
            let mut bytes = 0usize;
            let block = &mut self.blocks[index];
            for cell in &mut block.cells {
                let text = std::mem::take(&mut cell.text);
                let (cell_rows, cell_shaped, cell_bytes) = shape_rows_in(
                    cell,
                    &text,
                    block_top + block.chrome_top,
                    content_top,
                    content_bottom,
                    work,
                    pending,
                );
                cell.text = text;
                rows += cell_rows;
                shaped += cell_shaped;
                bytes += cell_bytes;
            }
            self.stats.reused_rows += rows;
            self.stats.shaped_rows += shaped;
            self.stats.shaped_bytes += bytes;
            return;
        }
        let text = &self.document.blocks[index].text.text;
        let (rows, shaped, bytes) = {
            let block = &mut self.blocks[index];
            shape_rows_in(
                block,
                text,
                block_top,
                content_top,
                content_bottom,
                work,
                pending,
            )
        };
        self.stats.reused_rows += rows;
        self.stats.shaped_rows += shaped;
        self.stats.shaped_bytes += bytes;
        // Drop glyphs far outside the viewport; their rows and heights remain.
        let keep_top = work.scroll - work.overscan - work.height * SHAPE_RETENTION_SCREENS;
        let keep_bottom =
            work.scroll + work.height + work.overscan + work.height * SHAPE_RETENTION_SCREENS;
        let block = &mut self.blocks[index];
        for row in &mut block.rows {
            if row.shaped.is_none() {
                continue;
            }
            let y = block_top + block.chrome_top + row.y;
            if y + row.height < keep_top || y > keep_bottom {
                row.shaped = None;
            }
        }
    }

    /// Paint the prepared viewport. Returns the number of rows painted.
    ///
    /// `origin` is the window position of content coordinate (0, 0); clipping
    /// comes from the current content mask, so glyphs never spill over chrome
    /// outside the scroll viewport.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        origin: Point<Pixels>,
        selection: Option<Selection>,
        caret: Option<Hit>,
        decorations: &[Decoration],
        focus: Option<&FocusSentence>,
        images: Option<&dyn LayoutImagePainter>,
    ) -> usize {
        let clip = window.content_mask().bounds;
        let content_top = f32::from(clip.top() - origin.y);
        let content_bottom = f32::from(clip.bottom() - origin.y);
        if content_bottom <= content_top || clip.size.width <= px(0.) {
            return 0;
        }
        let first = self.block_at_y(content_top).unwrap_or(0);
        let last = self
            .block_at_y(content_bottom)
            .unwrap_or(self.blocks.len().saturating_sub(1));
        let quads = selection
            .filter(|selection| !selection.is_empty())
            .map(|selection| self.selection_quads(selection))
            .unwrap_or_default();
        let visible = Bounds::new(
            point(clip.left() - origin.x, px(content_top)),
            size(clip.size.width, px((content_bottom - content_top).max(0.))),
        );
        let mut painted = 0usize;
        let dim_color = self.style.background.opacity(0.65);
        for index in first..=last {
            if self.blocks.is_empty() {
                break;
            }
            let block_top = self.block_top(index);
            let block_height = self.heights.values[index] as f32;
            if block_top > content_bottom {
                break;
            }
            if block_top + block_height < content_top {
                continue;
            }
            self.paint_chrome(window, cx, origin, index, block_top);
            let decoration_quads: Vec<(Bounds<Pixels>, Hsla)> = decorations
                .iter()
                .filter(|decoration| decoration.block == index)
                .flat_map(|decoration| {
                    self.block_range_quads(index, decoration.range.clone())
                        .into_iter()
                        .map(move |quad| (quad, decoration.color))
                })
                .collect();
            if let Some(spec) = self.blocks[index].image.clone() {
                let bounds = Bounds::new(
                    point(origin.x, origin.y + px(block_top)),
                    size(px(spec.width), px(spec.height)),
                );
                let painted_image =
                    images.is_some_and(|painter| painter.paint(&spec.url, bounds, window));
                if painted_image {
                    painted += 1;
                } else {
                    let placeholder = bounds.intersect(&visible);
                    if placeholder.size.width > px(0.) && placeholder.size.height > px(0.) {
                        window.paint_quad(fill(placeholder, self.style.code_background));
                    }
                    // Until the picture loads, its alt text names the block.
                    if self.blocks[index].marker_line.is_none() {
                        let alt = self.blocks[index].text.clone();
                        if !alt.is_empty() {
                            let font = self.style.font.clone();
                            let color = self.style.muted_foreground;
                            let run = ResolvedRun {
                                range: 0..0,
                                font,
                                color,
                                background: None,
                                strikethrough: false,
                            };
                            let line = window.text_system().shape_line(
                                alt.clone(),
                                self.style.font_size,
                                &[run.text_run(alt.len())],
                                None,
                            );
                            self.blocks[index].marker_line = Some(line);
                        }
                    }
                    if let Some(line) = self.blocks[index].marker_line.as_ref() {
                        let _ = line.paint(
                            point(origin.x + px(12.), origin.y + px(block_top + 12.)),
                            px(f32::from(self.style.font_size) * self.style.line_height),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                }
            }
            let chrome_top = self.blocks[index].chrome_top;
            let has_image = self.blocks[index].image.is_some();
            let cell_count = self.blocks[index].cells.len();
            let active_sentence = focus.filter(|sentence| sentence.block == index);
            if has_image {
                // Painted above; the alt text only exists for source mapping.
            } else if cell_count > 0 {
                for cell_index in 0..cell_count {
                    let cell = &self.blocks[index].cells[cell_index];
                    let cell_start = self.blocks[index].cell_range(cell_index).start;
                    let cell_focus = active_sentence.map(|sentence| {
                        sentence.range.start.saturating_sub(cell_start)
                            ..sentence.range.end.saturating_sub(cell_start)
                    });
                    painted += paint_block_rows(
                        cell,
                        origin,
                        block_top + chrome_top,
                        &quads,
                        &decoration_quads,
                        &visible,
                        content_top,
                        content_bottom,
                        self.style.selection,
                        cell_focus.as_ref().map(|range| (range, dim_color)),
                        window,
                        cx,
                    );
                }
            } else {
                painted += paint_block_rows(
                    &self.blocks[index],
                    origin,
                    block_top,
                    &quads,
                    &decoration_quads,
                    &visible,
                    content_top,
                    content_bottom,
                    self.style.selection,
                    active_sentence.map(|sentence| (&sentence.range, dim_color)),
                    window,
                    cx,
                );
            }
            if focus.is_some() && active_sentence.is_none() {
                let bounds = Bounds::new(
                    point(origin.x, origin.y + px(block_top)),
                    size(self.width, px(block_height)),
                )
                .intersect(&clip);
                if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                    window.paint_quad(fill(bounds, dim_color));
                }
            }
        }
        if let Some(hit) = caret
            && let Some(caret) = self.caret(hit)
        {
            let bounds = Bounds::new(
                point(origin.x + caret.origin.x, origin.y + caret.origin.y),
                size(px(1.5), caret.height),
            );
            if bounds.intersects(&Bounds::new(
                origin,
                size(clip.size.width, px(content_bottom - content_top)),
            )) {
                window.paint_quad(fill(bounds, caret.color));
            }
        }
        painted
    }

    fn paint_chrome(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        origin: Point<Pixels>,
        index: usize,
        block_top: f32,
    ) {
        let width = f32::from(self.width);
        let code_background = self.style.code_background;
        let border = self.style.border;
        let style_font = self.style.font.clone();
        let muted = self.style.muted_foreground;
        let block = &mut self.blocks[index];
        let rows_height = block.rows.len() as f32 * block.row_height;
        match block.kind {
            BlockKind::Code { .. } => {
                let bounds = Bounds::new(
                    point(origin.x, origin.y + px(block_top)),
                    size(
                        px(width),
                        px(block.chrome_top + rows_height + block.chrome_bottom),
                    ),
                );
                window.paint_quad(fill(bounds, code_background));
            }
            BlockKind::Rule => {
                let bounds = Bounds::new(
                    point(origin.x, origin.y + px(block_top + block.chrome_top)),
                    size(px(width), px(1.)),
                );
                window.paint_quad(fill(bounds, border));
            }
            BlockKind::Quote => {
                let bounds = Bounds::new(
                    point(origin.x, origin.y + px(block_top + block.chrome_top)),
                    size(px(2.), px(rows_height)),
                );
                window.paint_quad(fill(bounds, border));
            }
            BlockKind::ListItem { .. } => {
                if block.marker_line.is_none()
                    && let Some(marker) = block.marker.clone()
                {
                    let run = ResolvedRun {
                        range: 0..0,
                        font: style_font,
                        color: muted,
                        background: None,
                        strikethrough: false,
                    };
                    block.marker_line = Some(window.text_system().shape_line(
                        marker.clone(),
                        px(block.font_size),
                        &[run.text_run(marker.len())],
                        None,
                    ));
                }
                if let Some(line) = block.marker_line.as_ref() {
                    let marker_origin = point(
                        origin.x + px(block.marker_offset),
                        origin.y + px(block_top + block.chrome_top),
                    );
                    let _ = line.paint(
                        marker_origin,
                        px(block.row_height),
                        TextAlign::Right,
                        Some(px(18.)),
                        window,
                        cx,
                    );
                }
            }
            BlockKind::Footnote { .. } => {
                if block.marker_line.is_none()
                    && let Some(marker) = block.marker.clone()
                {
                    let run = ResolvedRun {
                        range: 0..0,
                        font: style_font,
                        color: muted,
                        background: None,
                        strikethrough: false,
                    };
                    block.marker_line = Some(window.text_system().shape_line(
                        marker.clone(),
                        px(block.font_size),
                        &[run.text_run(marker.len())],
                        None,
                    ));
                }
                if let Some(line) = block.marker_line.as_ref() {
                    let marker_origin = point(
                        origin.x + px(block.marker_offset),
                        origin.y + px(block_top + block.chrome_top),
                    );
                    let _ = line.paint(
                        marker_origin,
                        px(block.row_height),
                        TextAlign::Left,
                        Some(px(18.)),
                        window,
                        cx,
                    );
                }
            }
            BlockKind::Raw => {
                let bounds = Bounds::new(
                    point(origin.x, origin.y + px(block_top + block.chrome_top)),
                    size(px(2.), px(rows_height)),
                );
                window.paint_quad(fill(bounds, border));
            }
            BlockKind::TableRow { header, .. } => {
                let row_width = f32::from(self.width);
                let row_height = (block.height() - block.gap).max(1.);
                if header {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(origin.x, origin.y + px(block_top)),
                            size(px(row_width), px(row_height)),
                        ),
                        code_background,
                    ));
                }
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x, origin.y + px(block_top + row_height - 1.)),
                        size(px(row_width), px(1.)),
                    ),
                    border,
                ));
                for cell in block.cells.iter().skip(1) {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                origin.x + px(cell.x_offset - TABLE_CELL_PADDING * 0.5),
                                origin.y + px(block_top),
                            ),
                            size(px(1.), px(row_height)),
                        ),
                        border,
                    ));
                }
            }
            BlockKind::Image { .. } | BlockKind::Heading(_) | BlockKind::Paragraph => {}
        }
    }

    /// Rows discovered so far, across every block.
    pub(crate) fn measured_rows(&self) -> usize {
        self.blocks.iter().map(|block| block.rows.len()).sum()
    }

    /// Bytes of one block that still need wrapping.
    pub(crate) fn pending_bytes(&self, block: usize) -> usize {
        self.blocks.get(block).map_or(0, |block| {
            if block.is_table() {
                return block
                    .cells
                    .iter()
                    .map(|cell| cell.text_len.saturating_sub(cell.scanned))
                    .sum();
            }
            if block.complete || block.image.is_some() {
                0
            } else {
                block.text_len.saturating_sub(block.scanned)
            }
        })
    }
}

fn row_index_at(block: &BlockLayout, local_y: f32) -> usize {
    if local_y <= 0. {
        0
    } else {
        (local_y / block.row_height).floor() as usize
    }
}

/// Paint every shaped row of one block slice (a whole block or one cell).
#[allow(clippy::too_many_arguments)]
fn paint_block_rows(
    block: &BlockLayout,
    origin: Point<Pixels>,
    top: f32,
    quads: &[Bounds<Pixels>],
    decorations: &[(Bounds<Pixels>, Hsla)],
    visible: &Bounds<Pixels>,
    content_top: f32,
    content_bottom: f32,
    selection_color: Hsla,
    focus: Option<(&Range<usize>, Hsla)>,
    window: &mut Window,
    cx: &mut App,
) -> usize {
    let mut painted = 0usize;
    let first_row = row_index_at(block, content_top - top - block.chrome_top);
    for row in block.rows.iter().skip(first_row) {
        let row_y = top + block.chrome_top + row.y;
        if row_y > content_bottom {
            break;
        }
        let Some(shaped) = row.shaped.clone() else {
            continue;
        };
        let row_origin = point(
            origin.x + px(block.x_offset) + block.align_offset(shaped.width()),
            origin.y + px(row_y),
        );
        let row_bounds = Bounds::new(
            point(px(block.x_offset), px(row_y)),
            size(px(block.column_width), px(row.height)),
        );
        let _ = shaped.paint_background(
            row_origin,
            px(row.height),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        for (quad, color) in decorations
            .iter()
            .filter(|(quad, _)| quad.intersects(&row_bounds))
        {
            let clipped = quad.intersect(&row_bounds).intersect(visible);
            if clipped.size.width > px(0.) && clipped.size.height > px(0.) {
                window.paint_quad(fill(clipped, *color));
            }
        }
        for quad in quads.iter().filter(|quad| quad.intersects(&row_bounds)) {
            let clipped = quad.intersect(&row_bounds).intersect(visible);
            if clipped.size.width > px(0.) && clipped.size.height > px(0.) {
                window.paint_quad(fill(clipped, selection_color.opacity(0.35)));
            }
        }
        let _ = shaped.paint(
            row_origin,
            px(row.height),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        if let Some((sentence, color)) = focus {
            let start = sentence.start.max(row.range.start);
            let end = sentence.end.min(row.range.end);
            let width = shaped.width();
            let (left, right) = if start >= end {
                (width, width)
            } else {
                let a = shaped.x_for_index(start - row.range.start);
                let b = shaped.x_for_index(end - row.range.start);
                (a.min(b).clamp(px(0.), width), a.max(b).clamp(px(0.), width))
            };
            for span in [px(0.)..left, right..width] {
                if span.start < span.end {
                    let bounds = Bounds::new(
                        point(row_origin.x + span.start, row_origin.y),
                        size(span.end - span.start, px(row.height)),
                    );
                    window.paint_quad(fill(bounds, color));
                }
            }
        }
        painted += 1;
    }
    painted
}

/// Shape one row that is not yet shaped.
fn shape_row_in(
    block: &mut BlockLayout,
    text: &str,
    row_index: usize,
    text_system: &WindowTextSystem,
) -> (bool, usize) {
    if block.rows[row_index].shaped.is_some() {
        return (false, 0);
    }
    let shaped = {
        let row = &block.rows[row_index];
        shape_row(block, text, row, text_system)
    };
    let bytes = block.rows[row_index].range.len();
    block.rows[row_index].shaped = Some(shaped);
    (true, bytes)
}

/// Shape every row intersecting the viewport, under the frame budget.
#[allow(clippy::too_many_arguments)]
fn shape_rows_in(
    block: &mut BlockLayout,
    text: &str,
    block_top: f32,
    content_top: f32,
    content_bottom: f32,
    work: &ViewportWork<'_>,
    pending: &mut bool,
) -> (usize, usize, usize) {
    let rows = block.rows.len();
    let first_local = content_top - block_top - block.chrome_top;
    let first_row = if first_local <= 0. {
        0
    } else {
        ((first_local / block.row_height).floor() as usize).min(rows)
    };
    let mut reused = 0usize;
    let mut shaped = 0usize;
    let mut bytes = 0usize;
    for row_index in first_row..rows {
        let (y, height, has_shaped, range_len) = {
            let row = &block.rows[row_index];
            (
                block_top + block.chrome_top + row.y,
                row.height,
                row.shaped.is_some(),
                row.range.len(),
            )
        };
        if y + height < content_top {
            continue;
        }
        if y > content_bottom {
            break;
        }
        if has_shaped {
            reused += 1;
            continue;
        }
        if Instant::now() >= work.deadline {
            *pending = true;
            break;
        }
        let (did_shape, shaped_bytes) = shape_row_in(block, text, row_index, work.text_system);
        if did_shape {
            shaped += 1;
            bytes += shaped_bytes;
        }
        let _ = range_len;
    }
    (reused, shaped, bytes)
}

/// Shape one visual row, keeping shaping context from the same text line.
fn shape_row(
    block: &BlockLayout,
    text: &str,
    row: &Row,
    text_system: &WindowTextSystem,
) -> ShapedLine {
    let mut context_start = row.range.start.saturating_sub(LINE_CONTEXT_BYTES);
    while context_start > 0 && !text.is_char_boundary(context_start) {
        context_start -= 1;
    }
    if let Some(newline) = text[..row.range.start].rfind('\n') {
        context_start = context_start.max(newline + 1);
    }
    // The context must end on a grapheme boundary too; otherwise shaping the
    // slice would split a cluster and the split would not align to a glyph.
    if context_start < row.range.start {
        let mut boundary = context_start;
        for (offset, _) in text[context_start..row.range.start].grapheme_indices(true) {
            let absolute = context_start + offset;
            if absolute <= row.range.start {
                boundary = absolute;
            } else {
                break;
            }
        }
        context_start = boundary;
    }
    let slice = &text[context_start..row.range.end];
    let runs = block.runs_for(context_start..row.range.end);
    let shaped = text_system.shape_line(
        SharedString::from(slice.to_owned()),
        px(block.font_size),
        &runs,
        None,
    );
    let (_, line) = shaped.split_at(row.range.start - context_start);
    line
}

/// Fenwick tree over block heights.
///
/// Width/scroll edits update one block's height; the tree keeps block-position
/// queries cheap without rebuilding a prefix sum over every block.
struct HeightIndex {
    values: Vec<f64>,
    tree: Vec<f64>,
}

impl HeightIndex {
    fn new(len: usize) -> Self {
        Self {
            values: vec![0.; len],
            tree: vec![0.; len + 1],
        }
    }

    fn set(&mut self, index: usize, value: f64) {
        if index >= self.values.len() {
            return;
        }
        let delta = value - self.values[index];
        if delta == 0. {
            return;
        }
        self.values[index] = value;
        let mut cursor = index + 1;
        while cursor <= self.values.len() {
            self.tree[cursor] += delta;
            cursor += cursor.isolate_lowest_one();
        }
    }

    fn prefix(&self, count: usize) -> f64 {
        let mut cursor = count.min(self.values.len());
        let mut sum = 0.;
        while cursor > 0 {
            sum += self.tree[cursor];
            cursor -= cursor.isolate_lowest_one();
        }
        sum
    }

    fn total(&self) -> f64 {
        self.prefix(self.values.len())
    }

    fn block_at(&self, y: f64) -> Option<usize> {
        let len = self.values.len();
        if len == 0 {
            return None;
        }
        if y < 0. {
            return Some(0);
        }
        if y >= self.total() {
            return Some(len - 1);
        }
        let mut index = 0usize;
        let mut remaining = y;
        let mut bit = 1usize << (usize::BITS - 1 - len.leading_zeros());
        while bit > 0 {
            let next = index + bit;
            if next <= len && self.tree[next] <= remaining {
                index = next;
                remaining -= self.tree[next];
            }
            bit >>= 1;
        }
        Some(index.min(len - 1))
    }
}
