# Native-window acceptance — save, conflict, recovery and exit flush

Status: **MANUAL, PENDING HUMAN EXECUTION.** This document is a step-by-step procedure for
exercising the M04 persistence path in a **real native window** (GPUI). It does not record a
completed result yet.

## Why this exists (honest evidence boundary)

AGENTS.md requires real-platform evidence for native IME, focus, scaling and rendering: a
successful headless test or build **does not** establish those results. The save/recovery code
paths listed below are covered by headless tests (`mise run check` PASS — 256 Rust tests, 0
failed, 4 ignored; Astro fixture compatibility PASS), but those tests run without a native window,
compositor, GPU, real keyboard or OS quit path. In particular:

- A headless test can assert that `runtime::flush_on_quit` writes a recovery record; it cannot
  prove that an actual OS window-close/issues app-quit drains that flush before the process exits.
- A headless test can assert that a conflict is reported and the disk file is untouched; it cannot
  prove the native status line is readable, that focus/IME behave, or that scaling renders the
  Inspector correctly.

Treat this runbook as the boundary between "headless evidence exists" and "native evidence
pending". Fill in the result table at the end only after executing it on real hardware.

## Environment

- **Primary target:** Linux Wayland (external/real compositor). `mise run dev` opens the native
  window; per AGENTS.md the external browser preview is the normal Wayland path and an embedded
  preview is optional (not exercised here).
- **Also valid targets:** macOS and Windows (record which one you used).
- **Toolchain (from `mise.toml`):** Rust `1.99.1`, Node `26.12.1`, Bun `1.4.4`. Use
  `mise install` and `mise run …`; do not install project tool versions globally.
- **Fixtures used as the project under test:** copy `fixtures/astro5/workspace` (Astro `5.18.2`,
  `@astrojs/mdx 4.3.14`) or `fixtures/astro7/workspace` (Astro `7.4.4`, `@astrojs/mdx 8.1.2`) into a
  scratch directory and run `bun install --frozen-lockfile` in the copy. Astro must be installed in
  the copy so schema generation (`astro sync`) resolves; otherwise the frontmatter form stays
  unavailable (this is non-fatal by design).
- **Isolated recovery directory:** export `ASTROLITH_RECOVERY_DIR` to an **absolute** scratch path
  (e.g. `export ASTROLITH_RECOVERY_DIR="$HOME/.cache/astrolith-acceptance/recovery"`). It must be
  absolute or the app reports the setting as invalid. This keeps the test from touching your real
  `~/.config/astrolith/recovery`.
- **A second shell / text editor** to modify a file on disk for the conflict step.
- **Node on PATH** (mise provides it) — opening a project runs `astro sync` and can show a short
  delay.

### Preconditions

1. `mise install` succeeds and `mise run doctor` passes.
2. `mise run check` passes (headless baseline; record its exit status and test counts).
3. You have a `bun install`-ed fixture copy and a writable `ASTROLITH_RECOVERY_DIR`.
4. Start the app with `mise run dev` (or `cargo run --locked --package astrolith-desktop`).

## Procedure

Record a screen capture (or screenshots) and note the OS, compositor/GPU, display scale and
input-method details as you go. For each step record **Observed** vs **Expected**.

### A. Save success

1. In the app, open the fixture project (`Open project…` / `Ctrl+Shift+O` / `Cmd+Shift+O`).
2. In the navigator, open a Markdown entry (e.g. under `content/docs`).
3. Type a small, unique edit into the **body** (e.g. append `native-save-check`).
4. Press `Ctrl+S` / `Cmd+S` (also try the palette `Save` command and the Save button if present).
5. Expected: the status line reports a successful save; the document is no longer dirty; the file
   on disk contains the edit (`git diff`/`cat` in the second shell shows only that change, and
   **only** that change — untouched frontmatter/body bytes are preserved exactly).
6. Expected: the recovery directory contains no record for that source after the save clears it
   (a short delay is acceptable while the save task completes).

### B. Save conflict (external change must not be overwritten)

1. Open an entry and make an in-app edit **but do not save**.
2. In the second shell, modify the same file on disk externally (e.g. change a different line and
   save it).
3. Return to the app and press `Ctrl+S` / `Cmd+S`.
4. Expected: the status line reports a **save conflict: the file changed on disk**; the app's edits
   are **kept** in the buffer (document stays dirty); the on-disk file is **not** overwritten by the
   app (the external content remains).
5. Expected: only after "reopen the document to reload" does the on-disk version replace the
   buffer.

### C. Debounced recovery capture (do not save, wait)

1. Open an entry and type an edit. **Do not save.**
2. Wait longer than the 500 ms debounce (e.g. 2 s) without touching quit/save.
3. Expected: a recovery record file appears under `ASTROLITH_RECOVERY_DIR` (filename is a stable
   hash; content is JSON). Its stored content includes the unsaved edit. The original file on disk
   is **unchanged**.

### D. Startup auto-apply

1. Continuing from step C (a recovery record exists), quit the application **without saving**
   (use the window close / app quit; accept the "discard" prompt only if it appears *and* you have
   confirmed a recovery record already exists on disk — prefer the normal quit path so the exit
   flush also runs).
2. Reopen the app and open the same project.
3. Expected: the record is filtered to this project and **auto-applied** — the document reopens as a
   **dirty** session whose buffer contains the unsaved content, with the on-disk file as the
   baseline. The disk file is **not** written by the restore. The status line shows a restoring/
   restored message.
4. Expected idempotency: run `Recover unsaved changes…` again; it reports "No recovery records to
   restore." and does **not** duplicate the session.
5. Expected: switching projects does not carry stale pending records to the next project.

### E. Native-exit flush (edit then quit before the debounce)

1. Open an entry, type a distinctive edit, and **quit within the 500 ms debounce window** (before a
   record would be written by the periodic capture). Ideally do this with the app focused and use
   the native window/app quit.
2. Expected: the `App::on_app_quit` observer snapshots the dirty editor text and submits a durable
   flush; a recovery record for that source now exists in `ASTROLITH_RECOVERY_DIR` **even though the
   debounce had not fired**, and the original file on disk is untouched.
3. Reopen the app + project; confirm the edit is restored via startup auto-apply (step D behaviour).
4. This is the step that most needs real native evidence: a headless test cannot confirm the OS
   quit path actually drains the flush before process exit. If the record is missing after a real
   quit, treat native-exit flush as **FAILED** and report it.

### F. (Record, not a gate) form / rendering sanity in the same session

While the window is open, also note for the human reviewer: Chinese/Japanese IME composition in the
source editor and in a frontmatter text field, focus movement between the main editor and the
Inspector form, HiDPI/fractional scaling of the Inspector, and whether the frontmatter form reflects
edits. **Known open issue to watch:** editing the body in the source editor and then editing a
frontmatter field can overwrite the body edit (P1 in `docs/technical/m03-progress.md`). If observed,
record it explicitly; it is not fixed by this runbook.

## Result template (fill in after a real run)

| Field | Value |
|---|---|
| Date / operator | |
| OS + version | |
| Compositor / GPU / driver | |
| Display scale (e.g. 1.0 / 1.25 / 2.0) | |
| Input methods tested (IME) | |
| App build (commit) | |
| `mise run check` result | |
| A. Save success | PASS / FAIL — notes |
| B. Save conflict | PASS / FAIL — notes |
| C. Debounced recovery capture | PASS / FAIL — notes |
| D. Startup auto-apply (incl. idempotent manual recover) | PASS / FAIL — notes |
| E. Native-exit flush (quit before debounce) | PASS / FAIL — notes |
| Recovery record paths observed | |
| Evidence artifacts (screenshots / recording) | |

Until this table is filled in from a real native run, the native evidence for save, conflict,
debounced recovery capture, startup auto-apply and native-exit flush is **pending**, and only the
headless results in `mise run check` and the crate test suites are established.
