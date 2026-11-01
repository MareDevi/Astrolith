//! Mapping from collection-schema fields and parsed frontmatter to native form
//! controls, plus the value-edit bridge onto the lossless document patch
//! contract.
//!
//! This module is deliberately free of GPUI types so ordering, metadata,
//! control selection and edit rules can be unit-tested headlessly. The GPUI
//! rendering in [`super`] consumes [`FieldControl`] values produced here.
//!
//! Ordering follows the upstream panel: the mapped title first, the remaining
//! schema fields in schema declaration order, then document fields that the
//! schema does not know, alphabetically. A collection without a schema shows
//! only the fields that actually exist.
//!
//! Control selection never coerces: a value whose YAML shape the typed control
//! cannot represent losslessly is either a validated raw-YAML control (single-line
//! flow containers, block mappings and sequences of non-scalar items) or an
//! explicit read-only refusal (block scalars, anchors and other constructs the
//! parser already refuses). A `key:` null leaf with no comment is an editable
//! empty scalar. Existing values are never rewritten just by opening the form.

use std::collections::BTreeMap;

use astrolith_astro::schema::{Field, FieldConstraints, FieldKind};
use astrolith_document::{
    DocumentError, FieldShape, FrontmatterField, FrontmatterValue, ParsedDocument,
};
use astrolith_workspace::ReferenceOption;
use serde_json::Value;

use astrolith_core::preferences::FieldMappings;
use astrolith_workspace::list::{DEFAULT_DATE_FIELD, DEFAULT_DRAFT_FIELD, DEFAULT_TITLE_FIELD};

/// Reference options grouped by target collection name.
pub type ReferenceCatalog = BTreeMap<String, Vec<ReferenceOption>>;

/// The semantic role a mapped field plays in the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldRole {
    /// The mapped title; rendered first with a copy-as-slug action.
    Title,
    /// The mapped description; rendered as a tall textarea.
    Description,
    /// The mapped draft flag; drives badges and the draft filter.
    Draft,
    /// The mapped publication date; drives list order.
    Date,
}

/// The native control family a field maps to.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlKind {
    /// A plain text input, optionally with a semantic format hint.
    Text {
        /// Schema format hint such as `email` or `uri`.
        format: Option<String>,
    },
    /// A floating-point number input.
    Number,
    /// An integer input.
    Integer,
    /// A boolean toggle.
    Boolean,
    /// A date input backed by a native date picker.
    Date,
    /// An Astro `image()` path control.
    Image,
    /// A single-choice dropdown from a fixed value set.
    Enum {
        /// Allowed values.
        values: Vec<Value>,
    },
    /// A reference dropdown labelled from the target entries.
    Reference {
        /// Target collections in schema declaration order.
        collections: Vec<String>,
        /// Whether several values are allowed.
        multiple: bool,
    },
    /// A string-array control rendered as removable tags.
    Tags,
    /// A nested object group; its children carry their own controls.
    Object,
    /// A validated raw-YAML value editor.
    RawYaml {
        /// Why the typed control cannot represent the value.
        reason: String,
    },
    /// A fixed literal value from the schema.
    Literal {
        /// The fixed value.
        value: Value,
    },
    /// A value preserved verbatim and not editable without risking data loss.
    ReadOnly {
        /// Why editing is refused.
        reason: String,
    },
}

/// One field rendered by the form, with its resolved control and metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldControl {
    /// Frontmatter key (leaf name).
    pub name: String,
    /// Dot-separated path from the frontmatter root (e.g. `author.name`).
    pub path: String,
    /// Whether the schema marks the field required.
    pub required: bool,
    /// Resolved control family.
    pub kind: ControlKind,
    /// Whether an edit at this path can be persisted losslessly.
    pub editable: bool,
    /// Whether the field is currently present in the document.
    pub present: bool,
    /// Whether the field may be removed without breaking a required field.
    pub removable: bool,
    /// Explicit refusal reason when the field is shown read-only.
    pub refusal: Option<String>,
    /// Whether the field also accepts `null`.
    pub nullable: bool,
    /// Schema-authored description.
    pub description: Option<String>,
    /// Range, length and pattern constraints.
    pub constraints: FieldConstraints,
    /// Schema default, shown as a hint and never injected into existing files.
    pub default: Option<Value>,
    /// Mapped semantic role.
    pub role: Option<FieldRole>,
    /// Zero-based render position.
    pub order: usize,
}

impl FieldControl {
    /// Short label for the control family, used in tests and diagnostics.
    pub fn control_label(&self) -> &'static str {
        control_label(&self.kind)
    }

    /// A one-line hint combining the default and the declared constraints.
    pub fn hint(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(default) = &self.default {
            parts.push(format!("Default: {}", display_json(default)));
        }
        if let Some(minimum) = self.constraints.minimum {
            parts.push(format!("Min: {minimum}"));
        }
        if let Some(maximum) = self.constraints.maximum {
            parts.push(format!("Max: {maximum}"));
        }
        match (self.constraints.min_length, self.constraints.max_length) {
            (Some(min), Some(max)) => parts.push(format!("Length: {min}–{max}")),
            (Some(min), None) => parts.push(format!("Min length: {min}")),
            (None, Some(max)) => parts.push(format!("Max length: {max}")),
            (None, None) => {}
        }
        if let Some(pattern) = &self.constraints.pattern {
            parts.push(format!("Pattern: {pattern}"));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// Return the human-readable family name for a control kind.
pub fn control_label(kind: &ControlKind) -> &'static str {
    match kind {
        ControlKind::Text { .. } => "text",
        ControlKind::Number => "number",
        ControlKind::Integer => "integer",
        ControlKind::Boolean => "boolean",
        ControlKind::Date => "date",
        ControlKind::Image => "image",
        ControlKind::Enum { .. } => "enum",
        ControlKind::Reference { multiple: true, .. } => "reference list",
        ControlKind::Reference { .. } => "reference",
        ControlKind::Tags => "tags",
        ControlKind::Object => "object",
        ControlKind::RawYaml { .. } => "raw yaml",
        ControlKind::Literal { .. } => "literal",
        ControlKind::ReadOnly { .. } => "read-only",
    }
}

/// Build the ordered, metadata-carrying control list for one document.
///
/// `schema` is `None` for a collection without a generated schema; only the
/// fields present in the document are then listed, alphabetically.
pub fn build_controls(
    schema: Option<&astrolith_astro::schema::CollectionSchema>,
    mappings: &FieldMappings,
    document_fields: &[FrontmatterField],
) -> Vec<FieldControl> {
    let title = mappings.title.as_deref().unwrap_or(DEFAULT_TITLE_FIELD);
    let mut controls = Vec::new();
    match schema {
        Some(schema) => {
            let mut schema_fields: Vec<&Field> = schema.fields.iter().collect();
            schema_fields.sort_by_key(|field| field.order);
            if let Some(position) = schema_fields.iter().position(|field| field.name == title) {
                let title_field = schema_fields.remove(position);
                schema_fields.insert(0, title_field);
            }
            for field in schema_fields {
                build_field(field, "", document_fields, &mut controls);
            }
            let mut unknown: Vec<&FrontmatterField> = document_fields
                .iter()
                .filter(|info| {
                    !info.path.contains('.')
                        && !schema.fields.iter().any(|field| field.name == info.path)
                })
                .collect();
            unknown.sort_by(|left, right| left.path.cmp(&right.path));
            for info in unknown {
                build_unknown(info, document_fields, &mut controls);
            }
        }
        None => {
            let mut present: Vec<&FrontmatterField> = document_fields
                .iter()
                .filter(|info| !info.path.contains('.'))
                .collect();
            present.sort_by(|left, right| left.path.cmp(&right.path));
            for info in present {
                build_unknown(info, document_fields, &mut controls);
            }
        }
    }
    for (order, control) in controls.iter_mut().enumerate() {
        control.order = order;
        control.role = role_for(&control.path, mappings);
    }
    // The mapped title is first even when the schema does not declare it.
    if let Some(position) = controls.iter().position(|control| control.path == title)
        && position > 0
    {
        let title_control = controls.remove(position);
        controls.insert(0, title_control);
    }
    for (order, control) in controls.iter_mut().enumerate() {
        control.order = order;
    }
    controls
}

fn role_for(path: &str, mappings: &FieldMappings) -> Option<FieldRole> {
    let title = mappings.title.as_deref().unwrap_or(DEFAULT_TITLE_FIELD);
    let description = mappings
        .description
        .as_deref()
        .unwrap_or(DEFAULT_DESCRIPTION_FIELD);
    let draft = mappings.draft.as_deref().unwrap_or(DEFAULT_DRAFT_FIELD);
    let date = mappings.date.as_deref().unwrap_or(DEFAULT_DATE_FIELD);
    if path == title {
        Some(FieldRole::Title)
    } else if path == description {
        Some(FieldRole::Description)
    } else if path == draft {
        Some(FieldRole::Draft)
    } else if path == date {
        Some(FieldRole::Date)
    } else {
        None
    }
}

/// Default description field used when no mapping overrides it.
const DEFAULT_DESCRIPTION_FIELD: &str = "description";

fn build_field(
    field: &Field,
    prefix: &str,
    document_fields: &[FrontmatterField],
    controls: &mut Vec<FieldControl>,
) {
    let path = if prefix.is_empty() {
        field.name.clone()
    } else {
        format!("{prefix}.{}", field.name)
    };
    let info = document_fields.iter().find(|info| info.path == path);
    let mapped = map_kind(&field.kind, info);
    controls.push(FieldControl {
        name: field.name.clone(),
        path: path.clone(),
        required: field.required,
        kind: mapped.kind,
        editable: mapped.editable,
        present: info.is_some(),
        removable: info.is_some() && !field.required,
        refusal: mapped.refusal,
        nullable: mapped.nullable,
        description: field.description.clone(),
        constraints: field.constraints.clone(),
        default: field.default.clone(),
        role: None,
        order: 0,
    });
    let children = object_children(&field.kind);
    let present_mapping = info.is_some_and(|info| info.shape == FieldShape::Mapping);
    if let (Some(children), true) = (children, present_mapping) {
        for child in children {
            build_field(child, &path, document_fields, controls);
        }
        let prefix = format!("{path}.");
        let mut unknown: Vec<&FrontmatterField> = document_fields
            .iter()
            .filter(|info| {
                info.path.starts_with(&prefix)
                    && info.path[prefix.len()..].find('.').is_none()
                    && !children
                        .iter()
                        .any(|child| info.path == format!("{path}.{}", child.name))
            })
            .collect();
        unknown.sort_by(|left, right| left.path.cmp(&right.path));
        for info in unknown {
            build_unknown(info, document_fields, controls);
        }
    }
}

fn build_unknown(
    info: &FrontmatterField,
    document_fields: &[FrontmatterField],
    controls: &mut Vec<FieldControl>,
) {
    let mapped = map_unknown(info);
    let path = info.path.clone();
    let name = path.rsplit('.').next().unwrap_or(&path).to_owned();
    controls.push(FieldControl {
        name,
        path: path.clone(),
        required: false,
        kind: mapped.kind,
        editable: mapped.editable,
        present: true,
        removable: true,
        refusal: mapped.refusal,
        nullable: false,
        description: None,
        constraints: FieldConstraints::default(),
        default: None,
        role: None,
        order: 0,
    });
    if info.shape == FieldShape::Mapping {
        let prefix = format!("{path}.");
        let mut children: Vec<&FrontmatterField> = document_fields
            .iter()
            .filter(|child| {
                child.path.starts_with(&prefix) && child.path[prefix.len()..].find('.').is_none()
            })
            .collect();
        children.sort_by(|left, right| left.path.cmp(&right.path));
        for child in children {
            build_unknown(child, document_fields, controls);
        }
    }
}

struct Mapped {
    kind: ControlKind,
    editable: bool,
    refusal: Option<String>,
    nullable: bool,
}

impl Mapped {
    fn editable(kind: ControlKind) -> Self {
        Self {
            kind,
            editable: true,
            refusal: None,
            nullable: false,
        }
    }

    fn read_only(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            kind: ControlKind::ReadOnly {
                reason: reason.clone(),
            },
            editable: false,
            refusal: Some(reason),
            nullable: false,
        }
    }

    fn raw(reason: impl Into<String>) -> Self {
        Self {
            kind: ControlKind::RawYaml {
                reason: reason.into(),
            },
            editable: true,
            refusal: None,
            nullable: false,
        }
    }
}

fn map_kind(kind: &FieldKind, info: Option<&FrontmatterField>) -> Mapped {
    // A present value whose YAML shape cannot be represented by the schema
    // control is offered as raw YAML or refused; it is never coerced.
    if let Some(info) = info {
        match info.shape {
            FieldShape::BlockScalar => {
                return Mapped::read_only("block scalar values are preserved but not editable");
            }
            FieldShape::Mapping if !is_object_kind(kind) => {
                return Mapped::raw("mapping value in a non-object field");
            }
            FieldShape::Sequence => {
                if info.items.is_none() {
                    return match info.raw.as_ref() {
                        Some(_) => Mapped::raw("sequence of non-scalar items edited as raw YAML"),
                        None => Mapped::read_only(
                            "sequence items are not all scalars; preserved verbatim",
                        ),
                    };
                }
            }
            FieldShape::Flow if !is_object_kind(kind) => {
                return Mapped::raw("flow value edited as raw YAML");
            }
            _ => {}
        }
    }
    match kind {
        FieldKind::String { format } => Mapped::editable(ControlKind::Text {
            format: format.clone(),
        }),
        FieldKind::Number => Mapped::editable(ControlKind::Number),
        FieldKind::Integer => Mapped::editable(ControlKind::Integer),
        FieldKind::Boolean => Mapped::editable(ControlKind::Boolean),
        FieldKind::Date => Mapped::editable(ControlKind::Date),
        FieldKind::Image => Mapped::editable(ControlKind::Image),
        FieldKind::Reference { collections } => Mapped::editable(ControlKind::Reference {
            collections: collections.clone(),
            multiple: false,
        }),
        FieldKind::Enum { values } => Mapped::editable(ControlKind::Enum {
            values: values.clone(),
        }),
        FieldKind::Literal { value } => Mapped {
            kind: ControlKind::Literal {
                value: value.clone(),
            },
            editable: false,
            refusal: Some("literal value is fixed by the schema".to_owned()),
            nullable: false,
        },
        FieldKind::Object { .. } => Mapped {
            kind: ControlKind::Object,
            editable: false,
            refusal: None,
            nullable: false,
        },
        FieldKind::Array { items } => match items.as_ref() {
            FieldKind::String { .. } | FieldKind::Enum { .. } => {
                let all_strings = info.is_none_or(|info| {
                    info.items
                        .as_ref()
                        .is_none_or(|items| items.iter().all(is_string_value))
                });
                if all_strings {
                    Mapped::editable(ControlKind::Tags)
                } else {
                    Mapped::raw("array contains non-string values")
                }
            }
            FieldKind::Reference { collections } => {
                let all_strings = info.is_none_or(|info| {
                    info.items
                        .as_ref()
                        .is_none_or(|items| items.iter().all(is_string_value))
                });
                if all_strings {
                    Mapped::editable(ControlKind::Reference {
                        collections: collections.clone(),
                        multiple: true,
                    })
                } else {
                    Mapped::raw("array contains non-string references")
                }
            }
            FieldKind::Image => {
                let all_strings = info.is_none_or(|info| {
                    info.items
                        .as_ref()
                        .is_none_or(|items| items.iter().all(is_string_value))
                });
                if all_strings {
                    Mapped::editable(ControlKind::Tags)
                } else {
                    Mapped::raw("array contains non-string image paths")
                }
            }
            other => match info {
                Some(info) if info.raw.is_some() => Mapped::raw(raw_reason(other)),
                _ => Mapped::read_only("array items are not editable in the form"),
            },
        },
        FieldKind::Record { .. } => match info {
            Some(info) if info.raw.is_some() => Mapped::raw("record edited as raw YAML"),
            _ => Mapped::read_only("record values are preserved verbatim"),
        },
        FieldKind::Nullable { inner } => {
            let mut mapped = map_kind(inner, info);
            mapped.nullable = true;
            mapped
        }
        FieldKind::Unsupported { reason } => Mapped::read_only(reason.clone()),
    }
}

fn raw_reason(kind: &FieldKind) -> String {
    let name = match kind {
        FieldKind::Object { .. } => "object",
        FieldKind::Array { .. } => "array",
        FieldKind::Record { .. } => "record",
        FieldKind::Reference { .. } => "reference",
        FieldKind::Image => "image",
        FieldKind::Literal { .. } => "literal",
        _ => "value",
    };
    format!("{name} items are edited as raw YAML")
}

fn map_unknown(info: &FrontmatterField) -> Mapped {
    match info.shape {
        FieldShape::Scalar => match info.value {
            Some(FrontmatterValue::Boolean(_)) => Mapped::editable(ControlKind::Boolean),
            Some(FrontmatterValue::Integer(_)) => Mapped::editable(ControlKind::Integer),
            Some(FrontmatterValue::Float(_)) => Mapped::editable(ControlKind::Number),
            _ => Mapped::editable(ControlKind::Text { format: None }),
        },
        FieldShape::Null => Mapped::editable(ControlKind::Text { format: None }),
        FieldShape::Mapping => Mapped {
            kind: ControlKind::Object,
            editable: false,
            refusal: None,
            nullable: false,
        },
        FieldShape::Sequence => match &info.items {
            Some(items) if items.iter().all(is_string_value) => Mapped::editable(ControlKind::Tags),
            Some(_) => Mapped::raw("sequence contains non-string values"),
            None => match info.raw.as_ref() {
                Some(_) => Mapped::raw("sequence of non-scalar items edited as raw YAML"),
                None => Mapped::read_only("sequence items are not all scalars; preserved verbatim"),
            },
        },
        FieldShape::Flow => Mapped::raw("flow value edited as raw YAML"),
        FieldShape::BlockScalar => {
            Mapped::read_only("block scalar values are preserved but not editable")
        }
    }
}

fn is_string_value(value: &FrontmatterValue) -> bool {
    matches!(value, FrontmatterValue::String(_))
}

fn object_children(kind: &FieldKind) -> Option<&[Field]> {
    match kind {
        FieldKind::Object { fields } => Some(fields),
        FieldKind::Nullable { inner } => object_children(inner),
        _ => None,
    }
}

/// Whether the schema type describes an object, unwrapping `Nullable`.
fn is_object_kind(kind: &FieldKind) -> bool {
    object_children(kind).is_some()
}

fn display_json(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

/// Why a field edit was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldEditError {
    /// The field is read-only (literal, container, unsupported, block scalar).
    NotEditable,
    /// The raw input could not be parsed into the field's value type.
    InvalidValue(String),
    /// The field is not present and could not be inserted.
    Document(DocumentError),
}

impl std::fmt::Display for FieldEditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEditable => formatter.write_str("this field is read-only"),
            Self::InvalidValue(message) => formatter.write_str(message),
            Self::Document(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for FieldEditError {}

/// Parse a raw string into the typed [`FrontmatterValue`] for scalar `kind`.
pub fn parse_value(kind: &ControlKind, raw: &str) -> Result<FrontmatterValue, FieldEditError> {
    Ok(match kind {
        ControlKind::Text { .. }
        | ControlKind::Date
        | ControlKind::Image
        | ControlKind::Reference { .. } => FrontmatterValue::String(raw.to_owned()),
        ControlKind::Number => {
            FrontmatterValue::Float(raw.trim().parse::<f64>().map_err(|_| {
                FieldEditError::InvalidValue(format!("`{raw}` is not a valid number"))
            })?)
        }
        ControlKind::Integer => {
            FrontmatterValue::Integer(raw.trim().parse::<i64>().map_err(|_| {
                FieldEditError::InvalidValue(format!("`{raw}` is not a valid integer"))
            })?)
        }
        ControlKind::Boolean => match raw.trim() {
            "true" | "1" => FrontmatterValue::Boolean(true),
            "false" | "0" => FrontmatterValue::Boolean(false),
            other => {
                return Err(FieldEditError::InvalidValue(format!(
                    "`{other}` is not a valid boolean"
                )));
            }
        },
        ControlKind::Enum { values } => enum_value(values, raw)?,
        ControlKind::Tags => FrontmatterValue::String(raw.to_owned()),
        ControlKind::Literal { .. }
        | ControlKind::Object
        | ControlKind::RawYaml { .. }
        | ControlKind::ReadOnly { .. } => return Err(FieldEditError::NotEditable),
    })
}

/// Apply a raw scalar edit, inserting the field when it is absent.
pub fn apply_scalar_edit(
    document: &mut ParsedDocument,
    path: &str,
    kind: &ControlKind,
    raw: &str,
    present: bool,
) -> Result<(), FieldEditError> {
    let value = parse_value(kind, raw)?;
    if present {
        document
            .set_field(path, value)
            .map_err(FieldEditError::Document)
    } else {
        document
            .insert_field(path, value)
            .map_err(FieldEditError::Document)
    }
}

/// Replace a raw YAML container after validating it.
///
/// Single-line flow values replace their inline span; block containers replace
/// the whole value block with re-indented, validated text.
pub fn apply_raw_edit(
    document: &mut ParsedDocument,
    path: &str,
    raw: &str,
) -> Result<(), FieldEditError> {
    astrolith_document::yaml::validate_value(raw).map_err(FieldEditError::InvalidValue)?;
    document
        .replace_raw(path, raw)
        .map_err(FieldEditError::Document)
}

fn enum_value(values: &[Value], raw: &str) -> Result<FrontmatterValue, FieldEditError> {
    for value in values {
        if display_json(value) == raw {
            return json_to_frontmatter(value).ok_or_else(|| {
                FieldEditError::InvalidValue(format!("`{raw}` cannot be represented as a scalar"))
            });
        }
    }
    Err(FieldEditError::InvalidValue(format!(
        "`{raw}` is not one of the allowed values"
    )))
}

/// Convert a JSON scalar into a [`FrontmatterValue`], rejecting non-scalars.
pub fn json_to_frontmatter(value: &Value) -> Option<FrontmatterValue> {
    match value {
        Value::Null => Some(FrontmatterValue::Null),
        Value::Bool(flag) => Some(FrontmatterValue::Boolean(*flag)),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Some(FrontmatterValue::Integer(integer))
            } else {
                number.as_f64().map(FrontmatterValue::Float)
            }
        }
        Value::String(text) => Some(FrontmatterValue::String(text.clone())),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// Convert a [`FrontmatterValue`] into a JSON scalar.
pub fn frontmatter_to_json(value: &FrontmatterValue) -> Value {
    match value {
        FrontmatterValue::Null => Value::Null,
        FrontmatterValue::Boolean(flag) => Value::Bool(*flag),
        FrontmatterValue::Integer(integer) => Value::from(*integer),
        FrontmatterValue::Float(number) => Value::from(*number),
        FrontmatterValue::String(text) => Value::String(text.clone()),
    }
}

/// Slugify a title for the copy-as-slug action.
///
/// Lowercases every character, keeps Unicode alphanumerics, and collapses every
/// run of other characters into a single hyphen; leading/trailing hyphens are
/// removed. It never touches the document.
pub fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut pending_separator = false;
    for character in title.chars() {
        if character.is_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            pending_separator = false;
            for lowered in character.to_lowercase() {
                slug.push(lowered);
            }
        } else {
            pending_separator = true;
        }
    }
    slug
}

/// One edit the form emits for the live session document.
///
/// The shell applies it to the *live* text through the lossless document API,
/// so unsaved body edits are never overwritten by a stale snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum FrontmatterEdit {
    /// Set or insert a scalar value.
    Scalar {
        /// Dotted field path.
        path: String,
        /// New typed value.
        value: FrontmatterValue,
        /// Whether the field already exists.
        present: bool,
    },
    /// Insert one string item at `index` of a sequence (creating the field when absent).
    AddItem {
        /// Dotted field path.
        path: String,
        /// Insertion index.
        index: usize,
        /// Item value.
        value: FrontmatterValue,
    },
    /// Remove one sequence item.
    RemoveItem {
        /// Dotted field path.
        path: String,
        /// Item index.
        index: usize,
    },
    /// Remove an optional field.
    Remove {
        /// Dotted field path.
        path: String,
    },
    /// Replace a validated raw YAML value.
    Raw {
        /// Dotted field path.
        path: String,
        /// Raw YAML text.
        raw: String,
    },
}

#[cfg(test)]
#[path = "field_tests.rs"]
mod tests;
