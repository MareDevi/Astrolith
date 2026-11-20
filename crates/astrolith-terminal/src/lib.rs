//! Project-bound interactive terminal service.
//!
//! The service owns one PTY child, a writable master and a bounded ANSI screen.
//! It never depends on GPUI: the reader thread parses output under a small lock
//! and bumps a revision, while the view polls that revision and renders cells.

use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(unix)]
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{Config, Term, TermMode, cell::Flags, color::Colors},
    vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb, StdSyncHandler},
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Default visible rows when the view has not measured itself yet.
pub const DEFAULT_ROWS: u16 = 24;
/// Default visible columns when the view has not measured itself yet.
pub const DEFAULT_COLS: u16 = 80;
/// Bounded scrollback retained per terminal.
pub const SCROLLBACK_CAPACITY: usize = 2_000;
/// Maximum bytes parsed per reader batch.
const READ_BUFFER: usize = 16 * 1024;
/// Maximum number of PTY writes waiting for the writer thread.
const INPUT_QUEUE_CAPACITY: usize = 256;
/// Queue slots kept available for terminal protocol replies.
const CONTROL_REPLY_RESERVE: usize = 16;
/// Maximum bytes accepted by the PTY input queue, including the active write.
const INPUT_QUEUE_BYTES: usize = 8 * 1024 * 1024;
/// Queue bytes kept available for terminal protocol replies.
const CONTROL_REPLY_BYTE_RESERVE: usize = 64 * 1024;
/// Grace between `SIGHUP` and `SIGKILL` for the child's process group.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(150);
/// Upper bound for reaping the child after the group was signalled.
const REAP_TIMEOUT: Duration = Duration::from_millis(800);
/// Upper bound for the reader to drain after the process tree is gone.
const READER_TIMEOUT: Duration = Duration::from_millis(400);
/// Upper bound for the writer to drain after the process tree is gone.
const WRITER_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Clone, Copy)]
enum InputKind {
    Interactive,
    Control,
}

#[derive(Clone, Copy)]
struct InputLimits {
    messages: usize,
    interactive_messages: usize,
    bytes: usize,
    interactive_bytes: usize,
}

struct InputQueue {
    pending: VecDeque<Vec<u8>>,
    pending_bytes: usize,
    in_flight: usize,
    closed: bool,
}

#[derive(Clone)]
struct PtyInput {
    queue: Arc<(Mutex<InputQueue>, Condvar)>,
    limits: InputLimits,
}

impl PtyInput {
    fn spawn(
        mut writer: Box<dyn Write + Send>,
        capacity: usize,
        control_reserve: usize,
        byte_capacity: usize,
        control_byte_reserve: usize,
    ) -> io::Result<(Self, JoinHandle<()>)> {
        let capacity = capacity.max(1);
        let byte_capacity = byte_capacity.max(1);
        let limits = InputLimits {
            messages: capacity,
            interactive_messages: capacity.saturating_sub(control_reserve),
            bytes: byte_capacity,
            interactive_bytes: byte_capacity.saturating_sub(control_byte_reserve),
        };
        let queue = Arc::new((
            Mutex::new(InputQueue {
                pending: VecDeque::new(),
                pending_bytes: 0,
                in_flight: 0,
                closed: false,
            }),
            Condvar::new(),
        ));
        let input = Self {
            queue: queue.clone(),
            limits,
        };
        let worker_input = input.clone();
        let worker = thread::Builder::new()
            .name("astrolith-terminal-writer".into())
            .spawn(move || {
                loop {
                    let bytes = {
                        let (lock, ready) = &*queue;
                        let mut queue = lock
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        loop {
                            if queue.closed {
                                return;
                            }
                            if let Some(bytes) = queue.pending.pop_front() {
                                queue.in_flight += 1;
                                break bytes;
                            }
                            queue = ready
                                .wait(queue)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                    };
                    let result = writer.write_all(&bytes).and_then(|_| writer.flush());
                    worker_input.finish_write(bytes.len());
                    if result.is_err() {
                        worker_input.shutdown();
                        return;
                    }
                }
            })
            .map_err(io::Error::other)?;
        Ok((input, worker))
    }

    fn write(&self, bytes: &[u8], kind: InputKind) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.check_available(bytes.len(), kind)?;
        self.enqueue(bytes.to_vec(), kind)
    }

    fn limits(&self, kind: InputKind) -> (usize, usize) {
        match kind {
            InputKind::Interactive => (
                self.limits.interactive_messages,
                self.limits.interactive_bytes,
            ),
            InputKind::Control => (self.limits.messages, self.limits.bytes),
        }
    }

    fn check_size(bytes: usize, byte_limit: usize) -> io::Result<()> {
        if bytes > byte_limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("terminal input is {bytes} bytes, above the {byte_limit}-byte limit"),
            ));
        }
        Ok(())
    }

    fn check_capacity(
        queue: &InputQueue,
        bytes: usize,
        message_limit: usize,
        byte_limit: usize,
    ) -> io::Result<()> {
        if queue.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal session is shut down",
            ));
        }
        if queue.pending.len().saturating_add(queue.in_flight) >= message_limit
            || queue.pending_bytes.saturating_add(bytes) > byte_limit
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal input queue is full",
            ));
        }
        Ok(())
    }

    fn check_available(&self, bytes: usize, kind: InputKind) -> io::Result<()> {
        let (message_limit, byte_limit) = self.limits(kind);
        Self::check_size(bytes, byte_limit)?;
        let (lock, _) = &*self.queue;
        let queue = lock
            .lock()
            .map_err(|_| io::Error::other("terminal input queue lock poisoned"))?;
        Self::check_capacity(&queue, bytes, message_limit, byte_limit)
    }

    fn enqueue(&self, bytes: Vec<u8>, kind: InputKind) -> io::Result<()> {
        let (message_limit, byte_limit) = self.limits(kind);
        Self::check_size(bytes.len(), byte_limit)?;
        let (lock, ready) = &*self.queue;
        let mut queue = lock
            .lock()
            .map_err(|_| io::Error::other("terminal input queue lock poisoned"))?;
        Self::check_capacity(&queue, bytes.len(), message_limit, byte_limit)?;
        queue.pending_bytes += bytes.len();
        queue.pending.push_back(bytes);
        ready.notify_one();
        Ok(())
    }

    fn write_paste(&self, text: &str, bracketed: bool) -> io::Result<()> {
        let bracketed = bracketed.then_some((b"\x1b[200~", b"\x1b[201~"));
        let extra = bracketed.map_or(0, |(start, end)| start.len() + end.len());
        let length = text.len().checked_add(extra).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal paste size overflowed",
            )
        })?;
        self.check_available(length, InputKind::Interactive)?;
        let mut bytes = Vec::with_capacity(length);
        if let Some((start, end)) = bracketed {
            bytes.extend_from_slice(start);
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(end);
        } else {
            bytes.extend_from_slice(text.as_bytes());
        }
        self.enqueue(bytes, InputKind::Interactive)
    }

    fn finish_write(&self, bytes: usize) {
        let (lock, _) = &*self.queue;
        let mut queue = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.in_flight = queue.in_flight.saturating_sub(1);
        queue.pending_bytes = queue.pending_bytes.saturating_sub(bytes);
    }

    fn shutdown(&self) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.closed = true;
        queue.pending.clear();
        queue.pending_bytes = 0;
        queue.in_flight = 0;
        ready.notify_one();
    }
}

#[derive(Clone)]
struct TerminalEventProxy {
    input: PtyInput,
    revision: Arc<AtomicU64>,
    title: Arc<Mutex<Option<String>>>,
    cols: Arc<AtomicU16>,
    rows: Arc<AtomicU16>,
}

impl TerminalEventProxy {
    fn write_reply(&self, text: &str) {
        let _ = self.input.write(text.as_bytes(), InputKind::Control);
    }
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.write_reply(&text),
            Event::Title(title) => {
                if let Ok(mut current) = self.title.lock() {
                    *current = Some(title);
                }
            }
            Event::ResetTitle => {
                if let Ok(mut current) = self.title.lock() {
                    *current = None;
                }
            }
            Event::TextAreaSizeRequest(formatter) => self.write_reply(&formatter(WindowSize {
                num_lines: self.rows.load(Ordering::Acquire),
                num_cols: self.cols.load(Ordering::Acquire),
                cell_width: 0,
                cell_height: 0,
            })),
            Event::ColorRequest(index, formatter) => {
                self.write_reply(&formatter(xterm_rgb(index as u8)));
            }
            Event::Wakeup => {
                self.revision.fetch_add(1, Ordering::Release);
            }
            Event::MouseCursorDirty
            | Event::ClipboardStore(_, _)
            | Event::ClipboardLoad(_, _)
            | Event::CursorBlinkingChange
            | Event::Bell
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}

#[derive(Clone, Copy)]
struct TerminalGridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TerminalGridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// One terminal session bound to a project root.
pub struct TerminalSession {
    root: PathBuf,
    /// Master side of the PTY; taken (and closed) before shutdown signals the
    /// child so no new input can reach a dying shell.
    master: Option<Box<dyn MasterPty + Send>>,
    input: Option<PtyInput>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    screen: Arc<Mutex<Term<TerminalEventProxy>>>,
    title: Arc<Mutex<Option<String>>>,
    revision: Arc<AtomicU64>,
    exited: Arc<AtomicBool>,
    exit: Arc<Mutex<Option<ExitInfo>>>,
    rows: u16,
    cols: u16,
    event_cols: Arc<AtomicU16>,
    event_rows: Arc<AtomicU16>,
    reader: Option<JoinHandle<()>>,
    writer_thread: Option<JoinHandle<()>>,
}

/// How a terminal child ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    /// Exit code when the platform reports one.
    pub code: Option<u32>,
    /// Signal or platform description when available.
    pub detail: Option<String>,
}

/// ANSI color resolved by a terminal backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColor {
    /// Use the application terminal foreground or background.
    Default,
    /// 24-bit RGB color.
    Rgb(u8, u8, u8),
    /// xterm 256-color palette index.
    Indexed(u8),
}

/// Mouse reporting requested by the running terminal application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalMouseMode {
    /// Keep mouse input local for selection and scrollback.
    #[default]
    None,
    /// Report button presses.
    Press,
    /// Report button presses and releases.
    PressRelease,
    /// Also report movement while a button is held.
    ButtonMotion,
    /// Report all movement.
    AnyMotion,
}

/// Wire encoding selected for mouse reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalMouseEncoding {
    /// Legacy single-byte coordinate encoding.
    #[default]
    Default,
    /// UTF-8 coordinate encoding.
    Utf8,
    /// SGR coordinate encoding.
    Sgr,
}

/// Backend-neutral terminal cell used by the renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    contents: String,
    foreground: TerminalColor,
    background: TerminalColor,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
    wide: bool,
    wide_continuation: bool,
}

impl TerminalCell {
    /// Cell contents, which may be empty for a styled blank cell.
    pub fn contents(&self) -> &str {
        &self.contents
    }
    /// Foreground color.
    pub fn foreground(&self) -> TerminalColor {
        self.foreground
    }
    /// Background color.
    pub fn background(&self) -> TerminalColor {
        self.background
    }
    /// Whether the cell is bold.
    pub fn bold(&self) -> bool {
        self.bold
    }
    /// Whether the cell is dimmed.
    pub fn dim(&self) -> bool {
        self.dim
    }
    /// Whether the cell is italic.
    pub fn italic(&self) -> bool {
        self.italic
    }
    /// Whether the cell is underlined.
    pub fn underline(&self) -> bool {
        self.underline
    }
    /// Whether the cell has a strike-through decoration.
    pub fn strikethrough(&self) -> bool {
        self.strikethrough
    }
    /// Whether foreground and background are inverted.
    pub fn inverse(&self) -> bool {
        self.inverse
    }
    /// Whether the cell occupies two columns.
    pub fn wide(&self) -> bool {
        self.wide
    }
    /// Whether this cell is the trailing half of a wide glyph.
    pub fn wide_continuation(&self) -> bool {
        self.wide_continuation
    }
}

/// Immutable, backend-neutral copy of the visible terminal grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSnapshot {
    cells: Vec<Option<TerminalCell>>,
    cols: u16,
    rows: u16,
    cursor: (u16, u16),
    cursor_hidden: bool,
    live: bool,
}

impl TerminalSnapshot {
    /// Visible grid dimensions.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }
    /// Cursor position as `(row, column)`.
    pub fn cursor(&self) -> (u16, u16) {
        self.cursor
    }
    /// Whether the terminal requested a hidden cursor.
    pub fn cursor_hidden(&self) -> bool {
        self.cursor_hidden
    }
    /// Whether the viewport is at live output.
    pub fn is_live(&self) -> bool {
        self.live
    }
    /// Cell at `(row, column)`, if the point is inside the grid.
    pub fn cell(&self, row: u16, col: u16) -> Option<&TerminalCell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
            .and_then(Option::as_ref)
    }
}

impl ExitInfo {
    /// A signal death carries no exit code: `portable-pty` fabricates one, and
    /// reporting it would make `SIGINT` look like "exited with code 1".
    fn from_status(status: portable_pty::ExitStatus) -> Self {
        let detail = status.signal().map(str::to_owned);
        Self {
            code: detail.is_none().then(|| status.exit_code()),
            detail,
        }
    }

    /// Compact user-facing description.
    pub fn message(&self) -> String {
        match (&self.code, &self.detail) {
            (Some(code), Some(signal)) => format!("exited with code {code} ({signal})"),
            (Some(code), None) => format!("exited with code {code}"),
            (None, Some(signal)) => format!("terminated by {signal}"),
            (None, None) => "exited".to_owned(),
        }
    }
}

impl TerminalSession {
    /// Spawn the user's shell in `root`.
    ///
    /// On Unix the shell starts as a login, interactive shell (`-l -i`) so it
    /// reads the user's profile and enables job control, matching a normal
    /// terminal window. The child is a session and process-group leader
    /// (`portable-pty` calls `setsid` and `TIOCSCTTY`), which is what lets
    /// [`TerminalSession::kill`] signal the whole foreground tree.
    pub fn spawn(root: &Path, cols: u16, rows: u16) -> io::Result<Self> {
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    "cmd.exe".into()
                } else {
                    "/bin/sh".into()
                }
            });
        let mut command = CommandBuilder::new(shell);
        #[cfg(unix)]
        {
            command.arg("-l");
            command.arg("-i");
        }
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        Self::spawn_command(root, cols, rows, command)
    }

    /// Spawn an explicit command. Used by the shell and by service tests.
    pub fn spawn_command(
        root: &Path,
        cols: u16,
        rows: u16,
        command: CommandBuilder,
    ) -> io::Result<Self> {
        let rows = rows.max(2);
        let cols = cols.max(2);
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(io::Error::other)?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let (input, writer) = PtyInput::spawn(
            writer,
            INPUT_QUEUE_CAPACITY,
            CONTROL_REPLY_RESERVE,
            INPUT_QUEUE_BYTES,
            CONTROL_REPLY_BYTE_RESERVE,
        )?;
        let revision = Arc::new(AtomicU64::new(0));
        let exited = Arc::new(AtomicBool::new(false));
        let exit = Arc::new(Mutex::new(None));
        let title = Arc::new(Mutex::new(None));
        let event_cols = Arc::new(AtomicU16::new(cols));
        let event_rows = Arc::new(AtomicU16::new(rows));
        let event_proxy = TerminalEventProxy {
            input: input.clone(),
            revision: revision.clone(),
            title: title.clone(),
            cols: event_cols.clone(),
            rows: event_rows.clone(),
        };
        let grid_size = TerminalGridSize {
            cols: usize::from(cols),
            rows: usize::from(rows),
        };
        let screen = Arc::new(Mutex::new(Term::new(
            Config {
                scrolling_history: SCROLLBACK_CAPACITY,
                osc52: alacritty_terminal::term::Osc52::Disabled,
                ..Config::default()
            },
            &grid_size,
            event_proxy,
        )));
        let reader_screen = screen.clone();
        let reader_revision = revision.clone();
        let reader_exited = exited.clone();
        let reader = thread::Builder::new()
            .name("astrolith-terminal-reader".into())
            .spawn(move || {
                let mut buffer = [0u8; READ_BUFFER];
                let mut parser = Processor::<StdSyncHandler>::new();
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            let mut screen = reader_screen
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            parser.advance(&mut *screen, &buffer[..read]);
                            reader_revision.fetch_add(1, Ordering::Release);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                reader_exited.store(true, Ordering::Release);
                reader_revision.fetch_add(1, Ordering::Release);
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                input.shutdown();
                wait_for_thread(Some(writer), WRITER_TIMEOUT);
                return Err(io::Error::other(error));
            }
        };
        Ok(Self {
            root: root.to_path_buf(),
            master: Some(pair.master),
            input: Some(input),
            child: Arc::new(Mutex::new(child)),
            screen,
            title,
            revision,
            exited,
            exit,
            rows,
            cols,
            event_cols,
            event_rows,
            reader: Some(reader),
            writer_thread: Some(writer),
        })
    }

    /// Project root this terminal belongs to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Current visible size in `(columns, rows)`.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Rendered output revision; changes whenever new bytes were parsed.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Whether the child has exited or the reader reached end of output.
    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// Current application title requested by the terminal child.
    pub fn title(&self) -> Option<String> {
        self.title.lock().ok().and_then(|title| title.clone())
    }

    /// Number of retained scrollback rows.
    pub fn scrollback_len(&self) -> usize {
        self.screen
            .lock()
            .map(|screen| screen.history_size())
            .unwrap_or(0)
    }

    /// Copy the visible grid into backend-neutral renderer data.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let screen = self
            .screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let content = screen.renderable_content();
        let mut cells = vec![None; usize::from(self.rows) * usize::from(self.cols)];
        let offset = screen.grid().display_offset();
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset as i32;
            let col = indexed.point.column.0;
            if row < 0 || row >= i32::from(self.rows) || col >= usize::from(self.cols) {
                continue;
            }
            let cell = indexed.cell;
            let mut contents = if cell.c == ' ' {
                String::new()
            } else {
                cell.c.to_string()
            };
            if let Some(chars) = cell.zerowidth() {
                contents.extend(chars);
            }
            if !contents.is_empty()
                || cell.flags.intersects(
                    Flags::BOLD
                        | Flags::DIM
                        | Flags::ITALIC
                        | Flags::ALL_UNDERLINES
                        | Flags::STRIKEOUT
                        | Flags::INVERSE,
                )
                || cell.bg != Color::Named(NamedColor::Background)
            {
                cells[row as usize * usize::from(self.cols) + col] = Some(TerminalCell {
                    contents,
                    foreground: terminal_color(cell.fg, content.colors),
                    background: terminal_color(cell.bg, content.colors),
                    bold: cell.flags.contains(Flags::BOLD),
                    dim: cell.flags.contains(Flags::DIM),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                    strikethrough: cell.flags.contains(Flags::STRIKEOUT),
                    inverse: cell.flags.contains(Flags::INVERSE),
                    wide: cell.flags.contains(Flags::WIDE_CHAR),
                    wide_continuation: cell.flags.contains(Flags::WIDE_CHAR_SPACER),
                });
            }
        }
        let point = content.cursor.point;
        let cursor_row =
            (point.line.0 + offset as i32).clamp(0, i32::from(self.rows.saturating_sub(1))) as u16;
        TerminalSnapshot {
            cells,
            cols: self.cols,
            rows: self.rows,
            cursor: (
                cursor_row,
                point.column.0.min(usize::from(self.cols.saturating_sub(1))) as u16,
            ),
            cursor_hidden: content.cursor.shape == CursorShape::Hidden,
            live: offset == 0,
        }
    }

    /// Current screen text, excluding terminal styling.
    pub fn contents(&self) -> String {
        let snapshot = self.snapshot();
        (0..snapshot.rows)
            .map(|row| {
                (0..snapshot.cols)
                    .map(|col| snapshot.cell(row, col).map_or(" ", TerminalCell::contents))
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Text between two visible grid points, inclusive.
    pub fn contents_between(&self, start: (u16, u16), end: (u16, u16)) -> String {
        let screen = self
            .screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let offset = screen.grid().display_offset() as i32;
        screen.bounds_to_string(
            Point::new(Line(start.0 as i32 - offset), Column(start.1 as usize)),
            Point::new(Line(end.0 as i32 - offset), Column(end.1 as usize)),
        )
    }

    /// Whether application cursor key sequences are enabled.
    pub fn application_cursor(&self) -> bool {
        self.screen
            .lock()
            .map(|screen| screen.mode().contains(TermMode::APP_CURSOR))
            .unwrap_or(false)
    }

    /// Whether bracketed paste is enabled.
    pub fn bracketed_paste(&self) -> bool {
        self.screen
            .lock()
            .map(|screen| screen.mode().contains(TermMode::BRACKETED_PASTE))
            .unwrap_or(false)
    }

    /// Current mouse reporting mode and wire encoding.
    pub fn mouse_protocol(&self) -> (TerminalMouseMode, TerminalMouseEncoding) {
        let Ok(screen) = self.screen.lock() else {
            return (TerminalMouseMode::None, TerminalMouseEncoding::Default);
        };
        let mode = if screen.mode().contains(TermMode::MOUSE_MOTION) {
            TerminalMouseMode::AnyMotion
        } else if screen.mode().contains(TermMode::MOUSE_DRAG) {
            TerminalMouseMode::ButtonMotion
        } else if screen.mode().contains(TermMode::MOUSE_REPORT_CLICK) {
            TerminalMouseMode::PressRelease
        } else {
            TerminalMouseMode::None
        };
        let encoding = if screen.mode().contains(TermMode::SGR_MOUSE) {
            TerminalMouseEncoding::Sgr
        } else if screen.mode().contains(TermMode::UTF8_MOUSE) {
            TerminalMouseEncoding::Utf8
        } else {
            TerminalMouseEncoding::Default
        };
        (mode, encoding)
    }

    /// Queue bytes for the child without blocking the caller.
    ///
    /// A full queue reports [`io::ErrorKind::WouldBlock`]; terminal protocol
    /// replies use the reserved slots so GUI input cannot starve them.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.input
            .as_ref()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "terminal session is shut down")
            })?
            .write(bytes, InputKind::Interactive)
    }

    /// Maximum interactive input payload accepted by one write, in bytes.
    pub fn input_limit(&self) -> usize {
        self.input
            .as_ref()
            .map_or(0, |input| input.limits.interactive_bytes)
    }

    /// Queue a paste, framing it for bracketed-paste mode when requested.
    ///
    /// The size and queue capacity are checked before the framed payload is
    /// allocated.
    pub fn write_paste(&self, text: &str, bracketed: bool) -> io::Result<()> {
        self.input
            .as_ref()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "terminal session is shut down")
            })?
            .write_paste(text, bracketed)
    }

    /// Resize both the pseudo-terminal and the parsed screen.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let rows = rows.max(2);
        let cols = cols.max(2);
        if (cols, rows) == (self.cols, self.rows) {
            return Ok(());
        }
        let Some(master) = self.master.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal session is shut down",
            ));
        };
        master.resize(size(cols, rows)).map_err(io::Error::other)?;
        let mut screen = self
            .screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        screen.resize(TerminalGridSize {
            cols: usize::from(cols),
            rows: usize::from(rows),
        });
        self.event_cols.store(cols, Ordering::Release);
        self.event_rows.store(rows, Ordering::Release);
        self.cols = cols;
        self.rows = rows;
        self.revision.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Scroll the viewport back by `delta` rows; negative moves toward live output.
    pub fn scroll(&self, delta: i32) -> bool {
        let Ok(mut screen) = self.screen.lock() else {
            return false;
        };
        let current = screen.grid().display_offset();
        screen.scroll_display(Scroll::Delta(delta));
        if screen.grid().display_offset() == current {
            return false;
        }
        self.revision.fetch_add(1, Ordering::Release);
        true
    }

    /// Clear the visible screen and scrollback without touching the child.
    pub fn clear(&self) {
        if let Ok(mut screen) = self.screen.lock() {
            screen.grid_mut().reset::<Color>();
        }
        self.revision.fetch_add(1, Ordering::Release);
    }

    /// Jump to the live output.
    pub fn scroll_to_bottom(&self) {
        if let Ok(mut screen) = self.screen.lock() {
            screen.scroll_display(Scroll::Bottom);
        }
    }

    /// Poll the child once and record its exit.
    pub fn poll_exit(&self) -> Option<ExitInfo> {
        if let Ok(exit) = self.exit.lock()
            && exit.is_some()
        {
            return exit.clone();
        }
        let status = {
            let Ok(mut child) = self.child.lock() else {
                return None;
            };
            child.try_wait().ok().flatten()
        };
        let status = status?;
        let info = ExitInfo::from_status(status);
        self.exited.store(true, Ordering::Release);
        if let Ok(mut slot) = self.exit.lock() {
            *slot = Some(info.clone());
        }
        self.revision.fetch_add(1, Ordering::Release);
        Some(info)
    }

    /// Child process id, when the platform exposes one.
    pub fn process_id(&self) -> Option<u32> {
        self.child.lock().ok().and_then(|child| child.process_id())
    }

    /// Last observed exit, if any.
    pub fn exit(&self) -> Option<ExitInfo> {
        self.exit.lock().ok().and_then(|exit| exit.clone())
    }

    /// Terminate the child's process group and stop the reader.
    ///
    /// The child is a session and group leader, so its pid is also its group
    /// id: `SIGHUP` reaches the foreground job and every descendant that did
    /// not create its own session, and `SIGKILL` follows after [`KILL_GRACE`]
    /// so a job that ignores the hangup cannot outlive the terminal. The
    /// grace, the reap and the reader handoff all run on a short-lived reaper
    /// thread; the caller only closes the master handles and posts the first
    /// signal, so this is safe from `Drop` and from the UI thread.
    pub fn kill(&mut self) {
        // Closing our handles first lets the child notice the terminal went
        // away and keeps new input from reaching a dying shell.
        if let Some(input) = self.input.take() {
            input.shutdown();
        }
        self.master = None;
        let child = self.child.clone();
        let fallback_child = child.clone();
        let exit_slot = self.exit.clone();
        let revision = self.revision.clone();
        let reader = self.reader.take();
        let writer = self.writer_thread.take();

        #[cfg(unix)]
        let pgid = child
            .lock()
            .ok()
            .and_then(|child| child.process_id())
            .map(|pid| pid as i32);
        #[cfg(unix)]
        if let Some(pgid) = pgid {
            // A group that already disappeared is not an error.
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGHUP);
        }
        #[cfg(not(unix))]
        {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }

        self.exited.store(true, Ordering::Release);
        self.revision.fetch_add(1, Ordering::Release);

        let reaper = thread::Builder::new()
            .name("astrolith-terminal-reaper".into())
            .spawn(move || {
                #[cfg(unix)]
                if let Some(pgid) = pgid {
                    thread::sleep(KILL_GRACE);
                    let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
                }
                let deadline = Instant::now() + REAP_TIMEOUT;
                loop {
                    let status = child
                        .lock()
                        .ok()
                        .and_then(|mut child| child.try_wait().ok().flatten());
                    if let Some(status) = status {
                        if let Ok(mut slot) = exit_slot.lock()
                            && slot.is_none()
                        {
                            *slot = Some(ExitInfo::from_status(status));
                        }
                        break;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                wait_for_thread(writer, WRITER_TIMEOUT);
                wait_for_thread(reader, READER_TIMEOUT);
                revision.fetch_add(1, Ordering::Release);
            });
        if reaper.is_err() {
            // Without a reaper no `SIGKILL` is ever sent: fall back to the
            // platform kill so the child still dies.
            if let Ok(mut child) = fallback_child.lock() {
                let _ = child.kill();
            }
        }
    }
}

fn terminal_color(color: Color, colors: &Colors) -> TerminalColor {
    let rgb = match color {
        Color::Spec(rgb) => Some(rgb),
        Color::Indexed(index) => colors[index as usize],
        Color::Named(name) => colors[name],
    };
    if let Some(Rgb { r, g, b }) = rgb {
        return TerminalColor::Rgb(r, g, b);
    }
    match color {
        Color::Indexed(index) => TerminalColor::Indexed(index),
        Color::Named(
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::Cursor
            | NamedColor::BrightForeground
            | NamedColor::DimForeground,
        ) => TerminalColor::Default,
        Color::Named(name) => TerminalColor::Indexed(name as u8),
        Color::Spec(Rgb { r, g, b }) => TerminalColor::Rgb(r, g, b),
    }
}

fn xterm_rgb(index: u8) -> Rgb {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let (r, g, b) = match index {
        0..=15 => BASE[index as usize],
        16..=231 => {
            let n = index - 16;
            let f = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            (f(n / 36), f((n / 6) % 6), f(n % 6))
        }
        _ => {
            let v = 8 + 10 * (index - 232);
            (v, v, v)
        }
    };
    Rgb { r, g, b }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.kill();
    }
}

fn wait_for_thread(thread: Option<JoinHandle<()>>, timeout: Duration) {
    let Some(thread) = thread else {
        return;
    };
    let deadline = Instant::now() + timeout;
    while !thread.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if thread.is_finished() {
        let _ = thread.join();
    }
    // Otherwise the worker is detached instead of pinning shutdown on a
    // descendant that still holds the pty.
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

// These tests drive a real PTY through `sh`; the service itself is
// platform-neutral but the fixtures are POSIX shell scripts.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use portable_pty::CommandBuilder;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    struct GatedWriter {
        started: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        bytes: Arc<Mutex<Vec<u8>>>,
        gated: AtomicBool,
    }

    impl Write for GatedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.gated.swap(true, Ordering::AcqRel) {
                let _ = self.started.send(());
                self.release
                    .recv()
                    .map_err(|_| io::Error::other("test writer was not released"))?;
            }
            self.bytes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn ordered_writer_preserves_input_and_reserves_room_for_control_replies() {
        let (started, started_rx) = mpsc::sync_channel(0);
        let (release, release_rx) = mpsc::sync_channel(0);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = GatedWriter {
            started,
            release: release_rx,
            bytes: bytes.clone(),
            gated: AtomicBool::new(false),
        };
        let (input, worker) = PtyInput::spawn(Box::new(writer), 4, 1, 11, 5).unwrap();

        input.write(b"one", InputKind::Interactive).unwrap();
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the worker must start the first write");
        input.write(b"two", InputKind::Interactive).unwrap();

        let started_at = Instant::now();
        let error = input
            .write(b"lost", InputKind::Interactive)
            .expect_err("interactive input must stop at its bounded limit");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(started_at.elapsed() < Duration::from_millis(100));

        input.write(b"reply", InputKind::Control).unwrap();
        assert_eq!(
            input.write(b"lost", InputKind::Control).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );

        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while bytes.lock().unwrap().as_slice() != b"onetworeply" {
            assert!(Instant::now() < deadline, "writer did not drain the queue");
            thread::sleep(Duration::from_millis(5));
        }
        input.shutdown();
        worker.join().unwrap();
        assert_eq!(bytes.lock().unwrap().as_slice(), b"onetworeply");
    }

    #[test]
    fn shutdown_wakes_an_idle_writer_and_rejects_later_input() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (input, worker) =
            PtyInput::spawn(Box::new(CollectingWriter(bytes)), 4, 1, 64, 16).unwrap();
        input.write(b"queued", InputKind::Interactive).unwrap();

        input.shutdown();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline, "writer did not stop");
            thread::sleep(Duration::from_millis(5));
        }
        worker.join().unwrap();
        assert_eq!(
            input
                .write(b"late", InputKind::Interactive)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn oversized_input_is_rejected_before_copying_into_the_queue() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (input, worker) =
            PtyInput::spawn(Box::new(CollectingWriter(bytes.clone())), 4, 1, 8, 2).unwrap();
        let error = input
            .write_paste("too-big", false)
            .expect_err("an oversized paste must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("6-byte limit"), "{error}");
        assert!(bytes.lock().unwrap().is_empty());
        input.shutdown();
        worker.join().unwrap();
    }

    #[test]
    fn paste_framing_is_applied_before_enqueueing() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (input, worker) =
            PtyInput::spawn(Box::new(CollectingWriter(bytes.clone())), 4, 1, 64, 16).unwrap();
        input.write_paste("hello", false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while bytes.lock().unwrap().as_slice() != b"hello" {
            assert!(Instant::now() < deadline, "plain paste was not written");
            thread::sleep(Duration::from_millis(5));
        }
        bytes.lock().unwrap().clear();

        input.write_paste("hello", true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while bytes.lock().unwrap().as_slice() != b"\x1b[200~hello\x1b[201~" {
            assert!(Instant::now() < deadline, "bracketed paste was not written");
            thread::sleep(Duration::from_millis(5));
        }
        input.shutdown();
        worker.join().unwrap();
    }

    #[test]
    fn large_pastes_fit_within_the_payload_bound() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (input, worker) = PtyInput::spawn(
            Box::new(CollectingWriter(bytes.clone())),
            4,
            1,
            1024 * 1024,
            16,
        )
        .unwrap();
        let text = "x".repeat(512 * 1024);
        input.write_paste(&text, false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while bytes.lock().unwrap().len() != text.len() {
            assert!(Instant::now() < deadline, "large paste was not written");
            thread::sleep(Duration::from_millis(5));
        }
        input.shutdown();
        worker.join().unwrap();
    }

    struct CollectingWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CollectingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn wait_for(revision: &AtomicU64, last: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let current = revision.load(Ordering::Acquire);
            if current != last {
                return current;
            }
            assert!(Instant::now() < deadline, "terminal output timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_text(session: &TerminalSession, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let contents = session.contents();
            if contents.contains(needle) {
                return contents;
            }
            assert!(
                Instant::now() < deadline,
                "missing {needle:?} in {contents:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn command(script: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new("sh");
        command.arg("-c");
        command.arg(script);
        command
    }

    #[test]
    fn parser_retains_ansi_colors_and_reports_the_exit_code() {
        let root = std::env::temp_dir();
        let session = TerminalSession::spawn_command(
            &root,
            40,
            8,
            command("printf '\\033]0;Terminal title\\007\\033[?1000h\\033[?1006h\\033[31;3;9mred\\033[0m\\n'; exit 7"),
        )
        .unwrap();
        wait_for_text(&session, "red");
        let snapshot = session.snapshot();
        assert_eq!(snapshot.size(), (40, 8));
        let cell = snapshot.cell(0, 0).expect("first output cell");
        assert_eq!(cell.contents(), "r");
        assert!(cell.italic());
        assert!(cell.strikethrough());
        assert_eq!(cell.foreground(), TerminalColor::Indexed(1));
        assert_eq!(session.title().as_deref(), Some("Terminal title"));
        assert_eq!(
            session.mouse_protocol(),
            (TerminalMouseMode::PressRelease, TerminalMouseEncoding::Sgr)
        );
        let exit = loop {
            if let Some(exit) = session.poll_exit() {
                break exit;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(exit.code, Some(7));
        assert!(exit.message().contains("7"));
    }

    #[test]
    fn interactive_input_echo_resize_and_scrollback_are_bounded() {
        let mut session =
            TerminalSession::spawn_command(&std::env::temp_dir(), 20, 6, command("cat; exit 3"))
                .unwrap();
        session.write(b"hello\r").unwrap();
        wait_for_text(&session, "hello");
        session.resize(30, 4).unwrap();
        assert_eq!(session.size(), (30, 4));
        assert_eq!(session.snapshot().size(), (30, 4));
        session.write(b"\x04").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let exit = loop {
            if let Some(exit) = session.poll_exit() {
                break exit;
            }
            assert!(Instant::now() < deadline, "cat did not exit");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(exit.code, Some(3));
        assert!(session.scrollback_len() <= SCROLLBACK_CAPACITY);
    }

    #[test]
    fn interactive_shell_echoes_typed_commands_before_their_output() {
        let mut shell = CommandBuilder::new("sh");
        shell.arg("-i");
        shell.env("TERM", "xterm-256color");
        let session = TerminalSession::spawn_command(&std::env::temp_dir(), 40, 8, shell).unwrap();
        session.write(b"printf 'command-output\\n'\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let contents = session.contents();
            if contents.contains("command-output") {
                assert!(contents.contains("printf 'command-output"), "{contents:?}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "shell output timed out: {contents:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn configured_login_shell_echoes_typed_commands() {
        let session = TerminalSession::spawn(&std::env::temp_dir(), 40, 8).unwrap();
        session.write(b"echo CODEX_TYPED_MARKER\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let contents = session.contents();
            if contents.contains("CODEX_TYPED_MARKER") {
                assert!(contents.contains("echo CODEX_TYPED_MARKER"), "{contents:?}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "shell output timed out: {contents:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn killing_a_long_running_child_ends_the_read_loop() {
        let mut session =
            TerminalSession::spawn_command(&std::env::temp_dir(), 40, 8, command("sleep 60"))
                .unwrap();
        let root = session.root().to_path_buf();
        assert_eq!(root, std::env::temp_dir());
        session.kill();
        assert!(session.is_exited());
        assert!(session.reader.is_none());
    }

    #[test]
    fn signal_death_is_reported_without_a_fabricated_exit_code() {
        let session =
            TerminalSession::spawn_command(&std::env::temp_dir(), 40, 8, command("kill -TERM $$"))
                .unwrap();
        let exit = wait_for_exit(&session);
        assert_eq!(
            exit.code, None,
            "signal death must not report portable-pty's placeholder code"
        );
        assert!(
            exit.detail.is_some(),
            "signal death must name the signal, got {exit:?}"
        );
        assert!(exit.message().contains("terminated"), "{}", exit.message());
    }

    /// A descendant that ignores `SIGHUP` must not survive the session: the
    /// group is signalled again with `SIGKILL` after the grace period.
    #[test]
    fn killing_the_session_terminates_descendants_that_ignore_sighup() {
        let mut session = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            60,
            8,
            command("trap '' HUP; sh -c 'trap \"\" HUP; sleep 60' & echo child:$!; wait"),
        )
        .unwrap();
        let text = wait_for_text(&session, "child:");
        let child_pid: i32 = text
            .split("child:")
            .nth(1)
            .and_then(|rest| {
                rest.split(|character: char| !character.is_ascii_digit())
                    .next()
            })
            .and_then(|digits| digits.parse().ok())
            .expect("the fixture prints its descendant pid");
        session.kill();
        let deadline = Instant::now() + Duration::from_secs(3);
        while nix::sys::signal::kill(Pid::from_raw(child_pid), None).is_ok() {
            assert!(
                Instant::now() < deadline,
                "descendant {child_pid} survived the process-group kill"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// `kill` must never join the reader on the caller: a descendant that
    /// keeps the pty open would otherwise freeze the UI thread.
    #[test]
    fn kill_returns_before_the_reader_can_drain() {
        let mut session = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            40,
            8,
            command("sh -c 'sleep 60' & echo ready; wait"),
        )
        .unwrap();
        wait_for_text(&session, "ready");
        let started = Instant::now();
        session.kill();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "kill blocked for {elapsed:?}"
        );
        assert!(session.reader.is_none());
    }

    fn wait_for_exit(session: &TerminalSession) -> ExitInfo {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(exit) = session.poll_exit() {
                return exit;
            }
            assert!(Instant::now() < deadline, "terminal did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn revision_advances_when_output_arrives() {
        let session = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            40,
            8,
            command("printf 'batch'; sleep 0.1"),
        )
        .unwrap();
        let start = session.revision();
        let next = wait_for(&session.revision, start);
        assert!(next > start);
        wait_for_text(&session, "batch");
    }
}
