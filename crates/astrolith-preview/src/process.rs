//! Supervised child-process ownership for the Astro dev server.
//!
//! The runtime owns exactly one child per preview server. Both `stdout` and
//! `stderr` are piped and drained continuously into a bounded log, so a chatty
//! or stuck program can never fill an OS pipe buffer and deadlock the child. The
//! retained log is capped at [`OUTPUT_CAP`] bytes per stream; bytes past the cap
//! are still read and discarded (they are counted, never buffered).
//!
//! Ownership is platform specific so that a project switch or application exit
//! never leaves an Astro descendant (for example a detached `esbuild` service)
//! running:
//!
//! - **Unix:** the child is spawned as a new process group leader
//!   (`setsid`-equivalent `process_group(0)`) and the whole group is signalled,
//!   so grandchildren die with the server.
//! - **Windows:** the child is assigned to a Job Object configured with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so closing the job terminates the
//!   entire process tree.
//!
//! Stop is graceful first: a termination signal is sent, the direct child is
//! given a bounded grace period, then the whole tree is force-terminated and the
//! direct child is reaped (`wait`).

use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    task::JoinHandle,
    time::timeout,
};

/// Maximum bytes retained from each output stream (1 MiB per stream).
pub const OUTPUT_CAP: usize = 1024 * 1024;

/// How long a graceful-stop wait may take for the drain tasks to settle after
/// the child has exited.
const DRAIN_SETTLE: Duration = Duration::from_secs(2);

/// Bounded, continuously-drained capture of one child output stream.
///
/// Cloning shares the same buffer; the drain task and readers observe the same
/// state.
#[derive(Clone, Debug, Default)]
pub struct OutputCapture {
    state: Arc<Mutex<CaptureState>>,
}

#[derive(Debug, Default)]
struct CaptureState {
    /// Retained bytes, at most [`OUTPUT_CAP`].
    bytes: Vec<u8>,
    /// Whether any byte beyond the cap was observed.
    truncated: bool,
    /// Total bytes received from the stream, including discarded bytes.
    total_received: u64,
}

impl OutputCapture {
    /// Create an empty capture.
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, chunk: &[u8]) {
        let mut state = self.lock();
        state.total_received = state.total_received.saturating_add(chunk.len() as u64);
        let remaining = OUTPUT_CAP.saturating_sub(state.bytes.len());
        if chunk.len() <= remaining {
            state.bytes.extend_from_slice(chunk);
        } else {
            if remaining > 0 {
                state.bytes.extend_from_slice(&chunk[..remaining]);
            }
            state.truncated = true;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureState> {
        // A drain task never panics while holding the lock, but recover rather
        // than propagating a poisoned buffer into the lifecycle.
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Number of bytes currently retained (never more than [`OUTPUT_CAP`]).
    pub fn len(&self) -> usize {
        self.lock().bytes.len()
    }

    /// Whether the capture currently holds no retained bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether more than [`OUTPUT_CAP`] bytes were produced (bytes were dropped).
    pub fn is_truncated(&self) -> bool {
        self.lock().truncated
    }

    /// Total bytes observed on the stream, including dropped bytes.
    pub fn total_received(&self) -> u64 {
        self.lock().total_received
    }

    /// A copy of the retained bytes.
    pub fn bytes(&self) -> Vec<u8> {
        self.lock().bytes.clone()
    }

    /// Retained bytes after `offset`, plus the retained length.
    ///
    /// Readiness polling walks only the newly appended tail; the prefix is
    /// never copied again.
    pub fn bytes_from(&self, offset: usize) -> (Vec<u8>, usize) {
        let state = self.lock();
        let start = offset.min(state.bytes.len());
        (state.bytes[start..].to_vec(), state.bytes.len())
    }

    /// Retained bytes decoded lossily as UTF-8.
    pub fn text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.lock().bytes).into_owned()
    }

    /// Append bytes without a drain task, for readiness-scan tests.
    #[cfg(test)]
    pub(crate) fn push_bytes(&self, chunk: &[u8]) {
        self.push(chunk);
    }
}

/// One process to spawn, described independently of any GPUI type.
#[derive(Clone, Debug)]
pub struct SpawnSpec {
    /// Executable to run.
    pub program: PathBuf,
    /// Arguments, in order.
    pub args: Vec<OsString>,
    /// Working directory (the project root).
    pub current_dir: PathBuf,
    /// Extra environment variables.
    pub envs: Vec<(OsString, OsString)>,
}

impl SpawnSpec {
    /// A minimal spec for `program` running in `current_dir` with no arguments.
    pub fn new(program: impl Into<PathBuf>, current_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: current_dir.into(),
            envs: Vec::new(),
        }
    }

    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append an environment variable.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }
}

/// Failure while spawning or supervising a child process.
#[derive(Debug)]
pub enum ProcessError {
    /// The program could not be started.
    Spawn {
        /// Program that failed to start.
        program: String,
        /// Underlying OS error.
        source: io::Error,
    },
    /// A supervision operation failed.
    Io(io::Error),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { program, source } => {
                write!(formatter, "failed to start `{program}`: {source}")
            }
            Self::Io(error) => write!(formatter, "process supervision failed: {error}"),
        }
    }
}

impl std::error::Error for ProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } => Some(source),
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for ProcessError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A spawned child owned by the runtime, with bounded output drains and
/// platform-specific process-tree ownership.
pub struct RunningProcess {
    child: Child,
    stdout: OutputCapture,
    stderr: OutputCapture,
    stdout_task: Option<JoinHandle<()>>,
    stderr_task: Option<JoinHandle<()>>,
    /// Process-group id to signal (Unix). Cleared once the child is reaped so a
    /// stale pid can never be signalled.
    #[cfg(unix)]
    process_group: Option<i32>,
    /// Job Object that owns the process tree (Windows).
    #[cfg(windows)]
    job: Option<windows_job::Job>,
    /// Wall-clock spawn time, for diagnostics.
    started_at: Instant,
}

impl RunningProcess {
    /// Spawn `spec` as a runtime-owned child with both output streams drained.
    pub async fn spawn(spec: SpawnSpec) -> Result<Self, ProcessError> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.current_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &spec.envs {
            command.env(key, value);
        }
        #[cfg(unix)]
        {
            // New process group led by the child (`setpgid(0, 0)`), so signalling
            // the group also reaches grandchildren that inherit it.
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|source| ProcessError::Spawn {
            program: spec.program.display().to_string(),
            source,
        })?;

        #[cfg(unix)]
        let process_group = child.id().map(|id| id as i32);
        #[cfg(windows)]
        let job = match child.raw_handle() {
            Some(handle) => Some(windows_job::Job::assign(handle).map_err(ProcessError::Io)?),
            None => None,
        };

        tracing::debug!(
            program = %spec.program.display(),
            current_dir = %spec.current_dir.display(),
            pid = ?child.id(),
            "spawned a preview child process"
        );

        let stdout = OutputCapture::new();
        let stderr = OutputCapture::new();
        let stdout_task = child
            .stdout
            .take()
            .map(|reader| spawn_drain(reader, stdout.clone()));
        let stderr_task = child
            .stderr
            .take()
            .map(|reader| spawn_drain(reader, stderr.clone()));

        Ok(Self {
            child,
            stdout,
            stderr,
            stdout_task,
            stderr_task,
            #[cfg(unix)]
            process_group,
            #[cfg(windows)]
            job,
            started_at: Instant::now(),
        })
    }

    /// Operating-system process id, if still known.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Time since the process was spawned.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// The bounded stdout capture.
    pub fn stdout(&self) -> &OutputCapture {
        &self.stdout
    }

    /// The bounded stderr capture.
    pub fn stderr(&self) -> &OutputCapture {
        &self.stderr
    }

    /// Stdout and stderr text merged for readiness parsing.
    pub fn combined_text(&self) -> String {
        let mut combined = self.stdout.text_lossy();
        combined.push('\n');
        combined.push_str(&self.stderr.text_lossy());
        combined
    }

    /// Poll without blocking. Ownership is retained so a subsequent explicit
    /// cleanup (or `Drop`) can still terminate any lingering descendants.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Wait for the child to exit naturally, then drain and reap its tree.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait().await?;
        // Best-effort: kill any lingering descendants that inherited the pipes.
        self.signal_tree_force();
        self.forget_ownership();
        self.settle_drains().await;
        Ok(status)
    }

    /// Graceful stop: request termination, wait up to `grace`, then force-kill
    /// the whole tree and reap the direct child.
    pub async fn stop(&mut self, grace: Duration) -> io::Result<ExitStatus> {
        tracing::debug!(
            pid = ?self.pid(),
            grace_ms = grace.as_millis(),
            "requesting preview process shutdown"
        );
        if let Some(status) = self.child.try_wait()? {
            tracing::debug!(%status, "preview process had already exited");
            self.signal_tree_force();
            self.forget_ownership();
            self.settle_drains().await;
            return Ok(status);
        }
        self.signal_tree_graceful();
        let status = match timeout(grace, self.child.wait()).await {
            Ok(result) => result?,
            Err(_) => {
                self.signal_tree_force();
                self.child.wait().await?
            }
        };
        self.signal_tree_force();
        self.forget_ownership();
        self.settle_drains().await;
        Ok(status)
    }

    /// Force-terminate the child's whole tree immediately, then reap.
    pub async fn terminate(&mut self) -> io::Result<ExitStatus> {
        tracing::debug!(pid = ?self.pid(), "force-terminating preview process tree");
        let already_exited = self.child.try_wait()?.is_some();
        if !already_exited {
            self.signal_tree_force();
        }
        let status = self.child.wait().await?;
        self.signal_tree_force();
        self.forget_ownership();
        self.settle_drains().await;
        Ok(status)
    }

    /// Join the drain tasks, giving up after [`DRAIN_SETTLE`] so a lingering
    /// inherited pipe can never block stop.
    pub async fn settle_drains(&mut self) {
        join_drain(self.stdout_task.take()).await;
        join_drain(self.stderr_task.take()).await;
    }

    fn forget_ownership(&mut self) {
        #[cfg(unix)]
        {
            self.process_group = None;
        }
        #[cfg(windows)]
        {
            self.job = None;
        }
    }

    #[cfg(unix)]
    fn signal_tree_graceful(&self) {
        use nix::{
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        if let Some(pgid) = self.process_group {
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGTERM);
        }
    }

    #[cfg(unix)]
    fn signal_tree_force(&self) {
        use nix::{
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        if let Some(pgid) = self.process_group {
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        }
    }

    #[cfg(windows)]
    fn signal_tree_graceful(&self) {
        // Windows has no SIGTERM for an arbitrary tree; the Job Object
        // termination is the forceful path and is used for both requests.
        if let Some(job) = &self.job {
            job.terminate();
        }
    }

    #[cfg(windows)]
    fn signal_tree_force(&self) {
        if let Some(job) = &self.job {
            job.terminate();
        }
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        tracing::debug!(
            pid = ?self.child.id(),
            "dropping a preview process owner; killing any remaining tree"
        );
        // Safety net for project switch / application exit: never leave the
        // process tree running if the owner is dropped without an explicit stop.
        self.signal_tree_force();
        let _ = self.child.start_kill();
        if let Some(task) = self.stdout_task.take() {
            task.abort();
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

fn spawn_drain<R>(mut reader: R, capture: OutputCapture) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Err(error) => {
                    tracing::trace!(%error, "preview output stream ended with an error");
                    break;
                }
                Ok(read) => capture.push(&buffer[..read]),
            }
        }
    })
}

async fn join_drain(task: Option<JoinHandle<()>>) {
    if let Some(task) = task {
        let _ = timeout(DRAIN_SETTLE, task).await;
    }
}

#[cfg(windows)]
mod windows_job {
    //! Minimal safe wrapper around a Windows Job Object. Isolated here because
    //! the `windows-sys` calls are `unsafe`.
    #![allow(unsafe_code)]

    use std::io;

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, TRUE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
    };

    /// Owns a Job Object handle and closes it on drop.
    #[derive(Debug)]
    pub struct Job {
        handle: HANDLE,
    }

    impl Job {
        /// Create a kill-on-close job and assign `process` to it.
        pub fn assign(process: HANDLE) -> io::Result<Self> {
            // Safety: all handles are valid for the duration of the calls and
            // the job handle is owned by the returned wrapper.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle == 0 {
                    return Err(io::Error::last_os_error());
                }
                let job = Job { handle };

                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job.handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set != TRUE {
                    return Err(io::Error::last_os_error());
                }
                if AssignProcessToJobObject(job.handle, process) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        /// Terminate every process currently in the job.
        pub fn terminate(&self) {
            // Safety: `self.handle` is a live job handle.
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // Safety: the handle is owned by this wrapper and closed once.
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    use std::os::windows::io::AsRawHandle;

    /// Helper trait so `RunningProcess::spawn` reads a single raw handle value on
    /// both platforms.
    pub trait RawHandle {
        fn raw_handle(&self) -> Option<HANDLE>;
    }

    impl RawHandle for tokio::process::Child {
        fn raw_handle(&self) -> Option<HANDLE> {
            Some(self.as_raw_handle() as HANDLE)
        }
    }
}

#[cfg(windows)]
use windows_job::RawHandle as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_capture_caps_retained_bytes_but_counts_everything() {
        let capture = OutputCapture::new();
        let chunk = vec![b'a'; 64 * 1024];
        // Push 32 chunks = 2 MiB, twice the cap.
        for _ in 0..32 {
            capture.push(&chunk);
        }
        assert_eq!(capture.len(), OUTPUT_CAP, "retained bytes are capped");
        assert!(capture.is_truncated(), "overflow is flagged");
        assert_eq!(
            capture.total_received(),
            2 * 1024 * 1024,
            "all bytes are still drained and counted"
        );
    }

    #[test]
    fn output_capture_does_not_mark_small_streams_truncated() {
        let capture = OutputCapture::new();
        capture.push(b"hello ");
        capture.push(b"world");
        assert_eq!(capture.text_lossy(), "hello world");
        assert_eq!(capture.len(), 11);
        assert!(!capture.is_truncated());
        assert_eq!(capture.total_received(), 11);
    }

    #[test]
    fn spawn_spec_builds_arguments_and_environment() {
        let spec = SpawnSpec::new("node", "/tmp/project")
            .arg("dev")
            .env("ASTROLITH_PREVIEW_PORT", "4321");
        assert_eq!(spec.program, PathBuf::from("node"));
        assert_eq!(spec.args, vec![OsString::from("dev")]);
        assert_eq!(
            spec.envs,
            vec![(
                OsString::from("ASTROLITH_PREVIEW_PORT"),
                OsString::from("4321")
            )]
        );
    }
}
