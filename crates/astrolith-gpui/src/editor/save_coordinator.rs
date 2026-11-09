//! Autosave scheduling for retained documents.
//!
//! The coordinator owns *when* a save happens, never *how*: every request it
//! issues is executed by the guarded, conflict-aware atomic save in
//! `astrolith-document`. It keeps one state machine per canonical source:
//!
//! ```text
//! Clean -> Dirty(idle deadline) -> Saving(ticket) -> Clean
//!               |                       |
//!               +-> 10 s max deadline --+
//!               +-> Manual -------------+
//! Dirty + conflict -> Blocked
//! Saving + newer revision -> Dirty(new deadlines)
//! Saving + failure -> Dirty(error)
//! ```
//!
//! The effective idle delay is exactly 1, 2, 5 or 10 seconds, resolved from
//! preferences; every dirty run also checkpoints at [`MAX_CHECKPOINT`] while the
//! user keeps typing. One save is in flight per source: an edit during that save
//! keeps the buffer dirty and schedules a follow-up, and a manual request during
//! that save waits for it instead of running concurrently.
//!
//! All methods take the current [`Instant`] so policy is testable without a
//! clock; the workspace supplies `Instant::now()`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use astrolith_core::preferences::AutosaveDelay;

/// Longest uninterrupted dirty run before a checkpoint, even while typing.
pub(crate) const MAX_CHECKPOINT: Duration = Duration::from_secs(10);

/// Identity of one save attempt; completions are matched by ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SaveTicket {
    sequence: u64,
}

/// Why a save was requested; automatic requests stay silent on success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveKind {
    /// Idle deadline, maximum checkpoint, blur, or switch.
    Automatic,
    /// `Ctrl/Cmd+S` and the save half of a lifecycle operation.
    Manual,
}

/// One document save the workspace must run through the guarded path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveRequest {
    /// Absolute path of the canonical source.
    pub path: PathBuf,
    /// Ticket to report back on completion.
    pub ticket: SaveTicket,
    /// Whether this request came from the user's explicit save.
    pub kind: SaveKind,
}

/// The outcome of a guarded save, as the coordinator needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveResult {
    /// The revision whose complete source was persisted.
    Saved {
        /// Persisted content revision.
        revision: u64,
    },
    /// The file changed on disk; nothing was written.
    Conflict,
    /// The write failed; the buffer stays dirty.
    Failed {
        /// Human-readable failure reason.
        message: String,
    },
}

/// What a completion did to the state machine; `Stale` results are dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveCompletion {
    /// A completion that no longer matches the in-flight ticket or project.
    Stale,
    /// The save succeeded; `current` reports whether newer edits remain.
    Saved {
        /// Persisted content revision.
        revision: u64,
        /// Whether the persisted revision is still the newest content.
        current: bool,
        /// Whether a manual request queued behind this save is now satisfied.
        manual: bool,
    },
    /// The file changed on disk; automatic saving is blocked.
    Conflict,
    /// The write failed and the buffer stays dirty for a retry.
    Failed {
        /// Human-readable failure reason.
        message: String,
        /// Whether this is the first failure of its episode; later retries of the
        /// same episode report `false` so only one artifact is recorded.
        first: bool,
    },
}

/// Status shown for one document; automatic success is silent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveStatus {
    /// Persisted as of the newest revision.
    Saved,
    /// Newer content than the disk.
    Unsaved,
    /// A guarded save is in flight.
    Saving,
    /// An external change blocks automatic saving.
    Conflict,
    /// The last save failed; the buffer stays dirty.
    Failed(String),
}

#[derive(Clone, Debug)]
enum Phase {
    Clean,
    Dirty { last_edit: Instant, max: Instant },
    Conflict,
}

#[derive(Clone, Debug)]
struct DocumentSave {
    revision: u64,
    phase: Phase,
    in_flight: Option<SaveTicket>,
    /// A request that arrived while another save was in flight.
    pending: Option<SaveKind>,
    error: Option<String>,
    /// Set once a failure of the current episode has been recorded, so retries
    /// write no further artifacts until the document is clean again.
    failure_reported: bool,
}

impl DocumentSave {
    fn clean(revision: u64) -> Self {
        Self {
            revision,
            phase: Phase::Clean,
            in_flight: None,
            pending: None,
            error: None,
            failure_reported: false,
        }
    }

    fn start_run(&mut self, now: Instant) {
        // A saved run was checkpointed by the in-flight save, so newer edits
        // begin a fresh maximum window; typed-over deadlines keep their max.
        let max = if self.in_flight.is_some() {
            now + MAX_CHECKPOINT
        } else {
            match self.phase {
                Phase::Dirty { max, .. } => max,
                _ => now + MAX_CHECKPOINT,
            }
        };
        self.phase = Phase::Dirty {
            last_edit: now,
            max,
        };
        self.error = None;
    }
}

/// Autosave deadlines and per-source state for one open project.
pub(crate) struct SaveCoordinator {
    delay: AutosaveDelay,
    next_ticket: u64,
    documents: BTreeMap<PathBuf, DocumentSave>,
}

impl SaveCoordinator {
    /// Create a coordinator using `delay` as the idle autosave delay.
    pub(crate) fn new(delay: AutosaveDelay) -> Self {
        Self {
            delay,
            next_ticket: 0,
            documents: BTreeMap::new(),
        }
    }

    /// Return the resolved idle delay.
    #[cfg(test)]
    pub(crate) fn delay(&self) -> AutosaveDelay {
        self.delay
    }

    /// Adopt a new resolved delay; existing deadlines use it immediately.
    pub(crate) fn set_delay(&mut self, delay: AutosaveDelay) {
        self.delay = delay;
    }

    fn idle(&self) -> Duration {
        Duration::from_secs(self.delay.seconds())
    }

    fn issue(next: &mut u64) -> SaveTicket {
        *next = next.saturating_add(1);
        SaveTicket { sequence: *next }
    }

    /// Drop every document and ticket, for a project switch or teardown.
    pub(crate) fn clear(&mut self) {
        self.documents.clear();
    }

    /// Stop tracking one document (closed tab).
    pub(crate) fn forget(&mut self, path: &Path) {
        self.documents.remove(path);
    }

    /// Move a document's state after a rename or move.
    ///
    /// An in-flight save still targets the old path, so its entry is dropped and
    /// the completion arrives as a stale result instead of marking new content
    /// clean.
    pub(crate) fn retarget(&mut self, from: &Path, to: &Path) {
        if let Some(mut document) = self.documents.remove(from)
            && document.in_flight.is_none()
        {
            document.pending = None;
            self.documents.insert(to.to_path_buf(), document);
        }
    }

    /// Report whether any document is dirty, saving, blocked or failed.
    pub(crate) fn has_pending(&self) -> bool {
        self.documents
            .values()
            .any(|document| document.in_flight.is_some() || !matches!(document.phase, Phase::Clean))
    }

    /// Report whether a save is in flight for `path`.
    pub(crate) fn is_saving(&self, path: &Path) -> bool {
        self.documents
            .get(path)
            .is_some_and(|document| document.in_flight.is_some())
    }

    /// Report whether the document is still dirty (including a follow-up).
    pub(crate) fn is_dirty(&self, path: &Path) -> bool {
        self.documents
            .get(path)
            .is_some_and(|document| !matches!(document.phase, Phase::Clean))
    }

    /// Status for one document, or `None` when it is untracked.
    pub(crate) fn status(&self, path: &Path) -> Option<SaveStatus> {
        let document = self.documents.get(path)?;
        if document.in_flight.is_some() {
            return Some(SaveStatus::Saving);
        }
        Some(match &document.phase {
            Phase::Clean => SaveStatus::Saved,
            Phase::Conflict => SaveStatus::Conflict,
            Phase::Dirty { .. } => match &document.error {
                Some(message) => SaveStatus::Failed(message.clone()),
                None => SaveStatus::Unsaved,
            },
        })
    }

    /// Record the buffer state after any session change.
    ///
    /// Returns `true` when the deadlines changed and the workspace must re-arm
    /// its timer.
    pub(crate) fn observe(
        &mut self,
        path: &Path,
        revision: u64,
        dirty: bool,
        now: Instant,
    ) -> bool {
        let Some(document) = self.documents.get_mut(path) else {
            let mut document = DocumentSave::clean(revision);
            if dirty {
                document.start_run(now);
            }
            self.documents.insert(path.to_path_buf(), document);
            return dirty;
        };
        if revision > document.revision {
            document.revision = revision;
            if dirty {
                document.start_run(now);
            } else {
                document.phase = Phase::Clean;
                document.pending = None;
                document.error = None;
                document.failure_reported = false;
            }
            return true;
        }
        if dirty {
            if matches!(document.phase, Phase::Clean) && document.in_flight.is_none() {
                document.start_run(now);
                return true;
            }
            return false;
        }
        if document.in_flight.is_none() && !matches!(document.phase, Phase::Clean) {
            document.phase = Phase::Clean;
            document.pending = None;
            document.error = None;
            document.failure_reported = false;
            return true;
        }
        false
    }

    /// Earliest instant at which [`Self::due`] can produce a request.
    pub(crate) fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let idle = self.idle();
        self.documents
            .values()
            .filter_map(|document| {
                if document.in_flight.is_some() {
                    return None;
                }
                if document.pending.is_some() {
                    return Some(now);
                }
                match document.phase {
                    Phase::Dirty { last_edit, max } => Some(if now >= max {
                        now
                    } else {
                        (last_edit + idle).min(max).max(now)
                    }),
                    _ => None,
                }
            })
            .min()
    }

    /// Start every save whose idle or maximum deadline has passed.
    pub(crate) fn due(&mut self, now: Instant) -> Vec<SaveRequest> {
        let idle = self.idle();
        let ready: Vec<(PathBuf, SaveKind)> = self
            .documents
            .iter()
            .filter_map(|(path, document)| {
                if document.in_flight.is_some() {
                    return None;
                }
                if let Some(kind) = document.pending {
                    return Some((path.clone(), kind));
                }
                match document.phase {
                    Phase::Dirty { last_edit, max } => (now >= last_edit + idle || now >= max)
                        .then(|| (path.clone(), SaveKind::Automatic)),
                    _ => None,
                }
            })
            .collect();
        ready
            .into_iter()
            .map(|(path, kind)| {
                let ticket = Self::issue(&mut self.next_ticket);
                if let Some(document) = self.documents.get_mut(&path) {
                    document.in_flight = Some(ticket);
                    document.pending = None;
                }
                SaveRequest { path, ticket, kind }
            })
            .collect()
    }

    /// Request an immediate save, ignoring the idle deadline.
    ///
    /// A clean buffer is a no-op (`Ctrl/Cmd+S` on a saved document). When a save
    /// is already in flight the request is remembered and starts as soon as that
    /// save completes, so there is still one writer per source.
    pub(crate) fn request_now(
        &mut self,
        path: &Path,
        revision: u64,
        now: Instant,
        kind: SaveKind,
    ) -> Option<SaveRequest> {
        if !self.documents.contains_key(path) {
            self.documents
                .insert(path.to_path_buf(), DocumentSave::clean(revision));
        }
        let document = self
            .documents
            .get_mut(path)
            .expect("the document was just inserted");
        if revision > document.revision {
            document.revision = revision;
            if document.in_flight.is_none() {
                document.start_run(now);
            }
        }
        if document.in_flight.is_some() {
            document.pending = Some(kind);
            if matches!(document.phase, Phase::Clean) {
                document.start_run(now);
            }
            return None;
        }
        if matches!(document.phase, Phase::Clean) {
            return None;
        }
        let ticket = Self::issue(&mut self.next_ticket);
        document.in_flight = Some(ticket);
        document.pending = None;
        Some(SaveRequest {
            path: path.to_path_buf(),
            ticket,
            kind,
        })
    }

    /// Apply the guarded save result, dropping stale completions.
    pub(crate) fn complete(
        &mut self,
        path: &Path,
        ticket: SaveTicket,
        result: SaveResult,
        now: Instant,
    ) -> SaveCompletion {
        let Some(document) = self.documents.get_mut(path) else {
            return SaveCompletion::Stale;
        };
        if document.in_flight != Some(ticket) {
            return SaveCompletion::Stale;
        }
        document.in_flight = None;
        match result {
            SaveResult::Saved { revision } => {
                if revision >= document.revision {
                    document.revision = revision;
                    document.phase = Phase::Clean;
                    let manual = document.pending == Some(SaveKind::Manual);
                    document.pending = None;
                    document.error = None;
                    document.failure_reported = false;
                    SaveCompletion::Saved {
                        revision,
                        current: true,
                        manual,
                    }
                } else {
                    // Newer edits were typed while saving: never mark them clean.
                    if !matches!(document.phase, Phase::Dirty { .. }) {
                        document.start_run(now);
                    }
                    SaveCompletion::Saved {
                        revision,
                        current: false,
                        manual: false,
                    }
                }
            }
            SaveResult::Conflict => {
                document.phase = Phase::Conflict;
                document.pending = None;
                document.error = None;
                document.failure_reported = false;
                SaveCompletion::Conflict
            }
            SaveResult::Failed { message } => {
                // A retry gets a fresh run. Reusing the expired maximum (a
                // checkpoint-triggered save, or a failure after the run's 10 s)
                // would make `due` re-issue the save immediately and spin.
                document.phase = Phase::Dirty {
                    last_edit: now,
                    max: now + MAX_CHECKPOINT,
                };
                document.error = Some(message.clone());
                let first = !document.failure_reported;
                document.failure_reported = true;
                SaveCompletion::Failed { message, first }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH_A: &str = "/project/src/content/posts/a.md";
    const PATH_B: &str = "/project/src/content/posts/b.md";

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    fn dirty(coordinator: &mut SaveCoordinator, path: &str, revision: u64, now: Instant) {
        assert!(coordinator.observe(Path::new(path), revision, true, now));
    }

    #[test]
    fn the_idle_delay_is_exactly_each_supported_preference() {
        for delay in [
            AutosaveDelay::One,
            AutosaveDelay::Two,
            AutosaveDelay::Five,
            AutosaveDelay::Ten,
        ] {
            let base = Instant::now();
            let mut coordinator = SaveCoordinator::new(delay);
            dirty(&mut coordinator, PATH_A, 1, base);
            let millis = delay.seconds() * 1000;
            assert!(
                coordinator.due(at(base, millis - 1)).is_empty(),
                "{delay:?} must not save before the idle deadline"
            );
            assert_eq!(
                coordinator.due(at(base, millis)).len(),
                1,
                "{delay:?} must save at the idle deadline"
            );
        }
    }

    #[test]
    fn continuous_typing_checkpoints_at_the_ten_second_maximum() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Two);
        let mut revision = 0;
        dirty(&mut coordinator, PATH_A, revision, base);
        for second in 1..10 {
            revision += 1;
            let now = at(base, second * 1000);
            coordinator.observe(Path::new(PATH_A), revision, true, now);
            assert!(
                coordinator.due(now).is_empty(),
                "the idle deadline moved with the last keystroke"
            );
        }
        // The last keystroke moved the idle deadline to 12 s, but the run began
        // at 0 s, so the maximum checkpoint still fires at 10 s.
        assert_eq!(
            coordinator.next_deadline(at(base, 9_000)),
            Some(at(base, 10_000))
        );
        assert_eq!(coordinator.due(at(base, 9_999)).len(), 0);
        assert_eq!(coordinator.due(at(base, 10_000)).len(), 1);
    }

    #[test]
    fn a_maximum_checkpoint_starts_a_fresh_run_for_later_typing() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Two);
        dirty(&mut coordinator, PATH_A, 1, base);
        let checkpoint = coordinator.due(at(base, 10_000)).pop().unwrap();
        coordinator.observe(Path::new(PATH_A), 2, true, at(base, 10_500));
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                checkpoint.ticket,
                SaveResult::Saved { revision: 1 },
                at(base, 11_000)
            ),
            SaveCompletion::Saved {
                revision: 1,
                current: false,
                manual: false,
            }
        );
        // The new run owns its own idle deadline instead of inheriting the
        // expired maximum from the checkpointed one.
        assert!(coordinator.due(at(base, 12_499)).is_empty());
        assert_eq!(coordinator.due(at(base, 12_500)).len(), 1);
    }

    #[test]
    fn one_save_is_in_flight_per_source() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        let request = coordinator.due(at(base, 1000)).pop().unwrap();
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Saving)
        );
        coordinator.observe(Path::new(PATH_A), 2, true, at(base, 1500));
        assert!(
            coordinator.due(at(base, 60_000)).is_empty(),
            "a second save must wait for the first"
        );
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                request.ticket,
                SaveResult::Saved { revision: 1 },
                at(base, 2000)
            ),
            SaveCompletion::Saved {
                revision: 1,
                current: false,
                manual: false,
            }
        );
        let follow_up = coordinator.due(at(base, 2500)).pop().unwrap();
        assert_eq!(follow_up.kind, SaveKind::Automatic);
        assert!(follow_up.ticket > request.ticket);
        assert_eq!(coordinator.due(at(base, 2600)).len(), 0);
    }

    #[test]
    fn an_edit_during_a_save_is_never_marked_clean() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Two);
        dirty(&mut coordinator, PATH_A, 1, base);
        let request = coordinator.due(at(base, 2000)).pop().unwrap();
        coordinator.observe(Path::new(PATH_A), 2, true, at(base, 2500));
        coordinator.complete(
            Path::new(PATH_A),
            request.ticket,
            SaveResult::Saved { revision: 1 },
            at(base, 3000),
        );
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Unsaved)
        );
        assert!(coordinator.is_dirty(Path::new(PATH_A)));
        // The newer edit has its own idle deadline, measured from the edit.
        assert!(coordinator.due(at(base, 4499)).is_empty());
        assert_eq!(coordinator.due(at(base, 4500)).len(), 1);
    }

    #[test]
    fn a_conflict_blocks_automatic_saves_until_a_manual_retry() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        let request = coordinator.due(at(base, 1000)).pop().unwrap();
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                request.ticket,
                SaveResult::Conflict,
                at(base, 1500)
            ),
            SaveCompletion::Conflict
        );
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Conflict)
        );
        assert!(
            coordinator.due(at(base, 90_000)).is_empty(),
            "automatic retries cannot resolve an external change"
        );
        assert!(coordinator.next_deadline(at(base, 90_000)).is_none());
        let retry = coordinator
            .request_now(Path::new(PATH_A), 1, at(base, 90_000), SaveKind::Manual)
            .expect("a manual save retries after a conflict");
        assert_eq!(retry.kind, SaveKind::Manual);
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                retry.ticket,
                SaveResult::Saved { revision: 1 },
                at(base, 91_000)
            ),
            SaveCompletion::Saved {
                revision: 1,
                current: true,
                manual: false,
            }
        );
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Saved)
        );
    }

    #[test]
    fn a_failure_keeps_the_buffer_dirty_and_retries() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        let request = coordinator.due(at(base, 1000)).pop().unwrap();
        let completion = coordinator.complete(
            Path::new(PATH_A),
            request.ticket,
            SaveResult::Failed {
                message: "cannot write".into(),
            },
            at(base, 2000),
        );
        assert_eq!(
            completion,
            SaveCompletion::Failed {
                message: "cannot write".into(),
                first: true
            }
        );
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Failed("cannot write".into()))
        );
        assert!(coordinator.due(at(base, 2999)).is_empty());
        assert_eq!(
            coordinator.due(at(base, 3000)).len(),
            1,
            "a failed save retries after the idle delay"
        );
    }

    #[test]
    fn a_failure_after_the_maximum_rearms_the_idle_delay_instead_of_spinning() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Two);
        dirty(&mut coordinator, PATH_A, 1, base);
        // The maximum checkpoint fires at 10 s, after the run's maximum instant.
        let checkpoint = coordinator.due(at(base, 10_000)).pop().unwrap();
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                checkpoint.ticket,
                SaveResult::Failed {
                    message: "read-only".into(),
                },
                at(base, 10_000)
            ),
            SaveCompletion::Failed {
                message: "read-only".into(),
                first: true
            }
        );
        assert!(
            coordinator.due(at(base, 10_000)).is_empty(),
            "a retry never reuses the expired maximum"
        );
        assert_eq!(
            coordinator.next_deadline(at(base, 10_000)),
            Some(at(base, 12_000)),
            "the retry waits one fresh idle delay"
        );
    }

    #[test]
    fn only_the_first_failure_of_an_episode_is_reported() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        let first = coordinator.due(at(base, 1000)).pop().unwrap();
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                first.ticket,
                SaveResult::Failed {
                    message: "cannot write".into(),
                },
                at(base, 1500)
            ),
            SaveCompletion::Failed {
                message: "cannot write".into(),
                first: true
            }
        );
        let retry = coordinator.due(at(base, 2500)).pop().unwrap();
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                retry.ticket,
                SaveResult::Failed {
                    message: "cannot write".into(),
                },
                at(base, 3000)
            ),
            SaveCompletion::Failed {
                message: "cannot write".into(),
                first: false
            },
            "a retry never writes a second artifact for the same episode"
        );
        // A successful save ends the episode; the next failure is recorded again.
        let retry = coordinator.due(at(base, 4000)).pop().unwrap();
        coordinator.complete(
            Path::new(PATH_A),
            retry.ticket,
            SaveResult::Saved { revision: 1 },
            at(base, 4500),
        );
        dirty(&mut coordinator, PATH_A, 2, at(base, 5000));
        let next = coordinator.due(at(base, 6000)).pop().unwrap();
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                next.ticket,
                SaveResult::Failed {
                    message: "cannot write".into(),
                },
                at(base, 6500)
            ),
            SaveCompletion::Failed {
                message: "cannot write".into(),
                first: true
            }
        );
    }

    #[test]
    fn a_queued_manual_request_is_satisfied_by_a_current_completion() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        let automatic = coordinator.due(at(base, 1000)).pop().unwrap();
        assert_eq!(automatic.kind, SaveKind::Automatic);
        assert!(
            coordinator
                .request_now(Path::new(PATH_A), 1, at(base, 1100), SaveKind::Manual)
                .is_none(),
            "the manual request queues behind the automatic save"
        );
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                automatic.ticket,
                SaveResult::Saved { revision: 1 },
                at(base, 1200)
            ),
            SaveCompletion::Saved {
                revision: 1,
                current: true,
                manual: true
            },
            "the queued manual save is reported as satisfied"
        );
    }

    #[test]
    fn a_manual_request_during_a_save_waits_for_the_follow_up() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Five);
        dirty(&mut coordinator, PATH_A, 1, base);
        let first = coordinator.due(at(base, 5000)).pop().unwrap();
        coordinator.observe(Path::new(PATH_A), 2, true, at(base, 5100));
        assert!(
            coordinator
                .request_now(Path::new(PATH_A), 2, at(base, 5200), SaveKind::Manual)
                .is_none(),
            "a second writer is never started"
        );
        coordinator.complete(
            Path::new(PATH_A),
            first.ticket,
            SaveResult::Saved { revision: 1 },
            at(base, 5300),
        );
        let follow_up = coordinator
            .due(at(base, 5300))
            .pop()
            .expect("the manual request runs immediately after the save");
        assert_eq!(follow_up.kind, SaveKind::Manual);
    }

    #[test]
    fn saving_on_a_clean_buffer_does_nothing() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Two);
        coordinator.observe(Path::new(PATH_A), 3, false, base);
        assert!(
            coordinator
                .request_now(Path::new(PATH_A), 3, base, SaveKind::Manual)
                .is_none()
        );
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Saved)
        );
    }

    #[test]
    fn an_immediate_request_ignores_the_idle_deadline() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Ten);
        dirty(&mut coordinator, PATH_A, 1, base);
        let request = coordinator
            .request_now(Path::new(PATH_A), 1, at(base, 1), SaveKind::Automatic)
            .expect("a blur saves immediately");
        assert_eq!(request.path, Path::new(PATH_A));
    }

    #[test]
    fn stale_and_foreign_completions_are_dropped() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        dirty(&mut coordinator, PATH_B, 7, base);
        let request = coordinator
            .due(at(base, 1000))
            .into_iter()
            .find(|request| request.path == Path::new(PATH_A))
            .expect("the dirty document is due");
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                request.ticket,
                SaveResult::Saved { revision: 1 },
                at(base, 1100)
            ),
            SaveCompletion::Saved {
                revision: 1,
                current: true,
                manual: false,
            }
        );
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                request.ticket,
                SaveResult::Saved { revision: 1 },
                at(base, 1200)
            ),
            SaveCompletion::Stale,
            "a completion is applied once"
        );
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                request.ticket,
                SaveResult::Conflict,
                at(base, 1300)
            ),
            SaveCompletion::Stale
        );
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Saved),
            "a stale result does not change the state machine"
        );
    }

    #[test]
    fn a_new_project_generation_drops_old_completions() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        let request = coordinator.due(at(base, 1000)).pop().unwrap();
        coordinator.clear();
        assert_eq!(
            coordinator.complete(
                Path::new(PATH_A),
                request.ticket,
                SaveResult::Saved { revision: 1 },
                at(base, 2000)
            ),
            SaveCompletion::Stale
        );
        assert_eq!(coordinator.status(Path::new(PATH_A)), None);
    }

    #[test]
    fn status_reports_unsaved_saving_and_saved() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        coordinator.observe(Path::new(PATH_A), 0, false, base);
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Saved)
        );
        dirty(&mut coordinator, PATH_A, 1, base);
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Unsaved)
        );
        let request = coordinator.due(at(base, 1000)).pop().unwrap();
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Saving)
        );
        coordinator.complete(
            Path::new(PATH_A),
            request.ticket,
            SaveResult::Saved { revision: 1 },
            at(base, 1500),
        );
        assert_eq!(
            coordinator.status(Path::new(PATH_A)),
            Some(SaveStatus::Saved)
        );
    }

    #[test]
    fn docs_save_independently_and_in_path_order() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Two);
        dirty(&mut coordinator, PATH_B, 1, base);
        dirty(&mut coordinator, PATH_A, 1, base);
        let requests = coordinator.due(at(base, 2000));
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, Path::new(PATH_A));
        assert_eq!(requests[1].path, Path::new(PATH_B));
        assert!(requests[0].ticket < requests[1].ticket);
        assert_eq!(coordinator.next_deadline(at(base, 2000)), None);
    }

    #[test]
    fn the_next_deadline_tracks_the_earliest_document() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Two);
        dirty(&mut coordinator, PATH_A, 1, base);
        dirty(&mut coordinator, PATH_B, 1, at(base, 500));
        assert_eq!(coordinator.next_deadline(base), Some(at(base, 2000)));
        assert_eq!(
            coordinator.next_deadline(at(base, 600)),
            Some(at(base, 2000))
        );
    }

    #[test]
    fn changing_the_delay_moves_existing_deadlines() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::Ten);
        dirty(&mut coordinator, PATH_A, 1, base);
        assert!(coordinator.due(at(base, 1000)).is_empty());
        coordinator.set_delay(AutosaveDelay::One);
        assert_eq!(coordinator.due(at(base, 1000)).len(), 1);
    }

    #[test]
    fn forgetting_a_closed_document_drops_its_pending_work() {
        let base = Instant::now();
        let mut coordinator = SaveCoordinator::new(AutosaveDelay::One);
        dirty(&mut coordinator, PATH_A, 1, base);
        coordinator.forget(Path::new(PATH_A));
        assert!(!coordinator.has_pending());
        assert!(coordinator.due(at(base, 60_000)).is_empty());
    }
}
