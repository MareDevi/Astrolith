//! External-browser preview surface.
//!
//! The launcher only *spawns* the platform opener and returns immediately: a
//! browser process must never be awaited from a service call, and the desktop
//! opener may stay alive for as long as the browser window is open.
//!
//! Nothing here executes the Astro project itself; the caller starts the dev
//! server separately and passes the resolved preview URL.

use std::process::{Command, Stdio};

use astrolith_core::preferences::BrowserCommand;
use url::Url;

use crate::surface::{PreviewSurface, SurfaceError, validate_preview_url};

/// The program and arguments used to hand a URL to the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenCommand {
    /// Program to run.
    pub program: String,
    /// Arguments, including the URL itself.
    pub args: Vec<String>,
}

impl OpenCommand {
    /// Render the command for diagnostics.
    pub fn display(&self) -> String {
        let mut rendered = self.program.clone();
        for argument in &self.args {
            rendered.push(' ');
            rendered.push_str(argument);
        }
        rendered
    }
}

/// Spawns an opener program. Abstracted so tests never launch a real browser.
pub trait Opener: Send + Sync {
    /// Start `program` with `args` detached from the current process.
    fn spawn(&self, program: &str, args: &[String]) -> Result<(), String>;
}

/// The real desktop opener, used outside tests.
#[derive(Debug, Default)]
pub struct SystemOpener;

impl Opener for SystemOpener {
    fn spawn(&self, program: &str, args: &[String]) -> Result<(), String> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        let launched = spawn_reaped(&mut command).map(drop);
        #[cfg(not(unix))]
        let launched = command.spawn().map(drop);
        launched.map_err(|error| error.to_string())
    }
}

/// Start `command` and reap the child on a detached thread.
///
/// Launch stays fire-and-forget: this returns as soon as the process starts, and
/// the caller never awaits the opener. The thread waits for the opener to exit
/// so an exited `xdg-open` cannot linger as a zombie, then ends. It returns the
/// child id and a receiver that fires once the child has been reaped, which lets
/// tests observe the wait without retaining the child itself.
#[cfg(unix)]
fn spawn_reaped(command: &mut Command) -> std::io::Result<(u32, std::sync::mpsc::Receiver<()>)> {
    use std::sync::mpsc::channel;

    let (finished_tx, finished_rx) = channel();
    let (child_tx, child_rx) = channel::<std::process::Child>();
    std::thread::Builder::new()
        .name("opener-reaper".to_owned())
        .spawn(move || {
            if let Ok(mut child) = child_rx.recv() {
                let _ = child.wait();
            }
            let _ = finished_tx.send(());
        })?;
    let child = command.spawn()?;
    let pid = child.id();
    if let Err(std::sync::mpsc::SendError(mut child)) = child_tx.send(child) {
        // The reaper thread disappeared before it could take ownership; reap
        // here so no zombie remains.
        let _ = child.wait();
    }
    Ok((pid, finished_rx))
}

/// The platform opener command for `url`.
///
/// - macOS: `open <url>`
/// - Windows: `rundll32.exe url.dll,FileProtocolHandler <url>`
/// - other Unix/supported desktops: `xdg-open <url>`
///
/// Windows deliberately does not use `cmd /C start`: `cmd` re-parses `&`, `|`,
/// `^`, parentheses and `%`, and entry ids legitimately contain characters such
/// as `&` that URL encoding leaves intact. `rundll32` receives the URL as one
/// argument with no shell in between.
pub fn open_command(url: &Url) -> Result<OpenCommand, SurfaceError> {
    open_command_with(url, None)
}

/// The opener command for `url`, using `preferred` when it names an executable.
///
/// The preferred executable is started directly with its argv: no shell parses
/// the arguments. Every argument may contain `{url}`; when none does, the URL is
/// appended as the final argument. An unconfigured browser uses the platform
/// opener.
pub fn open_command_with(
    url: &Url,
    preferred: Option<&BrowserCommand>,
) -> Result<OpenCommand, SurfaceError> {
    validate_preview_url(url)?;
    let url = url.to_string();
    if let Some(preferred) = preferred.filter(|browser| browser.is_configured()) {
        let mut args: Vec<String> = preferred
            .arguments
            .iter()
            .map(|argument| argument.replace("{url}", &url))
            .collect();
        if !preferred
            .arguments
            .iter()
            .any(|argument| argument.contains("{url}"))
        {
            args.push(url);
        }
        return Ok(OpenCommand {
            program: preferred.executable.trim().to_owned(),
            args,
        });
    }
    let command = if cfg!(target_os = "macos") {
        OpenCommand {
            program: "open".to_owned(),
            args: vec![url],
        }
    } else if cfg!(windows) {
        OpenCommand {
            program: "rundll32.exe".to_owned(),
            args: vec!["url.dll,FileProtocolHandler".to_owned(), url],
        }
    } else if cfg!(unix) {
        OpenCommand {
            program: "xdg-open".to_owned(),
            args: vec![url],
        }
    } else {
        return Err(SurfaceError::UnsupportedPlatform {
            platform: std::env::consts::OS.to_owned(),
        });
    };
    Ok(command)
}

/// A [`PreviewSurface`] that hands preview URLs to the system browser.
pub struct ExternalBrowser {
    opener: Box<dyn Opener>,
    preferred: Option<BrowserCommand>,
}

impl std::fmt::Debug for ExternalBrowser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ExternalBrowser")
    }
}

impl Default for ExternalBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalBrowser {
    /// A browser surface backed by the real desktop opener.
    pub fn new() -> Self {
        Self::with_opener(Box::new(SystemOpener))
    }

    /// A browser surface using a configured browser argv.
    pub fn with_preferred(preferred: BrowserCommand) -> Self {
        Self {
            opener: Box::new(SystemOpener),
            preferred: Some(preferred),
        }
    }

    /// A browser surface using `opener` (tests inject a fake).
    pub fn with_opener(opener: Box<dyn Opener>) -> Self {
        Self {
            opener,
            preferred: None,
        }
    }

    /// A browser surface using `preferred` and `opener` (tests).
    pub fn with_preferred_opener(preferred: BrowserCommand, opener: Box<dyn Opener>) -> Self {
        Self {
            opener,
            preferred: Some(preferred),
        }
    }

    /// Spawn the opener for `url` without waiting for it.
    pub fn launch(&self, url: &Url) -> Result<OpenCommand, SurfaceError> {
        let command = open_command_with(url, self.preferred.as_ref())?;
        self.opener
            .spawn(&command.program, &command.args)
            .map_err(|message| SurfaceError::LaunchFailed {
                command: command.display(),
                message,
            })?;
        Ok(command)
    }
}

impl PreviewSurface for ExternalBrowser {
    fn open(&self, url: &Url) -> Result<(), SurfaceError> {
        self.launch(url).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// One recorded `(program, args)` invocation.
    type Invocation = (String, Vec<String>);

    #[derive(Default)]
    struct FakeOpener {
        calls: Arc<Mutex<Vec<Invocation>>>,
        fail: Option<String>,
    }

    impl Opener for FakeOpener {
        fn spawn(&self, program: &str, args: &[String]) -> Result<(), String> {
            if let Some(message) = &self.fail {
                return Err(message.clone());
            }
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((program.to_owned(), args.to_vec()));
            }
            Ok(())
        }
    }

    fn url() -> Url {
        Url::parse("http://127.0.0.1:4321/docs/intro").unwrap()
    }

    #[test]
    fn invalid_url_never_reaches_the_opener() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let browser = ExternalBrowser::with_opener(Box::new(FakeOpener {
            calls: calls.clone(),
            fail: None,
        }));
        let bad = Url::parse("file:///etc/passwd").unwrap();
        assert!(matches!(
            browser.launch(&bad),
            Err(SurfaceError::InvalidUrl { .. })
        ));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn launches_the_platform_command_with_the_url() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let browser = ExternalBrowser::with_opener(Box::new(FakeOpener {
            calls: calls.clone(),
            fail: None,
        }));
        let command = browser.launch(&url()).unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, command.program);
        assert_eq!(calls[0].1, command.args);
        assert!(
            calls[0].1.iter().any(|argument| argument == url().as_str()),
            "the URL must be passed through"
        );
    }

    #[test]
    fn opener_failure_is_reported_with_the_command() {
        let browser = ExternalBrowser::with_opener(Box::new(FakeOpener {
            calls: Arc::default(),
            fail: Some("no such file or directory".into()),
        }));
        match browser.launch(&url()) {
            Err(SurfaceError::LaunchFailed { command, message }) => {
                assert!(!command.is_empty());
                assert_eq!(message, "no such file or directory");
            }
            other => panic!("expected LaunchFailed, got {other:?}"),
        }
    }

    #[test]
    fn preferred_browser_argv_is_used_without_a_shell() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let preferred = BrowserCommand {
            executable: "firefox".into(),
            arguments: vec!["--new-window".into(), "{url}".into()],
        };
        let browser = ExternalBrowser::with_preferred_opener(
            preferred,
            Box::new(FakeOpener {
                calls: calls.clone(),
                fail: None,
            }),
        );
        let command = browser.launch(&url()).unwrap();
        assert_eq!(command.program, "firefox");
        assert_eq!(
            command.args,
            vec!["--new-window".to_owned(), url().to_string()]
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].0, "firefox");
        assert_eq!(calls[0].1, command.args);
    }

    #[test]
    fn preferred_browser_without_a_placeholder_appends_the_url_once() {
        let command = open_command_with(
            &url(),
            Some(&BrowserCommand {
                executable: "chromium".into(),
                arguments: vec!["--incognito".into()],
            }),
        )
        .unwrap();
        assert_eq!(
            command.args,
            vec!["--incognito".to_owned(), url().to_string()]
        );
    }

    #[test]
    fn an_empty_preferred_executable_uses_the_platform_opener() {
        let preferred = BrowserCommand {
            executable: "  ".into(),
            arguments: vec!["--flag".into()],
        };
        let command = open_command_with(&url(), Some(&preferred)).unwrap();
        assert_eq!(command, open_command(&url()).unwrap());
    }

    #[test]
    fn preferred_browser_never_receives_an_invalid_url() {
        let preferred = BrowserCommand {
            executable: "firefox".into(),
            arguments: vec![],
        };
        let bad = Url::parse("file:///etc/passwd").unwrap();
        assert!(matches!(
            open_command_with(&bad, Some(&preferred)),
            Err(SurfaceError::InvalidUrl { .. })
        ));
    }

    #[test]
    fn platform_command_matches_the_target() {
        let command = open_command(&url()).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(command.program, "open");
        } else if cfg!(windows) {
            assert_eq!(command.program, "rundll32.exe");
            assert_eq!(command.args[0], "url.dll,FileProtocolHandler");
        } else {
            assert_eq!(command.program, "xdg-open");
        }
        assert!(command.args.contains(&url().to_string()));
    }

    #[test]
    fn reserved_characters_stay_in_a_single_argument() {
        let command =
            open_command(&Url::parse("http://127.0.0.1:4321/docs/Q&A%20notes").expect("valid url"))
                .expect("a loopback url launches");
        let matching: Vec<_> = command
            .args
            .iter()
            .filter(|argument| argument.contains("Q&A"))
            .collect();
        assert_eq!(matching.len(), 1, "args were {:?}", command.args);
        assert_eq!(matching[0], "http://127.0.0.1:4321/docs/Q&A%20notes");
    }

    #[test]
    fn surface_trait_delegates_to_launch() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let browser = ExternalBrowser::with_opener(Box::new(FakeOpener {
            calls: calls.clone(),
            fail: None,
        }));
        browser.open(&url()).unwrap();
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    /// A launched opener must be waited on so it never lingers as a zombie.
    #[cfg(unix)]
    #[test]
    fn system_opener_reaps_the_child_it_starts() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "exit 7"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let (pid, finished) = spawn_reaped(&mut command).unwrap();
        assert!(pid > 0);
        finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the reaper thread must wait for the opener to exit");
        // A completed `wait` is what removes the process-table entry; a dropped
        // child would leave the reaper receiver waiting until process exit.
    }

    /// The production entry point still launches without waiting for the opener.
    #[cfg(unix)]
    #[test]
    fn system_opener_launches_a_short_lived_stub() {
        SystemOpener
            .spawn("sh", &["-c".to_owned(), "exit 0".to_owned()])
            .expect("a stub opener launches");
    }
}
