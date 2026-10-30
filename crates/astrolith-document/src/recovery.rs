//! Recovery records for unsaved document content.
//!
//! A [`RecoveryStore`] persists dirty buffers into a directory so that content
//! can be offered back after an unclean exit or crash. Records are written
//! atomically and keyed by project + canonical source, and are loaded back as
//! typed [`RecoveryRecord`]s. Recovery content is resolved against a
//! caller-supplied project root at recovery time — the record never stores an
//! authoritative absolute path, so a tampered record file cannot redirect a
//! write to an arbitrary location.
//!
//! Invalid record files are reported as diagnostics and skipped; one corrupt
//! file never aborts recovery.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use astrolith_core::{ProjectId, SourcePath};
use serde_json::{Value, json};

use crate::buffer::{DocumentBuffer, DocumentError, DocumentIdentity, DocumentRevision};
use crate::save::write_atomic;

/// One persisted unsaved document snapshot.
#[derive(Clone, Debug)]
pub struct RecoveryRecord {
    identity: DocumentIdentity,
    revision: DocumentRevision,
    content: String,
    modified_unix_secs: u64,
}

impl RecoveryRecord {
    /// Construct a recovery record from its parts.
    pub fn new(
        identity: DocumentIdentity,
        revision: DocumentRevision,
        content: impl Into<String>,
        modified_unix_secs: u64,
    ) -> Self {
        Self {
            identity,
            revision,
            content: content.into(),
            modified_unix_secs,
        }
    }

    /// Capture `buffer`'s current content as a recovery record.
    pub fn capture(buffer: &DocumentBuffer, modified_unix_secs: u64) -> Self {
        Self::new(
            buffer.identity().clone(),
            buffer.revision(),
            buffer.current_text().to_owned(),
            modified_unix_secs,
        )
    }

    /// Return the document identity.
    pub fn identity(&self) -> &DocumentIdentity {
        &self.identity
    }

    /// Return the captured content revision.
    pub fn revision(&self) -> DocumentRevision {
        self.revision
    }

    /// Return the recovered unsaved content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Return the last-modified time as Unix seconds.
    pub fn modified_unix_secs(&self) -> u64 {
        self.modified_unix_secs
    }

    /// Resolve the source path against a caller-supplied project root.
    ///
    /// The record stores only the validated relative [`SourcePath`], so this is
    /// the only way to obtain an absolute path; it cannot be redirected by a
    /// tampered record.
    pub fn resolve_path(&self, project_root: &Path) -> PathBuf {
        project_root.join(self.identity.canonical_source().as_str())
    }

    /// Report whether the original file still exists under `project_root`.
    pub fn has_original(&self, project_root: &Path) -> bool {
        self.resolve_path(project_root).is_file()
    }
}

/// The result of loading a recovery directory.
#[derive(Clone, Debug, Default)]
pub struct RecoveryLoad {
    records: Vec<RecoveryRecord>,
    diagnostics: Vec<String>,
}

impl RecoveryLoad {
    /// Return the successfully parsed records, newest first.
    pub fn records(&self) -> &[RecoveryRecord] {
        &self.records
    }

    /// Return human-readable diagnostics for skipped/corrupt records.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Report whether no records were loaded.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// A directory-backed store of recovery records.
#[derive(Clone, Debug)]
pub struct RecoveryStore {
    directory: PathBuf,
}

impl RecoveryStore {
    /// Create a store rooted at `directory` (created lazily on first write).
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// Return the backing directory.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Return the record file path for one document identity.
    pub fn record_path(&self, identity: &DocumentIdentity) -> PathBuf {
        self.directory
            .join(format!("{}.json", record_key(identity)))
    }

    /// Persist a recovery record atomically, replacing any prior record for the
    /// same document. Writes for one document are serialized in-process.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Io`] if the directory cannot be created,
    /// serialization fails, or the write fails.
    pub fn save(&self, record: &RecoveryRecord) -> Result<(), DocumentError> {
        let lock = save_lock(&record_key(record.identity()));
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        fs::create_dir_all(&self.directory).map_err(io_error)?;
        let path = self.record_path(record.identity());
        write_atomic(&path, &serialize_record(record)?).map_err(io_error)
    }

    /// Load every valid record, reporting corrupt files as diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Io`] only if the directory exists but cannot be
    /// listed; a missing directory is treated as empty.
    pub fn load(&self) -> Result<RecoveryLoad, DocumentError> {
        let mut load = RecoveryLoad::default();
        if !self.directory.is_dir() {
            return Ok(load);
        }
        let entries = fs::read_dir(&self.directory).map_err(io_error)?;
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    load.diagnostics.push(format!("cannot read entry: {error}"));
                    continue;
                }
            };
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(text) => match parse_record(&text) {
                    Ok(record) => load.records.push(record),
                    Err(message) => load
                        .diagnostics
                        .push(format!("{}: {message}", file_label(&path))),
                },
                Err(error) => load
                    .diagnostics
                    .push(format!("{}: {error}", file_label(&path))),
            }
        }
        load.records
            .sort_by_key(|record| std::cmp::Reverse(record.modified_unix_secs));
        Ok(load)
    }

    /// Remove the recovery record for `identity`, if present. Returns whether a
    /// file was removed.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Io`] if the file exists but cannot be removed.
    pub fn clear(&self, identity: &DocumentIdentity) -> Result<bool, DocumentError> {
        remove_if_present(&self.record_path(identity))
    }

    /// Remove the record for `identity` when the persisted content equals
    /// `saved_content` (a successful save made the record obsolete).
    ///
    /// Comparing durable content — not the in-memory revision counter, which
    /// resets every time a document is reopened — means a record holding *newer*
    /// unsaved content is always kept, across process restarts. An unparseable
    /// record is removed. Returns whether a file was removed.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Io`] for read/remove failures other than a
    /// missing file.
    pub fn clear_after_save(
        &self,
        identity: &DocumentIdentity,
        saved_content: &str,
    ) -> Result<bool, DocumentError> {
        let path = self.record_path(identity);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_error(error)),
        };
        match parse_record(&text) {
            Ok(record) if record.content() != saved_content => Ok(false),
            _ => remove_if_present(&path),
        }
    }
}

fn remove_if_present(path: &Path) -> Result<bool, DocumentError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn serialize_record(record: &RecoveryRecord) -> Result<String, DocumentError> {
    let identity = record.identity();
    let value = json!({
        "project": identity.project().as_str(),
        "generation": identity.project_generation(),
        "source": identity.canonical_source().as_str(),
        "revision": record.revision().get(),
        "content": record.content(),
        "modified": record.modified_unix_secs(),
    });
    serde_json::to_string(&value).map_err(|error| DocumentError::Io {
        message: format!("cannot serialize recovery record: {error}"),
    })
}

fn parse_record(text: &str) -> Result<RecoveryRecord, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    let project = string_field(&value, "project")?;
    let generation = value
        .get("generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| "`generation` must be a non-negative integer".to_owned())?;
    let source = string_field(&value, "source")?;
    let content = string_field(&value, "content")?;
    let revision = value
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| "`revision` must be a non-negative integer".to_owned())?;
    let modified = value
        .get("modified")
        .and_then(Value::as_u64)
        .ok_or_else(|| "`modified` must be a non-negative integer".to_owned())?;

    let project = ProjectId::new(project).map_err(|error| error.to_string())?;
    let source = SourcePath::new(source).map_err(|error| error.to_string())?;
    Ok(RecoveryRecord::new(
        DocumentIdentity::new(project, generation, source),
        DocumentRevision::new(revision),
        content,
        modified,
    ))
}

fn string_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` must be a string"))
}

fn record_key(identity: &DocumentIdentity) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let bytes = identity
        .project()
        .as_str()
        .bytes()
        .chain([0x1f])
        .chain(identity.canonical_source().as_str().bytes());
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn save_lock(key: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(key.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<record>")
        .to_owned()
}

fn io_error(error: std::io::Error) -> DocumentError {
    DocumentError::Io {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_for(source: &str) -> DocumentIdentity {
        DocumentIdentity::new(
            ProjectId::new("project-a").expect("valid project"),
            3,
            SourcePath::new(source).expect("valid source"),
        )
    }

    fn identity() -> DocumentIdentity {
        identity_for("content/doc.md")
    }

    fn record(revision: u64, content: &str, modified: u64) -> RecoveryRecord {
        RecoveryRecord::new(
            identity(),
            DocumentRevision::new(revision),
            content,
            modified,
        )
    }

    #[test]
    fn save_then_load_roundtrips_a_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());

        store.save(&record(5, "unsaved text", 1_700)).unwrap();

        let load = store.load().unwrap();
        assert!(load.diagnostics().is_empty());
        assert_eq!(load.records().len(), 1);
        let recovered = &load.records()[0];
        assert_eq!(recovered.content(), "unsaved text");
        assert_eq!(recovered.revision(), DocumentRevision::new(5));
        assert_eq!(recovered.modified_unix_secs(), 1_700);
        assert_eq!(recovered.identity(), &identity());
        assert_eq!(
            recovered.resolve_path(dir.path()),
            dir.path().join("content/doc.md")
        );
    }

    #[test]
    fn invalid_record_is_skipped_with_a_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        store.save(&record(2, "good", 10)).unwrap();
        fs::write(dir.path().join("garbage.json"), "not json").unwrap();

        let load = store.load().unwrap();
        assert_eq!(load.records().len(), 1);
        assert_eq!(load.diagnostics().len(), 1);
        assert!(load.diagnostics()[0].contains("garbage.json"));
    }

    #[test]
    fn valid_json_missing_field_or_bad_identity_is_a_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        fs::write(dir.path().join("missing.json"), r#"{"project":"p"}"#).unwrap();
        fs::write(
            dir.path().join("badid.json"),
            r#"{"project":"","generation":0,"source":"a.md","revision":1,"content":"x","modified":1}"#,
        )
        .unwrap();

        let load = store.load().unwrap();
        assert!(load.records().is_empty());
        assert_eq!(load.diagnostics().len(), 2);
    }

    #[test]
    fn record_with_missing_original_is_still_recoverable_and_path_is_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        store.save(&record(1, "orphan content", 42)).unwrap();

        let load = store.load().unwrap();
        assert_eq!(load.records().len(), 1);
        assert!(!load.records()[0].has_original(dir.path()));

        fs::create_dir_all(dir.path().join("content")).unwrap();
        fs::write(dir.path().join("content/doc.md"), "disk").unwrap();
        assert!(load.records()[0].has_original(dir.path()));
    }

    #[test]
    fn clear_after_confirmed_save_removes_the_obsolete_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        store.save(&record(4, "unsaved", 100)).unwrap();

        assert!(store.clear_after_save(&identity(), "unsaved").unwrap());
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn clear_after_save_keeps_newer_unsaved_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        store.save(&record(6, "newer unsaved", 100)).unwrap();

        assert!(!store.clear_after_save(&identity(), "older saved").unwrap());
        let load = store.load().unwrap();
        assert_eq!(load.records().len(), 1);
        assert_eq!(load.records()[0].content(), "newer unsaved");
    }

    #[test]
    fn corrupt_record_is_removed_on_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        let path = store.record_path(&identity());
        fs::write(&path, "not json").unwrap();

        assert!(store.clear_after_save(&identity(), "anything").unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn two_documents_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        let a = identity_for("content/a.md");
        let b = identity_for("content/b.md");
        store
            .save(&RecoveryRecord::new(
                a.clone(),
                DocumentRevision::new(1),
                "A",
                10,
            ))
            .unwrap();
        store
            .save(&RecoveryRecord::new(
                b.clone(),
                DocumentRevision::new(1),
                "B",
                20,
            ))
            .unwrap();

        assert_eq!(store.load().unwrap().records().len(), 2);
        assert!(store.clear(&a).unwrap());
        let remaining = store.load().unwrap();
        assert_eq!(remaining.records().len(), 1);
        assert_eq!(remaining.records()[0].identity(), &b);
    }

    #[test]
    fn clear_reports_absent_record_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        assert!(!store.clear(&identity()).unwrap());
    }

    #[test]
    fn a_directory_named_like_a_record_is_a_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path());
        let path = store.record_path(&identity());
        fs::create_dir_all(&path).unwrap();

        let load = store.load().unwrap();
        assert!(load.records().is_empty());
        assert_eq!(load.diagnostics().len(), 1);
    }
}
