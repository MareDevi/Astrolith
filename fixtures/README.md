# Astro compatibility fixtures

`mise run test:fixtures` installs committed frozen Bun locks into temporary copies and runs each project's actual Astro sync. A fixture-only wrapper delegates to the real `glob()` loader and records `store.set` IDs and source paths. The task then runs the production Rust discovery/index helper against that same installation and compares complete mappings. It also invokes the headless GPUI workflow on every installed project, checking picker selection, keyboard edits, MDX source, retained tabs and unchanged disk contents.

Verified pairs are Astro 5.18.2 / MDX 4.3.14, Astro 6.4.8 / MDX 6.0.3 and Astro 7.4.4 / MDX 8.1.2. The regular `workspace` config is the supported static subset. The separate `oracle` config includes a custom-ID collection to demonstrate that logical IDs cannot be used to reconstruct source paths; production rejects that option. `unsupported/legacy` is diagnostic-only.

## Pattern arrays and negative globs

The Astro 6 fixture exercises the `pattern: string | string[]` form introduced by its `glob()` loader typing. `publishedDocs` excludes `**/drafts/**`, `selectedDocs` excludes a directory and a root file, and `negativeFirstDocs` declares the exclusion before its include to prove that negative entries always win regardless of ordering. `content/docs/drafts/wip.md` is the excluded source; the committed oracle events prove that `docs` and `archive` still select it while the negative-pattern collections do not. Production accepts only the fixture-verified Markdown include patterns plus a bounded negative-glob subset (`**`, `*`, `?`, character classes and brace alternatives); anything else makes discovery report the project as unsupported instead of guessing.

## Schema-bearing collections

The `docs` collection in each `workspace/content.config.mjs` declares a Zod schema so `bun run sync` writes `.astro/collections/docs.schema.json`. The captured output is committed unchanged as the regression oracle under `crates/astrolith-astro/src/schema/fixtures/`:

- `astro5-docs.schema.json` — Astro 5.18.2, JSON Schema draft-07 (`$ref`/`definitions`, `z.coerce.date()` as an `anyOf`, `z.record(value)` without `propertyNames`).
- `astro6-docs.schema.json` — Astro 6.4.8, JSON Schema draft 2020-12 (inline object, `const` literals, `z.record(key, value)` with `propertyNames`); byte-identical to the Astro 7 oracle.
- `astro7-docs.schema.json` — Astro 7.4.4, JSON Schema draft 2020-12 (inline object, `const` literals, `z.record(key, value)` with `propertyNames`).

Every sample entry under `content/docs` satisfies the schema, so the full server-validation step exits zero and the whole fixture pipeline keeps passing. The schema deliberately covers a variety of field types across all fixtures: strings, an optional string, a coerced date with a default, a boolean with a default, a string array with a default, an enum, a nested object, a nullable nested number, a heterogeneous union (string|number), a literal and a record. The heterogeneous union is intentional: the normalizer must report it as unsupported rather than silently coerce it, so all oracles normalize to the identical dialect-independent model asserted by `crates/astrolith-astro/src/schema/tests.rs`.

To regenerate an oracle, copy the fixture into a temporary directory, run `bun install --frozen-lockfile` and `bun run sync -- --force` with the project-local Astro CLI, then copy `.astro/collections/docs.schema.json` over the matching committed file. The committed fixture and its `bun.lock` are the source of truth; no registry drift is allowed.

Every subprocess has a 60-second deadline and a 1 MiB output cap. Generated oracle JSON and a Markdown report, including installed source hashes, go to the OS temporary directory. `mise run test:fixtures -- --keep-temp` retains the temporary projects for inspection. Normal runs remove them. No selected user project is executed by discovery.
