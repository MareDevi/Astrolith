//! Project-bound interactive terminal service.
//!
//! The service owns one PTY child, a writable master and a bounded ANSI screen.
//! It never depends on GPUI: the reader thread parses output under a small lock
//! and bumps a revision, while the view polls that revision and renders cells.

use std::{
    io::{self, Read, Write},
    num::NonZeroU16,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(unix)]
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
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
/// Grace between `SIGHUP` and `SIGKILL` for the child's process group.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(150);
/// Upper bound for reaping the child after the group was signalled.
const REAP_TIMEOUT: Duration = Duration::from_millis(800);
/// Upper bound for the reader to drain after the process tree is gone.
const READER_TIMEOUT: Duration = Duration::from_millis(400);

/// One terminal session bound to a project root.
pub struct TerminalSession {
    root: PathBuf,
    /// Master side of the PTY; taken (and closed) before shutdown signals the
    /// child so no new input can reach a dying shell.
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    screen: Arc<Mutex<vt100::Parser>>,
    revision: Arc<AtomicU64>,
    exited: Arc<AtomicBool>,
    exit: Arc<Mutex<Option<ExitInfo>>>,
    rows: u16,
    cols: u16,
    reader: Option<JoinHandle<()>>,
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
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            nonzero(rows),
            nonzero(cols),
            SCROLLBACK_CAPACITY,
        )));
        let revision = Arc::new(AtomicU64::new(0));
        let exited = Arc::new(AtomicBool::new(false));
        let exit = Arc::new(Mutex::new(None));
        let reader_screen = screen.clone();
        let reader_revision = revision.clone();
        let reader_exited = exited.clone();
        let reader = thread::Builder::new()
            .name("astrolith-terminal-reader".into())
            .spawn(move || {
                let mut buffer = [0u8; READ_BUFFER];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            if let Ok(mut screen) = reader_screen.lock() {
                                screen.process(&buffer[..read]);
                            }
                            reader_revision.fetch_add(1, Ordering::Release);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                reader_exited.store(true, Ordering::Release);
                reader_revision.fetch_add(1, Ordering::Release);
            })
            .map_err(io::Error::other)?;
        Ok(Self {
            root: root.to_path_buf(),
            master: Some(pair.master),
            writer: Some(writer),
            child: Arc::new(Mutex::new(child)),
            screen,
            revision,
            exited,
            exit,
            rows,
            cols,
            reader: Some(reader),
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

    /// Read the terminal screen under a short lock.
    pub fn with_screen<T>(&self, read: impl FnOnce(&vt100::Screen) -> T) -> T {
        match self.screen.lock() {
            Ok(screen) => read(screen.screen()),
            Err(poisoned) => read(poisoned.into_inner().screen()),
        }
    }

    /// Copy the visible grid into backend-neutral renderer data.
    pub fn snapshot(&self) -> TerminalSnapshot {
        self.with_screen(|screen| {
            let (rows, cols) = screen.size();
            let mut cells = Vec::with_capacity(usize::from(rows.get()) * usize::from(cols.get()));
            for row in 0..rows.get() {
                for col in 0..cols.get() {
                    cells.push(screen.cell(row, col).map(|cell| TerminalCell {
                        contents: cell.contents().to_owned(),
                        foreground: terminal_color(cell.fgcolor()),
                        background: terminal_color(cell.bgcolor()),
                        bold: cell.bold(),
                        dim: cell.dim(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                        inverse: cell.inverse(),
                        wide: cell.is_wide(),
                        wide_continuation: cell.is_wide_continuation(),
                    }));
                }
            }
            TerminalSnapshot {
                cells,
                cols: cols.get(),
                rows: rows.get(),
                cursor: screen.cursor_position(),
                cursor_hidden: screen.hide_cursor(),
                live: screen.scrollback() == 0,
            }
        })
    }

    /// Current screen text, excluding terminal styling.
    pub fn contents(&self) -> String {
        self.with_screen(vt100::Screen::contents)
    }

    /// Text between two visible grid points, inclusive.
    pub fn contents_between(&self, start: (u16, u16), end: (u16, u16)) -> String {
        self.with_screen(|screen| screen.contents_between(start.0, start.1, end.0, end.1))
    }

    /// Whether application cursor key sequences are enabled.
    pub fn application_cursor(&self) -> bool {
        self.with_screen(vt100::Screen::application_cursor)
    }

    /// Whether bracketed paste is enabled.
    pub fn bracketed_paste(&self) -> bool {
        self.with_screen(vt100::Screen::bracketed_paste)
    }

    /// Current mouse reporting mode and wire encoding.
    pub fn mouse_protocol(&self) -> (TerminalMouseMode, TerminalMouseEncoding) {
        self.with_screen(|screen| {
            let mode = match screen.mouse_protocol_mode() {
                vt100::MouseProtocolMode::None => TerminalMouseMode::None,
                vt100::MouseProtocolMode::Press => TerminalMouseMode::Press,
                vt100::MouseProtocolMode::PressRelease => TerminalMouseMode::PressRelease,
                vt100::MouseProtocolMode::ButtonMotion => TerminalMouseMode::ButtonMotion,
                vt100::MouseProtocolMode::AnyMotion => TerminalMouseMode::AnyMotion,
            };
            let encoding = match screen.mouse_protocol_encoding() {
                vt100::MouseProtocolEncoding::Default => TerminalMouseEncoding::Default,
                vt100::MouseProtocolEncoding::Utf8 => TerminalMouseEncoding::Utf8,
                vt100::MouseProtocolEncoding::Sgr => TerminalMouseEncoding::Sgr,
            };
            (mode, encoding)
        })
    }

    /// Send bytes to the child.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal session is shut down",
            ));
        };
        writer.write_all(bytes)?;
        writer.flush()
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
        if let Ok(mut screen) = self.screen.lock() {
            screen.set_size(nonzero(rows), nonzero(cols));
        }
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
        let current = screen.screen().scrollback();
        let maximum = screen.screen().scrollback_len();
        let target = (current as i64 + i64::from(delta)).clamp(0, maximum as i64) as usize;
        if target == current {
            return false;
        }
        screen.screen_mut().set_scrollback(target);
        self.revision.fetch_add(1, Ordering::Release);
        true
    }

    /// Clear the visible screen and scrollback without touching the child.
    pub fn clear(&self) {
        if let Ok(mut screen) = self.screen.lock() {
            screen.process(b"\x1b[2J\x1b[H\x1b[3J");
            screen.screen_mut().set_scrollback(0);
        }
        self.revision.fetch_add(1, Ordering::Release);
    }

    /// Jump to the live output.
    pub fn scroll_to_bottom(&self) {
        if let Ok(mut screen) = self.screen.lock() {
            screen.screen_mut().set_scrollback(0);
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
        self.writer = None;
        self.master = None;
        let child = self.child.clone();
        let fallback_child = child.clone();
        let exit_slot = self.exit.clone();
        let revision = self.revision.clone();
        let reader = self.reader.take();

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
                if let Some(reader) = reader {
                    let deadline = Instant::now() + READER_TIMEOUT;
                    while !reader.is_finished() && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(5));
                    }
                    if reader.is_finished() {
                        let _ = reader.join();
                    }
                    // Otherwise the reader is detached instead of pinning
                    // shutdown on a descendant that still holds the pty.
                }
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

fn terminal_color(color: vt100::Color) -> TerminalColor {
    match color {
        vt100::Color::Default => TerminalColor::Default,
        vt100::Color::Rgb(red, green, blue) => TerminalColor::Rgb(red, green, blue),
        vt100::Color::Idx(index) => TerminalColor::Indexed(index),
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.kill();
    }
}

fn nonzero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.max(2)).expect("clamped terminal dimension")
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
    use std::time::{Duration, Instant};

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
            let contents = session.with_screen(|screen| screen.contents());
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
            command("printf '\\033[?1000h\\033[?1006h\\033[31;3mred\\033[0m\\n'; exit 7"),
        )
        .unwrap();
        wait_for_text(&session, "red");
        let red = session.with_screen(|screen| {
            let (row, _) = screen.cursor_position();
            let cell = screen.cell(row.saturating_sub(1), 0).unwrap();
            (cell.contents().to_owned(), format!("{:?}", cell.fgcolor()))
        });
        assert_eq!(red.0, "r");
        assert!(
            red.1.contains("Idx(1)") || red.1.contains("Rgb(205"),
            "ANSI red must survive parsing, got {}",
            red.1
        );
        let snapshot = session.snapshot();
        assert_eq!(snapshot.size(), (40, 8));
        let cell = snapshot.cell(0, 0).expect("first output cell");
        assert_eq!(cell.contents(), "r");
        assert!(cell.italic());
        assert_eq!(cell.foreground(), TerminalColor::Indexed(1));
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
        assert_eq!(
            session.with_screen(|screen| {
                let (rows, cols) = screen.size();
                (cols.get(), rows.get())
            }),
            (30, 4)
        );
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
        assert!(session.with_screen(|screen| { screen.scrollback_len() <= SCROLLBACK_CAPACITY }));
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
