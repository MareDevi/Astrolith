# Astrolith development

## Toolchain and architecture

- `mise.toml` owns project tool versions and task entry points. Use `mise install` and `mise run …`; never install project tool versions globally or add competing version files.
- Use `--locked` for Cargo builds, tests, and other commit checks that accept it. Track `Cargo.lock` and commit its changes with the dependency changes that require them.
- Depend on the pinned `gpui-kit` facade. Verify API signatures in the matching registry source. Read the GPUI Kit design and coding guides before changing the UI. Read the full design guide for a new screen or redesign. For a narrow UI change, read the required sections and the sections relevant to the change.
- Keep domain logic and services out of the desktop bootstrap. Keep filesystem operations and blocking I/O out of GUI rendering.
- Keep external browser preview as the normal Wayland path and embedded preview optional.

## Implementation

- Write only the code the task requires. Do not add speculative abstractions, unused helpers, dead branches, defensive checks with no failure mode, or duplicated logic; delete code made obsolete by the change.

## Tests

- Limit unit tests to important logic: domain rules, parser boundaries, persistence invariants, and error paths with observable consequences. Add only a few cases for gaps that matter. Do not add unit tests in bulk, chase coverage targets, or test trivial glue, configuration, getters, setters, and implementation details.
- Use TDD for behavior changes in domain logic, parsers, persistence, and services. Reuse a focused test that demonstrates the failure, or add one at the layer that owns the behavior. Use a smoke run for supplemental checks that do not warrant a permanent test.

## Verification and acceptance

- Treat focused regression checks as evidence for the fix, then complete the required project checks and delivery steps.
- For bootstrap or spike work, build the app and run a bounded window smoke test. Verify native IME, focus, scaling, and rendering on the relevant platform when the task requires acceptance of those behaviors.
- GPUI code covered by the Coding Guides' human review and testing requirement needs both. Complete independent verification allowed by the applicable gates and report automated checks, native evidence, and human review separately. Mark any unverified acceptance criterion as pending.

## Commits

- Require `mise run check` to pass before committing each independently buildable change. Report the actual result. If a failure remains after investigation, name the failing check and the blocker. `mise run ci` runs the same checks on Linux, macOS, and Windows.
- Commit each verified logical change promptly. Deliver each completed implementation task with at least one commit. Preserve existing review documents and unrelated changes. Keep unrelated files, generated artifacts, and secrets out of the commit.
- Use Conventional Commits in the form `<type>(<scope>): <description>`. Allowed types are `feat`, `fix`, `refactor`, `test`, `docs`, `build`, `ci`, and `chore`. Use an imperative, lowercase description. Add a body when the change needs rationale.
