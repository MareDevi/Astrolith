//! Serialization of a parsed document and of individual YAML scalars.
//!
//! Serialization is lossless: a document with no recorded patches is emitted as
//! its original bytes, and patched documents splice only the changed scalar
//! spans so comments, unknown keys, block scalars and body bytes are untouched.

use yaml_rust2::{Yaml, YamlLoader};

use crate::buffer::DocumentError;
use crate::parse::{FrontmatterValue, ParsedDocument};

/// Serialize a document, applying any recorded scalar patches.
///
/// # Errors
///
/// Propagates [`DocumentError::UnsupportedPatch`] when patches overlap.
pub fn serialize_document(document: &ParsedDocument) -> Result<String, DocumentError> {
    document.serialize()
}

/// Render a typed value as a safe YAML scalar, quoting strings when needed.
///
/// Returns the reason as an error string when the value has no safe rendering.
pub(crate) fn serialize_scalar(value: &FrontmatterValue) -> Result<String, String> {
    Ok(match value {
        FrontmatterValue::Null => "null".to_owned(),
        FrontmatterValue::Boolean(value) => {
            if *value {
                "true".to_owned()
            } else {
                "false".to_owned()
            }
        }
        FrontmatterValue::Integer(value) => value.to_string(),
        FrontmatterValue::Float(value) => {
            if !value.is_finite() {
                return Err("non-finite numbers cannot be serialized".to_owned());
            }
            let mut text = value.to_string();
            if !text.contains(['.', 'e', 'E']) {
                text.push_str(".0");
            }
            text
        }
        FrontmatterValue::String(text) => {
            if text.chars().any(char::is_control) {
                return Err(
                    "strings with control characters cannot be patched as a single scalar"
                        .to_owned(),
                );
            }
            render_string(text)
        }
    })
}

fn render_string(text: &str) -> String {
    if plain_string_is_safe(text) {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "''"))
    }
}

fn plain_string_is_safe(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let first = text.chars().next().expect("non-empty string");
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        return false;
    }
    if text.starts_with(' ') || text.ends_with(' ') {
        return false;
    }
    if text.chars().any(char::is_control) {
        return false;
    }
    if text.contains(": ") || text.contains(" #") {
        return false;
    }
    // Authoritative check: the rendered `key: value` line must parse back to the
    // exact same string under YAML, not merely under this crate's own parser.
    plain_scalar_reads_back_as_string(text)
}

fn plain_scalar_reads_back_as_string(text: &str) -> bool {
    let probe = format!("k: {text}\n");
    match YamlLoader::load_from_str(&probe).ok().as_deref() {
        Some([Yaml::Hash(map)]) if map.len() == 1 => map.iter().any(|(key, value)| {
            matches!(
                (key, value),
                (Yaml::String(name), Yaml::String(inner))
                    if name.as_str() == "k" && inner.as_str() == text
            )
        }),
        _ => false,
    }
}
