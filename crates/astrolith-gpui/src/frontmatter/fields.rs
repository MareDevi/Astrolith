//! Mapping from normalized collection-schema field kinds to native form controls,
//! plus the value-editing bridge onto the lossless document patch contract.
//!
//! This module is deliberately free of GPUI types so the mapping and edit rules
//! can be unit-tested headlessly. The GPUI rendering in [`super`] consumes
//! [`FieldControl`] values produced here.
//!
//! Only scalar leaves can be patched in place by
//! [`astrolith_document::ParsedDocument::set_field`]. Containers that the lossless
//! model preserves verbatim (arrays, records, flow-collection objects) and
//! schema constructs that cannot be edited safely (heterogeneous unions,
//! literals) are surfaced read-only with an explicit refusal reason rather than
//! being silently coerced to text.

use astrolith_astro::schema::{Field, FieldKind};
use astrolith_document::{DocumentError, FrontmatterValue, ParsedDocument};
use serde_json::Value;

/// The native control family a field maps to.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlKind {
    /// A plain text input, optionally with a semantic format (e.g. `email`).
    Text {
        /// Schema format hint such as `email`.
        format: Option<String>,
    },
    /// A floating-point number input.
    Number,
    /// An integer input.
    Integer,
    /// A boolean toggle.
    Boolean,
    /// A date input (stored as a YAML scalar string).
    Date,
    /// An Astro `image()` asset path input (a string in frontmatter).
    Image,
    /// An Astro `reference()` target input.
    Reference {
        /// Collections the reference may point at.
        collections: Vec<String>,
    },
    /// A single-choice control from a fixed value set.
    Enum {
        /// Allowed values.
        values: Vec<Value>,
    },
    /// A fixed literal value (read-only).
    Literal {
        /// The literal value from the schema.
        value: Value,
    },
    /// A nested object container; its children carry their own controls.
    Object,
    /// An array; preserved verbatim and not editable through the form.
    Array,
    /// A free-form map; preserved verbatim and not editable through the form.
    Record,
    /// A construct that cannot be edited safely (e.g. a heterogeneous union).
    Unsupported {
        /// Why the field cannot be safely edited.
        reason: String,
    },
}

/// One field rendered by the form, with its resolved control and editability.
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
    /// Explicit refusal reason when the field is shown read-only.
    pub refusal: Option<String>,
    /// Whether the field also accepts `null`.
    pub nullable: bool,
}

impl FieldControl {
    /// Short label for the control family, used in the mapping report and tests.
    pub fn control_label(&self) -> &'static str {
        control_label(&self.kind)
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
        ControlKind::Reference { .. } => "reference",
        ControlKind::Enum { .. } => "enum",
        ControlKind::Literal { .. } => "literal",
        ControlKind::Object => "object",
        ControlKind::Array => "array",
        ControlKind::Record => "record",
        ControlKind::Unsupported { .. } => "unsupported",
    }
}

/// Build the ordered control list for a set of schema fields.
///
/// Object fields emit a container control followed by their children (flattened
/// with dotted paths); nullable objects recurse the same way.
pub fn build_controls(fields: &[Field]) -> Vec<FieldControl> {
    let mut controls = Vec::new();
    build_into(fields, "", &mut controls);
    controls
}

fn build_into(fields: &[Field], prefix: &str, controls: &mut Vec<FieldControl>) {
    for field in fields {
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        let mapped = map_kind(&field.kind);
        controls.push(FieldControl {
            name: field.name.clone(),
            path: path.clone(),
            required: field.required,
            kind: mapped.kind.clone(),
            editable: mapped.editable,
            refusal: mapped.refusal.clone(),
            nullable: mapped.nullable,
        });
        if let Some(children) = object_children(&field.kind) {
            build_into(children, &path, controls);
        }
    }
}

struct Mapped {
    kind: ControlKind,
    editable: bool,
    refusal: Option<String>,
    nullable: bool,
}

fn map_kind(kind: &FieldKind) -> Mapped {
    match kind {
        FieldKind::String { format } => Mapped {
            kind: ControlKind::Text {
                format: format.clone(),
            },
            editable: true,
            refusal: None,
            nullable: false,
        },
        FieldKind::Number => Mapped::editable(ControlKind::Number),
        FieldKind::Integer => Mapped::editable(ControlKind::Integer),
        FieldKind::Boolean => Mapped::editable(ControlKind::Boolean),
        FieldKind::Date => Mapped::editable(ControlKind::Date),
        FieldKind::Image => Mapped::editable(ControlKind::Image),
        FieldKind::Reference { collections } => Mapped::editable(ControlKind::Reference {
            collections: collections.clone(),
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
        FieldKind::Array { .. } => Mapped::read_only(
            ControlKind::Array,
            "array values are preserved verbatim and cannot be edited in the form yet",
        ),
        FieldKind::Record { .. } => Mapped::read_only(
            ControlKind::Record,
            "record values are preserved verbatim and cannot be edited in the form yet",
        ),
        FieldKind::Nullable { inner } => {
            let mut mapped = map_kind(inner);
            mapped.nullable = true;
            mapped
        }
        FieldKind::Unsupported { reason } => Mapped {
            kind: ControlKind::Unsupported {
                reason: reason.clone(),
            },
            editable: false,
            refusal: Some(reason.clone()),
            nullable: false,
        },
    }
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

    fn read_only(kind: ControlKind, reason: &str) -> Self {
        Self {
            kind,
            editable: false,
            refusal: Some(reason.to_owned()),
            nullable: false,
        }
    }
}

/// Return object children for object kinds, including nullable objects.
fn object_children(kind: &FieldKind) -> Option<&[Field]> {
    match kind {
        FieldKind::Object { fields } => Some(fields),
        FieldKind::Nullable { inner } => object_children(inner),
        _ => None,
    }
}

/// Why a field edit was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldEditError {
    /// The field is read-only (array, record, literal, unsupported, container).
    NotEditable,
    /// The raw input could not be parsed into the field's value type.
    InvalidValue(String),
    /// The lossless document patch contract refused the edit.
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

/// Apply a raw string edit to `document` for the scalar control `kind`.
///
/// Parses the input into the appropriate [`FrontmatterValue`] and records a
/// lossless in-place patch through [`ParsedDocument::set_field`]. Read-only
/// control families are refused with [`FieldEditError::NotEditable`].
pub fn apply_edit(
    document: &mut ParsedDocument,
    path: &str,
    kind: &ControlKind,
    raw: &str,
) -> Result<(), FieldEditError> {
    let value = match kind {
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
        ControlKind::Literal { .. }
        | ControlKind::Object
        | ControlKind::Array
        | ControlKind::Record
        | ControlKind::Unsupported { .. } => return Err(FieldEditError::NotEditable),
    };
    document
        .set_field(path, value)
        .map_err(FieldEditError::Document)
}

fn enum_value(values: &[Value], raw: &str) -> Result<FrontmatterValue, FieldEditError> {
    for value in values {
        if json_display(value) == raw {
            return json_to_frontmatter(value).ok_or_else(|| {
                FieldEditError::InvalidValue(format!("`{raw}` cannot be represented as a scalar"))
            });
        }
    }
    Err(FieldEditError::InvalidValue(format!(
        "`{raw}` is not one of the allowed values"
    )))
}

fn json_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
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
