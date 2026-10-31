//! Local, filesystem- and GUI-free frontmatter validation.
//!
//! This is the first of two deliberately separate validation operations. It
//! checks already-parsed frontmatter values (a [`serde_json::Value`]) against a
//! normalized [`CollectionSchema`] and reports structured diagnostics keyed by
//! field path. It never runs Astro, reads files, or touches the UI. The second
//! operation — Astro tooling validation — lives in
//! [`super::validation_tooling`] and is intentionally independent so a local
//! check passing is never mistaken for Astro accepting the content.

use serde_json::{Map, Value};

use crate::schema::model::{CollectionSchema, Field, FieldKind};

/// How severe a validation finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationSeverity {
    /// The value does not satisfy the schema.
    Error,
    /// The value cannot be checked (for example an unsupported field kind).
    Warning,
}

/// Which validation operation produced a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationSource {
    /// The local schema checker ([`validate_frontmatter`]).
    Local,
    /// The Astro tooling operation ([`super::validation_tooling`]).
    AstroTooling,
}

/// One structured validation finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationDiagnostic {
    /// Dotted field path (local checks) or project-relative location (tooling).
    pub path: String,
    /// Human-readable description.
    pub message: String,
    pub severity: ValidationSeverity,
    pub source: ValidationSource,
}

impl ValidationDiagnostic {
    pub(crate) fn error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            severity: ValidationSeverity::Error,
            source: ValidationSource::Local,
        }
    }

    pub(crate) fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            severity: ValidationSeverity::Warning,
            source: ValidationSource::Local,
        }
    }

    pub(crate) fn tooling(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            severity: ValidationSeverity::Error,
            source: ValidationSource::AstroTooling,
        }
    }
}

/// Validate parsed frontmatter values against a normalized collection schema.
///
/// `values` is the JSON mapping produced by parsing the document frontmatter.
/// Diagnostics are returned sorted by `(path, message)` and de-duplicated. This
/// function is pure: it performs no I/O and allocates only its output.
pub fn validate_frontmatter(
    schema: &CollectionSchema,
    values: &Value,
) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();
    let Some(object) = values.as_object() else {
        diagnostics.push(ValidationDiagnostic::error(
            "<root>",
            "frontmatter must be a mapping of field names to values",
        ));
        return diagnostics;
    };
    validate_fields(&schema.fields, object, "", &mut diagnostics);
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.message.cmp(&right.message))
    });
    diagnostics.dedup();
    diagnostics
}

fn validate_fields(
    fields: &[Field],
    object: &Map<String, Value>,
    prefix: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for field in fields {
        let path = child_path(prefix, &field.name);
        match object.get(&field.name) {
            None => {
                if field.required {
                    diagnostics.push(ValidationDiagnostic::error(
                        path,
                        "required field is missing",
                    ));
                }
            }
            Some(value) => validate_value(&field.kind, value, &path, diagnostics),
        }
    }
}

fn validate_value(
    kind: &FieldKind,
    value: &Value,
    path: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    // Null is only valid for an explicitly nullable field (and unknown for an
    // unsupported one, which we report separately).
    if value.is_null() {
        match kind {
            FieldKind::Nullable { .. } | FieldKind::Unsupported { .. } => {}
            _ => {
                diagnostics.push(ValidationDiagnostic::error(
                    path,
                    "null is not allowed for this field",
                ));
                return;
            }
        }
    }

    match kind {
        FieldKind::Nullable { inner } => {
            if !value.is_null() {
                validate_value(inner, value, path, diagnostics);
            }
        }
        FieldKind::String { format } => match value.as_str() {
            None => diagnostics.push(ValidationDiagnostic::error(path, "expected a string")),
            Some(text) => {
                if let Some(format) = format {
                    validate_format(format, text, path, diagnostics);
                }
            }
        },
        FieldKind::Number => {
            if !value.is_number() {
                diagnostics.push(ValidationDiagnostic::error(path, "expected a number"));
            }
        }
        FieldKind::Integer => {
            if !is_integer(value) {
                diagnostics.push(ValidationDiagnostic::error(path, "expected an integer"));
            }
        }
        FieldKind::Boolean => {
            if !value.is_boolean() {
                diagnostics.push(ValidationDiagnostic::error(path, "expected a boolean"));
            }
        }
        FieldKind::Date => {
            if !is_valid_date(value) {
                diagnostics.push(ValidationDiagnostic::error(
                    path,
                    "expected a date or timestamp string, or a Unix time integer",
                ));
            }
        }
        FieldKind::Enum { values } => {
            if !values.contains(value) {
                let allowed = values
                    .iter()
                    .map(render_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                diagnostics.push(ValidationDiagnostic::error(
                    path,
                    format!("expected one of [{allowed}], found {}", render_value(value)),
                ));
            }
        }
        FieldKind::Literal { value: expected } => {
            if expected != value {
                diagnostics.push(ValidationDiagnostic::error(
                    path,
                    format!(
                        "expected the literal {}, found {}",
                        render_value(expected),
                        render_value(value)
                    ),
                ));
            }
        }
        FieldKind::Array { items } => match value.as_array() {
            None => diagnostics.push(ValidationDiagnostic::error(path, "expected an array")),
            Some(items_values) => {
                for (index, item) in items_values.iter().enumerate() {
                    let item_path = format!("{path}[{index}]");
                    validate_value(items, item, &item_path, diagnostics);
                }
            }
        },
        FieldKind::Object { fields } => match value.as_object() {
            None => diagnostics.push(ValidationDiagnostic::error(path, "expected an object")),
            Some(nested) => validate_fields(fields, nested, path, diagnostics),
        },
        FieldKind::Record { value: value_kind } => match value.as_object() {
            None => diagnostics.push(ValidationDiagnostic::error(
                path,
                "expected a mapping of string keys to values",
            )),
            Some(entries) => {
                for (key, entry) in entries {
                    let entry_path = format!("{path}.{key}");
                    validate_value(value_kind, entry, &entry_path, diagnostics);
                }
            }
        },
        FieldKind::Image => match value.as_str() {
            Some(text) if !text.trim().is_empty() => {}
            Some(_) => diagnostics.push(ValidationDiagnostic::error(
                path,
                "image path must not be empty",
            )),
            None => diagnostics.push(ValidationDiagnostic::error(
                path,
                "expected an image path string",
            )),
        },
        FieldKind::Reference { collections } => {
            validate_reference(value, collections, path, diagnostics);
        }
        FieldKind::Unsupported { reason } => {
            if !value.is_null() {
                diagnostics.push(ValidationDiagnostic::warning(
                    path,
                    format!("cannot validate unsupported field: {reason}"),
                ));
            }
        }
    }
}

fn validate_format(
    format: &str,
    text: &str,
    path: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    // Other formats (uri, url, uuid, ...) are accepted: the JSON Schema `format`
    // keyword is an annotation and formats vary across emitters. Astro's
    // generated email schema also carries a pattern; the local check stays
    // deliberately conservative to avoid false rejections.
    if format == "email" && !looks_like_email(text) {
        diagnostics.push(ValidationDiagnostic::error(
            path,
            "expected an email address",
        ));
    }
}

fn validate_reference(
    value: &Value,
    collections: &[String],
    path: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let target = if !collections.is_empty() {
        format!("reference to [{}]", collections.join(", "))
    } else {
        "reference".to_owned()
    };
    if let Some(text) = value.as_str() {
        if text.trim().is_empty() {
            diagnostics.push(ValidationDiagnostic::error(
                path,
                format!("{target} must not be empty"),
            ));
        }
        return;
    }
    if let Some(object) = value.as_object() {
        let has_id = object.get("id").is_some_and(Value::is_string);
        let has_slug = object.get("slug").is_some_and(Value::is_string);
        if has_id || has_slug {
            return;
        }
    }
    diagnostics.push(ValidationDiagnostic::error(
        path,
        format!("expected a {target} entry id or slug"),
    ));
}

fn is_integer(value: &Value) -> bool {
    value.as_i64().is_some()
        || value.as_u64().is_some()
        || value.as_f64().is_some_and(|number| number.fract() == 0.0)
}

fn is_valid_date(value: &Value) -> bool {
    if value.as_i64().is_some() || value.as_u64().is_some() {
        return true;
    }
    value.as_str().is_some_and(looks_like_date)
}

fn looks_like_date(text: &str) -> bool {
    let date_part = text.split(['T', ' ']).next().unwrap_or(text);
    let mut parts = date_part.split('-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<i32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    year >= 0 && (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day)
}

/// Number of days in `month` for `year`, honouring Gregorian leap years.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn looks_like_email(text: &str) -> bool {
    let mut parts = text.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && !text.contains(char::is_whitespace)
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => other.to_string(),
    }
}

fn child_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}.{name}")
    }
}
