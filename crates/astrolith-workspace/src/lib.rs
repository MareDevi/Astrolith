//! Project workspace indexing and filesystem observation.

#![deny(missing_docs)]

/// Collection entry scanning and source coalescing.
pub mod index;
/// Bounded filesystem watcher batches.
pub mod watcher;

mod slug_ranges;

pub use index::{
    CollectionEntry, SourceGroup, WorkspaceError, coalesce_by_source, scan_collections,
};
pub use watcher::{CollectionWatcher, WatchBatch};
