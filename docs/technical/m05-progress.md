# M05 — Astro preview services progress

Started: 2026-10-31. Base: `dev` at `d15daa0`, continuing from `fca3fcd` (M05.1, implemented by the
`astrolith-m05-preview` team before it was halted). Scope: M05.1 dev-server lifecycle, M05.2 entry route
resolution, M05.3 external browser preview plus the Preview-panel wiring, and the M05 acceptance gate.
The original product specification remains absent; this note follows the implementation plan and its review.

**Review status: independently reviewed and repaired (2026-10-31).** The independent review
found one P1 and four P2/P3 issues; all were fixed in `3f398fe` and re-verified, including a
successful opt-in M05 gate run. See "Independent review and repairs" below.

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

## Independent review and repairs (`3f398fe`)

The owed independent review found and the repairs fixed:

- **P1 — preview never attached without an edit.** `SessionChanged` fires only on text
  changes, so opening a project did not create a backend, project switches kept the old
  server alive, and the panel button could open a stale entry. The shell now also observes
  the workspace entity, and `ensure_attached` only rewrites a changed mapping.
- **P2 — Windows opener command injection.** `cmd /C start` re-parsed `&`, `|`, `^`,
  parentheses and `%` in entry-derived URLs. Windows now launches
  `rundll32.exe url.dll,FileProtocolHandler <url>` with the URL as one argument, and a
  regression test keeps reserved characters in a single argument.
- **P2 — auto-incremented ports killed.** A "port in use, trying another" warning was
  treated as fatal before the child exited. It is now classified only after `try_wait`
  returns an exit; a fixture that warns and then binds another port proves startup
  succeeds.
- **P2 — post-ready death unobserved and Start-while-ready state loss.** The panel polls a
  new `DevServer::poll_exit` every 250 ms and reports an unexpected exit without holding
  the server mutex (so Stop can still acquire it); Start while ready keeps the URL and says
  the server is already running.
- **Astro 7 foregrounding.** Astro 7 auto-backgrounds `astro dev` when it detects an agent
  environment, detaching into its own session outside the owned process group (observed as
  orphaned servers after a gate run). `DevServer` now passes `--ignore-lock` when the
  installed major is 7+, which opts out of that inference and the cross-process lock file;
  Astro 5/6 never receive the unknown flag.
- **P3 — gate credibility.** The gate now publishes the edit through a same-directory
  fsync+rename (the save path's atomic replacement) and asserts the whole process group is
  gone, not only that the port closed.

Verification after the repairs: `mise run check` **PASS**; the opt-in gate
(`cargo test -p astrolith-preview --locked --test preview_gate -- --ignored --nocapture`)
**PASS** in 2.3 s with no surviving `astro dev` process.

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
7. **Windows opener is untested on Windows.** The `rundll32` command shape is asserted by a
   platform-gated test but cannot run on this host; a Windows runner must confirm it launches a browser.
8. **Astro 7 detection relies on the installed `package.json` major.** If it cannot be read, the server
   starts without `--ignore-lock`; in an agent environment Astro 7 may then background itself. Resolution
   already needs that file, so this is a defensive fallback, not a normal path.
