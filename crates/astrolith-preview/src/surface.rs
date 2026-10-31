//! Preview surfaces: where a resolved entry URL is presented.
//!
//! A surface is deliberately tiny and synchronous. Opening a browser must never
//! block the UI thread, so the external implementation only *spawns* a detached
//! platform opener and returns; the process is not awaited.
//!
//! The embedded WebView surface (M06) is a separate, optional implementation of
//! the same trait, so callers do not depend on any particular presentation.

use url::Url;

/// Failure while presenting a preview URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceError {
    /// The URL is not an absolute `http(s)` URL and must not be launched.
    InvalidUrl {
        /// The rejected URL.
        url: String,
    },
    /// This platform has no known browser opener.
    UnsupportedPlatform {
        /// The target operating system.
        platform: String,
    },
    /// The opener command could not be started.
    LaunchFailed {
        /// The command that failed.
        command: String,
        /// Why it failed.
        message: String,
    },
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl { url } => {
                write!(formatter, "`{url}` is not a launchable http(s) URL")
            }
            Self::UnsupportedPlatform { platform } => {
                write!(formatter, "no browser opener is known for `{platform}`")
            }
            Self::LaunchFailed { command, message } => {
                write!(formatter, "could not launch `{command}`: {message}")
            }
        }
    }
}

impl std::error::Error for SurfaceError {}

/// Refuse anything that is not an absolute `http(s)` URL.
///
/// `file:` and custom schemes would hand arbitrary local paths to the desktop
/// opener, so they are rejected rather than launched.
pub fn validate_preview_url(url: &Url) -> Result<(), SurfaceError> {
    if matches!(url.scheme(), "http" | "https") && url.host().is_some() {
        Ok(())
    } else {
        Err(SurfaceError::InvalidUrl {
            url: url.to_string(),
        })
    }
}

/// A place a preview URL can be presented.
pub trait PreviewSurface {
    /// Present `url`. Implementations must not block on the presentation itself.
    fn open(&self, url: &Url) -> Result<(), SurfaceError>;
}

/// A surface that records what it was asked to open, for tests and headless use.
#[derive(Clone, Debug, Default)]
pub struct RecordingSurface {
    opened: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    fail_with: Option<SurfaceError>,
}

impl RecordingSurface {
    /// A surface that accepts every valid URL.
    pub fn new() -> Self {
        Self::default()
    }

    /// A surface whose `open` always fails with `error`.
    pub fn failing(error: SurfaceError) -> Self {
        Self {
            opened: std::sync::Arc::default(),
            fail_with: Some(error),
        }
    }

    /// URLs handed to [`PreviewSurface::open`], in order.
    pub fn opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .map(|urls| urls.clone())
            .unwrap_or_default()
    }
}

impl PreviewSurface for RecordingSurface {
    fn open(&self, url: &Url) -> Result<(), SurfaceError> {
        validate_preview_url(url)?;
        if let Some(error) = self.fail_with.clone() {
            return Err(error);
        }
        if let Ok(mut opened) = self.opened.lock() {
            opened.push(url.to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_urls() {
        for raw in [
            "file:///etc/passwd",
            "mailto:dev@example.test",
            "javascript:alert(1)",
        ] {
            let url = Url::parse(raw).unwrap();
            assert_eq!(
                validate_preview_url(&url),
                Err(SurfaceError::InvalidUrl {
                    url: url.to_string()
                }),
                "{raw} must be refused"
            );
        }
    }

    #[test]
    fn accepts_http_and_https_urls() {
        for raw in ["http://127.0.0.1:4321/docs/intro", "https://example.test/"] {
            let url = Url::parse(raw).unwrap();
            assert!(validate_preview_url(&url).is_ok(), "{raw} must be allowed");
        }
    }

    #[test]
    fn recording_surface_captures_and_validates() {
        let surface = RecordingSurface::new();
        let url = Url::parse("http://127.0.0.1:4321/docs/intro").unwrap();
        surface.open(&url).unwrap();
        assert_eq!(surface.opened(), vec![url.to_string()]);

        let bad = Url::parse("file:///tmp/x").unwrap();
        assert!(matches!(
            surface.open(&bad),
            Err(SurfaceError::InvalidUrl { .. })
        ));
        assert_eq!(surface.opened().len(), 1, "invalid URLs are not recorded");
    }

    #[test]
    fn failing_surface_reports_the_error_and_records_nothing() {
        let surface = RecordingSurface::failing(SurfaceError::LaunchFailed {
            command: "xdg-open".into(),
            message: "not found".into(),
        });
        let url = Url::parse("http://127.0.0.1:4321/").unwrap();
        assert!(matches!(
            surface.open(&url),
            Err(SurfaceError::LaunchFailed { .. })
        ));
        assert!(surface.opened().is_empty());
    }
}
