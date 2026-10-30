//! Pure editable-document state for Astrolith.
//!
//! This crate owns text, revision, and stale-result identity rules, and a
//! lossless frontmatter/body document model. It performs no filesystem or
//! user-interface work.

#![deny(missing_docs)]

/// Retained text, revision, dirty state, and load-result identity.
pub mod buffer;
/// Lossless frontmatter parse and scalar field spans.
pub mod parse;
/// Lossless document and scalar serialization.
pub mod serialize;

pub use buffer::{
    DocumentBuffer, DocumentError, DocumentIdentity, DocumentLoadTicket, DocumentRevision,
};
pub use parse::{FrontmatterValue, LineEnding, ParsedDocument, parse_document};
pub use serialize::serialize_document;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
