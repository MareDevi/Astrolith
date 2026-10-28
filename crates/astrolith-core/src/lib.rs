//! Pure domain identities and source-file mappings for Astrolith.
//!
//! IDs are project-local, case-sensitive opaque values. They never identify a
//! source path without an explicit [`Entry`] mapping supplied by discovery.
//! Nonblank IDs retain surrounding whitespace; trimming a loader's ID would
//! change its identity. Callers should display ambiguous IDs with suitable quoting.

#![deny(missing_docs)]

/// Collection identity within a project.
pub mod collection;
/// Logical entry identity and explicit source-file mapping.
pub mod entry;
/// Opaque workspace project identity.
pub mod project;

pub use collection::CollectionId;
pub use entry::{Entry, EntryId, SourcePath};
pub use project::ProjectId;

/// Invalid domain input; no filesystem access is performed during validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// An identity was blank or contained control characters.
    InvalidIdentity,
    /// A source path was not an unambiguous project-relative path.
    InvalidSourcePath,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidIdentity => "identity must be nonblank and contain no control characters",
            Self::InvalidSourcePath => "source path must be an unambiguous project-relative path",
        })
    }
}

impl std::error::Error for ModelError {}

fn validate_identity(value: &str) -> Result<(), ModelError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(ModelError::InvalidIdentity)
    } else {
        Ok(())
    }
}
