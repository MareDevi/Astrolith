//! Astro preview services: dev-server lifecycle, process ownership, and entry
//! route resolution.
//!
//! This crate is GUI-free domain/service code. It owns the project-local Astro
//! development server as a supervised child process: both output streams are
//! drained continuously into a bounded (1 MiB per stream) log, startup and stop
//! are bounded by deadlines, and project switch/exit never leaves orphaned Astro
//! descendants behind. It also maps logical entries to preview URLs.
//!
//! The binding process contract lives in
//! `docs/technical/concurrency-and-responsiveness.md` (M05).

pub mod config;
pub mod process;
pub mod routes;
pub mod server;

pub use config::{ConfigError, RouteMapping, RouteRule};
pub use process::{OUTPUT_CAP, OutputCapture, ProcessError, RunningProcess, SpawnSpec};
pub use routes::{RouteError, resolve_entry_url};
pub use server::{
    DEFAULT_STARTUP_TIMEOUT, DEFAULT_STOP_GRACE, DevServer, DevServerConfig, DevServerError,
    ServerStatus,
};

#[cfg(test)]
mod test_support;
