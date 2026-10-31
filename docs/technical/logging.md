# Logging

Astrolith logs through [`tracing`]. Library crates emit events; only the desktop
binary installs a subscriber. The bootstrap lives in `crates/astrolith-observability`.

## Defaults

| Sink | Format | Destination | Blocking |
| --- | --- | --- | --- |
| stderr | human-readable, ANSI when attached to a terminal | terminal or journal | no |
| file | one JSON object per line | platform log directory, daily rotation | no (dedicated writer thread) |

The default filter keeps Astrolith crates at `info` and third-party crates at
`warn`. File logging retains seven daily files and prunes older ones.

Platform log directories:

- Linux: `$XDG_STATE_HOME/astrolith/logs`, otherwise `~/.local/state/astrolith/logs`
- macOS: `~/Library/Logs/astrolith/logs`
- Windows: `%LOCALAPPDATA%\astrolith\logs`, otherwise `%APPDATA%\astrolith\logs`

Files are named `astrolith.YYYY-MM-DD.log`. Timestamps are UTC.

The writer is non-blocking: the GPUI thread only formats and enqueues records.
The process-wide guard is dropped after the event loop returns, which flushes
buffered records. A panic hook records the panic message and source location
before the previous hook runs.

## Configuration

All settings come from environment variables. Invalid values are ignored and
reported on stderr at startup; the process still starts with safe defaults. If
the log directory cannot be created or written, file logging is skipped and
stderr logging plus the panic hook still start.

| Variable | Values | Default |
| --- | --- | --- |
| `ASTROLITH_LOG` | `EnvFilter` directives | see below |
| `RUST_LOG` | `EnvFilter` directives, used when `ASTROLITH_LOG` is unset | see below |
| `ASTROLITH_LOG_FILE` | `off`, `0`, or `false` disables the file sink | enabled |
| `ASTROLITH_LOG_DIR` | absolute log directory | platform directory |
| `ASTROLITH_LOG_FORMAT` | `text` or `json` for stderr | `text` |
| `ASTROLITH_LOG_FILE_FORMAT` | `json` or `text` for the file | `json` |
| `ASTROLITH_LOG_MAX_FILES` | positive integer daily files retained | `7` |

Default filter:

```text
warn,astrolith_desktop=info,astrolith_gpui=info,astrolith_core=info,astrolith_astro=info,astrolith_document=info,astrolith_workspace=info,astrolith_preview=info,astrolith_observability=info
```

Examples:

```bash
# Verbose UI and preview debugging, without third-party noise.
ASTROLITH_LOG="warn,astrolith_gpui=debug,astrolith_preview=trace" mise run dev

# Keep logs next to a crash report, in JSON on stderr too.
ASTROLITH_LOG_DIR=/tmp/astrolith-logs ASTROLITH_LOG_FORMAT=json mise run dev

# Terminal-only logging (useful in CI or when the state directory is read-only).
ASTROLITH_LOG_FILE=off mise run dev
```

## Event levels

The application follows a fixed level discipline:

- `trace`: per-record and per-stream detail, such as recovery writes and drained
  watcher batches.
- `debug`: lifecycle transitions and counts: project inspection, indexing,
  watcher registration, document loads, save snapshots, preview process
  ownership, preference reads and writes.
- `info`: user-visible milestones: application start/exit, main window, project
  opened or recovered, document loaded/saved, schema generation finished,
  preview ready/stopped, browser opened.
- `warn`: recoverable anomalies: unsupported Astro projects, reindex failures,
  save conflicts, recovery records skipped, settings fallback, failed file opens.
- `error`: failed operations: save failures, preview start/stop failures,
  service-runtime startup failure, panic.

Instrumented subsystems:

| Target | Representative events |
| --- | --- |
| `astrolith_desktop` | startup metadata, selected spike, fatal window failures, exit |
| `astrolith_gpui` | runtime startup/shutdown, preferences, project open/index/watch, document load/save/recovery, preview panel, frontmatter edits |
| `astrolith_preview` | dev-server state transitions, port/startup failures, child spawn/stop/termination, output-drain errors |
| `astrolith_document` | save outcome and conflicts, atomic write failures, recovery reads/writes/clears |
| `astrolith_workspace` | bounded collection scans, watcher registration and errors |
| `astrolith_astro` | project inspection, unsupported diagnostics, `astro sync` schema generation |

## Privacy

Logs include project roots, document paths, collection identifiers, revisions,
PIDs, durations and bounded preview stderr excerpts. They never include document
content, preference bodies, environment variable values or full child-process
environments. Preview excerpts are capped at 2,000 characters.

## Verification

The bootstrap owns a real file-emission test:

```bash
cargo test -p astrolith-observability
```

It installs the subscriber once, writes a structured record, drops the guard and
asserts the JSON file contains the event fields. `mise run check` runs the same
test with the rest of the workspace suite.
