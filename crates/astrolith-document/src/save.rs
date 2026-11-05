//! Conflict-aware, one-writer-per-document save coordinator.
//!
//! [`save_snapshot`] writes an immutable [`DocumentSnapshot`] only when the file
//! on disk still matches the snapshot's [`DiskBaseline`]; otherwise it reports
//! [`SaveOutcome::Conflict`] and leaves the file untouched. Writes are atomic
//! (temp file in the same directory, `fsync`, `rename`), and concurrent saves for
//! the same path are serialized process-wide so there is one writer per document.
//!
//! Documented limits: the read-compare-then-replace sequence is not a race-free
//! guarantee against a third-party writer between the check and the rename, and
//! a deletion after the check can be recreated. Atomic replacement prevents
//! partial files, not that window. The one-writer guarantee is per application
//! process, assumes callers do not also write the file externally, and keys on
//! the canonicalized path so relative/absolute/symlink spellings share a lock.
//! Atomic replace relies on POSIX `rename`; Windows target replacement is a
//! platform-adapter (M06) concern.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::sync::Mutex as AsyncMutex;

use crate::buffer::{DocumentBuffer, DocumentError, DocumentIdentity, DocumentRevision};
use crate::version::DiskBaseline;

/// Largest document that may be read into, or written from, the editor.
///
/// The open path refuses files above this cap, so a save above it would create
/// a file the application can never reopen or fingerprint again.
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Outcome of attempting to save one document snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    /// The snapshot was persisted; `revision` is the saved content revision.
    Saved {
        /// The revision whose content was persisted.
        revision: u64,
    },
    /// The on-disk file changed or vanished since the snapshot baseline.
    Conflict,
}

/// An immutable snapshot of a document to persist.
#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    path: PathBuf,
    identity: DocumentIdentity,
    revision: DocumentRevision,
    content: String,
    baseline: DiskBaseline,
}

impl DocumentSnapshot {
    /// Construct a snapshot from its parts.
    pub(crate) fn new(
        path: PathBuf,
        identity: DocumentIdentity,
        revision: DocumentRevision,
        content: String,
        baseline: DiskBaseline,
    ) -> Self {
        Self {
            path,
            identity,
            revision,
            content,
            baseline,
        }
    }

    /// Capture the current text and load baseline of `buffer` for `path`.
    pub fn capture(buffer: &DocumentBuffer, path: impl AsRef<Path>) -> Self {
        Self::new(
            path.as_ref().to_path_buf(),
            buffer.identity().clone(),
            buffer.revision(),
            buffer.current_text().to_owned(),
            DiskBaseline::new(buffer.original_text()),
        )
    }

    /// Return the absolute target path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the document identity.
    pub fn identity(&self) -> &DocumentIdentity {
        &self.identity
    }

    /// Return the captured revision.
    pub fn revision(&self) -> DocumentRevision {
        self.revision
    }

    /// Return the captured content to persist.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Save a snapshot with conflict detection and atomic replacement.
///
/// Saves for the same path are serialized so one writer owns a document at a
/// time. A clean save reports [`SaveOutcome::Saved`]; a file that no longer
/// matches the baseline (including a deleted file) reports
/// [`SaveOutcome::Conflict`] and is not modified.
///
/// # Errors
///
/// Returns [`DocumentError::Io`] when the file cannot be read or written for a
/// reason other than a normal conflict.
pub async fn save_snapshot(snapshot: &DocumentSnapshot) -> Result<SaveOutcome, DocumentError> {
    let lock = path_lock(&snapshot.path);
    let _guard = lock.lock().await;

    let path = snapshot.path.clone();
    let content = snapshot.content.clone();
    let baseline = snapshot.baseline.clone();
    let revision = snapshot.revision.get();
    tracing::debug!(
        path = %path.display(),
        revision,
        bytes = content.len(),
        "saving document snapshot"
    );

    tokio::task::spawn_blocking(move || save_blocking(&path, &content, &baseline, revision))
        .await
        .map_err(|error| DocumentError::Io {
            message: format!("save task panicked: {error}"),
        })?
}

fn save_blocking(
    path: &Path,
    content: &str,
    baseline: &DiskBaseline,
    revision: u64,
) -> Result<SaveOutcome, DocumentError> {
    if content.len() > MAX_DOCUMENT_BYTES {
        tracing::warn!(
            path = %path.display(),
            bytes = content.len(),
            limit = MAX_DOCUMENT_BYTES,
            "refusing to save a document above the readable size cap"
        );
        return Err(DocumentError::TooLarge {
            bytes: content.len(),
            limit: MAX_DOCUMENT_BYTES,
        });
    }
    let current = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tracing::warn!(
                path = %path.display(),
                "save conflict: the file no longer exists"
            );
            return Ok(SaveOutcome::Conflict);
        }
        Err(error) => {
            tracing::error!(
                path = %path.display(),
                %error,
                "could not read a document before saving"
            );
            return Err(DocumentError::Io {
                message: format!("cannot read {}: {error}", path.display()),
            });
        }
    };
    if !baseline.matches(&current) {
        tracing::warn!(
            path = %path.display(),
            "save conflict: the file changed on disk"
        );
        return Ok(SaveOutcome::Conflict);
    }
    write_atomic(path, content).map_err(|error| {
        tracing::error!(
            path = %path.display(),
            %error,
            "atomic document write failed"
        );
        DocumentError::Io {
            message: format!("cannot write {}: {error}", path.display()),
        }
    })?;
    tracing::debug!(path = %path.display(), revision, "document snapshot saved");
    Ok(SaveOutcome::Saved { revision })
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("save path has no parent directory"))?;
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());

    let mut attempt = 0_u32;
    let (temporary, mut file) = loop {
        let candidate = directory.join(format!(
            ".astrolith-save-{}-{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 8 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    };

    let write_result = (|| {
        file.write_all(content.as_bytes())?;
        if let Some(permissions) = permissions {
            fs::set_permissions(&temporary, permissions)?;
        }
        file.sync_all()?;
        Ok::<(), io::Error>(())
    })();
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Ok(directory_handle) = fs::File::open(directory) {
        let _ = directory_handle.sync_all();
    }
    Ok(())
}

fn path_lock(path: &Path) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(lock_key(path))
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

/// Canonicalize a save path for the writer lock, tolerating a not-yet-existing
/// file by canonicalizing its parent directory.
fn lock_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(canonical_parent) = parent.canonicalize()
    {
        return canonical_parent.join(name);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::{ProjectId, SourcePath};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn identity() -> DocumentIdentity {
        DocumentIdentity::new(
            ProjectId::new("project-a").expect("valid project id"),
            1,
            SourcePath::new("content/doc.md").expect("valid source path"),
        )
    }

    fn buffer(source: &str) -> DocumentBuffer {
        DocumentBuffer::new(identity(), source.to_owned())
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("runtime builds")
    }

    #[test]
    fn clean_save_writes_content_and_reports_revision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();

        let mut document = buffer("old");
        let revision = document.apply_text("new".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);

        let outcome = runtime().block_on(save_snapshot(&snapshot)).unwrap();
        assert_eq!(
            outcome,
            SaveOutcome::Saved {
                revision: revision.get()
            }
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert!(document.confirm_saved(revision, "new"));
        assert!(!document.is_dirty());
    }

    #[test]
    fn external_change_is_a_conflict_and_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "changed externally").unwrap();

        let mut document = buffer("old");
        document.apply_text("mine".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);

        let outcome = runtime().block_on(save_snapshot(&snapshot)).unwrap();
        assert_eq!(outcome, SaveOutcome::Conflict);
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed externally");
    }

    #[test]
    fn deleted_file_is_a_conflict_and_is_not_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.md");

        let mut document = buffer("old");
        document.apply_text("mine".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);

        let outcome = runtime().block_on(save_snapshot(&snapshot)).unwrap();
        assert_eq!(outcome, SaveOutcome::Conflict);
        assert!(!path.exists());
    }

    #[test]
    fn read_error_other_than_missing_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();

        let mut document = buffer("old");
        document.apply_text("mine".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);

        let result = runtime().block_on(save_snapshot(&snapshot));
        assert!(matches!(result, Err(DocumentError::Io { .. })));
    }

    #[test]
    fn concurrent_saves_serialize_with_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();

        let mut first = buffer("old");
        first.apply_text("A".to_owned()).unwrap();
        let mut second = buffer("old");
        second.apply_text("B".to_owned()).unwrap();
        let snap_a = DocumentSnapshot::capture(&first, &path);
        let snap_b = DocumentSnapshot::capture(&second, &path);

        let runtime = runtime();
        let (outcome_a, outcome_b) = runtime.block_on(async move {
            let handle_a = tokio::spawn(async move { save_snapshot(&snap_a).await });
            let handle_b = tokio::spawn(async move { save_snapshot(&snap_b).await });
            (handle_a.await.unwrap(), handle_b.await.unwrap())
        });

        let outcomes = [outcome_a.unwrap(), outcome_b.unwrap()];
        let saved = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SaveOutcome::Saved { .. }))
            .count();
        let conflicts = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SaveOutcome::Conflict))
            .count();
        assert_eq!(
            (saved, conflicts),
            (1, 1),
            "exactly one save wins, one conflicts"
        );
        let final_content = fs::read_to_string(&path).unwrap();
        assert!(final_content == "A" || final_content == "B");
    }

    #[test]
    fn confirm_saved_keeps_newer_edits_dirty() {
        let mut document = buffer("old");
        let saved_revision = document.apply_text("v1".to_owned()).unwrap();
        document.apply_text("v2".to_owned()).unwrap();

        assert!(!document.confirm_saved(saved_revision, "v1"));
        assert!(document.is_dirty());

        let current = document.revision();
        assert!(document.confirm_saved(current, "v2"));
        assert!(!document.is_dirty());
    }

    #[test]
    fn rebasing_after_a_save_lets_the_follow_up_persist_the_newer_revision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();

        let mut document = buffer("old");
        let saved_revision = document.apply_text("v1".to_owned()).unwrap();
        // The first save persists v1; the user keeps typing before it completes.
        let first = DocumentSnapshot::capture(&document, &path);
        let outcome = runtime().block_on(save_snapshot(&first)).unwrap();
        assert!(matches!(outcome, SaveOutcome::Saved { .. }));
        document.apply_text("v2".to_owned()).unwrap();
        assert!(!document.confirm_saved(saved_revision, "v1"));
        assert!(document.rebase_persisted(saved_revision, "v1"));

        let revision = document.revision();
        assert!(document.is_dirty(), "the newer revision is still unsaved");
        assert_eq!(document.original_text(), "v1");
        assert_eq!(document.current_text(), "v2");

        let follow_up = DocumentSnapshot::capture(&document, &path);
        assert_eq!(
            runtime().block_on(save_snapshot(&follow_up)),
            Ok(SaveOutcome::Saved {
                revision: revision.get()
            }),
            "the follow-up save must not conflict with our own write"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
        assert!(document.confirm_saved(revision, "v2"));
        assert!(!document.is_dirty());
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_replaces_the_link_not_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        fs::write(&real, "old").unwrap();
        symlink(&real, &link).unwrap();

        let mut document = buffer("old");
        document.apply_text("new".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &link);

        let outcome = runtime().block_on(save_snapshot(&snapshot)).unwrap();
        assert!(matches!(outcome, SaveOutcome::Saved { .. }));
        assert_eq!(fs::read_to_string(&link).unwrap(), "new");
        assert_eq!(fs::read_to_string(&real).unwrap(), "old");
        assert!(
            !fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn stale_save_persists_old_revision_but_keeps_buffer_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();
        let mut document = buffer("old");
        let saved = document.apply_text("v1".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);
        document.apply_text("v2".to_owned()).unwrap();

        let outcome = runtime().block_on(save_snapshot(&snapshot)).unwrap();
        assert_eq!(
            outcome,
            SaveOutcome::Saved {
                revision: saved.get()
            }
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1");
        assert!(
            !document.confirm_saved(saved, "v1"),
            "a newer edit must stay dirty"
        );
        assert!(document.is_dirty());
    }

    #[test]
    fn a_save_above_the_readable_cap_is_refused_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();

        let oversized = "x".repeat(MAX_DOCUMENT_BYTES + 1);
        let mut document = buffer("old");
        document.apply_text(oversized).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);

        let result = runtime().block_on(save_snapshot(&snapshot));
        assert_eq!(
            result,
            Err(DocumentError::TooLarge {
                bytes: MAX_DOCUMENT_BYTES + 1,
                limit: MAX_DOCUMENT_BYTES,
            }),
            "a document that could never be reopened must not be written"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
    }

    #[test]
    fn a_save_exactly_at_the_readable_cap_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();

        let at_cap = "x".repeat(MAX_DOCUMENT_BYTES);
        let mut document = buffer("old");
        let revision = document.apply_text(at_cap.clone()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);

        let outcome = runtime().block_on(save_snapshot(&snapshot)).unwrap();
        assert_eq!(
            outcome,
            SaveOutcome::Saved {
                revision: revision.get()
            }
        );
        assert_eq!(fs::read_to_string(&path).unwrap().len(), MAX_DOCUMENT_BYTES);
        assert_eq!(fs::read_to_string(&path).unwrap(), at_cap);
    }

    #[test]
    fn relative_and_absolute_spellings_share_one_writer() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = dir.path().join("doc.md");
        fs::write(&absolute, "old").unwrap();
        let spelled = dir.path().join("./doc.md");

        let mut first = buffer("old");
        first.apply_text("A".to_owned()).unwrap();
        let mut second = buffer("old");
        second.apply_text("B".to_owned()).unwrap();
        let snap_a = DocumentSnapshot::capture(&first, &absolute);
        let snap_b = DocumentSnapshot::capture(&second, &spelled);

        let runtime = runtime();
        let (outcome_a, outcome_b) = runtime.block_on(async move {
            let handle_a = tokio::spawn(async move { save_snapshot(&snap_a).await });
            let handle_b = tokio::spawn(async move { save_snapshot(&snap_b).await });
            (handle_a.await.unwrap(), handle_b.await.unwrap())
        });
        let outcomes = [outcome_a.unwrap(), outcome_b.unwrap()];
        let saved = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SaveOutcome::Saved { .. }))
            .count();
        let conflicts = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SaveOutcome::Conflict))
            .count();
        assert_eq!(
            (saved, conflicts),
            (1, 1),
            "aliases of one file must share a writer lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn saving_preserves_existing_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let mut document = buffer("old");
        document.apply_text("new".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);
        runtime().block_on(save_snapshot(&snapshot)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn failed_write_leaves_original_intact_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        fs::write(&path, "old").unwrap();
        let mut document = buffer("old");
        document.apply_text("new".to_owned()).unwrap();
        let snapshot = DocumentSnapshot::capture(&document, &path);

        let original = fs::metadata(dir.path()).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_mode(0o500);
        fs::set_permissions(dir.path(), read_only).unwrap();
        let result = runtime().block_on(save_snapshot(&snapshot));
        fs::set_permissions(dir.path(), original).unwrap();

        if let Err(DocumentError::Io { .. }) = result {
            assert_eq!(fs::read_to_string(&path).unwrap(), "old");
            let leftovers = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".astrolith-save-")
                })
                .count();
            assert_eq!(leftovers, 0);
        }
    }
}
