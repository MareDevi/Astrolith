//! Scoped preference model re-exports used by the desktop application.
//!
//! The domain types and the single inheritance resolver live in
//! `astrolith-core::preferences`; this module keeps desktop persistence and
//! presentation code on one import surface.

pub(crate) use astrolith_core::preferences::{
    GLOBAL_PREFERENCES_VERSION, GlobalPreferencesV2, PROJECT_PREFERENCES_VERSION,
    PROJECT_REGISTRY_VERSION, ProjectOpen, ProjectPreferencesV1, ProjectRecord, ProjectRegistry,
    WindowGeometry,
};
