# M05 — Astro preview services progress

Started: 2026-10-31. Base: `dev` at `d15daa0`, continuing from `fca3fcd` (M05.1, implemented by the
`astrolith-m05-preview` team before it was halted). Scope: M05.1 dev-server lifecycle, M05.2 entry route
resolution, M05.3 external browser preview plus the Preview-panel wiring, and the M05 acceptance gate.
The original product specification remains absent; this note follows the implementation plan and its review.

**Review status: NOT independently reviewed.** The team that was implementing this milestone hit a provider
rate limit (WorkBuddy, reset 18:29 UTC+8) and the user terminated it, so the remaining work was completed
directly. This document is a self-assessment. The plan's independent-review step for M05 is still owed.

## Binding contract (from `concurrency-and-responsiveness.md`)

Runtime-owned child process, both output drains, 1 MiB retained log per stream, 30 s startup bound, 3 s
graceful stop then terminate and reap, no orphaned Astro descendants on project switch or exit, and
platform-specific process ownership.

## M05.1 — Dev server lifecycle (`fca3fcd`)

`crates/astrolith-preview/src/{server.rs,process.rs}`:

- `ServerStatus { Stopped, Starting, Ready(url), Stopping, Failed(String) }` plus `DevServer`, which resolves
  the project-local `node_modules/astro` entry and runs `node <entry> dev --host 127.0.0.1 --port <n>`.
  Readiness is parsed from the printed local URL; non-zero exit, occupied port, missing install and timeout
  all produce `Failed` with the captured output excerpt.
- Both streams are drained continuously into a `OutputCapture` capped at 1 MiB per stream (bytes past the cap
  are counted and dropped, so the pipe never fills). `settle_drains` is bounded, so a lingering grandchild
  cannot block a stop.
- Unix: the child is a process-group leader and the whole group receives SIGTERM, then SIGKILL after the
  grace period, then the direct child is reaped. Windows: a Job Object with `KILL_ON_JOB_CLOSE`, `cfg`-gated.
  `Drop` is a kill-on-drop safety net.
- 20 tests: ready/stopped transitions, missing install, bad exit, occupied port, timeout, cancellation with
  reaping, grandchild killed with the group, and a 4 MiB flood retaining exactly 1 MiB.

## M05.2 — Entry route resolution (`59e8544`)

`routes.rs` adds `resolve_entry_url(&EntryId, &RouteMapping, &Url)`. The default convention is
`/<collection>/<entry id>`; a collection can be explicit-only and an entry can override its route by full id
or by final slug. A trailing `index` segment folds into its parent. Each segment is percent-encoded through
`Url::path_segments_mut`, so CJK ids (the fixtures include `深い/日本語`) and reserved characters become a
portable path instead of a query/fragment surprise. `..`, empty segments and non-`http(s)` bases are refused.
`config.rs` builds the mapping from a content-config *source string* (static tree-sitter parse, no project
execution) or from an inspected `ProjectManifest`. 20 tests.

## M05.3 — External preview and the M05 gate (`2c8c562`, `365bcbe`, `1ba5ed3`)

- `surface.rs`: the `PreviewSurface` seam. `validate_preview_url` refuses anything that is not an absolute
  `http(s)` URL, so `file:` and custom schemes never reach the desktop opener. `RecordingSurface` records
  opens for tests.
- `external.rs`: `ExternalBrowser` spawns the platform opener (`open`, `cmd /C start`, `xdg-open`) detached
  with null stdio and never awaits it, so a service call cannot block on a browser that stays open. The
  `Opener` trait lets tests assert the exact command and launch failures without a desktop.
- `preview.rs`: `PreviewSession` turns a `ServerStatus::Ready` base plus a mapping into the entry URL and
  hands it to the surface. It never starts or stops a server, so reopening an entry reuses the running one.
- Fixtures gained `workspace/pages/docs/[...slug].astro` (both Astro 5 and 7), which renders `docs` entries
  with `getCollection` + `render(entry)`; without it every entry route was a 404.
- **M05 gate** (`crates/astrolith-preview/tests/preview_gate.rs`, opt-in `#[ignore]`): copies the frozen
  `astro7` fixture, runs `bun install --frozen-lockfile`, starts the real `astro dev` through `DevServer`,
  fetches the entry route, rewrites the entry's frontmatter **and body** on disk (what a save does), polls
  until the served HTML reflects the change, then stops the server and asserts the port is released. The
  entry id is `stable-source-slug`, because `introduction.md` declares that frontmatter slug.

  Run it with:

  ```bash
  cargo test -p astrolith-preview --locked --test preview_gate -- --ignored --nocapture
  ```

  Result against Astro 7.4.4: **PASS** (2.2 s wall clock with a warm Bun cache).

## Preview panel wiring (`1ba5ed3`, `37668a0`)

`crates/astrolith-gpui/src/preview/` hosts the panel that the shell mounts in the Preview dock:

- `PreviewBackend` is the seam; the real `AstroPreviewBackend` owns a `DevServer` behind a Tokio mutex and
  runs start/stop through `runtime::spawn`, so no UI thread blocking and no filesystem work in render.
- Status (`Stopped`/`Starting`/`Ready(url)`/`Stopping`/`Failed`) plus the last message are rendered, with
  Start, Stop and Open-in-browser buttons.
- New commands `StartPreview`, `StopPreview`, `OpenPreview` (palette + buttons; no new key bindings).
- The shell attaches the panel on session changes, refreshes only the route mapping when the same project is
  re-reported (so a session change never restarts a running server), tracks the active document's entry, and
  aborts the server on application quit. `DevServer::abort` is the synchronous exit-path kill.
- Seven panel tests (ready/failed start, stop, detach abort, not-ready refusal, surface launch, surface
  failure) plus a shell test that the commands explain what is missing when no project or entry is open.

## Verification

- `mise run check`: **PASS** — doctor, formatting, warnings-denied Clippy, locked build, workspace tests and
  the frozen Astro fixture task (**Astro fixture compatibility: PASS**).
- `cargo test --workspace --locked`: **323 passed, 0 failed, 5 ignored** (the ignored tests are the opt-in
  installed-project and performance diagnostics, including the M05 gate above).
- `cargo test -p astrolith-preview --locked`: 55 passed + 1 ignored (gate).
- `cargo test -p astrolith-gpui --locked`: 58 passed, 2 ignored; eight consecutive runs clean after the
  preview tests were made deterministic (the GPUI test scheduler could miss a condition notification when
  several runtime-driving scenarios ran concurrently).

## Gate status

The M05 gate — "editing an article and saving it results in an updated real Astro preview page" — is met
**headlessly**: the opt-in test drives the real dev server and the real file watcher and observes the saved
edit in the served HTML. It is **not** met for native presentation: the browser launch path and the rendered
page on a real desktop were not observed in this environment. Per `AGENTS.md`, a successful headless test
does not establish native behaviour; native browser/rendering acceptance remains **pending human execution**.

## Known limits

1. **Windows process ownership is unverified.** The Job Object path is `cfg`-gated and cannot be built or run
   on this Linux host; it needs Windows CI or native evidence before Windows is claimed.
2. **Route mapping is convention-based.** `/<collection>/<entry id>` matches the fixtures and a common Astro
   layout, but a project with custom `getStaticPaths` output is not analysed; an entry can be mapped
   explicitly, and an unmapped entry is a typed error rather than a guessed URL.
3. **The gate covers Astro 7 only.** The page exists in the Astro 5 fixture too, but the gate test runs
   against `fixtures/astro7` (the newest supported release).
4. **`bun`, `node` and network/cache are required** to run the gate test; that is why it is `#[ignore]`-d.
5. **Embedded preview is out of scope** — M06 owns the WebView surface and capability detection; the external
   surface is the required MVP path.
6. **No cross-launch server reuse**: a server is owned per project attachment; a previously running external
   server started outside Astrolith is not adopted (its URL is not discovered).
7. **Independent review is owed** (see the top of this document).
