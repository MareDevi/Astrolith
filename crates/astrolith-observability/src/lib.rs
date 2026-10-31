//! Process-wide logging bootstrap.
//!
//! Astrolith logs through [`tracing`] and initializes exactly one subscriber in
//! the desktop binary. Library crates only emit events; they never install a
//! subscriber. The default setup writes human-readable records to stderr and
//! structured JSON records to a daily-rotated file, both from a non-blocking
//! writer so the GPUI thread never performs file I/O.
//!
//! See `docs/technical/logging.md` for the environment variables and the
//! expected event levels.

use std::{
    env,
    fmt::{self, Write as _},
    io::{self, IsTerminal},
    panic,
    path::{Path, PathBuf},
    sync::Once,
};

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{InitError as RollingInitError, RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    Layer,
    filter::EnvFilter,
    fmt::{self as fmt_layer, time::SystemTime},
    layer::SubscriberExt,
    registry,
    util::SubscriberInitExt,
};

/// Filter applied when neither `ASTROLITH_LOG` nor `RUST_LOG` is set.
///
/// Application crates stay at `info`; third-party crates stay quiet at `warn`.
pub const DEFAULT_FILTER: &str = "warn,\
astrolith_desktop=info,\
astrolith_gpui=info,\
astrolith_core=info,\
astrolith_astro=info,\
astrolith_document=info,\
astrolith_workspace=info,\
astrolith_preview=info,\
astrolith_observability=info";

/// File name prefix used by the default rolling appender.
pub const DEFAULT_FILE_PREFIX: &str = "astrolith";

/// Number of daily log files retained by default.
pub const DEFAULT_MAX_LOG_FILES: usize = 7;

/// Destination format for a log sink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogFormat {
    /// Human-readable single-line records, suitable for a terminal.
    Text,
    /// One JSON object per record, suitable for aggregation and tooling.
    Json,
}

impl LogFormat {
    /// Parse `text`, `plain`, or `json` case-insensitively.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" | "plain" | "human" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Settings for the rolling JSON log file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileLog {
    /// Directory that receives `astrolith.YYYY-MM-DD.log`.
    pub directory: PathBuf,
    /// File name prefix before the rotation date.
    pub prefix: String,
    /// Number of daily files retained; older files are pruned.
    pub max_files: usize,
}

impl FileLog {
    /// Build a file sink in `directory` with the default prefix and retention.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            prefix: DEFAULT_FILE_PREFIX.to_owned(),
            max_files: DEFAULT_MAX_LOG_FILES,
        }
    }
}

/// Complete logging configuration consumed once at process start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogConfig {
    /// `EnvFilter` directives, for example `info,astrolith_gpui=debug`.
    pub filter: String,
    /// Whether human-readable records also go to stderr.
    pub stderr: bool,
    /// Enable ANSI colors on the stderr sink.
    pub stderr_ansi: bool,
    /// Format used for the stderr sink.
    pub stderr_format: LogFormat,
    /// Structured file sink; `None` disables file logging.
    pub file: Option<FileLog>,
    /// Format used for the file sink.
    pub file_format: LogFormat,
    /// Non-fatal problems found while reading the environment.
    pub warnings: Vec<String>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            filter: DEFAULT_FILTER.to_owned(),
            stderr: true,
            stderr_ansi: io::stderr().is_terminal(),
            stderr_format: LogFormat::Text,
            file: None,
            file_format: LogFormat::Json,
            warnings: Vec::new(),
        }
    }
}

impl LogConfig {
    /// Read configuration from the environment.
    ///
    /// Environment variables:
    /// - `ASTROLITH_LOG` (or `RUST_LOG`): `EnvFilter` directives.
    /// - `ASTROLITH_LOG_FILE`: `off`, `0`, or `false` disables the file sink.
    /// - `ASTROLITH_LOG_DIR`: absolute directory for log files.
    /// - `ASTROLITH_LOG_FORMAT`: stderr format (`text` or `json`).
    /// - `ASTROLITH_LOG_FILE_FORMAT`: file format (`json` or `text`).
    /// - `ASTROLITH_LOG_MAX_FILES`: positive number of daily files to retain.
    ///
    /// Invalid values are ignored, recorded in [`LogConfig::warnings`], and the
    /// process still starts with safe defaults.
    pub fn from_env() -> Self {
        let mut config = Self {
            filter: env_value("ASTROLITH_LOG")
                .or_else(|| env_value("RUST_LOG"))
                .unwrap_or_else(|| DEFAULT_FILTER.to_owned()),
            ..Self::default()
        };

        let mut file_requested = true;
        match env_value("ASTROLITH_LOG_FILE") {
            Some(value) if matches!(value.to_ascii_lowercase().as_str(), "off" | "0" | "false") => {
                file_requested = false;
            }
            Some(value) if !value.is_empty() => {
                config.warnings.push(format!(
                    "ASTROLITH_LOG_FILE={value} is not a recognized switch; file logging stays enabled \
                     (use off, 0, or false to disable it)"
                ));
            }
            _ => {}
        }

        let directory = match env_value("ASTROLITH_LOG_DIR") {
            Some(value) => {
                let path = PathBuf::from(&value);
                if path.is_absolute() {
                    path
                } else {
                    config.warnings.push(format!(
                        "ASTROLITH_LOG_DIR={value} is not absolute; ignoring it"
                    ));
                    default_log_dir().ok().unwrap_or_else(|| PathBuf::from("."))
                }
            }
            None => default_log_dir().unwrap_or_else(|error| {
                config.warnings.push(format!(
                    "No writable log directory was found ({error}); file logging is disabled"
                ));
                PathBuf::new()
            }),
        };
        if file_requested && !directory.as_os_str().is_empty() {
            let mut file = FileLog::new(directory);
            if let Some(value) = env_value("ASTROLITH_LOG_MAX_FILES") {
                match value.parse::<usize>() {
                    Ok(count) if count > 0 => file.max_files = count,
                    _ => config.warnings.push(format!(
                        "ASTROLITH_LOG_MAX_FILES={value} is not a positive integer; using {DEFAULT_MAX_LOG_FILES}"
                    )),
                }
            }
            config.file = Some(file);
        }

        if let Some(value) = env_value("ASTROLITH_LOG_FORMAT") {
            match LogFormat::parse(&value) {
                Some(format) => config.stderr_format = format,
                None => config.warnings.push(format!(
                    "ASTROLITH_LOG_FORMAT={value} is not text or json; using text"
                )),
            }
        }
        if let Some(value) = env_value("ASTROLITH_LOG_FILE_FORMAT") {
            match LogFormat::parse(&value) {
                Some(format) => config.file_format = format,
                None => config.warnings.push(format!(
                    "ASTROLITH_LOG_FILE_FORMAT={value} is not text or json; using json"
                )),
            }
        }
        config
    }
}

/// Failure while installing the global subscriber.
#[derive(Debug)]
pub enum InitError {
    /// The configured filter directives are invalid.
    Filter(String),
    /// The rolling log directory or file could not be initialized.
    File(RollingInitError),
    /// Neither stderr nor file output is enabled.
    NoSink,
    /// A global subscriber was installed earlier in this process.
    AlreadyInitialized,
}

impl fmt::Display for InitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Filter(error) => write!(formatter, "invalid log filter: {error}"),
            Self::File(error) => write!(formatter, "could not initialize the log file: {error}"),
            Self::NoSink => formatter.write_str("no logging sink is enabled"),
            Self::AlreadyInitialized => {
                formatter.write_str("a global tracing subscriber is already installed")
            }
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::File(error) => Some(error),
            _ => None,
        }
    }
}

/// Handle that owns the non-blocking file worker.
///
/// Keep it alive for the whole process lifetime. Dropping it flushes buffered
/// records and stops the writer thread.
#[derive(Debug)]
pub struct LogGuard {
    _worker: Option<WorkerGuard>,
    directory: Option<PathBuf>,
    prefix: Option<String>,
}

impl LogGuard {
    /// Directory receiving daily log files, if file logging is enabled.
    pub fn log_directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    /// File name prefix before the UTC rotation date.
    pub fn file_prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }
}

/// Install the global subscriber described by `config`.
///
/// The returned guard must remain alive until process exit. This function also
/// installs a panic hook that records the panic message and location through
/// `tracing` before delegating to the previous hook.
pub fn init(config: LogConfig) -> Result<LogGuard, InitError> {
    let filter =
        EnvFilter::try_new(&config.filter).map_err(|error| InitError::Filter(error.to_string()))?;

    let mut worker = None;
    let mut file_directory = None;
    let mut file_prefix = None;

    let stderr_layer = config.stderr.then(|| {
        let layer = fmt_layer::layer()
            .with_writer(io::stderr)
            .with_ansi(config.stderr_ansi)
            .with_target(true)
            .with_file(true)
            .with_line_number(true)
            .with_thread_names(true)
            .with_timer(SystemTime);
        match config.stderr_format {
            LogFormat::Text => layer.boxed(),
            LogFormat::Json => layer.json().boxed(),
        }
    });

    let mut file_error = None;
    let file_layer = match &config.file {
        Some(file) => match RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(&file.prefix)
            .filename_suffix("log")
            .max_log_files(file.max_files)
            .build(&file.directory)
        {
            Ok(appender) => {
                let (writer, guard) = tracing_appender::non_blocking(appender);
                worker = Some(guard);
                file_directory = Some(file.directory.clone());
                file_prefix = Some(file.prefix.clone());
                let layer = fmt_layer::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .with_target(true)
                    .with_file(true)
                    .with_line_number(true)
                    .with_thread_names(true)
                    .with_timer(SystemTime);
                Some(match config.file_format {
                    LogFormat::Text => layer.boxed(),
                    LogFormat::Json => layer.json().boxed(),
                })
            }
            Err(error) => {
                file_error = Some(error);
                None
            }
        },
        None => None,
    };
    if !config.stderr && file_layer.is_none() {
        return Err(match file_error {
            Some(error) => InitError::File(error),
            None => InitError::NoSink,
        });
    }
    if let Some(error) = &file_error {
        // File logging is an operational aid, not a startup prerequisite.
        // Keep stderr logging and the panic hook working when the state
        // directory is unavailable or read-only.
        eprintln!("Logging file unavailable ({error}); continuing with stderr only");
    }

    registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .map_err(|_| InitError::AlreadyInitialized)?;

    install_panic_hook();

    let guard = LogGuard {
        _worker: worker,
        directory: file_directory,
        prefix: file_prefix,
    };
    tracing::info!(
        filter = %config.filter,
        log_directory = ?guard.log_directory(),
        file_format = ?config.file_format,
        "logging initialized"
    );
    Ok(guard)
}

/// Install a panic hook that records panics through `tracing`.
///
/// Safe to call more than once; only the first call installs a hook.
pub fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|location| format!("{}:{}", location.file(), location.line()))
                .unwrap_or_else(|| "unknown".to_owned());
            let mut payload = String::new();
            let message = info.payload();
            if let Some(text) = message.downcast_ref::<&str>() {
                payload.push_str(text);
            } else if let Some(text) = message.downcast_ref::<String>() {
                payload.push_str(text);
            } else {
                payload.push_str("non-string panic payload");
            }
            let _ = write!(payload, " (at {location})");
            tracing::error!(panic = %payload, location = %location, "process panicked");
            previous(info);
        }));
    });
}

/// Platform log directory, respecting `ASTROLITH_LOG_DIR`.
pub fn default_log_dir() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("ASTROLITH_LOG_DIR") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err(io::Error::other("ASTROLITH_LOG_DIR must be absolute"))
        };
    }

    #[cfg(target_os = "windows")]
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Logs"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        });

    base.filter(|path| path.is_absolute())
        .map(|path| path.join("astrolith/logs"))
        .ok_or_else(|| io::Error::other("no platform state directory is available"))
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse_case_insensitively_and_reject_unknown_values() {
        assert_eq!(LogFormat::parse("JSON"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse(" text "), Some(LogFormat::Text));
        assert_eq!(LogFormat::parse("plain"), Some(LogFormat::Text));
        assert_eq!(LogFormat::parse("xml"), None);
    }

    #[test]
    fn default_filter_keeps_application_crates_at_info() {
        let filter = EnvFilter::try_new(DEFAULT_FILTER).expect("default filter must be valid");
        let rendered = filter.to_string();
        for directive in [
            "warn",
            "astrolith_desktop=info",
            "astrolith_gpui=info",
            "astrolith_preview=info",
        ] {
            assert!(
                rendered.split(',').any(|part| part == directive),
                "{directive} missing from {rendered}"
            );
        }
    }

    #[test]
    fn json_file_sink_writes_structured_records() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = LogConfig {
            stderr: false,
            ..LogConfig::default()
        };
        config.file = Some(FileLog::new(directory.path()));
        let guard = init(config).expect("subscriber installs once");

        tracing::info!(answer = 42, "file sink smoke");
        drop(guard);

        let entry = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "log"))
            .expect("daily log file exists");
        let text = std::fs::read_to_string(entry).unwrap();
        assert!(text.contains("\"answer\":42"), "structured field: {text}");
        assert!(text.contains("\"message\":\"file sink smoke\""), "{text}");
    }
}
