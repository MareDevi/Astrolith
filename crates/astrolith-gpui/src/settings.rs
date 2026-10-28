//! Versioned preference storage. Call disk operations only on a background executor.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{layout::state::LayoutState, theme::ThemeMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    version: u32,
    pub theme: ThemeMode,
    pub layout: LayoutState,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            theme: ThemeMode::System,
            layout: LayoutState::default(),
        }
    }
}

struct Stored {
    revision: u64,
    baseline: Option<Vec<u8>>,
    writable: bool,
}

pub(crate) struct SettingsStore {
    path: PathBuf,
    stored: Mutex<Stored>,
}

fn read_existing(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(65537).read_to_end(&mut bytes)?;
            if bytes.len() > 65536 {
                return Err(io::Error::other("Preferences exceed 64 KiB."));
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> (Self, Settings, Option<String>) {
        let loaded = read_existing(&path).and_then(|bytes| {
            let settings: Settings = match &bytes {
                Some(bytes) => serde_json::from_slice(bytes).map_err(io::Error::other)?,
                None => Settings::default(),
            };
            if settings.version != 1 {
                return Err(io::Error::other("Unsupported preference version."));
            }
            Ok((settings, bytes))
        });
        let (settings, baseline, warning, writable) = match loaded {
            Ok((settings, bytes)) => (settings, bytes, None, true),
            Err(error) => (
                Settings::default(),
                None,
                Some(format!(
                    "Could not load preferences: {error} Existing settings will be preserved."
                )),
                false,
            ),
        };
        (
            Self {
                path,
                stored: Mutex::new(Stored {
                    revision: 0,
                    baseline,
                    writable,
                }),
            },
            settings,
            warning,
        )
    }

    /// Serialize writers; stale tasks cannot replace a newer successful snapshot.
    pub fn save(&self, revision: u64, settings: &Settings) -> io::Result<bool> {
        let mut stored = self
            .stored
            .lock()
            .map_err(|_| io::Error::other("Preference writer unavailable."))?;
        if !stored.writable {
            return Err(io::Error::other(
                "Preferences are protected after a load error; fix the file and restart.",
            ));
        }
        if revision <= stored.revision {
            return Ok(false);
        }
        if read_existing(&self.path)? != stored.baseline {
            return Err(io::Error::other(
                "Preferences changed outside Astrolith; restart before saving settings.",
            ));
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Preferences need a parent directory."))?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        stored.baseline = Some(bytes);
        stored.revision = revision;
        Ok(true)
    }
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
        .map(|path| path.join("astrolith/preferences.json"))
        .ok_or_else(|| io::Error::other("Application configuration directory unavailable."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeMode;

    #[test]
    fn saved_preferences_restore_theme_visibility_and_native_split_widths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/preferences.json");
        let (store, _, warning) = SettingsStore::load(path.clone());
        assert!(warning.is_none());
        let mut settings = Settings {
            theme: ThemeMode::Dark,
            ..Default::default()
        };
        settings.layout.sidebar_visible = false;
        settings.layout.preview_visible = true;
        settings.layout.inspector_width = 260.;
        store.save(1, &settings).unwrap();
        let (_, restored, warning) = SettingsStore::load(path);
        assert!(warning.is_none());
        assert_eq!(restored.theme, ThemeMode::Dark);
        assert!(!restored.layout.sidebar_visible);
        assert!(restored.layout.preview_visible);
        assert_eq!(restored.layout.inspector_width, 260.);
    }

    #[test]
    fn older_write_cannot_replace_newer_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        let (store, mut settings, _) = SettingsStore::load(path.clone());
        settings.theme = ThemeMode::Dark;
        store.save(2, &settings).unwrap();
        settings.theme = ThemeMode::Light;
        assert!(!store.save(1, &settings).unwrap());
        assert_eq!(SettingsStore::load(path).1.theme, ThemeMode::Dark);
    }

    #[test]
    fn malformed_future_and_externally_changed_preferences_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        for content in ["{broken", "{\"version\":999}"] {
            std::fs::write(&path, content).unwrap();
            let (store, defaults, warning) = SettingsStore::load(path.clone());
            assert!(warning.is_some());
            assert!(store.save(1, &defaults).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
        std::fs::remove_file(&path).unwrap();
        let (store, settings, _) = SettingsStore::load(path.clone());
        store.save(1, &settings).unwrap();
        std::fs::write(&path, "external change").unwrap();
        assert!(store.save(2, &settings).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external change");
    }

    #[test]
    fn settings_read_errors_and_write_errors_remain_recoverable() {
        let directory = tempfile::tempdir().unwrap();
        let (store, settings, warning) = SettingsStore::load(directory.path().to_path_buf());
        assert!(warning.is_some());
        assert!(store.save(1, &settings).is_err());
        let blocker = directory.path().join("file");
        std::fs::write(&blocker, "keep").unwrap();
        let (store, settings, _) = SettingsStore::load(blocker.join("settings.json"));
        assert!(store.save(1, &settings).is_err());
        assert_eq!(std::fs::read_to_string(blocker).unwrap(), "keep");
    }
}
