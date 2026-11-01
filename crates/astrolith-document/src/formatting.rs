//! Pure writer formatting transforms.
//!
//! Every command produces exactly one contiguous replacement of the visible
//! text, so the editor records one undo transaction and the complete source is
//! rewritten once. Transforms never rewrite bytes outside the replacement:
//! indentation, line terminators (including CRLF) and every other character
//! are copied verbatim, and [`format_document`] refuses a replacement the
//! projection mapping cannot place inside one visible segment.
//!
//! Only the commands upstream ships are here: bold, italic, link insertion or
//! URL editing, headings 1–4 and plain paragraph. The Markdown comment toggle
//! (`Mod+/` in `.md`) is the ledger-driven addition recorded in
//! `docs/technical/astro-editor-parity.md` §18; MDX reserves that shortcut for
//! the component builder, so the command is only offered for Markdown
//! documents.

use std::fmt;
use std::ops::Range;

use crate::projection::{ProjectionError, TextEdit, WriterProjection, apply_visible_edit};

/// One upstream formatting command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatCommand {
    /// Toggle `**strong**` around the selection.
    Bold,
    /// Toggle `*emphasis*` around the selection.
    Italic,
    /// Insert a link, or select the URL of the link at the caret.
    Link,
    /// Set the selection's lines to one heading level.
    Heading(HeadingLevel),
    /// Remove the selection's heading markers.
    Paragraph,
    /// Toggle an HTML comment over the selection's lines.
    Comment,
}

/// Markdown heading levels the writer exposes as commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadingLevel {
    /// `#`
    H1,
    /// `##`
    H2,
    /// `###`
    H3,
    /// `####`
    H4,
}

impl HeadingLevel {
    /// Numeric level, `1`–`4`.
    pub fn level(self) -> u8 {
        match self {
            Self::H1 => 1,
            Self::H2 => 2,
            Self::H3 => 3,
            Self::H4 => 4,
        }
    }

    /// The marker text, without its separating space.
    pub fn marker(self) -> &'static str {
        match self {
            Self::H1 => "#",
            Self::H2 => "##",
            Self::H3 => "###",
            Self::H4 => "####",
        }
    }
}

/// One contiguous visible-text replacement plus the resulting selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatOutcome {
    /// Visible range the replacement applies to.
    pub range: Range<usize>,
    /// Text inserted in place of `range`.
    pub replacement: String,
    /// Selection after the transform, in the new visible text.
    pub selection: Range<usize>,
}

impl FormatOutcome {
    /// Whether the transform leaves the text unchanged and only moves the
    /// selection, as editing an existing link's URL does.
    pub fn is_unchanged(&self, text: &str) -> bool {
        text.get(self.range.clone()) == Some(self.replacement.as_str())
    }
}

/// A projected document after one formatting command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormattedDocument {
    /// Complete source with the transform applied through the projection.
    pub source: String,
    /// The visible replacement and resulting selection.
    pub outcome: FormatOutcome,
}

/// Why a formatting command produced no edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The selection is inverted, out of bounds or splits a character.
    InvalidSelection {
        /// Requested start offset.
        start: usize,
        /// Requested end offset.
        end: usize,
    },
    /// The projection mapping refused the replacement.
    Projection(ProjectionError),
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSelection { start, end } => {
                write!(formatter, "invalid formatting selection {start}..{end}")
            }
            Self::Projection(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FormatError {}

impl From<ProjectionError> for FormatError {
    fn from(error: ProjectionError) -> Self {
        Self::Projection(error)
    }
}

/// Apply one command to visible text.
///
/// `selection` is a UTF-8 byte range in `text`. The returned replacement is a
/// single contiguous edit, so a caller applying it to an editor produces one
/// undo step.
///
/// # Errors
///
/// Returns [`FormatError::InvalidSelection`] when the selection is inverted,
/// beyond the text, or splits a UTF-8 character.
pub fn format(
    command: FormatCommand,
    text: &str,
    selection: Range<usize>,
) -> Result<FormatOutcome, FormatError> {
    let selection = validated(text, selection)?;
    Ok(match command {
        FormatCommand::Bold => emphasis(text, &selection, Emphasis::Bold),
        FormatCommand::Italic => emphasis(text, &selection, Emphasis::Italic),
        FormatCommand::Link => link(text, &selection),
        FormatCommand::Heading(level) => blocks(text, &selection, Block::Heading(level)),
        FormatCommand::Paragraph => blocks(text, &selection, Block::Paragraph),
        FormatCommand::Comment => blocks(text, &selection, Block::Comment),
    })
}

/// Apply one command to a projected document.
///
/// The replacement is mapped through `projection` before it is accepted, so a
/// transform that would land on or across hidden bytes is refused with
/// [`FormatError::Projection`] and the source is left untouched.
///
/// # Errors
///
/// Returns [`FormatError::InvalidSelection`] for an unusable selection and
/// [`FormatError::Projection`] when the visible edit cannot be mapped.
pub fn format_document(
    source: &str,
    projection: &WriterProjection,
    selection: Range<usize>,
    command: FormatCommand,
) -> Result<FormattedDocument, FormatError> {
    let outcome = format(command, &projection.visible, selection)?;
    let edit = TextEdit::replace(outcome.range.clone(), outcome.replacement.clone());
    let source = apply_visible_edit(source, projection, edit)?;
    Ok(FormattedDocument { source, outcome })
}

fn validated(text: &str, selection: Range<usize>) -> Result<Range<usize>, FormatError> {
    let invalid = selection.start > selection.end
        || selection.end > text.len()
        || !text.is_char_boundary(selection.start)
        || !text.is_char_boundary(selection.end);
    if invalid {
        return Err(FormatError::InvalidSelection {
            start: selection.start,
            end: selection.end,
        });
    }
    Ok(selection)
}

#[derive(Clone, Copy)]
enum Emphasis {
    Bold,
    Italic,
}

impl Emphasis {
    fn marker(self) -> char {
        '*'
    }

    /// How many markers one side of the pair uses.
    fn width(self) -> usize {
        match self {
            Self::Bold => 2,
            Self::Italic => 1,
        }
    }
}

/// Toggle one inline emphasis marker around the selection.
fn emphasis(text: &str, selection: &Range<usize>, emphasis: Emphasis) -> FormatOutcome {
    let marker = emphasis.marker();
    let width = emphasis.width();
    let wrap = marker.to_string().repeat(width);
    if selection.is_empty() {
        // An empty selection opens the pair so the next keystroke is styled.
        return FormatOutcome {
            range: selection.clone(),
            replacement: wrap.clone() + &wrap,
            selection: selection.start + width..selection.start + width,
        };
    }
    let selected = &text[selection.clone()];
    // The selection already carries the pair: unwrap it.
    if selected.len() > width * 2 && selected.starts_with(&wrap) && selected.ends_with(&wrap) {
        let inner = &selected[width..selected.len() - width];
        return FormatOutcome {
            range: selection.clone(),
            replacement: inner.to_owned(),
            selection: selection.start..selection.start + inner.len(),
        };
    }
    // The pair sits outside the selection: unwrap around it.
    let before = marker_run_before(text, selection.start, marker);
    let after = marker_run_after(text, selection.end, marker);
    let outside = match emphasis {
        Emphasis::Bold => before >= width && after >= width,
        // An odd run contains the single italic marker, so `**word**` gains a
        // marker and `***word***` sheds one; an even run is the bold pair.
        Emphasis::Italic => before % 2 == 1 && after % 2 == 1,
    };
    if outside {
        return FormatOutcome {
            range: selection.start - width..selection.end + width,
            replacement: selected.to_owned(),
            selection: selection.start - width..selection.end - width,
        };
    }
    FormatOutcome {
        range: selection.clone(),
        replacement: wrap.clone() + selected + &wrap,
        selection: selection.start + width..selection.end + width,
    }
}

/// One inline Markdown link inside a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineLink {
    /// Whole `[anchor](destination)` range.
    pub range: Range<usize>,
    /// The anchor text between the brackets.
    pub anchor: Range<usize>,
    /// The destination between the parentheses.
    pub destination: Range<usize>,
}

/// Every inline `[anchor](destination)` link on one line of `text`.
///
/// Links never span lines, nesting is not recognised, and an escaped bracket
/// ends the scan instead of matching, so the result is a conservative set of
/// ranges a writer command may act on.
pub fn inline_links(text: &str) -> Vec<InlineLink> {
    let mut links = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let bytes = content.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != b'[' {
                index += 1;
                continue;
            }
            let Some(close) = bytes[index + 1..].iter().position(|byte| *byte == b']') else {
                break;
            };
            let anchor_start = index + 1;
            let anchor_end = anchor_start + close;
            let paren = anchor_end + 1;
            if bytes.get(paren) != Some(&b'(') {
                index += 1;
                continue;
            }
            let Some(destination_len) = bytes[paren + 1..].iter().position(|byte| *byte == b')')
            else {
                index += 1;
                continue;
            };
            let destination_start = paren + 1;
            let destination_end = destination_start + destination_len;
            links.push(InlineLink {
                range: offset + index..offset + destination_end + 1,
                anchor: offset + anchor_start..offset + anchor_end,
                destination: offset + destination_start..offset + destination_end,
            });
            index = destination_end + 1;
        }
        offset += line.len();
    }
    links
}

/// Insert a link, or move into the URL of the link at the caret.
fn link(text: &str, selection: &Range<usize>) -> FormatOutcome {
    if let Some(found) = inline_links(text)
        .into_iter()
        .find(|link| link.range.start <= selection.start && selection.end <= link.range.end)
    {
        // The command edits the URL: select it so typing replaces it.
        let destination = text[found.destination.clone()].to_owned();
        return FormatOutcome {
            range: found.destination.clone(),
            replacement: destination,
            selection: found.destination,
        };
    }
    if selection.is_empty() {
        // Nothing to wrap: insert the anchor placeholder and select it.
        let replacement = "[text]()".to_owned();
        return FormatOutcome {
            range: selection.clone(),
            replacement,
            selection: selection.start + 1..selection.start + 5,
        };
    }
    let selected = &text[selection.clone()];
    let replacement = format!("[{selected}]()");
    let cursor = selection.start + selected.len() + 3;
    FormatOutcome {
        range: selection.clone(),
        replacement,
        selection: cursor..cursor,
    }
}

enum Block {
    Heading(HeadingLevel),
    Paragraph,
    Comment,
}

/// Apply a line-oriented transform over every line the selection touches.
fn blocks(text: &str, selection: &Range<usize>, block: Block) -> FormatOutcome {
    let span = line_span(text, selection);
    let edits = match block {
        Block::Heading(level) => heading_edits(text, &span, level),
        Block::Paragraph => paragraph_edits(text, &span),
        Block::Comment => comment_edits(text, &span),
    };
    if edits.is_empty() {
        // Nothing to change: the selection only moves, if at all.
        return FormatOutcome {
            range: selection.clone(),
            replacement: text[selection.clone()].to_owned(),
            selection: selection.clone(),
        };
    }
    let span_edit = SpanEdit { span, edits };
    FormatOutcome {
        replacement: span_edit.replacement(text),
        range: span_edit.span.clone(),
        selection: {
            let start = span_edit.map(text, selection.start);
            let end = span_edit.map(text, selection.end);
            start..end
        },
    }
}

/// One line's transforms: each step replaces `removed` bytes at `pos` with
/// `inserted`, in coordinates relative to the line content.
#[derive(Clone, Debug)]
struct Step {
    pos: usize,
    removed: usize,
    inserted: &'static str,
}

#[derive(Clone, Debug)]
struct LineEdit {
    /// Content range in the complete text, excluding the terminator.
    old: Range<usize>,
    steps: Vec<Step>,
}

impl LineEdit {
    fn new_content(&self, text: &str) -> String {
        let mut out = String::with_capacity(self.old.len() + 8);
        let mut cursor = self.old.start;
        for step in &self.steps {
            let at = self.old.start + step.pos;
            out.push_str(&text[cursor..at]);
            out.push_str(step.inserted);
            cursor = at + step.removed;
        }
        out.push_str(&text[cursor..self.old.end]);
        out
    }

    /// Map a content-relative offset through this line's steps.
    fn map(&self, local: usize) -> usize {
        let mut delta: isize = 0;
        for step in &self.steps {
            if local < step.pos {
                break;
            }
            if local <= step.pos + step.removed {
                return (step.pos as isize + delta) as usize + step.inserted.len();
            }
            delta += step.inserted.len() as isize - step.removed as isize;
        }
        (local as isize + delta) as usize
    }
}

struct SpanEdit {
    span: Range<usize>,
    edits: Vec<LineEdit>,
}

impl SpanEdit {
    fn replacement(&self, text: &str) -> String {
        let mut out = String::with_capacity(self.span.len() + 16);
        let mut cursor = self.span.start;
        for edit in &self.edits {
            out.push_str(&text[cursor..edit.old.start]);
            out.push_str(&edit.new_content(text));
            cursor = edit.old.end;
        }
        out.push_str(&text[cursor..self.span.end]);
        out
    }

    fn map(&self, text: &str, offset: usize) -> usize {
        // Bytes before the span are identical in both texts.
        let mut new_pos = self.span.start;
        let mut old_pos = self.span.start;
        for edit in &self.edits {
            if offset < edit.old.start {
                return new_pos + (offset - old_pos);
            }
            new_pos += edit.old.start - old_pos;
            old_pos = edit.old.start;
            if offset <= edit.old.end {
                return new_pos + edit.map(offset - old_pos);
            }
            new_pos += edit.new_content(text).len();
            old_pos = edit.old.end;
        }
        let inside = offset.min(self.span.end) - old_pos;
        new_pos + inside + offset.saturating_sub(self.span.end)
    }
}

/// The content span of every line the selection touches.
///
/// The span starts at the first line's content start (indentation included) and
/// ends at the last line's content end, so terminators stay outside the edit
/// and are copied by the caller.
fn line_span(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = text[..selection.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let line_end = text[selection.end..]
        .find('\n')
        .map_or(text.len(), |index| selection.end + index);
    let end = if line_end > start && text.as_bytes().get(line_end.wrapping_sub(1)) == Some(&b'\r') {
        line_end - 1
    } else {
        line_end
    };
    start..end
}

/// Every line content range inside `span`, with its terminator outside.
fn content_ranges(text: &str, span: &Range<usize>) -> Vec<Range<usize>> {
    if span.is_empty() {
        return vec![span.clone()];
    }
    let mut ranges = Vec::new();
    let mut offset = span.start;
    for line in text[span.clone()].split_inclusive('\n') {
        let terminator = if line.ends_with("\r\n") {
            2
        } else {
            usize::from(line.ends_with('\n'))
        };
        let content_end = offset + line.len() - terminator;
        ranges.push(offset..content_end);
        offset += line.len();
    }
    ranges
}

/// Indentation width of one line, in bytes.
fn indent_len(content: &str) -> usize {
    content
        .char_indices()
        .find(|(_, character)| !matches!(character, ' ' | '\t'))
        .map_or(content.len(), |(index, _)| index)
}

/// A heading marker right after the indentation: its level and the byte offset
/// where the marker ends, just before the heading text.
fn heading_marker(content: &str) -> Option<(u8, usize)> {
    let indent = indent_len(content);
    let rest = &content[indent..];
    let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = rest.as_bytes().get(hashes).copied();
    let separator = match after {
        None => 0,
        Some(b' ' | b'\t') => rest[hashes..]
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count(),
        Some(_) => return None,
    };
    Some((hashes as u8, indent + hashes + separator))
}

fn is_blank(content: &str) -> bool {
    content.trim().is_empty()
}

fn heading_edits(text: &str, span: &Range<usize>, level: HeadingLevel) -> Vec<LineEdit> {
    let lines = content_ranges(text, span);
    let single_line = lines.len() == 1;
    let marked: Vec<Option<(u8, usize)>> = lines
        .iter()
        .map(|range| heading_marker(&text[range.clone()]))
        .collect();
    let considered: Vec<&Option<(u8, usize)>> = lines
        .iter()
        .zip(&marked)
        .filter(|(range, _)| !is_blank(&text[(*range).clone()]))
        .map(|(_, marker)| marker)
        .collect();
    let all_at_level = !considered.is_empty()
        && considered
            .iter()
            .all(|marker| matches!(marker, Some((found, _)) if *found == level.level()));
    let mut edits = Vec::new();
    for (range, marker) in lines.into_iter().zip(marked) {
        let content = &text[range.clone()];
        if is_blank(content) && !single_line {
            continue;
        }
        let indent = indent_len(content);
        let removed = marker.map_or(0, |(_, end)| end - indent);
        let remainder = &content[indent + removed..];
        let inserted: &'static str = if all_at_level {
            ""
        } else if removed > 0 && remainder.is_empty() {
            // A marker-only line keeps the bare hashes: no trailing space.
            level.marker()
        } else {
            match level {
                HeadingLevel::H1 => "# ",
                HeadingLevel::H2 => "## ",
                HeadingLevel::H3 => "### ",
                HeadingLevel::H4 => "#### ",
            }
        };
        if removed == 0 && inserted.is_empty() {
            continue;
        }
        edits.push(LineEdit {
            old: range,
            steps: vec![Step {
                pos: indent,
                removed,
                inserted,
            }],
        });
    }
    edits
}

fn paragraph_edits(text: &str, span: &Range<usize>) -> Vec<LineEdit> {
    let mut edits = Vec::new();
    for range in content_ranges(text, span) {
        let content = &text[range.clone()];
        let Some((_, removed)) = heading_marker(content) else {
            continue;
        };
        edits.push(LineEdit {
            old: range,
            steps: vec![Step {
                pos: indent_len(content),
                removed,
                inserted: "",
            }],
        });
    }
    edits
}

const COMMENT_OPEN: &str = "<!-- ";
const COMMENT_CLOSE: &str = " -->";

fn comment_edits(text: &str, span: &Range<usize>) -> Vec<LineEdit> {
    let lines: Vec<Range<usize>> = content_ranges(text, span)
        .into_iter()
        .filter(|range| !is_blank(&text[range.clone()]))
        .collect();
    let all_commented = lines.iter().all(|range| {
        let content = &text[range.clone()];
        let rest = &content[indent_len(content)..];
        rest.starts_with("<!--") && rest.ends_with("-->") && rest.len() >= 7
    });
    let mut edits = Vec::new();
    for range in lines {
        let content = &text[range.clone()];
        let indent = indent_len(content);
        let rest = &content[indent..];
        let steps = if all_commented {
            // Strip the wrapper in the exact inverse order of the wrap below.
            let mut steps = vec![Step {
                pos: indent,
                removed: 4,
                inserted: "",
            }];
            let body = &rest[4..rest.len() - 3];
            if body.starts_with(' ') {
                steps.push(Step {
                    pos: indent + 4,
                    removed: 1,
                    inserted: "",
                });
            }
            if body.len() > 1 && body.ends_with(' ') {
                steps.push(Step {
                    pos: indent + rest.len() - 4,
                    removed: 1,
                    inserted: "",
                });
            }
            steps.push(Step {
                pos: indent + rest.len() - 3,
                removed: 3,
                inserted: "",
            });
            steps
        } else {
            vec![
                Step {
                    pos: indent,
                    removed: 0,
                    inserted: COMMENT_OPEN,
                },
                Step {
                    pos: content.len(),
                    removed: 0,
                    inserted: COMMENT_CLOSE,
                },
            ]
        };
        edits.push(LineEdit { old: range, steps });
    }
    edits
}

fn marker_run_before(text: &str, offset: usize, marker: char) -> usize {
    text[..offset]
        .chars()
        .rev()
        .take_while(|character| *character == marker)
        .count()
}

fn marker_run_after(text: &str, offset: usize, marker: char) -> usize {
    text[offset..]
        .chars()
        .take_while(|character| *character == marker)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{ProjectionSegment, WriterProjection, project_writer};

    /// Apply one command the way a caller does: replace the returned range.
    fn apply(
        command: FormatCommand,
        text: &str,
        selection: Range<usize>,
    ) -> (String, Range<usize>) {
        let outcome = format(command, text, selection).expect("the selection is valid");
        let mut next = String::with_capacity(text.len() + outcome.replacement.len());
        next.push_str(&text[..outcome.range.start]);
        next.push_str(&outcome.replacement);
        next.push_str(&text[outcome.range.end..]);
        (next, outcome.selection)
    }

    #[test]
    fn bold_toggles_around_the_selection() {
        let (wrapped, selection) = apply(FormatCommand::Bold, "hello world", 0..5);
        assert_eq!(wrapped, "**hello** world");
        assert_eq!(selection, 2..7);

        let (unwrapped, selection) = apply(FormatCommand::Bold, &wrapped, selection);
        assert_eq!(unwrapped, "hello world");
        assert_eq!(selection, 0..5);

        // The markers are outside the selection: they are removed too.
        let (unwrapped, selection) = apply(FormatCommand::Bold, "**hello**", 2..7);
        assert_eq!(unwrapped, "hello");
        assert_eq!(selection, 0..5);
    }

    #[test]
    fn italic_toggles_without_consuming_a_bold_pair() {
        let (wrapped, selection) = apply(FormatCommand::Italic, "word", 0..4);
        assert_eq!(wrapped, "*word*");
        assert_eq!(selection, 1..5);
        assert_eq!(apply(FormatCommand::Italic, &wrapped, selection).0, "word");

        // Inside a bold pair the italic command adds its own emphasis and the
        // next press takes exactly that marker back off.
        let (nested, selection) = apply(FormatCommand::Italic, "**word**", 2..6);
        assert_eq!(nested, "***word***");
        assert_eq!(selection, 3..7);
        let (restored, selection) = apply(FormatCommand::Italic, &nested, selection);
        assert_eq!(restored, "**word**");
        assert_eq!(selection, 2..6);
        // The bold pair is still a bold pair, so bold toggles it off.
        assert_eq!(apply(FormatCommand::Bold, &restored, selection).0, "word");
    }

    #[test]
    fn empty_selection_opens_an_emphasis_pair() {
        let (text, selection) = apply(FormatCommand::Bold, "ab", 1..1);
        assert_eq!(text, "a****b");
        assert_eq!(selection, 3..3);
    }

    #[test]
    fn emphasis_ranges_are_character_safe_with_unicode() {
        let text = "héllo wörld";
        let start = text.find("wörld").unwrap();
        let (wrapped, selection) = apply(FormatCommand::Bold, text, start..text.len());
        assert_eq!(wrapped, "héllo **wörld**");
        assert_eq!(selection, start + 2..text.len() + 2);
        assert_eq!(apply(FormatCommand::Bold, &wrapped, selection).0, text);
    }

    #[test]
    fn headings_replace_each_other_and_toggle_off() {
        let (h2, selection) = apply(
            FormatCommand::Heading(HeadingLevel::H2),
            "Title\nbody",
            0..0,
        );
        assert_eq!(h2, "## Title\nbody");
        assert_eq!(selection, 3..3);

        let (h4, selection) = apply(FormatCommand::Heading(HeadingLevel::H4), &h2, 4..4);
        assert_eq!(h4, "#### Title\nbody");
        let (plain, _) = apply(FormatCommand::Heading(HeadingLevel::H4), &h4, selection);
        assert_eq!(plain, "Title\nbody");
    }

    #[test]
    fn headings_preserve_indentation_and_every_line_ending() {
        let text = "  indented\r\nsecond\r\n";
        let (next, _) = apply(FormatCommand::Heading(HeadingLevel::H3), text, 2..2);
        assert_eq!(next, "  ### indented\r\nsecond\r\n");

        let (next, _) = apply(
            FormatCommand::Heading(HeadingLevel::H1),
            text,
            0..text.len(),
        );
        assert_eq!(next, "  # indented\r\n# second\r\n");
        // Toggling the whole selection off restores the original bytes.
        let (restored, _) = apply(
            FormatCommand::Heading(HeadingLevel::H1),
            &next,
            0..next.len(),
        );
        assert_eq!(restored, "  indented\r\nsecond\r\n");
    }

    #[test]
    fn paragraph_removes_heading_markers_on_every_selected_line() {
        let text = "# One\n## Two\nplain\n";
        let (next, _) = apply(FormatCommand::Paragraph, text, 0..text.len());
        assert_eq!(next, "One\nTwo\nplain\n");
    }

    #[test]
    fn heading_applies_to_the_empty_line_the_caret_sits_on() {
        let (next, selection) = apply(FormatCommand::Heading(HeadingLevel::H1), "body\n", 5..5);
        assert_eq!(next, "body\n# ");
        assert_eq!(selection, 7..7);
    }

    #[test]
    fn link_wraps_a_selection_and_puts_the_caret_in_the_url() {
        let (next, selection) = apply(FormatCommand::Link, "read this", 5..9);
        assert_eq!(next, "read [this]()");
        assert_eq!(selection, 12..12);
        assert_eq!(&next[selection.start..selection.end], "");
        assert_eq!(next.chars().nth(selection.start).unwrap(), ')');
    }

    #[test]
    fn link_with_an_empty_selection_inserts_the_anchor_placeholder() {
        let (next, selection) = apply(FormatCommand::Link, "a", 1..1);
        assert_eq!(next, "a[text]()");
        assert_eq!(&next[selection.clone()], "text");
    }

    #[test]
    fn link_command_edits_an_existing_url_instead_of_nesting() {
        let text = "see [docs](https://example.dev) now";
        let links = inline_links(text);
        assert_eq!(links.len(), 1);
        let caret = text.find("docs").unwrap();
        let outcome = format(FormatCommand::Link, text, caret..caret).unwrap();
        assert_eq!(&text[outcome.range.clone()], "https://example.dev");
        assert!(outcome.is_unchanged(text));
        assert_eq!(outcome.selection, outcome.range);
        assert_eq!(&text[outcome.selection], "https://example.dev");

        // An empty destination selects nothing but moves to the parentheses.
        let text = "see [docs]() now";
        let caret = text.find("docs").unwrap();
        let outcome = format(FormatCommand::Link, text, caret..caret).unwrap();
        assert!(outcome.range.is_empty());
        assert_eq!(outcome.range.start, text.find("()").unwrap() + 1);
    }

    #[test]
    fn comment_toggles_lines_as_one_contiguous_replacement() {
        let text = "one\n  two\n";
        let (next, _) = apply(FormatCommand::Comment, text, 0..text.len());
        assert_eq!(next, "<!-- one -->\n  <!-- two -->\n");

        let (restored, _) = apply(FormatCommand::Comment, &next, 0..next.len());
        assert_eq!(restored, text);
    }

    #[test]
    fn comment_unwraps_a_wrapper_whose_body_is_one_space() {
        // The leading space is part of the opener, so the trailing-space step
        // must not reuse its position: the line is the whole wrapper.
        let text = "<!-- -->";
        let (next, selection) = apply(FormatCommand::Comment, text, 0..text.len());
        assert_eq!(next, "");
        assert_eq!(selection, 0..0);
    }

    #[test]
    fn comment_preserves_crlf_and_leaves_blank_lines_alone() {
        let text = "one\r\n\r\ntwo\r\n";
        let outcome = format(FormatCommand::Comment, text, 0..text.len()).unwrap();
        let mut next = String::new();
        next.push_str(&text[..outcome.range.start]);
        next.push_str(&outcome.replacement);
        next.push_str(&text[outcome.range.end..]);
        assert_eq!(next, "<!-- one -->\r\n\r\n<!-- two -->\r\n");
        assert_eq!(outcome.range, 0..text.len());
    }

    #[test]
    fn invalid_selections_are_refused() {
        for (start, end) in [(0usize, 9usize), (4, 2), (1, 2)] {
            let selection = start..end;
            assert_eq!(
                format(FormatCommand::Bold, "héllo", selection.clone()),
                Err(FormatError::InvalidSelection {
                    start: selection.start,
                    end: selection.end,
                })
            );
        }
        // 1..2 splits the two-byte `é`.
        assert!(matches!(
            format(FormatCommand::Bold, "héllo", 1..2),
            Err(FormatError::InvalidSelection { .. })
        ));
    }

    #[test]
    fn document_transforms_keep_hidden_frontmatter_bytes() {
        let source = "---\ntitle: Kept\nauthor: A. Writer\n---\n# Body\n";
        let projection = project_writer(source, crate::DocumentKind::Markdown).unwrap();
        let caret = projection.visible.find("Body").unwrap();
        let formatted =
            format_document(source, &projection, caret..caret + 4, FormatCommand::Bold).unwrap();
        assert!(formatted.source.ends_with("**Body**\n"));
        assert!(
            formatted
                .source
                .starts_with("---\ntitle: Kept\nauthor: A. Writer\n---\n")
        );
        assert_eq!(
            crate::project_writer(&formatted.source, crate::DocumentKind::Markdown)
                .unwrap()
                .visible,
            "# **Body**\n"
        );
    }

    #[test]
    fn document_transforms_refuse_edits_across_hidden_bytes() {
        // A synthetic layout with a hidden gap in the middle of the visible text:
        // only the projection can tell that these bytes are not contiguous.
        let source = "abcdefg";
        let projection = WriterProjection {
            visible: "abcefg".to_owned(),
            segments: vec![
                ProjectionSegment::Hidden { source: 3..4 },
                ProjectionSegment::Visible {
                    source: 0..3,
                    visible: 0..3,
                },
                ProjectionSegment::Visible {
                    source: 4..7,
                    visible: 3..6,
                },
            ],
        };
        let error = format_document(source, &projection, 2..5, FormatCommand::Bold).unwrap_err();
        assert!(matches!(
            error,
            FormatError::Projection(ProjectionError::HiddenBoundary { .. })
        ));
        // Inside one visible segment the same command is accepted.
        let formatted = format_document(source, &projection, 0..3, FormatCommand::Bold).unwrap();
        assert_eq!(formatted.source, "**abc**defg");
    }
}
