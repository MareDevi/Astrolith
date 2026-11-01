//! Project workspace indexing, filesystem observation and entry operations.

#![deny(missing_docs)]

/// Collection entry scanning and source coalescing.
pub mod index;
/// Deterministic file-list filtering and ordering.
pub mod list;
/// Typed, validated entry lifecycle operations.
pub mod operations;
/// Reference-target options from file and JSON data collections.
pub mod references;
/// Bounded filesystem watcher batches.
pub mod watcher;

mod slug_ranges;

pub use index::{
    CollectionEntry, EntryMetadata, MetaValue, SourceGroup, WorkspaceError, coalesce_by_source,
    scan_collections,
};
pub use references::ReferenceOption;
pub use watcher::{CollectionWatcher, WatchBatch};
