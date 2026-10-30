# M03 — schema-driven frontmatter progress

Started: 2026-10-30. Base: `dev` at `c985ca1`. Scope: M03.1 JSON Schema normalization and the R1 schema generation service. The original product specification remains absent; this note follows the implementation plan and its review and does not reconstruct that specification or approve later milestone gates.

## Ordering against the implementation review

The plan-review findings still govern M03 ordering and are not waived here:

- **R1** requires a schema-generation owner (project-local executable, working directory, `astro sync`, output discovery, diagnostics, cancellation, timeouts, freshness) before schemas are consumed in forms.
- **R3** makes the lossless document model and field-patch contract (M04.1) a prerequisite of editable frontmatter forms (M03.3).
- **R6** requires locked fixtures before the first Astro integration task; the Astro 5.18.2 and 7.4.4 fixtures already exist.

M03.1 is the pure JSON→model step. It does not execute project tooling, does not read the filesystem, and does not wire forms, so it is safe to land before the R1 generation service and before the R3 form gate. The R1 generation service is now implemented (below). Fixture schema authoring, M03.2 helper resolution, forms and validation remain outstanding; forms remain blocked on M04.

## Verified generation contract (input to M03.1)

The generated schema is produced by each fixture's own `astro sync` script (`bun run sync -- --force`, i.e. `astro sync`) and written to `.astro/collections/<collection>.schema.json`. This was confirmed by running the real fixture installs in temporary copies. Two important observations:

1. **`astro sync` writes the schema file before content validation runs**, so the schema is available even when the sample entries do not yet satisfy a newly added schema. A generation service must not treat a non-zero `astro sync` exit as "no schema produced".
2. **The two supported Astro releases emit different JSON Schema shapes for the same Zod source:**
   - **Astro 5.18.2 — draft-07:** a top-level `$ref: "#/definitions/<name>"` wrapper with a `definitions` map; `z.coerce.date()` becomes an `anyOf` of `string/date-time`, `string/date` and `integer/unix-time`; nested objects carry `additionalProperties: false`; email is `format: "email"`.
   - **Astro 7.4.4 — draft 2020-12:** the object schema is inline at the top level; dates are a single `string`/`date-time`; `z.literal` becomes `const`; `z.record` uses `propertyNames` + `additionalProperties`; email carries both `format` and `pattern`.

Both releases inject a synthetic `$schema` *property* into the object's `properties` (an editor convenience), which is not a user field and is excluded from the model.

The committed fixtures `crates/astrolith-astro/src/schema/fixtures/astro5-docs.schema.json` and `astro7-docs.schema.json` are the exact generated output captured from those runs; the normalizer is tested against both.

## Implemented contract (M03.1)

`crates/astrolith-astro/src/schema/` adds a filesystem-free, GUI-free module:

- `normalize_schema(&serde_json::Value) -> Result<CollectionSchema, SchemaError>` accepts either dialect and returns one dialect-independent tree (`SchemaDialect::{Draft7, Draft202012, Unknown}`).
- `FieldKind` covers `String { format }`, `Number`, `Integer`, `Boolean`, `Date`, `Enum`, `Literal`, `Array`, `Object`, `Record`, `Nullable`, and `Unsupported { reason }`. Each `Field` records `required` and any literal `default`.
- Dates are normalized to `Date` whether encoded as draft-07 `anyOf` or as a 2020-12 `string`/`date-time`.
- Nullable scalars (`type: ["number","null"]` or an `anyOf` with a single `null` branch) become `Nullable`.
- Heterogeneous unions (e.g. `type: ["string","number"]`) become `FieldKind::Unsupported` with a `SchemaDiagnostic` at the field path; they are never silently coerced to a string. Non-fatal problems surface as diagnostics so the rest of the schema stays editable; only structurally invalid documents (not an object, non-object root, or unresolved root `$ref`) are `SchemaError`s.

Field order is deterministic: fields are sorted by name, because `serde_json`'s map ordering depends on whether the `preserve_order` feature is unified in by another workspace crate (the same test failed under `cargo test -p` and passed under `cargo test --workspace` until fields were sorted). Alphabetical order is acceptable for the model but is a UI-ordering limitation to revisit when forms are built.

## Verification

Sixteen behavioral tests cover both real dialect fixtures, the shared dialect-independent model, scalar/object/container fields, nested objects, nullable scalars, heterogeneous unions, `anyOf` dates (including nullable dates), arrays and defaults, empty schemas, structural errors, `$defs` references, `$ref` cycles, open objects, tuple arrays, null-only fields, nested unresolved references, deterministic multi-diagnostic ordering, and depth bounding. The two fixtures differ in raw bytes but normalize to identical `fields` and `diagnostics`.

## Independent review and resolutions

An independent review verified the tests and Clippy and found no model/API or silent-coercion problems, with two [P1] issues and several [P2] nits. Both P1s were fixed:

1. **[P1] Reference-cycle / depth stack overflow.** A crafted `.schema.json` with a self-referential `$ref` (or programmatic deeply nested `Value`) could recurse without bound and overflow the stack. The traversal now carries a depth counter capped at 64 and an active-reference stack that reports a `cyclic schema reference` diagnostic, so normalization stays bounded.
2. **[P1] Non-deterministic diagnostic order.** Diagnostics were pushed in `properties` iteration order while only `fields` were sorted, so `CollectionSchema` equality varied with `preserve_order` feature unification once there were two or more unsupported fields. Diagnostics are now sorted by `(path, message)`.

P2 fixes applied: `$defs` (2020-12) references are resolved alongside `#/definitions/`; a nullable date flattened into the Astro 5 `anyOf` plus a `null` branch normalizes to `Nullable<Date>` instead of `Unsupported`; `type: ["null"]`, standalone `integer`/`unix-time`, tuple arrays, non-object `items` and array-item diagnostic paths are classified/labelled correctly. Residual: Astro 6 still has no fixture (honestly disclosed above; the "5/6/7" matrix is only satisfied for 5 and 7).


`mise run check` result: **PASS** — doctor, formatting, warnings-denied Clippy, locked build, workspace tests and the frozen Astro fixture task all succeeded (**158 Rust tests passed, 0 failed, 4 intentionally ignored**; 28 schema/generation tests are new, up from 130) and **Astro fixture compatibility: PASS**. The same schema tests initially passed under `cargo test -p astrolith-astro` but failed under `cargo test --workspace` until fields were sorted (the `preserve_order` feature unification described above); diagnostics were sorted for the same reason.

## R1 schema generation service (implemented)

`crates/astrolith-astro/src/schema/generation.rs` adds the R1 tooling owner:

- **Executable resolution:** reads `node_modules/astro/package.json`, resolves its `bin.astro` entry, canonicalizes it, and rejects an entry that escapes the project or is not a file. A missing installation is a clear error.
- **Generation command:** runs `node <astro-entry> sync` with the project root as working directory. Node defaults to `node` on `PATH`; `ASTROLITH_NODE` can be supplied through the request builder for non-PATH environments. Node must therefore be available to the app (a documented requirement, not yet packaged).
- **Bounds:** 30 s default deadline (overridable), the shared `Cancellation` polled during the run, a 1 MiB capture cap per output stream (drained continuously to avoid pipe deadlock), and a 1 MiB cap per schema file. Timeout/cancellation kill the direct child. Full process-tree cleanup (e.g. esbuild grandchildren) is the M05 process-lifecycle responsibility, not claimed here.
- **Output discovery:** lists `.astro/collections/*.schema.json`, reads each bounded file, parses it, and normalizes it via M03.1. A bad file becomes a per-collection diagnostic without discarding the others; results are sorted by collection.
- **Non-zero exit:** tolerated when schema files were still written (Astro writes schemas before content validation) and recorded as a diagnostic; a run that produces no schemas is an error.
- **Freshness:** `schemas_are_stale(root, content_config)` reports stale when `.astro/collections` is missing/empty or its newest schema predates `content.config`.

It is not yet wired into the GPUI shell or forms; that belongs with M03.3 after the M04 lossless-edit gate.

### Generation verification

- Twelve generation unit tests cover resolution, generation+normalization, non-zero exit with schemas, a malformed schema alongside a valid one, missing schemas, timeout, cancellation, freshness (older/newer/missing/any-stale), entry containment (bin without `astro`, bin escaping the project), and a missing installation. Spawn-based tests are serialized to avoid the Linux `ETXTBSY` fork race.
- An opt-in `installed_project_generates_schemas_end_to_end` test was run against freshly `bun install`-ed Astro **5.18.2** and **7.4.4** fixtures (with a `docs` schema added) under Node 26.12.1: both resolved the installed CLI, ran `astro sync`, discovered `.astro/collections/docs.schema.json`, and normalized the `title` field. These fixtures live in a temporary copy, so the committed fixtures and lockfiles are unchanged.

### Generation review and resolutions

Independent review found no P0 and one [P1] plus several [P2]s, all addressed:

- **[P1] Unbounded drain could defeat the timeout.** Captured output previously used reader threads joined without a bound; a surviving grandchild holding the pipe could block the join past the deadline. Output is now captured to an anonymous temp file (stdout discarded) and read with a hard 1 MiB cap, so the deadline is a hard bound and no reader threads leak on timeout/cancellation.
- **[P2] Zombie on wait error.** The `try_wait` error branch now reaps the child.
- **[P2] Freshness only checked the newest schema.** `schemas_are_stale` now reports stale if *any* schema predates the config (partial-sync case) and treats a missing config as stale.
- **[P2] Containment untested.** Added negative tests for a `bin` object without `astro` and a `bin` that escapes the project root.
- **[P2] Unbounded schema count.** Discovery now refuses more than 512 collection schema files.
- Limits kept/documented: leftover `.schema.json` files from a removed collection are still reported (not cleared) — obsolete but not silently lost; Windows process-spawn tests are not run locally (Node-on-PATH is required and documented).

## Remaining M03 scope (not done here)

1. **R1 schema generation service — done** (`src/schema/generation.rs`; see above). Wiring it into the shell/forms remains M03.3.
2. **Fixture schema authoring:** add schema-bearing collections whose existing entries satisfy the schema so the whole M02 fixture pipeline keeps passing, and capture their generated schema as regression oracles.
3. **M03.2 Astro helper resolution** (`image()`, `reference()`, nested/array fields) on top of the normalized model.
4. **M03.3 forms** — blocked on M04.1's lossless document model and field-patch contract (R3); no persistent field editing before that gate.
5. **M03.4 validation** — local JSON-schema checks and Astro tooling validation as separate operations.
