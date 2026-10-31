//! Normalization of Astro-generated collection JSON Schema.

pub mod generation;
mod helpers;
mod model;
mod parser;

pub use helpers::enhance_astro_fields;
pub use model::{CollectionSchema, Field, FieldKind, SchemaDiagnostic, SchemaDialect};
pub use parser::{SchemaError, normalize_schema};

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod helpers_tests;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
