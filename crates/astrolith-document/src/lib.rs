//! Editable-document state and persistence for Astrolith.
//!
//! This crate owns text, revision, and stale-result identity rules, a lossless
//! frontmatter/body document model, and a conflict-aware save coordinator. It
//! performs no user-interface work; saving runs through its own blocking-task
//! boundary so callers never block the UI thread.

#![deny(missing_docs)]

/// Retained text, revision, dirty state, and load-result identity.
pub mod buffer;
/// Pure writer formatting transforms.
pub mod formatting;
/// Lossless frontmatter parse and scalar field spans.
pub mod parse;
/// Lossless writer projection and visible-to-source mapping.
pub mod projection;
/// Recovery records for unsaved document content.
pub mod recovery;
/// Conflict-aware, one-writer-per-document save coordinator.
pub mod save;
/// Deterministic sentence detection for focus mode.
pub mod sentences;
/// Lossless document and scalar serialization.
pub mod serialize;
/// One token snapshot for counts and classification.
pub mod tokens;
/// Disk baseline for conflict-aware saving.
pub mod version;
/// Raw YAML value validation.
pub mod yaml;

pub use buffer::{
    DocumentBuffer, DocumentError, DocumentIdentity, DocumentLoadTicket, DocumentRevision,
};
pub use formatting::{
    FormatCommand, FormatError, FormatOutcome, FormattedDocument, HeadingLevel, InlineLink, format,
    format_document, inline_links,
};
pub use parse::{
    FieldShape, FrontmatterField, FrontmatterValue, LineEnding, ParsedDocument, SequenceStyle,
    parse_document,
};
pub use projection::{
    DocumentKind, ProjectionError, ProjectionSegment, TextEdit, TextReplacement, WriterProjection,
    apply_visible_edit, diff_visible, map_visible_offset, project_writer,
};
pub use recovery::{RecoveryLoad, RecoveryRecord, RecoveryStore};
pub use save::{DocumentSnapshot, SaveOutcome, save_snapshot};
pub use sentences::{cursor_sentence, other_sentences, sentences};
pub use serialize::serialize_document;
pub use tokens::{TokenSnapshot, TokenSummary, token_snapshot};
pub use version::DiskBaseline;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
