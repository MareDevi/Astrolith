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
