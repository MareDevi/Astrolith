# E2E verification plan

Audit date: 2026-11-05, baseline `dev` @ `326eeec`.

This plan defines **how** work from `optimization-roadmap.md` is proven. It follows the
testing rules in the assignment and in `AGENTS.md`:

1. No unit test written after the implementation to justify it.
2. E2E is the primary mechanism for behaviour.
3. When a subsystem must be tested in isolation, failure modes, expected behaviour,
   acceptance criteria and procedure are written **first**, then implemented, then executed.
4. Every executed E2E run produces a reproducible artifact.

## 1. What "E2E" means here

GPUI has no UI-automation framework, so "end to end" is layered. Every scenario below states
its layer; a scenario is only **Passed** at the highest layer it claims.

| Layer | Mechanism | Proves | Does not prove |
|---|---|---|---|
| **L1 — in-process** | The existing `#[gpui_kit::test]` harness (`TestAppContext`, `simulate_window_resize`, `render_frame`, fake language server) | Real entities, real buffers, real tasks, real FS in a temp dir | Window manager, GPU, IME, real servers, real PTY |
| **L2 — real process** | Launch `target/debug/astrolith-desktop` with an isolated `XDG_CONFIG_HOME`/`HOME`, a fixture project, and a **debug control socket**; drive commands, read state, take a screenshot via the compositor | Real startup, real window, real child processes, real servers | Human input latency; GPU correctness beyond a screenshot |
| **L3 — native manual** | A human performs the documented steps in a real session and records screen capture | IME, focus, scaling, drag and drop, menus, perceived latency | Reproducibility (mitigated by a written script + recording) |

**L2 requires one small addition**, proposed in Phase 0: extend the existing single-instance
socket (`single_instance.rs`, already a Unix socket with a JSON line protocol) with
`cfg(debug_assertions)`-only verbs: `OpenPath`, `ExecuteCommand <id>`, `DumpState`, `Screenshot`.
This is the mechanism Astro Editor uses (a token-guarded local bridge) and the only way to
script a GPUI window without a UI-automation stack. It is debug-only and off in release.

Anything a scenario claims as L2 must additionally have an L1 failure-mode test for the
deterministic parts (state transitions), because L2 is timing-sensitive.

## 2. Failure-mode inventories (written before implementation)

Each item is a case the implementation must handle; the acceptance column is the observable
result a test asserts.

### 2.1 Document save / conflict

| # | Failure mode | Expected behaviour | Acceptance |
|---|---|---|---|
| EXTERNAL-01 | File changed on disk while clean and open | Mark externally changed; do not overwrite; offer reload | Tab marker + status; reload yields disk bytes |
| SAVE-02 | File changed on disk while dirty | Conflict on save; buffer kept; recovery record kept | Save returns `Conflict`; buffer text unchanged; recovery file exists |
| SAVE-03 | File deleted on disk | Save conflicts; file is not recreated silently | `SaveOutcome::Conflict`; no new file |
| SAVE-04 | Save fails (permission, ENOSPC) | Status shows failure; recovery record written; no partial file | Original bytes intact; one artifact |
| SAVE-05 | Own save while watcher is running | No "changed on disk" | `externally_changed == false` after the batch |
| SAVE-06 | Save completes after newer edits | Newer edits remain dirty; follow-up save has no conflict | `rebase_persisted` path; second save succeeds |
| SAVE-07 | Rename requested while a save is in flight | Rename waits for the save; both complete | File at the new path with the saved content |
| SAVE-08 | Trash requested while another document saves | Both actions complete (no dropped continuation) | Two files' state correct |
| SAVE-09 | Document grows past 8 MiB | Typed save refusal, not a save that cannot be reopened | Error message; file unchanged |
| SAVE-10 | Quit with dirty buffers | Every dirty buffer saved or the quit is cancelled | On-disk content matches the buffer |

### 2.2 Recovery

| # | Failure mode | Expected | Acceptance |
|---|---|---|---|
| RECOVERY-01 | Crash with unsaved edits | Recovery offered at startup; disk not modified | Record loaded; buffer text equals record |
| RECOVERY-02 | Crash + disk edited afterwards | Conflict, not silent overwrite | First autosave does not clobber newer disk bytes |
| RECOVERY-03 | Corrupt recovery record | Reported, ignored, never deleted | Startup succeeds; file still present |
| RECOVERY-04 | Recovery applied, then discarded | Record cleared only after an explicit decision | File removed on discard; retained on keep |

### 2.3 Project discovery / index / watcher

| # | Failure mode | Expected | Acceptance |
|---|---|---|---|
| W1 | Collection base missing | That collection reports a diagnostic; project still opens | Navigator shows other collections |
| W2 | Symlink inside a collection | Skipped with a diagnostic; project opens | No hard failure |
| W3 | Duplicate slug | Diagnostic naming both files; other entries indexed | Navigator non-empty |
| W4 | TOML `+++` frontmatter | Per-entry diagnostic; entry listed as unsupported | Other entries fine |
| W5 | inotify limit exhausted | Diagnostic + degraded watching, rescan on demand | Project opens |
| W6 | `git checkout` burst (1000 files) | One batch, one index publish | Instrumented batch count == 1 |
| W7 | External create/delete/rename | Navigator converges within 1 s | List matches disk |
| W8 | Non-UTF-8 filename | Skipped with diagnostic | Project opens |

### 2.4 LSP

| # | Failure mode | Expected | Acceptance |
|---|---|---|---|
| L1 | No server installed | One diagnostic, no repeated spawn attempts while Build stays open | Log/state shows `unavailable` |
| L2 | Server crashes mid-session | User-visible state; one restart attempt; no zombie | `ps` has no child; state shows failure |
| L3 | Server never answers a request | Timeout; provider reports; `pending` shrinks | Timeout within the budget |
| L4 | Diagnostics for another file | Not rendered in this editor | Editor diagnostic set empty |
| L5 | Stale `publishDiagnostics` (older version) | Dropped | Newer set retained |
| L6 | Two files of the same language | One server process | `ps` count == 1 |
| L7 | Tab switch | No process churn | PID unchanged |
| L8 | Rapid typing | Requests coalesced/cancelled; no queue growth | `pending.len()` bounded |
| L9 | Quit with server running | Child exits within 2 s | No orphan |

### 2.5 Terminal

| # | Failure mode | Expected | Acceptance |
|---|---|---|---|
| T1 | `yes` for 60 s | UI responsive; memory bounded | frame p95 ≤ 16.7 ms; RSS stable |
| T2 | `Ctrl+C` in a foreground job | SIGINT delivered; prompt returns | Shell prompt visible |
| T3 | Resize while running `htop` | Grid and PTY agree | `stty size` matches the drawn grid |
| T4 | Project switch with a busy shell | No freeze; old tree gone | New prompt; `ps` clean |
| T5 | Quit with a running `npm run dev` | Whole process group exits | `ps` shows nothing |
| T6 | Shell exits | State shows exited; restart works | Exit code shown |
| T7 | Arrow keys in `vim`/`less` | Application cursor mode honoured | Navigation works |
| T8 | Multi-line paste | Not executed line-by-line before the user presses Enter | One paste event |

### 2.6 Preview

| # | Failure mode | Expected | Acceptance |
|---|---|---|---|
| P1 | Port in use | Recoverable failure with the stderr excerpt | Status shows the port and a retry |
| P2 | `astro dev` exits on startup | Failure state, not an infinite spinner | Status `Failed` |
| P3 | Edit → save | HMR reflects in the page | Screenshot differs within 2 s |
| P4 | Quit | Server group killed | No orphan node/astro |
| P5 | Preview hidden | Configured policy (stop or retain) is honoured | State matches the documented policy |

### 2.7 Configuration

See `config-cleanup.md` §6 (`CFG-E2E-01…10`) — that suite is normative and is not duplicated here.

### 2.8 Application lifecycle

| # | Failure mode | Expected | Acceptance |
|---|---|---|---|
| A1 | Second launch | Forwards to the running instance and exits 0 | One window; process exits |
| A2 | Quit with a 1 MiB document + servers | No hang; no orphan | Exit within 3 s; `ps` clean |
| A3 | 50 mode cycles | No session/watcher/PTY/server recreation | PID sets identical before/after |
| A4 | 8 h idle with a project | CPU ≈ 0 | `perf stat` sample; wakeups/s |

## 3. Artifact contract

### 3.1 Layout

```
artifacts/e2e/<test-id>/            # test-id = <area>-<n>, e.g. save-04
  manifest.json                     # required, committed (small)
  command.txt                       # exact command line(s), committed
  stdout.log  stderr.log            # committed (text)
  state-before.json state-after.json# L2 debug-socket dumps (committed if < 64 KiB)
  screenshot-before.png screenshot-after.png   # not committed if > 200 KiB
  perf.json                         # metrics for perf scenarios
  notes.md                          # manual steps for L3 (committed)
```

`.gitignore`: `artifacts/e2e/*/screenshot-*.png`, `artifacts/e2e/*/core.*`,
`artifacts/e2e/*/trace-*`; everything else stays tracked so the *evidence*, not the machine
state, is versioned.

### 3.2 `manifest.json` schema (required fields)

```json
{
  "schema": 1,
  "test_id": "save-04",
  "title": "Dirty document saved while the watcher is running is not marked changed on disk",
  "purpose": "Prove DOC-01 is fixed",
  "status": "passed",
  "layer": "L2",
  "started_at": "2026-11-05T12:00:00Z",
  "duration_s": 12.4,
  "source": {
    "sha": "326eeec5bcc69d90ea6af6fd50ab8f214b97e4b8",
    "branch": "dev",
    "dirty": false,
    "diff_sha256": null
  },
  "environment": {
    "os": "CachyOS Linux",
    "kernel": "7.3.1-1-cachyos",
    "arch": "x86_64",
    "session": "wayland",
    "compositor": "unknown",
    "display": "1000x700@1.0",
    "cpu": "Intel Core Ultra 9 185H",
    "ram_gib": 30,
    "gpu": "n/a (headless)",
    "rustc": "1.99.1",
    "gpui_kit": "0.8.1",
    "app_profile": "release",
    "features": []
  },
  "fixture": {
    "path": "fixtures/astro7",
    "version": "git:326eeec:fixtures/astro7",
    "setup": "scripts/e2e/fixtures/astro7-install.mjs",
    "config_home": "artifacts/e2e/save-04/config-home"
  },
  "command": "scripts/e2e/cases/save-04.mjs",
  "repro": "mise run e2e:case -- save-04",
  "expected": ["tab marker cleared after save", "no external-change status"],
  "observed": ["tab marker cleared at t+0.4s", "status 'Saved.'"],
  "evidence": ["state-after.json", "stdout.log"],
  "metrics": { "save_ms": 3.1, "watcher_batch_s": 0.22 },
  "limitations": ["compositor not recorded"],
  "rerun_notes": "Requires a writable temp dir; leaves no project changes."
}
```

Unexecuted scenarios are committed with `"status": "not_run"`. A scenario without a manifest is
**not** evidence.

### 3.3 Rules

- `status` ∈ `passed | failed | blocked | not_run`; never inferred.
- A failed or blocked run keeps its logs and a one-line cause in `observed`.
- Performance scenarios must include the same metric keys as the Phase 0 baseline so the
  comparison is mechanical.

## 4. Runner

```
scripts/e2e/
  run.mjs          # orchestrator: start fixture, isolated HOME/XDG_CONFIG_HOME, launch app,
                   #   drive the debug socket, collect artifacts, write manifest
  cases/<id>.mjs   # one file per scenario, exports { id, layer, fixture, steps, assert, metrics }
  fixtures/        # setup/teardown for the Astro fixtures, language server, PTY shell
  lib/socket.mjs   # newline-JSON client for the single-instance/debug socket
  lib/perf.mjs     # reads perf.json, compares against artifacts/e2e/baseline/perf.json
```

mise tasks (added in Phase 0):

```toml
[tasks."e2e"]
run = "node scripts/e2e/run.mjs --all"
[tasks."e2e:case"]
run = "node scripts/e2e/run.mjs"
[tasks."e2e:perf"]
run = "node scripts/e2e/run.mjs --suite perf"
```

Isolation requirements, non-negotiable:

- `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` point inside the case directory.
- `WAYLAND_DISPLAY`/`DISPLAY` inherited for L2/L3; unset for L1.
- A fresh copy of the fixture per case (`cp -a` of a prepared `node_modules` cache), never a
  shared working tree.
- Every case ends by killing the app and asserting no descendant processes remain
  (`/proc/*/stat` scan filtered by the case's session id / process group).

## 5. Scenario catalogue

Baseline-run scenarios are ordered by the phase that needs them. `L1` scenarios already exist
in the repository's test suite and are listed so that Phase 0 does not duplicate them.

| ID | Layer | Phase | What it proves | Key metrics |
|---|---|---|---|---|
| `E2E-00` | L2 | 0 | The harness itself: launch, dump state, screenshot, quit, no orphans | startup → first frame, quit s |
| `CFG-E2E-01…10` | L1+L2 | 1 | Config upgrade/quarantine/reset | file listings, JSON diffs |
| `SAVE-01` | L1 | 2a | Save round trip byte-exact (CRLF, Unicode, frontmatter-only edit) | bytes equal |
| `SAVE-05` | L2 | 2a | No false external-change after own save (DOC-01) | `externally_changed` |
| `SAVE-02` | L2 | 2a | Conflict resolution path exists and preserves both sides (DOC-02) | files present |
| `RECOVERY-01` | L2 | 2a | kill -9 with unsaved edits → recovery, disk untouched (DOC-03) | buffer vs disk |
| `RECOVERY-02` | L2 | 2a | Crash + newer disk edit → conflict, no overwrite | disk sha256 |
| `EXTERNAL-01` | L2 | 2a | External edit while clean → reload affordance | state dump |
| `PERF-E2E-01` | L1 | 2b | Preview-mode frame cost is not O(document) | `render_frame` ms at 1 KB / 1 MB |
| `PERF-E2E-02` | L2 | 2b | Idle CPU with a project open | wakeups/s, CPU % |
| `PERF-E2E-03` | L3 | 3 | Typing latency at 1 MiB | input→frame p95 |
| `PERF-E2E-04` | L3 | 3 | Window resize with a 1.3 MB entry | frame avg/p95 |
| `B-E2E-01` | L2 | 4 | Two `.ts` tabs → one language server | `ps` count |
| `B-E2E-02` | L2 | 4 | Diagnostics land on the right file | state diff |
| `B-E2E-03` | L2 | 4 | Terminal flood responsiveness | frame p95, RSS |
| `B-E2E-04` | L2 | 4 | Project switch / quit leaves no shell tree | `ps` after |
| `B-E2E-05` | L2 | 4 | Terminal dev server visible to preview | URL/port match |
| `WS-E2E-01` | L1 | 5 | One save → one entry re-read | instrumented read count |
| `WS-E2E-02` | L2 | 5 | Odd files degrade one collection, not the project | navigator contents |
| `UI-E2E-01` | L2/L3 | 6 | Every command reachable by menu *and* palette | command coverage report |
| `PKG-E2E-01` | L3 | 6 | Install from `.deb`/AppImage, launch, associate, uninstall | package scripts |
| `PERF-E2E-05` | L2 | 7 | Before/after comparison table | all metric keys |

Manual (L3) scripts are required for: IME composition (CJK), focus after mode switch, display
scaling (100 %, 150 %, 200 %), native menus, drag and drop of files, browser launch, and
perceived typing latency. Each gets `notes.md` with every interaction and a recording path.

## 6. Metrics and gates

| Gate | Threshold | Source |
|---|---|---|
| `mise run check` | green, no ignored-test skips added | CI |
| First frame (1 MiB document) | ≤ 16.7 ms p95 | `SHELL_OPEN` |
| Typing p95 (1 MiB) | ≤ 16.7 ms | `PROJECTED_WRITER` + frame |
| Typing p95 (100 KB) | ≤ 8 ms | same |
| Idle CPU with a project, terminal hidden | ≤ 1 % of one core | `PERF-E2E-02` |
| Terminal flood frame p95 | ≤ 16.7 ms | `B-E2E-03` |
| Save p95 (1 MiB) | ≤ 50 ms | `SAVE-01` |
| Quit | ≤ 3 s, zero descendants | `E2E-00`, `B-E2E-04` |
| Project open (500 entries) | ≤ 500 ms | Phase 0 baseline |
| Mode cycle (50×) | no PID change for any service | `A3` |

Thresholds are provisional until Phase 0 records the baseline on the reference machine; after
that they are frozen for the duration of the roadmap and renegotiated only with numbers.

## 7. Relationship to the existing suite

- Keep every existing test that encodes a real invariant (buffer revisions, save outcomes,
  projection mapping, watcher batching, LSP framing/positions, terminal parser).
- **Delete or rewrite** tests that pin implementation details the roadmap changes (e.g. a test
  asserting `disk_fingerprint` is captured at construction; a test asserting the 16 ms poll
  interval; a test asserting the migration backup filenames from `config-cleanup.md` §4).
- Do not convert manual checks into unit tests. The rule for the whole roadmap: a permanent
  test exists only if it catches a plausible consumer-visible bug; everything else is an E2E
  case or a throwaway smoke run.
- New permanent tests permitted by this plan are the failure-mode assertions in §2 that are
  deterministic at L1 (conflict outcomes, recovery revisions, watcher batch counts, LSP URI
  filtering). They must be written **before** the corresponding implementation.

## 8. What this plan cannot prove

No E2E design in this repository can currently prove GPU frame time, input-device latency, IME
composition, or accessibility-tree correctness. Those remain L3 manual evidence with a written
script and a recording, and must be labelled as such in the manifest (`"layer": "L3"` plus
`limitations`).
