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

    /// Add a mark over the already-rendered range.
    fn mark(&mut self, range: Range<usize>, kind: InlineMarkKind) {
        if range.start < range.end {
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
    /// Indentation level for quotes and lists.
    pub depth: usize,
}

/// A parsed writer document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WysiwygDocument {
    /// Rendered blocks in document order.
    pub blocks: Vec<RichBlock>,
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
        let lines = lines(source);
        let mut blocks = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let line = &lines[index];
            let content = &source[line.start..line.content_end];
            if content.trim().is_empty() {
                index += 1;
                continue;
            }
            let indent = content.len() - content.trim_start_matches(' ').len();
            let rest = &content[indent..];
            if indent <= 3
                && let Some(block) = fenced_code(source, &lines, index, line, rest)
            {
                index = block.next;
                blocks.push(block.block);
                continue;
            }
            if indent <= 3
                && let Some((level, content_start)) = heading(rest, line, indent)
            {
                let content_end = trim_end(source, content_start, line.content_end);
                blocks.push(RichBlock {
                    kind: BlockKind::Heading(level),
                    source: line.start..line.end,
                    content: content_start..content_end,
                    text: inline(source, content_start..content_end),
                    depth: 0,
                });
                index += 1;
                continue;
            }
            if indent <= 3 && is_rule(rest) {
                blocks.push(RichBlock {
                    kind: BlockKind::Rule,
                    source: line.start..line.end,
                    content: line.start..line.start,
                    text: RichText::default(),
                    depth: 0,
                });
                index += 1;
                continue;
            }
            if indent <= 3 && rest.starts_with('>') {
                let (block, next) = blockquote(source, &lines, index, indent);
                blocks.push(block);
                index = next;
                continue;
            }
            if indent <= 3
                && let Some((marker, content_start, ordered, number)) =
                    list_marker(rest, line, indent)
            {
                let content_end = trim_end(source, content_start, line.content_end);
                blocks.push(RichBlock {
                    kind: BlockKind::ListItem { ordered, number },
                    source: line.start..line.end,
                    content: content_start..content_end,
                    text: inline(source, content_start..content_end),
                    depth: indent / 2,
                });
                let _ = marker;
                index += 1;
                continue;
            }
            let (block, next) = paragraph(source, &lines, index, indent);
            blocks.push(block);
            index = next;
        }
        if source.ends_with('\n')
            && blocks
                .last()
                .is_none_or(|block| block.content.end < source.len())
        {
            // Keep the empty line opened by a trailing newline addressable, so
            // pressing Enter at the end of the document puts the caret on a
            // new line instead of at the end of the previous block.
            blocks.push(RichBlock {
                kind: BlockKind::Paragraph,
                source: source.len()..source.len(),
                content: source.len()..source.len(),
                text: RichText::default(),
                depth: 0,
            });
        }
        Self { blocks }
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
            if block.source.start <= source && source <= block.source.end {
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
            depth: 0,
        },
        next,
    })
}

struct FencedBlock {
    block: RichBlock,
    next: usize,
}

fn blockquote(source: &str, lines: &[Line], index: usize, indent: usize) -> (RichBlock, usize) {
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
        text.append(inline(source, start..end));
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
            depth: 1,
        },
        next,
    )
}

fn paragraph(source: &str, lines: &[Line], index: usize, indent: usize) -> (RichBlock, usize) {
    let mut next = index;
    let mut text = RichText::default();
    let mut content_start = 0;
    let mut content_end = 0;
    let mut first = true;
    let mut previous_end = 0;
    while let Some(line) = lines.get(next) {
        let content = &source[line.start..line.content_end];
        if content.trim().is_empty() || (next > index && starts_block(source, line)) {
            break;
        }
        let start = line.start;
        let end = trim_end(source, start, line.content_end);
        if first {
            content_start = start;
            first = false;
        } else {
            text.push_newline(previous_end..line.start);
        }
        text.append(inline(source, start..end));
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
}

/// Parse one block's content range into rendered inline text.
fn inline(source: &str, range: Range<usize>) -> RichText {
    let mut text = RichText::default();
    let slice = &source[range.clone()];
    let mut index = 0;
    parse_inline(slice, range.start, &mut text, &mut index, 0);
    text
}

fn parse_inline(text: &str, base: usize, out: &mut RichText, index: &mut usize, depth: usize) {
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
                if !emphasis(text, base, out, index, depth) {
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
                ) {
                    push_char(text, base, out, index);
                }
            }
            b'!' if bytes.get(*index + 1) == Some(&b'[') => {
                if !link_or_image(text, base, out, index, true) {
                    push_char(text, base, out, index);
                }
            }
            b'[' => {
                if !link_or_image(text, base, out, index, false) {
                    push_char(text, base, out, index);
                }
            }
            b'<' => {
                if !autolink(text, base, out, index) {
                    push_char(text, base, out, index);
                }
            }
            _ => push_char(text, base, out, index),
        }
    }
}

fn push_plain(text: &str, base: usize, out: &mut RichText, index: &mut usize, end: usize) {
    while *index < end {
        push_char(text, base, out, index);
    }
}

fn push_char(text: &str, base: usize, out: &mut RichText, index: &mut usize) {
    let character = text[*index..].chars().next().unwrap_or('\0');
    let len = character.len_utf8();
    out.push_run(
        &character.to_string(),
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

fn emphasis(text: &str, base: usize, out: &mut RichText, index: &mut usize, depth: usize) -> bool {
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
    parse_inline(&text[..end], base, out, &mut inner, depth + 1);
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

fn wrapped(
    text: &str,
    base: usize,
    out: &mut RichText,
    index: &mut usize,
    depth: usize,
    delimiter: &str,
    kind: InlineMarkKind,
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
    parse_inline(&text[..end], base, out, &mut inner, depth + 1);
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
) -> bool {
    let label_open = if image { *index + 2 } else { *index + 1 };
    let Some(label_close) = find_unescaped(text, label_open, ']') else {
        return false;
    };
    if text.as_bytes().get(label_close + 1) != Some(&b'(') {
        return false;
    }
    let Some(destination_close) = matching_parenthesis(text, label_close + 1) else {
        return false;
    };
    let destination = text[label_close + 2..destination_close]
        .split_whitespace()
        .next()
        .unwrap_or("");
    let rendered_start = out.text.len();
    let mut inner = label_open;
    parse_inline(&text[..label_close], base, out, &mut inner, 0);
    let range = rendered_start..out.text.len();
    let kind = if image {
        InlineMarkKind::Image(destination.to_owned())
    } else {
        InlineMarkKind::Link(destination.to_owned())
    };
    out.mark(range, kind);
    *index = destination_close + 1;
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
}
