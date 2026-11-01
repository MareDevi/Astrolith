//! One-time migration from the version-1 single preferences file.
//!
//! The legacy file is read once, mapped to the global scope, then retained as
//! `preferences.v1.migrated.json`. Later launches never read it again.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::{
    model::{GLOBAL_PREFERENCES_VERSION, GlobalPreferencesV2},
    store::{unique_destination, write_atomic},
};
use astrolith_core::preferences::{PanelLayout, ThemePreference};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPreferencesV1 {
    version: u32,
    #[serde(default)]
    theme: ThemePreference,
    #[serde(default)]
    layout: PanelLayout,
}

/// Legacy location derived from the global file: `<config>/astrolith/preferences.json`.
///
/// An `ASTROLITH_SETTINGS_PATH` override isolates a run from the user profile
/// and therefore never reads a neighbouring legacy file.
pub(super) fn legacy_settings_path(global_path: &Path) -> Option<PathBuf> {
    if std::env::var_os("ASTROLITH_SETTINGS_PATH").is_some() {
        return None;
    }
    global_path
        .parent()
        .and_then(Path::parent)
        .map(|directory| directory.join("preferences.json"))
}

/// Migrate the legacy file once. Returns a user-facing warning on any failure.
pub(super) fn migrate_legacy(global_path: &Path, legacy_path: Option<&Path>) -> Option<String> {
    let legacy = legacy_path?;
    if !legacy.exists() {
        return None;
    }
    let migrated = unique_destination(&legacy.with_file_name("preferences.v1.migrated.json"));
    if global_path.exists() {
        return match fs::rename(legacy, &migrated) {
            Ok(()) => None,
            Err(error) => Some(format!(
                "Could not retire the legacy preferences at {}: {error}.",
                legacy.display()
            )),
        };
    }
    let bytes = match fs::read(legacy) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Some(format!(
                "Could not read legacy preferences at {}: {error}.",
                legacy.display()
            ));
        }
    };
    let legacy_preferences = match serde_json::from_slice::<LegacyPreferencesV1>(&bytes) {
        Ok(preferences) if preferences.version == 1 => preferences,
        Ok(preferences) => {
            return Some(format!(
                "Unsupported legacy preference version {} at {}; the file is preserved.",
                preferences.version,
                legacy.display()
            ));
        }
        Err(error) => {
            return Some(quarantine_legacy(legacy, &error));
        }
    };
    let global = GlobalPreferencesV2 {
        version: GLOBAL_PREFERENCES_VERSION,
        theme: legacy_preferences.theme,
        layout: legacy_preferences.layout,
        ..GlobalPreferencesV2::default()
    };
    let encoded = match serde_json::to_vec_pretty(&global) {
        Ok(encoded) => encoded,
        Err(error) => {
            return Some(format!("Could not encode migrated preferences: {error}."));
        }
    };
    if let Err(error) = write_atomic(global_path, &encoded) {
        return Some(format!("Could not write migrated preferences: {error}."));
    }
    match fs::rename(legacy, &migrated) {
        Ok(()) => {
            tracing::info!(
                legacy = %legacy.display(),
                migrated = %migrated.display(),
                "migrated legacy preferences"
            );
            None
        }
        Err(error) => Some(format!(
            "Preferences were migrated, but the legacy file at {} could not be retained as {}: {error}.",
            legacy.display(),
            migrated.display()
        )),
    }
}

fn quarantine_legacy(path: &Path, error: &dyn std::fmt::Display) -> String {
    let destination = unique_destination(&path.with_file_name("preferences.v1.corrupt.json"));
    match fs::rename(path, &destination) {
        Ok(()) => format!(
            "Could not migrate legacy preferences: {error}. The invalid file was moved to {}; defaults are active.",
            destination.display()
        ),
        Err(rename_error) => format!(
            "Could not migrate legacy preferences: {error}. The file is preserved ({rename_error})."
        ),
    }
}
