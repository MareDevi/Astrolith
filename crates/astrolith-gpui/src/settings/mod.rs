//! Versioned, scoped preference storage. Call disk operations on a background executor.
//!
//! `model` exposes the domain types, `resolve` the single inheritance pass,
//! `store` the atomic global/registry/project files and `upgrade` the only
//! (versioned, one-shot) schema upgrade.

mod model;
mod resolve;
mod store;
mod upgrade;

pub(crate) use model::{GlobalPreferencesV2, ProjectPreferencesV1, ProjectRecord, WindowGeometry};
pub(crate) use resolve::resolve;
pub(crate) use store::{PreferencesStore, recovery_dir, save_failure_dir, settings_path};
