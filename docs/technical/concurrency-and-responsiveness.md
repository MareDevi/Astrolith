# Concurrency and responsiveness

Date: 2026-10-28; execution verified 2026-10-29 (Asia/Shanghai). Scope: the user authorized Tokio/concurrency planning and direct repair of confirmed responsiveness problems. This amendment does not reconstruct the missing original product specification.

## Decision

Keep GPUI responsible for native events, entities, focus and presentation. Add one application-owned Tokio runtime with two async workers for service I/O, timers and future Astro process supervision. Run ordinary synchronous filesystem work through a bounded blocking lane: at most two jobs executing, with four blocking threads as a runtime ceiling including critical exit flushing. Keep CPU work on the GPUI background executor initially; future scans and parsers must also enforce admission limits. Never use `block_on`, synchronous channel receives or contended service locks on the UI thread.

The runtime adapter owns the bridge, not domain crates. Service code must accept owned Send data and return results without GPUI types. Bridge tasks abort queued async work when their GPUI owner drops. Running synchronous operations cannot be forcibly cancelled; their concurrency permits live until the operation returns. The final preferences flush submits directly to the blocking pool, bypassing ordinary file admission, and survives its GPUI observer being dropped. GPUI allows quit observers 200 ms; runtime disposal then waits at most two seconds for blocking work. This wait occurs during teardown only. Disk operations exceeding that budget can still outlive shutdown. Document-save draining and child-process cleanup require further explicit coordination before M04/M05 acceptance.

Alternatives considered: GPUI-only execution does not provide Tokio reactor context for future Tokio services; moving all work to Tokio duplicates GPUI's existing CPU scheduler. The split above follows Zed's separation while keeping the initial footprint small. Tokio does not accelerate synchronous editor mutation.

## Zed references

Inspected revision: `33d111eac45ef581324592352d90efc91c5ec98d`.

- [GPUI/Tokio bridge](https://github.com/zed-industries/zed/blob/33d111eac45ef581324592352d90efc91c5ec98d/crates/gpui_tokio/src/gpui_tokio.rs): two-worker runtime, app-global ownership, abort-on-drop bridging and nonblocking disposal.
- [Buffer store](https://github.com/zed-industries/zed/blob/33d111eac45ef581324592352d90efc91c5ec98d/crates/project/src/buffer_store.rs): separate open-buffer identity and shared in-flight loading tasks. Adopt these contracts when M02 document sessions arrive; do not copy the entire collaborative editor.
- [Development profiles](https://github.com/zed-industries/zed/blob/33d111eac45ef581324592352d90efc91c5ec98d/Cargo.toml): optimize selected expensive dependencies during development instead of treating unoptimized editor performance as a release result.
- [Performance workflow](https://github.com/zed-industries/zed/blob/33d111eac45ef581324592352d90efc91c5ec98d/docs/src/performance.md): distinguish foreground task stalls from background time and presented-frame latency.

## Confirmed mechanisms and repair tasks

1. Development builds leave editor/layout/parser dependencies unoptimized. Same-machine headless baseline for a 1,048,898-byte / 13,972-line document: debug `set_value` 428.062 ms plus frame 422.312 ms; release 38.024 ms plus frame 42.578 ms. Long-line input/frame p95: debug 47.920 ms; release 8.022 ms. These are CPU measurements, not native latency.
2. Opening a readonly snapshot calls `set_value` on the previous editor and clones its full String on the foreground thread. Pinned Base 0.8.1 routes this through the full edit/highlight/display update before resetting history/highlighting. Initialize a fresh readonly snapshot editor through `default_value` instead, preserving source ownership and focus; measure first frame too so deferred cost remains visible. This strategy applies only to M01 readonly snapshots, not future dirty document sessions.
3. Every changed dock width schedules an immediate detached save, including `sync_all`, with no debounce. Coalesce settings changes for 250 ms and retain the task handle. Keep exit's latest-snapshot flush independent of that timer; preserve existing serialized writer and external-change protection.

Execute these tasks with failing behavior tests for settings coalescing and snapshot isolation, bounded file-to-frame diagnostics, runtime cancellation/concurrency tests, `mise run check`, native window smoke and a Conventional Commit. Record actual evidence below after execution. No timing threshold belongs in ordinary shared-runner CI.

## Required contracts before later milestones

- Before M02.2: runtime bridge and ownership exist; discovery is cancellable and cannot execute blocking operations in UI callbacks. Measure readonly open by picker response, read/validation, editor installation and first frame separately.
- M02.3: at most two scan jobs and one index publisher per project; bounded queue of 64 event batches, watcher merge window 100 ms. Overflow requests a full rescan rather than dropping changes silently. Project generation rejects results from old projects. Coalesce duplicate loads by authoritative source identity.
- M02.4/M03: each result includes project generation, document identity and revision. Latest applicable result wins; cancellation alone is insufficient. Parsing has cooperative cancellation checkpoints or bounded work units. Mutable buffers survive tab switching and must not be rebuilt to refresh a view.
- M04: one writer per document, immutable save snapshots, revision/baseline checks and bounded exit draining. Settings coalescing is not a document autosave policy. Abort must never interrupt an atomic replacement halfway through its integrity protocol.
- M05: runtime-owned child process and both output drains, capped retained logs (1 MiB per stream), startup timeout 30 s, graceful stop 3 s followed by termination and reaping. Project switch/exit must not orphan Astro descendants; implement platform-specific process ownership.

## Performance acceptance

Use identical generated multilingual workloads: 1 KiB, 100 KiB, 1 MiB and a 100 KiB single line; log actual bytes/lines. Run the real shell open path as well as the disposable editor spike, in dev and release separately. Record file read/validation, foreground installation, first headless frame and total CPU time; exclude fixture creation. Retain before/after samples with exact profile and environment.

Native target on the documented Linux Wayland reference machine: 1 MiB selected file to first presented content at most 500 ms; at least 100 visible input/scroll/resize samples with p95 at most 50 ms. Exclude human time in the picker, report portal response separately, and exercise background indexing concurrently. Repeat long-line and fractional-scale cases. These are proposed acceptance budgets; a headless pass or a mapped window does not establish them. The user's current report of native sluggishness remains open until instrumented native interaction or explicit user acceptance confirms improvement.

## Execution evidence

Measured on the existing Linux Wayland development machine, Rust 1.99.1 and pinned GPUI Kit 0.8.1. Samples below are individual bounded CPU diagnostics, not statistical native latency guarantees. Historical M00/M01 reports and original review findings remain unchanged.

The shell diagnostic creates fixtures before timing, validates the regular UTF-8 file through `read_document`, then calls the production `show_document` path and renders a complete headless frame. The command uses the entire workspace so its Markdown grammar features match the desktop binary. Read and UI stages are measured separately; executor scheduling, native picker/portal response and compositor presentation are excluded.

| Shell workload (actual bytes / lines) | Original dev read / install / frame (ms) | Repaired dev read / install / frame (ms) |
| --- | ---: | ---: |
| 1,160 / 16 | 0.018 / 5.206 / 26.990 | 0.022 / 0.043 / 8.154 |
| 102,515 / 1,414 | 0.148 / 50.326 / 68.511 | 0.141 / 0.095 / 20.097 |
| 1,048,640 / 14,464 | 1.695 / 486.268 / 479.348 | 1.544 / 0.875 / 126.936 |
| 102,410 / 1 long line | 0.091 / 53.087 / 60.113 | 0.084 / 0.123 / 17.184 |

Changing only snapshot initialization before changing the profile measured 1 MiB installation 8.197 ms and frame 551.077 ms: total 560.904 ms including read. This establishes that some work moved to the first frame, while total CPU work still fell from 967.311 ms. Selective dependency optimization and final runtime integration reduced the combined measured stages to 129.355 ms (about 7.5 times faster than the original sample). The first frame remains an appreciable foreground cost and is not claimed to fit a single display refresh. Final release measurements of the same shell fixture were read 0.910 ms, installation 0.652 ms and frame 57.455 ms (59.017 ms total).

The unchanged editor spike's 100 input/frame samples per workload provide a separate check of dependency optimization. Dev 1 MiB input/frame p95 fell from 10.605 to 3.366 ms; dev 100 KiB single-line p95 fell from 47.920 to 9.170 ms. Baseline release p95 was 1.653 and 8.022 ms respectively. These workloads differ from the shell fixture; compare each command only with its own baseline.

Settings regression: before repair, `settings_burst_does_not_write_intermediate_drag_positions` failed with “dragging must not sync every intermediate width”. After retaining a timer task it passed: no file at 200 ms, a second change resets the debounce, and the final width persists after 250 ms of quiet.

Runtime regression: the initial result-delivery test failed with “Runtime not implemented”. Six final tests cover result and I/O-error delivery, worker-panic reporting, the timer reactor, aborting a bridge before its first poll, cancelling queued filesystem work before admission, retaining running blocking jobs' permits after cancellation, and draining a critical exit flush. The shutdown regression first failed with an empty completion channel; it now passes with both ordinary admission permits held, the GPUI bridge dropped and a simulated 250 ms save. The semaphore bounds ordinary running synchronous jobs, not critical exit flushing or the number of arbitrary service futures: current callers retain one settings task and one open task per shell; future scanners must implement the separately specified queue/admission limits.

`mise run check` passed before the shutdown review correction: doctor, formatting, Clippy with warnings denied, locked build, 45 tests passed and two explicitly ignored performance diagnostics. The initial sandbox run could not start `rustc` from Node (`EPERM`); the same required command passed with sandbox escalation. Both shell performance tasks and the dev editor diagnostic passed separately. Final verification after the additional shutdown regression is recorded below.

Bounded native smoke (under one second for the successful run): Hyprland 0.56.2 / Wayland, HDMI-A-1, scale 1, native window 1263×1384, `xwayland=false`. Targeted Ctrl+B hid the sidebar and the coalesced settings write persisted it. A second Ctrl+B followed by graceful close after 50 ms preserved the final visible-sidebar state, proving the exit flush works before the 250 ms timer. Process exited 0 with empty stdout/stderr. Temporary preferences isolated this run from user settings. The first automation attempt used obsolete compositor dispatcher syntax and was terminated; the successful attempt used the installed version's documented Lua dispatchers.

Native file-to-present, input/scroll p95, fractional-scale performance and concurrent indexing acceptance remain unmeasured. The smoke and CPU improvements do not establish those results.

Final verification after review: `mise run check` passed again with 46 tests passed and two ignored diagnostics. The rebuilt desktop repeated the native smoke successfully: pending preference change survived graceful close, exit code 0, empty stdout/stderr. The final shutdown regression covers an operation longer than GPUI's quit-observer budget; the native smoke checks the real application path with a normal disk.
