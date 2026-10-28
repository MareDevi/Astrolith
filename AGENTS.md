# Astrolith development

- Read the current task in `docs/superpowers/plans/2026-10-28-astrolith-implementation.md` and its review before implementation. The referenced design specification is currently missing; M00 uses the plan provisionally. Do not invent the specification or begin product architecture before the M00 gate is resolved.
- `mise.toml` owns project tool versions and task entry points. Use `mise install` and `mise run …`; never install project tool versions globally or add competing version files.
- Commit `Cargo.lock`. Build and test with `--locked`. Commit Bun lockfiles when Astro fixtures are introduced; no fixture exists at bootstrap.
- Depend on the pinned `gpui-kit` facade. Verify APIs in the matching registry source before using examples from newer documentation. Read the GPUI Kit design and coding guides before changing the UI.
- Keep domain/services out of the desktop bootstrap. GUI rendering must not perform filesystem operations or blocking I/O. Domain logic, parsers, persistence, and services require TDD.
- Run `mise run check` before committing and report actual results. It composes doctor, formatting, lint, build, and headless tests. `mise run ci` uses the same checks on Linux, macOS, and Windows.
- Bootstrap/spikes need real build and bounded window smoke evidence. Do not add tests that merely repeat configuration. Later meaningful behavior must receive appropriate tests.
- Native IME, focus, scaling, and rendering acceptance require real-platform evidence; a successful headless test or build does not establish those results. External browser preview remains the normal Wayland path; embedded preview must remain optional.
- End each implementation task with verification and a Conventional Commit. Preserve unrelated changes and the existing review documents.

## Current library documentation

For library/framework/SDK/API/CLI/cloud usage, first resolve the library with Context7 MCP, then query current documentation. Use the exact `/org/project` ID directly only when supplied. Select the best official relevant match; if the result is insufficient, retry the same library with research mode when the installed tool supports it. Validate library-specific APIs against the pinned source as well. This requirement does not apply to general programming, business logic debugging, refactoring, or code review.

Follow the developer's RTK command-output guidance when available at `/home/maredevi/.config/codex/RTK.md`; its absence on another developer's machine is not a build prerequisite.
