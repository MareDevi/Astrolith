//! Normalization of Astro-generated collection JSON Schema.

mod model;
mod parser;

pub use model::{CollectionSchema, Field, FieldKind, SchemaDiagnostic, SchemaDialect};
pub use parser::{SchemaError, normalize_schema};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
