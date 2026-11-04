//! Project workspace indexing, filesystem observation and entry operations.

#![deny(missing_docs)]

/// Asset copying, Markdown links and Content Finder targets.
pub mod assets;
/// Bounded project file tree for the Build navigator.
pub mod files;
/// Collection entry scanning and source coalescing.
pub mod index;
/// Deterministic file-list filtering and ordering.
pub mod list;
/// Typed, validated entry lifecycle operations.
pub mod operations;
/// Project-root policy for system and credential directories.
pub mod paths;
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
