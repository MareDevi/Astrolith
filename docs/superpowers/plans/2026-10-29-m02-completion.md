# M02 Completion Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans with independently scoped parallel workers and a fresh final review. Steps use checkboxes for tracking.

**Goal:** Open a supported real Astro project, discover/index local Markdown and MDX entries, and edit retained document sessions without losing text across navigation.

**Architecture:** GPUI owns presentation and editor entities. Pure Astro inspection and workspace filesystem services run through the existing bounded runtime lane. Logical entry identity stays separate from source identity. Document buffers track revisions and original content; saving remains M04.

**Tech Stack:** pinned GPUI Kit 0.8.1; Mise Rust/Node/Bun; Tree-sitter TypeScript; exact-version Astro fixtures; filesystem watcher.

**Spec:** ../specs/2026-10-29-m02-project-discovery-design.md, original implementation plan M02.1–M02.4 and its review, concurrency amendment. User approved discovery proposal and completion of all M02 on 2026-10-29.

## Global constraints

- Keep desktop bootstrap thin; no filesystem work or blocking waits in render/UI callbacks.
- Use Mise task entry points; commit Cargo/Bun locks; build/test with --locked.
- Retain mutable sessions across entry/tab switches; reject stale project/document/revision completions.
- No saving or destructive discard. Dirty project switching and native window closing require an explicit decision; default preserves the buffer. Platform/application shutdown has no veto in the pinned GPUI path and remains a documented limitation until M04 recovery.
- Static discovery rejects unsupported configuration. Index never guesses custom loader IDs.
- One retained owner per background operation; at most two scans, bounded watcher batches (64), merge window 100 ms, overflow requests a rescan.
- Native smoke and headless interaction evidence remain distinct from IME/scaling/native latency acceptance.

## Work and validation

### M02.2 — discovery and fixtures

- [x] Lock exact Astro 5 and current available major fixtures; verify config/loader APIs in installed source.
- [x] Add a fixture-only real glob loader oracle and bounded frozen fixture checks.
- [x] RED: metadata, static AST subset, unsupported syntax, paths, version and cancellation tests.
- [x] GREEN: GPUI-independent Astro service and explicit Ready/Unsupported results.
- [x] Add runtime ownership and project generation rejection during shell integration.
- [x] Run check, review, commit.

### M02.3 — collection index and watcher

- [x] RED: nested MD/MDX, ignored files, overlapping collections, duplicate IDs, symlink/traversal, cancellation.
- [x] GREEN: explicit entry-to-source mapping compatible with real Astro oracle.
- [x] Add bounded watcher coalescing, overflow rescans, generation validation and duplicate load coalescing.
- [x] Verify create/delete/rename and project-switch stale results; run check, review, commit.

### M02.4 — document sessions and shell

- [x] RED: revisions/dirty undo, retained buffers, async completion identities, picker/project flow and actual keyboard editing.
- [x] GREEN: pure document buffers; retained GPUI EditorState per canonical source; one source session for overlapping collections.
- [x] Add project picker, navigable indexed entries/tabs, dirty status, reload-safe watcher behavior and guarded dirty close/project switch/native window close; record the platform-quit limitation.
- [x] Verify complete real-project workflow including MDX body, out-of-order loads and background indexing.
- [x] Run check, bounded native smoke, independent review, update progress and commit.

## Execution decisions

The user's approval resolves the proposal gate. Work proceeds in the existing clean checkout on a task branch; no additional user-owned chat or worktree is needed. Independent workers have disjoint file ownership; the main executor owns shared Cargo/Mise configuration, discovery, runtime and shell integration. Exact fixture compatibility and unresolved native acceptance are reported honestly.

## Completion evidence

`mise run check` passed with 130 Rust tests, zero failures, two intentionally unrun performance diagnostics and one installed-fixture editor test that the fixture stage explicitly ran against both actual releases. Five Node harness tests passed; both real loader/production comparisons matched all 11 supported mappings. Both installed-project GPUI editing workflows passed with unchanged disk sources. Native Wayland mapping and opening the GTK project picker passed; native project selection/editing, IME/scaling and latency acceptance remain separately unverified. Discovery and integration reviews were resolved; bounded whole-index refresh remains a documented M02 performance limit. See `docs/technical/m02-progress.md`.
