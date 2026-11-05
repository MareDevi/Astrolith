# Terminal (PTY) and Build Mode — evidence audit

Read-only audit of the interactive terminal service/view and its composition into
Build Mode. Repository at `dev` / `326eeec5`. No build, test, clippy or fmt run.
Every cost figure marked **arithmetic** is derived from constants in the source,
not measured; figures marked **Hypothesis** name the measurement in
`## Open questions`.

## Scope inspected

| File | Lines | Read |
|---|---:|---|
| `crates/astrolith-terminal/src/lib.rs` | 435 | full (raw) |
| `crates/astrolith-gpui/src/terminal/mod.rs` | 754 | full (raw) |
| `crates/astrolith-gpui/src/shell.rs` | 7363 | 240–300, 340–520, 1409–1456, 1540–1740, 1747–1835, 1864–1950, 1960–2070, 2170–2210, 4470–4612 |
| `crates/astrolith-gpui/src/commands.rs` | 1489 | 278–356 (availability), 355–360, 440–470, 630–650, 780–830, 960–1090 |
| `crates/astrolith-gpui/src/layout/panels.rs` | 583 | 1–90, 185–195 (`set_content`) |
| `crates/astrolith-gpui/src/editor/header.rs` | 330 | 1–130, grep for Build/source (none) |
| `crates/astrolith-gpui/src/desktop_actions.rs` | 194 | 1–80 (IDE launch only; not part of Build composition) |
| `crates/astrolith-gpui/src/editor/session.rs` | 78.5K | 100–120, 1330–1400, 1414–1525, 1612–1624 |
| `crates/astrolith-gpui/src/project_discovery.rs` | 532.4K | 1100–1125, 3070–3200, 3648–3700, 3760–3790, 4495–4530, 4848–4875, 6290–6300 |
| `crates/astrolith-gpui/src/file_browser.rs` | 15.8K | 159–240 |
| `crates/astrolith-gpui/src/preview/mod.rs` | 39.0K | 300–400, 560–740 |
| `crates/astrolith-gpui/src/preview/embedded.rs` | 9.6K | grep reload/load_url |
| `crates/astrolith-preview/src/server.rs` | — | 300–460, 625–692, 687–692 |
| `crates/astrolith-preview/src/process.rs` | — | 1–40, 215–260, 331–430 |
| `crates/astrolith-workspace/src/watcher.rs` | — | 20–30, 220–270, 499–545 |
| `crates/astrolith-workspace/src/files.rs` | — | 10–40, 64–140 |
| `portable-pty 0.9.0` `src/{lib,unix,cmdbuilder}.rs` | registry | 60–110, 200–300, 340–400, 180–300 |
| `vt100 0.18.1` `src/{parser,screen,cell}.rs` | registry | 230–300, 620–700, 90–130 |
| `gpui-pre 0.3.10` `text_system.rs` | registry | 230–280, 570–630, 1453–1480 |
| `gpui-pre-wgpu 0.3.10` `cosmic_text_system.rs` | registry | 190–210, 339–440 |
| `gpui-component 0.8.1` `theme/mono_font.rs` | registry | 1–80 |
| `gpui-base 0.8.1` `dock/dock_area.rs` | registry | 271–284, 600–700, 1060–1110 |
| `docs/refactor/{ARCHITECTURE,MILESTONES,ACCEPTANCE}.md` | — | terminal/Build lines |

Dependency versions confirmed in `Cargo.lock`: `portable-pty 0.9.0`, `vt100 0.18.1`
(`Cargo.lock:6053`, `Cargo.lock:8804`), matching `crates/astrolith-terminal/Cargo.toml:10-11`.

## Highest-risk items (ranked)

| Rank | ID | One-line risk | Fix surface |
|---|---|---|---|
| 1 | REN-3 | Terminal text is laid out by a **proportional** fallback face inside fixed 7.8 px cells: overlapping glyphs, wrong cursor, PTY grid unrelated to what is drawn | `terminal/mod.rs:425, 536` → use `cx.theme().mono_font_family`; derive `CELL_WIDTH` from `text_system().advance('m', font)` (`gpui-pre/src/text_system.rs:404`) and cache it per (family, size) |
| 2 | OUT-3, OUT-4 | The parser mutex and the renderer mutex are the same lock; no flood control. One `yes`/noisy build stalls the UI thread every frame | double-buffer: parse into a separate `Screen` snapshot and swap under a short lock; add a per-tick byte budget and an “output truncated/paused” state |
| 3 | PTY-7 | `kill()` → `reader.join()` runs on the UI thread during project switch, Restart and quit; blocks forever if a descendant holds the slave fd | `killpg(pgid, SIGHUP/SIGKILL)` like `astrolith-preview/src/process.rs:396-405`, close the master before joining, join on the service runtime |
| 4 | PTY-6, PTY-9 | No process-group signalling, no `wait()` after `SIGKILL`, no quit hook: orphaned shells/`npm` trees and `Z` entries | same as 3 plus an explicit shutdown path before entity teardown |
| 5 | INT-4 | Terminal dev server and preview dev server are disconnected; the Build loop “run here, see there” does not exist and costs two Astro processes | either attach the preview to a URL/port (parse terminal output or a configured command) or start the preview server *through* the terminal session |
| 6 | BLD-7 | Focus is never moved off the terminal when leaving Build or hiding the dock; the interceptor keeps sending keystrokes to a hidden shell | in `set_mode`/`toggle_panel`, focus the editor (or `window.blur()`) when the terminal leaves the screen; also guard `handle_key` on panel visibility |
| 7 | BLD-5 | `ToggleTerminal` from Writing/Preview can enter Build and immediately hide the dock | make the toggle idempotent: enter Build with “terminal visible”, only toggle when already in Build |
| 8 | EMU-5, EMU-7 | DECCKM ignored and no bracketed paste: arrow keys wrong in `vim`/`less`/`fzf`, multi-line paste executes immediately | read `vt100::Screen::application_cursor()` in `translate_key`; wrap paste in `ESC[200~`/`ESC[201~` when the app enabled it |

## Where the behaviour lives (route index)

| Concern | Entry point |
|---|---|
| Fork/exec, env, cwd, pty size | `astrolith-terminal/src/lib.rs:76-160` |
| Reader thread, parse, revision | `astrolith-terminal/src/lib.rs:121-143` |
| Resize / scroll / clear / exit | `astrolith-terminal/src/lib.rs:193-265` |
| Kill + drop | `astrolith-terminal/src/lib.rs:277-293` |
| View polling / resize estimate | `astrolith-gpui/src/terminal/mod.rs:231-287` |
| Key translation | `astrolith-gpui/src/terminal/mod.rs:652-741` |
| Cell grid construction | `astrolith-gpui/src/terminal/mod.rs:345-401` |
| Terminal dock composition | `astrolith-gpui/src/shell.rs:1684-1716` |
| Mode switch | `astrolith-gpui/src/shell.rs:1875-1916` |
| Preview server lifecycle | `astrolith-preview/src/server.rs:359-431`, `crates/astrolith-gpui/src/preview/mod.rs:316-368, 574-730` |

## Findings — PTY lifecycle (Q1)

| ID | Title | Evidence | Level | Impact |
|---|---|---|---|---|
| PTY-1 | Shell choice is `$SHELL` (else `/bin/sh` on unix), spawned **without** `-l`/`-i` | `astrolith-terminal/src/lib.rs:76-90` | Strong | No login profiles read (`~/.zprofile`, `~/.bash_profile`); PATH/XDG additions from login shells are missing, so `node`/`pnpm` may be absent from the PATH the shell sees even though the GUI inherited it. |
| PTY-2 | Environment fully inherited; only `TERM`/`COLORTERM` overridden | `astrolith-terminal/src/lib.rs:87-89`; base env from `portable-pty/cmdbuilder.rs:74-93` (`std::env::vars_os`) | Strong | Predictable. No `env_clear` anywhere in the crate. `TERM_PROGRAM`, `KITTY_*` etc. leak from the launching terminal emulator, so programs may emit escape sequences this parser does not implement. |
| PTY-3 | `cwd` is the project root | `astrolith-terminal/src/lib.rs:87`; `spawn_command` at `:94` | Strong | Correct; `root` is stored and exposed (`:162`). |
| PTY-4 | Child is its own session leader with a controlling tty | `portable-pty/src/unix.rs:257-273` (`setsid`, `TIOCSCTTY` gated on `get_controlling_tty`, default `true` at `cmdbuilder.rs:222`) | Strong | Kernel delivers `SIGWINCH`, `SIGHUP`, `SIGINT` to the foreground process group as a normal tty. This is what makes PTY-5 (Ctrl+C) work at all. |
| PTY-5 | Ctrl+C works only through the tty line discipline, not through `kill` | `terminal/mod.rs:695-707` (`control` + alphabetic → `0x01..0x1a`); no `libc::kill` in `astrolith-terminal` | Strong | `openpty` is called with a null `termios` (`portable-pty/src/unix.rs:36`), so `ISIG` is on and `0x03` raises `SIGINT` in the foreground group. `Ctrl+Shift+C` is claimed first for copy (`terminal/mod.rs:321-323`), so that chord can never send an interrupt — acceptable, but undocumented. |
| PTY-6 | **Kill signals only the shell pid; no process-group kill, no `wait()` after `SIGKILL`** | `astrolith-terminal/src/lib.rs:277-287` → `portable-pty/src/lib.rs:340-379` (`kill(pid, SIGHUP)`, 5×50 ms `try_wait`, then `Child::kill`) | Strong (code) / Hypothesis (orphans) | Contrast with the preview runtime, which does this correctly: `process_group(0)` at `astrolith-preview/src/process.rs:234-238`, `killpg(SIGTERM/SIGKILL)` at `:396-405`, reaping `wait()` at `:331-368`. Orphan paths: (a) a foreground job that ignores `SIGHUP` — the 250 ms grace elapses and the `SIGKILL` only reaches the shell; (b) a `setsid`/daemonizing grandchild; (c) `exec` of a supervisor whose own children are not signalled. Any of these survive project switch and quit. Zombie: if the grace loop never observes the exit, nothing reaps the pid after `SIGKILL`; `std::process::Child` has no reaping `Drop`, so the shell stays `Z` for the app's lifetime. |
| PTY-7 | `kill()` joins the reader thread, so it can block the UI thread forever | `astrolith-terminal/src/lib.rs:277-287` (`reader.join()`), `Drop` at `:289-293`; called synchronously from `attach` → `stop` (`terminal/mod.rs:130-138, 206-211`) inside the `ProjectOpened` subscription (`shell.rs:498-506`) and from `Restart` (`terminal/mod.rs:175-179`) | Strong (blocking call on the UI path) / Hypothesis (duration) | The master `read` returns only on EOF/EIO. If any surviving descendant still holds the slave fd, the reader never sees EOF and `join()` never returns → the window freezes on project switch, Restart, or window close. Even in the good case the call costs up to 250 ms of `sleep(50ms)` on the render thread (`portable-pty/src/lib.rs:359-361`). |
| PTY-8 | Shell is spawned eagerly on project open, even if Build is never visited | `shell.rs:498-506` (`ProjectOpened` → `terminal.attach`), asserted by `shell.rs:4481-4523` | Confirmed (test asserts `is_running`) | One interactive shell per opened project — including the auto-reopened last project at startup. That is a forked process tree the user never asked for; combined with PTY-7 it is also a *new* hang candidate on every project switch. |
| PTY-9 | No detach/stop on project close; no quit hook for the terminal | `grep -n "ProjectClosed\|close_project"` → no hits; `shell.rs:362-369` only guards unsaved work; quit shutdown only drains `runtime` futures (`runtime.rs:25-33`) | Strong | The only cleanup path is `TerminalSession::drop` during window teardown, i.e. the blocking `kill()` of PTY-7 runs on the main thread at exit. `SIGKILL` of the app leaves the whole tree running. |
| PTY-10 | Resize is derived from a pixel estimate, not from the font | `terminal/mod.rs:277-287` (`bounds/7.8`, `bounds/17`, clamped 20–500 × 3–200); `lib.rs:193-209` (`master.resize` + `parser.set_size`) | Strong | Every dock drag issues `SIGWINCH` at up to 62.5 Hz (one per poll tick). Clamps mean a maximised 4K dock silently truncates columns at 500 and rows at 200. |
| PTY-11 | Exit reporting | `lib.rs:38-44`, `:244-265`; `portable-pty/src/lib.rs:208-227` | Strong | `try_wait` reaps normally. For signal death the code is `status.code().unwrap_or(1)`, so `SIGINT` prints `exited with code 1 (Interrupt)` — the detail is right, the code is fabricated. `ExitInfo::message`'s `(None, …)` arms (`lib.rs:50-56`) are unreachable because `code` is always `Some`. |
| PTY-12 | Session outlives shell exit; only the poll task stops | `terminal/mod.rs:248-275` (`self.poll_task = None` on exit, `session` kept), `render` still shows a green dot because `running = self.session.is_some()` (`:428`, `:488-494`) | Strong | After the shell exits, the header claims “running” (green) while the text says `Terminal exited with code 1`; keystrokes are swallowed rather than racing a dead pty (`handle_key` returns `true` even when `write` fails, `:335-341`). |

### PTY lifecycle state diagram

```mermaid
stateDiagram-v2
    [*] --> Unattached: TerminalPanel::new (no process)
    Unattached --> Spawning: ProjectOpened → attach(root)\nshell.rs:502
    Spawning --> Running: openpty + fork/exec ok\nlib.rs:94-160
    Spawning --> Failed: openpty/exec error (failure copy)\nmod.rs:224-227
    Failed --> Spawning: Restart (button always enabled)\nmod.rs:175-179
    Running --> Hidden: dock removed / mode switch\n(session retained)
    Hidden --> Running: dock re-attached
    Running --> Running: resize (SIGWINCH) / scroll / clear
    Running --> Exited: child exits, poll_exit observes\nlib.rs:244
    Exited --> Spawning: Restart → stop() then spawn()
    Running --> Killing: project switch / Restart / Drop\nattach→stop, lib.rs:277
    Hidden --> Killing: same paths (hidden ≠ stopped)
    Exited --> Killing: Drop / project switch
    Killing --> [*]: SIGHUP + ≤250ms grace + SIGKILL\njoin(reader) ← may block forever\nlib.rs:277-293

    note right of Killing
      Reader thread blocks in read() until
      every slave fd closes. Descendants that
      survive SIGHUP keep it open → join hangs
      on the UI thread.
    end note

    note right of Hidden
      Poll loop keeps running (16 ms timer,
      try_wait each tick) and the reader keeps
      parsing. Nothing is throttled.
    end note
```

### Retained vs destroyed per transition

`PTY` = `TerminalSession` (process + master + parsed screen + reader thread).
`View` = `TerminalPanel` entity. `Poll` = 16 ms poll task.

| Transition | PTY / parsed screen | View | Poll | Editor buffer + history | LSP process | Build file tree | Astro preview server | Embedded webview |
|---|---|---|---|---|---|---|---|---|
| Open project (`ProjectOpened`) | **created** (eager, PTY-8) | retained | created | retained (session registry) | not running | created/loaded | retained (not started) | not mounted |
| Writing → Build (`set_mode`) | retained (running) | retained, mounted in bottom dock if `layout.build.preview_visible` | continues | retained; active session switches to source presentation (`session.rs:1414-1468`) | **spawned** (`session.rs:1463`, `1480-1524`) | **full rescan** (`file_browser.rs:199-215` via `project_discovery.rs:1116-1123`) | retained; `leave_preview` clears visit state only (`preview/mod.rs:325-333`) | removed (`shell.rs:2064` via `set_mode`) |
| Build → Writing | retained | retained, dock removed | continues | retained; projection rebuilt, history untouched (`absorb` no-ops when text matches, `session.rs:1341-1346`) | **stopped/killed** (`session.rs:1466`, `1612-1624`) | retained (no refresh) | retained | removed |
| Build → Preview | retained | retained | continues | retained | stopped | retained | **auto-started if embedded** (`preview/mod.rs:336-368` → `574`) | **mounted** |
| Toggle dock off (ctrl-`) | retained | retained (dock entity removed) | continues | — | — | — | — | removed |
| Tab switch inside Build | retained | retained | continues | retained | **stop + start** for the new file (`project_discovery.rs:3666-3676`) | retained | — | — |
| Project switch | **destroyed** (`attach`→`stop`, blocking kill) | retained | continues (polls new session) | registry cleared (`project_discovery.rs:3110-3113`) | destroyed with sessions | re-scanned for new root | not restarted; `ensure_attached` only (`shell.rs:1977-1993`) | reloaded by `set_url` |
| Shell exits (clean) | retained, `exited=true` | retained | **stopped** (`mod.rs:271-274`) | — | still running if Build | — | — | — |
| Window close / quit | destroyed via `Drop` → blocking kill (PTY-7) | destroyed | dropped | flushed by quit observer (`shell.rs:616-652`) | `kill_on_drop` (`astrolith-lsp/src/client.rs:256`) | — | `RunningProcess::drop` (`process.rs:427`) | destroyed |

## Findings — emulation, scrollback, selection (Q2)

| ID | Title | Evidence | Level | Impact |
|---|---|---|---|---|
| EMU-1 | Parser is `vt100 0.18.1`; only SGR/cursor/scrollback subset | `Cargo.lock:8804`, `astrolith-terminal/Cargo.toml:11`; parser use at `lib.rs:111-116, 129-134` | Strong | No sixel, no OSC 8 hyperlinks, no OSC 52 clipboard, no synchronized output (`?2026`). `vim`/`htop` render, but hyperlinks in build logs are plain text. |
| EMU-2 | Scrollback is capped at 2 000 rows | `lib.rs:27`, `vt100/src/parser.rs:33-54` | Strong | Bounded as required. Memory is `2000 × cols × sizeof(Cell)`; at the 500-col clamp that is 1 000 000 cells → tens of MB (arithmetic). Truncation is silent — no “history truncated” marker in the UI. |
| EMU-3 | Wide and combining characters are handled in the renderer | `terminal/mod.rs:360-406` (`is_wide_continuation` skip, `width_cells = 2`), `vt100/src/cell.rs:101-130` (`contents` may hold combining marks) | Strong | Correct at the model level. Rendered width is still the constant `CELL_WIDTH * width_cells` (15.6 px for wide), which is only right if the font advance is exactly 7.8 (see REN-3). |
| EMU-4 | Selection is viewport-only and is cleared by scroll and by any input | `terminal/mod.rs:60-79` (`GridPoint` has no scrollback coordinate), `:581-590` (wheel clears selection), `:335-338` (input clears) | Strong | Cannot select across a scrollback boundary or drag past the edge; selecting 2 000 lines of build output is impossible. Copy is `contents_between` over `visible_rows` (`vt100/src/screen.rs:241-298`). |
| EMU-5 | **Application cursor mode (DECCKM) is ignored** | `vt100/src/screen.rs:683-686` exposes `application_cursor()`; `translate_key` never reads it (`terminal/mod.rs:652-741`, arrows always `ESC [ A..D`, Home/End always `ESC [ H/F`) | Strong | Arrows/Home/End misbehave in any program that sets `?1h` — `vim` in normal mode, `less`, `fzf`, `htop`, `psql`. Classic terminal-integration defect, one line to fix. |
| EMU-6 | No mouse reporting to the child | `terminal/mod.rs:544-580` uses mouse events only for local selection; no `1000h/1006h` re-encoding | Strong | `htop`, `midnight commander`, `vim` mouse scroll cannot work. The wheel is always interpreted as scrollback (`:581-591`), so in the alternate screen it does nothing. |
| EMU-7 | No bracketed paste, no multiline-paste guard | `:325-331` writes clipboard text verbatim | Strong | Pasting a multi-line snippet executes each line as it arrives; `vim` gets no `ESC[200~`. |
| EMU-8 | Colour handling is a hardcoded xterm palette; bold does not brighten; dim is `opacity(0.65)` | `terminal/mod.rs:721-770`, `:391-393` | Strong | Fine functionally, but the 16 ANSI colours ignore the app theme, so contrast can fail against light themes. |
| EMU-9 | `Ctrl+V` never reaches the child | `:324-331` claims `control || platform` + `v` for paste before `translate_key` | Strong | `readline` quoted-insert (`C-v`) and `vim` visual-block are unreachable. `Ctrl+Shift+V` is the terminal convention; here both are paste. |

## Findings — output path, batching, notify (Q3)

Path: pty master → reader thread (`astrolith-terminal/src/lib.rs:121-143`) → `vt100::Parser::process` under `Mutex` → `revision.fetch_add(1)` per read → GPUI view polls `revision` every 16 ms → `cx.notify()` when it changed → `render_rows` reads the same mutex.

| ID | Title | Evidence | Level | Impact |
|---|---|---|---|---|
| OUT-1 | One parse lock per 16 KiB read; revision bumped per chunk | `lib.rs:29` (`READ_BUFFER = 16 * 1024`), `:125-141` | Strong | No byte cap per tick and no yield: the reader loop is `read → lock → process → unlock → read`. |
| OUT-2 | **`notify()` is not per chunk** — it is poll-coalesced to ≤ 1 per 16 ms | `terminal/mod.rs:231-246` (timer), `:248-275` (`if revision != last_revision { changed = true }`, single `cx.notify()` at `:269`) | Strong | ~62.5 repaints/s maximum regardless of output volume. This is the one good part of the pipeline; the batching is time-based, not size- or damage-based. |
| OUT-3 | **The same mutex is held by the parser across the whole read and by the renderer across the whole grid walk** | parse: `lib.rs:129-134`; render: `terminal/mod.rs:353-398` inside `with_screen`, executed on the UI thread | Strong | Under flood the UI thread blocks on `std::sync::Mutex` (not fair on Linux) once per frame; frames stall and input latency spikes. Directly violates `docs/refactor/MILESTONES.md:84` (“输出使用有界缓冲和批量更新，不能阻塞 GPUI”). |
| OUT-4 | No backpressure, no flood guard, no “output paused” state | no `sleep`/`yield`/budget in `lib.rs:121-143`; view never throttles the reader | Strong (absence) | `yes` keeps one core busy in the reader and burns 62.5 full-grid rebuilds/s on the UI thread. Memory stays bounded (scrollback cap) — CPU does not. |
| OUT-5 | Poll loop is a permanent 16 ms foreground timer doing a `try_wait` syscall per tick | `terminal/mod.rs:236-246`, `:248-266` | Strong | 62.5 wakeups/s + 62.5 `waitpid(WNOHANG)`/s for the lifetime of the project, even with no output and even while the terminal is hidden. Idle cost, not a correctness bug. |
| OUT-6 | Input is write+flush per keystroke, synchronous on the UI thread | `lib.rs:187-191`, `terminal/mod.rs:335-341` | Strong | A blocked pty (child not reading) can stall the UI on `write_all`. Writes are not queued or timed out. |
| OUT-7 | `clear()` injects `ESC[2J ESC[H ESC[3J` into the parser only | `lib.rs:228-235` | Strong | Clears the local screen; the child keeps its own idea of the screen. A `\r`-redrawing TUI repaints immediately, which looks like “Clear did nothing”. |

**How to measure OUT-3/OUT-4** (Hypothesis until done): run the app, execute
`yes` in the terminal, then (a) `perf top`/`perf record -g -p <pid>` to attribute
CPU between the `astrolith-terminal-reader` thread and the UI thread;
(b) `strace -c -p <pid>` for `futex` count/latency on the render thread;
(c) instrument `render_rows` with an `Instant` around the `with_screen` closure and
log the p99; (d) compare frame times while idle vs while `yes` runs. Expect the
p99 mutex wait to dominate.

## Findings — rendering (Q4)

| ID | Title | Evidence | Level | Impact |
|---|---|---|---|---|
| REN-1 | Only the visible region is laid out; scrollback is never turned into elements | `terminal/mod.rs:345-352` iterates `0..rows`/`0..cols` from `session.size()`, not `scrollback_len()` | Strong | Correct and important: 2 000 history rows cost no layout. |
| REN-2 | **The whole visible grid is rebuilt every frame**, with one `SharedString` and one `div()` per cell | `:360-406` (`SharedString::from(cell.contents())`, `" ".into()` for blanks), `:607-628` (`children(row.cells.map(div))`) | Strong | 80×24 = 1 920 cell elements + 24 row elements per frame ≈ 120 k element builds/s at 62.5 fps; at the 500×200 clamp that is 100 000 cell elements **per frame** (arithmetic). No damage tracking, no run coalescing, no `canvas` text painting. This is the dominant render cost and the reason a flood makes the whole window sluggish. |
| REN-3 | **No font measurement at all; the cell grid is a constant, and the family literally does not exist** | `CELL_WIDTH = 7.8`, `LINE_HEIGHT = 17.` at `:29-31`; `grid_size` at `:277-287`; `font_family(SharedString::from("monospace"))` at `:425, 536` — note the theme’s resolved face `cx.theme().mono_font_family` (gpui-component `theme/mod.rs:181`) is not used | Strong | gpui resolves families by exact name against installed faces (`gpui-pre-wgpu/src/cosmic_text_system.rs:386-393`: `faces().filter(|face| face.families …)`), with only `.SystemUIFont`/`.ZedMono`/`.ZedSans` remapped (`gpui-pre/src/text_system.rs:1453-1462`). `"monospace"` is a fontconfig *generic*, not a family name, so it misses and falls through to the fallback stack `.ZedMono`→Lilex, `.ZedSans`→IBM Plex Sans, Helvetica, Segoe UI, Ubuntu, Adwaita Sans, … (`text_system.rs:262-274`; `mono_font.rs:19-27` documents the same panic/fallback hazard). On this machine `fc-list` has **no** family named `monospace`, no Lilex and no IBM Plex Sans, but has `Adwaita Sans` — so terminal text is laid out in a **proportional** face inside fixed 7.8 px cells. Consequence: overlapping glyphs, cursor drawn in the wrong place, selection rectangles misaligned, PTY column count unrelated to what is drawn. Verify by rendering the terminal and comparing `text_system().advance('m')` with 7.8. |
| REN-4 | Five separate lock acquisitions per frame, then the big one | `:353-357` (`hide_cursor`, `cursor_position`, `scrollback`), `:360` (`size`), `:353-398` | Strong | Minor on its own, but each acquisition can wait on the parser (OUT-3). |
| REN-5 | Cursor is a fg/bg swap on the cell under the cursor, hidden when scrolled or unfocused | `:373-390` | Strong | With a pending wrap (cursor at `col == cols`) there is no cell, so the cursor disappears at end of line. |
| REN-6 | Bold → `SEMIBOLD`, underline → `underline()`; no italic, strikethrough, blink | `:404-405`, `:622-626` | Strong | `vt100::Cell` does carry italic; it is dropped. Cosmetic. |
| REN-7 | Status header is rebuilt from `session.exit()`/`status`/`failure` each render; Restart is never disabled | `:424-460`, `:492-520` | Strong | Restart with no project or a failed spawn silently does nothing (`spawn` returns early when `root` is `None`, `:213-215`). |

## Findings — Build Mode lifecycle (Q5)

| ID | Title | Evidence | Level | Impact |
|---|---|---|---|---|
| BLD-1 | Activation is presentation-only and well factored | `shell.rs:1875-1916` (`set_mode`): capture layout → close inspector → set mode → `set_source_view(Build)` → `apply_layout` → `sync_center` → `sync_embedded` → persist `last_mode` | Strong | No document reopen, no session recreation, no project reload. Matches `docs/refactor/ARCHITECTURE.md:91-92`. |
| BLD-2 | Editor state is preserved across the switch | `session.rs:1414-1468`: `absorb` (no-op when the editor text equals the projection, `:1341-1346`), same buffer/`history`, projection swapped, `editor.set_value(full text)` with code presentation flags | Strong | Caret/selection mapped back (`source_range_to_projection`, `:1435`); dirty state and undo history untouched. `set_value` is a full replacement, so the editor’s own layout cache and scroll position are rebuilt on every switch — a cost, not a state loss. |
| BLD-3 | **LSP is started/stopped by mode, and restarted on every Build tab switch** | start on entering Build `session.rs:1463, 1480-1524`; stop on leaving `:1466, 1612-1624`; tab switch stops the outgoing file’s server `project_discovery.rs:3666-3676` | Strong | Build↔Writing thrash costs a full `astrolith_lsp::discover` (filesystem search) + process spawn + initialize + full-text sync per switch. Switching files in Build does the same per file. Diagnostics therefore exist only while Build is visible — by design, but expensive in the common “edit, run, look” loop. |
| BLD-4 | Terminal is retained while hidden and across modes | field comment `shell.rs:247-249`; `set_preview_visible` `1684-1716`; test `4481-4600` asserts the same pid survives hide + mode change | Confirmed (test) | Meets `MILESTONES.md:86` (“隐藏 Dock 或切模式不销毁 PTY”). |
| BLD-5 | **`ToggleTerminal` from another mode enters Build and then immediately hides the terminal** when the Build layout remembered it open | `shell.rs:2188-2194` (`set_mode(Build)` then `toggle_panel(Preview)`) + `apply_layout` `1657-1683` → `set_preview_visible(layout.preview_visible)` `1684-1716` + `read_dock_layout` persists `preview_visible = dock.is_dock_open(Bottom)` `1409-1424` | Confirmed (logic) | ctrl-` from Writing/Preview therefore enters Build *and closes the dock* whenever the last Build session left it visible — from the user’s view, nothing appears. Also ctrl-` targets `PanelKind::Preview`, so `TogglePreview` in Build toggles the terminal and the menu check state for “Toggle preview” reports terminal visibility (`commands.rs:1071`). |
| BLD-6 | Tab strip is real and closes with the dirty prompt | `shell.rs:1747-1835`; `project.close_path` `project_discovery.rs:3760-3790` | Strong | Tabs reflect retained sessions and `dirty`; `close` on a dirty tab opens a save/discard dialog. |
| BLD-7 | **Focus isolation leaks in both directions** | interceptor checks only `focus.is_focused(window)` (`terminal/mod.rs:100-113`); `set_mode` restores focus only for `inspector_open`, `mode == Preview`, or `previous == Preview` (`shell.rs:1907-1913`); gpui focus is `window.focus == Some(handle)` with no tree-membership test (`gpui-pre/src/window.rs:514-516`); `remove_dock` gives no focus handoff (handoff exists only in `remove_panel_id`, `gpui-base/src/dock/dock_area.rs:626-690`, not in `remove_dock` `:271-284`) | Strong (code path) / Hypothesis (observable) | Leaving Build for Writing while the terminal had focus leaves the terminal focused; every key is then translated and written to the shell (`handle_key`) instead of reaching the editor, and `stop_propagation` suppresses app shortcuts. Same after ctrl-` hides the dock. Manual repro: click terminal, ctrl-shift-1, type. |
| BLD-8 | File tree refresh happens on entry and on every index publication | `file_browser.rs:195-215` (`refresh` flag), `project_discovery.rs:1116-1123`, called from `apply_discovered_index` `:4516` | Strong | Entering Build runs a full bounded walk (`scan_project_files`, ≤ 20 000 inspected / 5 000 nodes, `astrolith-workspace/src/files.rs:30-36`) and discards nothing if the tree is unchanged. Combined with INT-2 this is a walk per edit-save while in Build. |
| BLD-9 | Mode switch writes preferences twice per switch | `set_mode` → `capture_layout` (`:1427-1457`) and later `persist_project` (`:1898-1903`, comment at `:1903`) | Strong | Two persistence requests per mode change (debounced by the store); with PTY-8’s eager shell, cheap, but worth knowing for the write-amplification budget in `ARCHITECTURE.md:103`. |
| BLD-10 | `editor/header.rs` and `desktop_actions.rs` carry no Build-specific composition | `header.rs` grep: only shared tab model (`:66-130`); `desktop_actions.rs:1-80` is IDE launch/reveal only | Strong | Nothing to review there for Build; the header’s menu is the only Build-visible surface (`header.rs:209-215`). |

## Findings — end-to-end integration (Q6)

Traced path: open project → Build → edit → save → diagnostics → terminal dev server → preview → HMR → leave mode → close.

| ID | Title | Evidence | Level | Impact |
|---|---|---|---|---|
| INT-1 | Project open spawns the shell before any Build visit and attaches the terminal to the root | `shell.rs:498-506`; `terminal/mod.rs:130-138` | Strong | See PTY-8. |
| INT-2 | **Each saved edit costs two independent filesystem scans while in Build** | watcher poll `project_discovery.rs:3172-3190` (100 ms timer + a blocking task that only calls `watcher.drain()`), merge window `astrolith-workspace/src/watcher.rs:25` (100 ms), index publish `:4516` → `sync_file_browser_root` (`:1116-1123`) → `FileBrowser::refresh` full walk | Strong | Index rescan (content) + file-tree walk (navigation) per batch; no shared listing. On a 5 000-node project that is two full directory walks per edit. |
| INT-3 | The watcher and the scanner disagree about ignored directories | scanner list `astrolith-workspace/src/files.rs:15-28` (`dist`, `build`, `.next`, `.output`, `.astro`, …); watcher ignore `watcher.rs:540-543` (`name.starts_with('.') \|\| node_modules \|\| .git`) | Strong | `dist/`, `build/` writes from the dev server or a build task are relevant to the watcher but invisible to the tree, so each write triggers an index rescan whose result cannot change — pure churn. `.astro`/dot dirs are correctly excluded. |
| INT-4 | **Two independent Astro dev servers, no bridge between the terminal and the preview** | terminal: `astrolith-terminal/src/lib.rs:76-92` (plain `$SHELL`, no server awareness) and its only observability is `revision`/`with_screen` (`lib.rs:169-185`) consumed solely by `TerminalPanel` (`terminal/mod.rs:345-401`); preview: `astrolith-preview/src/server.rs:359-431` spawns `astro dev --host 127.0.0.1 --port <free>` (`:625-664`); no subscriber in `shell.rs`/`preview/` reads terminal output | Strong | The canonical loop “Build → `npm run dev` → look at Preview” cannot work: the preview webview loads the URL of *its own* server, on a *different* port, started by `PreviewPanel::start`. Running the manual server as well gives two watchers/compilers/HMR sockets over the same files, both writing `.astro`, doubling CPU and memory. `--ignore-lock` (`server.rs:645-654`) prevents the Astro 7 lock conflict but not the duplicate work. `docs/refactor/MILESTONES.md:86` explicitly says the terminal process is not the dev server — correct as an ownership rule, but nothing implements the handoff (no “attach preview to this URL”, no port discovery from terminal output). |
| INT-5 | Preview server is not stopped when preview leaves the screen | `preview/mod.rs:325-333` (`leave_preview` clears `visiting`/`pending_open`, comment: “drop the pending launch, not the server”); `shell.rs:1700-1716` calls it whenever the bottom dock hides or Build is entered | Strong | A server started in Writing keeps running in Build — exactly when the user is most likely to start a second one. Only explicit Stop/Quit ends it. |
| INT-6 | No HMR code in Astrolith; HMR is entirely the Astro process’s | `grep -rn "hmr\|websocket" crates/astrolith-preview/src crates/astrolith-gpui/src/preview` → no hits; embedded view only `load_url`/`reload` (`preview/embedded.rs:55-76, 148-152`) | Strong | HMR works only inside the embedded/external page served by the preview-owned server. There is no reload-on-save for the external browser, no error overlay plumbing, no awareness that the page is stale. |
| INT-7 | Preview attachment does not restart a running server (good) but does resync the mapping on every index revision | `shell.rs:1969-1998` (`preview_sync_key = (index_revision, active_path, mapping)`); `preview.ensure_attached` | Strong | Correct avoidance of recreate-on-edit; verified by the doc comment and key comparison. |
| INT-8 | Requested port can be lost, but the readiness parser follows the printed URL | `astrolith-preview/src/server.rs:687-692` (bind `:0`, read, drop) then `:625-664` passes `--port`; Astro falls back to the next free port, and `parse_ready_url` (`:719-740`) takes the **printed** URL’s port, not the requested one | Strong | Benign: the user sees the actual URL. Two wrinkles: (a) the `available_port` probe is redundant work whose result may be ignored; (b) Astro’s “Port … in use, trying another one” text is only treated as a failure if the process exits (`:459-472`), which is the right call but means the requested port is never enforced. |
| INT-8b | Port-in-use is classified, not silently retried | `server.rs:459-482`, `:743-752` | Strong | If Astro does exit on a bind failure, the error is `PortInUse(port)` with the stderr excerpt — this part is well handled and is the reference behaviour the terminal side lacks. |
| INT-9 | Quit has no explicit terminal/preview shutdown ordering | `shell.rs:362-369` (save guard only), `runtime.rs:25-33` (`shutdown_timeout(2s)`), terminal `Drop` `lib.rs:289-293` | Strong | The 2 s runtime drain does not cover the PTY: the shell is killed during GPUI entity teardown on the main thread, subject to PTY-7. A hung `join` means a hung quit; the preview’s own `Drop` (`process.rs:427`) is the only tree-safe teardown in the app. |
| INT-10 | Terminal is not part of the single-instance/URL-surface story | `single_instance.rs` (no terminal hits) | Strong (absence) | Opening a file in the running instance cannot focus the terminal; out of scope but worth stating. |

## Integration trace — open → Build → edit → diagnostics → dev server → preview → leave → close

| # | Step | Anchor | What actually happens | Duplication / cost |
|---|---|---|---|---|
| 1 | App start, shell constructed | `shell.rs:399-409` | `TerminalPanel::new` (no process, one `intercept_keystrokes` subscription), `WorkspacePanel` wrapper with empty-state copy | one app-wide keystroke interceptor |
| 2 | Project open completes | `project_discovery.rs:3130` emits `ProjectOpened`; `shell.rs:498-506` | `open_project_scope` (loads project prefs, sets `mode = resolved.last_mode`, applies layout) **and** `terminal.attach(root)` **and** `finish_pending_entry` | shell process forked before the user can see Build (PTY-8) |
| 3 | Terminal attach | `terminal/mod.rs:130-137, 213-246` | `stop()` (drop old session, blocking) → `spawn` → `openpty` + `fork/exec` + reader thread → `start_polling` (16 ms) | synchronous on the UI thread |
| 4 | Mode applied from preferences | `shell.rs:1624-1632` | `source_view = mode == Build`; `project.set_source_view`; `apply_layout`; `sync_center`; `reopen_last_entry` | — |
| 5 | Enter Build (ctrl-shift-2 / mode tap) | `shell.rs:1875-1916` | active session → source presentation + LSP start (`session.rs:1463`), file browser full rescan (`file_browser.rs:199-215`), terminal dock shown only if the Build layout says so, embedded webview removed | one fs walk + one server spawn per entry |
| 6 | Edit and save | save coordinator (not audited) → `SessionChanged` | shell `sync_preview` + `note_typing` (`shell.rs:486-493`), preview mapping key recomputed | — |
| 7 | Watcher sees the write | `project_discovery.rs:3172-3190`, merge window `watcher.rs:25` | 100 ms poll → `runtime::blocking(drain)` → rescan → `apply_discovered_index` (`:4499-4529`) → file-browser walk (Build) + entry republish + reference rebuild + `sync_file_browser_root` | two walks per save (INT-2) |
| 8 | Diagnostics | `session.rs:1480-1524`, `editor/lsp.rs` | `didChange` (debounced task) → diagnostics into the editor; **only** while Build is active and only for the active file | diagnostics vanish on leaving Build |
| 9 | User runs `npm run dev` in the terminal | bytes → `TerminalSession::write` (`lib.rs:187`) → pty | `astro dev` starts as a child of the shell, in the shell's session, with its own pgid (job control); output goes only to the parser | nothing in Astrolith observes the port/pid/readiness |
| 10 | Preview shown (dock toggle or Writing/Preview layout) | `shell.rs:1684-1716` → `preview/mod.rs:316-333` → `resume_visit` `:336-368` | `astro dev --port <free>` spawned by the preview runtime if not already ready (`server.rs:359-431`), URL parsed from stdout, webview `load_url` (`preview/embedded.rs:55-76`) | **second** dev server (INT-4) |
| 11 | HMR | none in this repo | the page served by server #10 keeps its own WebSocket; server #9's HMR is invisible | two HMR channels, one unused |
| 12 | Leave Build | `shell.rs:1875-1916` | LSP killed, projection rebuilt, terminal retained, preview server untouched | LSP restart on re-entry |
| 13 | Close / quit | `shell.rs:362-369`, entity teardown, `lib.rs:289-293` | save guard → GPUI drops the shell → `TerminalSession::kill` joins the reader on the main thread → preview `RunningProcess::drop` kills its group | quit can block on PTY-7 |

## Key translation matrix (`terminal/mod.rs:652-741`)

| Input | Sent | Correct? |
|---|---|---|
| Enter / Tab / Shift+Tab / Esc | `\r`, `\t`, `ESC[Z`, `ESC` | yes |
| Backspace / Delete / Insert | `0x7f`, `ESC[3~`, `ESC[2~` | yes (macOS `0x08` not handled) |
| Home / End | `ESC[H` / `ESC[F` | only in normal cursor mode; `ESC OH`/`ESC OF` never sent |
| Arrows | `ESC[A..D` | **ignored modifiers and DECCKM** (EMU-5): Ctrl+Left does not word-jump, Alt+Left sends nothing extra |
| PgUp / PgDn | `ESC[5~` / `ESC[6~` | yes |
| F1–F4 / F5–F12 | `ESC OP..S` / `ESC[15~..24~` | yes; F13+ unsupported |
| Ctrl+letter | `0x01..0x1a`, `ESC` prefix with Alt | yes; non-ASCII Ctrl chords return `None` |
| Ctrl+Space / Ctrl+[ | `0x00` / `ESC` | yes |
| Ctrl+V (or Cmd+V) | clipboard paste | `0x16` unreachable (EMU-9) |
| Ctrl+Shift+C | local copy | no `SIGINT` on that chord (acceptable, undocumented) |
| Alt+char, printable text | `ESC <char>`, `key_char` | yes, incl. IME-committed text |
| Mouse, wheel | local selection / scrollback | never forwarded (EMU-6) |

## Periodic work and notify budget (per window, arithmetic)

| Source | Cadence | Work per tick | Stops when? |
|---|---|---|---|
| Terminal poll task (`terminal/mod.rs:236-246`) | 16 ms → 62.5/s | `try_wait` syscall + `revision` check + `size` check; `notify` only on change | only when the child exits (`:271-274`) — not when hidden |
| Terminal repaint (`render_rows`) | ≤ 62.5/s, only after a revision change | full visible grid rebuild: `rows` row divs + `rows × cols` cell divs and `SharedString`s | — |
| Parser lock (reader thread `lib.rs:125-141`) | one per 16 KiB read | `vt100::Parser::process` under the mutex shared with the renderer | reader exits on EOF/error |
| Watcher poll (`project_discovery.rs:3172-3190`) | 100 ms → 10/s | acquire a blocking-slot permit + `spawn_blocking(drain)` even when empty | project close/switch |
| File-browser scan (`file_browser.rs:218-239`) | per index publish while Build | full `scan_project_files` walk (≤ 20 000 inspected) | superseded by a newer request |
| Preview exit watcher (`preview/mod.rs:522-572`) | per running server | server task poll | server stop |

Consequence: an idle window with a project open performs ~72.5 timer wakeups/s
plus 62.5 `try_wait` and 10 blocking-pool dispatches/s before any user input.
The terminal timer is the largest single contributor and is the only one that
keeps running while its UI is invisible.

## Cross-cutting: requirement vs implementation

| Requirement | Source | Status |
|---|---|---|
| Real PTY, interactive shell I/O, ANSI, resize, exit code, restart (A24) | `docs/refactor/ACCEPTANCE.md:40`, `MILESTONES.md:84` | **implemented** (PTY-1..12; DECCKM/mouse/paste gaps are EMU-5..7) |
| Bounded output buffer with batched updates that never block GPUI | `MILESTONES.md:84` | **partial** — bounded scrollback yes (EMU-2), notify coalescing yes (OUT-2), non-blocking no (OUT-3/OUT-4) |
| Hidden dock / mode switch must not destroy the PTY | `MILESTONES.md:86` | **implemented** (BLD-4, tested) |
| Close/switch/quit must clean up owned child processes and not leave shells | `MILESTONES.md:86`, `ARCHITECTURE.md:123` | **defective** — no group kill, no reaping after SIGKILL, no quit hook (PTY-6, PTY-7, PTY-9, INT-9) |
| Terminal focus isolated from editor/app shortcuts | `MILESTONES.md:84`, `ACCEPTANCE.md:40` | **defective** — interceptor is correct, but focus is never moved off the terminal when it leaves the screen (BLD-7) |
| Terminal must not be conflated with the dev server | `MILESTONES.md:86` | **implemented as separation, missing as integration** — no bridge, hence duplicate servers (INT-4) |

## Open questions / not inspected

1. **PTY-6/PTY-7 runtime proof.** Reproduce with a child that ignores `SIGHUP`:
   `nohup yes > /dev/null 2>&1 &` (in a subshell that keeps the slave fd), then
   switch project and measure. Also check `ps -o stat= -p <pid>` for `Z` after a
   kill that outlived the grace window. Requires building the app (not done here).
2. **OUT-3 contention numbers.** p99 mutex wait in `render_rows` under `yes`;
   frame-time distribution; reader-thread CPU share. Instrumentation described
   above.
3. **REN-3 on screen.** Confirm with a screenshot that terminal glyphs overlap and
   that the PTY column count (from `stty size` in the shell) differs from the
   visible character count per line.
4. **BLD-7 observable repro** in a native window (click terminal → ctrl-shift-1 →
   type). Whether gpui clears focus when a view stops being rendered is the only
   unknown; `is_focused` does not check tree membership, but the app may refocus
   on mode change through some other path not found by reading.
5. **IME/compose path.** `translate_key` falls back to `key_char` (`terminal/mod.rs:708-718`),
   so committed CJK text should work, but multi-codepoint IME commits and dead
   keys were not exercised.
6. **`editor.set_value` internal history.** Whether repeated mode switches grow
   `gpui-component`’s internal undo stack was not read; the session’s own history
   is untouched (BLD-2).
7. **Windows/macOS.** All PTY conclusions are Unix-specific (`setsid`,
   `TIOCSCTTY`, `SIGHUP`). `cfg!(windows)` paths in `spawn` (`lib.rs:80-85`) and
   `portable-pty`’s ConPTY were not audited.
8. **Scrollback reflow on resize.** `vt100 0.18.1::Parser::set_size` reflows rows
   into scrollback (`parser.rs:64-110`); with the 500-col clamp a dock resize can
   push lines out of history. Not measured.
