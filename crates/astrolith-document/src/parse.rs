//! Lossless frontmatter document model.
//!
//! [`parse_document`] splits a Markdown/MDX source into its exact frontmatter and
//! body bytes and records the byte span of each supported scalar field so a
//! caller can patch one value in place. [`serialize_document`](crate::serialize::serialize_document)
//! then reproduces every untouched byte exactly, including comments, unknown
//! keys, quoting style, block scalars, Unicode, line endings and trailing
//! newlines.
//!
//! The supported editable subset is a YAML block mapping of plain keys whose
//! values are plain or quoted scalars, including nested block mappings. Flow
//! collections (`[...]`, `{...}`), block scalars (`|`, `>`) and complex
//! constructs are preserved as opaque fields that can be read but not patched;
//! anchors, aliases, tags, merge keys, quoted or complex keys and sequence items
//! are refused outright with [`DocumentError::UnsupportedFrontmatter`]. Refusing
//! unsupported input is safer than guessing: it can never corrupt a document.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::{Yaml, YamlLoader};

use crate::buffer::DocumentError;

/// The dominant line ending of a document, preserved on serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    /// LF (`\n`) line endings.
    Lf,
    /// CRLF (`\r\n`) line endings.
    Crlf,
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

#[derive(Clone, Debug)]
struct Field {
    path: String,
    value: FrontmatterValue,
    span: Range<usize>,
    kind: FieldKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    /// A patchable scalar leaf.
    Scalar,
    /// A `key:` with a null value; readable but not patchable.
    NullLeaf,
    /// A flow collection or block scalar; preserved verbatim, not patchable.
    Opaque,
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
}

/// A parsed document that can be serialized back to its exact source bytes.
#[derive(Clone, Debug)]
pub struct ParsedDocument {
    source: String,
    line_ending: LineEnding,
    frontmatter: Option<Frontmatter>,
    body: String,
    patches: Vec<Patch>,
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
    pub fn field(&self, path: &str) -> Option<&FrontmatterValue> {
        self.frontmatter
            .as_ref()?
            .fields
            .iter()
            .find(|field| field.path == path && field.kind != FieldKind::Opaque)
            .map(|field| &field.value)
    }

    /// Record an in-place scalar patch for one field.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnknownField`] when `path` is absent,
    /// [`DocumentError::UnsupportedPatch`] when the field is not a patchable
    /// scalar (flow collection, block scalar, or otherwise opaque), or when the
    /// value cannot be rendered as a safe YAML scalar.
    pub fn set_field(&mut self, path: &str, value: FrontmatterValue) -> Result<(), DocumentError> {
        let Some(field) = self
            .frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter.fields.iter().find(|field| field.path == path))
        else {
            return Err(DocumentError::UnknownField {
                path: path.to_owned(),
            });
        };
        if field.kind != FieldKind::Scalar {
            let reason = match field.kind {
                FieldKind::NullLeaf => "null fields are not editable",
                _ => "field is an opaque collection or block scalar",
            };
            return Err(DocumentError::UnsupportedPatch {
                path: path.to_owned(),
                reason: reason.to_owned(),
            });
        }
        let span = field.span.clone();
        let replacement = crate::serialize::serialize_scalar(&value).map_err(|reason| {
            DocumentError::UnsupportedPatch {
                path: path.to_owned(),
                reason,
            }
        })?;
        self.patches.retain(|patch| patch.path != path);
        self.patches.push(Patch {
            path: path.to_owned(),
            span,
            replacement,
        });
        Ok(())
    }

    /// Serialize the document, applying any recorded patches.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnsupportedPatch`] if recorded patches overlap,
    /// which would make the output ambiguous.
    pub fn serialize(&self) -> Result<String, DocumentError> {
        if self.patches.is_empty() {
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
        let mut output = String::with_capacity(self.source.len());
        let mut cursor = 0;
        for patch in ordered {
            output.push_str(&self.source[cursor..patch.span.start]);
            output.push_str(&patch.replacement);
            cursor = patch.span.end;
        }
        output.push_str(&self.source[cursor..]);
        Ok(output)
    }
}

/// Split a source into frontmatter and body, recording editable scalar spans.
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

    let bom = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let after_bom = &source[bom..];

    // No frontmatter unless the first line is exactly `---`.
    let Some(first_nl) = after_bom.find('\n') else {
        if after_bom.trim_end_matches([' ', '\r']) == "---" {
            return Err(DocumentError::InvalidFrontmatter);
        }
        return Ok(ParsedDocument {
            source: source.to_owned(),
            line_ending,
            frontmatter: None,
            body: source.to_owned(),
            patches: Vec::new(),
        });
    };
    if after_bom[..first_nl].trim_end_matches([' ', '\r']) != "---" {
        return Ok(ParsedDocument {
            source: source.to_owned(),
            line_ending,
            frontmatter: None,
            body: source.to_owned(),
            patches: Vec::new(),
        });
    }

    let inner_start = bom + first_nl + 1;
    let Some((closing_start, closing_next)) = find_closing_delimiter(source, inner_start) else {
        return Err(DocumentError::InvalidFrontmatter);
    };
    let inner = &source[inner_start..closing_start];
    validate_yaml_mapping(inner)?;

    let mut fields = Vec::new();
    scan_fields(inner, inner_start, &mut fields)?;

    Ok(ParsedDocument {
        body: source[closing_next..].to_owned(),
        source: source.to_owned(),
        line_ending,
        frontmatter: Some(Frontmatter { fields }),
        patches: Vec::new(),
    })
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

fn next_content_indent(inner: &str, lines: &[Line], index: usize) -> Option<usize> {
    for line in &lines[index + 1..] {
        let text = &inner[line.start..line.end];
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        return Some(text.bytes().take_while(|byte| *byte == b' ').count());
    }
    None
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
        if content.starts_with('-')
            && content
                .as_bytes()
                .get(1)
                .is_none_or(u8::is_ascii_whitespace)
        {
            return Err(DocumentError::UnsupportedFrontmatter {
                reason: "sequence items are unsupported".to_owned(),
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

        let rest_trimmed = rest.trim_start_matches(' ');
        if rest_trimmed.is_empty() || rest_trimmed.starts_with('#') {
            let is_parent =
                next_content_indent(inner, &lines, index).is_some_and(|child| child > indent);
            if is_parent {
                frames.push((indent, path));
            } else {
                let point = base + lines[index].start + indent + content.len();
                fields.push(Field {
                    path,
                    value: FrontmatterValue::Null,
                    span: point..point,
                    kind: FieldKind::NullLeaf,
                });
            }
            index += 1;
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
            let start = base + lines[index].start + indent + value_offset;
            let end = base + block_end;
            fields.push(Field {
                path,
                value: FrontmatterValue::Null,
                span: start..end,
                kind: FieldKind::Opaque,
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
            let start = base + lines[index].start + indent + value_offset;
            let end = base + lines[index].start + indent + end_offset;
            fields.push(Field {
                path,
                value: FrontmatterValue::Null,
                span: start..end,
                kind: FieldKind::Opaque,
            });
            index += 1;
            continue;
        }

        let (token, token_end) = scalar_token(content, value_offset)?;
        ensure_trailing_is_comment(&content[token_end..])?;
        let value = parse_scalar(token)?;
        let start = base + lines[index].start + indent + value_offset;
        let end = base + lines[index].start + indent + token_end;
        fields.push(Field {
            path,
            value,
            span: start..end,
            kind: FieldKind::Scalar,
        });
        index += 1;
    }
    Ok(())
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
