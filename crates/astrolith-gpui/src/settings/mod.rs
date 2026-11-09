//! Versioned, scoped preference storage. Call disk operations on a background executor.
//!
//! `model` exposes the domain types, `resolve` the single inheritance pass,
//! `store` the atomic global/registry/project files.

mod model;
mod resolve;
mod store;
pub(crate) use model::{GlobalPreferences, ProjectPreferences, ProjectRecord, WindowGeometry};
pub(crate) use resolve::resolve;
pub(crate) use store::{PreferencesStore, recovery_dir, save_failure_dir, settings_path};
