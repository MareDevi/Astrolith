# M06 — Hybrid preview and platform integration progress

Started: 2026-10-31. Scope: M06.1 preview capability detection, M06.2 embedded WebView
spike and M06.3 platform integration. The original product specification remains absent;
this note follows the implementation plan and the concurrency amendment.

**Review status: M06.1 not yet independently reviewed.** M05's owed review is tracked in
`m05-progress.md`.

## Contract

Embedded preview must stay optional and must never become a prerequisite for normal
editing or external preview. On Linux Wayland the application must not switch the native
window to XWayland for the sake of an embedded pane, so Wayland is external-only by
policy, not by fallback after a failure.

## M06.1 — Preview capability detection

`crates/astrolith-preview/src/capabilities.rs`:

- `PreviewCapability::{Embedded, ExternalOnly}` with `is_embedded()`.
- `HostEnvironment { platform, webview_available }` is the complete input to the
  decision, so classification is a pure function. `HostEnvironment::detect()` reads the
  real host once; `PreviewCapability::detect()` is the startup convenience.
- `HostPlatform::{Linux { session }, MacOs, Windows, Other}` and
  `LinuxSession::{Wayland, X11, Headless}`.
- `LinuxSession::from_lookup` classifies `WAYLAND_DISPLAY`, `DISPLAY` and
  `XDG_SESSION_TYPE`. Wayland wins when both Wayland and X11 variables are present
  (XWayland), because the conservative answer keeps the native Wayland window. Blank
  values are treated as absent; anything without a display variable is headless.
- Decision table: embedded requires `webview_available` **and** (Linux X11 | macOS |
  Windows). Wayland, headless Linux, other targets and all default builds are
  external-only.
- `webview_available` comes from the new `embedded-webview` feature on
  `astrolith-preview`, which is off by default and compiles no WebView dependency yet.
  It is the switch M06.2 will attach the real surface to. A compiled-in WebView that
  fails at launch must degrade to the external surface at runtime; capability detection
  does not probe system libraries.
- 14 focused tests: Wayland (with and without a WebView), X11 with/without, headless,
  macOS, Windows, unknown targets, XWayland precedence, session-type-only detection,
  blank values, missing variables, and a default-build test asserting detection never
  claims embedded.

## Verification

- `cargo test -p astrolith-preview --locked capabilities`: 14 passed.
- `mise run check`: see the commit message for the recorded result.

## Known limits

1. Embedded preview is not implemented; `embedded-webview` is an empty feature gate until
   M06.2.
2. Capability is per-process, computed from the environment at call time. A session
   change (for example restarting under a different display server) requires a restart;
   that matches the native window's lifetime.
3. Wayland with a functioning WebView is still external-only by design. Supporting an
   embedded Wayland WebView requires a real-platform spike and an explicit product
   decision, not a detection change.
4. X11 classification under a Wayland compositor relies on `WAYLAND_DISPLAY` being
   exported to child processes, which compositors do for XWayland clients.
