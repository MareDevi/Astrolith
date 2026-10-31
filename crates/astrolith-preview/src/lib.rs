//! Astro preview services: dev-server lifecycle and process ownership.
//!
//! This crate is GUI-free domain/service code. It owns the project-local Astro
//! development server as a supervised child process: both output streams are
//! drained continuously into a bounded (1 MiB per stream) log, startup and stop
//! are bounded by deadlines, and project switch/exit never leaves orphaned Astro
//! descendants behind.
//!
//! The binding process contract lives in
//! `docs/technical/concurrency-and-responsiveness.md` (M05).

pub mod process;
pub mod server;

pub use process::{OUTPUT_CAP, OutputCapture, ProcessError, RunningProcess, SpawnSpec};
pub use server::{
    DEFAULT_STARTUP_TIMEOUT, DEFAULT_STOP_GRACE, DevServer, DevServerConfig, DevServerError,
    ServerStatus,
};

#[cfg(test)]
mod test_support;
