//! Recent-project commands over the P1 registry.
//!
//! The registry is the only source of ordering: entries are listed by most
//! recent open, labels are generated once here, and the menu, palette and
//! welcome dialog consume the same values.

use std::path::{Path, PathBuf};

use astrolith_core::preferences::ProjectRecord;
use astrolith_workspace::paths::blocked_root;

/// One recent project ready for menus and the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecentEntry {
    /// Stable registry identity.
    pub id: String,
    /// Canonical project root.
    pub path: PathBuf,
    /// Label shared by every menu and the palette.
    pub label: String,
}

/// Build labels for the registry order, disambiguating same-named projects.
pub(crate) fn entries(records: &[ProjectRecord]) -> Vec<RecentEntry> {
    records
        .iter()
        .map(|record| {
            let name = if record.display_name.trim().is_empty() {
                Path::new(&record.path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| record.path.clone())
            } else {
                record.display_name.clone()
            };
            let duplicate = records
                .iter()
                .filter(|other| other.id != record.id && other.display_name == record.display_name)
                .count()
                > 0;
            let label = if duplicate {
                let parent = Path::new(&record.path)
                    .parent()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                tr!(
                    "browser.recent.open_in_parent",
                    name = name,
                    parent = parent
                )
            } else {
                tr!("browser.recent.open", name = name)
            };
            RecentEntry {
                id: record.id.clone(),
                path: PathBuf::from(&record.path),
                label,
            }
        })
        .collect()
}

/// The most recent project that can still be opened.
///
/// A missing, protected or unreadable entry is skipped so a stale registry
/// cannot block the launch on the welcome state.
pub(crate) fn last_openable(records: &[ProjectRecord]) -> Option<PathBuf> {
    records
        .iter()
        .find(|record| {
            let path = Path::new(&record.path);
            path.is_dir()
                && blocked_root(
                    path,
                    astrolith_workspace::paths::home_directory().as_deref(),
                )
                .is_none()
        })
        .map(|record| PathBuf::from(&record.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, path: &str, name: &str) -> ProjectRecord {
        ProjectRecord {
            id: id.into(),
            path: path.into(),
            display_name: name.into(),
            package_name: None,
            last_opened_at: 1,
        }
    }

    #[test]
    fn labels_disambiguate_only_when_names_collide() {
        let entries = entries(&[
            record("one", "/projects/one/site", "site"),
            record("two", "/projects/two/site", "site"),
            record("three", "/projects/three/other", "other"),
        ]);
        assert_eq!(entries[0].label, "Open site (one)");
        assert_eq!(entries[1].label, "Open site (two)");
        assert_eq!(entries[2].label, "Open other");
    }

    #[test]
    fn the_last_openable_project_skips_missing_and_protected_roots() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("site");
        std::fs::create_dir(&existing).unwrap();
        let records = [
            record("gone", "/definitely/missing/site", "site"),
            record("system", "/etc", "etc"),
            record("ok", existing.to_str().unwrap(), "site"),
        ];
        assert_eq!(last_openable(&records), Some(existing));
        assert_eq!(last_openable(&records[..2]), None);
    }
}
