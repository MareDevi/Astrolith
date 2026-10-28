# Astrolith implementation plan v1.0 — review

Date: 2026-10-28  
Disposition: Revise before approving the complete implementation plan.  
Scope: Plan review only; implementation has not started.

Reviewed plan: [Implementation plan](2026-10-28-astrolith-implementation.md).

## Prerequisite and evidence

The referenced `docs/superpowers/specs/2026-10-28-astrolith-design.md` is absent. The workspace initially contained only `mise.toml`, with `rust = "latest"`, and was not a Git repository. Consequently, this review checks the plan's internal consistency and documented framework contracts, rather than conformance to the missing specification.

Context7 was used for GPUI Kit and Astro documentation. Astro's official documentation was also inspected directly. GPUI Kit results referenced current main-branch documentation, which does not establish compatibility with the proposed 0.8.1 release. No toolchain installation, compilation, native GUI testing, or Astro fixture execution was performed.

## Findings

### R1 — [P1] Add an owner for schema generation before consuming generated schemas

**Location:** M03.1 and M03.4, plan lines 398–441.

M03.1 starts with an existing JSON value. No preceding task produces fresh schemas from an opened project, handles absent generated files, or invalidates them when configuration changes. A clean project can therefore complete discovery and indexing without providing the forms with any schema. M03.4 also leaves the concrete Astro validation operation unspecified, making its scope difficult to verify.

**Change:** Add an Astro tooling task before schema normalization. Define project-local executable resolution, working directory, generation command, output discovery, diagnostics, cancellation, timeouts, and schema freshness. Define what Astro-side validation checks beyond local JSON Schema checks, and how results attach to a document revision. Do not assume that one CLI operation proves all content validation requirements.

**Acceptance:** Start from an exact-version fixture without `.astro`; generate and consume its schemas. Change its collection schema and verify refreshed controls. Test missing dependencies, tool failure, stale output, and obsolete validation results.

**Evidence:** Astro documents generated schemas under `.astro/collections/` and a separate `astro sync` command. These establish integration points, but do not prove the full proposed validation contract: [collection schemas](https://docs.astro.build/en/guides/content-collections/#using-json-schema-files-in-your-editor), [CLI reference](https://docs.astro.build/en/reference/cli-reference/#astro-sync).

### R2 — [P1] Define the mapping from collection entries to editable source files

**Location:** M02.1–M02.3, plan lines 323–373.

`EntryId` expresses logical identity, but the plan never establishes how discovery produces authoritative collection roots, loader patterns, and entry-to-file mappings. Assuming `src/content/<collection>` or reconstructing a filename from an entry ID would miss valid projects or open the wrong file. The existing tests do not name these cases.

**Change:** Specify editable loader support and carry logical entry identity separately from source-file identity. Support a documented subset of local loaders; diagnose configurations outside that subset instead of guessing. Define project scope and symlink handling for content outside the project root.

**Acceptance:** Test a `glob()` collection with a custom base, a custom `generateId`, an overridden slug, overlapping collections, and an unsupported loader. Confirm that opening an entry selects the correct source even when its ID differs from its path.

**Evidence:** Astro loaders can use custom directories and IDs; collection identity alone does not identify an editable path: [Content Loader API](https://docs.astro.build/en/reference/content-loader-reference/).

### R3 — [P1] Establish lossless metadata edits before wiring editable forms

**Location:** M03.3 and M04.1, plan lines 431–470.

M03 enables form editing before M04 introduces the document parser and serializer. M04.1's unchanged round-trip test can pass by returning retained source verbatim, without proving that changing one field preserves anything else. A serializer can pass that test and still remove comments, rewrite unrelated scalars, or alter an MDX body after an actual edit.

**Change:** Make the document model and field-patch contract prerequisites of M03.3. Keep forms non-persistent until the save coordinator passes its gate. Require preservation of untouched source regions during supported field edits, and explicit refusal of unsupported patches.

**Acceptance:** Change one nested field in documents containing comments, quoted values, multiline scalars, unknown keys, Unicode, and CRLF. Assert that unrelated regions and body bytes are unchanged. Exercise ambiguous YAML constructs and malformed frontmatter without destructive fallback serialization.

### R4 — [P1] Cover the complete dirty-document lifecycle and save completion contract

**Location:** M02.4, M04.2–M04.3, plan lines 375–388 and 472–498.

Dirty tracking and external modification are mentioned, but no task owns unsaved tab closure, project switching, application exit, or watcher-driven reload decisions. Recovery alone cannot define whether these operations may discard a buffer. The save API also leaves its baseline disk version and the effect of a late save completion implicit.

**Change:** Assign these transitions to a document-session coordinator. Define the snapshot's document identity, revision, expected disk baseline, and save serialization policy. Mark a document clean only when its current content matches the successfully persisted snapshot. Specify which concurrent external-write guarantees the platform adapter can provide, including any remaining check-to-replace race; atomic replacement alone is not a conflict check.

**Acceptance:** Edit A, switch to B, and return to A with its buffer intact. Test close/project-switch/exit choices. Save revision N while editing N+1; N+1 must remain dirty. Complete an old save after switching documents without changing the new document's state. Test external change during a dirty edit and during the save sequence, plus recovery cleanup when newer unsaved content exists.

### R5 — [P2] Make MDX a tested deliverable or narrow the stated goal

**Location:** Goal and M00.2/M02/M04 acceptance criteria, plan lines 11, 212–230, 319–390, and 451–470.

The goal promises Markdown/MDX editing, but the editor spike and workspace gate explicitly verify Markdown. There is no MDX-specific acceptance test. Markdown highlighting and round trips do not establish correct behavior around JSX, expressions, or imports.

**Change:** Add MDX fixtures to discovery, editor, document-patching, and preview tests. Define whether Alpha promises source preservation only or also MDX-aware highlighting. If MDX is deferred, state that in the goal and compatibility limits.

**Acceptance:** Open and edit a `.mdx` entry with frontmatter, imports, JSX, and expressions. Save a metadata edit without changing its body, then verify the configured Astro preview renders it.

### R6 — [P2] Schedule fixture creation and define executable milestone gates

**Location:** Sections 2–3, M03.1, M07.1, and execution protocol, plan lines 66–186, 413, 602–610, and 660–667.

The plan requires Astro 5/6/7 fixtures and lockfiles, but no task creates them before M03.1 consumes their schemas. Every milestone requires `mise run check`, yet its introduction is deferred until unspecified subsystems exist. M07's P0 gate also refers to a test classification that the plan never defines.

**Change:** Add exact-version fixture setup before the first Astro integration task. Introduce `check` during M00.1 for existing checks and extend it as subsystems appear. Define which tests block each milestone and Alpha; distinguish unit, integration, and manual platform evidence.

**Acceptance:** A clean checkout runs the current milestone's checks through Mise. Fixture installation uses committed lockfiles and fails on drift. Each release-blocking test has a named owner and command or manual procedure.

### R7 — [P2] Turn feasibility observations into reproducible go/no-go criteria

**Location:** M00.2–M00.3, plan lines 212–253.

"Large-document responsiveness," IME composition, and fractional scaling are sensible risks, but the plan does not identify document sizes, measured interactions, acceptable latency, or the manual test environment. Two workers could report opposite gate results using different workloads. A headless input test also cannot substitute for actual compositor/IME evidence.

**Change:** Record the exact GPUI Kit release or revision, Cargo features, toolchain, platform, compositor, IME, scale settings, and representative document workloads. Set measurable performance thresholds before accepting the spike. Label untested cases explicitly and keep M01 blocked until critical failures have an agreed resolution.

**Acceptance:** Spike reports distinguish documented APIs, automated results, manual results, and unverified behavior. Another developer can repeat the procedures and evaluate the same gate.

## Recommended dependency correction

1. Recover the design specification, or explicitly approve the plan as the authority for M00 only.
2. Run M00 feasibility work and record its gate evidence.
3. Establish document-session and lossless-patch contracts before editable metadata forms; establish conflict-aware saving before persistent form edits.
4. Create locked Astro fixtures, implement source mapping, and implement schema generation before consuming schemas in forms.
5. Continue with preview and release work after the corrected safety and schema gates pass.

The native GPUI shell, independent services, Mise toolchain policy, and external-preview fallback remain viable directions. This review does not establish runtime compatibility or platform acceptance.
