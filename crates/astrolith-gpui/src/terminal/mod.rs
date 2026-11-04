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

use astrolith_terminal::TerminalSession;
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

/// Terminal font size in logical pixels.
const FONT_SIZE: f32 = METRICS.type_scale.mono;
/// Monospace advance used for the cell grid; PTY columns follow this width.
const CELL_WIDTH: f32 = 7.8;
/// Row height in logical pixels.
const LINE_HEIGHT: f32 = 17.;
/// Scroll rows per wheel line.
const WHEEL_ROWS: i32 = 3;

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
    selection: Option<Selection>,
    selecting: bool,
    status: Option<String>,
    failure: Option<String>,
    poll_task: Option<Task<()>>,
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
            selection: None,
            selecting: false,
            status: None,
            failure: None,
            poll_task: None,
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

    /// Whether a live session exists.
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.session.is_some()
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

    fn stop(&mut self) {
        self.session = None;
        self.selection = None;
        self.status = None;
        self.failure = None;
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

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.poll_task.is_some() {
            return;
        }
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep_going = this.update(cx, |this, cx| this.poll(cx)).unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let (cols, rows) = self.grid_size();
        let Some(session) = &mut self.session else {
            return false;
        };
        let mut changed = false;
        if session.size() != (cols, rows) && session.resize(cols, rows).is_err() {
            self.failure = Some("Could not resize the terminal.".into());
            changed = true;
        }
        if let Some(exit) = session.poll_exit() {
            self.status = Some(exit.message());
            changed = true;
        }
        let revision = session.revision();
        if revision != self.last_revision {
            self.last_revision = revision;
            changed = true;
        }
        let exited = session.is_exited();
        if changed {
            cx.notify();
        }
        if exited {
            self.poll_task = None;
        }
        !exited
    }

    fn grid_size(&self) -> (u16, u16) {
        let Some(bounds) = self.last_bounds.get() else {
            return (80, 24);
        };
        let cols = (f32::from(bounds.size.width) / CELL_WIDTH).floor() as u16;
        let rows = (f32::from(bounds.size.height) / LINE_HEIGHT).floor() as u16;
        (cols.clamp(20, 500), rows.clamp(3, 200))
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
        let (cols, rows) = self.grid_size();
        Some(GridPoint {
            row: ((y / LINE_HEIGHT).floor() as u16).min(rows.saturating_sub(1)),
            col: ((x / CELL_WIDTH).floor() as u16).min(cols.saturating_sub(1)),
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
        let mono = SharedString::from("monospace");
        let rows = self.render_rows(_window, cx);
        let bounds_cell = self.last_bounds.clone();
        let running = self.session.is_some();
        let exit = self
            .session
            .as_ref()
            .and_then(|session| session.exit())
            .or_else(|| {
                self.status
                    .clone()
                    .map(|message| astrolith_terminal::ExitInfo {
                        code: None,
                        detail: Some(message),
                    })
            });
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
                    .text_size(px(FONT_SIZE))
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
                    .on_scroll(cx.listener(|this, event: &gpui_kit::ScrollEvent, _, cx| {
                        let lines = match event.delta {
                            gpui_kit::ScrollDelta::Lines(lines) => lines.y,
                            gpui_kit::ScrollDelta::Pixels(pixels) => {
                                f32::from(pixels.y) / LINE_HEIGHT
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
                    }))
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
                                    .h(px(LINE_HEIGHT))
                                    .w_full()
                                    .flex()
                                    .flex_shrink_0()
                                    .children(row.cells.into_iter().map(|cell| {
                                        div()
                                            .h_full()
                                            .w(px(CELL_WIDTH * f32::from(cell.width_cells)))
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
