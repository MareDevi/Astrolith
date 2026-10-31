//! External-browser preview surface.
//!
//! The launcher only *spawns* the platform opener and returns immediately: a
//! browser process must never be awaited from a service call, and the desktop
//! opener may stay alive for as long as the browser window is open.
//!
//! Nothing here executes the Astro project itself; the caller starts the dev
//! server separately and passes the resolved preview URL.

use std::process::{Command, Stdio};

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
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|error| error.to_string())
    }
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
    validate_preview_url(url)?;
    let url = url.to_string();
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

    /// A browser surface using `opener` (tests inject a fake).
    pub fn with_opener(opener: Box<dyn Opener>) -> Self {
        Self { opener }
    }

    /// Spawn the opener for `url` without waiting for it.
    pub fn launch(&self, url: &Url) -> Result<OpenCommand, SurfaceError> {
        let command = open_command(url)?;
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
}
