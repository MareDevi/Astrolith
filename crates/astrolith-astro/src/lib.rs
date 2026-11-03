//! Conservative local Astro project inspection; never executes project code.
pub mod components;
pub mod config;
pub mod discovery;
pub mod pattern;
pub mod schema;
pub use discovery::*;
pub use pattern::{CollectionPattern, EntryKind};
