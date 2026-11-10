//! Rendered writer model with a lossless map back to Markdown source.
//!
//! The writing surface shows typography (headings, emphasis, code, links,
//! lists and quotes) instead of Markdown punctuation, but every visible
//! character still belongs to the authoritative source. This module owns the
//! pure half of that contract:
//!
//! - [`WysiwygDocument::parse`] turns the writer projection into blocks and
//!   rendered inline text, hiding only syntax the reader should not see;
//! - every rendered run keeps the exact source byte range it came from, so a
//!   caret or selection can be mapped in both directions;
//! - the parser never rewrites or normalizes the source: unsupported or
//!   ambiguous constructs stay visible as plain text.
//!
//! The UI layer (`astrolith-gpui`) builds GPUI Kit block nodes from this model
//! and uses the ranges for hit-testing, selection and caret placement.

use std::ops::Range;

/// One rendered inline mark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineMarkKind {
    /// Strong emphasis.
    Bold,
    /// Emphasis.
    Italic,
    /// Strikethrough.
    Strikethrough,
    /// Inline code span.
    Code,
    /// Link text, with its destination.
    Link(String),
    /// Image alt text, with its destination.
    Image(String),
}

/// A mark over a rendered byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineMark {
    /// Rendered byte range.
    pub range: Range<usize>,
    /// What the range means.
    pub kind: InlineMarkKind,
}

/// A rendered byte range paired with the source bytes it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSegment {
    /// Byte range in [`RichText::text`].
    pub rendered: Range<usize>,
    /// Byte range in the document's visible text.
    pub source: Range<usize>,
    /// Whether rendered bytes map one-to-one onto source bytes.
    ///
    /// False for escapes and other decoded runs, where only the whole segment
    /// has a meaningful source range.
    pub linear: bool,
}

/// Rendered inline text plus its source mapping and marks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    /// Text exactly as the writing surface renders it.
    pub text: String,
    /// Inline marks over [`Self::text`], in document order.
    pub marks: Vec<InlineMark>,
    /// Non-overlapping mapping segments, in rendered order.
    pub segments: Vec<SourceSegment>,
}

impl RichText {
    /// Append `other`, shifting its rendered ranges into this text.
    pub fn append(&mut self, other: RichText) {
        let offset = self.text.len();
        for mark in other.marks {
            self.marks.push(InlineMark {
                range: offset + mark.range.start..offset + mark.range.end,
                kind: mark.kind,
            });
        }
        for segment in other.segments {
            self.segments.push(SourceSegment {
                rendered: offset + segment.rendered.start..offset + segment.rendered.end,
                source: segment.source,
                linear: segment.linear,
            });
        }
        self.text.push_str(&other.text);
    }

    /// Append one literal newline that came from `source`.
    fn push_newline(&mut self, source: Range<usize>) {
        self.push_run("\n", source, false);
    }

    /// Append `text` and merge adjacent runs with the same source continuity.
    fn push_run(&mut self, text: &str, source: Range<usize>, linear: bool) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        if let Some(last) = self.segments.last_mut()
            && last.linear == linear
            && last.rendered.end == start
            && last.source.end == source.start
        {
            last.rendered.end = end;
            last.source.end = source.end;
            return;
        }
        self.segments.push(SourceSegment {
            rendered: start..end,
            source,
            linear,
        });
    }

    /// Add a mark over the already-rendered range. Images retain their
    /// destination even when they have no rendered alt text.
    fn mark(&mut self, range: Range<usize>, kind: InlineMarkKind) {
        if range.start < range.end || matches!(kind, InlineMarkKind::Image(_)) {
            self.marks.push(InlineMark { range, kind });
        }
    }

    /// The rendered offset for a source offset, or `None` when the source
    /// bytes render nothing (Markdown punctuation, fences, link destinations).
    pub fn rendered_offset(&self, source: usize) -> Option<usize> {
        for segment in &self.segments {
            if segment.source.contains(&source) {
                if !segment.linear {
                    return Some(segment.rendered.start);
                }
                let offset = source.min(segment.source.end) - segment.source.start;
                return Some((segment.rendered.start + offset).min(segment.rendered.end));
            }
        }
        None
    }

    /// The rendered offset nearest to a source offset.
    ///
    /// A caret in hidden syntax is drawn at the next visible character, or at
    /// the end of this text when there is none.
    pub fn nearest_rendered_offset(&self, source: usize) -> usize {
        if let Some(offset) = self.rendered_offset(source) {
            return offset;
        }
        for segment in &self.segments {
            if source <= segment.source.start {
                return segment.rendered.start;
            }
        }
        self.text.len()
    }

    /// The source offset for a rendered offset.
    pub fn source_offset(&self, rendered: usize) -> Option<usize> {
        for segment in &self.segments {
            if segment.rendered.start <= rendered && rendered < segment.rendered.end {
                if !segment.linear {
                    return Some(segment.source.start);
                }
                let offset = rendered - segment.rendered.start;
                return Some((segment.source.start + offset).min(segment.source.end));
            }
        }
        // A boundary belongs to the run that starts there, which is exactly
        // the end of the last run when there is no next run.
        self.segments
            .last()
            .filter(|segment| rendered == segment.rendered.end)
            .map(|segment| segment.source.end)
    }

    /// The source range covered by a rendered range, or `None` when the range
    /// covers no visible source bytes.
    pub fn source_range(&self, rendered: Range<usize>) -> Option<Range<usize>> {
        let mut found: Option<Range<usize>> = None;
        for segment in &self.segments {
            let start = segment.rendered.start.max(rendered.start);
            let end = segment.rendered.end.min(rendered.end);
            if start >= end {
                continue;
            }
            let (source_start, source_end) = if segment.linear {
                (
                    segment.source.start + (start - segment.rendered.start),
                    segment.source.start + (end - segment.rendered.start),
                )
            } else {
                (segment.source.start, segment.source.end)
            };
            found = Some(match found {
                Some(range) => range.start.min(source_start)..range.end.max(source_end),
                None => source_start..source_end,
            });
        }
        found
    }

    /// Build a linearly mapped text from a source slice.
    fn linear(source: &str, range: Range<usize>) -> Self {
        let mut text = Self::default();
        text.push_run(&source[range.clone()], range, true);
        text
    }
}

/// Horizontal alignment of one table column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnAlign {
    /// Default alignment.
    Left,
    /// Centered cells.
    Center,
    /// Trailing alignment.
    Right,
}

/// What a block is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// ATX heading, level 1–6.
    Heading(u8),
    /// Body paragraph.
    Paragraph,
    /// One list item; `ordered` items carry their number.
    ListItem {
        /// Whether the list uses ordinals.
        ordered: bool,
        /// The number rendered for the item.
        number: u32,
    },
    /// Blockquote content.
    Quote,
    /// Fenced code block.
    Code {
        /// The fence's info string's first word.
        language: String,
    },
    /// Thematic break; it has no editable text.
    Rule,
    /// One GFM pipe-table row. Cells live in [`RichBlock::cells`].
    TableRow {
        /// Whether this is the header row.
        header: bool,
        /// Alignment of every column.
        align: Vec<ColumnAlign>,
    },
    /// A footnote definition; the label is drawn as a marker.
    Footnote {
        /// The label after `[^`.
        label: String,
    },
    /// Raw HTML or MDX flow content, shown without interpretation.
    Raw,
    /// A standalone image; the reader sees the picture, the writer its alt
    /// text.
    Image {
        /// Destination as written in the Markdown source.
        url: String,
    },
}

/// One rendered block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RichBlock {
    /// What the block is.
    pub kind: BlockKind,
    /// Full block bytes, Markdown syntax included.
    pub source: Range<usize>,
    /// Editable content bytes, syntax excluded.
    pub content: Range<usize>,
    /// Rendered content and its source mapping.
    pub text: RichText,
    /// Table cells in column order; empty for every other block kind.
    pub cells: Vec<RichText>,
    /// Indentation level for quotes and lists.
    pub depth: usize,
}

/// A parsed writer document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WysiwygDocument {
    /// Rendered blocks in document order.
    pub blocks: Vec<RichBlock>,
}

/// One applied incremental parse: the document plus how many blocks were
/// reused unchanged from the previous revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncrementalParse {
    /// The reparsed document.
    pub document: WysiwygDocument,
    /// Blocks carried over from the previous document.
    pub reused_blocks: usize,
}

/// One source line.
struct Line {
    start: usize,
    content_end: usize,
    end: usize,
}

impl WysiwygDocument {
    /// Parse the writer projection into rendered blocks.
    ///
    /// Blank lines separate blocks and are not rendered; all other bytes stay
    /// represented either in a block's text or in the block's source range.
    pub fn parse(source: &str) -> Self {
        Self::parse_cancellable(source, || false).unwrap_or_default()
    }

    /// Map each source line directly to an editable block with its Markdown visible.
    pub fn literal_cancellable(source: &str, mut cancelled: impl FnMut() -> bool) -> Option<Self> {
        let mut blocks = Vec::new();
        for line in lines(source) {
            if cancelled() {
                return None;
            }
            let content = line.start..line.content_end;
            blocks.push(RichBlock {
                kind: BlockKind::Paragraph,
                source: line.start..line.end,
                text: RichText::linear(source, content.clone()),
                content,
                cells: Vec::new(),
                depth: 0,
            });
        }
        append_trailing_paragraph(source, &mut blocks);
        Some(Self { blocks })
    }

    /// Parse a background snapshot, stopping between blocks when obsolete.
    /// Returns `None` rather than exposing a partially mapped document.
    /// A single block's parsing is not interrupted midway.
    pub fn parse_cancellable(source: &str, mut cancelled: impl FnMut() -> bool) -> Option<Self> {
        if cancelled() {
            return None;
        }
        let lines = lines(source);
        let definitions = reference_definitions(source, &lines);
        let mut blocks = parse_block_range(source, &lines, &definitions, 0, None, &mut cancelled)?;
        append_trailing_paragraph(source, &mut blocks);
        if cancelled() {
            return None;
        }
        Some(Self { blocks })
    }

    /// Reparse only the blocks around one replacement, reusing every block
    /// whose source bytes did not change.
    ///
    /// Returns `None` when the edit cannot be applied locally — a class change
    /// such as an inserted fence that would swallow following blocks, or a
    /// touched reference definition that can re-resolve links everywhere — so
    /// the caller can fall back to [`Self::parse_cancellable`]. `old` and `new`
    /// are the replaced and replacement ranges reported by the same diff; both
    /// share their start offset. A single huge block still re-parses as a whole.
    pub fn parse_incremental_cancellable(
        source: &str,
        previous_source: &str,
        previous: &WysiwygDocument,
        old: Range<usize>,
        new: Range<usize>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Option<IncrementalParse> {
        if cancelled() {
            return None;
        }
        let blocks = &previous.blocks;
        if blocks.is_empty()
            || old.start != new.start
            || old.start > old.end
            || new.start > new.end
            || old.end > previous_source.len()
            || new.end > source.len()
        {
            return None;
        }
        // A reference definition can resolve links in any block, so an edit
        // that touches one keeps the whole-document path.
        if touches_reference_definition(previous_source, &old)
            || touches_reference_definition(source, &new)
        {
            return None;
        }
        let delta = source.len() as isize - previous_source.len() as isize;
        let first = blocks.partition_point(|block| block.source.end <= old.start);
        let start_block = extend_table_back(blocks, first.saturating_sub(1));
        let end_block = extend_table_forward(
            blocks,
            blocks
                .partition_point(|block| block.source.start < old.end)
                .min(blocks.len() - 1),
        )
        .max(start_block);
        let anchor_start = blocks[start_block].source.start;
        let anchor_end = if end_block + 1 == blocks.len() {
            previous_source.len()
        } else {
            blocks[end_block].source.end
        };
        if anchor_start > old.start || anchor_end < old.end {
            return None;
        }
        let segment_start = anchor_start;
        let segment_end = if anchor_end == previous_source.len() {
            source.len()
        } else {
            (anchor_end as isize + delta) as usize
        };
        if segment_end > source.len() || segment_end < new.end {
            return None;
        }

        let lines = lines(source);
        let definitions = reference_definitions(source, &lines);
        let start_line = lines.partition_point(|line| line.start < segment_start);
        let stop = (segment_end < source.len()).then_some(segment_end);
        let segment = parse_block_range(
            source,
            &lines,
            &definitions,
            start_line,
            stop,
            &mut cancelled,
        )?;

        // A block unit may extend past the end anchor (a table row batch, an
        // inserted fence, a merged paragraph); every old block it overlaps is
        // superseded. Everything starting after its end is still reusable.
        let segment_last_end = segment
            .last()
            .map_or(segment_start, |block| block.source.end);
        let suffix_start = if segment_end == source.len() || segment_last_end >= source.len() {
            blocks.len()
        } else {
            blocks
                .partition_point(|block| {
                    block.source.start < (segment_last_end as isize - delta) as usize
                })
                .max(end_block + 1)
        };
        let suffix = &blocks[suffix_start..];
        let mut parsed_blocks = Vec::with_capacity(start_block + segment.len() + suffix.len());
        parsed_blocks.extend(blocks[..start_block].iter().cloned());
        parsed_blocks.extend(segment);
        for block in suffix {
            if cancelled() {
                return None;
            }
            parsed_blocks.push(shift_block(block.clone(), delta));
        }
        if segment_end == source.len() || segment_last_end >= source.len() {
            append_trailing_paragraph(source, &mut parsed_blocks);
        }
        let reused_blocks = start_block + suffix.len();
        Some(IncrementalParse {
            document: Self {
                blocks: parsed_blocks,
            },
            reused_blocks,
        })
    }

    /// The block that owns `source`, or the nearest block by distance.
    pub fn block_index(&self, source: usize) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        for (index, block) in self.blocks.iter().enumerate() {
            if block.content.start <= source && source <= block.content.end {
                return Some(index);
            }
        }
        for (index, block) in self.blocks.iter().enumerate() {
            if block.source.contains(&source) {
                return Some(index);
            }
        }
        let mut nearest = 0;
        let mut distance = usize::MAX;
        for (index, block) in self.blocks.iter().enumerate() {
            let block_distance = if source < block.content.start {
                block.content.start - source
            } else {
                source.saturating_sub(block.content.end)
            };
            if block_distance < distance {
                distance = block_distance;
                nearest = index;
            }
        }
        Some(nearest)
    }

    /// Map a source offset to `(block index, rendered offset)`.
    pub fn caret_location(&self, source: usize) -> Option<(usize, usize)> {
        let index = self.block_index(source)?;
        let rendered = self.blocks[index].text.nearest_rendered_offset(source);
        Some((index, rendered))
    }

    /// The source offset for a rendered offset in one block.
    pub fn source_at(&self, block: usize, rendered: usize) -> Option<usize> {
        self.blocks.get(block)?.text.source_offset(rendered)
    }

    /// The link destination whose rendered text contains `block`/`rendered`.
    pub fn link_at(&self, block: usize, rendered: usize) -> Option<&str> {
        self.blocks
            .get(block)?
            .text
            .marks
            .iter()
            .find_map(|mark| match &mark.kind {
                InlineMarkKind::Link(url)
                    if mark.range.start <= rendered && rendered < mark.range.end =>
                {
                    Some(url.as_str())
                }
                _ => None,
            })
    }

    /// The image destination whose rendered alt text contains the position.
    pub fn image_at(&self, block: usize, rendered: usize) -> Option<&str> {
        self.blocks
            .get(block)?
            .text
            .marks
            .iter()
            .find_map(|mark| match &mark.kind {
                InlineMarkKind::Image(url)
                    if mark.range.start <= rendered && rendered < mark.range.end =>
                {
                    Some(url.as_str())
                }
                _ => None,
            })
    }
}

/// Split `source` into lines with exact byte ranges.
fn lines(source: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0;
    for part in source.split_inclusive('\n') {
        let end = start + part.len();
        let mut content_end = end;
        if part.ends_with('\n') {
            content_end -= 1;
            if content_end > start && source.as_bytes()[content_end - 1] == b'\r' {
                content_end -= 1;
            }
        }
        lines.push(Line {
            start,
            content_end,
            end,
        });
        start = end;
    }
    if source.is_empty() {
        lines.push(Line {
            start: 0,
            content_end: 0,
            end: 0,
        });
    }
    lines
}

/// Parse blocks from `start`, stopping once a parsed block ends at or past the
/// byte `stop` (or at the document end when `None`).
///
/// The block containing `stop` is parsed whole, so a unit such as a table or a
/// fenced block is never cut in half. Cancellation is checked between blocks;
/// one block is never interrupted midway.
fn parse_block_range(
    source: &str,
    lines: &[Line],
    definitions: &Definitions,
    start: usize,
    stop: Option<usize>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<Vec<RichBlock>> {
    let mut blocks = Vec::new();
    let mut index = start;
    while index < lines.len() {
        if cancelled() {
            return None;
        }
        if let Some(stop) = stop {
            if blocks
                .last()
                .is_some_and(|block: &RichBlock| block.source.end >= stop)
            {
                break;
            }
            if lines[index].start >= stop {
                break;
            }
        }
        let line = &lines[index];
        let content = &source[line.start..line.content_end];
        if content.trim().is_empty() {
            index += 1;
            continue;
        }
        let indent = content.len() - content.trim_start_matches(' ').len();
        let rest = &content[indent..];
        if indent <= 3 {
            if let Some((block, next)) = footnote_definition(source, lines, index, definitions) {
                blocks.push(block);
                index = next;
                continue;
            }
            if reference_definition(rest).is_some() {
                index += 1;
                continue;
            }
            if let Some((mut rows, next)) = table(source, lines, index, definitions) {
                blocks.append(&mut rows);
                index = next;
                continue;
            }
            if let Some((block, next)) = raw_flow(source, lines, index) {
                blocks.push(block);
                index = next;
                continue;
            }
            if let Some(block) = fenced_code(source, lines, index, line, rest) {
                index = block.next;
                blocks.push(block.block);
                continue;
            }
            if let Some((level, content_start)) = heading(rest, line, indent) {
                let content_end = trim_end(source, content_start, line.content_end);
                blocks.push(RichBlock {
                    kind: BlockKind::Heading(level),
                    source: line.start..line.end,
                    content: content_start..content_end,
                    text: inline(source, content_start..content_end, definitions),
                    cells: Vec::new(),
                    depth: 0,
                });
                index += 1;
                continue;
            }
            if is_rule(rest) {
                blocks.push(RichBlock {
                    kind: BlockKind::Rule,
                    source: line.start..line.end,
                    content: line.start..line.start,
                    text: RichText::default(),
                    cells: Vec::new(),
                    depth: 0,
                });
                index += 1;
                continue;
            }
            if rest.starts_with('>') {
                let (block, next) = blockquote(source, lines, index, indent, definitions);
                blocks.push(block);
                index = next;
                continue;
            }
            if let Some((marker, content_start, ordered, number)) = list_marker(rest, line, indent)
            {
                let content_end = trim_end(source, content_start, line.content_end);
                blocks.push(RichBlock {
                    kind: BlockKind::ListItem { ordered, number },
                    source: line.start..line.end,
                    content: content_start..content_end,
                    text: inline(source, content_start..content_end, definitions),
                    cells: Vec::new(),
                    depth: indent / 2,
                });
                let _ = marker;
                index += 1;
                continue;
            }
        }
        let (block, next) = paragraph(source, lines, index, indent, definitions);
        blocks.push(block);
        index = next;
    }
    Some(blocks)
}

/// Keep the empty line opened by a trailing newline addressable, so pressing
/// Enter at the end of the document puts the caret on a new line instead of at
/// the end of the previous block.
fn append_trailing_paragraph(source: &str, blocks: &mut Vec<RichBlock>) {
    if source.ends_with('\n')
        && blocks
            .last()
            .is_none_or(|block| block.content.end < source.len())
    {
        blocks.push(RichBlock {
            kind: BlockKind::Paragraph,
            source: source.len()..source.len(),
            content: source.len()..source.len(),
            text: RichText::default(),
            cells: Vec::new(),
            depth: 0,
        });
    }
}

/// Widen a block index to the first row of its pipe table.
fn extend_table_back(blocks: &[RichBlock], mut index: usize) -> usize {
    while index > 0
        && matches!(blocks[index].kind, BlockKind::TableRow { .. })
        && matches!(blocks[index - 1].kind, BlockKind::TableRow { .. })
    {
        index -= 1;
    }
    index
}

/// Widen a block index to the last row of its pipe table.
fn extend_table_forward(blocks: &[RichBlock], mut index: usize) -> usize {
    while index + 1 < blocks.len()
        && matches!(blocks[index].kind, BlockKind::TableRow { .. })
        && matches!(blocks[index + 1].kind, BlockKind::TableRow { .. })
    {
        index += 1;
    }
    index
}

/// Move one reused block's source ranges by `delta` bytes.
///
/// Rendered offsets and marks are relative to the block's own text, so only
/// the source mapping moves.
fn shift_block(mut block: RichBlock, delta: isize) -> RichBlock {
    let shift = |range: Range<usize>| {
        let start = (range.start as isize + delta) as usize;
        let end = (range.end as isize + delta) as usize;
        start..end
    };
    block.source = shift(block.source);
    block.content = shift(block.content);
    for segment in &mut block.text.segments {
        segment.source = shift(segment.source.clone());
    }
    for cell in &mut block.cells {
        for segment in &mut cell.segments {
            segment.source = shift(segment.source.clone());
        }
    }
    block
}

/// Whether an edit range includes a line that is a link reference definition.
///
/// The span covers every line the replaced bytes touch plus the line at the
/// replacement end, so a definition created or destroyed by merging lines is
/// still detected. An unchanged definition elsewhere keeps its value.
fn touches_reference_definition(source: &str, range: &Range<usize>) -> bool {
    let start = range.start.min(source.len());
    let end = range.end.min(source.len());
    let from = source[..start].rfind('\n').map_or(0, |newline| newline + 1);
    let to = source[end..]
        .find('\n')
        .map_or(source.len(), |newline| end + newline);
    source[from..to].split('\n').any(|line| {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let indent = line.len() - line.trim_start_matches(' ').len();
        indent <= 3 && reference_definition(&line[indent..]).is_some()
    })
}

fn trim_end(source: &str, start: usize, mut end: usize) -> usize {
    while end > start && source.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

fn heading(rest: &str, line: &Line, indent: usize) -> Option<(u8, usize)> {
    let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &rest[hashes..];
    let separator = after.len() - after.trim_start_matches([' ', '\t']).len();
    if separator == 0 && !after.is_empty() {
        return None;
    }
    Some((hashes as u8, line.start + indent + hashes + separator))
}

fn is_rule(rest: &str) -> bool {
    let trimmed = rest.trim_end_matches([' ', '\t']);
    let Some(marker) = trimmed.chars().next() else {
        return false;
    };
    if !matches!(marker, '-' | '*' | '_') {
        return false;
    }
    let count = trimmed
        .chars()
        .filter(|character| *character == marker)
        .count();
    count >= 3
        && trimmed
            .chars()
            .all(|character| character == marker || character == ' ' || character == '\t')
}

fn list_marker(rest: &str, line: &Line, indent: usize) -> Option<(char, usize, bool, u32)> {
    let bytes = rest.as_bytes();
    match bytes.first()? {
        marker @ (b'-' | b'*' | b'+') => {
            let space = bytes[1..].iter().take_while(|byte| **byte == b' ').count();
            if space == 0 {
                return None;
            }
            Some((*marker as char, line.start + indent + 1 + space, false, 0))
        }
        byte if byte.is_ascii_digit() => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            let delimiter = *bytes.get(digits)?;
            if !matches!(delimiter, b'.' | b')') {
                return None;
            }
            let space = bytes[digits + 1..]
                .iter()
                .take_while(|byte| **byte == b' ')
                .count();
            if space == 0 {
                return None;
            }
            let number = rest[..digits].parse().ok()?;
            Some((
                delimiter as char,
                line.start + indent + digits + 1 + space,
                true,
                number,
            ))
        }
        _ => None,
    }
}

fn fenced_code(
    source: &str,
    lines: &[Line],
    index: usize,
    opening: &Line,
    rest: &str,
) -> Option<FencedBlock> {
    let marker = rest.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let run = rest
        .chars()
        .take_while(|character| *character == marker)
        .count();
    if run < 3 {
        return None;
    }
    let info = rest[run..].trim();
    let mut next = index + 1;
    let content_start = lines.get(next).map_or(source.len(), |line| line.start);
    let mut content_end = content_start;
    let mut block_end = opening.end;
    while let Some(line) = lines.get(next) {
        let text = &source[line.start..line.content_end];
        let trimmed = text.trim_start_matches(' ');
        let closing = trimmed.starts_with(marker)
            && trimmed
                .chars()
                .take_while(|character| *character == marker)
                .count()
                >= run
            && trimmed[run..].trim().is_empty();
        block_end = line.end;
        if closing {
            next += 1;
            break;
        }
        content_end = line.end;
        next += 1;
    }
    Some(FencedBlock {
        block: RichBlock {
            kind: BlockKind::Code {
                language: info.split_whitespace().next().unwrap_or("").to_owned(),
            },
            source: opening.start..block_end,
            content: content_start..content_end,
            text: RichText::linear(source, content_start..content_end),
            cells: Vec::new(),
            depth: 0,
        },
        next,
    })
}

struct FencedBlock {
    block: RichBlock,
    next: usize,
}

fn blockquote(
    source: &str,
    lines: &[Line],
    index: usize,
    indent: usize,
    definitions: &Definitions,
) -> (RichBlock, usize) {
    let mut next = index;
    let mut text = RichText::default();
    let mut content_start = 0;
    let mut content_end = 0;
    let mut first = true;
    let mut previous_end = 0;
    while let Some(line) = lines.get(next) {
        let content = &source[line.start..line.content_end];
        let spaces = content.len() - content.trim_start_matches(' ').len();
        let rest = content.get(spaces..).unwrap_or_default();
        if spaces > 3 || !rest.starts_with('>') {
            break;
        }
        let after_quote = &rest[1..];
        let gap = after_quote.len() - after_quote.trim_start_matches(' ').len();
        let start = line.start + spaces + 1 + gap;
        let end = trim_end(source, start, line.content_end);
        if first {
            content_start = start;
            first = false;
        } else {
            text.push_newline(previous_end..line.start);
        }
        text.append(inline(source, start..end, definitions));
        content_end = end;
        previous_end = line.end;
        next += 1;
    }
    let source_start = lines[index].start + indent;
    (
        RichBlock {
            kind: BlockKind::Quote,
            source: source_start..lines[next.saturating_sub(1)].end,
            content: content_start..content_end,
            text,
            cells: Vec::new(),
            depth: 1,
        },
        next,
    )
}

fn paragraph(
    source: &str,
    lines: &[Line],
    index: usize,
    indent: usize,
    definitions: &Definitions,
) -> (RichBlock, usize) {
    let mut next = index;
    let mut text = RichText::default();
    let mut content_start = 0;
    let mut content_end = 0;
    let mut first = true;
    let mut previous_end = 0;
    while let Some(line) = lines.get(next) {
        let content = &source[line.start..line.content_end];
        if content.trim().is_empty()
            || (next > index
                && (starts_block(source, line) || table_row_start(source, lines, next)))
        {
            break;
        }
        let start = line.start;
        let end = trim_end(source, start, line.content_end);
        let line_text = inline(source, start..end, definitions);
        if let Some(url) = standalone_image(&line_text) {
            if !first {
                break;
            }
            return (
                RichBlock {
                    kind: BlockKind::Image {
                        url: url.to_owned(),
                    },
                    source: start..line.end,
                    content: start + indent..end,
                    text: line_text,
                    cells: Vec::new(),
                    depth: 0,
                },
                next + 1,
            );
        }
        if first {
            content_start = start;
            first = false;
        } else {
            text.push_newline(previous_end..line.start);
        }
        text.append(line_text);
        content_end = end;
        previous_end = line.end;
        next += 1;
    }
    (
        RichBlock {
            kind: BlockKind::Paragraph,
            source: lines[index].start + indent..lines[next.saturating_sub(1)].end,
            content: content_start..content_end,
            text,
            cells: Vec::new(),
            depth: 0,
        },
        next,
    )
}

fn starts_block(source: &str, line: &Line) -> bool {
    let content = &source[line.start..line.content_end];
    if content.trim().is_empty() {
        return true;
    }
    let indent = content.len() - content.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let rest = &content[indent..];
    heading(rest, line, indent).is_some()
        || rest.starts_with('>')
        || is_rule(rest)
        || list_marker(rest, line, indent).is_some()
        || rest.starts_with("```")
        || rest.starts_with("~~~")
        || rest.starts_with('<')
        || footnote_marker(rest).is_some()
        || reference_definition(rest).is_some()
}

/// Link reference definitions collected from the document, keyed by their
/// normalized label.
type Definitions = std::collections::HashMap<String, String>;

/// An image on its own line becomes a picture block, even without blank lines.
fn standalone_image(rich: &RichText) -> Option<&str> {
    let text = &rich.text;
    let start = text.len() - text.trim_start().len();
    let end = text.trim_end().len();
    let mut images = rich.marks.iter().filter_map(|mark| match &mark.kind {
        InlineMarkKind::Image(url) => Some((&mark.range, url)),
        _ => None,
    });
    let (range, url) = images.next()?;
    (images.next().is_none() && range.start <= start && range.end >= end).then_some(url)
}

/// Whether `offset` sits after an odd number of backslashes.
fn is_escaped(text: &str, offset: usize) -> bool {
    let mut count = 0;
    let mut cursor = offset;
    while cursor > 0 && text.as_bytes()[cursor - 1] == b'\\' {
        count += 1;
        cursor -= 1;
    }
    count % 2 == 1
}

/// Split one table line into trimmed cell ranges, ignoring escaped pipes and
/// pipes inside code spans.
fn split_cells(source: &str, start: usize, end: usize) -> Vec<Range<usize>> {
    let text = &source[start..end];
    let mut cells = Vec::new();
    let mut cell_start = 0;
    let mut cursor = 0;
    let mut code_run = 0usize;
    while cursor < text.len() {
        let byte = text.as_bytes()[cursor];
        if byte == b'`' {
            let run = text.as_bytes()[cursor..]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            if code_run == 0 {
                code_run = run;
            } else if run == code_run {
                code_run = 0;
            }
            cursor += run;
            continue;
        }
        if byte == b'|' && code_run == 0 && !is_escaped(text, cursor) {
            cells.push(cell_start..cursor);
            cell_start = cursor + 1;
        }
        cursor += 1;
    }
    cells.push(cell_start..text.len());
    // A leading or trailing pipe delimits the row instead of creating an
    // empty cell.
    if cells.first().is_some_and(|range| range.is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|range| range.is_empty()) {
        cells.pop();
    }
    cells
        .into_iter()
        .map(|range| {
            let mut cell_start = start + range.start;
            let mut cell_end = start + range.end;
            while cell_start < cell_end && source.as_bytes()[cell_start].is_ascii_whitespace() {
                cell_start += 1;
            }
            while cell_end > cell_start && source.as_bytes()[cell_end - 1].is_ascii_whitespace() {
                cell_end -= 1;
            }
            cell_start..cell_end
        })
        .collect()
}

fn table_delimiter(content: &str) -> Option<Vec<ColumnAlign>> {
    if !content.contains('|') {
        return None;
    }
    let mut align = Vec::new();
    for cell in split_cells(content, 0, content.len()) {
        let cell = content[cell].trim();
        if cell.is_empty() || !cell.chars().any(|character| character == '-') {
            return None;
        }
        let left = cell.starts_with(':');
        let right = cell.ends_with(':');
        align.push(match (left, right) {
            (true, true) => ColumnAlign::Center,
            (false, true) => ColumnAlign::Right,
            _ => ColumnAlign::Left,
        });
        if !cell.chars().all(|character| matches!(character, '-' | ':')) {
            return None;
        }
    }
    (!align.is_empty()).then_some(align)
}

fn table_row_start(source: &str, lines: &[Line], index: usize) -> bool {
    let Some(line) = lines.get(index) else {
        return false;
    };
    let content = &source[line.start..line.content_end];
    if !content.contains('|') {
        return false;
    }
    lines
        .get(index + 1)
        .and_then(|next| {
            let next = &source[next.start..next.content_end];
            table_delimiter(next.trim_start())
        })
        .is_some()
}

/// Parse a GFM pipe table into one block per row.
fn table(
    source: &str,
    lines: &[Line],
    index: usize,
    definitions: &Definitions,
) -> Option<(Vec<RichBlock>, usize)> {
    if !table_row_start(source, lines, index) {
        return None;
    }
    let delimiter_line = &lines[index + 1];
    let align =
        table_delimiter(source[delimiter_line.start..delimiter_line.content_end].trim_start())?;
    let mut blocks = Vec::new();
    let mut row_indexes = vec![index];
    let mut next = index + 2;
    while let Some(line) = lines.get(next) {
        let content = &source[line.start..line.content_end];
        if content.trim().is_empty() || !content.contains('|') {
            break;
        }
        row_indexes.push(next);
        next += 1;
    }
    for row_index in row_indexes {
        let line = &lines[row_index];
        let content_start = line.start;
        let content_end = line.content_end;
        let ranges = split_cells(source, content_start, content_end);
        let header = row_index == index;
        let mut cells = Vec::with_capacity(align.len());
        for (column, align_kind) in align.iter().enumerate() {
            let mut cell = match ranges.get(column) {
                Some(range) => inline(source, range.clone(), definitions),
                None => RichText::default(),
            };
            if header {
                let len = cell.text.len();
                cell.mark(0..len, InlineMarkKind::Bold);
            }
            let _ = align_kind;
            cells.push(cell);
        }
        let mut text = RichText::default();
        for (column, cell) in cells.iter().enumerate() {
            if column > 0 {
                let separator = ranges
                    .get(column - 1)
                    .map_or(content_start..content_start, |range| range.end..range.end);
                text.push_run("\t", separator, false);
            }
            text.append(cell.clone());
        }
        blocks.push(RichBlock {
            kind: BlockKind::TableRow {
                header,
                align: align.clone(),
            },
            source: line.start..line.end,
            content: content_start..content_end,
            text,
            cells,
            depth: 0,
        });
    }
    Some((blocks, next))
}

fn footnote_definition(
    source: &str,
    lines: &[Line],
    index: usize,
    definitions: &Definitions,
) -> Option<(RichBlock, usize)> {
    let line = &lines[index];
    let content = &source[line.start..line.content_end];
    let indent = content.len() - content.trim_start_matches(' ').len();
    let rest = &content[indent..];
    let label = footnote_marker(rest)?;
    let after_marker = indent + rest.find(']')? + 1;
    let colon = content[after_marker..].find(':')?;
    let body_start = line.start + after_marker + colon + 1;
    let body_start = body_start
        + (content[after_marker + colon + 1..].len()
            - content[after_marker + colon + 1..].trim_start().len());
    let mut text = inline(source, body_start..line.content_end, definitions);
    let mut next = index + 1;
    let mut previous_end = line.end;
    while let Some(continuation) = lines.get(next) {
        let continuation_text = &source[continuation.start..continuation.content_end];
        if continuation_text.trim().is_empty()
            || !(continuation_text.starts_with("  ") || continuation_text.starts_with('\t'))
        {
            break;
        }
        let start =
            continuation.start + (continuation_text.len() - continuation_text.trim_start().len());
        text.push_newline(previous_end..continuation.start);
        text.append(inline(source, start..continuation.content_end, definitions));
        previous_end = continuation.end;
        next += 1;
    }
    Some((
        RichBlock {
            kind: BlockKind::Footnote {
                label: label.to_owned(),
            },
            source: line.start..lines[next - 1].end,
            content: body_start..lines[next - 1].content_end,
            text,
            cells: Vec::new(),
            depth: 0,
        },
        next,
    ))
}

/// Raw HTML or MDX flow, kept verbatim as one block until a blank line.
fn raw_flow(source: &str, lines: &[Line], index: usize) -> Option<(RichBlock, usize)> {
    let line = &lines[index];
    let content = &source[line.start..line.content_end];
    let indent = content.len() - content.trim_start_matches(' ').len();
    let rest = &content[indent..];
    if !(rest.starts_with('<') || rest.starts_with('{')) {
        return None;
    }
    let start = line.start + indent;
    let mut next = index + 1;
    let mut end = line.content_end;
    while let Some(line) = lines.get(next) {
        let content = &source[line.start..line.content_end];
        if content.trim().is_empty() {
            break;
        }
        end = line.content_end;
        next += 1;
    }
    Some((
        RichBlock {
            kind: BlockKind::Raw,
            source: start..lines[next - 1].end,
            content: start..end,
            text: RichText::linear(source, start..end),
            cells: Vec::new(),
            depth: 0,
        },
        next,
    ))
}

fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A `[label]: destination` definition; footnote definitions are excluded.
fn reference_definition(rest: &str) -> Option<(&str, &str)> {
    let inner = rest.strip_prefix('[')?;
    if inner.starts_with('^') {
        return None;
    }
    let close = inner.find(']')?;
    let label = &inner[..close];
    if label.trim().is_empty() || label.contains('[') {
        return None;
    }
    let after = inner[close + 1..].strip_prefix(':')?;
    let destination = after.split_whitespace().next()?;
    let destination = destination
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(destination);
    if destination.is_empty() {
        return None;
    }
    Some((label, destination))
}

/// `[^label]:` prefix of a footnote definition.
fn footnote_marker(rest: &str) -> Option<&str> {
    let inner = rest.strip_prefix("[^")?;
    let close = inner.find(']')?;
    let label = &inner[..close];
    if label.trim().is_empty() {
        return None;
    }
    inner[close + 1..].strip_prefix(':')?;
    Some(label)
}

fn reference_definitions(source: &str, lines: &[Line]) -> Definitions {
    let mut definitions = Definitions::new();
    for line in lines {
        let content = &source[line.start..line.content_end];
        let indent = content.len() - content.trim_start_matches(' ').len();
        if indent > 3 {
            continue;
        }
        if let Some((label, destination)) = reference_definition(&content[indent..]) {
            definitions
                .entry(normalize_label(label))
                .or_insert_with(|| destination.to_owned());
        }
    }
    definitions
}

/// Parse one block's content range into rendered inline text.
fn inline(source: &str, range: Range<usize>, definitions: &Definitions) -> RichText {
    let mut text = RichText::default();
    let slice = &source[range.clone()];
    let mut index = 0;
    parse_inline(slice, range.start, &mut text, &mut index, 0, definitions);
    text
}

fn parse_inline(
    text: &str,
    base: usize,
    out: &mut RichText,
    index: &mut usize,
    depth: usize,
    definitions: &Definitions,
) {
    if depth > 8 {
        push_plain(text, base, out, index, text.len());
        return;
    }
    let bytes = text.as_bytes();
    while *index < bytes.len() {
        match bytes[*index] {
            b'\\' if escape_len(text, *index).is_some() => {
                let character = text[*index + 1..].chars().next().unwrap_or('\\');
                let end = *index + 1 + character.len_utf8();
                out.push_run(&character.to_string(), base + *index..base + end, false);
                *index = end;
            }
            b'`' => {
                if !code_span(text, base, out, index) {
                    push_char(text, base, out, index);
                }
            }
            b'*' | b'_' => {
                if !emphasis(text, base, out, index, depth, definitions) {
                    push_char(text, base, out, index);
                }
            }
            b'~' if bytes.get(*index + 1) == Some(&b'~') => {
                if !wrapped(
                    text,
                    base,
                    out,
                    index,
                    depth,
                    "~~",
                    InlineMarkKind::Strikethrough,
                    definitions,
                ) {
                    push_char(text, base, out, index);
                }
            }
            b'!' if bytes.get(*index + 1) == Some(&b'[') => {
                if !link_or_image(text, base, out, index, true, definitions) {
                    push_char(text, base, out, index);
                }
            }
            b'[' => {
                if !link_or_image(text, base, out, index, false, definitions) {
                    push_char(text, base, out, index);
                }
            }
            b'<' => {
                if !autolink(text, base, out, index) {
                    push_char(text, base, out, index);
                }
            }
            _ => {
                // Syntax bytes are ASCII, so the next one is also a UTF-8
                // boundary. Preserve a whole literal span in one source run.
                let start = *index;
                let end = bytes[start + 1..]
                    .iter()
                    .position(|byte| {
                        matches!(byte, b'\\' | b'`' | b'*' | b'_' | b'~' | b'!' | b'[' | b'<')
                    })
                    .map_or(bytes.len(), |offset| start + 1 + offset);
                push_plain(text, base, out, index, end);
            }
        }
    }
}

fn push_plain(text: &str, base: usize, out: &mut RichText, index: &mut usize, end: usize) {
    out.push_run(&text[*index..end], base + *index..base + end, true);
    *index = end;
}

fn push_char(text: &str, base: usize, out: &mut RichText, index: &mut usize) {
    let character = text[*index..].chars().next().unwrap_or('\0');
    let len = character.len_utf8();
    out.push_run(
        &text[*index..*index + len],
        base + *index..base + *index + len,
        true,
    );
    *index += len;
}

fn escape_len(text: &str, index: usize) -> Option<usize> {
    let next = text[index + 1..].chars().next()?;
    matches!(
        next,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
            | '|'
            | '>'
            | '~'
    )
    .then_some(next.len_utf8())
}

fn code_span(text: &str, base: usize, out: &mut RichText, index: &mut usize) -> bool {
    let bytes = text.as_bytes();
    let run = bytes[*index..]
        .iter()
        .take_while(|byte| **byte == b'`')
        .count();
    let Some(close) = find_run(text, *index + run, b'`', run) else {
        return false;
    };
    let mut content_start = *index + run;
    let mut content_end = close;
    if content_end > content_start
        && text[content_start..content_end].starts_with(' ')
        && text[content_start..content_end].ends_with(' ')
        && !text[content_start..content_end].trim().is_empty()
    {
        content_start += 1;
        content_end -= 1;
        // Both padding spaces are syntax; the content is what the reader sees.
    }
    let rendered_start = out.text.len();
    let mut cursor = content_start;
    while cursor < content_end {
        push_char(text, base, out, &mut cursor);
    }
    let source = base + content_start..base + content_end;
    if out.text.len() > rendered_start && source.end > source.start {
        // Rewrite the segments of the newly pushed content as linear over the
        // code span's exact source slice. This also covers escapes inside code,
        // which are literal there.
        let rendered = rendered_start..out.text.len();
        out.segments
            .retain(|segment| segment.rendered.end <= rendered.start);
        out.segments.push(SourceSegment {
            rendered: rendered.clone(),
            source,
            linear: true,
        });
        out.mark(rendered, InlineMarkKind::Code);
    }
    *index = close + run;
    true
}

fn emphasis(
    text: &str,
    base: usize,
    out: &mut RichText,
    index: &mut usize,
    depth: usize,
    definitions: &Definitions,
) -> bool {
    let bytes = text.as_bytes();
    let marker = bytes[*index];
    let run = bytes[*index..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count();
    if marker == b'_'
        && *index > 0
        && text[..*index]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
    {
        return false;
    }
    let width = if run >= 3 { 3 } else { run.min(2) };
    let Some(close) = find_run(text, *index + width, marker, width) else {
        return false;
    };
    if close == *index + width {
        return false;
    }
    if marker == b'_'
        && text[close + width..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric)
    {
        return false;
    }
    let start = out.text.len();
    let mut inner = *index + width;
    let end = close;
    parse_inline(&text[..end], base, out, &mut inner, depth + 1, definitions);
    // `parse_inline` was given a shortened slice; skip the bytes already parsed.
    *index = end + width;
    let range = start..out.text.len();
    match width {
        1 => out.mark(range, InlineMarkKind::Italic),
        2 => out.mark(range, InlineMarkKind::Bold),
        _ => {
            out.mark(range.clone(), InlineMarkKind::Bold);
            out.mark(range, InlineMarkKind::Italic);
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn wrapped(
    text: &str,
    base: usize,
    out: &mut RichText,
    index: &mut usize,
    depth: usize,
    delimiter: &str,
    kind: InlineMarkKind,
    definitions: &Definitions,
) -> bool {
    let start = *index;
    let content_start = start + delimiter.len();
    let Some(relative) = text[content_start..].find(delimiter) else {
        return false;
    };
    let end = content_start + relative;
    if end == content_start {
        return false;
    }
    let rendered_start = out.text.len();
    let mut inner = content_start;
    parse_inline(&text[..end], base, out, &mut inner, depth + 1, definitions);
    let range = rendered_start..out.text.len();
    out.mark(range, kind);
    *index = end + delimiter.len();
    true
}

fn link_or_image(
    text: &str,
    base: usize,
    out: &mut RichText,
    index: &mut usize,
    image: bool,
    definitions: &Definitions,
) -> bool {
    let label_open = if image { *index + 2 } else { *index + 1 };
    let Some(label_close) = find_unescaped(text, label_open, ']') else {
        return false;
    };
    let label = &text[label_open..label_close];
    let next = text.as_bytes().get(label_close + 1).copied();
    let (destination, end) = match next {
        Some(b'(') => {
            let Some(destination_close) = matching_parenthesis(text, label_close + 1) else {
                return false;
            };
            (
                text[label_close + 2..destination_close]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_owned(),
                destination_close + 1,
            )
        }
        Some(b'[') => {
            let Some(reference_close) = find_unescaped(text, label_close + 2, ']') else {
                return false;
            };
            let reference = &text[label_close + 2..reference_close];
            let reference = if reference.trim().is_empty() {
                label
            } else {
                reference
            };
            let Some(destination) = definitions.get(&normalize_label(reference)) else {
                return false;
            };
            (destination.clone(), reference_close + 1)
        }
        _ => {
            let Some(destination) = definitions.get(&normalize_label(label)) else {
                return false;
            };
            (destination.clone(), label_close + 1)
        }
    };
    let rendered_start = out.text.len();
    let mut inner = label_open;
    parse_inline(&text[..label_close], base, out, &mut inner, 0, definitions);
    let range = rendered_start..out.text.len();
    let kind = if image {
        InlineMarkKind::Image(destination)
    } else {
        InlineMarkKind::Link(destination)
    };
    out.mark(range, kind);
    *index = end;
    true
}

fn autolink(text: &str, base: usize, out: &mut RichText, index: &mut usize) -> bool {
    let Some(relative) = text[*index + 1..].find('>') else {
        return false;
    };
    let end = *index + 1 + relative;
    let url = &text[*index + 1..end];
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")) {
        return false;
    }
    let rendered_start = out.text.len();
    let mut inner = *index + 1;
    while inner < end {
        push_char(text, base, out, &mut inner);
    }
    out.mark(
        rendered_start..out.text.len(),
        InlineMarkKind::Link(url.to_owned()),
    );
    *index = end + 1;
    true
}

fn find_run(text: &str, from: usize, marker: u8, width: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == marker {
            let run = bytes[index..]
                .iter()
                .take_while(|byte| **byte == marker)
                .count();
            if run == width {
                return Some(index);
            }
            index += run;
        } else {
            index += 1;
        }
    }
    None
}

fn find_unescaped(text: &str, from: usize, needle: char) -> Option<usize> {
    let mut escaped = false;
    for (offset, character) in text[from..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == needle {
            return Some(from + offset);
        }
    }
    None
}

fn matching_parenthesis(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (offset, character) in text[open..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_texts(document: &WysiwygDocument) -> Vec<(BlockKind, String)> {
        document
            .blocks
            .iter()
            .map(|block| (block.kind.clone(), block.text.text.clone()))
            .collect()
    }

    #[test]
    fn cancelled_parse_never_returns_a_partially_mapped_document() {
        let source = "# First

Second paragraph.

# Third
";
        assert!(WysiwygDocument::parse_cancellable(source, || true).is_none());
        let mut checks = 0;
        let result = WysiwygDocument::parse_cancellable(source, || {
            checks += 1;
            checks >= 3
        });
        assert!(result.is_none());
        assert_eq!(checks, 3);
    }

    #[test]
    fn plain_inline_spans_preserve_unicode_and_source_boundaries() {
        let source = "prefix 中文 e\u{301}👩\u{200d}💻 ! ~ < tail \\* **bold** suffix";
        let start = "prefix ".len();
        let parsed = inline(source, start..source.len(), &Definitions::default());
        assert_eq!(
            parsed.text,
            "中文 e\u{301}👩\u{200d}💻 ! ~ < tail * bold suffix"
        );
        let escape = source.find("\\*").unwrap();
        assert_eq!(
            parsed.segments[0],
            SourceSegment {
                rendered: 0..escape - start,
                source: start..escape,
                linear: true,
            }
        );
        assert!(parsed.segments.iter().any(|segment| {
            !segment.linear
                && segment.source == (escape..escape + 2)
                && &parsed.text[segment.rendered.clone()] == "*"
        }));
        let bold = parsed.text.find("bold").unwrap();
        assert_eq!(
            parsed.marks,
            [InlineMark {
                range: bold..bold + 4,
                kind: InlineMarkKind::Bold
            }]
        );
    }

    #[test]
    fn headings_hide_their_hashes_and_keep_the_content_mapping() {
        let source = "# Title\n\nBody **bold** text.\n";
        let document = WysiwygDocument::parse(source);
        assert_eq!(
            block_texts(&document),
            [
                (BlockKind::Heading(1), "Title".to_owned()),
                (BlockKind::Paragraph, "Body bold text.".to_owned()),
                (BlockKind::Paragraph, String::new()),
            ]
        );
        let heading = &document.blocks[0];
        assert_eq!(&source[heading.content.clone()], "Title");
        assert_eq!(heading.text.source_offset(0), Some(2));
        assert_eq!(heading.text.rendered_offset(2), Some(0));
        let paragraph = &document.blocks[1];
        assert_eq!(&source[paragraph.content.clone()], "Body **bold** text.");
        assert_eq!(
            paragraph.text.rendered_offset(paragraph.content.start + 7),
            Some(5)
        );
        assert_eq!(
            paragraph.text.rendered_offset(paragraph.content.start + 11),
            None
        );
        assert_eq!(
            paragraph.text.source_range(5..9),
            Some(paragraph.content.start + 7..paragraph.content.start + 11)
        );
        assert!(paragraph.text.marks.iter().any(|mark| {
            mark.kind == InlineMarkKind::Bold && &paragraph.text.text[mark.range.clone()] == "bold"
        }));
    }

    #[test]
    fn code_fences_are_hidden_and_code_is_literal() {
        let source = "```rust\nlet x = *y*;\n```\n";
        let document = WysiwygDocument::parse(source);
        let block = &document.blocks[0];
        assert_eq!(
            block.kind,
            BlockKind::Code {
                language: "rust".to_owned()
            }
        );
        assert_eq!(block.text.text, "let x = *y*;\n");
        assert_eq!(&source[block.content.clone()], "let x = *y*;\n");
        assert!(block.text.marks.is_empty());
    }

    #[test]
    fn list_and_quote_markers_are_not_rendered() {
        let document = WysiwygDocument::parse("- one\n- two\n\n> quoted **text**\n");
        assert_eq!(
            block_texts(&document),
            [
                (
                    BlockKind::ListItem {
                        ordered: false,
                        number: 0
                    },
                    "one".to_owned()
                ),
                (
                    BlockKind::ListItem {
                        ordered: false,
                        number: 0
                    },
                    "two".to_owned()
                ),
                (BlockKind::Quote, "quoted text".to_owned()),
                (BlockKind::Paragraph, String::new()),
            ]
        );
    }

    #[test]
    fn links_and_images_keep_their_destinations_on_the_visible_text() {
        let source = "Read [Astro](https://astro.build) and ![cover](./cover.png).";
        let document = WysiwygDocument::parse(source);
        let text = &document.blocks[0].text;
        assert_eq!(text.text, "Read Astro and cover.");
        let link = text
            .marks
            .iter()
            .find(|mark| matches!(mark.kind, InlineMarkKind::Link(_)))
            .expect("link mark");
        assert_eq!(&text.text[link.range.clone()], "Astro");
        assert_eq!(
            text.source_range(link.range.clone()),
            Some(6..11),
            "the label's source range excludes brackets and the destination"
        );
        let image = text
            .marks
            .iter()
            .find(|mark| matches!(mark.kind, InlineMarkKind::Image(_)))
            .expect("image mark");
        assert_eq!(&text.text[image.range.clone()], "cover");
        assert_eq!(document.image_at(0, image.range.start), Some("./cover.png"));
    }

    #[test]
    fn codespans_trim_one_space_and_escapes_map_as_whole_runs() {
        let document = WysiwygDocument::parse("a ` code ` and \\*star\\*\n");
        assert_eq!(
            document.blocks[0].text.text, "a code and *star*",
            "the span's padding spaces are syntax, and escapes decode"
        );
        let text = &document.blocks[0].text;
        assert_eq!(text.source_offset(text.text.find("code").unwrap()), Some(4));
        let escaped = text.text.find('*').unwrap();
        assert_eq!(text.source_range(escaped..escaped + 1), Some(15..17));
        assert_eq!(text.rendered_offset(16), Some(escaped));
    }

    #[test]
    fn a_trailing_newline_opens_an_addressable_empty_line() {
        let document = WysiwygDocument::parse("Body\n");
        assert_eq!(document.blocks.len(), 2);
        let empty = document.blocks.last().unwrap();
        assert_eq!(empty.content, 5..5);
        assert_eq!(document.caret_location(5), Some((1, 0)));
    }

    #[test]
    fn caret_mapping_clamps_out_of_hidden_syntax() {
        let document = WysiwygDocument::parse("# Heading\n");
        let location = document.caret_location(1);
        assert_eq!(location, Some((0, 0)), "the caret after # sits on the H");
        let text = &document.blocks[0].text;
        assert_eq!(text.nearest_rendered_offset(0), 0);
        assert_eq!(text.nearest_rendered_offset(10), text.text.len());
    }

    #[test]
    fn tables_become_rows_with_aligned_cells() {
        let source = "| a | b |\n| :-- | --: |\n| 中 | **文** |\n";
        let document = WysiwygDocument::parse(source);
        assert_eq!(document.blocks.len(), 3, "header, body, trailing empty");
        let header = &document.blocks[0];
        assert_eq!(
            header.kind,
            BlockKind::TableRow {
                header: true,
                align: vec![ColumnAlign::Left, ColumnAlign::Right],
            }
        );
        assert_eq!(
            header
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(
            header.cells[0]
                .marks
                .iter()
                .any(|mark| mark.kind == InlineMarkKind::Bold),
            "header cells are bold"
        );
        let row = &document.blocks[1];
        assert_eq!(
            row.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            ["中", "文"]
        );
        assert!(row.cells[1].marks.iter().any(|mark| {
            mark.kind == InlineMarkKind::Bold && &row.cells[1].text[mark.range.clone()] == "文"
        }));
        let source_range = row.cells[0].source_range(0..3).unwrap();
        assert_eq!(&source[source_range], "中");
    }

    #[test]
    fn reference_links_and_definitions_are_resolved_and_hidden() {
        let source = "Read [the docs][r] and ![cover][img].\n\n[r]: https://example.com/ref \"T\"\n[img]: ./cover.png\n";
        let document = WysiwygDocument::parse(source);
        assert_eq!(
            document.blocks.len(),
            2,
            "definitions render nothing; the trailing newline keeps an empty line"
        );
        let block = &document.blocks[0];
        assert_eq!(block.text.text, "Read the docs and cover.");
        assert!(
            block
                .text
                .marks
                .iter()
                .any(|mark| mark.kind == InlineMarkKind::Link("https://example.com/ref".to_owned()))
        );
        assert!(
            block
                .text
                .marks
                .iter()
                .any(|mark| mark.kind == InlineMarkKind::Image("./cover.png".to_owned()))
        );
    }

    #[test]
    fn footnotes_and_raw_flow_keep_their_content() {
        let source = "Text[^1].\n\n[^1]: Footnote **body**.\n\n<Callout tone=\"note\">\nMDX block\n</Callout>\n";
        let document = WysiwygDocument::parse(source);
        assert!(
            document.blocks[0].text.text.contains("[^1]"),
            "reference stays visible as text"
        );
        let footnote = document
            .blocks
            .iter()
            .find(|block| matches!(block.kind, BlockKind::Footnote { .. }))
            .expect("footnote definition");
        assert_eq!(&source[footnote.content.clone()], "Footnote **body**.");
        assert_eq!(footnote.text.text, "Footnote body.");
        let raw = document
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::Raw)
            .expect("raw MDX block");
        assert!(raw.text.text.starts_with("<Callout"));
        assert!(raw.text.text.contains("MDX block"));
        assert!(raw.text.text.ends_with("</Callout>"));
    }

    #[test]
    fn a_standalone_image_becomes_a_picture_block() {
        let source = "![cover](./cover.png)\n";
        let document = WysiwygDocument::parse(source);
        assert_eq!(
            document.blocks[0].kind,
            BlockKind::Image {
                url: "./cover.png".to_owned()
            }
        );
        assert_eq!(document.blocks[0].text.text, "cover");
    }

    #[test]
    fn a_standalone_image_with_empty_alt_keeps_its_destination() {
        let url = "https://example.com/cover.png";
        let source = format!("## Heading\n\n![]({url})  \n\nFollowing text.");
        let document = WysiwygDocument::parse(&source);
        let image = &document.blocks[1];
        assert_eq!(
            image.kind,
            BlockKind::Image {
                url: url.to_owned()
            }
        );
        assert!(image.text.text.is_empty(), "empty alt text remains empty");
    }

    #[test]
    fn image_only_lines_remain_pictures_next_to_text_and_other_images() {
        let source = "Before\n![](./one.png)\n![](./two.png)\nAfter";
        let document = WysiwygDocument::parse(source);
        assert_eq!(document.blocks.len(), 4);
        assert_eq!(
            &source[document.blocks[1].content.clone()],
            "![](./one.png)"
        );
        assert!(matches!(document.blocks[1].kind, BlockKind::Image { .. }));
        assert!(matches!(document.blocks[2].kind, BlockKind::Image { .. }));
    }

    #[test]
    fn a_paragraph_with_multiple_images_does_not_discard_an_image() {
        let document = WysiwygDocument::parse("![](./a.png) ![](./b.png)\n");
        let paragraph = &document.blocks[0];
        assert_eq!(paragraph.kind, BlockKind::Paragraph);
        assert_eq!(
            paragraph
                .text
                .marks
                .iter()
                .filter_map(|mark| match &mark.kind {
                    InlineMarkKind::Image(url) => Some(url.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["./a.png", "./b.png"],
        );
    }

    #[test]
    fn malformed_syntax_stays_visible_and_lossless() {
        let source = "**unclosed\n\n- not a list marker\n\n[link] without destination\n";
        let document = WysiwygDocument::parse(source);
        let joined: String = document
            .blocks
            .iter()
            .map(|block| block.text.text.clone())
            .collect();
        assert!(joined.contains("**unclosed"));
        assert!(joined.contains("[link] without destination"));
        // Every visible character can be traced back into the source.
        for block in &document.blocks {
            for segment in &block.text.segments {
                assert!(segment.source.end <= source.len());
                assert!(source.is_char_boundary(segment.source.start));
                assert!(source.is_char_boundary(segment.source.end));
            }
        }
    }

    /// The replaced/replacement ranges a byte diff reports for `before`/`after`.
    fn diff(before: &str, after: &str) -> (Range<usize>, Range<usize>) {
        let (a, b) = (before.as_bytes(), after.as_bytes());
        let mut prefix = 0;
        while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
            prefix += 1;
        }
        while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = 0;
        while suffix < a.len() - prefix
            && suffix < b.len() - prefix
            && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
        {
            suffix += 1;
        }
        while !before.is_char_boundary(before.len() - suffix)
            || !after.is_char_boundary(after.len() - suffix)
        {
            suffix -= 1;
        }
        (prefix..before.len() - suffix, prefix..after.len() - suffix)
    }

    fn incremental(before: &str, after: &str) -> Option<IncrementalParse> {
        let previous = WysiwygDocument::parse(before);
        let (old, new) = diff(before, after);
        WysiwygDocument::parse_incremental_cancellable(after, before, &previous, old, new, || false)
    }

    #[test]
    fn incremental_parse_matches_full_parse_for_local_edits() {
        let base = "# Title\n\nFirst **bold** paragraph.\nSecond line.\n\n- item one\n- item two\n\n> quote\n\n```rust\nfn main() {}\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n[^1]: footnote body\n";
        let cases = [
            ("First **bold**", "First **bold and bright**"),
            ("Second line.", "Second."),
            ("- item two\n", ""),
            ("\n\n> quote", "> quote\n> more"),
            ("fn main() {}", "fn run() {}"),
            ("| 1 | 2 |", "| 1 | 3 |"),
            ("# Title\n\n", "# Renamed\n\n"),
            ("footnote body", "footnote text"),
            ("\n\n- item one", "\n\n- item zero\n- item one"),
            (
                "| a | b |\n| - | - |\n| 1 | 2 |\n\n",
                "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n\n",
            ),
        ];
        for (find, replace) in cases {
            let before = base.to_owned();
            let at = before.find(find).expect("edit target exists");
            let mut after = before.clone();
            after.replace_range(at..at + find.len(), replace);
            let full = WysiwygDocument::parse(&after);
            let applied = incremental(&before, &after)
                .unwrap_or_else(|| panic!("incremental apply failed for {find:?}"));
            assert_eq!(applied.document, full, "mismatch for {find:?}");
        }
    }

    #[test]
    fn incremental_parse_reuses_blocks_that_did_not_change() {
        let before = "alpha\n\nbeta\n\ngamma\n\ndelta\n";
        let after = "alpha!\n\nbeta\n\ngamma\n\ndelta\n";
        let applied = incremental(before, after).expect("local edit applies");
        assert_eq!(applied.document, WysiwygDocument::parse(after));
        assert!(
            applied.reused_blocks >= 3,
            "unchanged blocks after the edit are carried over"
        );
    }

    #[test]
    fn incremental_parse_falls_back_for_reference_definition_edits() {
        let before = "See [docs].\n\n[docs]: https://example.com\n";
        let after = "See [docs].\n\n[docs]: https://example.org\n";
        assert!(incremental(before, after).is_none());

        let inserted =
            "See [docs].\n\n[docs]: https://example.org\n\n[docs]: https://example.com\n";
        assert!(incremental(before, inserted).is_none());
    }

    #[test]
    fn incremental_parse_absorbs_a_block_that_crosses_the_end_anchor() {
        // An opening fence inserted before `beta` swallows the following
        // blocks; the spliced suffix must not keep the overridden ones.
        let before = "alpha\n\nbeta\n\ngamma\n";
        let after = "alpha\n\n```\nbeta\n\ngamma\n";
        let applied = incremental(before, after).expect("the crossing unit applies");
        assert_eq!(applied.document, WysiwygDocument::parse(after));
        assert!(matches!(
            applied.document.blocks[1].kind,
            BlockKind::Code { .. }
        ));
    }

    #[test]
    fn incremental_parse_agrees_with_full_parse_over_random_edits() {
        let base = "# Head\n\nIntro **bold** text.\nContinuation line.\n\n- one\n- two\n\n> quote\n\n```rust\nfn main() {}\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n\n[^n]: note body\n";
        let pieces = [
            "x", "中", "\n", "\n\n", "# ", "```\n", "| c |\n", "- ", "> ",
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state >> 33
        };
        let mut applied = 0;
        for _ in 0..400 {
            let before = base.to_owned();
            let mut after = before.clone();
            let boundaries: Vec<usize> = after
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(after.len()))
                .collect();
            let start = boundaries[next() as usize % boundaries.len()];
            let later: Vec<usize> = boundaries
                .iter()
                .copied()
                .filter(|position| *position >= start)
                .collect();
            let end = later[next() as usize % later.len()];
            let piece = pieces[next() as usize % pieces.len()];
            match next() % 3 {
                0 => after.insert_str(start, piece),
                1 => {
                    if end > start {
                        after.replace_range(start..end, "");
                    }
                }
                _ => after.replace_range(start..end, piece),
            }
            let full = WysiwygDocument::parse(&after);
            if let Some(parsed) = incremental(&before, &after) {
                assert_eq!(parsed.document, full, "mismatch after random edit");
                applied += 1;
            }
        }
        assert!(applied > 200, "most local edits must take the fast path");
    }
}
