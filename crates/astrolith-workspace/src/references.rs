//! Reference-target options for schema `reference()` controls.
//!
//! A reference dropdown lists the entries of every collection named by the
//! schema. File-based collections come from the project index; JSON data
//! collections declared with a `file()` loader are read here. Labels prefer the
//! mapped title, then `name`, `title` or `slug`, and fall back to the entry id,
//! matching the reference pickers documented by the upstream editor.

use std::{fs, path::Path};

use astrolith_core::preferences::FieldMappings;
use serde_json::Value;

use crate::{CollectionEntry, MetaValue, WorkspaceError};

/// Upper bound on one JSON data-collection file.
pub const MAX_DATA_COLLECTION_BYTES: usize = 4 * 1024 * 1024;

/// One selectable reference target.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReferenceOption {
    /// Value stored in frontmatter (the entry id).
    pub value: String,
    /// Human-readable label.
    pub label: String,
}

/// Build the options for one file-based collection, preserving index order.
pub fn entry_options(
    entries: &[CollectionEntry],
    mappings: Option<&FieldMappings>,
) -> Vec<ReferenceOption> {
    entries
        .iter()
        .map(|entry| {
            let id = entry.entry().id().id();
            let label = label_from(mappings, id, |name| match entry.metadata().field(name) {
                Some(MetaValue::String(text)) => Some(text.as_str()),
                _ => None,
            });
            ReferenceOption {
                value: id.to_owned(),
                label,
            }
        })
        .collect()
}

/// Build options from one JSON data-collection document.
///
/// Entries are JSON objects; the identifier is `id`, else `slug`. Objects with
/// neither are skipped so a malformed row cannot produce a bogus option, while
/// a document that is not a JSON array is an error.
pub fn data_options(
    json: &str,
    mappings: Option<&FieldMappings>,
) -> Result<Vec<ReferenceOption>, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| format!("invalid JSON: {error}"))?;
    let Value::Array(entries) = value else {
        return Err("data collection must be a JSON array of objects".to_owned());
    };
    let mut options = Vec::new();
    for entry in entries {
        let Value::Object(object) = entry else {
            continue;
        };
        let string_field = |name: &str| object.get(name).and_then(Value::as_str);
        let Some(id) = string_field("id").or_else(|| string_field("slug")) else {
            continue;
        };
        if id.trim().is_empty() {
            continue;
        }
        let label = label_from(mappings, id, string_field);
        options.push(ReferenceOption {
            value: id.to_owned(),
            label,
        });
    }
    Ok(options)
}

/// Read and label one JSON data-collection file.
pub fn read_data_options(
    path: &Path,
    mappings: Option<&FieldMappings>,
) -> Result<Vec<ReferenceOption>, WorkspaceError> {
    let metadata = fs::metadata(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(WorkspaceError::InvalidData {
            path: path.to_path_buf(),
            message: "data collection is not a regular file".to_owned(),
        });
    }
    if metadata.len() > MAX_DATA_COLLECTION_BYTES as u64 {
        return Err(WorkspaceError::InvalidData {
            path: path.to_path_buf(),
            message: format!("data collection exceeds {MAX_DATA_COLLECTION_BYTES} bytes"),
        });
    }
    let text = fs::read_to_string(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    data_options(&text, mappings).map_err(|message| WorkspaceError::InvalidData {
        path: path.to_path_buf(),
        message,
    })
}

/// Resolve a label using the mapped title, `name`, `title`, `slug`, then the id.
fn label_from<'id, 'value>(
    mappings: Option<&FieldMappings>,
    id: &'id str,
    field: impl Fn(&str) -> Option<&'value str>,
) -> String {
    let mapped = mappings.and_then(|mappings| mappings.title.as_deref());
    let mut candidates = Vec::with_capacity(4);
    if let Some(mapped) = mapped {
        candidates.push(mapped);
    }
    for name in ["name", "title", "slug"] {
        if !candidates.contains(&name) {
            candidates.push(name);
        }
    }
    for name in candidates {
        if let Some(value) = field(name)
            && !value.trim().is_empty()
        {
            return value.to_owned();
        }
    }
    id.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_options_label_from_title_name_slug_else_id() {
        let json = r#"[
            {"id": "ada", "name": "Ada Lovelace", "title": "Ignored", "slug": "ada"},
            {"id": "grace", "title": "Grace Hopper", "slug": "grace"},
            {"slug": "linus-t", "name": "Linus"},
            {"id": "bare"},
            {"id": "missing-label", "name": ""}
        ]"#;
        let options = data_options(json, None).unwrap();
        assert_eq!(
            options,
            vec![
                ReferenceOption {
                    value: "ada".into(),
                    label: "Ada Lovelace".into()
                },
                ReferenceOption {
                    value: "grace".into(),
                    label: "Grace Hopper".into()
                },
                ReferenceOption {
                    value: "linus-t".into(),
                    label: "Linus".into()
                },
                ReferenceOption {
                    value: "bare".into(),
                    label: "bare".into()
                },
                ReferenceOption {
                    value: "missing-label".into(),
                    label: "missing-label".into()
                },
            ]
        );
    }

    #[test]
    fn data_options_prefer_the_mapped_title_field() {
        let json = r#"[{"id": "ada", "name": "Ada Lovelace", "headline": "Pioneer"}]"#;
        let mappings = FieldMappings {
            title: Some("headline".into()),
            ..FieldMappings::default()
        };
        let options = data_options(json, Some(&mappings)).unwrap();
        assert_eq!(options[0].label, "Pioneer");
    }

    #[test]
    fn data_options_skip_rows_without_an_identifier() {
        let json = r#"[{"name": "No id"}, "scalar", {"id": "ok"}]"#;
        let options = data_options(json, None).unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].value, "ok");
    }

    #[test]
    fn data_options_reject_non_array_documents() {
        assert!(data_options("{}", None).is_err());
        assert!(data_options("not json", None).is_err());
    }
}
