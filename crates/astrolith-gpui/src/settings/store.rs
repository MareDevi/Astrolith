//! Atomic, envelope-preserving stores for global, registry and project scopes.
//!
//! Every operation performs blocking disk I/O; callers run them on a background
//! executor. Unknown top-level JSON fields survive a load/save cycle. Malformed
//! data is quarantined beside the original instead of blocking persistence.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use super::{
    migrate,
    model::{
        GLOBAL_PREFERENCES_VERSION, GlobalPreferencesV2, PROJECT_PREFERENCES_VERSION,
        PROJECT_REGISTRY_VERSION, ProjectOpen, ProjectPreferencesV1, ProjectRecord,
        ProjectRegistry,
    },
};

const MAX_PREFERENCE_BYTES: u64 = 1024 * 1024;

struct Stored<T> {
    path: PathBuf,
    revision: u64,
    baseline: Option<Vec<u8>>,
    envelope: Option<Map<String, Value>>,
    writable: bool,
    value: T,
    warning: Option<String>,
}

impl<T> Stored<T>
where
    T: DeserializeOwned + Serialize + Default + Clone,
{
    fn load(path: PathBuf, version: u32) -> Self {
        let mut stored = Self {
            path: path.clone(),
            revision: 0,
            baseline: None,
            envelope: None,
            writable: true,
            value: T::default(),
            warning: None,
        };
        let bytes = match read_existing(&path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return stored,
            Err(error) => {
                stored.writable = false;
                stored.warning = Some(format!(
                    "Could not read preferences at {}: {error}. The file is preserved; fix it and restart to enable persistence.",
                    path.display()
                ));
                return stored;
            }
        };
        let parsed = serde_json::from_slice::<Value>(&bytes);
        let map = match parsed {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                stored.quarantine(&"preferences must be a JSON object");
                return stored;
            }
            Err(error) => {
                stored.quarantine(&error);
                return stored;
            }
        };
        let expected = u64::from(version);
        match map.get("version") {
            Some(Value::Number(number)) if number.as_u64() == Some(expected) => {}
            // A structurally valid but different version is a newer (or older)
            // format: preserve it read-only rather than quarantining data a
            // newer build may still need.
            Some(Value::Number(number)) if number.as_u64().is_some() => {
                stored.writable = false;
                stored.warning = Some(format!(
                    "Unsupported preference version in {}. The file is preserved and will not be overwritten.",
                    path.display()
                ));
                return stored;
            }
            // A missing, negative, fractional or non-numeric version is not a
            // valid envelope: recover through the corruption path.
            _ => {
                stored.quarantine(&"preference envelope must carry a non-negative integer version");
                return stored;
            }
        }
        match serde_json::from_value::<T>(Value::Object(map.clone())) {
            Ok(value) => {
                stored.baseline = Some(bytes);
                stored.envelope = Some(map);
                stored.value = value;
            }
            Err(error) => stored.quarantine(&error),
        }
        stored
    }

    fn quarantine(&mut self, error: &dyn std::fmt::Display) {
        let destination = match quarantine_file(&self.path) {
            Ok(destination) => destination,
            Err(rename_error) => {
                self.writable = false;
                self.warning = Some(format!(
                    "Could not load preferences at {}: {error}. The file is preserved; fix it and restart to enable persistence ({rename_error}).",
                    self.path.display()
                ));
                return;
            }
        };
        tracing::warn!(
            path = %self.path.display(),
            quarantined = %destination.display(),
            %error,
            "quarantined invalid preferences"
        );
        self.warning = Some(format!(
            "Could not load preferences at {}: {error}. The invalid file was moved to {}; defaults are active.",
            self.path.display(),
            destination.display()
        ));
    }

    /// Serialize writers; stale tasks cannot replace a newer successful snapshot.
    fn save(&mut self, revision: u64, value: &T) -> io::Result<bool> {
        if !self.writable {
            return Err(io::Error::other(
                "Preferences are read-only after a load error; fix the file and restart.",
            ));
        }
        if revision <= self.revision {
            return Ok(false);
        }
        if read_existing(&self.path)? != self.baseline {
            return Err(io::Error::other(
                "Preferences changed outside Astrolith; restart before saving settings.",
            ));
        }
        let Value::Object(typed) = serde_json::to_value(value).map_err(io::Error::other)? else {
            return Err(io::Error::other("Preference value must be an object."));
        };
        let mut merged = self.envelope.clone().unwrap_or_default();
        for (key, value) in typed {
            merged.insert(key, value);
        }
        let bytes =
            serde_json::to_vec_pretty(&Value::Object(merged.clone())).map_err(io::Error::other)?;
        write_atomic(&self.path, &bytes)?;
        self.baseline = Some(bytes);
        self.envelope = Some(merged);
        self.value = value.clone();
        self.revision = revision;
        tracing::debug!(
            path = %self.path.display(),
            revision,
            "preferences saved"
        );
        Ok(true)
    }

    fn save_current(&mut self) -> io::Result<bool> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Preference revision exhausted."))?;
        let value = self.value.clone();
        self.save(revision, &value)
    }
}

impl Stored<GlobalPreferencesV2> {
    /// Retire the pre-neutral light/dark palette selectors from `global.json`.
    ///
    /// The exact file is copied beside itself before the two keys are removed,
    /// so a user can always inspect what the previous build persisted. The
    /// neutral appearance is now the only one, and selection has moved out of
    /// the user's configuration. Unknown top-level fields are still preserved.
    fn retire_legacy_palettes(&mut self) -> Option<String> {
        const RETIRED: [&str; 2] = ["light_theme", "dark_theme"];
        let envelope = self.envelope.as_mut()?;
        if !RETIRED.iter().any(|key| envelope.contains_key(*key)) {
            return None;
        }
        if !self.writable {
            return Some(format!(
                "Retired appearance palettes remain in {} because the file is read-only.",
                self.path.display()
            ));
        }
        let baseline = self.baseline.as_deref()?;
        let backup = unique_destination(&self.path.with_file_name("global.json.pre-neutral.json"));
        if let Err(error) = fs::write(&backup, baseline) {
            return Some(format!(
                "Could not back up retired appearance palettes to {}: {error}. The settings file is preserved.",
                backup.display()
            ));
        }
        for key in RETIRED {
            envelope.remove(key);
        }
        let bytes = match serde_json::to_vec_pretty(&Value::Object(envelope.clone())) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Some(format!(
                    "Could not encode migrated appearance settings: {error}."
                ));
            }
        };
        match write_atomic(&self.path, &bytes) {
            Ok(()) => {
                self.baseline = Some(bytes);
                tracing::info!(
                    path = %self.path.display(),
                    backup = %backup.display(),
                    "retired legacy appearance palettes"
                );
                None
            }
            Err(error) => Some(format!(
                "Could not retire legacy appearance palettes in {}: {error}. The file is preserved.",
                self.path.display()
            )),
        }
    }
}

/// Project scopes and the open-project registry share one store so a project
/// open updates both files under one lock and one atomic write.
pub(crate) struct PreferencesStore {
    global: Mutex<Stored<GlobalPreferencesV2>>,
    registry: Mutex<Stored<ProjectRegistry>>,
    projects: Mutex<BTreeMap<String, Stored<ProjectPreferencesV1>>>,
    projects_dir: PathBuf,
}

impl PreferencesStore {
    /// Load every scope root and migrate the legacy single file once.
    pub fn load(global_path: PathBuf) -> (Self, GlobalPreferencesV2, Option<String>) {
        tracing::debug!(path = %global_path.display(), "loading preferences");
        let migration_warning = migrate::migrate_legacy(
            &global_path,
            migrate::legacy_settings_path(&global_path).as_deref(),
        );
        let root = global_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut global =
            Stored::<GlobalPreferencesV2>::load(global_path, GLOBAL_PREFERENCES_VERSION);
        let palette_migration_warning = global.retire_legacy_palettes();
        let registry =
            Stored::<ProjectRegistry>::load(root.join("projects.json"), PROJECT_REGISTRY_VERSION);
        let warning = combine_warnings([
            migration_warning,
            palette_migration_warning,
            global.warning.clone(),
            registry.warning.clone(),
        ]);
        let preferences = global.value.clone();
        (
            Self {
                global: Mutex::new(global),
                registry: Mutex::new(registry),
                projects: Mutex::new(BTreeMap::new()),
                projects_dir: root.join("projects"),
            },
            preferences,
            warning,
        )
    }

    pub fn save_global(
        &self,
        revision: u64,
        preferences: &GlobalPreferencesV2,
    ) -> io::Result<bool> {
        lock(&self.global)?.save(revision, preferences)
    }

    /// Test-only view of the registry; the recent-project UI lands in P12.
    #[cfg(test)]
    pub fn registry(&self) -> (ProjectRegistry, u64, Option<String>) {
        match self.registry.lock() {
            Ok(registry) => (
                registry.value.clone(),
                registry.revision,
                registry.warning.clone(),
            ),
            Err(_) => (
                ProjectRegistry::default(),
                0,
                Some("Project registry unavailable.".into()),
            ),
        }
    }

    /// Recent projects ordered by most recent open.
    pub fn recent_projects(&self) -> (Vec<ProjectRecord>, Option<String>) {
        match self.registry.lock() {
            Ok(registry) => (
                registry.value.recent().into_iter().cloned().collect(),
                registry.warning.clone(),
            ),
            Err(_) => (Vec::new(), Some("Project registry unavailable.".into())),
        }
    }

    /// Remove one project from the recent list and persist the registry.
    pub fn remove_recent_project(&self, id: &str) -> io::Result<bool> {
        let mut registry = lock(&self.registry)?;
        if !registry.value.remove(id) {
            return Ok(false);
        }
        registry.save_current()?;
        Ok(true)
    }

    /// Load one project scope, creating in-memory defaults for an unknown ID.
    ///
    /// The returned revision is the last accepted revision; pass a greater
    /// value to [`Self::save_project`].
    pub fn project(&self, id: &str) -> io::Result<(ProjectPreferencesV1, u64, Option<String>)> {
        let path = self.project_path(id)?;
        let mut projects = self
            .projects
            .lock()
            .map_err(|_| io::Error::other("Project preference store unavailable."))?;
        let stored = match projects.entry(id.to_owned()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let mut stored: Stored<ProjectPreferencesV1> =
                    Stored::load(path, PROJECT_PREFERENCES_VERSION);
                if stored.value.project_id.is_empty() {
                    stored.value.project_id = id.to_owned();
                }
                entry.insert(stored)
            }
        };
        Ok((
            stored.value.clone(),
            stored.revision,
            stored.warning.clone(),
        ))
    }

    pub fn save_project(
        &self,
        revision: u64,
        preferences: &ProjectPreferencesV1,
    ) -> io::Result<bool> {
        let id = preferences.project_id.as_str();
        let path = self.project_path(id)?;
        let mut projects = self
            .projects
            .lock()
            .map_err(|_| io::Error::other("Project preference store unavailable."))?;
        let stored = projects
            .entry(id.to_owned())
            .or_insert_with(|| Stored::load(path, PROJECT_PREFERENCES_VERSION));
        stored.save(revision, preferences)
    }

    /// Record a project open, rebinding a moved project only on package identity.
    ///
    /// Canonicalization and existence checks are blocking filesystem work.
    pub fn record_project_open(
        &self,
        path: &Path,
        package_name: Option<&str>,
        opened_at: u64,
    ) -> io::Result<ProjectOpen> {
        let canonical = fs::canonicalize(path)?;
        if !canonical.is_dir() {
            return Err(io::Error::other("Choose a project directory."));
        }
        if let Some(blocked) = astrolith_workspace::paths::blocked_root(
            &canonical,
            astrolith_workspace::paths::home_directory().as_deref(),
        ) {
            return Err(io::Error::other(blocked.message()));
        }
        let canonical = canonical
            .to_str()
            .ok_or_else(|| io::Error::other("Project path is not valid UTF-8."))?
            .to_owned();
        let mut registry = lock(&self.registry)?;
        let open = registry
            .value
            .record_open(&canonical, package_name, opened_at, |old_path| {
                Path::new(old_path).exists()
            });
        let project_path = self.project_path(&open.id)?;
        let mut projects = self
            .projects
            .lock()
            .map_err(|_| io::Error::other("Project preference store unavailable."))?;
        let stored = projects
            .entry(open.id.clone())
            .or_insert_with(|| Stored::load(project_path, PROJECT_PREFERENCES_VERSION));
        let mut preferences = if stored.value.project_id.is_empty() {
            ProjectPreferencesV1::for_project(
                open.id.clone(),
                open.path.clone(),
                open.display_name.clone(),
                opened_at,
            )
        } else {
            stored.value.clone()
        };
        preferences.path = open.path.clone();
        preferences.display_name = open.display_name.clone();
        preferences.last_opened_at = Some(opened_at);
        stored.value = preferences;
        stored.save_current()?;
        registry.save_current()?;
        Ok(open)
    }

    fn project_path(&self, id: &str) -> io::Result<PathBuf> {
        if id.is_empty()
            || !id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(io::Error::other("Invalid project identity."));
        }
        Ok(self.projects_dir.join(format!("{id}.json")))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> io::Result<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| io::Error::other("Preference store unavailable."))
}

fn combine_warnings<const N: usize>(warnings: [Option<String>; N]) -> Option<String> {
    let warnings: Vec<String> = warnings.into_iter().flatten().collect();
    (!warnings.is_empty()).then(|| warnings.join(" "))
}

fn read_existing(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAX_PREFERENCE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_PREFERENCE_BYTES {
                return Err(io::Error::other(format!(
                    "Preferences exceed {} KiB.",
                    MAX_PREFERENCE_BYTES / 1024
                )));
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("Preferences need a parent directory."))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn quarantine_file(path: &Path) -> io::Result<PathBuf> {
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "preference file is missing",
        ));
    }
    let destination = unique_destination(&path.with_file_name(format!(
        "{}.corrupt",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default()
    )));
    fs::rename(path, &destination)?;
    Ok(destination)
}

pub(super) fn unique_destination(desired: &Path) -> PathBuf {
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let stem = desired
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "preferences".into());
    let extension = desired
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let parent = desired.parent().unwrap_or_else(|| Path::new("."));
    for suffix in 2..=u32::MAX {
        let candidate = parent.join(format!("{stem}-{suffix}{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    desired.with_file_name(format!("{stem}-overflow{extension}"))
}

fn application_config_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("Application configuration directory unavailable."))
}

pub(crate) fn settings_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("ASTROLITH_SETTINGS_PATH") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err(io::Error::other(
                "ASTROLITH_SETTINGS_PATH must be absolute.",
            ))
        };
    }
    Ok(application_config_dir()?.join("astrolith/preferences/global.json"))
}

/// Directory holding redacted save-failure artifacts, alongside preferences.
pub(crate) fn save_failure_dir() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("ASTROLITH_SAVE_FAILURE_DIR") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err(io::Error::other(
                "ASTROLITH_SAVE_FAILURE_DIR must be absolute.",
            ))
        };
    }
    Ok(application_config_dir()?.join("astrolith/save-failures"))
}

/// Directory holding crash-recovery records, alongside preferences.
pub(crate) fn recovery_dir() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("ASTROLITH_RECOVERY_DIR") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err(io::Error::other("ASTROLITH_RECOVERY_DIR must be absolute."))
        };
    }
    Ok(application_config_dir()?.join("astrolith/recovery"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::preferences::{PanelLayout, ThemePreference};
    use std::fs;

    fn global_path(directory: &Path) -> PathBuf {
        directory.join("preferences/global.json")
    }

    #[test]
    fn saved_theme_and_layout_restart_and_preserve_unknown_envelope_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"version":2,"theme":"system","layout":{"sidebar_visible":true,"inspector_visible":true,"preview_visible":false,"sidebar_width":240.0,"inspector_width":260.0},"future_field":{"keep":true}}"#,
        )
        .unwrap();
        let (store, mut preferences, warning) = PreferencesStore::load(path.clone());
        assert!(warning.is_none());
        preferences.theme = ThemePreference::Dark;
        preferences.layout.sidebar_visible = false;
        preferences.layout.preview_visible = true;
        preferences.layout.inspector_width = 275.;
        store.save_global(1, &preferences).unwrap();

        let (_, restored, warning) = PreferencesStore::load(path.clone());
        assert!(warning.is_none());
        assert_eq!(restored.theme, ThemePreference::Dark);
        assert!(!restored.layout.sidebar_visible);
        assert!(restored.layout.preview_visible);
        assert_eq!(restored.layout.inspector_width, 275.);

        let envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(envelope["future_field"]["keep"], true);
    }

    #[test]
    fn retired_palettes_are_backed_up_and_removed_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"{"version":2,"theme":"dark","light_theme":"nord_light","dark_theme":"ink","future_field":{"keep":true}}"#;
        fs::write(&path, original).unwrap();

        let (store, mut preferences, warning) = PreferencesStore::load(path.clone());
        assert!(warning.is_none());
        assert_eq!(preferences.theme, ThemePreference::Dark);

        let backup = path.with_file_name("global.json.pre-neutral.json");
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        let envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(envelope.get("light_theme").is_none());
        assert!(envelope.get("dark_theme").is_none());
        assert_eq!(envelope["future_field"]["keep"], true);
        assert_eq!(envelope["theme"], "dark");

        preferences.theme = ThemePreference::Light;
        store.save_global(1, &preferences).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("light_theme"));
        assert!(!saved.contains("dark_theme"));

        let (_, restored, warning) = PreferencesStore::load(path.clone());
        assert!(warning.is_none());
        assert_eq!(restored.theme, ThemePreference::Light);
        assert!(
            !path
                .with_file_name("global.json.pre-neutral-2.json")
                .exists(),
            "the migration is a one-time backup"
        );
    }

    #[test]
    fn legacy_preferences_migrate_once_and_are_never_read_again() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        let legacy = directory.path().join("preferences.json");
        fs::write(
            &legacy,
            r#"{"version":1,"theme":"dark","layout":{"sidebar_visible":false,"inspector_visible":true,"preview_visible":true,"sidebar_width":200.0,"inspector_width":300.0}}"#,
        )
        .unwrap();

        let (_, migrated, warning) = PreferencesStore::load(path.clone());
        assert!(warning.is_none());
        assert_eq!(migrated.theme, ThemePreference::Dark);
        assert!(!migrated.layout.sidebar_visible);
        assert!(migrated.layout.preview_visible);
        assert_eq!(migrated.layout.inspector_width, 300.);
        assert!(path.exists());
        assert!(!legacy.exists());
        assert!(
            directory
                .path()
                .join("preferences.v1.migrated.json")
                .exists()
        );

        fs::remove_file(&path).unwrap();
        let (_, defaults, _) = PreferencesStore::load(path);
        assert_eq!(defaults.theme, ThemePreference::System);
        assert_eq!(defaults.layout, PanelLayout::default());
    }

    #[test]
    fn malformed_global_preferences_are_quarantined_and_persistence_recovers() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{broken").unwrap();
        let (store, defaults, warning) = PreferencesStore::load(path.clone());
        let warning = warning.expect("corruption is surfaced");
        assert!(warning.contains("moved"));
        assert!(!path.exists());
        let quarantined = path.with_file_name("global.json.corrupt");
        assert_eq!(fs::read_to_string(&quarantined).unwrap(), "{broken");
        store.save_global(1, &defaults).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn future_versions_are_preserved_and_writes_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"version":999}"#).unwrap();
        let (store, defaults, warning) = PreferencesStore::load(path.clone());
        assert!(warning.is_some());
        assert!(store.save_global(1, &defaults).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"version":999}"#);
    }

    #[test]
    fn invalid_version_envelopes_are_quarantined() {
        for content in [
            r#"{}"#,
            r#"{"version":"2"}"#,
            r#"{"version":-2}"#,
            r#"{"version":1.5}"#,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = global_path(directory.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            let (store, defaults, warning) = PreferencesStore::load(path.clone());
            let warning = warning.expect("an invalid envelope is surfaced");
            assert!(warning.contains("moved"), "{content}: {warning}");
            assert!(!path.exists(), "{content}");
            store.save_global(1, &defaults).unwrap();
            assert!(path.exists(), "{content}");
        }
    }

    #[test]
    fn older_write_cannot_replace_a_newer_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        let (store, mut preferences, _) = PreferencesStore::load(path.clone());
        preferences.theme = ThemePreference::Dark;
        store.save_global(2, &preferences).unwrap();
        preferences.theme = ThemePreference::Light;
        assert!(!store.save_global(1, &preferences).unwrap());
        assert_eq!(PreferencesStore::load(path).1.theme, ThemePreference::Dark);
    }

    #[test]
    fn recent_projects_are_ordered_and_can_be_removed() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        let (store, _, _) = PreferencesStore::load(path.clone());
        let first = directory.path().join("one");
        let second = directory.path().join("two");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let first = store.record_project_open(&first, Some("one"), 10).unwrap();
        let second = store.record_project_open(&second, Some("two"), 20).unwrap();
        let (recent, warning) = store.recent_projects();
        assert!(warning.is_none());
        assert_eq!(
            recent
                .iter()
                .map(|project| project.id.as_str())
                .collect::<Vec<_>>(),
            [second.id.as_str(), first.id.as_str()]
        );
        assert!(store.remove_recent_project(&first.id).unwrap());
        assert!(!store.remove_recent_project(&first.id).unwrap());
        let (reload, _, _) = PreferencesStore::load(path);
        let (recent, _) = reload.recent_projects();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, second.id);
    }

    #[cfg(unix)]
    #[test]
    fn a_system_directory_is_refused_and_never_registered() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _, _) = PreferencesStore::load(global_path(directory.path()));
        assert!(
            store
                .record_project_open(Path::new("/etc"), None, 1)
                .is_err()
        );
        let (recent, _) = store.recent_projects();
        assert!(recent.is_empty());
        let registry_path = directory.path().join("projects.json");
        assert!(
            !registry_path.exists()
                || fs::read_to_string(&registry_path)
                    .unwrap()
                    .contains("\"projects\":[]")
        );
    }

    #[test]
    fn registry_rebinds_a_moved_project_and_persists_its_scope() {
        let directory = tempfile::tempdir().unwrap();
        let path = global_path(directory.path());
        let left = directory.path().join("left/astro");
        let right = directory.path().join("right/astro");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        let (store, _, _) = PreferencesStore::load(path.clone());
        let canonical_left = fs::canonicalize(&left)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let first = store
            .record_project_open(&left, Some("astro-site"), 10)
            .unwrap();
        fs::remove_dir_all(&left).unwrap();
        let moved = store
            .record_project_open(&right, Some("astro-site"), 20)
            .unwrap();
        assert_eq!(moved.id, first.id);
        assert_eq!(moved.rebound_from.as_deref(), Some(canonical_left.as_str()));
        let (mut preferences, revision, warning) = store.project(&moved.id).unwrap();
        assert!(warning.is_none());
        assert_eq!(preferences.path, moved.path);
        preferences.last_entry = Some("blog/first.md".into());
        preferences.layout = Some(PanelLayout {
            sidebar_visible: false,
            preview_visible: true,
            ..PanelLayout::default()
        });
        assert!(store.save_project(revision + 1, &preferences).unwrap());

        let (restored, _, warning) = PreferencesStore::load(path.clone())
            .0
            .project(&moved.id)
            .unwrap();
        assert!(warning.is_none());
        assert_eq!(restored.last_entry.as_deref(), Some("blog/first.md"));
        assert!(!restored.layout.as_ref().unwrap().sidebar_visible);
        assert!(restored.layout.as_ref().unwrap().preview_visible);
        assert_eq!(restored.layout.as_ref().unwrap().preview_height, 160.);

        let registry_path = path.parent().unwrap().join("projects.json");
        let registry: serde_json::Value =
            serde_json::from_slice(&fs::read(registry_path).unwrap()).unwrap();
        let projects = registry["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["id"], first.id);
        assert_eq!(projects[0]["path"], moved.path);
    }
}
