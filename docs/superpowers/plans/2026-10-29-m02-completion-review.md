# M02 completion review

Date: 2026-10-29. Base: `3e35e87`. Scope: approved M02.2–M02.4 implementation, fixtures and shell integration. The original implementation review remains unchanged.

## Findings and resolutions

1. **Static JavaScript authority:** special `__proto__` object semantics and forward-reference TDZ could produce false manifests. Both now receive explicit diagnostics with parser regressions.
2. **Compatibility evidence:** fixture expectations initially did not consume production output. The actual loader oracle now compares every supported `(collection,id,filePath)` triple against the production Rust index for both exact releases.
3. **Publication races:** watcher startup could miss configuration changes, and first document loads lacked an index revision. Discovery is repeated after watch registration; index publications cancel old loads. The owning completion path checks project identity, generation, index revision and initial document revision before creating a session. Mutation tests reproduced the old stale-tab and false-change behavior before restoring the fix.
4. **Retained snapshots:** source changes originally marked all sessions and lost their visible warning across tabs. Background fingerprints now cover open and in-flight sources; a refresh admission gate closes snapshot/load races. Per-source warnings and index errors persist, including failed reindexing. Editors are never replaced by background refresh.
5. **Lifecycle and resource bounds:** production watch registration now uses selective nonrecursive paths, including root-based collections, while excluding dependency/metadata trees. The service runtime owns final native-watcher disposal. Loaded baseline source admission caps at 32 MiB; document count and individual size limits are explicit.
6. **Existing navigation:** readonly file opening after project initialization now mounts an editor; opening a project reveals a hidden navigator. Retained sessions survive either navigation path.
7. **Frontmatter authority:** BOM/leading blanks are handled; TOML, aliases, merge keys, tags, complex keys, implicit timestamp slugs and ambiguous plain slug types receive explicit diagnostics. YAML events/depth are bounded before materialization. Truthy non-string slugs are rejected rather than coerced using guessed JavaScript semantics.
8. **Final independent P1 review:** MDX patterns originally lacked integration verification, and legacy flags were accepted as inert values. Actual verified MDX import/call provenance and exact installed Astro/MDX pairs are now required. Unknown integrations, options and legacy/experimental configuration are rejected. Iterative AST limits run before recursive interpretation.

The final independent targeted re-review reported **no blockers** after the last authority fixes. Focused parser/discovery tests passed; the reviewer's ordinary sandbox denied Unix socket creation with `EPERM`. The complete parent-run suite outside that restriction passed.

## Final evidence

- `mise run check`: doctor, formatting, warnings-denied Clippy, locked build and tests passed; **130 Rust tests passed, zero failed**.
- Three ordinary-suite ignored tests: two performance diagnostics and one installed-project editor workflow. The fixture stage explicitly ran the workflow for both Astro 5.18.2 and 7.4.4; both passed.
- Five Node harness tests passed. Real frozen installs, Astro sync and loader oracles matched all **11 supported mappings per release**. Actual installed-project GPUI picker/edit/MDX/tab flows preserved disk source bytes.
- Bounded native Wayland smoke mapped the shell and opened the GTK project directory picker. Automated native project selection did not complete; native editing, IME/scaling and latency are not claimed by that smoke.

## Accepted M02 limits

Watcher bursts currently trigger bounded full rediscovery/rescan; incremental updates remain future performance work. Full-text domain mirroring still occurs on editor changes. Native responsiveness acceptance is separate. Saving, conflict resolution and recovery remain M04; forced or platform-wide shutdown can discard in-memory edits, while native window close is guarded. Hosted macOS/Windows results remain unverified.
