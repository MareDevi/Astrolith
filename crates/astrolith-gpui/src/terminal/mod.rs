//! Native interactive terminal view backed by a project-bound PTY.
//!
//! The service owns the process and bounded ANSI screen; this view only renders
//! cells, translates input and asks for a resize after layout.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use astrolith_terminal::{ExitInfo, TerminalSession};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    App, Bounds, Context, FocusHandle, Focusable, Hsla, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Render, SharedString, Task, Window, canvas, div, prelude::*, px,
};

use crate::theme::METRICS;
use astrolith_terminal::vt100;

/// Advance used for the cell grid until the mono face has been measured.
const FALLBACK_CELL_WIDTH: f32 = 7.8;
/// Row height used until the mono face has been measured.
const FALLBACK_LINE_HEIGHT: f32 = 17.;
/// Scroll rows per wheel line.
const WHEEL_ROWS: i32 = 3;
/// Polling cadence while the terminal dock is visible.
const ACTIVE_POLL: Duration = Duration::from_millis(16);
/// Polling cadence while the terminal dock is hidden. Slow enough that a
/// hidden terminal is not a repaint source, fast enough to notice an exit.
const HIDDEN_POLL: Duration = Duration::from_millis(250);

/// Measured geometry of one terminal cell.
///
/// gpui draws by exact family: a fontconfig generic such as `monospace` is not
/// a family, so the grid has to use the theme's resolved mono face and the
/// advance that face actually reports.
#[derive(Clone, Debug)]
struct CellMetrics {
    family: SharedString,
    size: f32,
    /// Advance width of one cell in logical pixels.
    col_width: f32,
    /// Height of one terminal row in logical pixels.
    row_height: f32,
}

impl CellMetrics {
    fn fallback() -> Self {
        Self {
            family: SharedString::default(),
            size: METRICS.type_scale.mono,
            col_width: FALLBACK_CELL_WIDTH,
            row_height: FALLBACK_LINE_HEIGHT,
        }
    }

    /// Measure the advance and row height once per (family, size).
    fn measure(family: SharedString, size: f32, window: &Window) -> Self {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&gpui_kit::font(family.clone()));
        let size_px = px(size);
        let col_width = text_system
            .advance(font_id, size_px, 'M')
            .map(|size| f32::from(size.width))
            .unwrap_or(FALLBACK_CELL_WIDTH);
        let row_height =
            f32::from(text_system.ascent(font_id, size_px) + text_system.descent(font_id, size_px));
        Self {
            family,
            size,
            col_width: if col_width > 0.5 {
                col_width
            } else {
                FALLBACK_CELL_WIDTH
            },
            row_height: if row_height > 0.5 {
                row_height.ceil()
            } else {
                FALLBACK_LINE_HEIGHT
            },
        }
    }

    /// Visible `(columns, rows)` for a measured viewport.
    fn grid(&self, bounds: Bounds<Pixels>) -> (u16, u16) {
        let cols = (f32::from(bounds.size.width) / self.col_width).floor() as u16;
        let rows = (f32::from(bounds.size.height) / self.row_height).floor() as u16;
        (cols.clamp(20, 500), rows.clamp(3, 200))
    }
}

/// One rendered cell.
struct CellView {
    text: SharedString,
    foreground: Hsla,
    background: Hsla,
    bold: bool,
    underline: bool,
    width_cells: u16,
}

/// One rendered row.
struct RowView {
    cells: Vec<CellView>,
}

/// A grid position inside the visible terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridPoint {
    row: u16,
    col: u16,
}

/// Inclusive terminal selection between two grid points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: GridPoint,
    focus: GridPoint,
}

impl Selection {
    fn ordered(self) -> (GridPoint, GridPoint) {
        if (self.anchor.row, self.anchor.col) <= (self.focus.row, self.focus.col) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    fn contains(self, point: GridPoint) -> bool {
        let (start, end) = self.ordered();
        (point.row, point.col) >= (start.row, start.col)
            && (point.row, point.col) <= (end.row, end.col)
    }
}

/// The terminal panel entity.
pub(crate) struct TerminalPanel {
    root: Option<PathBuf>,
    session: Option<TerminalSession>,
    focus: FocusHandle,
    last_revision: u64,
    last_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Measured cell geometry, recomputed only when the mono face changes.
    metrics: Option<CellMetrics>,
    selection: Option<Selection>,
    selecting: bool,
    /// Last recorded exit, kept separate from the live session so the header
    /// keeps reporting a dead shell after the reader reached EOF.
    exit: Option<ExitInfo>,
    status: Option<String>,
    failure: Option<String>,
    /// Whether the terminal dock is on screen and in Build.
    active: bool,
    poll_task: Option<Task<()>>,
    /// Poll intervals chosen by the loop, recorded for tests.
    #[cfg(test)]
    poll_intervals: Vec<Duration>,
    _keys: gpui_kit::Subscription,
}

impl TerminalPanel {
    /// Create an empty terminal panel; no process starts until a project attaches.
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let weak = cx.weak_entity();
        let keys = cx.intercept_keystrokes({
            let focus = focus.clone();
            move |event, window, cx| {
                if !focus.is_focused(window) {
                    return;
                }
                let handled = weak
                    .update(cx, |this, cx| this.handle_key(event, cx))
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            }
        });
        Self {
            root: None,
            session: None,
            focus,
            last_revision: 0,
            last_bounds: Rc::new(Cell::new(None)),
            metrics: None,
            selection: None,
            selecting: false,
            exit: None,
            status: None,
            failure: None,
            active: false,
            poll_task: None,
            #[cfg(test)]
            poll_intervals: Vec::new(),
            _keys: keys,
        }
    }

    /// Bind this terminal to a project root, ending any previous child.
    pub(crate) fn attach(&mut self, root: &Path, cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(root) {
            return;
        }
        self.stop();
        self.root = Some(root.to_path_buf());
        self.spawn(cx);
    }

    /// Project root, if attached.
    #[cfg(test)]
    pub(crate) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Whether the recorded state is a live shell.
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.running_state()
    }

    /// A session that has not recorded an exit or reader EOF.
    fn running_state(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| self.exit.is_none() && !session.is_exited())
    }

    /// Write raw bytes to the child (service-level tests through the view).
    #[cfg(test)]
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        if let Some(session) = &mut self.session {
            let _ = session.write(bytes);
        }
    }

    /// Current parsed screen text.
    #[cfg(test)]
    pub(crate) fn screen_contents(&self) -> String {
        self.session
            .as_ref()
            .map(|session| session.with_screen(|screen| screen.contents()))
            .unwrap_or_default()
    }

    /// Child process id, for lifecycle tests.
    #[cfg(test)]
    pub(crate) fn process_id(&self) -> Option<u32> {
        self.session.as_ref().and_then(TerminalSession::process_id)
    }

    /// Restart the shell in the same project.
    pub(crate) fn restart(&mut self, cx: &mut Context<Self>) {
        self.stop();
        self.spawn(cx);
        cx.notify();
    }

    /// Clear the rendered screen.
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            session.clear();
            self.last_revision = session.revision();
        }
        self.selection = None;
        cx.notify();
    }

    /// Copy the current selection, returning whether anything was copied.
    pub(crate) fn copy_selection(&self, cx: &mut App) -> bool {
        let (Some(session), Some(selection)) = (&self.session, self.selection) else {
            return false;
        };
        let (start, end) = selection.ordered();
        let text = session
            .with_screen(|screen| screen.contents_between(start.row, start.col, end.row, end.col));
        if text.is_empty() {
            return false;
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        true
    }

    /// Explicit shutdown used before a project switch and at quit.
    ///
    /// The session's child is signalled immediately; the panel does not rely
    /// on `Drop` during window teardown.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.stop();
        cx.notify();
    }

    fn stop(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.kill();
        }
        self.selection = None;
        self.exit = None;
        self.status = None;
        self.failure = None;
        self.poll_task = None;
    }

    fn spawn(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        match TerminalSession::spawn(&root, 80, 24) {
            Ok(session) => {
                tracing::info!(root = %root.display(), "started the project terminal");
                self.last_revision = session.revision();
                self.session = Some(session);
                self.start_polling(cx);
            }
            Err(error) => {
                tracing::warn!(%error, root = %root.display(), "could not start the project terminal");
                self.failure = Some(format!("Could not start the shell: {error}"));
            }
        }
    }

    /// Tell the view whether the terminal dock is on screen.
    ///
    /// Hidden terminals keep noticing exits at a slow cadence instead of
    /// repainting every frame. Becoming visible restarts the loop so the
    /// first frame after showing does not wait out the hidden interval.
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active && self.session.is_some() && !self.running_state() {
            return;
        }
        if active && self.session.is_some() {
            // Restart immediately: the old task may be asleep on the slow
            // hidden interval and would delay the first visible frame.
            self.poll_task = None;
            self.start_polling(cx);
        }
    }

    /// Cadence for the next poll, chosen from visibility.
    fn poll_interval(&self) -> Duration {
        if self.active {
            ACTIVE_POLL
        } else {
            HIDDEN_POLL
        }
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.poll_task.is_some() {
            return;
        }
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let interval = this
                    .update(cx, |this, cx| {
                        let interval = this.poll_interval();
                        #[cfg(test)]
                        this.poll_intervals.push(interval);
                        this.poll(cx).then_some(interval)
                    })
                    .ok()
                    .flatten();
                let Some(interval) = interval else {
                    break;
                };
                cx.background_executor().timer(interval).await;
            }
        }));
    }

    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let (cols, rows) = self.grid_size();
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let mut changed = false;
        let mut resize_failed = false;
        if session.size() != (cols, rows) && session.resize(cols, rows).is_err() {
            resize_failed = true;
            changed = true;
        }
        let exit = session.poll_exit();
        let revision = session.revision();
        let reader_exited = session.is_exited();
        if resize_failed {
            self.failure = Some("Could not resize the terminal.".into());
        }
        if let Some(exit) = exit {
            self.status = Some(exit.message());
            self.exit = Some(exit);
            changed = true;
        }
        if revision != self.last_revision {
            self.last_revision = revision;
            changed = true;
        }
        let exited = self.exit.is_some() || reader_exited;
        if changed {
            cx.notify();
        }
        if exited {
            self.poll_task = None;
        }
        !exited
    }

    /// Cached measurements, or the fallback before the first frame.
    fn cell_metrics(&self) -> CellMetrics {
        self.metrics.clone().unwrap_or_else(CellMetrics::fallback)
    }

    /// Re-measure when the theme's mono face or size changes.
    fn ensure_metrics(&mut self, window: &Window, cx: &App) -> CellMetrics {
        let family = cx.theme().mono_font_family.clone();
        let size = f32::from(cx.theme().mono_font_size);
        if let Some(metrics) = &self.metrics
            && metrics.family == family
            && metrics.size == size
        {
            return metrics.clone();
        }
        let metrics = CellMetrics::measure(family, size, window);
        self.metrics = Some(metrics.clone());
        metrics
    }

    fn grid_size(&self) -> (u16, u16) {
        let Some(bounds) = self.last_bounds.get() else {
            return (80, 24);
        };
        self.cell_metrics().grid(bounds)
    }

    fn point_at(&self, position: gpui_kit::Point<Pixels>) -> Option<GridPoint> {
        let bounds = self.last_bounds.get()?;
        let x = f32::from(position.x - bounds.origin.x);
        let y = f32::from(position.y - bounds.origin.y);
        if x < 0.
            || y < 0.
            || x >= f32::from(bounds.size.width)
            || y >= f32::from(bounds.size.height)
        {
            return None;
        }
        let metrics = self.cell_metrics();
        let (cols, rows) = metrics.grid(bounds);
        Some(GridPoint {
            row: ((y / metrics.row_height).floor() as u16).min(rows.saturating_sub(1)),
            col: ((x / metrics.col_width).floor() as u16).min(cols.saturating_sub(1)),
        })
    }

    fn selected_text(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let selection = self.selection?;
        let (start, end) = selection.ordered();
        Some(
            session.with_screen(|screen| {
                screen.contents_between(start.row, start.col, end.row, end.col)
            }),
        )
    }

    fn handle_key(&mut self, event: &gpui_kit::KeystrokeEvent, cx: &mut App) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        let modifiers = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        // Clipboard chords belong to the terminal, not the application.
        if modifiers.control && modifiers.shift && key.eq_ignore_ascii_case("c") {
            return self.copy_selection(cx);
        }
        if (modifiers.control || modifiers.platform) && key.eq_ignore_ascii_case("v") {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let _ = session.write(text.as_bytes());
                session.scroll_to_bottom();
                return true;
            }
            return false;
        }
        let bytes = translate_key(event);
        if let Some(bytes) = bytes {
            if session.write(&bytes).is_ok() {
                session.scroll_to_bottom();
                self.selection = None;
            }
            true
        } else {
            false
        }
    }

    fn render_rows(&self, window: &Window, cx: &App) -> Vec<RowView> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let foreground: Hsla = cx.theme().foreground;
        let background: Hsla = cx.theme().background;
        let cursor_color: Hsla = cx.theme().foreground;
        let selection_color: Hsla = cx.theme().selection;
        let cursor_hidden = session.with_screen(|screen| screen.hide_cursor());
        let cursor = session.with_screen(|screen| screen.cursor_position());
        let live = session.with_screen(|screen| screen.scrollback() == 0);
        let selection = self.selection;
        let (cols, rows) = session.size();
        session.with_screen(|screen| {
            let mut output = Vec::with_capacity(usize::from(rows));
            for row in 0..rows {
                let mut cells = Vec::with_capacity(usize::from(cols));
                let mut col = 0;
                while col < cols {
                    let Some(cell) = screen.cell(row, col) else {
                        break;
                    };
                    if cell.is_wide_continuation() {
                        col += 1;
                        continue;
                    }
                    let point = GridPoint { row, col };
                    let selected = selection.is_some_and(|selection| selection.contains(point));
                    let is_cursor = live
                        && !cursor_hidden
                        && cursor == (row, col)
                        && self.focus.is_focused(window);
                    let mut fg = ansi_color(cell.fgcolor(), foreground);
                    let mut bg = ansi_color(cell.bgcolor(), background);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    if selected {
                        bg = selection_color;
                        fg = foreground;
                    }
                    if is_cursor {
                        std::mem::swap(&mut fg, &mut bg);
                        bg = cursor_color;
                        fg = background;
                    }
                    if cell.dim() {
                        fg = fg.opacity(0.65);
                    }
                    let text = if cell.contents().is_empty() {
                        " ".into()
                    } else {
                        SharedString::from(cell.contents())
                    };
                    let width_cells = if cell.is_wide() { 2 } else { 1 };
                    cells.push(CellView {
                        text,
                        foreground: fg,
                        background: bg,
                        bold: cell.bold(),
                        underline: cell.underline(),
                        width_cells,
                    });
                    col += width_cells;
                }
                output.push(RowView { cells });
            }
            output
        })
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let metrics = self.ensure_metrics(_window, cx);
        let mono = metrics.family.clone();
        let row_height = metrics.row_height;
        let col_width = metrics.col_width;
        let rows = self.render_rows(_window, cx);
        let bounds_cell = self.last_bounds.clone();
        let running = self.running_state();
        let exit = self.exit.clone();
        let status = self
            .failure
            .clone()
            .or_else(|| {
                exit.as_ref()
                    .map(|exit| format!("Terminal {}", exit.message()))
            })
            .unwrap_or_else(|| {
                if running {
                    "Shell running".to_owned()
                } else {
                    "No shell".to_owned()
                }
            });
        let selected = self.selected_text();
        let clear = cx.listener(|this, _, _, cx| this.clear(cx));
        let restart = cx.listener(|this, _, _, cx| this.restart(cx));
        let copy = cx.listener(|this, _, _, cx| {
            this.copy_selection(cx);
        });

        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .h_flex()
                    .h(px(32.))
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .px(px(METRICS.space.md as f32))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1p5()
                            .px(px(METRICS.space.sm as f32))
                            .py(px(2.))
                            .rounded(px(METRICS.radius.small))
                            .bg(cx.theme().tab_bar_segmented)
                            .text_xs()
                            .child(div().size_1p5().rounded_full().bg(if running {
                                cx.theme().success
                            } else if self.failure.is_some() {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            }))
                            .child("Terminal"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    )
                    .child(
                        Button::new("terminal-copy")
                            .small()
                            .ghost()
                            .label("Copy")
                            .disabled(selected.is_none())
                            .on_click(copy),
                    )
                    .child(
                        Button::new("terminal-clear")
                            .small()
                            .ghost()
                            .label("Clear")
                            .disabled(!running)
                            .on_click(clear),
                    )
                    .child(
                        Button::new("terminal-restart")
                            .small()
                            .ghost()
                            .label("Restart")
                            .on_click(restart),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .font_family(mono.clone())
                    .text_size(px(metrics.size))
                    .track_focus(&self.focus)
                    .key_context("Terminal")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.focus.focus(window, cx);
                            if let Some(point) = this.point_at(event.position) {
                                this.selection = Some(Selection {
                                    anchor: point,
                                    focus: point,
                                });
                                this.selecting = true;
                            }
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if !this.selecting {
                            return;
                        }
                        if let Some(point) = this.point_at(event.position)
                            && let Some(selection) = this.selection.as_mut()
                            && selection.focus != point
                        {
                            selection.focus = point;
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.selecting = false;
                            cx.notify();
                        }),
                    )
                    .on_scroll(
                        cx.listener(move |this, event: &gpui_kit::ScrollEvent, _, cx| {
                            let lines = match event.delta {
                                gpui_kit::ScrollDelta::Lines(lines) => lines.y,
                                gpui_kit::ScrollDelta::Pixels(pixels) => {
                                    f32::from(pixels.y) / row_height
                                }
                            };
                            if lines.abs() < f32::EPSILON {
                                return;
                            }
                            let delta = -(lines.round() as i32) * WHEEL_ROWS;
                            if let Some(session) = &this.session
                                && session.scroll(delta)
                            {
                                this.selection = None;
                                this.last_revision = session.revision();
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                bounds_cell.set(Some(bounds));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .v_flex()
                            .children(rows.into_iter().map(|row| {
                                div()
                                    .h(px(row_height))
                                    .w_full()
                                    .flex()
                                    .flex_shrink_0()
                                    .children(row.cells.into_iter().map(|cell| {
                                        div()
                                            .h_full()
                                            .w(px(col_width * f32::from(cell.width_cells)))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .bg(cell.background)
                                            .text_color(cell.foreground)
                                            .when(cell.bold, |cell| {
                                                cell.font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            })
                                            .when(cell.underline, |cell| cell.underline())
                                            .child(cell.text)
                                    }))
                            })),
                    )
                    .when(!running, |viewport| {
                        viewport.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    self.failure
                                        .clone()
                                        .unwrap_or_else(|| "No project terminal".to_owned()),
                                ),
                        )
                    }),
            )
    }
}

/// Key translation from GPUI keystrokes to terminal byte sequences.
fn translate_key(event: &gpui_kit::KeystrokeEvent) -> Option<Vec<u8>> {
    let key = event.keystroke.key.as_str();
    let modifiers = event.keystroke.modifiers;
    let control = modifiers.control;
    let alt = modifiers.alt;
    let platform = modifiers.platform;
    let function = key.starts_with('f') && key.len() > 1 && key[1..].parse::<u8>().is_ok();
    if function && !control && !alt && !platform {
        let number: u8 = key[1..].parse().ok()?;
        let sequence = match number {
            1..=4 => format!("\x1bO{}", char::from(b'P' + number - 1)),
            5 => "\x1b[15~".to_owned(),
            6 => "\x1b[17~".to_owned(),
            7 => "\x1b[18~".to_owned(),
            8 => "\x1b[19~".to_owned(),
            9 => "\x1b[20~".to_owned(),
            10 => "\x1b[21~".to_owned(),
            11 => "\x1b[23~".to_owned(),
            12 => "\x1b[24~".to_owned(),
            _ => return None,
        };
        return Some(sequence.into_bytes());
    }
    match key {
        "enter" => return Some(vec![b'\r']),
        "tab" if modifiers.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return Some(vec![b'\t']),
        "backspace" => return Some(vec![0x7f]),
        "escape" => return Some(vec![0x1b]),
        "delete" => return Some(b"\x1b[3~".to_vec()),
        "insert" => return Some(b"\x1b[2~".to_vec()),
        "home" => return Some(b"\x1b[H".to_vec()),
        "end" => return Some(b"\x1b[F".to_vec()),
        "pageup" => return Some(b"\x1b[5~".to_vec()),
        "pagedown" => return Some(b"\x1b[6~".to_vec()),
        "up" => return Some(b"\x1b[A".to_vec()),
        "down" => return Some(b"\x1b[B".to_vec()),
        "right" => return Some(b"\x1b[C".to_vec()),
        "left" => return Some(b"\x1b[D".to_vec()),
        "space" if control => return Some(vec![0]),
        "[" if control => return Some(vec![0x1b]),
        _ => {}
    }
    if control {
        let byte = key.bytes().next()?;
        if byte.is_ascii_alphabetic() {
            let byte = byte.to_ascii_lowercase();
            let control = byte - b'a' + 1;
            return Some(if alt {
                vec![0x1b, control]
            } else {
                vec![control]
            });
        }
        return None;
    }
    let text = event
        .keystroke
        .key_char
        .clone()
        .or_else(|| (key.chars().count() == 1).then(|| key.to_owned()))?;
    let mut bytes = Vec::with_capacity(text.len() + 1);
    if alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

/// Map a vt100 color to the terminal surface; indexed values use xterm's palette.
fn ansi_color(color: vt100::Color, default: Hsla) -> Hsla {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(red, green, blue) => gpui_kit::rgba(
            u32::from(red) << 24 | u32::from(green) << 16 | u32::from(blue) << 8 | 0xff,
        )
        .into(),
        vt100::Color::Idx(index) => indexed_color(index).into(),
    }
}

fn indexed_color(index: u8) -> gpui_kit::Rgba {
    const BASE: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
        0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    let rgb = match index {
        0..=15 => BASE[usize::from(index)],
        16..=231 => {
            const STEPS: [u32; 6] = [0, 95, 135, 175, 215, 255];
            let index = index - 16;
            let red = STEPS[usize::from(index / 36)];
            let green = STEPS[usize::from((index / 6) % 6)];
            let blue = STEPS[usize::from(index % 6)];
            red << 16 | green << 8 | blue
        }
        _ => {
            let level = 8 + 10 * u32::from(index - 232);
            level << 16 | level << 8 | level
        }
    };
    gpui_kit::rgb(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, WindowOptions, base::Root, size};

    fn metrics(col_width: f32, row_height: f32) -> CellMetrics {
        CellMetrics {
            family: "Measured Mono".into(),
            size: 13.,
            col_width,
            row_height,
        }
    }

    #[test]
    fn grid_follows_the_measured_advance_instead_of_the_fallback() {
        let bounds = Bounds::new(gpui_kit::Point::default(), size(px(800.), px(480.)));
        assert_eq!(
            metrics(FALLBACK_CELL_WIDTH, FALLBACK_LINE_HEIGHT).grid(bounds),
            (102, 28)
        );
        assert_eq!(metrics(8., 16.).grid(bounds), (100, 30));
        assert_eq!(
            metrics(10., 20.).grid(bounds),
            (80, 24),
            "a wider advance must reduce the column count"
        );
    }

    #[gpui_kit::test]
    async fn painted_font_family_is_the_resolved_mono_face(cx: &mut TestAppContext) {
        let mut panel = None;
        let (handle, _host) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                panel = Some(cx.new(|cx| TerminalPanel::new(window, cx)));
                cx.new(|_| Host)
            })
            .unwrap()
        });
        let handle = handle.downcast::<Root>().unwrap();
        let panel = panel.expect("the panel is built with the window");
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                let metrics = panel.ensure_metrics(window, cx);
                assert_eq!(metrics.family, cx.theme().mono_font_family);
                assert_ne!(
                    metrics.family.as_ref(),
                    "monospace",
                    "fontconfig generics are not families gpui can resolve"
                );
                assert!(metrics.col_width > 0. && metrics.row_height > 0.);
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn hidden_terminal_backs_off_the_poll_loop(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let mut panel = None;
        let (handle, _host) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                panel = Some(cx.new(|cx| TerminalPanel::new(window, cx)));
                cx.new(|_| Host)
            })
            .unwrap()
        });
        let handle = handle.downcast::<Root>().unwrap();
        let panel = panel.expect("the panel is built with the window");
        cx.update_window(handle.into(), |_, _, cx| {
            panel.update(cx, |panel, cx| {
                panel.attach(root.path(), cx);
                panel.set_active(true, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(96));
        cx.run_until_parked();
        let active_ticks = panel.read_with(cx, |panel, _| panel.poll_intervals.len());
        assert!(
            active_ticks >= 2,
            "the visible terminal must poll on the active cadence"
        );
        assert!(
            panel
                .read_with(cx, |panel, _| panel.poll_intervals.clone())
                .iter()
                .all(|interval| *interval == ACTIVE_POLL)
        );

        cx.update_window(handle.into(), |_, _, cx| {
            panel.update(cx, |panel, cx| panel.set_active(false, cx));
        })
        .unwrap();
        cx.executor().advance_clock(Duration::from_millis(750));
        cx.run_until_parked();
        let intervals = panel.read_with(cx, |panel, _| panel.poll_intervals.clone());
        let hidden: Vec<_> = intervals.iter().skip(active_ticks).copied().collect();
        assert!(
            !hidden.is_empty(),
            "a hidden terminal must still poll to notice exit"
        );
        assert!(
            hidden.iter().all(|interval| *interval == HIDDEN_POLL),
            "hidden polls must not run at frame rate: {hidden:?}"
        );
        cx.update_window(handle.into(), |_, _, cx| {
            panel.update(cx, |panel, cx| panel.shutdown(cx));
        })
        .unwrap();
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
}
