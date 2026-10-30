# M03 — schema-driven frontmatter progress

Started: 2026-10-30. Base: `dev` at `c985ca1`. Scope: M03.1 JSON Schema normalization only. The original product specification remains absent; this note follows the implementation plan and its review and does not reconstruct that specification or approve later milestone gates.

## Ordering against the implementation review

The plan-review findings still govern M03 ordering and are not waived here:

- **R1** requires a schema-generation owner (project-local executable, working directory, `astro sync`, output discovery, diagnostics, cancellation, timeouts, freshness) before schemas are consumed in forms.
- **R3** makes the lossless document model and field-patch contract (M04.1) a prerequisite of editable frontmatter forms (M03.3).
- **R6** requires locked fixtures before the first Astro integration task; the Astro 5.18.2 and 7.4.4 fixtures already exist.

M03.1 is the pure JSON→model step. It does not execute project tooling, does not read the filesystem, and does not wire forms, so it is safe to land before the R1 generation service and before the R3 form gate. Generation, freshness and fixture schema authoring remain outstanding M03 work; forms remain blocked on M04.

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

Nine behavioral tests cover both real dialect fixtures, the shared dialect-independent model, scalar/object/container fields, nested objects, nullable scalars, heterogeneous unions, `anyOf` dates, arrays and defaults, empty schemas, and structural errors. The two fixtures differ in raw bytes but normalize to identical `fields` and `diagnostics`.

`mise run check` result: **PASS** — doctor, formatting, warnings-denied Clippy, locked build, workspace tests and the frozen Astro fixture task all succeeded (**139 Rust tests passed, 0 failed, 3 intentionally ignored**; the 9 schema tests are new, up from 130) and **Astro fixture compatibility: PASS**. Note that the same schema tests initially passed under `cargo test -p astrolith-astro` but failed under `cargo test --workspace` until fields were sorted (the `preserve_order` feature unification described above).

## Remaining M03 scope (not done here)

1. **R1 schema generation service:** resolve the project-local `astro` binary, choose the working directory, run `astro sync` within a deadline, discover `.astro/collections/*.schema.json`, report diagnostics, support cancellation, and model freshness/invalidation when `content.config` changes. This is required before forms consume schemas.
2. **Fixture schema authoring:** add schema-bearing collections whose existing entries satisfy the schema so the whole M02 fixture pipeline keeps passing, and capture their generated schema as regression oracles.
3. **M03.2 Astro helper resolution** (`image()`, `reference()`, nested/array fields) on top of the normalized model.
4. **M03.3 forms** — blocked on M04.1's lossless document model and field-patch contract (R3); no persistent field editing before that gate.
5. **M03.4 validation** — local JSON-schema checks and Astro tooling validation as separate operations.
