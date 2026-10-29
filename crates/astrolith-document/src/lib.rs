//! Pure editable-document state for Astrolith.
//!
//! This crate owns text, revision, and stale-result identity rules. It performs
//! no filesystem or user-interface work.

#![deny(missing_docs)]

/// Retained text, revision, dirty state, and load-result identity.
pub mod buffer;

pub use buffer::{
    DocumentBuffer, DocumentError, DocumentIdentity, DocumentLoadTicket, DocumentRevision,
};
