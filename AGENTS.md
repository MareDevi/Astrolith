# Astrolith development

## Toolchain and architecture

- `mise.toml` owns project tool versions and task entry points. Use `mise install` and `mise run …`; never install project tool versions globally or add competing version files.
- Use `--locked` for Cargo builds and tests. Track `Cargo.lock` and commit its changes with the dependency changes that require them. Commit Bun lockfiles with Astro fixtures.
- Depend on the pinned `gpui-kit` facade and verify APIs against the matching registry source. Read the GPUI Kit design and coding guides before changing the UI, using their reading scope for the size of the change.
- Keep domain logic and services out of the desktop bootstrap. Keep filesystem operations and blocking I/O out of GUI rendering.
- Keep external browser preview as the normal Wayland path and embedded preview optional.

## Implementation and tests

- Write only the code the task requires. Do not add speculative abstractions, unused helpers, dead branches, defensive checks with no failure mode, or duplicated logic; delete code made obsolete by the change.
- Write unit tests only for important logic, such as domain rules, parser boundaries, persistence invariants, and consequential error paths. Reuse existing tests where they catch the regression. Add only the few cases needed to prove the behavior. Do not pursue coverage targets, add tests in bulk, or test trivial glue, configuration, getters, setters, or internal details with no observable effect.
- Use TDD for changes to behavior in domain logic, parsers, persistence, and services. Start with a focused failing test at the layer that owns the behavior. Reuse an existing test when it can demonstrate the failure. Use smoke runs for supplemental verification without turning every manual check into a permanent test.

## Verification and acceptance

- Bootstrap and spike work requires a real build and a bounded window smoke run. Native IME, focus, scaling, and rendering acceptance requires evidence from the relevant platform.
- Preserve the GPUI Coding Guides' human review and testing requirement. Report automated checks, native evidence, and pending human review separately. Continue any independent verification permitted by the applicable gates.

## Commits

- Run `mise run check` before committing each independently buildable change. Focused verification is one step in this workflow. Report the actual result and any outstanding acceptance checks. `mise run ci` uses the same checks on Linux, macOS, and Windows.
- Commit each verified logical change promptly and end every completed implementation task with at least one commit. Keep unrelated changes out of each commit and preserve existing review documents. Do not stage unrelated files, generated artifacts, or secrets.
- Use Conventional Commits in the form `<type>(<scope>): <description>`. Allowed types are `feat`, `fix`, `refactor`, `test`, `docs`, `build`, `ci`, and `chore`. Use an imperative, lowercase description. Add a body when the change needs rationale.
