//! Lossless writer projection.
//!
//! The writer shows the document *body* and keeps the leading metadata regions
//! out of view: the exact leading frontmatter for Markdown, and additionally
//! the contiguous top-level MDX ESM prelude for MDX. The complete source stays
//! authoritative; a visible edit is mapped back through the projection's byte
//! segments so hidden bytes can never be rewritten by an editor.
//!
//! Three properties hold for every projection:
//!
//! - `visible` is exactly the concatenation of the `Visible` segments' source
//!   slices, so an unchanged source round-trips byte-for-byte;
//! - source and visible offsets are UTF-8 character boundaries and the
//!   visible-to-source mapping is monotonic;
//! - an edit that cannot be proven to lie inside one visible segment is
//!   refused instead of guessed.
//!
//! The mapping is a whole-visible-text diff at explicit boundaries, not an
//! incremental edit stream: the pinned editor reports only that the text
//! changed. See `docs/technical/editor-capabilities.md` §8 for the constraint.

use std::ops::Range;

use tree_sitter::{Node, Parser};

use crate::buffer::DocumentError;
use crate::parse::scan_leading_frontmatter;

/// Largest leading MDX region parsed while looking for the ESM prelude.
///
/// A prelude beyond this bound is still hidden up to its last complete
/// statement; the remainder stays visible rather than being guessed.
const PRELUDE_LIMIT: usize = 64 * 1024;

/// Which source syntax a writer projection targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    /// Markdown (`.md`): only the exact leading frontmatter is hidden.
    Markdown,
    /// MDX (`.mdx`): the leading frontmatter and the proven ESM prelude.
    Mdx,
    /// Any other text source: everything stays visible.
    Plain,
}

impl DocumentKind {
    /// Resolve a document kind from a lowercase file extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "md" => Some(Self::Markdown),
            "mdx" => Some(Self::Mdx),
            _ => None,
        }
    }
}

/// One byte region of a projected document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionSegment {
    /// Source bytes the writer hides.
    Hidden {
        /// Hidden range in the complete source.
        source: Range<usize>,
    },
    /// Source bytes the writer shows.
    Visible {
        /// Range in the complete source.
        source: Range<usize>,
        /// Corresponding range in [`WriterProjection::visible`].
        visible: Range<usize>,
    },
}

/// The writer's visible text plus the byte mapping that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriterProjection {
    /// Text the editor displays, in complete-source order.
    pub visible: String,
    /// Source order: hidden lead, then the visible remainder.
    pub segments: Vec<ProjectionSegment>,
}

impl WriterProjection {
    /// Build a projection that hides nothing.
    pub fn complete(source: &str) -> Self {
        Self::leading(source, 0)
    }

    fn leading(source: &str, hidden_end: usize) -> Self {
        let mut segments = Vec::with_capacity(2);
        if hidden_end > 0 {
            segments.push(ProjectionSegment::Hidden {
                source: 0..hidden_end,
            });
        }
        if hidden_end < source.len() {
            segments.push(ProjectionSegment::Visible {
                source: hidden_end..source.len(),
                visible: 0..source.len() - hidden_end,
            });
        }
        Self {
            visible: source[hidden_end..].to_owned(),
            segments,
        }
    }

    /// The complete source length the projection covers.
    pub fn source_len(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| match segment {
                ProjectionSegment::Hidden { source } => source.end,
                ProjectionSegment::Visible { source, .. } => source.end,
            })
            .max()
            .unwrap_or(0)
    }

    /// Map one range of visible bytes to complete-source bytes.
    ///
    /// A visible range must lie inside a single visible segment; an empty range
    /// shared by two visible runs is ambiguous and is refused. Only bytes the
    /// writer shows can ever be rewritten through this mapping.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::HiddenBoundary`] when the range crosses or
    /// touches a hidden region.
    pub fn map_visible_range(&self, range: &Range<usize>) -> Result<Range<usize>, ProjectionError> {
        map_range_to_source(self, range)
    }

    /// Renew the visible text after an edit that stayed inside visible bytes.
    ///
    /// Hidden bytes and their source offsets are kept unchanged; the visible
    /// run is retargeted to the new text. Only the layout [`project_writer`]
    /// produces — one hidden lead followed by the visible remainder — can be
    /// renewed without re-projecting.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::UnrenewableLayout`] for a synthetic projection
    /// whose hidden segments do not form a single leading run.
    pub fn renew_visible(&self, visible: String) -> Result<Self, ProjectionError> {
        let mut segments = Vec::with_capacity(self.segments.len());
        let mut hidden_end = 0;
        for segment in &self.segments {
            match segment {
                ProjectionSegment::Hidden { source } => {
                    if source.start != hidden_end {
                        return Err(ProjectionError::UnrenewableLayout);
                    }
                    hidden_end = source.end;
                    segments.push(segment.clone());
                }
                ProjectionSegment::Visible { source, .. } => {
                    if source.start != hidden_end {
                        return Err(ProjectionError::UnrenewableLayout);
                    }
                }
            }
        }
        if !visible.is_empty() {
            segments.push(ProjectionSegment::Visible {
                source: hidden_end..hidden_end + visible.len(),
                visible: 0..visible.len(),
            });
        }
        Ok(Self { visible, segments })
    }
}

/// One replacement in visible-text byte coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextReplacement {
    /// Visible range to replace.
    pub range: Range<usize>,
    /// Text inserted in its place.
    pub replacement: String,
}

impl TextReplacement {
    /// Build one replacement.
    pub fn new(range: Range<usize>, replacement: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
        }
    }
}

/// A batch of visible-text replacements applied as one complete-source edit.
///
/// Replacements are applied right-to-left against the pre-edit offsets, so a
/// replace-all run stays atomic: the complete source is replaced exactly once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextEdit {
    /// Non-overlapping replacements in visible-text byte coordinates.
    pub replacements: Vec<TextReplacement>,
}

impl TextEdit {
    /// Build a single-region edit.
    pub fn replace(range: Range<usize>, replacement: impl Into<String>) -> Self {
        Self {
            replacements: vec![TextReplacement::new(range, replacement)],
        }
    }

    /// Report whether this edit changes nothing.
    pub fn is_empty(&self) -> bool {
        self.replacements.is_empty()
    }
}

/// Why a projection or a visible edit was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    /// The source starts with a frontmatter opener that is never closed.
    MalformedFrontmatter,
    /// A replacement range is inverted or beyond one text's bounds.
    InvalidRange {
        /// Requested start offset.
        start: usize,
        /// Requested end offset.
        end: usize,
    },
    /// Two replacements claim the same bytes.
    OverlappingEdits,
    /// A replacement touches or straddles a hidden segment.
    HiddenBoundary {
        /// Requested start offset.
        start: usize,
        /// Requested end offset.
        end: usize,
    },
    /// The projection's hidden segments are not one leading run.
    UnrenewableLayout,
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedFrontmatter => {
                formatter.write_str("the document opens frontmatter that is never closed")
            }
            Self::InvalidRange { start, end } => {
                write!(formatter, "invalid visible edit range {start}..{end}")
            }
            Self::OverlappingEdits => formatter.write_str("visible edits overlap"),
            Self::HiddenBoundary { start, end } => write!(
                formatter,
                "visible edit range {start}..{end} touches hidden source bytes"
            ),
            Self::UnrenewableLayout => {
                formatter.write_str("projection layout cannot be renewed without re-projecting")
            }
        }
    }
}

impl std::error::Error for ProjectionError {}

/// Project the writer's visible text for `source`.
///
/// Markdown hides exactly the leading frontmatter block. MDX hides that block
/// and then the contiguous top-level ESM import/export prelude that Tree-sitter
/// proves complete; anything ambiguous stays visible. A leading blank line that
/// separates the hidden regions belongs to the hidden lead, so the hidden run
/// stays contiguous and the visible text starts at real content.
///
/// # Errors
///
/// Returns [`ProjectionError::MalformedFrontmatter`] when the source starts with
/// an unclosed frontmatter block, so the caller can show the complete source
/// instead of hiding a region it cannot delimit.
pub fn project_writer(
    source: &str,
    kind: DocumentKind,
) -> Result<WriterProjection, ProjectionError> {
    let mut hidden_end = 0;
    if kind == DocumentKind::Markdown || kind == DocumentKind::Mdx {
        let span = scan_leading_frontmatter(source)
            .map_err(|_: DocumentError| ProjectionError::MalformedFrontmatter)?;
        if let Some(span) = span {
            hidden_end = span.end;
        }
    }
    if kind == DocumentKind::Mdx
        && let Some(prelude) = esm_prelude_len(&source[hidden_end..])
    {
        hidden_end += prelude;
    }
    // Whole blank lines between the hidden lead and the first body line are
    // separators, not content: keep them hidden so the writer starts at text.
    if hidden_end > 0 {
        hidden_end += blank_line_run(&source[hidden_end..]);
    }
    Ok(WriterProjection::leading(source, hidden_end))
}

/// Bytes of a run of whole blank lines at the start of `text`.
///
/// Only ASCII space, tab and CR count as blank, and a trailing blank line
/// without a line ending is hidden too, so a document that ends in its own
/// separator has no visible text at all.
fn blank_line_run(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        if !content
            .chars()
            .all(|character| matches!(character, ' ' | '\t' | '\r'))
        {
            break;
        }
        offset += line.len();
    }
    offset
}

/// Apply one batch of visible edits to the complete source.
///
/// Every replacement must lie entirely inside one visible segment; the batch
/// must be non-overlapping. Replacements are spliced right-to-left against the
/// pre-edit offsets, so the caller can apply a whole replace-all result as one
/// source string.
///
/// # Errors
///
/// Returns [`ProjectionError::InvalidRange`] for an inverted range, an offset
/// beyond the visible text, or a range that splits a UTF-8 character;
/// [`ProjectionError::HiddenBoundary`] when a range touches or straddles hidden
/// bytes; and [`ProjectionError::OverlappingEdits`] when two replacements claim
/// the same bytes.
pub fn apply_visible_edit(
    source: &str,
    projection: &WriterProjection,
    edit: TextEdit,
) -> Result<String, ProjectionError> {
    if edit.is_empty() {
        return Ok(source.to_owned());
    }
    debug_assert!(
        projection.source_len() <= source.len(),
        "the projection must fit the source it is applied to"
    );
    let visible = projection.visible.as_str();
    let mut mapped = Vec::with_capacity(edit.replacements.len());
    for replacement in &edit.replacements {
        let range = &replacement.range;
        if range.start > range.end || range.end > visible.len() {
            return Err(ProjectionError::InvalidRange {
                start: range.start,
                end: range.end,
            });
        }
        if !visible.is_char_boundary(range.start) || !visible.is_char_boundary(range.end) {
            return Err(ProjectionError::InvalidRange {
                start: range.start,
                end: range.end,
            });
        }
        let source_range = projection.map_visible_range(range)?;
        mapped.push((source_range, replacement.replacement.as_str()));
    }
    mapped.sort_by_key(|(range, _)| range.start);
    if mapped
        .windows(2)
        .any(|pair| pair[0].0.end > pair[1].0.start)
    {
        return Err(ProjectionError::OverlappingEdits);
    }

    let added = mapped.iter().map(|(_, text)| text.len()).sum::<usize>();
    let mut result = String::with_capacity(source.len() + added);
    let mut cursor = 0;
    for (range, replacement) in mapped {
        result.push_str(&source[cursor..range.start]);
        result.push_str(replacement);
        cursor = range.end;
    }
    result.push_str(&source[cursor..]);
    Ok(result)
}

/// Diff two visible texts into the one replacement that turns one into the other.
///
/// The returned range is trimmed to the first and last differing byte, then
/// widened to UTF-8 character boundaries in both texts. Identical texts yield an
/// empty edit, so an unchanged document round-trips without touching the source.
pub fn diff_visible(previous: &str, next: &str) -> TextEdit {
    if previous == next {
        return TextEdit::default();
    }
    let mut prefix = common_prefix(previous.as_bytes(), next.as_bytes());
    while prefix > 0 && !(previous.is_char_boundary(prefix) && next.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let remaining = previous
        .len()
        .saturating_sub(prefix)
        .min(next.len().saturating_sub(prefix));
    let mut suffix = common_suffix(previous.as_bytes(), next.as_bytes(), remaining);
    while suffix > 0
        && !(previous.is_char_boundary(previous.len() - suffix)
            && next.is_char_boundary(next.len() - suffix))
    {
        suffix -= 1;
    }
    TextEdit::replace(
        prefix..previous.len() - suffix,
        next[prefix..next.len() - suffix].to_owned(),
    )
}

/// Longest common byte prefix, compared in machine-sized blocks.
fn common_prefix(left: &[u8], right: &[u8]) -> usize {
    const BLOCK: usize = 64;
    let limit = left.len().min(right.len());
    let mut index = 0;
    while index + BLOCK <= limit {
        if left[index..index + BLOCK] != right[index..index + BLOCK] {
            break;
        }
        index += BLOCK;
    }
    while index < limit && left[index] == right[index] {
        index += 1;
    }
    index
}

/// Longest common byte suffix, bounded by `limit`, compared in blocks.
fn common_suffix(left: &[u8], right: &[u8], limit: usize) -> usize {
    const BLOCK: usize = 64;
    let mut index = 0;
    while index + BLOCK <= limit {
        let left_end = left.len() - index;
        let right_end = right.len() - index;
        if left[left_end - BLOCK..left_end] != right[right_end - BLOCK..right_end] {
            break;
        }
        index += BLOCK;
    }
    while index < limit && left[left.len() - 1 - index] == right[right.len() - 1 - index] {
        index += 1;
    }
    index
}

/// Map one caret offset from `previous` to `next` across the edit between them.
///
/// A rebuild moves hidden bytes around the visible text; the caret belongs to
/// the same visible character, so it is mapped through the visible diff rather
/// than through source offsets. A caret inside replaced text collapses to the
/// end of its replacement.
pub fn map_visible_offset(previous: &str, next: &str, offset: usize) -> usize {
    let offset = offset.min(previous.len());
    let edit = diff_visible(previous, next);
    let Some(replacement) = edit.replacements.first() else {
        return offset;
    };
    let range = &replacement.range;
    if offset <= range.start {
        offset
    } else if offset >= range.end {
        offset + replacement.replacement.len() - range.len()
    } else {
        range.start + replacement.replacement.len()
    }
}

fn map_range_to_source(
    projection: &WriterProjection,
    range: &Range<usize>,
) -> Result<Range<usize>, ProjectionError> {
    let mut mapped = None;
    for segment in &projection.segments {
        if let ProjectionSegment::Visible { source, visible } = segment
            && visible.start <= range.start
            && range.end <= visible.end
        {
            if mapped.is_some() {
                // An empty range on a visible edge shared by two runs cannot say
                // which side of the hidden bytes it belongs to.
                return Err(ProjectionError::HiddenBoundary {
                    start: range.start,
                    end: range.end,
                });
            }
            mapped = Some(
                source.start + (range.start - visible.start)
                    ..source.start + (range.end - visible.start),
            );
        }
    }
    if let Some(mapped) = mapped {
        return Ok(mapped);
    }
    if range.is_empty() && range.start == 0 && projection.visible.is_empty() {
        // Nothing is visible, so typing can only append after the hidden lead.
        let end = projection.source_len();
        return Ok(end..end);
    }
    Err(ProjectionError::HiddenBoundary {
        start: range.start,
        end: range.end,
    })
}

/// Length of the leading MDX ESM prelude, counted from the first body byte.
fn esm_prelude_len(body: &str) -> Option<usize> {
    let first = body.find(|character: char| !character.is_whitespace())?;
    let line_start = body[..first].rfind('\n').map_or(0, |newline| newline + 1);
    if first != line_start || !starts_esm_statement(&body[first..]) {
        return None;
    }
    let limit = floor_char_boundary(body, body.len().min(PRELUDE_LIMIT));
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .ok()?;
    let tree = parser.parse(&body[..limit], None)?;
    let mut cursor = tree.root_node().walk();
    let mut end = None;
    for child in tree.root_node().named_children(&mut cursor) {
        if !is_esm_statement(child) || child.has_error() {
            break;
        }
        let gap_start = end.unwrap_or(first);
        if !body[gap_start..child.start_byte()]
            .bytes()
            .all(|byte| byte.is_ascii_whitespace())
        {
            break;
        }
        end = Some(child.end_byte());
    }
    let last = end?;
    // The line terminator that ends the last statement belongs to the hidden block.
    let line_end = body[last..]
        .find('\n')
        .map_or(body.len(), |newline| last + newline + 1);
    Some(if body[last..line_end].trim().is_empty() {
        line_end
    } else {
        last
    })
}

fn is_esm_statement(node: Node<'_>) -> bool {
    matches!(node.kind(), "import_statement" | "export_statement")
}

fn starts_esm_statement(text: &str) -> bool {
    ["import", "export"].into_iter().any(|keyword| {
        text.strip_prefix(keyword).is_some_and(|rest| {
            rest.starts_with(|character: char| {
                character.is_whitespace() || matches!(character, '{' | '*')
            })
        })
    })
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esm_prelude_is_contiguous_and_bounded() {
        let body =
            "\n\nimport A from \"a\";\nimport B from \"b\";\n\nexport const c = 1;\n\n# Body\n";
        let end = esm_prelude_len(body).expect("a proven prelude must be hidden");
        assert_eq!(&body[end..], "\n# Body\n");
        assert!(body[..end].contains("export const c"));
    }

    #[test]
    fn esm_prelude_accepts_a_complete_statement_without_a_semicolon() {
        let body = "import A from \"a\"\n# Body\n";
        let end = esm_prelude_len(body).expect("the complete import is provable");
        assert_eq!(&body[end..], "# Body\n");
    }

    #[test]
    fn esm_prelude_refuses_ambiguous_syntax() {
        for body in [
            "import {\n  A,\n# Body\n",
            "// note\nimport A from \"a\";\n\n# Body\n",
            "  import A from \"a\";\n\n# Body\n",
            "imported prose\n",
            "# Body\n\nimport A from \"a\";\n",
            "export\n",
        ] {
            assert_eq!(esm_prelude_len(body), None, "{body:?}");
        }
    }
}
