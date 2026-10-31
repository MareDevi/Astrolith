//! Bounded filesystem watcher implementation.

use astrolith_astro::ProjectManifest;
use notify::{
    ErrorKind as NotifyErrorKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::ModifyKind,
};
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel},
    },
    time::{Duration, Instant},
};

use crate::WorkspaceError;

/// Capacity of the callback-to-poller event channel.
pub const WATCH_CHANNEL_CAPACITY: usize = 64;
/// Quiet period used to merge a filesystem event burst.
pub const WATCH_MERGE_WINDOW: Duration = Duration::from_millis(100);
/// Maximum unique directories registered by a project-scoped watcher.
pub const MAX_PROJECT_WATCH_PATHS: usize = 100_000;
/// Maximum directory depth below a project collection base.
pub const MAX_PROJECT_WATCH_DEPTH: usize = 64;
/// Maximum directory entries inspected while building project registrations.
pub const MAX_PROJECT_WATCH_ENTRIES: usize = 100_000;

/// A nonblocking result from one watcher poll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchBatch {
    /// At least one relevant source-tree event was observed.
    pub changed: bool,
    /// Package or Astro content configuration changed and discovery should run again.
    pub config_changed: bool,
    /// Changes may have been lost and the project needs a full rescan.
    pub needs_rescan: bool,
}

impl WatchBatch {
    fn observe(&mut self, event: &Event, root: &Path) {
        let relevant_changes = classify_event(event, root);
        self.changed |= relevant_changes.changed;
        self.config_changed |= relevant_changes.config_changed;
        self.needs_rescan |= event.need_rescan();
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct EventRelevance {
    changed: bool,
    config_changed: bool,
}

#[derive(Default)]
struct PendingBatch {
    last_seen: Option<Instant>,
    batch: WatchBatch,
}

struct WatchState {
    watcher: RecommendedWatcher,
    registrations: BTreeMap<PathBuf, RecursiveMode>,
}

/// A retained bounded watcher for a canonical project root.
///
/// Dropping this value stops the native watcher. [`Self::drain`] never waits for the
/// filesystem callback or another poller and returns no work when either lock is contended.
pub struct CollectionWatcher {
    root: PathBuf,
    watch_state: Mutex<WatchState>,
    _sender: SyncSender<notify::Result<Event>>,
    receiver: Mutex<Receiver<notify::Result<Event>>>,
    pending: Mutex<PendingBatch>,
    overflow: Arc<AtomicBool>,
}

impl std::fmt::Debug for CollectionWatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CollectionWatcher")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl CollectionWatcher {
    /// Start recursive observation of one directory.
    ///
    /// This keeps the original test/simple API. Production project use should call
    /// [`Self::for_project`] so dependency and metadata trees are not recursively watched.
    pub fn new(root: &Path) -> Result<Self, WorkspaceError> {
        let root = canonical_directory(root)?;
        let registrations = BTreeMap::from([(root.clone(), RecursiveMode::Recursive)]);
        Self::start(root, registrations)
    }

    /// Start project-scoped observation before the initial collection scan.
    ///
    /// Every allowed directory needed by the manifest is watched non-recursively. Directory
    /// walks skip dot directories, `node_modules`, and `.git`, stop at depth 64, inspect at
    /// most 100,000 directory entries, and register at most 100,000 paths. A project-root
    /// collection base is supported through the same selective bounded walk. This method
    /// performs filesystem and native watch I/O and must run on the application's blocking
    /// lane.
    pub fn for_project(project: &ProjectManifest) -> Result<Self, WorkspaceError> {
        let (root, registrations) = project_registrations(project)?;
        Self::start(root, registrations)
    }

    /// Replace the current registration set using a newly rediscovered manifest.
    ///
    /// Refresh keeps the existing bounded event channel and merge state. It is intended for
    /// the blocking lane after a directory/config change has caused discovery to run again.
    /// A watcher cannot be repointed at a different project root.
    pub fn refresh(&self, project: &ProjectManifest) -> Result<(), WorkspaceError> {
        let (root, desired) = project_registrations(project)?;
        if root != self.root {
            return Err(WorkspaceError::Model(format!(
                "cannot refresh watcher from {} to a different project root {}",
                self.root.display(),
                root.display()
            )));
        }

        let mut state = self.watch_state.lock().map_err(|_| {
            WorkspaceError::Model("collection watcher registration lock is poisoned".into())
        })?;
        tracing::debug!(
            root = %self.root.display(),
            desired_registrations = desired.len(),
            current_registrations = state.registrations.len(),
            "refreshing collection watcher registrations"
        );
        let stale = state
            .registrations
            .iter()
            .filter(|(path, mode)| desired.get(*path) != Some(*mode))
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        for path in stale {
            match state.watcher.unwatch(&path) {
                Ok(()) => {}
                Err(error) if matches!(error.kind, NotifyErrorKind::WatchNotFound) => {}
                Err(source) => return Err(watch_io_error(path, source)),
            }
            state.registrations.remove(&path);
        }

        for (path, mode) in desired {
            if state.registrations.contains_key(&path) {
                continue;
            }
            state
                .watcher
                .watch(&path, mode)
                .map_err(|source| watch_io_error(path.clone(), source))?;
            state.registrations.insert(path, mode);
        }
        Ok(())
    }

    fn start(
        root: PathBuf,
        registrations: BTreeMap<PathBuf, RecursiveMode>,
    ) -> Result<Self, WorkspaceError> {
        let overflow = Arc::new(AtomicBool::new(false));
        let callback_flag = Arc::clone(&overflow);
        let (sender, receiver) = sync_channel(WATCH_CHANNEL_CAPACITY);
        let callback_sender = sender.clone();
        let callback_root = root.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    let relevance = classify_event(&event, &callback_root);
                    if !event.need_rescan() && !relevance.changed && !relevance.config_changed {
                        return;
                    }
                    if event.need_rescan() {
                        callback_flag.store(true, Ordering::Release);
                    }
                    if let Err(TrySendError::Full(_)) = callback_sender.try_send(Ok(event)) {
                        callback_flag.store(true, Ordering::Release);
                    }
                }
                Err(error) => {
                    callback_flag.store(true, Ordering::Release);
                    tracing::warn!(%error, "filesystem watcher reported an error");
                    let _ = callback_sender.try_send(Err(error));
                }
            })
            .map_err(|source| watch_io_error(root.clone(), source))?;
        for (path, mode) in &registrations {
            watcher
                .watch(path, *mode)
                .map_err(|source| watch_io_error(path.clone(), source))?;
        }
        tracing::debug!(
            root = %root.display(),
            registrations = registrations.len(),
            "collection watcher started"
        );

        Ok(Self {
            root,
            watch_state: Mutex::new(WatchState {
                watcher,
                registrations,
            }),
            _sender: sender,
            receiver: Mutex::new(receiver),
            pending: Mutex::new(PendingBatch::default()),
            overflow,
        })
    }

    /// Drain only events that have been quiet for [`WATCH_MERGE_WINDOW`].
    ///
    /// The call is nonblocking even if another poller currently owns either lock. Channel
    /// overflow and watcher errors set [`WatchBatch::needs_rescan`].
    pub fn drain(&self) -> WatchBatch {
        let Some(receiver) = try_lock(&self.receiver) else {
            return WatchBatch::default();
        };
        let Some(mut pending) = try_lock(&self.pending) else {
            return WatchBatch::default();
        };

        let mut saw_input = false;
        for _ in 0..WATCH_CHANNEL_CAPACITY {
            match receiver.try_recv() {
                Ok(Ok(event)) => {
                    saw_input = true;
                    pending.batch.observe(&event, &self.root);
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    saw_input = true;
                    pending.batch.needs_rescan = true;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if self.overflow.swap(false, Ordering::AcqRel) {
            saw_input = true;
            pending.batch.needs_rescan = true;
        }
        if saw_input {
            pending.last_seen = Some(Instant::now());
        }

        let ready = pending
            .last_seen
            .is_some_and(|last| last.elapsed() >= WATCH_MERGE_WINDOW);
        if !ready {
            return WatchBatch::default();
        }
        let batch = pending.batch;
        if batch.changed || batch.needs_rescan {
            tracing::trace!(
                changed = batch.changed,
                needs_rescan = batch.needs_rescan,
                "drained a merged watcher batch"
            );
        }
        *pending = PendingBatch::default();
        batch
    }

    #[cfg(test)]
    fn registration_targets(&self) -> Vec<(PathBuf, RecursiveMode)> {
        let state = self
            .watch_state
            .lock()
            .expect("watcher state lock should not be poisoned");
        state
            .registrations
            .iter()
            .map(|(path, mode)| (path.clone(), *mode))
            .collect()
    }
}

fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    mutex.try_lock().ok()
}

fn project_registrations(
    project: &ProjectManifest,
) -> Result<(PathBuf, BTreeMap<PathBuf, RecursiveMode>), WorkspaceError> {
    let root = canonical_directory(project.root())?;
    let mut planner = ProjectRegistrationPlanner::new(&root);
    planner.add_directory(root.clone())?;
    let mut collection_bases = Vec::with_capacity(project.collections().len());
    let mut config_parents = Vec::with_capacity(project.collections().len());
    for descriptor in project.collections() {
        let base = canonical_directory(descriptor.base())?;
        if !base.starts_with(&root) {
            return Err(WorkspaceError::BaseOutsideProject(base));
        }
        let relative_base = base
            .strip_prefix(&root)
            .map_err(|_| WorkspaceError::BaseOutsideProject(base.clone()))?;
        if path_contains_ignored_component(relative_base) {
            return Err(WorkspaceError::Model(format!(
                "collection base {} is inside an ignored dependency or metadata tree",
                base.display()
            )));
        }
        collection_bases.push(base);

        let config = canonical_regular_file(descriptor.config())?;
        if !config.starts_with(&root) {
            return Err(WorkspaceError::Model(format!(
                "collection configuration {} is outside project root {}",
                config.display(),
                root.display()
            )));
        }
        let relative_config = config.strip_prefix(&root).map_err(|_| {
            WorkspaceError::Model(format!(
                "collection configuration {} is outside project root {}",
                config.display(),
                root.display()
            ))
        })?;
        if path_contains_ignored_component(relative_config) {
            return Err(WorkspaceError::Model(format!(
                "collection configuration {} is inside an ignored dependency or metadata tree",
                config.display()
            )));
        }
        let parent = config.parent().ok_or_else(|| {
            WorkspaceError::Model(format!(
                "collection configuration {} has no parent directory",
                config.display()
            ))
        })?;
        config_parents.push(parent.to_path_buf());
    }

    collection_bases.sort_unstable();
    for base in collection_bases {
        planner.add_tree(&base)?;
    }
    for parent in config_parents {
        planner.add_directory(parent)?;
    }

    let registrations = planner.finish();
    Ok((root, registrations))
}

struct ProjectRegistrationPlanner<'a> {
    root: &'a Path,
    registrations: BTreeMap<PathBuf, RecursiveMode>,
    visited_directories: HashSet<PathBuf>,
    inspected_entries: usize,
}

impl<'a> ProjectRegistrationPlanner<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            registrations: BTreeMap::new(),
            visited_directories: HashSet::new(),
            inspected_entries: 0,
        }
    }

    fn add_directory(&mut self, path: PathBuf) -> Result<(), WorkspaceError> {
        add_registration(&mut self.registrations, path, RecursiveMode::NonRecursive)
    }

    fn add_tree(&mut self, base: &Path) -> Result<(), WorkspaceError> {
        let mut pending = vec![(base.to_path_buf(), 0_usize)];
        while let Some((directory, depth)) = pending.pop() {
            if depth > MAX_PROJECT_WATCH_DEPTH {
                return Err(WorkspaceError::DepthLimit {
                    path: directory,
                    max: MAX_PROJECT_WATCH_DEPTH,
                });
            }
            if !self.visited_directories.insert(directory.clone()) {
                continue;
            }
            self.add_directory(directory.clone())?;

            let entries = fs::read_dir(&directory).map_err(|source| WorkspaceError::Io {
                path: directory.clone(),
                source,
            })?;
            let mut child_directories = Vec::new();
            for entry in entries {
                if self.inspected_entries >= MAX_PROJECT_WATCH_ENTRIES {
                    return Err(WorkspaceError::TraversalLimit {
                        path: directory,
                        max: MAX_PROJECT_WATCH_ENTRIES,
                    });
                }
                self.inspected_entries += 1;
                let entry = entry.map_err(|source| WorkspaceError::Io {
                    path: directory.clone(),
                    source,
                })?;
                if is_ignored_name(&entry.file_name()) {
                    continue;
                }
                let path = entry.path();
                let file_type = entry.file_type().map_err(|source| WorkspaceError::Io {
                    path: path.clone(),
                    source,
                })?;
                if file_type.is_symlink() {
                    return Err(WorkspaceError::Symlink(path));
                }
                if file_type.is_dir() {
                    if depth >= MAX_PROJECT_WATCH_DEPTH {
                        return Err(WorkspaceError::DepthLimit {
                            path,
                            max: MAX_PROJECT_WATCH_DEPTH,
                        });
                    }
                    child_directories.push((path, depth + 1));
                }
            }
            child_directories.sort_unstable_by(|left, right| right.0.cmp(&left.0));
            pending.extend(child_directories);
        }

        debug_assert!(base.starts_with(self.root));
        Ok(())
    }

    fn finish(self) -> BTreeMap<PathBuf, RecursiveMode> {
        self.registrations
    }
}

fn add_registration(
    registrations: &mut BTreeMap<PathBuf, RecursiveMode>,
    path: PathBuf,
    mode: RecursiveMode,
) -> Result<(), WorkspaceError> {
    if let Some(existing) = registrations.get_mut(&path) {
        if mode == RecursiveMode::Recursive {
            *existing = mode;
        }
        return Ok(());
    }
    if registrations.len() >= MAX_PROJECT_WATCH_PATHS {
        return Err(WorkspaceError::Model(format!(
            "project watcher requires more than {MAX_PROJECT_WATCH_PATHS} registration paths"
        )));
    }
    registrations.insert(path, mode);
    Ok(())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !canonical.is_dir() {
        return Err(WorkspaceError::Io {
            path: canonical,
            source: io::Error::new(io::ErrorKind::InvalidInput, "expected a directory"),
        });
    }
    Ok(canonical)
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !canonical.is_file() {
        return Err(WorkspaceError::Io {
            path: canonical,
            source: io::Error::new(io::ErrorKind::InvalidInput, "expected a regular file"),
        });
    }
    Ok(canonical)
}

fn watch_io_error(path: PathBuf, source: notify::Error) -> WorkspaceError {
    WorkspaceError::Io {
        path,
        source: io::Error::other(source),
    }
}

fn classify_event(event: &Event, root: &Path) -> EventRelevance {
    if event.need_rescan() {
        return EventRelevance {
            changed: true,
            config_changed: false,
        };
    }
    let changed = match event.kind {
        EventKind::Access(_) | EventKind::Other => false,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Create(_)
        | EventKind::Remove(_)
        | EventKind::Modify(ModifyKind::Any | ModifyKind::Name(_) | ModifyKind::Other)
        | EventKind::Any => event.paths.iter().any(|path| path_is_relevant(path, root)),
        EventKind::Modify(ModifyKind::Data(_)) => event
            .paths
            .iter()
            .any(|path| path_is_index_relevant(path, root)),
    };
    EventRelevance {
        changed,
        config_changed: changed && event.paths.iter().any(|path| is_discovery_config(path)),
    }
}

fn path_is_relevant(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    !path_contains_ignored_component(relative)
}

fn path_contains_ignored_component(path: &Path) -> bool {
    path.components().any(|component| {
        let std::path::Component::Normal(segment) = component else {
            return false;
        };
        is_ignored_name(segment)
    })
}

fn is_ignored_name(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') || name == "node_modules" || name == ".git"
}

fn path_is_index_relevant(path: &Path, root: &Path) -> bool {
    path_is_relevant(path, root)
        && matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("md" | "mdx" | "js" | "mjs" | "ts" | "json")
        )
}

fn is_discovery_config(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == "package.json"
        || matches!(
            name,
            "astro.config.js"
                | "astro.config.mjs"
                | "astro.config.ts"
                | "content.config.js"
                | "content.config.mjs"
                | "content.config.ts"
                | "config.js"
                | "config.mjs"
                | "config.ts"
        )
}

#[cfg(test)]
mod tests {
    use super::{
        CollectionWatcher, MAX_PROJECT_WATCH_PATHS, WATCH_CHANNEL_CAPACITY, WATCH_MERGE_WINDOW,
        add_registration, classify_event,
    };
    use astrolith_astro::{
        Cancellation, DiscoveryOutcome, DiscoveryRequest, ProjectManifest, inspect_project,
    };
    use astrolith_core::ProjectId;
    use notify::{
        Event, EventKind, RecursiveMode,
        event::{CreateKind, DataChange, ModifyKind},
    };
    use std::{collections::BTreeMap, fs, path::PathBuf, sync::mpsc::TrySendError, thread};

    #[test]
    fn saturated_channel_is_deterministically_marked_for_rescan() {
        let temp = tempfile::tempdir().unwrap();
        let watcher = CollectionWatcher::new(temp.path()).unwrap();
        let event = Event::new(EventKind::Create(CreateKind::File))
            .add_path(PathBuf::from(temp.path()).join("entry.md"));
        for _ in 0..WATCH_CHANNEL_CAPACITY {
            watcher._sender.try_send(Ok(event.clone())).unwrap();
        }
        assert!(matches!(
            watcher._sender.try_send(Ok(event)),
            Err(TrySendError::Full(_))
        ));
        watcher
            .overflow
            .store(true, std::sync::atomic::Ordering::Release);

        assert_eq!(watcher.drain(), super::WatchBatch::default());
        thread::sleep(WATCH_MERGE_WINDOW + std::time::Duration::from_millis(20));
        let batch = watcher.drain();
        assert!(batch.changed);
        assert!(batch.needs_rescan);
    }

    #[test]
    fn data_modification_classification_covers_discovery_and_editorial_sources() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let relevant_paths = [
            "content.config.ts",
            "package.json",
            "src/helpers/collections.ts",
            "src/content/entry.md",
            "src/content/entry.mdx",
        ];
        for relative in relevant_paths {
            let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                .add_path(root.join(relative));
            assert!(
                classify_event(&event, root).changed,
                "expected {relative} to invalidate discovery or indexing"
            );
        }

        for relative in ["node_modules/astro/index.js", ".git/index", "notes.txt"] {
            let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                .add_path(root.join(relative));
            assert!(
                !classify_event(&event, root).changed,
                "expected ignored or unsupported data in {relative} to be dropped"
            );
        }
    }

    fn fixture_project() -> (tempfile::TempDir, ProjectManifest) {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("node_modules/astro")).unwrap();
        fs::create_dir_all(temp.path().join("src/posts")).unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"astro":"5.18.2"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("node_modules/astro/package.json"),
            r#"{"version":"5.18.2"}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("src/content.config.ts"),
            r#"import {defineCollection} from 'astro:content';
import {glob} from 'astro/loaders';
const docs = defineCollection({loader:glob({base:'src/posts',pattern:'**/*.md'})});
export const collections = {docs};
"#,
        )
        .unwrap();
        let request = DiscoveryRequest::new(
            ProjectId::new("watcher-unit").unwrap(),
            1,
            temp.path().to_path_buf(),
            Cancellation::default(),
        );
        let DiscoveryOutcome::Ready(manifest) = inspect_project(&request).unwrap() else {
            panic!("fixture discovery must succeed");
        };
        (temp, manifest)
    }

    #[test]
    fn project_registration_targets_are_narrow_and_exclude_dependency_trees() {
        let (temp, manifest) = fixture_project();
        let watcher = CollectionWatcher::for_project(&manifest).unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        assert_eq!(
            watcher.registration_targets(),
            vec![
                (root.clone(), RecursiveMode::NonRecursive),
                (root.join("src"), RecursiveMode::NonRecursive),
                (root.join("src/posts"), RecursiveMode::NonRecursive),
            ]
        );
        assert!(watcher.registration_targets().iter().all(|(path, _)| {
            !path.components().any(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|segment| segment == "node_modules" || segment == ".git")
            })
        }));
    }

    #[test]
    fn project_root_collection_base_is_walked_selectively() {
        let (temp, _manifest) = fixture_project();
        fs::write(
            temp.path().join("src/content.config.ts"),
            r#"import {defineCollection} from 'astro:content';
import {glob} from 'astro/loaders';
const docs = defineCollection({loader:glob({base:'.',pattern:'**/*.md'})});
export const collections = {docs};
"#,
        )
        .unwrap();
        let request = DiscoveryRequest::new(
            ProjectId::new("watcher-root-base").unwrap(),
            2,
            temp.path().to_path_buf(),
            Cancellation::default(),
        );
        let DiscoveryOutcome::Ready(manifest) = inspect_project(&request).unwrap() else {
            panic!("root-base fixture discovery must succeed");
        };

        let watcher = CollectionWatcher::for_project(&manifest).unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        assert_eq!(
            watcher.registration_targets(),
            vec![
                (root.clone(), RecursiveMode::NonRecursive),
                (root.join("src"), RecursiveMode::NonRecursive),
                (root.join("src/posts"), RecursiveMode::NonRecursive),
            ]
        );
        assert!(watcher.registration_targets().iter().all(|(path, mode)| {
            *mode == RecursiveMode::NonRecursive
                && !path.starts_with(root.join("node_modules"))
                && !path.starts_with(root.join(".git"))
        }));
    }

    #[test]
    fn project_registration_targets_are_bounded() {
        let root = PathBuf::from("/project");
        let mut registrations = BTreeMap::new();
        for index in 0..MAX_PROJECT_WATCH_PATHS {
            add_registration(
                &mut registrations,
                root.join(format!("collection-{index}")),
                RecursiveMode::Recursive,
            )
            .unwrap();
        }
        let error = add_registration(
            &mut registrations,
            root.join("one-too-many"),
            RecursiveMode::Recursive,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            crate::WorkspaceError::Model(message) if message.contains("more than")
        ));
    }
}
