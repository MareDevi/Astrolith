//! Lossless frontmatter document model.
//!
//! [`parse_document`] splits a Markdown/MDX source into its exact frontmatter and
//! body bytes and records the byte span of each supported field so a caller can
//! patch one value in place. [`serialize_document`](crate::serialize::serialize_document)
//! then reproduces every untouched byte exactly, including comments, unknown
//! keys, quoting style, block scalars, Unicode, line endings and trailing
//! newlines.
//!
//! The editable subset is a YAML block mapping whose leaves are plain, quoted
//! or null scalars, whose containers are nested block mappings or block/flow
//! sequences of scalars, and whose container values can be replaced as
//! validated raw YAML. A block sequence may be indented under its key or start
//! at the key's own indentation (indentless sequence); both styles are
//! item-editable when every item is a scalar. [`ParsedDocument::fields`] exposes
//! a read-only shape snapshot for every field, while
//! [`ParsedDocument::insert_field`], [`ParsedDocument::remove_field`], the
//! sequence item methods and [`ParsedDocument::replace_raw`] record byte patches
//! that only take effect on serialization. Inserting into a document that has no
//! frontmatter creates a leading block after any byte-order mark and leaves the
//! body byte-for-byte.
//!
//! Block scalars (`|`, `>`) are preserved as opaque fields that can be read but
//! not patched; anchors, aliases, tags, merge keys, quoted or complex keys,
//! multi-line flow collections and sequence items in positions where no
//! sequence is allowed are refused. A sequence or mapping whose items are not
//! scalar leaves is not item-editable but is still replaceable as raw YAML.
//! Refusing unsupported input is safer than guessing: it can never corrupt a
//! document.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::{Yaml, YamlLoader};

use crate::buffer::DocumentError;
use crate::yaml::validate_value;

/// The dominant line ending of a document, preserved on serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    /// LF (`\n`) line endings.
    Lf,
    /// CRLF (`\r\n`) line endings.
    Crlf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

/// A typed scalar value read from, or written to, frontmatter.
#[derive(Clone, Debug, PartialEq)]
pub enum FrontmatterValue {
    /// YAML `null`.
    Null,
    /// A boolean.
    Boolean(bool),
    /// An integer.
    Integer(i64),
    /// A floating-point number.
    Float(f64),
    /// A string.
    String(String),
}

/// The YAML shape of one frontmatter field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldShape {
    /// A plain or quoted scalar leaf.
    Scalar,
    /// A `key:` with no value.
    Null,
    /// A block mapping container that has children.
    Mapping,
    /// A block or flow sequence.
    Sequence,
    /// A `|` or `>` block scalar preserved verbatim.
    BlockScalar,
    /// A single-line flow mapping (`{...}`) or another opaque single-line value.
    Flow,
}

/// The YAML style of a sequence field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceStyle {
    /// One `- item` line per item.
    Block,
    /// A single-line `[a, b]` collection.
    Flow,
}

/// A read-only snapshot of one parsed frontmatter field.
#[derive(Clone, Debug, PartialEq)]
pub struct FrontmatterField {
    /// The dotted path of the field, in document order.
    pub path: String,
    /// The YAML shape of the field.
    pub shape: FieldShape,
    /// The typed scalar value, present only for [`FieldShape::Scalar`].
    pub value: Option<FrontmatterValue>,
    /// The scalar items of an editable sequence; `None` when any item is not a
    /// scalar or when the field is not a sequence.
    pub items: Option<Vec<FrontmatterValue>>,
    /// The exact source text of a raw-editable container: a single-line flow
    /// collection, or the dedented value block of a block mapping/sequence.
    pub raw: Option<String>,
    /// The sequence style, present only for [`FieldShape::Sequence`].
    pub sequence_style: Option<SequenceStyle>,
}

#[derive(Clone, Debug)]
struct Field {
    path: String,
    shape: FieldShape,
    value: FrontmatterValue,
    span: Range<usize>,
    kind: FieldKind,
    indent: usize,
    key_line: Range<usize>,
    block: Range<usize>,
    value_pos: usize,
    child_indent: Option<usize>,
    items: Option<Vec<FrontmatterValue>>,
    raw: Option<String>,
    sequence_style: Option<SequenceStyle>,
    sequence: Option<SequenceInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    /// A patchable scalar leaf.
    Scalar,
    /// A `key:` with a null value; readable but not patchable.
    NullLeaf,
    /// A container, flow collection or block scalar; not patchable as a scalar.
    Opaque,
}

#[derive(Clone, Debug)]
enum SequenceInfo {
    /// A block sequence with at least one `- item` line.
    Block(BlockSequence),
    /// A single-line flow sequence.
    Flow(FlowSequence),
    /// A sequence whose items cannot be edited individually.
    Opaque,
}

#[derive(Clone, Debug)]
struct BlockSequence {
    item_indent: usize,
    item_spans: Vec<Range<usize>>,
    lines: Vec<Range<usize>>,
}

#[derive(Clone, Debug)]
struct FlowSequence {
    collection: Range<usize>,
    item_spans: Vec<Range<usize>>,
    separators: Vec<String>,
}

#[derive(Clone, Debug)]
struct Patch {
    path: String,
    span: Range<usize>,
    replacement: String,
}

#[derive(Clone, Debug)]
struct Frontmatter {
    fields: Vec<Field>,
    public: Vec<FrontmatterField>,
    /// Absolute offset of the closing `---` line.
    end: usize,
}

/// A parsed document that can be serialized back to its exact source bytes.
#[derive(Clone, Debug)]
pub struct ParsedDocument {
    source: String,
    line_ending: LineEnding,
    frontmatter: Option<Frontmatter>,
    body: String,
    patches: Vec<Patch>,
    /// A leading frontmatter block created for a document that had none.
    leading: Option<LeadingFrontmatter>,
}

/// Fields inserted into a document that did not start with a frontmatter block.
///
/// The block is emitted only on serialization, after any byte-order mark and
/// before the untouched body.
#[derive(Clone, Debug)]
struct LeadingFrontmatter {
    offset: usize,
    lines: Vec<(String, String)>,
}

impl ParsedDocument {
    /// Return the exact raw body (everything after the closing frontmatter line).
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Return the dominant line ending detected in the source.
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Report whether the source had a frontmatter block.
    pub fn has_frontmatter(&self) -> bool {
        self.frontmatter.is_some()
    }

    /// Return the typed value of a dotted field path, if present.
    ///
    /// Only scalar leaves are exposed; containers, sequences, flow collections
    /// and block scalars return `None`.
    pub fn field(&self, path: &str) -> Option<&FrontmatterValue> {
        self.frontmatter
            .as_ref()?
            .fields
            .iter()
            .find(|field| field.path == path && field.kind != FieldKind::Opaque)
            .map(|field| &field.value)
    }

    /// Return the shape snapshot of every field, in source order.
    ///
    /// The returned slice is a read-only snapshot taken at parse time: pending
    /// mutations are not reflected until the document is re-parsed.
    pub fn fields(&self) -> &[FrontmatterField] {
        self.frontmatter
            .as_ref()
            .map_or(&[], |frontmatter| frontmatter.public.as_slice())
    }

    /// Return the shape snapshot of one field, if present.
    pub fn field_info(&self, path: &str) -> Option<&FrontmatterField> {
        self.frontmatter
            .as_ref()?
            .public
            .iter()
            .find(|field| field.path == path)
    }

    /// Report whether the frontmatter contains a field at `path`.
    pub fn has_field(&self, path: &str) -> bool {
        self.field_info(path).is_some()
    }

    /// Record an in-place scalar patch for one field.
    ///
    /// A `key:` null leaf records the value directly after the colon, before any
    /// trailing comment, so `title:` becomes `title: value` while the rest of
    /// the line is preserved.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when `path` is absent,
    /// [`DocumentError::UnsupportedPatch`] when the field is not a patchable
    /// scalar (container, sequence, flow collection or block scalar), or when
    /// the value cannot be rendered as a safe YAML scalar.
    pub fn set_field(&mut self, path: &str, value: FrontmatterValue) -> Result<(), DocumentError> {
        let field = self.field_record(path)?;
        let replacement = render_value(path, &value)?;
        let span = match field.kind {
            FieldKind::Scalar => field.span.clone(),
            FieldKind::NullLeaf => {
                let position = field.value_pos;
                let replacement = format!(" {replacement}");
                self.record_patch(path, position..position, replacement);
                return Ok(());
            }
            FieldKind::Opaque => {
                return Err(DocumentError::UnsupportedPatch {
                    path: path.to_owned(),
                    reason: "field is not a scalar".to_owned(),
                });
            }
        };
        self.record_patch(path, span, replacement);
        Ok(())
    }

    /// Insert an absent scalar field.
    ///
    /// `path` is a dotted path whose parent mapping must already exist (or be
    /// top-level). Nested insertion uses the parent mapping's indentation and
    /// the document's line ending. A document without a frontmatter block gets
    /// a leading `---` block after any byte-order mark; the body bytes are kept
    /// exactly.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when the path already exists, and
    /// [`DocumentError::UnsupportedPatch`] when the parent mapping is missing or
    /// is not a mapping, or the value cannot be rendered as a YAML scalar.
    pub fn insert_field(
        &mut self,
        path: &str,
        value: FrontmatterValue,
    ) -> Result<(), DocumentError> {
        let rendered = render_value(path, &value)?;
        let key = last_segment(path);
        let line = format!("{key}: {rendered}");
        if self.frontmatter.is_none() {
            return self.insert_leading_line(path, line);
        }
        let (position, indent) = self.insertion_point(path)?;
        let line = format!("{}{line}{}", " ".repeat(indent), self.line_ending.as_str());
        self.record_patch(path, position..position, line);
        Ok(())
    }

    /// Insert an absent sequence field with the given scalar items.
    ///
    /// Items are rendered as a single-line flow sequence (`key: [a, "b"]`)
    /// using the document line ending; empty `items` inserts `key: []`. Items
    /// are quoted whenever a plain rendering could merge into a neighbour or
    /// change meaning in flow context, so `a,b` stays one item. An existing
    /// null value (`key:` or `key: null`) is replaced by that flow sequence
    /// instead. The parent mapping must already exist (or be top-level), and no
    /// other byte of the document changes.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when the path already holds a
    /// non-null value, and [`DocumentError::UnsupportedPatch`] when the parent
    /// mapping is missing or is not a mapping, or an item cannot be rendered as
    /// a YAML scalar.
    pub fn insert_sequence(
        &mut self,
        path: &str,
        items: &[FrontmatterValue],
    ) -> Result<(), DocumentError> {
        let mut rendered = Vec::with_capacity(items.len());
        for item in items {
            rendered.push(render_flow_value(path, item)?);
        }
        if let Ok(field) = self.field_record(path) {
            let null_value = matches!(field.kind, FieldKind::NullLeaf)
                || (matches!(field.kind, FieldKind::Scalar)
                    && field.sequence.is_none()
                    && matches!(field.value, FrontmatterValue::Null));
            if null_value {
                let span = field.value_pos..field.span.end;
                let value = format!(" [{}]", rendered.join(", "));
                self.record_patch(path, span, value);
                return Ok(());
            }
            return Err(DocumentError::UnknownField {
                path: path.to_owned(),
            });
        }
        let key = last_segment(path);
        let line = format!("{key}: [{}]", rendered.join(", "));
        if self.frontmatter.is_none() {
            return self.insert_leading_line(path, line);
        }
        let (position, indent) = self.insertion_point(path)?;
        let line = format!("{}{line}{}", " ".repeat(indent), self.line_ending.as_str());
        self.record_patch(path, position..position, line);
        Ok(())
    }

    /// Record one line of the leading frontmatter block created on demand.
    fn insert_leading_line(&mut self, path: &str, line: String) -> Result<(), DocumentError> {
        if path.rsplit_once('.').is_some() {
            return Err(DocumentError::UnsupportedPatch {
                path: path.to_owned(),
                reason: "parent mapping does not exist".to_owned(),
            });
        }
        let offset = if self.source.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        };
        let leading = self.leading.get_or_insert(LeadingFrontmatter {
            offset,
            lines: Vec::new(),
        });
        if leading.lines.iter().any(|(existing, _)| existing == path) {
            return Err(DocumentError::UnknownField {
                path: path.to_owned(),
            });
        }
        leading.lines.push((path.to_owned(), line));
        Ok(())
    }

    /// The byte position and indentation where an absent field is inserted.
    fn insertion_point(&self, path: &str) -> Result<(usize, usize), DocumentError> {
        let Some(frontmatter) = self.frontmatter.as_ref() else {
            return Err(DocumentError::UnknownField {
                path: path.to_owned(),
            });
        };
        if frontmatter.fields.iter().any(|field| field.path == path) {
            return Err(DocumentError::UnknownField {
                path: path.to_owned(),
            });
        }
        match path.rsplit_once('.') {
            Some((parent_path, _)) => {
                let Some(parent) = frontmatter
                    .fields
                    .iter()
                    .find(|field| field.path == parent_path)
                else {
                    return Err(DocumentError::UnsupportedPatch {
                        path: path.to_owned(),
                        reason: "parent mapping does not exist".to_owned(),
                    });
                };
                if parent.shape != FieldShape::Mapping {
                    return Err(DocumentError::UnsupportedPatch {
                        path: path.to_owned(),
                        reason: "parent field is not a mapping".to_owned(),
                    });
                }
                Ok((
                    parent.block.end,
                    parent.child_indent.unwrap_or(parent.indent + 2),
                ))
            }
            None => Ok((frontmatter.end, 0)),
        }
    }

    /// Remove an existing field, including its nested block and comment suffix.
    ///
    /// Removing the only child of a mapping leaves the parent key with a null
    /// value. Removing a sequence removes the whole sequence block.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when `path` is absent.
    pub fn remove_field(&mut self, path: &str) -> Result<(), DocumentError> {
        let field = self.field_record(path)?;
        let span = match field.shape {
            FieldShape::Scalar | FieldShape::Null | FieldShape::Flow => field.key_line.clone(),
            _ => field.block.clone(),
        };
        self.record_patch(path, span, String::new());
        Ok(())
    }

    /// Replace one item of a scalar sequence (block or flow).
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when `path` is absent,
    /// [`DocumentError::UnsupportedPatch`] when the field is not an item-editable
    /// sequence, the index is out of range, or the value cannot be rendered.
    pub fn set_sequence_item(
        &mut self,
        path: &str,
        index: usize,
        value: FrontmatterValue,
    ) -> Result<(), DocumentError> {
        let (span, flow) = {
            let sequence = self.sequence_info(path)?;
            let span =
                sequence_item_span(sequence, index).ok_or_else(|| index_out_of_range(path))?;
            (span, matches!(sequence, SequenceInfo::Flow(_)))
        };
        let rendered = if flow {
            render_flow_value(path, &value)?
        } else {
            render_value(path, &value)?
        };
        self.record_patch(path, span, rendered);
        Ok(())
    }

    /// Insert a scalar item at `index` (`index <= len`).
    ///
    /// Block sequences get a new `- value` line at the sequence indentation;
    /// flow sequences get `, value` before the closing bracket. Inserting into an
    /// empty flow sequence yields `[value]` and preserves surrounding bytes. A
    /// null field (`key:` or `key: null`) accepts its first item and becomes
    /// `key: [value]`.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when `path` is absent,
    /// [`DocumentError::UnsupportedPatch`] when the field is not an
    /// item-editable sequence, the index is out of range, or the value cannot be
    /// rendered.
    pub fn insert_sequence_item(
        &mut self,
        path: &str,
        index: usize,
        value: FrontmatterValue,
    ) -> Result<(), DocumentError> {
        let eol = self.line_ending.as_str();
        let null_span = match self.field_record(path) {
            Ok(field)
                if matches!(field.kind, FieldKind::NullLeaf)
                    || (matches!(field.kind, FieldKind::Scalar)
                        && field.sequence.is_none()
                        && matches!(field.value, FrontmatterValue::Null)) =>
            {
                Some(field.value_pos..field.span.end)
            }
            _ => None,
        };
        if let Some(span) = null_span {
            if index > 0 {
                return Err(index_out_of_range(path));
            }
            let rendered = render_flow_value(path, &value)?;
            self.record_patch(path, span, format!(" [{rendered}]"));
            return Ok(());
        }
        let (span, replacement) = {
            let sequence = self.sequence_info(path)?;
            match sequence {
                SequenceInfo::Block(block) => {
                    let count = block.item_spans.len();
                    if index > count {
                        return Err(index_out_of_range(path));
                    }
                    if count == 0 {
                        return Err(DocumentError::UnsupportedPatch {
                            path: path.to_owned(),
                            reason: "block sequence is empty".to_owned(),
                        });
                    }
                    let rendered = render_value(path, &value)?;
                    let position = if index == count {
                        block.lines[count - 1].end
                    } else {
                        block.lines[index].start
                    };
                    let line = format!("{}- {rendered}{eol}", " ".repeat(block.item_indent));
                    (position..position, line)
                }
                SequenceInfo::Flow(flow) => {
                    let count = flow.item_spans.len();
                    if index > count {
                        return Err(index_out_of_range(path));
                    }
                    let rendered = render_flow_value(path, &value)?;
                    if count == 0 {
                        let start = flow.collection.start + 1;
                        let end = flow.collection.end - 1;
                        (start..end, rendered)
                    } else if index == 0 {
                        let separator = flow
                            .separators
                            .first()
                            .cloned()
                            .unwrap_or_else(|| ", ".to_owned());
                        let position = flow.item_spans[0].start;
                        (position..position, format!("{rendered}{separator}"))
                    } else if index == count {
                        let separator = flow
                            .separators
                            .last()
                            .cloned()
                            .unwrap_or_else(|| ", ".to_owned());
                        let position = flow.item_spans[count - 1].end;
                        (position..position, format!("{separator}{rendered}"))
                    } else {
                        let separator = flow.separators[index - 1].clone();
                        let position = flow.item_spans[index].start;
                        (position..position, format!("{rendered}{separator}"))
                    }
                }
                SequenceInfo::Opaque => unreachable!("sequence_info rejects opaque sequences"),
            }
        };
        self.record_patch(path, span, replacement);
        Ok(())
    }

    /// Remove one scalar sequence item. Removing the last item yields `[]`.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when `path` is absent and
    /// [`DocumentError::UnsupportedPatch`] when the field is not an
    /// item-editable sequence or the index is out of range.
    pub fn remove_sequence_item(&mut self, path: &str, index: usize) -> Result<(), DocumentError> {
        let patches: Vec<(Range<usize>, String)> = {
            let sequence = self.sequence_info(path)?;
            match sequence {
                SequenceInfo::Block(block) => {
                    let count = block.lines.len();
                    if index >= count {
                        return Err(index_out_of_range(path));
                    }
                    if count == 1 {
                        let value_pos = self.field_record(path)?.value_pos;
                        vec![
                            (value_pos..value_pos, " []".to_owned()),
                            (block.lines[0].clone(), String::new()),
                        ]
                    } else {
                        vec![(block.lines[index].clone(), String::new())]
                    }
                }
                SequenceInfo::Flow(flow) => {
                    let count = flow.item_spans.len();
                    if index >= count {
                        return Err(index_out_of_range(path));
                    }
                    if count == 1 {
                        vec![(flow.collection.clone(), "[]".to_owned())]
                    } else if index == 0 {
                        vec![(
                            flow.item_spans[0].start..flow.item_spans[1].start,
                            String::new(),
                        )]
                    } else {
                        vec![(
                            flow.item_spans[index - 1].end..flow.item_spans[index].end,
                            String::new(),
                        )]
                    }
                }
                SequenceInfo::Opaque => unreachable!("sequence_info rejects opaque sequences"),
            }
        };
        for (span, replacement) in patches {
            self.record_patch(path, span, replacement);
        }
        Ok(())
    }

    /// Replace a field's exact raw text after validating it as one YAML value.
    ///
    /// Single-line flow collections replace their inline span. Block containers
    /// (block sequences and block mappings) replace the whole value block: the
    /// text is dedented, re-indented into the field's own indentation and
    /// validated as a child of the field key before any byte changes.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when `path` is absent,
    /// [`DocumentError::UnsupportedPatch`] for scalar leaves and block scalars,
    /// or when `raw` is not a valid value for the field's context (empty,
    /// anchors, aliases, tags, block scalars or a structure that does not nest
    /// under one key).
    pub fn replace_raw(&mut self, path: &str, raw: &str) -> Result<(), DocumentError> {
        let field = self.field_record(path)?;
        let inline = match field.shape {
            FieldShape::Flow => true,
            FieldShape::Sequence => field.sequence_style == Some(SequenceStyle::Flow),
            _ => false,
        };
        if inline {
            let span = field.span.clone();
            let trimmed = raw.trim();
            validate_value(trimmed).map_err(|reason| DocumentError::UnsupportedPatch {
                path: path.to_owned(),
                reason,
            })?;
            self.record_patch(path, span, trimmed.to_owned());
            return Ok(());
        }
        if matches!(field.shape, FieldShape::Mapping | FieldShape::Sequence) {
            let span = field.key_line.end..field.block.end;
            // An indentless sequence's child indentation equals its own, so
            // re-indent a raw replacement one level deeper: emitting it at the
            // key's indentation would turn a mapping value into sibling keys.
            let indent = field
                .child_indent
                .filter(|child| *child > field.indent)
                .unwrap_or(field.indent + 2);
            let rendered = render_block_raw(raw, indent, self.line_ending).map_err(|reason| {
                DocumentError::UnsupportedPatch {
                    path: path.to_owned(),
                    reason,
                }
            })?;
            self.record_patch(path, span, rendered);
            return Ok(());
        }
        Err(DocumentError::UnsupportedPatch {
            path: path.to_owned(),
            reason: "field is not a container collection".to_owned(),
        })
    }

    /// Serialize the document, applying any recorded patches.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnsupportedPatch`] if recorded patches overlap,
    /// which would make the output ambiguous.
    pub fn serialize(&self) -> Result<String, DocumentError> {
        if self.patches.is_empty() && self.leading.is_none() {
            return Ok(self.source.clone());
        }
        let mut ordered: Vec<&Patch> = self.patches.iter().collect();
        ordered.sort_by_key(|patch| patch.span.start);
        for pair in ordered.windows(2) {
            if pair[0].span.end > pair[1].span.start {
                return Err(DocumentError::UnsupportedPatch {
                    path: pair[1].path.clone(),
                    reason: "patches overlap".to_owned(),
                });
            }
        }
        let mut output = String::with_capacity(self.source.len() + 64);
        let mut cursor = 0;
        if let Some(leading) = self.leading.as_ref() {
            let eol = self.line_ending.as_str();
            output.push_str(&self.source[..leading.offset]);
            output.push_str("---");
            output.push_str(eol);
            for (_, line) in &leading.lines {
                output.push_str(line);
                output.push_str(eol);
            }
            output.push_str("---");
            output.push_str(eol);
            output.push_str(eol);
            cursor = leading.offset;
        }
        for patch in ordered {
            output.push_str(&self.source[cursor..patch.span.start]);
            output.push_str(&patch.replacement);
            cursor = patch.span.end;
        }
        output.push_str(&self.source[cursor..]);
        Ok(output)
    }

    fn field_record(&self, path: &str) -> Result<&Field, DocumentError> {
        self.frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter.fields.iter().find(|field| field.path == path))
            .ok_or_else(|| DocumentError::UnknownField {
                path: path.to_owned(),
            })
    }

    fn sequence_info(&self, path: &str) -> Result<&SequenceInfo, DocumentError> {
        match self.field_record(path)?.sequence.as_ref() {
            Some(info @ (SequenceInfo::Block(_) | SequenceInfo::Flow(_))) => Ok(info),
            Some(SequenceInfo::Opaque) => Err(DocumentError::UnsupportedPatch {
                path: path.to_owned(),
                reason: "sequence items are not editable".to_owned(),
            }),
            None => Err(DocumentError::UnsupportedPatch {
                path: path.to_owned(),
                reason: "field is not a sequence".to_owned(),
            }),
        }
    }

    fn record_patch(&mut self, path: &str, span: Range<usize>, replacement: String) {
        if let Some(existing) = self
            .patches
            .iter_mut()
            .find(|patch| patch.path == path && patch.span == span)
        {
            existing.replacement = replacement;
            return;
        }
        self.patches.push(Patch {
            path: path.to_owned(),
            span,
            replacement,
        });
    }
}

fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

fn sequence_item_span(sequence: &SequenceInfo, index: usize) -> Option<Range<usize>> {
    match sequence {
        SequenceInfo::Block(block) => block.item_spans.get(index).cloned(),
        SequenceInfo::Flow(flow) => flow.item_spans.get(index).cloned(),
        SequenceInfo::Opaque => None,
    }
}

fn index_out_of_range(path: &str) -> DocumentError {
    DocumentError::UnsupportedPatch {
        path: path.to_owned(),
        reason: "sequence index is out of range".to_owned(),
    }
}

fn render_value(path: &str, value: &FrontmatterValue) -> Result<String, DocumentError> {
    crate::serialize::serialize_scalar(value).map_err(|reason| DocumentError::UnsupportedPatch {
        path: path.to_owned(),
        reason,
    })
}

/// Render a value as one item of a single-line flow sequence.
///
/// Flow context treats `,` `[` `]` `{` `}` and `#` as structure, so a string
/// carrying any of them is quoted rather than emitted plain.
fn render_flow_value(path: &str, value: &FrontmatterValue) -> Result<String, DocumentError> {
    crate::serialize::serialize_flow_scalar(value).map_err(|reason| {
        DocumentError::UnsupportedPatch {
            path: path.to_owned(),
            reason,
        }
    })
}

/// Render raw block-container text into an indented value block.
///
/// The text is normalized to the document line ending, dedented by its smallest
/// common indentation, validated as exactly one value nested under a probe key
/// and re-indented under the field's own indentation.
fn render_block_raw(raw: &str, indent: usize, line_ending: LineEnding) -> Result<String, String> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim_end();
    if trimmed.trim().is_empty() {
        return Err("value is empty".to_owned());
    }
    let first = trimmed.as_bytes()[0];
    if matches!(first, b'&' | b'*' | b'!' | b'|' | b'>') {
        return Err("anchors, aliases, tags and block scalars are not allowed".to_owned());
    }
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    let stripped: Vec<&str> = lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                ""
            } else {
                line.get(min_indent..).unwrap_or("").trim_end()
            }
        })
        .collect();
    let mut probe = String::from("value:\n");
    for line in &stripped {
        probe.push_str("  ");
        probe.push_str(line);
        probe.push('\n');
    }
    let documents =
        YamlLoader::load_from_str(&probe).map_err(|_| "value is not valid YAML".to_owned())?;
    match documents.as_slice() {
        [Yaml::Hash(map)]
            if map.len() == 1
                && map
                    .get(&Yaml::String("value".to_owned()))
                    .is_some_and(|value| !matches!(value, Yaml::BadValue)) => {}
        _ => return Err("value is not a single nested YAML value".to_owned()),
    }
    let eol = line_ending.as_str();
    let mut rendered = String::new();
    for line in &stripped {
        if !line.is_empty() {
            rendered.push_str(&" ".repeat(indent));
            rendered.push_str(line);
        }
        rendered.push_str(eol);
    }
    Ok(rendered)
}

/// Split a source into frontmatter and body, recording editable field spans.
///
/// # Errors
///
/// Returns [`DocumentError::InvalidFrontmatter`] for missing closing delimiters
/// or malformed YAML, and [`DocumentError::UnsupportedFrontmatter`] for
/// constructs outside the supported lossless subset.
pub fn parse_document(source: &str) -> Result<ParsedDocument, DocumentError> {
    let line_ending = if source.contains("\r\n") {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    };

    let Some(span) = scan_leading_frontmatter(source)? else {
        return Ok(ParsedDocument {
            source: source.to_owned(),
            line_ending,
            frontmatter: None,
            body: source.to_owned(),
            patches: Vec::new(),
            leading: None,
        });
    };

    let inner = &source[span.inner_start..span.closing_start];
    validate_yaml_mapping(inner)?;

    let mut fields = Vec::new();
    scan_fields(inner, span.inner_start, &mut fields)?;
    let public = fields.iter().map(public_field).collect();

    Ok(ParsedDocument {
        body: source[span.end..].to_owned(),
        source: source.to_owned(),
        line_ending,
        frontmatter: Some(Frontmatter {
            fields,
            public,
            end: span.closing_start,
        }),
        patches: Vec::new(),
        leading: None,
    })
}

/// Byte layout of one exact leading frontmatter block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrontmatterSpan {
    /// Offset just after the opening delimiter line.
    pub(crate) inner_start: usize,
    /// Offset of the closing delimiter line.
    pub(crate) closing_start: usize,
    /// Offset just after the closing delimiter line, including its line ending.
    pub(crate) end: usize,
}

/// Locate a frontmatter block that starts at the very beginning of `source`.
///
/// Only delimiter structure is inspected: the YAML in between is not validated,
/// so a writer projection can hide an otherwise malformed block exactly as it
/// appears. A byte-order mark before the opening delimiter belongs to the block.
///
/// # Errors
///
/// Returns [`DocumentError::InvalidFrontmatter`] when the source starts with an
/// opening delimiter that has no closing delimiter line.
pub(crate) fn scan_leading_frontmatter(
    source: &str,
) -> Result<Option<FrontmatterSpan>, DocumentError> {
    let bom = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let after_bom = &source[bom..];

    // No frontmatter unless the first line is exactly `---`.
    let Some(first_nl) = after_bom.find('\n') else {
        return if after_bom.trim_end_matches([' ', '\r']) == "---" {
            Err(DocumentError::InvalidFrontmatter)
        } else {
            Ok(None)
        };
    };
    if after_bom[..first_nl].trim_end_matches([' ', '\r']) != "---" {
        return Ok(None);
    }

    let inner_start = bom + first_nl + 1;
    let Some((closing_start, end)) = find_closing_delimiter(source, inner_start) else {
        return Err(DocumentError::InvalidFrontmatter);
    };
    Ok(Some(FrontmatterSpan {
        inner_start,
        closing_start,
        end,
    }))
}

fn public_field(field: &Field) -> FrontmatterField {
    FrontmatterField {
        path: field.path.clone(),
        shape: field.shape,
        value: (field.shape == FieldShape::Scalar).then(|| field.value.clone()),
        items: field.items.clone(),
        raw: field.raw.clone(),
        sequence_style: field.sequence_style,
    }
}

fn find_closing_delimiter(source: &str, start: usize) -> Option<(usize, usize)> {
    let mut pos = start;
    loop {
        let (content_end, next) = line_bounds(source, pos);
        if source[pos..content_end].trim_end_matches([' ', '\r']) == "---" {
            return Some((pos, next));
        }
        if next == pos {
            return None;
        }
        pos = next;
    }
}

fn validate_yaml_mapping(inner: &str) -> Result<(), DocumentError> {
    if inner.trim().is_empty() {
        return Ok(());
    }
    let documents =
        YamlLoader::load_from_str(inner).map_err(|_| DocumentError::InvalidFrontmatter)?;
    match documents.as_slice() {
        [] | [Yaml::BadValue] | [Yaml::Null] | [Yaml::Hash(_)] => Ok(()),
        _ => Err(DocumentError::InvalidFrontmatter),
    }
}

struct Line {
    start: usize,
    end: usize,
    next: usize,
}

fn line_bounds(text: &str, pos: usize) -> (usize, usize) {
    if pos >= text.len() {
        return (text.len(), text.len());
    }
    match text[pos..].find('\n') {
        Some(relative) => {
            let newline = pos + relative;
            let content_end = if newline > pos && text.as_bytes()[newline - 1] == b'\r' {
                newline - 1
            } else {
                newline
            };
            (content_end, newline + 1)
        }
        None => (text.len(), text.len()),
    }
}

fn split_lines(inner: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut pos = 0;
    while pos < inner.len() {
        let (end, next) = line_bounds(inner, pos);
        lines.push(Line {
            start: pos,
            end,
            next,
        });
        if next == pos {
            break;
        }
        pos = next;
    }
    lines
}

/// Number of leading spaces, rejecting tab indentation.
fn leading_indent(text: &str) -> Result<usize, DocumentError> {
    let mut count = 0;
    for byte in text.bytes() {
        match byte {
            b' ' => count += 1,
            b'\t' => {
                return Err(DocumentError::UnsupportedFrontmatter {
                    reason: "tab indentation is unsupported".to_owned(),
                });
            }
            _ => break,
        }
    }
    Ok(count)
}

fn is_sequence_item(content: &str) -> bool {
    content.starts_with('-')
        && content
            .as_bytes()
            .get(1)
            .is_none_or(|byte| *byte == b' ' || *byte == b'\t')
}

/// Whether an unquoted sequence item is a `key: value` mapping entry.
fn looks_like_mapping_item(rest: &str) -> bool {
    if rest.starts_with(['"', '\'']) {
        return false;
    }
    rest.contains(": ") || rest.ends_with(':')
}

/// The first non-blank, non-comment line after `index`.
fn next_content<'a>(inner: &'a str, lines: &[Line], index: usize) -> Option<(usize, &'a str)> {
    for line in &lines[index + 1..] {
        let text = &inner[line.start..line.end];
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = text.bytes().take_while(|byte| *byte == b' ').count();
        return Some((indent, &text[indent..]));
    }
    None
}

/// End offset (relative to `inner`) of the last line belonging to the block of a
/// container key at `index` and indentation `indent`.
fn container_end(inner: &str, lines: &[Line], index: usize, indent: usize) -> usize {
    let mut cursor = index + 1;
    let mut end = lines[index].next;
    while cursor < lines.len() {
        let text = &inner[lines[cursor].start..lines[cursor].end];
        let trimmed = text.trim();
        if trimmed.is_empty() {
            cursor += 1;
            continue;
        }
        let line_indent = text.bytes().take_while(|byte| *byte == b' ').count();
        if line_indent > indent {
            end = lines[cursor].next;
        } else if !trimmed.starts_with('#') {
            break;
        }
        cursor += 1;
    }
    end
}

/// Dedented, `\n`-normalized text of the block lines belonging to a key.
///
/// Offsets are relative to `inner`; `indent` is the block's child indentation.
/// The result is the text a raw editor shows, and re-indenting it by the same
/// amount reproduces the source block.
fn block_value_text(
    inner: &str,
    lines: &[Line],
    key_index: usize,
    end: usize,
    indent: usize,
) -> String {
    let start = lines[key_index].next;
    let mut text = String::new();
    for line in lines {
        if line.start < start || line.start >= end {
            continue;
        }
        let content = &inner[line.start..line.end];
        let leading = content.len() - content.trim_start_matches(' ').len();
        text.push_str(&content[leading.min(indent)..]);
        text.push('\n');
    }
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

fn scan_fields(inner: &str, base: usize, fields: &mut Vec<Field>) -> Result<(), DocumentError> {
    let lines = split_lines(inner);
    let mut frames: Vec<(usize, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &inner[lines[index].start..lines[index].end];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        let indent = leading_indent(line)?;
        let content = &line[indent..];
        if is_sequence_item(content) {
            return Err(DocumentError::UnsupportedFrontmatter {
                reason: "sequence items are only supported directly under a mapping key".to_owned(),
            });
        }
        while let Some((frame_indent, _)) = frames.last() {
            if *frame_indent >= indent {
                frames.pop();
            } else {
                break;
            }
        }
        let prefix = frames
            .last()
            .map(|(_, path)| path.clone())
            .unwrap_or_default();
        let (key, rest) = split_key(content)?;
        let path = if prefix.is_empty() {
            key
        } else {
            format!("{prefix}.{key}")
        };
        if !seen.insert(path.clone()) {
            return Err(DocumentError::InvalidFrontmatter);
        }

        let key_line_start = base + lines[index].start;
        let key_line = key_line_start..base + lines[index].next;
        let value_pos = key_line_start + (content.len() - rest.len());
        let rest_trimmed = rest.trim_start_matches(' ');
        if rest_trimmed.is_empty() || rest_trimmed.starts_with('#') {
            match next_content(inner, &lines, index) {
                // An indentless block sequence starts at the key's own
                // indentation, so equal indentation opens a child only for a
                // sequence item.
                Some((child_indent, child_content))
                    if child_indent > indent
                        || (child_indent == indent && is_sequence_item(child_content)) =>
                {
                    if is_sequence_item(child_content) {
                        let (field, next) = parse_block_sequence(
                            inner,
                            base,
                            &lines,
                            index,
                            &SequenceKey {
                                path: &path,
                                indent,
                                line: key_line,
                                value_pos,
                            },
                        )?;
                        fields.push(field);
                        index = next;
                    } else {
                        let block_end = base + container_end(inner, &lines, index, indent);
                        let raw =
                            block_value_text(inner, &lines, index, block_end - base, child_indent);
                        fields.push(Field {
                            path: path.clone(),
                            shape: FieldShape::Mapping,
                            value: FrontmatterValue::Null,
                            span: key_line_start..block_end,
                            kind: FieldKind::Opaque,
                            indent,
                            key_line,
                            block: key_line_start..block_end,
                            value_pos,
                            child_indent: Some(child_indent),
                            items: None,
                            raw: Some(raw),
                            sequence_style: None,
                            sequence: None,
                        });
                        frames.push((indent, path));
                        index += 1;
                    }
                }
                _ => {
                    let point = key_line_start + indent + content.len();
                    fields.push(Field {
                        path,
                        shape: FieldShape::Null,
                        value: FrontmatterValue::Null,
                        span: point..point,
                        kind: FieldKind::NullLeaf,
                        indent,
                        key_line: key_line.clone(),
                        block: key_line,
                        value_pos,
                        child_indent: None,
                        items: None,
                        raw: None,
                        sequence_style: None,
                        sequence: None,
                    });
                    index += 1;
                }
            }
            continue;
        }

        let value_offset = content.len() - rest_trimmed.len();
        let content_bytes = content.as_bytes();
        let first = content_bytes[value_offset];
        if matches!(first, b'&' | b'*' | b'!') {
            return Err(DocumentError::UnsupportedFrontmatter {
                reason: "anchors, aliases and tags are unsupported".to_owned(),
            });
        }

        if matches!(first, b'|' | b'>') {
            let (block_end, next_index) = block_scalar_range(inner, &lines, index, indent);
            let start = key_line_start + indent + value_offset;
            let end = base + block_end;
            fields.push(Field {
                path,
                shape: FieldShape::BlockScalar,
                value: FrontmatterValue::Null,
                span: start..end,
                kind: FieldKind::Opaque,
                indent,
                key_line: key_line.clone(),
                block: key_line_start..end,
                value_pos,
                child_indent: None,
                items: None,
                raw: None,
                sequence_style: None,
                sequence: None,
            });
            index = next_index;
            continue;
        }

        if matches!(first, b'[' | b'{') {
            let end_offset = flow_end(content, value_offset).ok_or_else(|| {
                DocumentError::UnsupportedFrontmatter {
                    reason: "multi-line flow collections are unsupported".to_owned(),
                }
            })?;
            ensure_trailing_is_comment(&content[end_offset..])?;
            let start = key_line_start + indent + value_offset;
            let end = key_line_start + indent + end_offset;
            let span = start..end;
            let text = &content[value_offset..end_offset];
            let mut field = Field {
                path,
                shape: FieldShape::Flow,
                value: FrontmatterValue::Null,
                span: span.clone(),
                kind: FieldKind::Opaque,
                indent,
                key_line: key_line.clone(),
                block: key_line.clone(),
                value_pos,
                child_indent: None,
                items: None,
                raw: Some(text.to_owned()),
                sequence_style: None,
                sequence: None,
            };
            if first == b'[' {
                field.shape = FieldShape::Sequence;
                field.sequence_style = Some(SequenceStyle::Flow);
                match parse_flow_sequence(text, span) {
                    Some(flow) => {
                        field.items = Some(flow.values);
                        field.sequence = Some(SequenceInfo::Flow(flow.data));
                    }
                    None => {
                        // The single-line raw text is still available, so the
                        // form can offer a validated raw-YAML control.
                        field.sequence = Some(SequenceInfo::Opaque);
                    }
                }
            }
            fields.push(field);
            index += 1;
            continue;
        }

        let (token, token_end) = scalar_token(content, value_offset)?;
        ensure_trailing_is_comment(&content[token_end..])?;
        let value = parse_scalar(token)?;
        let start = key_line_start + indent + value_offset;
        let end = key_line_start + indent + token_end;
        fields.push(Field {
            path,
            shape: FieldShape::Scalar,
            value,
            span: start..end,
            kind: FieldKind::Scalar,
            indent,
            key_line: key_line.clone(),
            block: key_line,
            value_pos,
            child_indent: None,
            items: None,
            raw: None,
            sequence_style: None,
            sequence: None,
        });
        index += 1;
    }
    Ok(())
}

struct ParsedFlow {
    values: Vec<FrontmatterValue>,
    data: FlowSequence,
}

fn parse_flow_sequence(text: &str, span: Range<usize>) -> Option<ParsedFlow> {
    let body = text.get(1..text.len().checked_sub(1)?)?;
    let relative = crate::yaml::split_flow_items(body)?;
    let mut item_spans = Vec::with_capacity(relative.len());
    let mut values = Vec::with_capacity(relative.len());
    let mut separators = Vec::new();
    for (position, range) in relative.iter().enumerate() {
        let slice = &body[range.clone()];
        let (token, token_end) = scalar_token(slice, 0).ok()?;
        if token_end != slice.len() {
            return None;
        }
        values.push(parse_scalar(token).ok()?);
        let start = span.start + 1 + range.start;
        item_spans.push(start..span.start + 1 + range.end);
        if let Some(next) = relative.get(position + 1) {
            separators.push(body[range.end..next.start].to_owned());
        }
    }
    Some(ParsedFlow {
        values,
        data: FlowSequence {
            collection: span,
            item_spans,
            separators,
        },
    })
}

/// The mapping key that owns a block sequence.
struct SequenceKey<'a> {
    path: &'a str,
    indent: usize,
    line: Range<usize>,
    value_pos: usize,
}

fn parse_block_sequence(
    inner: &str,
    base: usize,
    lines: &[Line],
    key_index: usize,
    key: &SequenceKey<'_>,
) -> Result<(Field, usize), DocumentError> {
    let mut cursor = key_index + 1;
    let mut item_indent: Option<usize> = None;
    let mut item_spans = Vec::new();
    let mut item_lines = Vec::new();
    let mut values = Vec::new();
    let mut editable = true;
    let mut last_end = base + lines[key_index].next;
    while cursor < lines.len() {
        let text = &inner[lines[cursor].start..lines[cursor].end];
        let trimmed = text.trim();
        if trimmed.is_empty() {
            cursor += 1;
            continue;
        }
        let indent = leading_indent(text)?;
        if indent < key.indent {
            break;
        }
        let content = &text[indent..];
        // Items may share the key's indentation (indentless sequence); then the
        // sequence ends at the next same-indent line that is not an item.
        if indent == key.indent && !is_sequence_item(content) && !trimmed.starts_with('#') {
            break;
        }
        last_end = base + lines[cursor].next;
        if trimmed.starts_with('#') {
            cursor += 1;
            continue;
        }
        let expected_indent = *item_indent.get_or_insert(indent);
        if indent != expected_indent || !is_sequence_item(content) {
            editable = false;
            cursor += 1;
            continue;
        }
        let rest = content[1..].trim_start_matches(' ');
        let item_offset = content.len() - rest.len();
        if rest.is_empty()
            || rest.starts_with('#')
            || is_sequence_item(rest)
            || looks_like_mapping_item(rest)
            || matches!(
                rest.as_bytes()[0],
                b'&' | b'*' | b'!' | b'|' | b'>' | b'[' | b'{'
            )
        {
            editable = false;
            cursor += 1;
            continue;
        }
        match scalar_token(content, item_offset) {
            Ok((token, token_end)) => {
                if ensure_trailing_is_comment(&content[token_end..]).is_err() {
                    editable = false;
                } else if let Ok(value) = parse_scalar(token) {
                    item_spans.push(
                        base + lines[cursor].start + indent + item_offset
                            ..base + lines[cursor].start + indent + token_end,
                    );
                    item_lines.push(base + lines[cursor].start..base + lines[cursor].next);
                    values.push(value);
                } else {
                    editable = false;
                }
            }
            Err(_) => editable = false,
        }
        cursor += 1;
    }

    let block = key.line.start..last_end;
    let item_indent = item_indent.unwrap_or(key.indent + 2);
    let raw = block_value_text(inner, lines, key_index, last_end - base, item_indent);
    let sequence = if editable {
        SequenceInfo::Block(BlockSequence {
            item_indent,
            item_spans,
            lines: item_lines,
        })
    } else {
        SequenceInfo::Opaque
    };
    let field = Field {
        path: key.path.to_owned(),
        shape: FieldShape::Sequence,
        value: FrontmatterValue::Null,
        span: block.clone(),
        kind: FieldKind::Opaque,
        indent: key.indent,
        key_line: key.line.clone(),
        block,
        value_pos: key.value_pos,
        child_indent: item_indent.into(),
        items: editable.then_some(values),
        raw: Some(raw),
        sequence_style: Some(SequenceStyle::Block),
        sequence: Some(sequence),
    };
    Ok((field, cursor))
}

fn split_key(content: &str) -> Result<(String, &str), DocumentError> {
    if content.starts_with('"') || content.starts_with('\'') {
        return Err(DocumentError::UnsupportedFrontmatter {
            reason: "quoted keys are unsupported".to_owned(),
        });
    }
    let mut separator = None;
    for (position, character) in content.char_indices() {
        if character == ':' {
            let next = content[position + 1..].chars().next();
            if next.is_none_or(|c| c == ' ' || c == '\t') {
                separator = Some(position);
                break;
            }
        }
        if character == '#' {
            break;
        }
    }
    let Some(separator) = separator else {
        return Err(DocumentError::UnsupportedFrontmatter {
            reason: "line is not a mapping entry".to_owned(),
        });
    };
    let key = content[..separator].trim();
    if key.is_empty() {
        return Err(DocumentError::InvalidFrontmatter);
    }
    if key.chars().any(|c| c.is_whitespace()) {
        return Err(DocumentError::UnsupportedFrontmatter {
            reason: "keys with whitespace are unsupported".to_owned(),
        });
    }
    if key
        .chars()
        .next()
        .is_some_and(|c| "-?:,[]{}#&*!|>'\"%@`".contains(c))
    {
        return Err(DocumentError::UnsupportedFrontmatter {
            reason: "complex keys are unsupported".to_owned(),
        });
    }
    Ok((key.to_owned(), &content[separator + 1..]))
}

fn block_scalar_range(inner: &str, lines: &[Line], index: usize, indent: usize) -> (usize, usize) {
    let mut cursor = index + 1;
    while cursor < lines.len() {
        let text = &inner[lines[cursor].start..lines[cursor].end];
        if text.trim().is_empty() {
            cursor += 1;
            continue;
        }
        let line_indent = text.bytes().take_while(|byte| *byte == b' ').count();
        if line_indent > indent {
            cursor += 1;
        } else {
            break;
        }
    }
    let end = if cursor == index + 1 {
        lines[index].next
    } else {
        lines[cursor - 1].next
    };
    (end, cursor)
}

fn flow_end(content: &str, start: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut depth = 0i32;
    let mut position = start;
    let mut quote: Option<u8> = None;
    while position < bytes.len() {
        let byte = bytes[position];
        if let Some(active) = quote {
            if active == b'"' && byte == b'\\' {
                position += 2;
                continue;
            }
            if byte == active {
                quote = None;
            }
            position += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(position + 1);
                }
            }
            _ => {}
        }
        position += 1;
    }
    None
}

fn ensure_trailing_is_comment(rest: &str) -> Result<(), DocumentError> {
    let trimmed = rest.trim_start_matches(' ');
    if trimmed.is_empty() || trimmed.starts_with('#') {
        Ok(())
    } else {
        Err(DocumentError::UnsupportedFrontmatter {
            reason: "trailing tokens after a scalar are unsupported".to_owned(),
        })
    }
}

fn scalar_token(content: &str, start: usize) -> Result<(&str, usize), DocumentError> {
    let bytes = content.as_bytes();
    let first = bytes[start];
    if first == b'\'' || first == b'"' {
        let mut position = start + 1;
        while position < bytes.len() {
            let byte = bytes[position];
            if first == b'"' && byte == b'\\' {
                position += 2;
                continue;
            }
            if byte == first {
                if first == b'\'' && bytes.get(position + 1) == Some(&b'\'') {
                    position += 2;
                    continue;
                }
                return Ok((&content[start..=position], position + 1));
            }
            position += 1;
        }
        return Err(DocumentError::InvalidFrontmatter);
    }

    let mut position = start;
    while position < bytes.len() {
        let byte = bytes[position];
        if byte == b'#' && (position == start || bytes[position - 1] == b' ') {
            break;
        }
        position += 1;
    }
    let end = content[start..position].trim_end_matches(' ').len() + start;
    Ok((&content[start..end], end))
}

fn parse_scalar(raw: &str) -> Result<FrontmatterValue, DocumentError> {
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        let inner = &raw[1..raw.len() - 1];
        return Ok(FrontmatterValue::String(inner.replace("''", "'")));
    }
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return Ok(FrontmatterValue::String(unescape_double(
            &raw[1..raw.len() - 1],
        )?));
    }
    Ok(match raw {
        "null" | "Null" | "NULL" | "~" => FrontmatterValue::Null,
        "true" | "True" | "TRUE" => FrontmatterValue::Boolean(true),
        "false" | "False" | "FALSE" => FrontmatterValue::Boolean(false),
        _ => {
            if let Ok(value) = raw.parse::<i64>() {
                FrontmatterValue::Integer(value)
            } else if let Ok(value) = raw.parse::<f64>() {
                FrontmatterValue::Float(value)
            } else {
                FrontmatterValue::String(raw.to_owned())
            }
        }
    })
}

fn unescape_double(inner: &str) -> Result<String, DocumentError> {
    let mut output = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('t') => output.push('\t'),
            Some('r') => output.push('\r'),
            Some('b') => output.push('\u{8}'),
            Some('f') => output.push('\u{c}'),
            Some('a') => output.push('\u{7}'),
            Some('v') => output.push('\u{b}'),
            Some('e') => output.push('\u{1b}'),
            Some('0') => output.push('\0'),
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some('/') => output.push('/'),
            Some(' ') => output.push(' '),
            Some('N') => output.push('\u{85}'),
            Some('_') => output.push('\u{a0}'),
            Some('L') => output.push('\u{2028}'),
            Some('P') => output.push('\u{2029}'),
            Some('x') => output.push(read_hex(&mut chars, 2)?),
            Some('u') => output.push(read_hex(&mut chars, 4)?),
            Some('U') => output.push(read_hex(&mut chars, 8)?),
            _ => {
                return Err(DocumentError::UnsupportedFrontmatter {
                    reason: "unsupported escape sequence".to_owned(),
                });
            }
        }
    }
    Ok(output)
}

fn read_hex(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    digits: usize,
) -> Result<char, DocumentError> {
    let mut hex = String::new();
    for _ in 0..digits {
        hex.push(chars.next().ok_or(DocumentError::InvalidFrontmatter)?);
    }
    let code = u32::from_str_radix(&hex, 16).map_err(|_| DocumentError::InvalidFrontmatter)?;
    char::from_u32(code).ok_or(DocumentError::InvalidFrontmatter)
}
