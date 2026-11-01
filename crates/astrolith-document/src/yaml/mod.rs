//! Raw YAML value validation and flow-sequence item scanning.
//!
//! Frontmatter field spans and byte patches are owned by [`crate::parse`] and
//! [`crate::serialize`]; this module only answers "is this exact text a single
//! usable YAML value?" so callers (including live editors) can validate input
//! before recording a patch.

use std::ops::Range;

use yaml_rust2::{Yaml, YamlLoader};

/// Validate that `raw` is exactly one YAML value usable inline in frontmatter.
///
/// Multi-document input, anchors, aliases, tags, block scalars and anything that
/// is not a scalar or single-line flow collection are rejected.
///
/// # Errors
///
/// Returns the reason as a human-readable string when the text is not a single
/// safe YAML value.
pub fn validate_value(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("value is empty".to_owned());
    }
    if trimmed.contains(['\n', '\r']) {
        return Err("value must fit on a single line".to_owned());
    }
    let first = trimmed.as_bytes()[0];
    if matches!(first, b'&' | b'*' | b'!') {
        return Err("anchors, aliases and tags are not allowed".to_owned());
    }
    if matches!(first, b'|' | b'>') {
        return Err("block scalars are not allowed".to_owned());
    }
    let probe = format!("value: {trimmed}\n");
    let Ok(documents) = YamlLoader::load_from_str(&probe) else {
        return Err("value is not valid YAML".to_owned());
    };
    match documents.as_slice() {
        [Yaml::Hash(map)] if map.len() == 1 => match map.get(&Yaml::String("value".to_owned())) {
            Some(Yaml::BadValue) | None => Err("value is not valid YAML".to_owned()),
            Some(_) => Ok(()),
        },
        _ => Err("value is not a single YAML value".to_owned()),
    }
}

/// Split the body of a single-line flow sequence (the text between `[` and `]`)
/// into trimmed item ranges, relative to `body`.
///
/// Returns `None` when an item is not a plain or quoted scalar, including nested
/// flow collections, anchors, aliases, tags, block scalar markers, comments and
/// empty items.
pub(crate) fn split_flow_items(body: &str) -> Option<Vec<Range<usize>>> {
    if body.trim().is_empty() {
        return Some(Vec::new());
    }
    let bytes = body.as_bytes();
    let mut items = Vec::new();
    let mut start = 0;
    let mut quote: Option<u8> = None;
    let mut position = 0;
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
            b',' => {
                items.push(trimmed_range(body, start, position)?);
                start = position + 1;
            }
            b'[' | b']' | b'{' | b'}' | b'&' | b'*' | b'!' | b'|' | b'>' | b'#' | b'\t' => {
                return None;
            }
            _ => {}
        }
        position += 1;
    }
    if quote.is_some() {
        return None;
    }
    items.push(trimmed_range(body, start, bytes.len())?);
    if !items
        .iter()
        .all(|range| is_scalar_item(&body[range.clone()]))
    {
        return None;
    }
    Some(items)
}

fn trimmed_range(body: &str, start: usize, end: usize) -> Option<Range<usize>> {
    let bytes = body.as_bytes();
    let mut first = start;
    while first < end && bytes[first] == b' ' {
        first += 1;
    }
    let mut last = end;
    while last > first && bytes[last - 1] == b' ' {
        last -= 1;
    }
    (first < last).then_some(first..last)
}

fn is_scalar_item(text: &str) -> bool {
    let bytes = text.as_bytes();
    let first = bytes[0];
    if first == b'\'' || first == b'"' {
        let mut position = 1;
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
                return position + 1 == bytes.len();
            }
            position += 1;
        }
        return false;
    }
    if text.contains(['[', ']', '{', '}', '&', '*', '!', '|', '>', '#']) {
        return false;
    }
    if text.contains(": ") || text.ends_with(':') {
        return false;
    }
    !(text == "-" || text.starts_with("- "))
}
