//! The only preference upgrade: one numeric version step, one backup, one pass.
//!
//! A file is upgraded only when its envelope carries a numeric `version` lower
//! than the current schema version. The complete previous file is copied to
//! `<name>.v<old>.bak` before the first mutation, the retired keys are removed,
//! old defaults are re-baselined, and the new version is written atomically.
//! After that the stored version equals [`GLOBAL_PREFERENCES_VERSION`] (or
//! [`PROJECT_PREFERENCES_VERSION`]), so this module never touches the file
//! again and never produces another backup.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use astrolith_core::preferences::{
    GLOBAL_PREFERENCES_VERSION, GlobalPreferencesV2, PROJECT_PREFERENCES_VERSION,
    ProjectPreferencesV1, ThemePreference,
};

use super::store::{Stored, unique_destination, write_atomic};

/// Keys that no longer belong to the global schema.
const RETIRED_GLOBAL_KEYS: [&str; 5] = [
    "light_theme",
    "dark_theme",
    "editor_width",
    "interface_scale",
    "layout",
];

/// Legacy `preferences.json` shape; `layout` is intentionally not modelled.
#[derive(serde::Deserialize)]
struct LegacyPreferencesV1 {
    version: u32,
    #[serde(default)]
    theme: ThemePreference,
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

/// Retire the version-1 single file: import its theme once or ignore it once.
pub(super) fn import_legacy(global_path: &Path, legacy_path: Option<&Path>) -> Option<String> {
    let legacy = legacy_path?;
    if !legacy.exists() {
        return None;
    }
    if global_path.exists() {
        // Downgrade then upgrade again: `global.json` is authoritative and the
        // re-created legacy file is retained under a name no launch reads.
        return match fs::rename(
            legacy,
            unique_destination(&legacy.with_file_name("preferences.v1.ignored.json")),
        ) {
            Ok(()) => Some(format!(
                "Legacy preferences at {} were not imported because global preferences already exist; the file was moved aside.",
                legacy.display()
            )),
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
    let preferences = match serde_json::from_slice::<LegacyPreferencesV1>(&bytes) {
        Ok(preferences) if preferences.version == 1 => preferences,
        Ok(preferences) => {
            return retire_once(
                legacy,
                "preferences.v1.ignored.json",
                format!(
                    "Unsupported legacy preference version {} at {}; the file was moved aside.",
                    preferences.version,
                    legacy.display()
                ),
            );
        }
        Err(error) => {
            return Some(quarantine_legacy(legacy, &error));
        }
    };
    let global = GlobalPreferencesV2 {
        theme: preferences.theme,
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
    let migrated = unique_destination(&legacy.with_file_name("preferences.v1.migrated.json"));
    match fs::rename(legacy, &migrated) {
        Ok(()) => {
            tracing::info!(
                legacy = %legacy.display(),
                migrated = %migrated.display(),
                "imported legacy preferences"
            );
            None
        }
        Err(error) => Some(format!(
            "Preferences were imported, but the legacy file at {} could not be retained as {}: {error}.",
            legacy.display(),
            migrated.display()
        )),
    }
}

/// Rename a legacy file out of the read path and warn exactly once.
fn retire_once(legacy: &Path, name: &str, warning: String) -> Option<String> {
    let destination = unique_destination(&legacy.with_file_name(name));
    match fs::rename(legacy, &destination) {
        Ok(()) => Some(warning),
        Err(error) => Some(format!("{warning} The file could not be moved ({error}).")),
    }
}

fn quarantine_legacy(path: &Path, error: &dyn std::fmt::Display) -> String {
    let destination = unique_destination(&path.with_file_name("preferences.v1.corrupt.json"));
    match fs::rename(path, &destination) {
        Ok(()) => format!(
            "Could not import legacy preferences: {error}. The invalid file was moved to {}; defaults are active.",
            destination.display()
        ),
        Err(rename_error) => format!(
            "Could not import legacy preferences: {error}. The file is preserved ({rename_error})."
        ),
    }
}

/// Apply the global version step, returning one user-facing warning on failure.
pub(super) fn upgrade_global(stored: &mut Stored<GlobalPreferencesV2>) -> Option<String> {
    let old_version = old_version(stored.envelope.as_ref()?, GLOBAL_PREFERENCES_VERSION)?;
    let mut upgraded = stored.envelope.as_ref()?.clone();
    let defaults = GlobalPreferencesV2::default();

    if number(&upgraded, "ui_font_size")
        .is_some_and(|size| (size - 14.).abs() < f64::EPSILON || (size - 16.).abs() < f64::EPSILON)
    {
        insert_number(&mut upgraded, "ui_font_size", defaults.ui_font_size);
    }
    if number(&upgraded, "editor_font_size").is_some_and(|size| (size - 18.).abs() < f64::EPSILON) {
        insert_number(&mut upgraded, "editor_font_size", defaults.editor_font_size);
    }
    if !upgraded.contains_key("page_width_percent") {
        let percent = number(&upgraded, "editor_width")
            .map(|width| ((width / 1200. * 100.).clamp(30., 100.) * 10.).round() / 10.)
            .unwrap_or(f64::from(defaults.page_width_percent));
        insert_number(&mut upgraded, "page_width_percent", percent as f32);
    }
    if !upgraded.contains_key("editor_line_height") {
        insert_number(
            &mut upgraded,
            "editor_line_height",
            defaults.editor_line_height,
        );
    }
    if upgraded.remove("layout").is_some() {
        // The pre-mode composition has no converter; install the per-mode
        // defaults once, keeping any per-mode value already stored.
        if !upgraded.contains_key("default_mode") {
            match serde_json::to_value(defaults.default_mode) {
                Ok(value) => {
                    upgraded.insert("default_mode".to_owned(), value);
                }
                Err(error) => {
                    return Some(format!("Could not encode the default mode: {error}."));
                }
            }
        }
        if !upgraded.contains_key("layouts") {
            match serde_json::to_value(&defaults.layouts) {
                Ok(value) => {
                    upgraded.insert("layouts".to_owned(), value);
                }
                Err(error) => {
                    return Some(format!("Could not encode the mode layouts: {error}."));
                }
            }
        }
    }
    for key in RETIRED_GLOBAL_KEYS {
        upgraded.remove(key);
    }
    upgraded.insert(
        "version".to_owned(),
        Value::from(GLOBAL_PREFERENCES_VERSION),
    );

    match finish_upgrade(stored, upgraded, old_version, "global.json") {
        Ok(backup) => {
            tracing::info!(
                path = %stored.path.display(),
                backup = %backup.display(),
                from = old_version,
                to = GLOBAL_PREFERENCES_VERSION,
                "upgraded global preferences"
            );
            None
        }
        Err(warning) => warning,
    }
}

/// Apply the project version step (retired project layout, new version).
pub(super) fn upgrade_project(stored: &mut Stored<ProjectPreferencesV1>) -> Option<String> {
    let old_version = old_version(stored.envelope.as_ref()?, PROJECT_PREFERENCES_VERSION)?;
    let mut upgraded = stored.envelope.as_ref()?.clone();
    upgraded.remove("layout");
    upgraded.insert(
        "version".to_owned(),
        Value::from(PROJECT_PREFERENCES_VERSION),
    );
    match finish_upgrade(stored, upgraded, old_version, "project.json") {
        Ok(backup) => {
            tracing::info!(
                path = %stored.path.display(),
                backup = %backup.display(),
                from = old_version,
                to = PROJECT_PREFERENCES_VERSION,
                "upgraded project preferences"
            );
            None
        }
        Err(warning) => warning,
    }
}

/// Copy the exact previous file, write the upgraded envelope, update the store.
fn finish_upgrade<T>(
    stored: &mut Stored<T>,
    upgraded: Map<String, Value>,
    old_version: u64,
    label: &str,
) -> Result<PathBuf, Option<String>>
where
    T: DeserializeOwned + Serialize + Default + Clone,
{
    let baseline = match stored.baseline.as_deref() {
        Some(baseline) => baseline,
        None => {
            return Err(Some(format!(
                "Could not upgrade {} because the previous file content is unavailable.",
                stored.path.display()
            )));
        }
    };
    let file_name = stored
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| label.to_owned());
    let backup = unique_destination(
        &stored
            .path
            .with_file_name(format!("{file_name}.v{old_version}.bak")),
    );
    if let Err(error) = write_backup(&backup, baseline) {
        return Err(Some(format!(
            "Could not back up preferences to {}: {error}. The settings file is preserved.",
            backup.display()
        )));
    }
    let value = match serde_json::from_value::<T>(Value::Object(upgraded.clone())) {
        Ok(value) => value,
        Err(error) => {
            return Err(Some(format!(
                "Could not read the upgraded preferences in {}: {error}. The original was backed up to {}.",
                stored.path.display(),
                backup.display()
            )));
        }
    };
    let bytes = match serde_json::to_vec_pretty(&Value::Object(upgraded.clone())) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Err(Some(format!(
                "Could not encode the upgraded preferences: {error}."
            )));
        }
    };
    if let Err(error) = write_atomic(&stored.path, &bytes) {
        return Err(Some(format!(
            "Could not write the upgraded preferences in {}: {error}. The original was backed up to {}.",
            stored.path.display(),
            backup.display()
        )));
    }
    stored.baseline = Some(bytes);
    stored.envelope = Some(upgraded);
    stored.value = value;
    Ok(backup)
}

fn write_backup(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// The numeric envelope version, when it is a one-step downgrade we can upgrade.
fn old_version(envelope: &Map<String, Value>, current: u32) -> Option<u64> {
    let old = envelope.get("version")?.as_u64()?;
    (old < u64::from(current)).then_some(old)
}

fn number(envelope: &Map<String, Value>, key: &str) -> Option<f64> {
    envelope.get(key).and_then(Value::as_f64)
}

fn insert_number(envelope: &mut Map<String, Value>, key: &str, value: f32) {
    envelope.insert(key.to_owned(), serde_json::json!(value));
}
