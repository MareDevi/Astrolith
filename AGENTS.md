# Astrolith development

- `mise.toml` owns project tool versions and task entry points. Use `mise install` and `mise run …`; never install project tool versions globally or add competing version files.
- Commit `Cargo.lock`. Build and test with `--locked`. Commit Bun lockfiles when Astro fixtures are introduced; no fixture exists at bootstrap.
- Depend on the pinned `gpui-kit` facade. Verify APIs in the matching registry source before using examples from newer documentation. Read the GPUI Kit design and coding guides before changing the UI.
- Keep domain/services out of the desktop bootstrap. GUI rendering must not perform filesystem operations or blocking I/O. Domain logic, parsers, persistence, and services require TDD.
- Run `mise run check` before committing and report actual results. It composes doctor, formatting, lint, build, and headless tests. `mise run ci` uses the same checks on Linux, macOS, and Windows.
- Bootstrap/spikes need real build and bounded window smoke evidence. Do not add tests that merely repeat configuration. Later meaningful behavior must receive appropriate tests.
- Native IME, focus, scaling, and rendering acceptance require real-platform evidence; a successful headless test or build does not establish those results. External browser preview remains the normal Wayland path; embedded preview must remain optional.
- Commit often. Every atomic, self-contained change gets its own commit — never batch unrelated work or wait until a task is finished. End each implementation task with verification and at least one commit. Preserve unrelated changes and the existing review documents.

## Minimal tests, minimal code

- Tests cover only important logic and functionality: the behavior and domain rules the task is about, plus the boundaries, invariants, and error paths that matter for them. Do not test trivial glue, configuration, UI wiring, getters/setters, or incidental behavior no consumer can observe.
- Write only necessary tests: do not add redundant tests, tautologies, duplicates of an existing case, or tests that restate the implementation.
- Verification beyond a focused test is a throwaway smoke run, not a committed test; do not turn every manual check into a permanent test.
- Write only the code the task requires. Do not add speculative abstractions, unused helpers, dead branches, defensive checks with no failure mode, or duplicated logic; delete code made obsolete by the change.

## Commits

- Use Conventional Commits: `<type>(<scope>): <description>`, with `feat`, `fix`, `refactor`, `test`, `docs`, `build`, `ci`, `chore` as the allowed types; imperative, lowercase description. Add a body only when the change needs rationale.
- One logical change per commit: it must build and pass `mise run check` on its own, and be revertible without dragging in unrelated files. Split refactors, behavior changes, formatting, dependency bumps, and docs into separate commits.
- Commit as soon as a change is verified, including intermediate progress that leaves the tree green. Do not stage unrelated files, generated artifacts, or secrets.
- Use `--locked` for any Cargo command in the commit workflow; commit the resulting `Cargo.lock` change with the dependency it belongs to.

## Current library documentation

For library/framework/SDK/API/CLI/cloud usage, first resolve the library with Context7 MCP, then query current documentation. Use the exact `/org/project` ID directly only when supplied. Select the best official relevant match; if the result is insufficient, retry the same library with research mode when the installed tool supports it. Validate library-specific APIs against the pinned source as well. This requirement does not apply to general programming, business logic debugging, refactoring, or code review.

Follow the developer's RTK command-output guidance when available at `/home/maredevi/.config/codex/RTK.md`; its absence on another developer's machine is not a build prerequisite.
