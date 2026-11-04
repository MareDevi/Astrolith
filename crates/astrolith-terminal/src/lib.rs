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
};

pub use vt100;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Default visible rows when the view has not measured itself yet.
pub const DEFAULT_ROWS: u16 = 24;
/// Default visible columns when the view has not measured itself yet.
pub const DEFAULT_COLS: u16 = 80;
/// Bounded scrollback retained per terminal.
pub const SCROLLBACK_CAPACITY: usize = 2_000;
/// Maximum bytes parsed per reader batch.
const READ_BUFFER: usize = 16 * 1024;

/// One terminal session bound to a project root.
pub struct TerminalSession {
    root: PathBuf,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
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

impl ExitInfo {
    fn from_status(status: portable_pty::ExitStatus) -> Self {
        Self {
            code: Some(status.exit_code()),
            detail: status.signal().map(str::to_owned),
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
            master: pair.master,
            writer,
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

    /// Send bytes to the child.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize both the pseudo-terminal and the parsed screen.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let rows = rows.max(2);
        let cols = cols.max(2);
        if (cols, rows) == (self.cols, self.rows) {
            return Ok(());
        }
        self.master
            .resize(size(cols, rows))
            .map_err(io::Error::other)?;
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

    /// Terminate the child and stop the reader.
    pub fn kill(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.exited.store(true, Ordering::Release);
        self.revision.fetch_add(1, Ordering::Release);
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

#[cfg(test)]
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
            command("printf '\\033[31mred\\033[0m\\n'; exit 7"),
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
