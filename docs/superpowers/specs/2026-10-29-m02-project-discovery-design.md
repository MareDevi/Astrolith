# M02.2 project discovery — proposal

Date: 2026-10-29 (Asia/Shanghai). Status: proposed; user approval pending.

This is a scoped proposal for M02.2, not a reconstruction of the missing original Astrolith specification. The implementation plan, original review R2/R6, M02.1 models, and concurrency amendment remain authoritative. M00 is accepted; M01 and M02.1/M02.R are implemented. Native responsiveness acceptance remains open.

## Outcome

Identify a selected local Astro project and describe the local Markdown/MDX collection sources that Astrolith can safely understand. Preserve explicit source mappings; never derive source paths from entry IDs. No editable sessions, index watchers, schema generation, document writes, preview processes, or new UI belong to this task.

Project discovery must distinguish “recognized Astro project” from “collections can be indexed.” Unsupported configuration returns diagnostics and no authoritative collection descriptors. It must never report an empty, successfully understood project when configuration could not be understood.

## Approaches

1. **Conservative static inspection (recommended).** Parse a documented subset of JavaScript/TypeScript configuration without executing it. Handle literal local `glob()` declarations; reject dynamic configuration. This keeps project opening read-only and avoids starting arbitrary loaders, but deliberately limits initial compatibility.
2. **Project-local Astro execution.** Resolve and run project-local tooling to obtain actual configuration and entry/source mappings. This can preserve dynamic configuration and custom IDs, but requires process ownership, termination, deadlines, output limits, dependency readiness, and an experimentally verified adapter per supported Astro version. `astro sync` and generated schemas alone do not establish authoritative loader/source mappings. This moves substantial M03/M05 responsibilities forward.

The recommendation is the author's proposal, not an existing user decision. A static parser must use a real syntax tree, not regular expressions or JavaScript evaluation. If pinned parser APIs or fixtures cannot establish the subset reliably, narrow the supported subset explicitly; never add inference fallbacks.

## Proposed static contract

### Project metadata

- Canonicalize a selected directory; inspect only that project, without ancestor search or automatic monorepo traversal.
- Require a regular UTF-8 `package.json` with an object at the top level and a nonempty string-valued `astro` dependency in `dependencies` or `devDependencies`. Preserve the declared requirement. Diagnose conflicting declarations and malformed metadata.
- Treat absent `astro.config.*` as default configuration, not proof that the project is invalid. Diagnose multiple candidate config files instead of picking one silently. Final extension support must be verified against each pinned Astro fixture.
- Accept default source directory and a literal project-contained `srcDir` in a supported `defineConfig({ ... })` export. Diagnose spreads, computed keys, imported configuration, callbacks and other dynamic forms that could affect the source directory. Do not execute configuration to resolve them.
- Detect modern content config under the resolved source directory. Detect legacy `content/config.*` separately and report unsupported legacy mode. Legacy compatibility flags must not silently activate guessed collection roots.
- Record canonical root and actual config paths. Discovery does not assign a persistent `ProjectId`; the workspace owner supplies it.

### Collection descriptors

- Require verified named imports of `defineCollection` from `astro:content` and `glob` from `astro/loaders`, accounting for aliases and rejecting shadowing.
- Accept exported `collections` as a literal object containing direct supported declarations or references to supported local constant declarations. Preserve exported collection names, including names differing from local variable names.
- Accept `defineCollection({ loader: glob({ base: <string>, pattern: <string> }) })` and an optional schema expression. Never evaluate schemas. Reject object spreads, reassignments, computed properties and unknown loader options; these may alter source or ID behavior.
- Require an explicit project-relative literal `base`. Canonicalize it; reject missing/non-directory bases and paths or symlinks escaping the canonical project root. Reject external config symlinks as well. Revalidate source containment at later file access; discovery is not a race-free security boundary.
- Initially accept only patterns `**/*.md`, `**/*.mdx`, and `**/*.{md,mdx}`. Pin and test their behavior against real Astro before marking them supported. Do not claim general micromatch compatibility.
- Preserve collection identity, canonical base, declared pattern, and config provenance in descriptors. Overlapping collection bases remain distinct.
- Diagnose custom `generateId`, `file()`, remote/custom loaders, imported collection declarations, dynamic bases/patterns and unsupported options. No entry IDs are synthesized at discovery time.
- M02.3 must establish Astro-compatible default IDs and explicit `Entry` mappings before indexing entries. Custom `generateId` remains unsupported until a project-local execution adapter proves its mapping. Tests must prove refusal instead of silently assuming filename IDs.
- If any declaration or export cannot be interpreted, expose diagnostics and make the collection set non-authoritative as a whole. Do not partially publish a potentially incomplete index.

### Service ownership

- Add a GPUI-independent `astrolith-astro` crate. Separate pure syntax/config interpretation from synchronous filesystem inspection.
- The application's existing bounded blocking lane runs synchronous discovery. An `async` facade must delegate to this lane; it must not run filesystem operations directly while polled on the UI thread. Avoid creating a second runtime or a per-project thread pool.
- Retain one request handle per owner. Carry the workspace-supplied project identity and monotonically increasing project generation through request and result. Check cancellation between bounded operations; a running OS read cannot be forcibly aborted.
- Reject obsolete generations at publication even when cancellation loses a race. Test queued cancellation and a late completion after switching projects. A pure comparison helper alone does not prove publication behavior.
- Cap each metadata/config read at 1 MiB using a bounded read, including files that grow after metadata inspection. Reject special files before reading. Discovery does not recursively walk content directories.

## Fixtures and acceptance

Before implementation, select available exact Astro releases and record their actual versions. Introduce an Astro 5 fixture and one current supported major fixture, each with a committed Bun lockfile. Verify installed source APIs for both. Do not invent an Astro 7 fixture if no release can be verified. Additional majors remain unclaimed until independently tested.

Fixtures include real `package.json`, Astro config, modern content config, custom local bases, overlapping collections, nested `.md` and `.mdx` sources, and unsupported custom-ID/legacy variants. Read-only metadata tests alone do not prove Astro compatibility: use each fixture's project-local `astro sync` to validate accepted declarations. Run sync only on test fixtures, with bounded process execution and temporary generated output; never on a selected user's project during static discovery.

TDD coverage must include:

- Valid project with default and explicit `srcDir`, absent optional Astro config, missing content config, non-Astro metadata, malformed JSON and conflicting Astro declarations.
- Literal local glob with custom base, aliased imports, exported names, two collections sharing a source base, and Markdown/MDX patterns.
- Legacy config, custom IDs, slug-bearing source, remote/file loaders, comments or string literals containing fake configuration, dynamic expressions, shadowed imports, spreads and reassignment. Supported syntax yields descriptors; unsupported syntax yields explicit diagnostics and no authoritative collection set.
- Invalid paths, missing bases, symlink escape, oversized/growing input and special files, with platform-appropriate cases.
- Queued cancellation, cancellation during bounded work and obsolete-project publication rejection through the actual owning coordinator.

Before commit, run `mise run check` with locked Rust builds/tests and frozen Bun fixture installs through Mise-owned task entry points. Add a bounded fixture compatibility task to `check`; use project-local Astro only. Record actual results, independent review, compatibility limits and a Conventional Commit. This service-only task changes no native UI, so it does not establish or require new native rendering acceptance.

## Implementation sequence after approval

1. Resolve current parser/Bun/Astro documentation and verify pinned APIs. Choose exact fixture/parser versions; record a focused implementation plan.
2. Create locked fixtures and prove local Astro accepts supported declarations. Keep unsupported cases diagnostic-only.
3. Add failing metadata and configuration tests; implement pure models, syntax inspection and bounded filesystem discovery.
4. Add failing cancellation/publication tests; connect discovery ownership to the existing runtime without adding UI behavior.
5. Run focused and full checks, independent review, update M02 progress and commit. Keep the M02 editing gate open for M02.3/M02.4.

## Evidence and limitations

Context7 resolved Astro to `/withastro/docs` and retrieved its official configuration, content collection, loader and legacy-flag documentation on 2026-10-29. These establish that local glob bases may differ from `src/content`, custom ID generation exists, `srcDir` is configurable, and legacy behavior is version-dependent. They do not establish compatibility of an unimplemented parser or any installed Astro version.

References: [configuration](https://docs.astro.build/en/reference/configuration-reference/), [content collections](https://docs.astro.build/en/guides/content-collections/), [loader API](https://docs.astro.build/en/reference/content-loader-reference/), [legacy flags](https://docs.astro.build/en/reference/legacy-flags/).

## Proposal verification

The proposal adds documentation only. `mise run check` passed on 2026-10-29: doctor, Rust formatting, Clippy with warnings denied, locked workspace build and all 46 tests; two performance diagnostics remain intentionally ignored. The first sandbox attempt stopped at doctor's Node-to-rustc spawn (`EPERM`); the same command passed with sandbox escalation. These checks validate the existing tree, not the proposed discovery implementation.

## Builder review resolutions

An independent builder review identified four missing contracts. The following resolutions are part of this proposal; they are author decisions awaiting user approval.

### Result and ownership boundary

Replace the plan's illustrative standalone async signature with a synchronous service function `inspect_project(request: &DiscoveryRequest) -> Result<DiscoveryOutcome, AstroError>`. `DiscoveryRequest` carries workspace-assigned `ProjectId`, `u64` generation, owned root path and a cloneable cooperative cancellation flag. `AstroError` represents cancellation, I/O failure, malformed metadata and non-Astro roots.

`DiscoveryOutcome` is an enum: `Ready(ProjectManifest)` or `Unsupported { metadata: ProjectMetadata, diagnostics: Vec<DiscoveryDiagnostic> }`. Only `Ready` contains collection descriptors. Missing content configuration is `Unsupported` with a specific diagnostic, not a successfully empty collection set. A verified literal export of an empty collection object may produce `Ready` with zero descriptors.

Add a crate-private `astrolith-gpui::project_discovery` coordinator. It owns the current project identity/generation, cancellation flag and retained runtime bridge task. Starting or replacing a request cancels the previous flag and drops its bridge. It submits `inspect_project` through the existing `runtime::blocking`, without exposing that runtime to `astrolith-astro`. At completion, the coordinator compares both project identity and generation before storing or publishing either success or failure. Tests exercise this actual bridge/coordinator path. No shell command or new UI is wired in M02.2; M02.3 will consume this coordinator.

### Compatibility and file selection

Record the declared requirement separately from the installed release. Resolve the regular package metadata at project-local `node_modules/astro/package.json`, allowing package-manager symlinks for dependency metadata without treating those paths as editable content. Do not execute Node package exports or search global installations. If the package is absent, its version is malformed, or its exact release has not passed the fixture compatibility matrix, return `Unsupported` with a version diagnostic. A requirement such as `^5` alone never establishes installed compatibility. Initial compatibility claims cover exact tested releases only; broadening ranges requires evidence.

Candidate Astro config names are `astro.config.js`, `astro.config.mjs` and `astro.config.ts`. Candidate modern content names are `content.config.js`, `content.config.mjs` and `content.config.ts` under resolved `srcDir`. Candidate legacy names are `content/config.js`, `content/config.mjs` and `content/config.ts`. Multiple candidates, modern plus legacy, or recognized alternate config extensions such as `.mts`, `.cts` and `.cjs` yield explicit unsupported/ambiguous diagnostics. They must not be mistaken for absent default configuration. Verify this deliberately narrow allowlist against pinned source before implementation; extend only with corresponding tests.

### Fixture mapping oracle

`astro sync` proves declarations load; it is not the source-mapping oracle. Add a fixture-only adapter around the pinned real Astro `glob` loader. In a separate oracle configuration, delegate its real lifecycle and wrap the loader context's `store.set` operation to record emitted collection name, logical ID and source `filePath`. Keep all original operations delegated, including parsing and schema validation. Verify this adapter against each installed pinned source before relying on it; do not emulate default ID generation inside the oracle.

Compare recorded paths with the expected file set selected by discovery descriptors. Include custom bases, overlapping collections, nested Markdown and MDX. Use a custom-ID variant to prove that IDs can differ from paths while production discovery explicitly rejects that variant. A slug-bearing source must not change its recorded source path. M02.3 will compare its actual `Entry` mappings against the same oracle. Adapter output is bounded test data, never an input trusted from a user project; all generated files live in temporary fixture copies. If a pinned version cannot provide reliable emitted mappings through this adapter, do not claim that version supported until another real-tool oracle is proven.
