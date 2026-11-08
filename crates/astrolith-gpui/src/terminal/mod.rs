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

use astrolith_terminal::{
    ExitInfo, TerminalCell, TerminalColor, TerminalMouseEncoding, TerminalMouseMode,
    TerminalSession, TerminalSnapshot,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    App, Bounds, Context, FocusHandle, Focusable, Hsla, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Render, SharedString, Task, Window, canvas, div,
    prelude::*, px,
};

use crate::theme::METRICS;

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

/// Paint state shared by a run of adjacent cells.
#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    foreground: Hsla,
    background: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    /// Kept in the key so a wide glyph never shares a run with narrow ones.
    wide: bool,
    /// Selection and cursor cells split runs even if their colours coincide.
    highlight: bool,
}

/// Adjacent cells that share one paint state, painted as one text run.
struct RunView {
    text: String,
    style: RunStyle,
    /// Columns covered, so wide cells and padding keep their exact extent.
    width_cells: u16,
}

impl RunView {
    /// Append a cell to `runs`, merging with the previous run when the paint
    /// state matches. The cell text is copied into the run instead of
    /// allocating one string per cell.
    fn push(runs: &mut Vec<Self>, text: &str, width_cells: u16, style: RunStyle) {
        if let Some(run) = runs.last_mut()
            && run.style == style
        {
            run.text.push_str(text);
            run.width_cells = run.width_cells.saturating_add(width_cells);
            return;
        }
        runs.push(Self {
            text: text.to_owned(),
            style,
            width_cells,
        });
    }
}

/// One rendered row.
struct RowView {
    runs: Vec<RunView>,
}

/// A copy of the visible screen taken under the parser lock.
///
/// The reader thread parses under the same mutex, so the render side must not
/// hold it while it allocates strings and converts colours. Copying the cells
/// is a flat clone of the visible screen; everything expensive happens after
/// the lock is released.
struct ScreenSnapshot {
    terminal: TerminalSnapshot,
}

impl ScreenSnapshot {
    fn cell(&self, row: u16, col: u16) -> Option<&TerminalCell> {
        self.terminal.cell(row, col)
    }
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
struct TerminalInstance {
    id: u64,
    session: Option<TerminalSession>,
    last_revision: u64,
    exit: Option<ExitInfo>,
    status: Option<String>,
    failure: Option<String>,
}

pub(crate) struct TerminalPanel {
    root: Option<PathBuf>,
    terminals: Vec<TerminalInstance>,
    active_terminal: Option<u64>,
    next_terminal_id: u64,
    focus: FocusHandle,
    last_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Measured cell geometry, recomputed only when the mono face changes.
    metrics: Option<CellMetrics>,
    selection: Option<Selection>,
    selecting: bool,
    last_mouse_report: Option<(GridPoint, u8)>,
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
            terminals: Vec::new(),
            active_terminal: None,
            next_terminal_id: 1,
            focus,
            last_bounds: Rc::new(Cell::new(None)),
            metrics: None,
            selection: None,
            selecting: false,
            last_mouse_report: None,
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

    /// Whether terminal input currently owns the keyboard.
    pub(crate) fn has_focus(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    fn active_instance(&self) -> Option<&TerminalInstance> {
        let id = self.active_terminal?;
        self.terminals.iter().find(|terminal| terminal.id == id)
    }

    fn active_instance_mut(&mut self) -> Option<&mut TerminalInstance> {
        let id = self.active_terminal?;
        self.terminals.iter_mut().find(|terminal| terminal.id == id)
    }

    fn active_session(&self) -> Option<&TerminalSession> {
        self.active_instance()?.session.as_ref()
    }

    fn active_session_mut(&mut self) -> Option<&mut TerminalSession> {
        self.active_instance_mut()?.session.as_mut()
    }

    /// A session that has not recorded an exit or reader EOF.
    fn running_state(&self) -> bool {
        self.active_instance().is_some_and(|terminal| {
            terminal
                .session
                .as_ref()
                .is_some_and(|session| terminal.exit.is_none() && !session.is_exited())
        })
    }

    /// Write raw bytes to the child (service-level tests through the view).
    #[cfg(test)]
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        if let Some(session) = self.active_session_mut() {
            let _ = session.write(bytes);
        }
    }

    /// Current parsed screen text.
    #[cfg(test)]
    pub(crate) fn screen_contents(&self) -> String {
        self.active_session()
            .map(TerminalSession::contents)
            .unwrap_or_default()
    }

    /// Child process id, for lifecycle tests.
    #[cfg(test)]
    pub(crate) fn process_id(&self) -> Option<u32> {
        self.active_session().and_then(TerminalSession::process_id)
    }

    /// Restart the shell in the same project.
    pub(crate) fn restart(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(terminal) = self.active_instance_mut() else {
            return;
        };
        if let Some(mut session) = terminal.session.take() {
            session.kill();
        }
        terminal.exit = None;
        terminal.status = None;
        terminal.failure = None;
        match TerminalSession::spawn(&root, 80, 24) {
            Ok(session) => {
                terminal.last_revision = session.revision();
                terminal.session = Some(session);
                self.start_polling(cx);
            }
            Err(error) => terminal.failure = Some(format!("Could not start the shell: {error}")),
        }
        cx.notify();
    }

    /// Clear the rendered screen.
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session() {
            session.clear();
            let revision = session.revision();
            if let Some(terminal) = self.active_instance_mut() {
                terminal.last_revision = revision;
            }
        }
        self.selection = None;
        cx.notify();
    }

    /// Copy the current selection, returning whether anything was copied.
    pub(crate) fn copy_selection(&self, cx: &mut App) -> bool {
        let (Some(session), Some(selection)) = (self.active_session(), self.selection) else {
            return false;
        };
        let (start, end) = selection.ordered();
        let text = session.contents_between((start.row, start.col), (end.row, end.col));
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
        for terminal in &mut self.terminals {
            if let Some(mut session) = terminal.session.take() {
                session.kill();
            }
        }
        self.terminals.clear();
        self.active_terminal = None;
        self.selection = None;
        self.poll_task = None;
    }

    fn spawn(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let id = self.next_terminal_id;
        self.next_terminal_id = self.next_terminal_id.saturating_add(1);
        let (session, failure) = match TerminalSession::spawn(&root, 80, 24) {
            Ok(session) => {
                tracing::info!(root = %root.display(), "started the project terminal");
                (Some(session), None)
            }
            Err(error) => {
                tracing::warn!(%error, root = %root.display(), "could not start the project terminal");
                (None, Some(format!("Could not start the shell: {error}")))
            }
        };
        let last_revision = session.as_ref().map_or(0, TerminalSession::revision);
        self.terminals.push(TerminalInstance {
            id,
            session,
            last_revision,
            exit: None,
            status: None,
            failure,
        });
        self.active_terminal = Some(id);
        self.selection = None;
        self.start_polling(cx);
    }

    /// Start another shell in the current project.
    fn new_terminal(&mut self, cx: &mut Context<Self>) {
        self.spawn(cx);
        cx.notify();
    }

    /// Select a retained shell tab.
    fn select_terminal(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.active_terminal == Some(id) {
            return;
        }
        if self.terminals.iter().any(|terminal| terminal.id == id) {
            self.active_terminal = Some(id);
            self.selection = None;
            self.start_polling(cx);
            cx.notify();
        }
    }

    /// Close the selected shell and activate its nearest remaining neighbor.
    fn close_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_terminal else {
            return;
        };
        let Some(index) = self.terminals.iter().position(|terminal| terminal.id == id) else {
            return;
        };
        let mut removed = self.terminals.remove(index);
        if let Some(mut session) = removed.session.take() {
            session.kill();
        }
        self.active_terminal = self
            .terminals
            .get(index.min(self.terminals.len().saturating_sub(1)))
            .map(|terminal| terminal.id);
        self.selection = None;
        cx.notify();
    }

    /// Tell the view whether the terminal dock is on screen.
    ///
    /// Hidden terminals keep noticing exits at a slow cadence instead of
    /// repainting every frame. Becoming visible restarts the loop so the
    /// first frame after showing does not wait out the hidden interval.
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) -> bool {
        if self.active == active {
            return false;
        }
        self.active = active;
        if active
            && !self
                .terminals
                .iter()
                .any(|terminal| terminal.session.is_some() && terminal.exit.is_none())
        {
            return active;
        }
        if active && !self.terminals.is_empty() {
            // Restart immediately: the old task may be asleep on the slow
            // hidden interval and would delay the first visible frame.
            self.poll_task = None;
            self.start_polling(cx);
        }
        active
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
        let mut changed = false;
        let active_id = self.active_terminal;
        let mut running = false;
        for terminal in &mut self.terminals {
            let Some(session) = terminal.session.as_mut() else {
                continue;
            };
            if Some(terminal.id) == active_id
                && session.size() != (cols, rows)
                && session.resize(cols, rows).is_err()
            {
                terminal.failure = Some("Could not resize the terminal.".into());
                changed = true;
            }
            if let Some(exit) = session.poll_exit() {
                terminal.status = Some(exit.message());
                terminal.exit = Some(exit);
                changed = true;
            }
            let revision = session.revision();
            if revision != terminal.last_revision {
                terminal.last_revision = revision;
                changed = true;
            }
            if terminal.exit.is_none() && !session.is_exited() {
                running = true;
            }
        }
        if changed {
            cx.notify();
        }
        if !running {
            self.poll_task = None;
        }
        running
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
        let session = self.active_session()?;
        let selection = self.selection?;
        let (start, end) = selection.ordered();
        Some(session.contents_between((start.row, start.col), (end.row, end.col)))
    }

    fn handle_key(&mut self, event: &gpui_kit::KeystrokeEvent, cx: &mut App) -> bool {
        let Some(session) = self.active_session_mut() else {
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
                let bracketed = session.bracketed_paste();
                let _ = session.write(&paste_payload(&text, bracketed));
                session.scroll_to_bottom();
                return true;
            }
            return false;
        }
        let application_cursor = session.application_cursor();
        let bytes = translate_key(event, application_cursor);
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

    fn mouse_mode(&self) -> TerminalMouseMode {
        self.active_session()
            .map(|session| session.mouse_protocol().0)
            .unwrap_or_default()
    }

    fn send_mouse_report(
        &mut self,
        button: u8,
        point: GridPoint,
        modifiers: gpui_kit::Modifiers,
        release: bool,
        motion: bool,
    ) {
        let Some(session) = self.active_session() else {
            return;
        };
        let (mode, encoding) = session.mouse_protocol();
        if mode == TerminalMouseMode::None {
            return;
        }
        let code = button
            + u8::from(modifiers.shift) * 4
            + u8::from(modifiers.alt) * 8
            + u8::from(modifiers.control) * 16
            + if motion { 32 } else { 0 };
        let report = encode_mouse_report(code, point, release, encoding);
        if let Some(session) = self.active_session_mut() {
            let _ = session.write(&report);
        }
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        let Some(point) = self.point_at(event.position) else {
            return;
        };
        if self.mouse_mode() != TerminalMouseMode::None {
            if let Some(button) = mouse_button_code(event.button) {
                self.send_mouse_report(button, point, event.modifiers, false, false);
                self.last_mouse_report = Some((point, button));
            }
            self.selecting = false;
            return;
        }
        if event.button == MouseButton::Left {
            self.selection = Some(Selection {
                anchor: point,
                focus: point,
            });
            self.selecting = true;
            self.last_mouse_report = None;
            cx.notify();
        }
    }

    fn handle_mouse_up(&mut self, event: &MouseUpEvent) {
        let mode = self.mouse_mode();
        if matches!(
            mode,
            TerminalMouseMode::PressRelease
                | TerminalMouseMode::ButtonMotion
                | TerminalMouseMode::AnyMotion
        ) && let Some(point) = self.point_at(event.position)
        {
            self.send_mouse_report(3, point, event.modifiers, true, false);
        }
        self.selecting = false;
        self.last_mouse_report = None;
    }

    /// Copy the visible screen under the parser lock, once per frame.
    fn snapshot(&self) -> Option<ScreenSnapshot> {
        Some(ScreenSnapshot {
            terminal: self.active_session()?.snapshot(),
        })
    }

    fn render_rows(&self, window: &Window, cx: &App) -> Vec<RowView> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let foreground: Hsla = cx.theme().foreground;
        let background: Hsla = cx.theme().background;
        let cursor_color: Hsla = cx.theme().foreground;
        let selection_color: Hsla = cx.theme().selection;
        let cursor = snapshot.terminal.cursor();
        let selection = self.selection;
        let focused = self.focus.is_focused(window);
        let (cols, rows) = snapshot.terminal.size();
        let mut output = Vec::with_capacity(usize::from(rows));
        for row in 0..rows {
            // Find the last cell that paints something: trailing blanks
            // on the default background need no element at all.
            let mut end = 0;
            let mut col = cols;
            while col > 0 {
                col -= 1;
                let Some(cell) = snapshot.cell(row, col) else {
                    continue;
                };
                if cell.wide_continuation() {
                    continue;
                }
                let point = GridPoint { row, col };
                let padding = cell.contents().is_empty()
                    && matches!(cell.background(), TerminalColor::Default)
                    && !cell.inverse()
                    && !selection.is_some_and(|selection| selection.contains(point))
                    && !(snapshot.terminal.is_live()
                        && !snapshot.terminal.cursor_hidden()
                        && cursor == (row, col)
                        && focused);
                if !padding {
                    end = col + if cell.wide() { 2 } else { 1 };
                    break;
                }
            }

            let mut runs = Vec::new();
            let mut col = 0;
            while col < end {
                let Some(cell) = snapshot.cell(row, col) else {
                    break;
                };
                if cell.wide_continuation() {
                    col += 1;
                    continue;
                }
                let point = GridPoint { row, col };
                let selected = selection.is_some_and(|selection| selection.contains(point));
                let is_cursor = snapshot.terminal.is_live()
                    && !snapshot.terminal.cursor_hidden()
                    && cursor == (row, col)
                    && focused;
                let mut fg = ansi_color(cell.foreground(), foreground);
                let mut bg = ansi_color(cell.background(), background);
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
                    " "
                } else {
                    cell.contents()
                };
                let wide = cell.wide();
                let width_cells = if wide { 2 } else { 1 };
                RunView::push(
                    &mut runs,
                    text,
                    width_cells,
                    RunStyle {
                        foreground: fg,
                        background: bg,
                        bold: cell.bold(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                        strikethrough: cell.strikethrough(),
                        wide,
                        highlight: selected || is_cursor,
                    },
                );
                col += width_cells;
            }
            output.push(RowView { runs });
        }
        output
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
        let active = self.active_terminal;
        let active_failure = self
            .active_instance()
            .and_then(|terminal| terminal.failure.clone());
        let status = self
            .active_instance()
            .and_then(|terminal| {
                terminal.failure.clone().or_else(|| {
                    terminal
                        .exit
                        .as_ref()
                        .map(|exit| format!("Terminal {}", exit.message()))
                })
            })
            .unwrap_or_else(|| {
                if running {
                    "Shell running".to_owned()
                } else {
                    "No shell".to_owned()
                }
            });
        let tabs = self
            .terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| (terminal.id, format!("Terminal {}", index + 1)))
            .collect::<Vec<_>>();
        let selected = self.selected_text();
        let terminal_size = self
            .active_session()
            .map(TerminalSession::size)
            .unwrap_or((0, 0));
        let focused = self.focus.is_focused(_window);
        let clear = cx.listener(|this, _, _, cx| this.clear(cx));
        let restart = cx.listener(|this, _, _, cx| this.restart(cx));
        let create = cx.listener(|this, _, window, cx| {
            this.new_terminal(cx);
            this.focus.focus(window, cx);
        });
        let close = cx.listener(|this, _, _, cx| this.close_terminal(cx));
        let copy = cx.listener(|this, _, _, cx| {
            this.copy_selection(cx);
        });

        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().group_box)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .h_flex()
                    .h(px(40.))
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .px(px(METRICS.space.sm as f32))
                    .child(div().h_flex().gap(px(2.)).children(tabs.into_iter().map(
                        |(id, label)| {
                            let select = cx.listener(move |this, _, window, cx| {
                                this.select_terminal(id, cx);
                                this.focus.focus(window, cx);
                            });
                            Button::new(format!("terminal-tab-{id}"))
                                .small()
                                .ghost()
                                .label(label)
                                .when(active == Some(id), |button| {
                                    button
                                        .bg(cx.theme().tab_active)
                                        .text_color(cx.theme().tab_active_foreground)
                                })
                                .when(active != Some(id), |button| {
                                    button.text_color(cx.theme().tab_foreground)
                                })
                                .on_click(select)
                        },
                    )))
                    .child(
                        Button::new("terminal-new")
                            .small()
                            .ghost()
                            .label("+ New")
                            .on_click(create),
                    )
                    .child(div().flex_1().min_w_0())
                    .child(
                        Button::new("terminal-close")
                            .small()
                            .ghost()
                            .label("Close")
                            .disabled(active.is_none())
                            .on_click(close),
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
                    .flex_1()
                    .min_h_0()
                    .p(px(METRICS.space.sm as f32))
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .min_h_0()
                            .overflow_hidden()
                            .rounded_md()
                            .border_1()
                            .border_color(if focused {
                                cx.theme().tokens.ring.color
                            } else {
                                cx.theme().border
                            })
                            .bg(cx.theme().background)
                            .font_family(mono.clone())
                            .text_size(px(metrics.size))
                            .track_focus(&self.focus)
                            .key_context("Terminal")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                    this.handle_mouse_down(event, window, cx)
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                    this.handle_mouse_down(event, window, cx)
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                    this.handle_mouse_down(event, window, cx)
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                                let mode = this.mouse_mode();
                                let report = match mode {
                                    TerminalMouseMode::AnyMotion => Some(
                                        event
                                            .pressed_button
                                            .and_then(mouse_button_code)
                                            .unwrap_or(3),
                                    ),
                                    TerminalMouseMode::ButtonMotion => {
                                        event.pressed_button.and_then(mouse_button_code)
                                    }
                                    _ => None,
                                };
                                if let Some(button) = report {
                                    if let Some(point) = this.point_at(event.position)
                                        && this.last_mouse_report != Some((point, button))
                                    {
                                        this.send_mouse_report(
                                            button,
                                            point,
                                            event.modifiers,
                                            false,
                                            true,
                                        );
                                        this.last_mouse_report = Some((point, button));
                                    }
                                    return;
                                }
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
                                cx.listener(|this, event: &MouseUpEvent, _, _| {
                                    this.handle_mouse_up(event)
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseUpEvent, _, _| {
                                    this.handle_mouse_up(event)
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Middle,
                                cx.listener(|this, event: &MouseUpEvent, _, _| {
                                    this.handle_mouse_up(event)
                                }),
                            )
                            .on_scroll(cx.listener(
                                move |this, event: &gpui_kit::ScrollEvent, _, cx| {
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
                                    if this.mouse_mode() != TerminalMouseMode::None {
                                        if lines.abs() >= f32::EPSILON
                                            && let Some(point) = this.point_at(event.position)
                                        {
                                            this.send_mouse_report(
                                                if lines < 0. { 64 } else { 65 },
                                                point,
                                                event.modifiers,
                                                false,
                                                false,
                                            );
                                        }
                                        return;
                                    }
                                    let revision = this.active_session().and_then(|session| {
                                        session.scroll(delta).then(|| session.revision())
                                    });
                                    if let Some(revision) = revision {
                                        this.selection = None;
                                        if let Some(terminal) = this.active_instance_mut() {
                                            terminal.last_revision = revision;
                                        }
                                        cx.notify();
                                    }
                                },
                            ))
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
                            .child(div().absolute().inset_0().v_flex().children(
                                rows.into_iter().map(|row| {
                                    div()
                                        .h(px(row_height))
                                        .w_full()
                                        .flex()
                                        .flex_shrink_0()
                                        .children(row.runs.into_iter().map(|run| {
                                            div()
                                                .h_full()
                                                .w(px(col_width * f32::from(run.width_cells)))
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                .bg(run.style.background)
                                                .text_color(run.style.foreground)
                                                .when(run.style.bold, |run| {
                                                    run.font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                })
                                                .when(run.style.italic, |run| run.italic())
                                                .when(run.style.underline, |run| run.underline())
                                                .when(run.style.strikethrough, |run| {
                                                    run.line_through()
                                                })
                                                .child(SharedString::from(run.text))
                                        }))
                                }),
                            ))
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
                                            active_failure.clone().unwrap_or_else(|| {
                                                "No project terminal".to_owned()
                                            }),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .h(px(26.))
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().status_bar)
                    .px(px(METRICS.space.md as f32))
                    .child(div().size(px(6.)).rounded_full().bg(if running {
                        cx.theme().success
                    } else {
                        cx.theme().muted_foreground
                    }))
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
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} × {}", terminal_size.0, terminal_size.1)),
                    ),
            )
    }
}

/// Cursor-key sequence for the terminal's DECCKM state.
///
/// Applications that enable the application cursor mode (`vim`, `less`,
/// `fzf`) expect SS3 sequences; normal mode uses the CSI forms.
fn cursor_sequence(key: &str, application_cursor: bool) -> Option<&'static str> {
    if !matches!(key, "home" | "end" | "up" | "down" | "right" | "left") {
        return None;
    }
    let application = match key {
        "home" => "\x1bOH",
        "end" => "\x1bOF",
        "up" => "\x1bOA",
        "down" => "\x1bOB",
        "right" => "\x1bOC",
        "left" => "\x1bOD",
        _ => return None,
    };
    let normal = match key {
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        _ => return None,
    };
    Some(if application_cursor {
        application
    } else {
        normal
    })
}

fn modified_cursor_sequence(key: &str, modifier: u8) -> Option<String> {
    let final_byte = match key {
        "home" => 'H',
        "end" => 'F',
        "up" => 'A',
        "down" => 'B',
        "right" => 'C',
        "left" => 'D',
        _ => return None,
    };
    Some(format!("\x1b[1;{modifier}{final_byte}"))
}

fn modified_tilde_sequence(code: u8, modifier: u8) -> Vec<u8> {
    format!("\x1b[{code};{modifier}~").into_bytes()
}

fn tilde_sequence(code: u8, modifier: Option<u8>) -> Vec<u8> {
    modifier.map_or_else(
        || format!("\x1b[{code}~").into_bytes(),
        |modifier| modified_tilde_sequence(code, modifier),
    )
}

fn function_sequence(number: u8, modifier: Option<u8>) -> Option<Vec<u8>> {
    let (code, final_byte) = match number {
        1 => (None, Some('P')),
        2 => (None, Some('Q')),
        3 => (None, Some('R')),
        4 => (None, Some('S')),
        5 => (Some(15), None),
        6 => (Some(17), None),
        7 => (Some(18), None),
        8 => (Some(19), None),
        9 => (Some(20), None),
        10 => (Some(21), None),
        11 => (Some(23), None),
        12 => (Some(24), None),
        _ => return None,
    };
    let sequence = match (modifier, code, final_byte) {
        (Some(modifier), Some(code), _) => modified_tilde_sequence(code, modifier),
        (Some(modifier), _, Some(final_byte)) => {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        }
        (None, Some(code), _) => tilde_sequence(code, None),
        (None, _, Some(final_byte)) => format!("\x1bO{final_byte}").into_bytes(),
        _ => return None,
    };
    Some(sequence)
}

/// Clipboard payload, bracketed when the child enabled paste mode.
fn paste_payload(text: &str, bracketed: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 12);
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    bytes.extend_from_slice(text.as_bytes());
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    bytes
}

fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

fn encode_mouse_report(
    button: u8,
    point: GridPoint,
    release: bool,
    encoding: TerminalMouseEncoding,
) -> Vec<u8> {
    let col = u32::from(point.col) + 1;
    let row = u32::from(point.row) + 1;
    if encoding == TerminalMouseEncoding::Sgr {
        let suffix = if release { 'm' } else { 'M' };
        return format!("\x1b[<{button};{col};{row}{suffix}").into_bytes();
    }

    let mut report = vec![0x1b, b'[', b'M'];
    let coordinates = [u32::from(button) + 32, col + 32, row + 32];
    for coordinate in coordinates {
        if encoding == TerminalMouseEncoding::Utf8 {
            if let Some(character) = char::from_u32(coordinate) {
                let mut bytes = [0; 4];
                report.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
            }
        } else {
            report.push(coordinate.min(u32::from(u8::MAX)) as u8);
        }
    }
    report
}

/// Key translation from GPUI keystrokes to terminal byte sequences.
fn translate_key(event: &gpui_kit::KeystrokeEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let key = event.keystroke.key.as_str();
    let modifiers = event.keystroke.modifiers;
    let control = modifiers.control;
    let alt = modifiers.alt;
    let platform = modifiers.platform;
    let modifier = (modifiers.shift || control || alt || platform).then(|| {
        1 + u8::from(modifiers.shift)
            + 2 * u8::from(alt)
            + 4 * u8::from(control)
            + 8 * u8::from(platform)
    });
    let function = key.starts_with('f') && key.len() > 1 && key[1..].parse::<u8>().is_ok();
    if function {
        let number: u8 = key[1..].parse().ok()?;
        return function_sequence(number, modifier);
    }
    match key {
        "enter" => return Some(vec![b'\r']),
        "tab" if modifiers.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return Some(vec![b'\t']),
        "backspace" => return Some(vec![0x7f]),
        "escape" => return Some(vec![0x1b]),
        "delete" => return Some(tilde_sequence(3, modifier)),
        "insert" => return Some(tilde_sequence(2, modifier)),
        "pageup" => return Some(tilde_sequence(5, modifier)),
        "pagedown" => return Some(tilde_sequence(6, modifier)),
        "home" | "end" | "up" | "down" | "right" | "left" => {
            if let Some(modifier) = modifier {
                return modified_cursor_sequence(key, modifier).map(String::into_bytes);
            }
            return cursor_sequence(key, application_cursor)
                .map(|sequence| sequence.as_bytes().to_vec());
        }
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
fn ansi_color(color: TerminalColor, default: Hsla) -> Hsla {
    match color {
        TerminalColor::Default => default,
        TerminalColor::Rgb(red, green, blue) => gpui_kit::rgba(
            u32::from(red) << 24 | u32::from(green) << 16 | u32::from(blue) << 8 | 0xff,
        )
        .into(),
        TerminalColor::Indexed(index) => indexed_color(index).into(),
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

    fn plain_style() -> RunStyle {
        RunStyle {
            foreground: gpui_kit::hsla(0., 0., 1., 1.),
            background: gpui_kit::hsla(0., 0., 0., 1.),
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            wide: false,
            highlight: false,
        }
    }

    #[test]
    fn application_cursor_mode_changes_arrow_and_home_end_bytes() {
        assert_eq!(cursor_sequence("up", false), Some("\x1b[A"));
        assert_eq!(cursor_sequence("up", true), Some("\x1bOA"));
        assert_eq!(cursor_sequence("left", false), Some("\x1b[D"));
        assert_eq!(cursor_sequence("left", true), Some("\x1bOD"));
        assert_eq!(cursor_sequence("home", false), Some("\x1b[H"));
        assert_eq!(cursor_sequence("home", true), Some("\x1bOH"));
        assert_eq!(cursor_sequence("end", true), Some("\x1bOF"));
        assert_eq!(cursor_sequence("pageup", true), None);
        assert_eq!(cursor_sequence("a", true), None);
    }

    #[test]
    fn modified_keys_encode_xterm_modifier_parameters() {
        assert_eq!(
            modified_cursor_sequence("left", 5).as_deref(),
            Some("\x1b[1;5D")
        );
        assert_eq!(tilde_sequence(5, Some(3)), b"\x1b[5;3~");
        assert_eq!(
            function_sequence(1, Some(2)).as_deref(),
            Some(b"\x1b[1;2P".as_slice())
        );
        assert_eq!(
            function_sequence(12, None).as_deref(),
            Some(b"\x1b[24~".as_slice())
        );
    }

    #[test]
    fn paste_is_bracketed_only_when_the_child_enabled_it() {
        assert_eq!(paste_payload("hello", false), b"hello".to_vec());
        assert_eq!(
            paste_payload("hello", true),
            b"\x1b[200~hello\x1b[201~".to_vec()
        );
    }

    #[test]
    fn mouse_reports_follow_sgr_and_legacy_terminal_encodings() {
        let point = GridPoint { row: 2, col: 1 };
        assert_eq!(
            encode_mouse_report(0, point, false, TerminalMouseEncoding::Sgr),
            b"\x1b[<0;2;3M"
        );
        assert_eq!(
            encode_mouse_report(3, point, true, TerminalMouseEncoding::Sgr),
            b"\x1b[<3;2;3m"
        );
        assert_eq!(
            encode_mouse_report(0, point, false, TerminalMouseEncoding::Default),
            b"\x1b[M \"#"
        );
    }

    #[test]
    fn a_plain_line_coalesces_into_one_run() {
        let mut runs = Vec::new();
        for _ in 0..80 {
            RunView::push(&mut runs, " ", 1, plain_style());
        }
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text.len(), 80);
        assert_eq!(runs[0].width_cells, 80);
    }

    #[test]
    fn style_highlight_and_wide_cells_split_runs_at_their_boundaries() {
        let mut runs = Vec::new();
        for _ in 0..4 {
            RunView::push(&mut runs, "a", 1, plain_style());
        }
        let mut bold = plain_style();
        bold.bold = true;
        RunView::push(&mut runs, "b", 1, bold);
        for _ in 0..4 {
            RunView::push(&mut runs, "c", 1, plain_style());
        }
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].text, "b");
        assert!(runs[1].style.bold);

        let mut highlighted = Vec::new();
        RunView::push(&mut highlighted, "aa", 1, plain_style());
        RunView::push(&mut highlighted, "b", 1, plain_style());
        assert_eq!(highlighted.len(), 1);
        let mut selected = plain_style();
        selected.highlight = true;
        RunView::push(&mut highlighted, "s", 1, selected);
        RunView::push(&mut highlighted, "t", 1, plain_style());
        assert_eq!(
            highlighted.len(),
            3,
            "a selected cell must split the run even with equal colours"
        );

        let mut wide = plain_style();
        wide.wide = true;
        let mut cells = Vec::new();
        RunView::push(&mut cells, "x", 1, plain_style());
        RunView::push(&mut cells, "界", 2, wide);
        RunView::push(&mut cells, "界", 2, wide);
        assert_eq!(cells.len(), 2, "narrow and wide cells never share a run");
        assert_eq!(cells[1].width_cells, 4);
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

    #[gpui_kit::test]
    async fn terminal_tabs_keep_sessions_alive_and_close_only_the_selected_shell(
        cx: &mut TestAppContext,
    ) {
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
            panel.update(cx, |panel, cx| panel.attach(root.path(), cx));
        })
        .unwrap();
        let first_id = panel.read_with(cx, |panel, _| panel.active_terminal.unwrap());
        let first_pid = panel.read_with(cx, |panel, _| panel.process_id().unwrap());

        cx.update_window(handle.into(), |_, _, cx| {
            panel.update(cx, |panel, cx| panel.new_terminal(cx));
        })
        .unwrap();
        let (second_id, second_pid, count) = panel.read_with(cx, |panel, _| {
            (
                panel.active_terminal.unwrap(),
                panel.process_id().unwrap(),
                panel.terminals.len(),
            )
        });
        assert_ne!(first_id, second_id);
        assert_ne!(first_pid, second_pid);
        assert_eq!(count, 2);

        cx.update_window(handle.into(), |_, _, cx| {
            panel.update(cx, |panel, cx| {
                panel.select_terminal(first_id, cx);
                assert_eq!(panel.process_id(), Some(first_pid));
                panel.select_terminal(second_id, cx);
                panel.close_terminal(cx);
            });
        })
        .unwrap();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.terminals.len(), 1);
            assert_eq!(panel.active_terminal, Some(first_id));
            assert_eq!(panel.process_id(), Some(first_pid));
        });
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
