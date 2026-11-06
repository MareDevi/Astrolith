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

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, IntoElement, LayoutId, Pixels,
    Window,
};

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

/// Emit one Build (source editor) frame: the Kit `Editor` element's own
/// request-layout, prepaint and paint phases.
pub(crate) fn build_frame(layout: Duration, prepaint: Duration, paint: Duration) {
    if !enabled() {
        return;
    }
    let sequence = FRAME_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    tracing::info!(
        target: "astrolith::perf",
        event = "build_frame",
        sequence,
        layout_ms = millis(layout),
        prepaint_ms = millis(prepaint),
        paint_ms = millis(paint),
        total_ms = millis(layout + prepaint + paint),
        "source editor frame"
    );
}

/// Wrap the Build source editor so its element phases are measured.
///
/// Returns the element unchanged when counters are off, so production frames
/// keep exactly one element and no extra timing branches.
pub(crate) fn build_frame_element(inner: AnyElement) -> AnyElement {
    if !enabled() {
        return inner;
    }
    BuildFrameElement {
        inner,
        layout: Duration::ZERO,
        prepaint: Duration::ZERO,
    }
    .into_any_element()
}

struct BuildFrameElement {
    inner: AnyElement,
    layout: Duration,
    prepaint: Duration,
}

impl Element for BuildFrameElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        self.inner.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.inner.source_location()
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let started = Instant::now();
        let layout_id = self.inner.request_layout(window, cx);
        self.layout = started.elapsed();
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let started = Instant::now();
        self.inner.prepaint(window, cx);
        self.prepaint = started.elapsed();
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        self.inner.paint(window, cx);
        build_frame(self.layout, self.prepaint, started.elapsed());
    }
}

impl IntoElement for BuildFrameElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
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

/// Emit one prepared application-owned text layout viewport.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_layout(
    mode: &str,
    stats: crate::editor::text_layout::LayoutStats,
    prepare: Duration,
    rows: usize,
    blocks: usize,
    total_height: f32,
    pending_bytes: usize,
    width: f32,
    scroll: f32,
) {
    if !enabled() {
        return;
    }
    let sequence = FRAME_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    tracing::info!(
        target: "astrolith::perf",
        event = "text_layout_frame",
        sequence,
        mode,
        blocks,
        rows,
        wrapped_rows = stats.wrapped_rows,
        wrapped_bytes = stats.wrapped_bytes,
        shaped_rows = stats.shaped_rows,
        shaped_bytes = stats.shaped_bytes,
        reused_rows = stats.reused_rows,
        pending = stats.pending,
        pending_bytes,
        total_height,
        width,
        scroll,
        prepare_ms = millis(prepare),
        "application text layout"
    );
}

/// Emit one incremental reconcile of the application text layout.
pub(crate) fn text_layout_edit(outcome: crate::editor::text_layout::UpdateOutcome) {
    if !enabled() {
        return;
    }
    tracing::info!(
        target: "astrolith::perf",
        event = "text_layout_edit",
        reused_blocks = outcome.reused_blocks,
        reused_rows = outcome.reused_rows,
        rebuilt_blocks = outcome.rebuilt_blocks,
        "application text layout edit"
    );
}

/// Emit one prepared complete-source viewport.
pub(crate) fn source_layout(
    stats: crate::editor::text_layout::source::SourceStats,
    prepare: Duration,
    total_width: f32,
    total_height: f32,
) {
    if !enabled() {
        return;
    }
    tracing::info!(
        target: "astrolith::perf",
        event = "source_layout_frame",
        measured_chunks = stats.measured_chunks,
        measured_bytes = stats.measured_bytes,
        shaped_lines = stats.shaped_lines,
        shaped_bytes = stats.shaped_bytes,
        reused_lines = stats.reused_lines,
        pending = stats.pending,
        prepare_ms = millis(prepare),
        total_width,
        total_height,
        "source viewport layout"
    );
}

/// Emit one complete-source surface frame.
///
/// `total_ms` includes the render-time preparation, so the source surface can
/// be compared with the stock editor's element phases without hiding shaping.
pub(crate) fn source_frame(prepare: Duration, render: Duration, layout: Duration, paint: Duration) {
    if !enabled() {
        return;
    }
    let sequence = FRAME_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    tracing::info!(
        target: "astrolith::perf",
        event = "build_frame",
        source = "surface",
        sequence,
        prepare_ms = millis(prepare),
        layout_ms = millis(render + layout),
        prepaint_ms = millis(layout),
        paint_ms = millis(paint),
        total_ms = millis(prepare + render + layout + paint),
        "source surface frame"
    );
}
