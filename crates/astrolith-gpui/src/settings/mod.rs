//! Versioned, scoped preference storage. Call disk operations on a background executor.
//!
//! `model` exposes the domain types, `resolve` the single inheritance pass,
//! `store` the atomic global/registry/project files and `migrate` the one-time
//! version-1 import.

mod migrate;
mod model;
mod resolve;
mod store;

pub(crate) use model::{GlobalPreferencesV2, ProjectPreferencesV1};
pub(crate) use resolve::resolve;
pub(crate) use store::{PreferencesStore, recovery_dir, save_failure_dir, settings_path};
