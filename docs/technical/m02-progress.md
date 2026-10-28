# M02 — domain and editor progress

Date: 2026-10-28.

## M02.1 — domain IDs and models

Scope: the next implementation task, M02.1, requested by the user after M01. The original design specification remains absent. This task follows the existing plan and its review; this note does not reconstruct that specification or approve later milestone gates.

Added dependency-free `astrolith-core` with project, collection and entry modules. Project and collection IDs preserve exact case-sensitive strings. Blank identities and control characters fail construction. Entry identity includes its collection and retains custom loader IDs without path or slug normalization. IDs are local to a project: cross-project registries must additionally key by `ProjectId`. Assigning persistent project IDs remains the workspace service's responsibility.

The plan's illustrative public string fields are private validated fields with read accessors. `CollectionId` makes collection identity explicit. This prevents callers from bypassing validation without adding persistence, discovery, or GUI dependencies.

Review R2: `Entry` carries a logical `EntryId` and an independent `SourcePath`. A custom ID such as a published slug need not resemble its actual `.md` or `.mdx` file. Overlapping collections can refer to one source file without collapsing their logical identities. No API reconstructs a source filename from an entry ID.

`SourcePath` validates relative UTF-8 paths lexically, with `/` separators on every platform. It rejects absolute paths, all colons (drive/alternate-stream syntax, including otherwise legal POSIX names), backslashes, empty/dot/parent segments, controls and segments ending in dots or spaces (Windows aliases). It preserves accepted Unicode, interior spaces and case. This is not a filesystem security boundary: discovery/indexing and eventual I/O must enforce project-root containment, symlink policy and target validity at access time.

Verification: the new behavioral tests first failed because the domain API did not exist. A subsequent Windows alias regression test failed on `.. /outside.md` before the path validator was corrected. Final `mise run check` passed doctor, formatting, Clippy, locked build and the full headless suite: 39 passed, zero failures, one intentionally ignored editor performance workload. The sandbox prevented doctor's Node process from spawning `rustc` (`EPERM`); the approved check outside the sandbox passed.

Independent review found a formatting failure in the test while it was being edited; the final formatted tree passed `mise run fmt` and the complete check. Review also prompted explicit documentation of the colon restriction. The suggestion to trim IDs was declined: nonblank IDs are opaque and must preserve loader-provided whitespace rather than silently changing identity. No correctness blockers remain after review. No native-window smoke is required for this pure domain addition; no GUI behavior changed. Native M01 acceptance remains as recorded separately.

## Next task

M02.2 — project discovery. Before implementing it, define the supported local loader/source-mapping subset and introduce exact-version locked Astro fixtures as required by review R2/R6. Do not infer collection roots from logical IDs. M02's real-project editing gate remains incomplete.
