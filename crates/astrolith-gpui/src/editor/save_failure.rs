//! Redacted local artifacts for failed document saves.
//!
//! A save failure leaves a small JSON record in a dedicated directory so the
//! user can attach it to a bug report. The record is deliberately content-free:
//! it carries the timestamp, the error class, the application version, the build
//! identity and the *project-relative* source path. Buffer text, frontmatter
//! values, snippets, environment variables and absolute paths never appear.
//!
//! Artifacts are written off the render path, are never transmitted, and are
//! deleted by nothing automatic.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

use serde::Serialize;

/// Application version reported by this crate's package metadata.
pub(crate) const APPLICATION_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build identity supplied by the packaging build, or `dev` locally.
pub(crate) fn build_identity() -> &'static str {
    option_env!("ASTROLITH_BUILD_ID").unwrap_or("dev")
}

/// One redacted save-failure record.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct SaveFailureArtifact {
    /// Seconds since the Unix epoch.
    pub timestamp_unix_secs: u64,
    /// Redacted error class, never the error message.
    pub error_class: &'static str,
    /// Application version.
    pub application_version: &'static str,
    /// Build identity.
    pub build_identity: &'static str,
    /// Canonical project-relative source path.
    pub source: String,
}

impl SaveFailureArtifact {
    /// Classify a save failure without retaining any of its message.
    pub(crate) fn new(error_class: &'static str, source: impl Into<String>) -> Self {
        let timestamp_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        Self {
            timestamp_unix_secs,
            error_class,
            application_version: APPLICATION_VERSION,
            build_identity: build_identity(),
            source: source.into(),
        }
    }

    /// Write this record atomically into `directory`, returning its path.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the directory or file cannot be
    /// written; a failure to record never changes the document state.
    pub(crate) fn write(&self, directory: &Path) -> io::Result<PathBuf> {
        fs::create_dir_all(directory)?;
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            "save-failure-{}-{}-{sequence}",
            self.timestamp_unix_secs,
            std::process::id()
        );
        let path = directory.join(format!("{name}.json"));
        let temporary = directory.join(format!(".{name}.tmp"));
        let json = serde_json::to_vec_pretty(self)
            .map_err(|error| io::Error::other(format!("cannot serialize the artifact: {error}")))?;
        fs::write(&temporary, json)?;
        fs::rename(&temporary, &path)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn an_artifact_records_status_metadata_and_the_relative_source_only() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = SaveFailureArtifact::new("io", "src/content/posts/a.md");
        let path = artifact.write(directory.path()).unwrap();
        let value = json(&path);
        assert_eq!(value["error_class"], "io");
        assert_eq!(value["source"], "src/content/posts/a.md");
        assert_eq!(value["application_version"], APPLICATION_VERSION);
        assert_eq!(value["build_identity"], build_identity());
        assert!(value["timestamp_unix_secs"].as_u64().unwrap() > 0);
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "application_version",
                "build_identity",
                "error_class",
                "source",
                "timestamp_unix_secs"
            ],
            "no document content may reach an artifact"
        );
    }

    #[test]
    fn writing_twice_keeps_both_records_and_leaves_no_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        SaveFailureArtifact::new("io", "a.md")
            .write(directory.path())
            .unwrap();
        SaveFailureArtifact::new("io", "b.md")
            .write(directory.path())
            .unwrap();
        let names: Vec<String> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|name| name.ends_with(".json")));
    }
}
