//! Preview lifecycle reconciliation and stale-generation rejection.
//!
//! The preview panel can have several asynchronous operations in flight: a
//! start, a stop, a restart, an exit watcher and a project switch. A result
//! that arrives after its operation was superseded must never change the
//! current state. This module owns that generation rule and the decision about
//! whether a project-lifecycle change may reuse the running dev server.
//!
//! Reuse is keyed by the project **root**, not the generated project id: the
//! workspace issues a new id for every open, while the same directory keeps the
//! same project-local `node_modules` and dev server.

use std::path::PathBuf;

use astrolith_core::ProjectId;
use url::Url;

use crate::server::ServerStatus;

/// The project a preview panel is attached to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewTarget {
    project: ProjectId,
    root: PathBuf,
}

impl PreviewTarget {
    /// A project bound to its canonical-ish root path.
    pub fn new(project: ProjectId, root: impl Into<PathBuf>) -> Self {
        Self {
            project,
            root: root.into(),
        }
    }

    /// Stable project identity for this open.
    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    /// Project root the dev server runs in.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

/// What a project-lifecycle change requires of the preview panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetChange {
    /// The open project did not change (same root).
    Unchanged,
    /// No server was attached and a project is now available.
    Attach,
    /// A different root is open: the old server must stop before rebinding.
    Retarget,
    /// No project remains: the server must stop.
    Detach,
}

/// Decide whether the running server may be reused for `next`.
///
/// Only the same project root is reusable; a different root always retargets.
pub fn target_change(
    current: Option<&PreviewTarget>,
    next: Option<&PreviewTarget>,
) -> TargetChange {
    match (current, next) {
        (None, None) => TargetChange::Unchanged,
        (None, Some(_)) => TargetChange::Attach,
        (Some(_), None) => TargetChange::Detach,
        (Some(current), Some(next)) if current.root == next.root => TargetChange::Unchanged,
        (Some(_), Some(_)) => TargetChange::Retarget,
    }
}

/// Generation-checked dev-server state.
///
/// Every operation that can outlive its trigger takes a ticket from a
/// [`PreviewLifecycle`] and applies its result through the matching method.
/// A stale ticket is rejected without touching the current status.
#[derive(Clone, Debug)]
pub struct PreviewLifecycle {
    generation: u64,
    status: ServerStatus,
}

impl Default for PreviewLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewLifecycle {
    /// A stopped lifecycle at generation zero.
    pub fn new() -> Self {
        Self {
            generation: 0,
            status: ServerStatus::Stopped,
        }
    }

    /// Current status.
    pub fn status(&self) -> &ServerStatus {
        &self.status
    }

    /// Current generation ticket.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether `generation` is still the live one.
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    fn next(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Invalidate outstanding results without changing the status.
    ///
    /// Used when an exit watcher is abandoned or a panel is detached.
    pub fn invalidate(&mut self) -> u64 {
        self.next()
    }

    /// Reset to stopped for a project open/rebind.
    pub fn attach(&mut self) -> u64 {
        self.status = ServerStatus::Stopped;
        self.next()
    }

    /// Reset to stopped for a project close or application exit.
    pub fn detach(&mut self) -> u64 {
        self.status = ServerStatus::Stopped;
        self.next()
    }

    /// Begin a start operation and return its ticket.
    pub fn begin_start(&mut self) -> u64 {
        self.status = ServerStatus::Starting;
        self.next()
    }

    /// Begin a stop operation and return its ticket.
    pub fn begin_stop(&mut self) -> u64 {
        self.status = ServerStatus::Stopping;
        self.next()
    }

    /// Begin a restart operation and return its ticket.
    ///
    /// The intermediate stop is not observable as `stopping`: a restart is one
    /// user action, and the panel reports a single starting phase.
    pub fn begin_restart(&mut self) -> u64 {
        self.status = ServerStatus::Starting;
        self.next()
    }

    /// Apply a ready result if its generation is current.
    pub fn ready(&mut self, generation: u64, url: Url) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.status = ServerStatus::Ready(url);
        true
    }

    /// Apply a failure result if its generation is current.
    pub fn failed(&mut self, generation: u64, message: impl Into<String>) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.status = ServerStatus::Failed(message.into());
        true
    }

    /// Apply a stopped result if its generation is current.
    pub fn stopped(&mut self, generation: u64) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.status = ServerStatus::Stopped;
        true
    }

    /// Apply an unexpected-exit observation if its generation is current.
    ///
    /// Only terminal statuses are accepted; a watcher may never move a stopped
    /// generation back to ready.
    pub fn observed_exit(&mut self, generation: u64, status: ServerStatus) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        if !matches!(
            status,
            ServerStatus::Stopped | ServerStatus::Failed(_) | ServerStatus::Ready(_)
        ) {
            return false;
        }
        self.status = status;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(project: &str, root: &str) -> PreviewTarget {
        PreviewTarget::new(ProjectId::new(project).unwrap(), root)
    }

    fn url(port: u16) -> Url {
        Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap()
    }

    #[test]
    fn a_new_project_id_reuses_the_server_only_for_the_same_root() {
        let first = target("workspace-1", "/tmp/site");
        let reopened = target("workspace-2", "/tmp/site");
        let other = target("workspace-3", "/tmp/other");

        assert_eq!(target_change(None, None), TargetChange::Unchanged);
        assert_eq!(target_change(None, Some(&first)), TargetChange::Attach);
        assert_eq!(target_change(Some(&first), None), TargetChange::Detach);
        assert_eq!(
            target_change(Some(&first), Some(&reopened)),
            TargetChange::Unchanged
        );
        assert_eq!(
            target_change(Some(&first), Some(&other)),
            TargetChange::Retarget
        );
    }

    #[test]
    fn a_late_ready_from_an_older_generation_is_rejected() {
        let mut lifecycle = PreviewLifecycle::new();
        let stale = lifecycle.begin_start();
        let current = lifecycle.begin_restart();

        assert!(lifecycle.ready(current, url(4322)));
        assert!(!lifecycle.ready(stale, url(4321)));
        assert_eq!(lifecycle.status().url(), Some(&url(4322)));
    }

    #[test]
    fn a_stopped_generation_cannot_be_resurrected() {
        let mut lifecycle = PreviewLifecycle::new();
        let start = lifecycle.begin_start();
        let stop = lifecycle.begin_stop();

        assert!(lifecycle.stopped(stop));
        assert!(!lifecycle.ready(start, url(4321)));
        assert!(!lifecycle.failed(start, "late failure"));
        assert_eq!(lifecycle.status(), &ServerStatus::Stopped);
    }

    #[test]
    fn detaching_rejects_results_from_the_previous_generation() {
        let mut lifecycle = PreviewLifecycle::new();
        let start = lifecycle.begin_start();
        lifecycle.detach();
        assert!(!lifecycle.ready(start, url(4321)));
        assert_eq!(lifecycle.status(), &ServerStatus::Stopped);
    }

    #[test]
    fn an_exit_watcher_only_applies_to_its_own_generation() {
        let mut lifecycle = PreviewLifecycle::new();
        let first = lifecycle.begin_start();
        assert!(lifecycle.ready(first, url(4321)));
        let watcher_generation = lifecycle.generation();
        let restart = lifecycle.begin_restart();
        assert!(lifecycle.ready(restart, url(4322)));

        assert!(!lifecycle.observed_exit(watcher_generation, ServerStatus::Stopped));
        assert_eq!(lifecycle.status().url(), Some(&url(4322)));
    }
}
