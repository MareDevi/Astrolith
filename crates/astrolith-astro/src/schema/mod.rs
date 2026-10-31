//! Normalization of Astro-generated collection JSON Schema.

pub mod generation;
mod helpers;
mod model;
mod parser;
mod validation;
mod validation_tooling;

pub use helpers::enhance_astro_fields;
pub use model::{CollectionSchema, Field, FieldKind, SchemaDiagnostic, SchemaDialect};
pub use parser::{SchemaError, normalize_schema};
pub use validation::{
    ValidationDiagnostic, ValidationSeverity, ValidationSource, validate_frontmatter,
};
pub use validation_tooling::{
    AstroValidationReport, AstroValidationRequest, AstroValidationStatus, ValidationError,
    accept_for_revision, validate_with_astro_tooling, validate_with_astro_tooling_async,
};

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod helpers_tests;

#[cfg(test)]
mod test_support;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "validation_tests.rs"]
mod validation_tests;
