//! Editable-document state and persistence for Astrolith.
//!
//! This crate owns text, revision, and stale-result identity rules, a lossless
//! frontmatter/body document model, and a conflict-aware save coordinator. It
//! performs no user-interface work; saving runs through its own blocking-task
//! boundary so callers never block the UI thread.

#![deny(missing_docs)]

/// Retained text, revision, dirty state, and load-result identity.
pub mod buffer;
/// Lossless frontmatter parse and scalar field spans.
pub mod parse;
/// Recovery records for unsaved document content.
pub mod recovery;
/// Conflict-aware, one-writer-per-document save coordinator.
pub mod save;
/// Lossless document and scalar serialization.
pub mod serialize;
/// Disk baseline for conflict-aware saving.
pub mod version;

pub use buffer::{
    DocumentBuffer, DocumentError, DocumentIdentity, DocumentLoadTicket, DocumentRevision,
};
pub use parse::{FrontmatterValue, LineEnding, ParsedDocument, parse_document};
pub use recovery::{RecoveryLoad, RecoveryRecord, RecoveryStore};
pub use save::{DocumentSnapshot, SaveOutcome, save_snapshot};
pub use serialize::serialize_document;
pub use version::DiskBaseline;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
