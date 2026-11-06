//! Opt-in workload counters for the editor rendering path.
//!
//! Nothing here runs unless `ASTROLITH_EDITOR_PERF` is set in the environment.
//! The output is numeric only: block/fragment counts, bytes handed to shaping,
//! cache reuse and phase timings. No document text, selection or composition
//! content is written, so a recorded run is safe to keep beside other artifacts.
//!
//! A line is emitted per measured event with target `astrolith::perf`, which the
//! native harness (`scripts/editor-perf.mjs`) collects and aggregates.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Whether the opt-in counters are enabled for this process.
pub(crate) fn enabled() -> bool {
    static ENABLED: LazyLock<bool> = LazyLock::new(|| {
        std::env::var_os("ASTROLITH_EDITOR_PERF").is_some_and(|value| !value.is_empty())
    });
    *ENABLED
}

/// Start a span when counters are enabled; `None` costs one branch otherwise.
pub(crate) fn start() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// Milliseconds with sub-microsecond precision, for log fields.
fn millis(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1_000.
}

/// One rendered writer frame.
#[derive(Clone, Copy, Default)]
pub(crate) struct Frame {
    pub(crate) blocks_total: usize,
    pub(crate) blocks_rendered: usize,
    pub(crate) nodes_built: usize,
    pub(crate) nodes_reused: usize,
    pub(crate) decorations: usize,
    /// UTF-8 bytes handed to Kit's Inline layout this frame.
    pub(crate) shaping_bytes: usize,
    pub(crate) geometry_leaves: usize,
    pub(crate) caret_blinks: u64,
}

static FRAME_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Emit one writer frame. `render` covers node preparation and element
/// construction, `layout`/`prepaint`/`paint` the element phases.
pub(crate) fn frame(
    stats: Frame,
    render: Duration,
    layout: Duration,
    prepaint: Duration,
    paint: Duration,
) {
    if !enabled() {
        return;
    }
    let sequence = FRAME_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    tracing::info!(
        target: "astrolith::perf",
        event = "writer_frame",
        sequence,
        blocks_total = stats.blocks_total,
        blocks_rendered = stats.blocks_rendered,
        nodes_built = stats.nodes_built,
        nodes_reused = stats.nodes_reused,
        decorations = stats.decorations,
        shaping_bytes = stats.shaping_bytes,
        geometry_leaves = stats.geometry_leaves,
        caret_blinks = stats.caret_blinks,
        render_ms = millis(render),
        layout_ms = millis(layout),
        prepaint_ms = millis(prepaint),
        paint_ms = millis(paint),
        "writer frame"
    );
}

/// Emit a paint of the cached render tree: the caret layer changed, the text
/// layout did not. Only the paint phase is meaningful here.
pub(crate) fn frame_repaint(stats: Frame, paint: Duration) {
    if !enabled() {
        return;
    }
    let sequence = FRAME_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    tracing::info!(
        target: "astrolith::perf",
        event = "writer_repaint",
        sequence,
        blocks_rendered = stats.blocks_rendered,
        geometry_leaves = stats.geometry_leaves,
        caret_blinks = stats.caret_blinks,
        paint_ms = millis(paint),
        "writer repaint"
    );
}

/// Emit one named application-side span (change handler, parse wait, mode
/// handler, driver action).
pub(crate) fn span(event: &'static str, name: &str, elapsed: Duration) {
    if !enabled() {
        return;
    }
    tracing::info!(
        target: "astrolith::perf",
        event,
        name,
        ms = millis(elapsed),
        "editor span"
    );
}

/// Monotonic timestamp in nanoseconds since the process started, for
/// correlating an in-app action with the harness's wall clock.
pub(crate) fn monotonic_ns() -> u128 {
    static ORIGIN: LazyLock<Instant> = LazyLock::new(Instant::now);
    ORIGIN.elapsed().as_nanos()
}

/// Unix epoch milliseconds of the same instant, so an external clock can be
/// aligned with the monotonic timestamps.
pub(crate) fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
