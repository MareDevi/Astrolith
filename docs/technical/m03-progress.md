# M03 — schema-driven frontmatter progress

Started: 2026-10-30. Base: `dev` at `c985ca1`. Scope: M03.1 JSON Schema normalization and the R1 schema generation service. The original product specification remains absent; this note follows the implementation plan and its review and does not reconstruct that specification or approve later milestone gates.

## Ordering against the implementation review

The plan-review findings still govern M03 ordering and are not waived here:

- **R1** requires a schema-generation owner (project-local executable, working directory, `astro sync`, output discovery, diagnostics, cancellation, timeouts, freshness) before schemas are consumed in forms.
- **R3** makes the lossless document model and field-patch contract (M04.1) a prerequisite of editable frontmatter forms (M03.3).
- **R6** requires locked fixtures before the first Astro integration task; the Astro 5.18.2 and 7.4.4 fixtures already exist.

M03.1 is the pure JSON→model step. It does not execute project tooling, does not read the filesystem, and does not wire forms, so it is safe to land before the R1 generation service and before the R3 form gate. The R1 generation service is now implemented (below). Fixture schema authoring, M03.2 helper resolution, M03.3 forms and M03.4 validation were completed on 2026-10-31 (below); forms landed after the M04.2/M04.3 save coordinator, satisfying R3.

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

The generation service is now wired into the shell: `ProjectWorkspace::load_collection_schemas` runs `generate_collection_schemas` off-thread on project open and injects the normalized schemas into the frontmatter form (M03.3). As of `b676845`, `load_generated_schemas` also applies the M03.2 helper pass before returning.

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

## Fixture schema authoring (2026-10-31, `b647a88`)

Both `fixtures/astro5` and `fixtures/astro7` gained a schema-bearing `docs` collection (Zod) and every `content/docs` entry was extended so `astro sync` validation exits zero. The captured output from each real install is committed unchanged as the regression oracles `crates/astrolith-astro/src/schema/fixtures/astro5-docs.schema.json` and `astro7-docs.schema.json`. The schema exercises strings, an optional string, a coerced date with default, a boolean, a string array, an enum, a nested object, a nullable nested number, a heterogeneous union, a literal and a record; the two oracles normalize to the identical dialect-independent model asserted in `schema/tests.rs`. `fixtures/README.md` documents regeneration from the frozen lockfiles. Verification: `mise run test:fixtures` PASS, `mise run test:astro` PASS, `mise run check` exit 0.

## M03.2 Astro helper resolution (implemented, `3ac79d6`)

`crates/astrolith-astro/src/schema/helpers.rs` adds `enhance_astro_fields(&mut CollectionSchema, config_source) -> Vec<SchemaDiagnostic>`. It parses the content config with the vendored tree-sitter TypeScript grammar (never executes project code) and refines normalized fields:

- `image()` → `FieldKind::Image` (Astro emits a plain string, so the JSON Schema alone loses it).
- `reference("collection")` / `reference(["a","b"])` → `FieldKind::Reference { collections }` (Astro emits a heterogeneous union that alone normalizes to `Unsupported`).
- Handles nested `object()`/`array()`/`optional()`/`nullable()`/`default()` and other transparent wrappers, imported aliases, the `({ image }) => z.object(...)` schema-function form, and local `const` schemas; selects the matching collection by top-level field names.
- Malformed/unsupported configs become non-fatal diagnostics; a field is never silently string-coerced; diagnostics for resolved fields are cleared and the diagnostic list is sorted/deduplicated.

Twelve unit tests cover direct/nested/array helpers, nullable wrappers, alias imports, local const schemas, collection selection, malformed config, non-literal and missing reference arguments, no-helper configs, conflicting helpers across collections, and comma-separated reference collections. `mise run test:astro` (70 passed at this commit) and `mise run check` exit 0.

**P1 repair (`b676845`) — wired end-to-end.** `generation::load_generated_schemas` (called by `generate_collection_schemas`, which the shell's `load_collection_schemas` invokes on project open) now reads the project's content config source (honouring `srcDir` from `astro.config`) and applies `enhance_astro_fields` to every generated collection before returning. A real `image()` field therefore loads as `FieldKind::Image` and `reference("c")` as `FieldKind::Reference { collections }` before schemas reach the form. A missing or malformed config is non-fatal: normalized schemas are still returned and the problem becomes a diagnostic; fields never panic or string-coerce. The wiring is proved by `schema::generation::tests::generate_applies_astro_helper_resolution_end_to_end` (exercises the production `generate_collection_schemas` path and asserts `Image` + `Reference`), plus malformed- and missing-config non-fatal tests.

## M03.3 Schema-driven frontmatter forms (implemented, `9f37f89`)

`crates/astrolith-gpui/src/frontmatter/{fields.rs,mod.rs}` maps every normalized `FieldKind` to a native GPUI Kit control and edits only supported scalar leaves through the M04.1 `ParsedDocument::set_field` patch contract, so untouched frontmatter/body bytes are preserved exactly.

| FieldKind | Control | Editable |
|---|---|---|
| String{format} | text Editor (format hint) | yes → string |
| Number / Integer | text Editor | yes → float / integer |
| Boolean | Checkbox | yes → boolean |
| Date / Image / Reference | text Editor | yes → string |
| Enum{values} | one Button per allowed value | yes → typed enum |
| Literal{value} | read-only (fixed by schema) | no |
| Object{fields} | heading + nested child controls (dotted paths) | container no; scalar children yes |
| Array / Record | read-only with explicit refusal | no |
| Nullable{inner} | inner control + nullable flag | when inner scalar |
| Unsupported{reason} | read-only with explicit reason (never coerced to string) | no |

`FrontmatterEditor` re-parses after each edit so reads/validation see the new value and runs the M03.4 `validate_frontmatter` over readable scalar leaves (filtered to authoritative paths so unreadable arrays/records never yield a false "required"). `FrontmatterForm` renders with real `Editor`/`Checkbox`/`Button` controls. **P1 fix (`fddbd34`):** a form edit now emits a targeted `FrontmatterChanged { path, value }` (never a whole-document snapshot); the shell's `ProjectWorkspace::apply_frontmatter_field` reads the *live* session text, patches exactly that field through the lossless `set_field` contract, re-serializes and replaces the session text — so concurrent unsaved body edits are preserved byte-for-byte. The form's local snapshot is refreshed best-effort only for validation/selection and never written back. Saving still goes through the conflict-aware coordinator (no filesystem work on the UI thread). The regression test `frontmatter_form_field_edit_preserves_concurrent_body_edit` edits the body in the source editor, then a form field, and asserts the body survives, only the field changed, dirty is set, and `save_active` persists the combined result. Fifteen form tests + the shell wiring/regression test pass.

## M03.4 Frontmatter validation service (implemented, `1ae3e37`)

Two deliberately separate operations:

1. **Local** `validate_frontmatter(schema, &Value) -> Vec<ValidationDiagnostic>` (`schema/validation.rs`) is pure/fs-free/GUI-free. It checks required, string/email format, number/integer/boolean, date, enum, literal, nested objects, arrays, records, nullable, image and reference, and warns on unsupported fields. Diagnostics are structured `{path, message, severity, source}`, sorted and de-duplicated. Wired into the live form.
2. **Astro tooling** `validate_with_astro_tooling(request)` (`schema/validation_tooling.rs`) reuses the R1 `resolve_astro_entry` + shared `run_astro_tool` to run `astro sync`, maps stderr error blocks to revision-tagged diagnostics with project-relative locations, returns `Unavailable` for a missing install, and returns errors on timeout/cancellation. Stale results are rejected via `reject_if_stale`/`accept_for_revision`; `validate_with_astro_tooling_async` runs it on the blocking lane.

Documented limits: `astro sync` validates on-disk content (not unsaved buffers), reference existence is deferred to `getEntry()`, and `.astro` type checking needs `astro check`. Twenty-two tests cover every field kind, sorted/deterministic output, revision staleness, unavailable installs, timeout/cancellation, structured stderr mapping and the async blocking-lane wrapper. Date checking was additionally hardened (`b676845`) to reject impossible calendar dates (days-per-month + Gregorian leap years) while accepting valid leap days, covered by `dates_reject_impossible_calendar_dates_but_accept_leap_days`. **The Astro-tooling operation is not yet surfaced in the UI** (local validation is).

## M03 verification (2026-10-31, final after P1 repairs)

`mise run check` result at the final commit (`b676845`, on top of `fddbd34`): **PASS** — doctor, formatting, warnings-denied Clippy, locked build, workspace tests and the frozen Astro fixture task all succeeded. **260 Rust tests passed, 0 failed, 4 intentionally ignored**, and **Astro fixture compatibility: PASS**. Crate-local counts: `astrolith-astro` 96 passed / 1 ignored (adds 12 helper, 22 validation, 4 helper-wiring and the calendar-date test), `astrolith-gpui` 51 passed / 2 ignored (15 frontmatter tests incl. the concurrent-body-edit regression, plus lifecycle/wiring), `astrolith-document` 50, plus core/workspace/desktop. Focused suites at the final commit: `mise run test:astro` = 96 passed / 1 ignored (plus 9 discovery), `mise run test:gpui` = 51 passed / 2 ignored.

## Independent review (2026-10-31)

An independent read-only review of the changed crates found **no P0**. Findings:

- **[P1 — RESOLVED in `fddbd34`] Frontmatter form could overwrite unsaved source-editor edits.** Originally `FrontmatterForm::set_document` loaded a whole-document snapshot only when the active document/schema key `(path, has_schema)` changed (`frontmatter/mod.rs`, `shell.rs`), and `apply_value` re-serialized that *stale* snapshot; `apply_frontmatter_text` (`project_discovery.rs`) then replaced the whole editor text, silently discarding an in-memory body edit. **Fix:** `FrontmatterChanged` now carries only `{ path, value }`; `ProjectWorkspace::apply_frontmatter_field` patches the *live* editor text via `set_field` and re-serializes, preserving concurrent unsaved body edits. Regression test `frontmatter_form_field_edit_preserves_concurrent_body_edit` verified by this review.
- **[P1 — RESOLVED in `b676845`] M03.2 helper resolution was unwired.** Originally `enhance_astro_fields` (`helpers.rs`) had no caller in `generation.rs`/`project_discovery.rs`, so the live form did not refine `image()`/`reference()` fields. **Fix:** `load_generated_schemas` reads the content config and applies the helper pass per generated collection; `generate_applies_astro_helper_resolution_end_to_end` proves `Image`/`Reference` reach `generate_collection_schemas`. Verified by this review.
- **[P2 — RESOLVED in `b676845`] `looks_like_date` accepted impossible calendar dates** (e.g. `2024-02-31`). Now bounds days per month and honours Gregorian leap years; `dates_reject_impossible_calendar_dates_but_accept_leap_days` covers both acceptance and rejection.
- **[P2 — OPEN] `select_collections` heuristic can merge helpers from unrelated collections** when no collection's top-level field-name set matches the schema (`helpers.rs:193`); conflicts surface a diagnostic, but a same-named field could be refined by the wrong collection. Prefer matching by the generated collection id.
- **[P2 — OPEN, reduced] Whole-document round-trip per form keystroke.** After `fddbd34` each form keystroke still parses the *live* full document, patches one field and re-serializes (no longer any stale-snapshot overwrite, but heavier than necessary); a debounce or span-targeted patch would reduce churn.
- **[P2 — OPEN] Astro tooling validation is not surfaced in the UI**, and `load_collection_schemas` runs `astro sync` automatically on project open (`project_discovery.rs`), which can add a visible delay and writes `.astro/collections`; it is cancellable and non-fatal, which is the required posture.
- **[P2 — OPEN, minor] Form display can be stale** relative to manual frontmatter edits made in the source editor (the local snapshot is refreshed only on document/schema key change); editing a field still patches correctly from the live text, so this is a display-only nit, not data loss.

None of these change the "unsupported types are never silently serialized as strings" behaviour, which remains enforced and tested.

## Repair verification (2026-10-31)

This review re-verified the two P1 repairs rather than trusting the repair reports:

- **`b676845` (helper wiring):** confirmed the call chain `start()` → `load_collection_schemas` → `generate_collection_schemas` → `load_generated_schemas` → `apply_astro_helpers` → `enhance_astro_fields`; ran `generate_applies_astro_helper_resolution_end_to_end` → `ok` (asserts `FieldKind::Image` and `FieldKind::Reference { collections: ["authors"] }`), plus malformed/missing-config non-fatal tests.
- **`fddbd34` (form data loss):** confirmed `apply_frontmatter_field` reads live text and patches one field; `apply_frontmatter_text` is gone; ran `frontmatter_form_field_edit_preserves_concurrent_body_edit` → `ok` (body edit survives, only field changes, combined result saves).
- Focused suites at the final commit: `test:astro` 96 passed / 1 ignored (+9 discovery), `test:gpui` 51 passed / 2 ignored. Full `mise run check` PASS (260 passed, 0 failed, 4 ignored; Astro fixture PASS).
- No regression found. **Native-window acceptance remains a manual human step and is still PENDING** (see `docs/technical/native-acceptance-save-recovery.md`).

## Remaining M03 scope

1. **R1 schema generation service — done** (`src/schema/generation.rs`).
2. **Fixture schema authoring — done** (`b647a88`).
3. **M03.2 Astro helper resolution — done and WIRED** (`3ac79d6` + `b676845`); `image()`/`reference()` refine end-to-end.
4. **M03.3 forms — implemented and headless-tested** (`9f37f89`); P1 overwrite risk **fixed** (`fddbd34`). Native acceptance outstanding.
5. **M03.4 validation — implemented** (`1ae3e37` + date hardening `b676845`); local wired, Astro-tooling operation not yet surfaced in the UI (P2).
6. **Open P2s:** helper collection-selection heuristic; per-keystroke whole-document re-parse; Astro-tooling validation not in UI; stale form display after manual frontmatter edits.
7. **Native form acceptance** (IME, focus, scaling, rendering) — **PENDING human execution**; see `docs/technical/native-acceptance-save-recovery.md`.

