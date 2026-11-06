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
use gpui_kit::component::Theme;
use gpui_kit::{Context, EntityInputHandler as _, Window, px, size};
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
    /// Negative horizontal scroll translation. Absent means zero.
    #[serde(default)]
    scroll_x: Option<f32>,
    /// `end` or `start`, placing the caret in the active editor.
    #[serde(default)]
    caret: Option<String>,
    /// Toggle code folding on the active editor.
    #[serde(default)]
    set_folding: Option<bool>,
    /// Toggle soft wrap on the active editor.
    #[serde(default)]
    set_soft_wrap: Option<bool>,
    /// Change the theme's monospace size, invalidating source metrics.
    #[serde(default)]
    mono_font_size: Option<f32>,
    /// Change the theme's monospace family, invalidating source metrics.
    #[serde(default)]
    mono_font_family: Option<String>,
    /// Resize the native window to this content size.
    #[serde(default)]
    resize_width: Option<f32>,
    #[serde(default)]
    resize_height: Option<f32>,
    #[serde(default)]
    input: Option<String>,
    /// Open build search with this query.
    #[serde(default)]
    find_query: Option<String>,
    /// Steps to the next search match this many times.
    #[serde(default)]
    find_next: Option<u32>,
    /// Replace every current search match with this text.
    #[serde(default)]
    find_replace_all: Option<String>,
    /// Close the search bar.
    #[serde(default)]
    find_close: bool,
    /// Close the active document. Only valid while it has no unsaved edits.
    #[serde(default)]
    close: bool,
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
        } else if step.find_query.is_some() {
            "find_query".to_owned()
        } else if step.find_next.is_some() {
            "find_next".to_owned()
        } else if step.find_replace_all.is_some() {
            "find_replace_all".to_owned()
        } else if step.find_close {
            "find_close".to_owned()
        } else if step.scroll_y.is_some() || step.scroll_x.is_some() {
            "scroll".to_owned()
        } else if step.caret.is_some() {
            "caret".to_owned()
        } else if step.set_folding.is_some() {
            "folding".to_owned()
        } else if step.set_soft_wrap.is_some() {
            "soft_wrap".to_owned()
        } else if step.mono_font_size.is_some() || step.mono_font_family.is_some() {
            "font".to_owned()
        } else if step.resize_width.is_some() || step.resize_height.is_some() {
            "resize".to_owned()
        } else if step.close {
            "close".to_owned()
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
        if self.mode == WorkspaceMode::Build
            && (step.find_query.is_some()
                || step.find_next.is_some()
                || step.find_replace_all.is_some()
                || step.find_close)
            && let Some(session) = session.clone()
        {
            let editor = session.read(cx).editor.clone();
            editor.update(cx, |editor, cx| {
                if let Some(query) = step.find_query.as_deref() {
                    editor.open_search(false, cx);
                    editor.set_search_query(query, true, cx);
                }
                if let Some(steps) = step.find_next {
                    for _ in 0..steps {
                        editor.next_search_match(cx);
                    }
                    cx.notify();
                }
                if let Some(replacement) = step.find_replace_all.as_deref() {
                    editor.replace_all_search_matches(replacement, window, cx);
                }
                if step.find_close {
                    editor.close_search(cx);
                }
            });
        }
        if step.scroll_y.is_some() || step.scroll_x.is_some() {
            let x = step.scroll_x.unwrap_or(0.);
            let y = step.scroll_y.unwrap_or(0.);
            if self.mode == WorkspaceMode::Preview {
                self.rendered
                    .update(cx, |rendered, cx| rendered.set_scroll_offset(y, cx));
            } else if let Some(session) = session.clone() {
                session.update(cx, |session, cx| session.perf_scroll(x, y, cx));
            }
        }
        if let Some(caret) = step.caret.as_deref()
            && let Some(session) = session.clone()
        {
            match caret {
                "end" => session.update(cx, |session, cx| session.perf_cursor_to_end(cx)),
                "start" => {
                    let editor = session.read(cx).editor.clone();
                    editor.update(cx, |editor, cx| editor.set_selected_range(0..0, cx));
                }
                _ => accepted = false,
            }
        }
        if let Some(folding) = step.set_folding
            && let Some(session) = session.clone()
        {
            let editor = session.read(cx).editor.clone();
            editor.update(cx, |editor, cx| editor.set_folding(folding, window, cx));
        }
        if let Some(wrap) = step.set_soft_wrap
            && let Some(session) = session.clone()
        {
            let editor = session.read(cx).editor.clone();
            editor.update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
        }
        if step.mono_font_size.is_some() || step.mono_font_family.is_some() {
            let size = step.mono_font_size;
            let family = step.mono_font_family.clone();
            Theme::update(cx, |theme| {
                if let Some(size) = size {
                    theme.mono_font_size = px(size);
                }
                if let Some(family) = family {
                    theme.mono_font_family = family.into();
                }
            });
        }
        if let (Some(width), Some(height)) = (step.resize_width, step.resize_height) {
            window.resize(size(px(width), px(height)));
        }
        if let Some(text) = step.input.as_deref()
            && let Some(session) = session
        {
            if self.mode == WorkspaceMode::Build {
                let editor = session.read(cx).editor.clone();
                editor.update(cx, |editor, cx| {
                    editor.replace_text_in_range(None, text, window, cx);
                });
            } else {
                let writer = session.read(cx).writer.clone();
                writer.update(cx, |writer, cx| {
                    writer.replace_text_in_range(None, text, window, cx);
                });
            }
        }
        if step.close {
            let started = Instant::now();
            self.project
                .update(cx, |project, cx| project.close_active(window, cx));
            perf::span("document_close", "close_active", started.elapsed());
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
