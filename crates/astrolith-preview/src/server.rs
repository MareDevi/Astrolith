//! Astro dev-server lifecycle.
//!
//! [`DevServer`] owns the project-local Astro CLI `dev` process on `127.0.0.1`
//! and moves through [`ServerStatus`] as it starts, becomes ready, stops, or
//! fails. Readiness is detected by parsing the local URL Astro prints once its
//! listener is up. Every operation is bounded: [`DEFAULT_STARTUP_TIMEOUT`] for
//! startup and [`DEFAULT_STOP_GRACE`] of graceful stop before force termination.
//!
//! This module is GUI-free: it accepts owned data (`ProjectId`, paths) and
//! returns plain results. The GPUI layer bridges these futures onto the
//! application runtime and observes [`ServerStatus`].

use std::{
    ffi::OsString,
    io,
    net::TcpListener,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use astrolith_astro::Cancellation;
use astrolith_core::ProjectId;
use url::Url;

use crate::process::{OutputCapture, ProcessError, RunningProcess, SpawnSpec};

/// Default bound on how long Astro may take to become ready.
pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Default grace period between a graceful stop request and force termination.
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(3);
/// Poll interval while waiting for readiness or exit.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Lifecycle state of the preview dev server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    /// No child process is running.
    Stopped,
    /// A child has been spawned and readiness is being awaited.
    Starting,
    /// The server is serving; the URL is the local preview root.
    Ready(Url),
    /// A graceful stop is in progress.
    Stopping,
    /// Startup or supervision failed; the message is user-reportable.
    Failed(String),
}

impl ServerStatus {
    /// Whether the server is ready to serve requests.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    /// The ready URL, when the server is ready.
    pub fn url(&self) -> Option<&Url> {
        match self {
            Self::Ready(url) => Some(url),
            _ => None,
        }
    }

    /// A short stable label, useful for UI and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Ready(_) => "ready",
            Self::Stopping => "stopping",
            Self::Failed(_) => "failed",
        }
    }
}

/// Failure detail from one start attempt.
///
/// Astro reports the actionable cause on stderr (a missing dependency, a
/// configuration error, an occupied port). The panel shows [`Self::report`] so
/// the user can fix the project and retry without opening a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupFailure {
    message: String,
    stderr_excerpt: Option<String>,
}

impl StartupFailure {
    /// Build a failure from a short message and captured stderr.
    pub fn new(message: impl Into<String>, stderr: Option<String>) -> Self {
        let stderr_excerpt = stderr
            .map(|text| excerpt(&text))
            .filter(|text| !text.is_empty());
        Self {
            message: message.into(),
            stderr_excerpt,
        }
    }

    /// The short classification shown as the status.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The captured stderr excerpt, when the child produced one.
    pub fn stderr_excerpt(&self) -> Option<&str> {
        self.stderr_excerpt.as_deref()
    }

    /// The user-facing message: classification plus any stderr evidence.
    pub fn report(&self) -> String {
        match &self.stderr_excerpt {
            Some(stderr) => format!("{}\n\nAstro output:\n{stderr}", self.message),
            None => self.message.clone(),
        }
    }
}

impl std::fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.report())
    }
}

impl std::error::Error for StartupFailure {}

/// Failure to start or stop the dev server.
#[derive(Debug)]
pub enum DevServerError {
    /// Astro is not installed in the project.
    NotInstalled(String),
    /// The child could not be spawned.
    Spawn(String),
    /// The chosen port is already in use.
    PortInUse(u16),
    /// Startup exceeded the configured deadline.
    Timeout(Duration),
    /// Startup was cancelled cooperatively.
    Cancelled,
    /// The child exited before it became ready.
    Exited(String),
    /// Another supervision error.
    Io(String),
}

impl std::fmt::Display for DevServerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled(message) => write!(formatter, "Astro is not installed: {message}"),
            Self::Spawn(message) => write!(formatter, "cannot start Astro dev server: {message}"),
            Self::PortInUse(port) => write!(
                formatter,
                "port {port} is in use; the Astro preview server could not start"
            ),
            Self::Timeout(bound) => write!(
                formatter,
                "Astro dev server did not become ready within {}s",
                bound.as_secs()
            ),
            Self::Cancelled => write!(formatter, "Astro dev server startup was cancelled"),
            Self::Exited(message) => {
                write!(formatter, "Astro dev server exited before ready: {message}")
            }
            Self::Io(message) => write!(formatter, "Astro dev server error: {message}"),
        }
    }
}

impl std::error::Error for DevServerError {}

impl From<ProcessError> for DevServerError {
    fn from(error: ProcessError) -> Self {
        match error {
            ProcessError::Spawn { program, source } => {
                Self::Spawn(format!("`{program}`: {source}"))
            }
            ProcessError::Io(source) => Self::Io(source.to_string()),
        }
    }
}

/// How to launch the dev server. Production resolves the project-local Astro
/// entry; tests may substitute a direct program with the same argv contract.
#[derive(Clone, Debug)]
enum Launch {
    /// `node <project node_modules/astro entry> dev --host 127.0.0.1 --port N`.
    Astro { node: PathBuf },
    /// `<program> <prefix...> dev --host 127.0.0.1 --port N` (test/override).
    Direct {
        program: PathBuf,
        prefix: Vec<OsString>,
    },
}

/// Configuration for one [`DevServer`].
#[derive(Clone, Debug)]
pub struct DevServerConfig {
    root: PathBuf,
    launch: Launch,
    port: Option<u16>,
    startup_timeout: Duration,
    stop_grace: Duration,
}

impl DevServerConfig {
    /// Config that runs the project-local Astro CLI via `node` from `PATH`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            launch: Launch::Astro {
                node: PathBuf::from("node"),
            },
            port: None,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            stop_grace: DEFAULT_STOP_GRACE,
        }
    }

    /// Override the Node executable used to run the Astro entry.
    pub fn with_node(mut self, node: impl Into<PathBuf>) -> Self {
        if let Launch::Astro { .. } = self.launch {
            self.launch = Launch::Astro { node: node.into() };
        }
        self
    }

    /// Run `program` directly instead of resolving `node_modules/astro`.
    ///
    /// `program` receives the same trailing argv as the Astro CLI:
    /// `dev --host 127.0.0.1 --port <n>`. Used by tests.
    pub fn with_direct_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.launch = Launch::Direct {
            program: program.into(),
            prefix: Vec::new(),
        };
        self
    }

    /// Run `runner script dev --host ...` instead of the Astro CLI.
    ///
    /// The `script` is passed as the first argument to `runner` (for example
    /// `/bin/sh`), so the script never has to be an executable file itself.
    pub fn with_script_runner(
        mut self,
        runner: impl Into<PathBuf>,
        script: impl Into<PathBuf>,
    ) -> Self {
        self.launch = Launch::Direct {
            program: runner.into(),
            prefix: vec![script.into().into_os_string()],
        };
        self
    }

    /// Force a specific port; when unset an available port is chosen.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Override the startup deadline.
    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    /// Override the graceful-stop grace period.
    pub fn with_stop_grace(mut self, grace: Duration) -> Self {
        self.stop_grace = grace;
        self
    }

    /// Project root the dev server runs in.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

/// One supervised Astro dev server owned by the runtime.
pub struct DevServer {
    project: ProjectId,
    config: DevServerConfig,
    status: ServerStatus,
    process: Option<RunningProcess>,
    last_failure: Option<StartupFailure>,
}

impl DevServer {
    /// Create a stopped server for `project`.
    pub fn new(project: ProjectId, config: DevServerConfig) -> Self {
        Self {
            project,
            config,
            status: ServerStatus::Stopped,
            process: None,
            last_failure: None,
        }
    }

    /// The project this server belongs to.
    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    /// Current lifecycle state.
    pub fn status(&self) -> &ServerStatus {
        &self.status
    }

    /// The ready URL, if any.
    pub fn ready_url(&self) -> Option<&Url> {
        self.status.url()
    }

    /// Detail of the most recent failed start, including stderr evidence.
    pub fn failure(&self) -> Option<&StartupFailure> {
        self.last_failure.as_ref()
    }

    /// Record a start failure and return the same error for the caller.
    fn fail(&mut self, error: DevServerError, stderr: Option<String>) -> DevServerError {
        let failure = StartupFailure::new(error.to_string(), stderr);
        tracing::error!(%failure.message, "Astro dev server start failed");
        self.status = ServerStatus::Failed(failure.report());
        self.last_failure = Some(failure);
        error
    }

    /// Retained stdout text of the running (or last) child.
    pub fn stdout_text(&self) -> String {
        self.process
            .as_ref()
            .map(|process| process.stdout().text_lossy())
            .unwrap_or_default()
    }

    /// Retained stderr text of the running (or last) child.
    pub fn stderr_text(&self) -> String {
        self.process
            .as_ref()
            .map(|process| process.stderr().text_lossy())
            .unwrap_or_default()
    }

    /// Bounded stdout capture of the running child, when one exists.
    pub fn stdout_capture(&self) -> Option<&OutputCapture> {
        self.process.as_ref().map(RunningProcess::stdout)
    }

    /// Bounded stderr capture of the running child, when one exists.
    pub fn stderr_capture(&self) -> Option<&OutputCapture> {
        self.process.as_ref().map(RunningProcess::stderr)
    }

    /// PID of the running child, if any.
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(|process| process.pid())
    }

    /// Start the server, waiting until it is ready.
    pub async fn start(&mut self) -> Result<Url, DevServerError> {
        self.start_inner(None).await
    }

    /// Start the server, allowing cooperative cancellation during startup.
    pub async fn start_cancellable(
        &mut self,
        cancellation: &Cancellation,
    ) -> Result<Url, DevServerError> {
        self.start_inner(Some(cancellation)).await
    }

    async fn start_inner(
        &mut self,
        cancellation: Option<&Cancellation>,
    ) -> Result<Url, DevServerError> {
        if matches!(self.status, ServerStatus::Starting) {
            return Err(self.fail(
                DevServerError::Io("server is already starting".to_owned()),
                None,
            ));
        }
        if self.process.is_some() {
            return Err(self.fail(
                DevServerError::Io(
                    "server is already running; stop it before starting again".to_owned(),
                ),
                None,
            ));
        }
        self.last_failure = None;
        if let Some(cancellation) = cancellation
            && cancellation.check().is_err()
        {
            self.status = ServerStatus::Stopped;
            return Err(DevServerError::Cancelled);
        }

        self.status = ServerStatus::Starting;
        tracing::debug!(
            project = %self.project.as_str(),
            root = %self.config.root.display(),
            "starting Astro dev server"
        );

        let port = match self.config.port {
            Some(port) => port,
            None => match available_port() {
                Ok(port) => port,
                Err(error) => {
                    let message = format!("no available local port: {error}");
                    tracing::error!(%message, "could not allocate a preview port");
                    return Err(self.fail(DevServerError::Io(message), None));
                }
            },
        };

        let spec = match self.build_spec(port) {
            Ok(spec) => spec,
            Err(error) => {
                tracing::error!(%error, "could not build the Astro dev server command");
                return Err(self.fail(error, None));
            }
        };

        let mut process = match RunningProcess::spawn(spec).await {
            Ok(process) => process,
            Err(error) => {
                let error = DevServerError::from(error);
                tracing::error!(%error, "could not spawn the Astro dev server");
                return Err(self.fail(error, None));
            }
        };
        tracing::debug!(pid = ?process.pid(), port, "Astro dev server process spawned");

        let deadline = Instant::now() + self.config.startup_timeout;
        loop {
            if let Some(cancellation) = cancellation
                && cancellation.check().is_err()
            {
                let _ = process.terminate().await;
                self.status = ServerStatus::Stopped;
                return Err(DevServerError::Cancelled);
            }

            let text = process.combined_text();
            if let Some(url) = parse_ready_url(&text, port) {
                tracing::info!(
                    project = %self.project.as_str(),
                    %url,
                    pid = ?process.pid(),
                    elapsed_ms = process.elapsed().as_millis(),
                    "Astro dev server ready"
                );
                self.status = ServerStatus::Ready(url.clone());
                self.last_failure = None;
                self.process = Some(process);
                return Ok(url);
            }

            match process.try_wait() {
                Ok(Some(exit)) => {
                    // Astro prints "Port … is in use, trying another one…" and
                    // then binds the next port, so an in-use message is only a
                    // failure if the process actually exited. Settle both drains
                    // first: the final stderr bytes may still be in flight when
                    // `try_wait` reports the exit.
                    let _ = process.terminate().await;
                    let text = process.combined_text();
                    let stderr = process.stderr().text_lossy();
                    if let Some(message) = port_in_use_message(&text, port) {
                        tracing::error!(port, %message, "Astro dev server port is in use");
                        return Err(self.fail(DevServerError::PortInUse(port), Some(stderr)));
                    }
                    tracing::error!(
                        %exit,
                        stderr = %excerpt(&stderr),
                        "Astro dev server exited during startup"
                    );
                    return Err(self.fail(
                        DevServerError::Exited(format!("exited ({exit})")),
                        Some(stderr),
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    let stderr = process.stderr().text_lossy();
                    let _ = process.terminate().await;
                    tracing::error!(%error, "could not poll the Astro dev server");
                    return Err(self.fail(DevServerError::Io(error.to_string()), Some(stderr)));
                }
            }

            if Instant::now() >= deadline {
                tracing::error!(
                    timeout_ms = self.config.startup_timeout.as_millis(),
                    "Astro dev server startup timed out"
                );
                let stderr = process.stderr().text_lossy();
                let _ = process.terminate().await;
                return Err(self.fail(
                    DevServerError::Timeout(self.config.startup_timeout),
                    Some(stderr),
                ));
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Gracefully stop the server: request termination, wait up to the configured
    /// grace period, then force-terminate the tree and reap.
    pub async fn stop(&mut self) -> Result<(), DevServerError> {
        match self.status {
            ServerStatus::Stopped | ServerStatus::Failed(_) => {
                self.process = None;
                return Ok(());
            }
            _ => {}
        }
        self.status = ServerStatus::Stopping;
        tracing::debug!(
            project = %self.project.as_str(),
            pid = ?self.pid(),
            "stopping the Astro dev server"
        );
        if let Some(mut process) = self.process.take() {
            match process.stop(self.config.stop_grace).await {
                Ok(status) => {
                    tracing::debug!(%status, "Astro dev server stopped");
                    self.status = ServerStatus::Stopped;
                    Ok(())
                }
                Err(error) => {
                    let message = error.to_string();
                    tracing::warn!(%error, "Astro dev server did not stop cleanly");
                    self.status = ServerStatus::Failed(message.clone());
                    Err(DevServerError::Io(message))
                }
            }
        } else {
            self.status = ServerStatus::Stopped;
            Ok(())
        }
    }

    /// Immediately kill the server's whole process tree without awaiting.
    ///
    /// This is the application-exit path: there is no time for a graceful stop,
    /// and dropping the owned [`RunningProcess`] signals the process group (or
    /// terminates the Job Object) and reaps the direct child. Callers use it
    /// synchronously from a quit observer; normal shutdown should use
    /// [`DevServer::stop`].
    pub fn abort(&mut self) {
        if let Some(pid) = self.pid() {
            tracing::debug!(pid, "aborting the Astro dev server");
        }
        // Dropping the process applies the kill-on-drop safety net.
        self.process = None;
        self.status = ServerStatus::Stopped;
    }

    /// Poll for an unexpected child exit without waiting for it.
    ///
    /// This is the UI-safe counterpart of [`DevServer::wait_for_exit`]: it takes
    /// the process check synchronously, so a caller polling it never holds the
    /// server across an `await` and a concurrent [`DevServer::stop`] can still
    /// acquire it. Returns the new status once the ready process has exited.
    pub fn poll_exit(&mut self) -> Result<Option<ServerStatus>, DevServerError> {
        if !matches!(self.status, ServerStatus::Ready(_)) {
            return Ok(None);
        }
        let Some(process) = self.process.as_mut() else {
            return Ok(None);
        };
        match process.try_wait() {
            Ok(Some(exit)) => {
                // Dropping the owner applies the kill-on-drop safety net to any
                // descendants that outlived the direct child.
                self.process = None;
                if exit.success() {
                    tracing::info!(%exit, "Astro dev server exited");
                    self.status = ServerStatus::Stopped;
                } else {
                    tracing::warn!(%exit, "Astro dev server exited unexpectedly");
                    self.status = ServerStatus::Failed(format!(
                        "Astro dev server exited unexpectedly ({exit})"
                    ));
                }
                Ok(Some(self.status.clone()))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                self.process = None;
                let message = error.to_string();
                tracing::error!(%error, "could not poll the Astro dev server");
                self.status = ServerStatus::Failed(message);
                Ok(Some(self.status.clone()))
            }
        }
    }

    /// Await natural child exit and reflect it in the status.
    ///
    /// A clean exit becomes [`ServerStatus::Stopped`]; a non-zero exit becomes
    /// [`ServerStatus::Failed`]. Driving this future from the runtime is how an
    /// unexpected server death is observed.
    pub async fn wait_for_exit(&mut self) -> Result<ServerStatus, DevServerError> {
        let Some(mut process) = self.process.take() else {
            return Ok(self.status.clone());
        };
        let exit = process
            .wait()
            .await
            .map_err(|error| DevServerError::Io(error.to_string()))?;
        if exit.success() {
            tracing::info!(%exit, "Astro dev server exited");
            self.status = ServerStatus::Stopped;
        } else {
            tracing::warn!(%exit, "Astro dev server exited unexpectedly");
            self.status =
                ServerStatus::Failed(format!("Astro dev server exited unexpectedly ({exit})"));
        }
        Ok(self.status.clone())
    }

    fn build_spec(&self, port: u16) -> Result<SpawnSpec, DevServerError> {
        let mut args: Vec<OsString> = Vec::new();
        let program = match &self.config.launch {
            Launch::Astro { node } => {
                let entry =
                    astrolith_astro::schema::generation::resolve_astro_entry(&self.config.root)
                        .map_err(|error| DevServerError::NotInstalled(error.to_string()))?;
                args.push(entry.into_os_string());
                node.clone()
            }
            Launch::Direct { program, prefix } => {
                args.extend(prefix.iter().cloned());
                program.clone()
            }
        };
        args.push("dev".into());
        args.push("--host".into());
        args.push("127.0.0.1".into());
        args.push("--port".into());
        args.push(port.to_string().into());
        // Astro 7 auto-backgrounds `astro dev` when it detects an agent
        // environment, detaching into its own session so a process-group stop
        // cannot reach it. `--ignore-lock` opts out of that inference (and of
        // the cross-process lock file we do not want) while staying a
        // foreground child we own. Astro 5 and 6 do not know the flag.
        if matches!(self.config.launch, Launch::Astro { .. })
            && installed_astro_major(&self.config.root).is_some_and(|major| major >= 7)
        {
            args.push("--ignore-lock".into());
        }

        Ok(SpawnSpec {
            program,
            args,
            current_dir: self.config.root.clone(),
            envs: vec![(
                OsString::from("ASTROLITH_PREVIEW_PORT"),
                OsString::from(port.to_string()),
            )],
        })
    }
}

/// Major version of the project-local Astro package, when it is readable.
///
/// The process-lifecycle flags differ across releases (Astro 7 can background
/// itself), so startup inspects the installed release rather than the declared
/// range. A missing or malformed package is not fatal here: resolution already
/// failed or succeeded independently.
fn installed_astro_major(root: &Path) -> Option<u64> {
    let source = std::fs::read_to_string(root.join("node_modules/astro/package.json")).ok()?;
    let package: serde_json::Value = serde_json::from_str(&source).ok()?;
    package
        .get("version")?
        .as_str()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Pick an available ephemeral port on the loopback interface.
fn available_port() -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Strip ANSI SGR/CSI escape sequences so readiness parsing sees clean text.
fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        output.push(character);
    }
    output
}

/// Parse the first loopback preview URL Astro prints once it is listening.
///
/// Accepts `localhost` or `127.0.0.1` on any port (Astro may auto-increment past
/// an occupied port). The returned URL is normalized to `127.0.0.1`.
fn parse_ready_url(text: &str, fallback_port: u16) -> Option<Url> {
    let cleaned = strip_ansi(text);
    for token in cleaned.split(|character: char| {
        character.is_whitespace()
            || matches!(character, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']')
    }) {
        let Some(index) = token.find("http://") else {
            continue;
        };
        let raw = token[index..].trim_end_matches([',', ';', '.']);
        let Ok(url) = Url::parse(raw) else {
            continue;
        };
        let local = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"));
        if !local {
            continue;
        }
        let port = url.port().unwrap_or(fallback_port);
        return Url::parse(&format!("http://127.0.0.1:{port}/")).ok();
    }
    None
}

/// Detect Astro's "port in use" failure in captured output.
fn port_in_use_message(text: &str, port: u16) -> Option<String> {
    let lowered = strip_ansi(text).to_lowercase();
    if lowered.contains("eaddrinuse") || (lowered.contains("port") && lowered.contains("in use")) {
        Some(format!(
            "port {port} is in use; the Astro preview server could not start"
        ))
    } else {
        None
    }
}

fn excerpt(text: &str) -> String {
    let trimmed = text.trim();
    const LIMIT: usize = 2000;
    if trimmed.chars().count() <= LIMIT {
        trimmed.to_owned()
    } else {
        let mut shortened: String = trimmed.chars().take(LIMIT).collect();
        shortened.push('…');
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_status_exposes_url_and_label() {
        let stopped = ServerStatus::Stopped;
        assert!(!stopped.is_ready());
        assert!(stopped.url().is_none());
        assert_eq!(stopped.label(), "stopped");

        let url = Url::parse("http://127.0.0.1:4321/").unwrap();
        let ready = ServerStatus::Ready(url.clone());
        assert!(ready.is_ready());
        assert_eq!(ready.url(), Some(&url));
        assert_eq!(ready.label(), "ready");

        assert_eq!(ServerStatus::Starting.label(), "starting");
        assert_eq!(ServerStatus::Stopping.label(), "stopping");
        assert_eq!(ServerStatus::Failed("boom".into()).label(), "failed");
    }

    #[test]
    fn parses_plain_and_ansi_colored_ready_lines() {
        let plain = "  Local    http://localhost:4321/\n";
        assert_eq!(
            parse_ready_url(plain, 4321).unwrap().as_str(),
            "http://127.0.0.1:4321/"
        );

        let colored = "\u{1b}[32m┃ Local\u{1b}[0m    \u{1b}[36mhttp://127.0.0.1:5199/\u{1b}[0m\n";
        assert_eq!(
            parse_ready_url(colored, 4321).unwrap().as_str(),
            "http://127.0.0.1:5199/"
        );
    }

    #[test]
    fn ignores_network_urls_and_non_local_hosts() {
        let text = "Network use --host to expose\nhttp://192.168.1.10:4321/\n";
        assert!(parse_ready_url(text, 4321).is_none());
    }

    #[test]
    fn ignores_trailing_punctuation_after_url() {
        let text = "Local: http://127.0.0.1:4321/, ready";
        assert_eq!(
            parse_ready_url(text, 1).unwrap().as_str(),
            "http://127.0.0.1:4321/"
        );
    }

    #[test]
    fn detects_port_in_use_messages() {
        assert!(port_in_use_message("Error: Port 4321 is in use", 4321).is_some());
        assert!(
            port_in_use_message("Error: listen EADDRINUSE: address already in use", 4321).is_some()
        );
        assert!(port_in_use_message("astro started fine", 4321).is_none());
    }

    #[test]
    fn available_port_is_bindable_and_nonzero() {
        let port = available_port().unwrap();
        assert_ne!(port, 0);
        // The port we returned should be (re)bindable after being dropped.
        TcpListener::bind(("127.0.0.1", port)).expect("returned port should be free");
    }

    #[test]
    fn excerpt_shortens_long_output() {
        let short = excerpt("  ok  ");
        assert_eq!(short, "ok");
        let long = "x".repeat(2500);
        let shortened = excerpt(&long);
        assert_eq!(shortened.chars().count(), 2001);
    }
}

#[cfg(all(test, unix))]
#[path = "server_tests.rs"]
mod unix_tests;
