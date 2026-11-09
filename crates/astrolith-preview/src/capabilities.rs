//! Preview capability detection.
//!
//! External-browser preview is always available. Embedded preview is an optional
//! build feature and is only offered where the platform can host it: the native
//! GPUI window must never be forced through XWayland just for a preview pane, so
//! every Linux Wayland session — including one that also exposes an X11
//! `DISPLAY` through XWayland — and every headless host are external-only.
//!
//! Detection is split from the platform facts ([`HostEnvironment`]) so every
//! branch is testable without changing the process environment or the target
//! triple. [`HostEnvironment::detect`] reads the real environment once and
//! [`PreviewCapability::detect`] is the convenience entry point; callers should
//! compute it at startup and treat an embedded launch failure as a recoverable
//! fallback to the external surface (M06.2 owns the real WebView surface).

/// How preview URLs can be presented on this host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewCapability {
    /// An embedded WebView is compiled in and this host can host it.
    Embedded,
    /// Only the external browser surface is available.
    ExternalOnly,
}

impl PreviewCapability {
    /// Detect the capability of the running host.
    ///
    /// `webview_available` is whether this build compiled the real embedded
    /// surface. Detection never probes a library installation: a WebView that
    /// cannot be created at runtime degrades to the external surface.
    pub fn detect_with(webview_available: bool) -> Self {
        HostEnvironment::detect_with(webview_available).capability()
    }

    /// Whether the optional embedded surface may be offered.
    pub fn is_embedded(self) -> bool {
        matches!(self, Self::Embedded)
    }
}

/// Why the embedded surface is not offered on this host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbeddedUnavailable {
    /// The build did not compile the embedded WebView surface.
    NotCompiled,
    /// A Wayland window cannot host the native WebView.
    Wayland,
    /// No display server was discovered.
    Headless,
    /// Any other target the WebView does not support
    UnsupportedPlatform,
}

impl std::fmt::Display for EmbeddedUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCompiled => formatter.write_str(
                "this build was compiled without the embedded WebView feature",
            ),
            Self::Wayland => formatter.write_str(
                "embedded WebView needs an X11 or XWayland window; this Wayland window is external-only",
            ),
            Self::Headless => formatter.write_str("no display server is available"),
            Self::UnsupportedPlatform => {
                formatter.write_str("the embedded WebView is not supported on this platform")
            }
        }
    }
}

/// Operating system family relevant to preview capability.
///
/// Linux carries the display-server detail because Wayland is the one case where
/// an available WebView must still not be offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPlatform {
    /// Linux with the detected display server.
    Linux {
        /// Wayland, X11 or no display at all.
        session: LinuxSession,
    },
    /// macOS, where WKWebView ships with the OS.
    MacOs,
    /// Windows, where WebView2 ships with the OS.
    Windows,
    /// Any other target.
    Other,
}

impl HostPlatform {
    /// Detect the running operating system and, on Linux, the display server.
    pub fn detect() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux {
                session: LinuxSession::detect(),
            }
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// Linux display-server family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxSession {
    /// A Wayland session; embedded preview stays off even under XWayland.
    Wayland,
    /// An X11 session, which can host a native embedded surface.
    X11,
    /// No display server was discovered.
    Headless,
}

impl LinuxSession {
    /// Detect from the process environment.
    pub fn detect() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Classify a session from a variable lookup.
    ///
    /// A lookup keeps the detection order testable without mutating process
    /// state. Wayland wins whenever `WAYLAND_DISPLAY` is set or
    /// `XDG_SESSION_TYPE` says `wayland`: an XWayland session exposes both,
    /// and choosing X11 there would silently require the compositor path the
    /// application explicitly avoids.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let session_type = lookup("XDG_SESSION_TYPE")
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let has_wayland_display = lookup("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty());
        let has_x11_display = lookup("DISPLAY").is_some_and(|value| !value.is_empty());

        // Match the window the application will actually use: GPUI prefers a
        // Wayland display when it is exported, then X11. An application started
        // with only `DISPLAY` in a Wayland session runs on XWayland, which can
        // host the WebView; `XDG_SESSION_TYPE` must not override that.
        if has_wayland_display {
            Self::Wayland
        } else if has_x11_display {
            Self::X11
        } else if session_type.as_deref() == Some("wayland") {
            Self::Wayland
        } else if session_type.as_deref() == Some("x11") {
            Self::X11
        } else {
            Self::Headless
        }
    }
}

/// The facts that decide preview capability.
///
/// `webview_available` is the compiled-in `embedded-webview` feature. It does
/// not probe the system for a working WebView library: a WebView that fails to
/// start must degrade to the external surface, not change the capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostEnvironment {
    /// Operating system and, on Linux, display server.
    pub platform: HostPlatform,
    /// Whether an embedded WebView surface is compiled in.
    pub webview_available: bool,
}

impl HostEnvironment {
    /// Build the facts explicitly (tests, startup configuration).
    pub fn new(platform: HostPlatform, webview_available: bool) -> Self {
        Self {
            platform,
            webview_available,
        }
    }

    /// Detect the running host and whether the build compiled the surface.
    pub fn detect_with(webview_available: bool) -> Self {
        Self::new(HostPlatform::detect(), webview_available)
    }

    /// Classify these facts.
    pub fn capability(self) -> PreviewCapability {
        if !self.webview_available {
            return PreviewCapability::ExternalOnly;
        }
        let platform_supports_embedded = match self.platform {
            // XWayland is still a Wayland session; headless has no surface.
            HostPlatform::Linux {
                session: LinuxSession::X11,
            } => true,
            HostPlatform::MacOs | HostPlatform::Windows => true,
            HostPlatform::Linux { .. } | HostPlatform::Other => false,
        };
        if platform_supports_embedded {
            PreviewCapability::Embedded
        } else {
            PreviewCapability::ExternalOnly
        }
    }

    /// Why embedded preview is unavailable, when it is.
    pub fn embedded_unavailable(self) -> Option<EmbeddedUnavailable> {
        if !self.webview_available {
            return Some(EmbeddedUnavailable::NotCompiled);
        }
        match self.platform {
            HostPlatform::Linux {
                session: LinuxSession::Wayland,
            } => Some(EmbeddedUnavailable::Wayland),
            HostPlatform::Linux {
                session: LinuxSession::Headless,
            } => Some(EmbeddedUnavailable::Headless),
            HostPlatform::Other => Some(EmbeddedUnavailable::UnsupportedPlatform),
            HostPlatform::Linux {
                session: LinuxSession::X11,
            }
            | HostPlatform::MacOs
            | HostPlatform::Windows => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let map: BTreeMap<&str, &str> = pairs.iter().copied().collect();
        move |name| map.get(name).map(|value| (*value).to_owned())
    }

    #[test]
    fn wayland_is_external_only_even_with_a_webview() {
        let host = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::Wayland,
            },
            true,
        );
        assert_eq!(host.capability(), PreviewCapability::ExternalOnly);
    }

    #[test]
    fn headless_linux_is_external_only() {
        let host = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::Headless,
            },
            true,
        );
        assert_eq!(host.capability(), PreviewCapability::ExternalOnly);
    }

    #[test]
    fn x11_with_a_webview_offers_embedded() {
        let host = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::X11,
            },
            true,
        );
        assert_eq!(host.capability(), PreviewCapability::Embedded);
    }

    #[test]
    fn x11_without_a_webview_falls_back_to_external() {
        let host = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::X11,
            },
            false,
        );
        assert_eq!(host.capability(), PreviewCapability::ExternalOnly);
    }

    #[test]
    fn macos_and_windows_use_their_system_webview() {
        for platform in [HostPlatform::MacOs, HostPlatform::Windows] {
            assert_eq!(
                HostEnvironment::new(platform, true).capability(),
                PreviewCapability::Embedded
            );
            assert_eq!(
                HostEnvironment::new(platform, false).capability(),
                PreviewCapability::ExternalOnly
            );
        }
    }

    #[test]
    fn other_platforms_are_external_only() {
        assert_eq!(
            HostEnvironment::new(HostPlatform::Other, true).capability(),
            PreviewCapability::ExternalOnly
        );
    }

    #[test]
    fn xwayland_prefers_wayland_over_x11() {
        let session = LinuxSession::from_lookup(lookup_from(&[
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("DISPLAY", ":0"),
            ("XDG_SESSION_TYPE", "x11"),
        ]));
        assert_eq!(session, LinuxSession::Wayland);
    }

    #[test]
    fn an_xwayland_window_is_x11_even_in_a_wayland_session() {
        // GPUI selects Wayland from WAYLAND_DISPLAY, then X11 from DISPLAY. A
        // launch with only DISPLAY in a Wayland session draws through XWayland,
        // so the WebView surface is allowed there.
        let session = LinuxSession::from_lookup(lookup_from(&[
            ("DISPLAY", ":1"),
            ("XDG_SESSION_TYPE", "wayland"),
        ]));
        assert_eq!(session, LinuxSession::X11);
    }

    #[test]
    fn session_type_selects_wayland_without_a_display_variable() {
        let session = LinuxSession::from_lookup(lookup_from(&[("XDG_SESSION_TYPE", "wayland")]));
        assert_eq!(session, LinuxSession::Wayland);
    }

    #[test]
    fn session_type_selects_x11_without_a_display_variable() {
        let session = LinuxSession::from_lookup(lookup_from(&[("XDG_SESSION_TYPE", "x11")]));
        assert_eq!(session, LinuxSession::X11);
    }

    #[test]
    fn blank_values_do_not_count_as_a_display() {
        let session = LinuxSession::from_lookup(lookup_from(&[
            ("WAYLAND_DISPLAY", ""),
            ("DISPLAY", ""),
            ("XDG_SESSION_TYPE", "  "),
        ]));
        assert_eq!(session, LinuxSession::Headless);
    }

    #[test]
    fn missing_variables_are_headless() {
        let session = LinuxSession::from_lookup(lookup_from(&[]));
        assert_eq!(session, LinuxSession::Headless);
    }

    #[test]
    fn unavailability_names_the_blocking_reason() {
        let wayland = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::Wayland,
            },
            true,
        );
        assert_eq!(
            wayland.embedded_unavailable(),
            Some(EmbeddedUnavailable::Wayland)
        );
        let headless = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::Headless,
            },
            true,
        );
        assert_eq!(
            headless.embedded_unavailable(),
            Some(EmbeddedUnavailable::Headless)
        );
        let not_compiled = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::X11,
            },
            false,
        );
        assert_eq!(
            not_compiled.embedded_unavailable(),
            Some(EmbeddedUnavailable::NotCompiled)
        );
        let x11 = HostEnvironment::new(
            HostPlatform::Linux {
                session: LinuxSession::X11,
            },
            true,
        );
        assert_eq!(x11.embedded_unavailable(), None);
    }
}
