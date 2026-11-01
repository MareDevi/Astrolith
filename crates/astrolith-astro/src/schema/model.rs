//! GUI-independent, normalized representation of an Astro collection schema.

use serde_json::Value;

/// The JSON Schema dialect used by a generated Astro collection schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaDialect {
    /// Astro 5 emits draft-07 with a named definition and a top-level `$ref`.
    Draft7,
    /// Astro 7 emits draft 2020-12 with the object schema at the top level.
    Draft202012,
    /// A recognized object schema whose dialect metadata is absent.
    Unknown,
}

/// A normalized field within a collection schema.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Frontmatter key.
    pub name: String,
    /// Whether Astro requires the key.
    pub required: bool,
    /// Literal default emitted by the schema generator, if any.
    pub default: Option<Value>,
    /// Normalized field type.
    pub kind: FieldKind,
    /// Schema-authored description shown as control help.
    pub description: Option<String>,
    /// Range, length and pattern constraints that drive validation hints.
    pub constraints: FieldConstraints,
    /// Zero-based declaration order. Title-first rendering uses this to keep
    /// every other schema field in source declaration order.
    pub order: usize,
}

/// The JSON Schema constraints a field carries.
///
/// Numeric bounds apply to [`FieldKind::Number`] and [`FieldKind::Integer`];
/// length bounds apply to [`FieldKind::String`] text and sequence item counts;
/// the pattern applies to string values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldConstraints {
    /// Inclusive minimum (`minimum`).
    pub minimum: Option<f64>,
    /// Inclusive maximum (`maximum`).
    pub maximum: Option<f64>,
    /// Minimum string length or sequence item count (`minLength`/`minItems`).
    pub min_length: Option<usize>,
    /// Maximum string length or sequence item count (`maxLength`/`maxItems`).
    pub max_length: Option<usize>,
    /// Regular-expression pattern (`pattern`).
    pub pattern: Option<String>,
}

impl FieldConstraints {
    /// Whether any constraint is present and worth rendering.
    pub fn is_empty(&self) -> bool {
        self.minimum.is_none()
            && self.maximum.is_none()
            && self.min_length.is_none()
            && self.max_length.is_none()
            && self.pattern.is_none()
    }
}

/// A normalized field type.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    String {
        format: Option<String>,
    },
    Number,
    Integer,
    Boolean,
    /// A calendar date or timestamp, regardless of the emitted representation.
    Date,
    /// An Astro `image()` asset reference.
    ///
    /// Astro's generated JSON Schema renders `image()` as a plain string (it is
    /// a `z.string().transform(...)`), so this kind can only be recovered from
    /// the content config source by [`enhance_astro_fields`](super::enhance_astro_fields).
    Image,
    /// An Astro `reference()` to one or more other collections.
    ///
    /// Astro's generated JSON Schema renders `reference()` as a heterogeneous
    /// union, which alone normalizes to [`FieldKind::Unsupported`]; the helper
    /// resolution recovers the target collection names from the config.
    Reference {
        collections: Vec<String>,
    },
    Enum {
        values: Vec<Value>,
    },
    Literal {
        value: Value,
    },
    Array {
        items: Box<FieldKind>,
    },
    Object {
        fields: Vec<Field>,
    },
    /// A free-form string-keyed map, represented by its value schema.
    Record {
        value: Box<FieldKind>,
    },
    /// A single type that also accepts `null`.
    Nullable {
        inner: Box<FieldKind>,
    },
    /// A construct that cannot be edited safely, such as a heterogeneous union.
    Unsupported {
        reason: String,
    },
}

/// A whole normalized collection schema plus non-fatal diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionSchema {
    pub dialect: SchemaDialect,
    pub fields: Vec<Field>,
    pub diagnostics: Vec<SchemaDiagnostic>,
}

/// A non-fatal problem encountered while normalizing part of a schema.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaDiagnostic {
    /// Dot-separated path to the affected field (e.g. `author.email`).
    pub path: String,
    pub message: String,
}
