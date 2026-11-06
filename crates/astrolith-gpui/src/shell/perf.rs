//! Dev-only scripted driver for the native performance harness.
//!
//! A development build reads `ASTROLITH_PERF_SCENARIO` (a small JSON object)
//! and performs the listed actions against the real window, document sessions
//! and input adapter. Every action is logged with target `astrolith::perf`, so
//! `scripts/editor-perf.mjs` can align the in-app clock with the harness.
//!
//! This is not a user feature: only `cfg(debug_assertions)` builds start the
//! driver, and no action can rewrite project files.

use super::Shell;
use crate::editor::perf;
use astrolith_core::preferences::WorkspaceMode;
use gpui_kit::{Context, EntityInputHandler as _, Window};
use serde::Deserialize;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct Scenario {
    #[serde(default = "default_settle")]
    settle_ms: u64,
    #[serde(default)]
    wait_document_ms: u64,
    #[serde(default)]
    steps: Vec<Step>,
}

fn default_settle() -> u64 {
    1_000
}

#[derive(Deserialize)]
struct Step {
    #[serde(default)]
    wait_ms: u64,
    #[serde(default)]
    mode: Option<String>,
    /// Negative scroll translation, matching GPUI's scroll convention.
    #[serde(default)]
    scroll_y: Option<f32>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    quit: bool,
}

impl Shell {
    pub(super) fn start_perf_driver(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(raw) = std::env::var("ASTROLITH_PERF_SCENARIO") else {
            return;
        };
        let scenario = match serde_json::from_str::<Scenario>(&raw) {
            Ok(scenario) => scenario,
            Err(error) => {
                tracing::error!(%error, "invalid ASTROLITH_PERF_SCENARIO");
                return;
            }
        };
        tracing::info!(
            target: "astrolith::perf",
            event = "driver_start",
            steps = scenario.steps.len(),
            settle_ms = scenario.settle_ms,
            wait_document_ms = scenario.wait_document_ms,
            monotonic_ns = perf::monotonic_ns(),
            epoch_ms = perf::epoch_ms(),
            "perf driver"
        );
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(scenario.settle_ms))
                .await;
            if scenario.wait_document_ms > 0 {
                let deadline = Instant::now() + Duration::from_millis(scenario.wait_document_ms);
                loop {
                    let ready = this
                        .update(cx, |shell, cx| {
                            shell.project.read(cx).active_session(cx).is_some()
                        })
                        .unwrap_or(false);
                    if ready || Instant::now() >= deadline {
                        tracing::info!(
                            target: "astrolith::perf",
                            event = "driver_document",
                            ready,
                            monotonic_ns = perf::monotonic_ns(),
                            epoch_ms = perf::epoch_ms(),
                            "perf document wait finished"
                        );
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                }
            }
            for step in scenario.steps {
                if step.wait_ms > 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(step.wait_ms))
                        .await;
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        shell.apply_perf_step(step, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_perf_step(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let label = if let Some(mode) = step.mode.as_deref() {
            mode.to_owned()
        } else if step.input.is_some() {
            "input".to_owned()
        } else if step.scroll_y.is_some() {
            "scroll".to_owned()
        } else if step.quit {
            "quit".to_owned()
        } else {
            "wait".to_owned()
        };
        let started = Instant::now();
        let mut accepted = true;
        if let Some(mode) = step.mode.as_deref() {
            accepted = match mode {
                "writing" | "Writing" => {
                    self.set_mode(WorkspaceMode::Writing, window, cx);
                    true
                }
                "build" | "Build" => {
                    self.set_mode(WorkspaceMode::Build, window, cx);
                    true
                }
                "preview" | "Preview" => {
                    self.set_mode(WorkspaceMode::Preview, window, cx);
                    true
                }
                _ => false,
            };
        }
        let session = self.project.read(cx).active_session(cx);
        if let Some(y) = step.scroll_y
            && let Some(session) = session.clone()
        {
            session.update(cx, |session, cx| session.perf_scroll(y, cx));
        }
        if let Some(text) = step.input.as_deref()
            && let Some(session) = session
        {
            let writer = session.read(cx).writer.clone();
            writer.update(cx, |writer, cx| {
                writer.replace_text_in_range(None, text, window, cx);
            });
        }
        perf::span("driver_step", &label, started.elapsed());
        if step.quit {
            tracing::info!(
                target: "astrolith::perf",
                event = "driver_quit",
                monotonic_ns = perf::monotonic_ns(),
                epoch_ms = perf::epoch_ms(),
                "perf driver"
            );
            cx.quit();
        }
        accepted
    }
}
