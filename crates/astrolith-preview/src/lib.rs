//! Astro preview services: dev-server lifecycle, process ownership, entry route
//! resolution and external presentation.
//!
//! This crate is GUI-free domain/service code. It owns the project-local Astro
//! development server as a supervised child process: both output streams are
//! drained continuously into a bounded (1 MiB per stream) log, startup and stop
//! are bounded by deadlines, and project switch/exit never leaves orphaned Astro
//! It also maps logical entries to preview URLs and hands
//! them to a preview surface.
//!
//! Embedded preview is optional and build-gated: [`PreviewCapability`] reports
//! whether the current host and build may present it, and Linux Wayland always
//! falls back to the external browser.
//!
//! The binding process contract lives in
//! `docs/technical/concurrency-and-responsiveness.md` (M05).

pub mod capabilities;
pub mod config;
pub mod external;
pub mod preview;
pub mod process;
pub mod routes;
pub mod server;
pub mod surface;

pub use capabilities::{HostEnvironment, HostPlatform, LinuxSession, PreviewCapability};
pub use config::{ConfigError, RouteMapping, RouteRule};
pub use external::{ExternalBrowser, OpenCommand, Opener, SystemOpener, open_command};
pub use preview::{PreviewError, PreviewSession};
pub use process::{OUTPUT_CAP, OutputCapture, ProcessError, RunningProcess, SpawnSpec};
pub use routes::{RouteError, resolve_entry_url};
pub use server::{
    DEFAULT_STARTUP_TIMEOUT, DEFAULT_STOP_GRACE, DevServer, DevServerConfig, DevServerError,
    ServerStatus,
};
pub use surface::{PreviewSurface, RecordingSurface, SurfaceError, validate_preview_url};

#[cfg(test)]
mod test_support;
