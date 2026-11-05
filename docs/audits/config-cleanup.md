# Configuration cleanup audit

Audit date: 2026-11-05, `dev` @ `326eeec`. Scope: everything Astrolith persists about
itself (not user documents). Full inventory and control-flow proofs:
`evidence/config-migration.md`.

**This document proposes deletion; nothing has been deleted.** `AGENTS.md` and the audit
brief both require approval before production changes.

## 1. Current configuration architecture (as implemented)

| Artifact | Path | Version constant |
|---|---|---|
| Global scope | `<config>/astrolith/preferences/global.json` | `GLOBAL_PREFERENCES_VERSION = 2` |
| Project registry | `<config>/astrolith/preferences/projects.json` | `PROJECT_REGISTRY_VERSION = 1` |
| Project scope | `<config>/astrolith/preferences/projects/<id>.json` | `PROJECT_PREFERENCES_VERSION = 1` |
| Collection scope | **not a file** — `collections` map inside the project file | — |
| Legacy v1 | `<config>/astrolith/preferences.json` | `1` |
| Recovery records | `<config>/astrolith/recovery/<fnv16>.json` | none (strict parse) |
| Save-failure artifacts | `<config>/astrolith/save-failures/*.json` | none (content-free) |

`<config>` = `%APPDATA%` / `~/Library/Application Support` / `$XDG_CONFIG_HOME|~/.config`
(`settings/store.rs:864-877`). `ASTROLITH_SETTINGS_PATH` overrides the global path only.

### 1.1 Properties that must be preserved

1. **Unknown top-level keys survive a write.** `Stored::save` merges the typed object
   over the raw envelope (`store.rs:150-156`). This is what makes deleting migration code
   non-destructive, and it must not be "simplified" away.
2. **Future versions are read-only.** A numeric `version != 2` yields defaults plus
   `writable = false` and a warning (`store.rs:82-91`); no write can clobber a newer build.
3. **Corruption is quarantined, never overwritten** (`*.corrupt`, suffix `-2`…,
   `store.rs:825-862`).
4. **Writes are atomic** (temp + `sync_all` + rename) and **revision-monotonic**
   (`store.rs:141-143, 812-823`); an out-of-process edit is detected by baseline
   comparison and refused (`store.rs:135-148`).
5. **Explicit `false`/`0`/`null` inheritance semantics** in the project/collection scopes
   (`Option<T>` = inherit, `Some(false)` = explicit).

### 1.2 Defects found while reading the schema

| ID | Finding | Evidence | Level |
|---|---|---|---|
| CFG-01 | **`version` no longer identifies a schema.** Every post-2.0 change (`layouts`, `page_width_percent`, `editor_line_height`, `interface_scale`) landed inside `version: 2`, and migrations trigger on *key presence*, not the version. A user deleting a key by hand re-runs a migration. | `core/preferences.rs:14-18`; `store.rs:191,245,304,381,447,512` | Strong |
| CFG-02 | **Six `!writable` guards inside migrations are unreachable**: `load` only sets `writable = false` on paths that return before the envelope is populated, while every migration starts with `envelope.as_mut()?`. | `store.rs:57,84,113` vs `:194,250,307,390,450,515` | Confirmed (control flow) |
| CFG-03 | **Backups are incremental, not "the exact previous file"** as the comments state, and `Reset All` deletes `global.json` but leaves every `*.pre-*.json` and `*.corrupt` behind. | `store.rs:201-222,241-242,457-462`; `advanced.rs:57-69` | Strong |
| CFG-04 | **`write_atomic` does not fsync the parent directory**, so a rename can be lost on power failure. | `store.rs:812-823` | Strong |
| CFG-05 | **A >1 MiB preferences file is a read error, not a corruption quarantine**, so the app stays read-only with no in-app recovery. | `store.rs:27,53-63,799-805` | Strong |
| CFG-06 | **Downgrade → upgrade silently discards a re-created v1 file**: if `global.json` exists, `preferences.json` is renamed without importing its theme. | `migrate.rs:52-60` | Strong |
| CFG-07 | **An unsupported v1 version is preserved but re-warned on every launch**, with no user action available. | `migrate.rs:70-79` | Strong |
| CFG-08 | **Dead schema fields**: `telemetry_enabled` (never read; no telemetry exists), project-file `display_name` / `last_opened_at` (written, never read — the registry is authoritative), `interface_scale` tombstone (not a schema field). | `core:675,733,735`; `store.rs:381,414` | Strong |
| CFG-09 | **Defaults are duplicated**: `18/16/2/64` appear in `GlobalPreferencesV2::default`, `theme/metrics.rs:122-129` and `theme/typography.rs:14`; validation bounds appear in 3–4 places. Only `mono` from `metrics` is actually consumed. | see left | Strong |
| CFG-10 | **The Preferences UI shows the retired scale** (`14`) as the size placeholder while the default and hint say `18`. | `preferences/mod.rs:339` vs `appearance.rs:71-73` | Strong |

## 2. Migration inventory and classification

Legend: **(a)** obsolete migration, **(b)** current-schema defaults, **(c)** corruption
recovery, **(d)** platform compatibility, **(e)** document/DB migration, **(f)** still
required, **(g)** dead code.

| ID | Location | Trigger | What it does | Class | Legacy state reachable today? |
|---|---|---|---|---|---|
| M1 | `settings/migrate.rs:46-114` | old `preferences.json` exists | v1 `{version,theme,layout}` → `global.json`; layout discarded | **a** (+ c for the corrupt branch) | Only a build from 2026-11-01 |
| M2 | `store.rs:188-235` | `light_theme`/`dark_theme` present | delete retired palette names, back up | **a** | builds 11-03…11-04 |
| M3 | `store.rs:243-299` | `page_width_percent` absent + `editor_width` numeric | px → percent (px/1200) | **a** | builds before 11-04 |
| M4 | `store.rs:302-377` | `editor_line_height` absent | `18→16` font size, materialize writing defaults | **a** | builds before 11-04 |
| M5 | `store.rs:379-443` | `interface_scale` absent + `ui_font_size ∈ {14,16}` | `→18`, write tombstone | **a** | builds before 11-04 |
| M6 | `store.rs:445-503` | global envelope has `layout` | delete old layout, install per-mode layouts | **a** | 11-01…11-04 |
| M7 | `store.rs:510-570` | project envelope has `layout` | delete project layout | **a** | same |
| C1 | `store.rs:47-133` | malformed / unknown version | quarantine, read-only, warnings | **c** + **f** | any time |
| C2 | `core:900-960` | — | scope resolver | **b/f** | n/a |
| C3 | `core:966-1011` | — | registry serde defaults | **b** | any time |
| G1 | `store.rs:194,250,307,390,450,515` | unreachable | read-only guards | **g** | never |
| G2 | `store.rs:414` | — | `interface_scale` tombstone | **g** | — |
| G3 | `migrate.rs:24-28` | — | `#[allow(dead_code)] LegacyPreferencesV1.layout` | **g** | — |

**Why (a) is safe to remove:** there is **no tag, no release, no installer**
(`git tag -l` empty), and each migration was introduced on 2026-11-04, the same day the
format it reads was replaced. Every real file on a developer machine has already been
migrated by the build that introduced the migration. After deletion the old keys become
inert unknown fields that the envelope merge preserves (property 1.1.1), and serde
defaults fill anything absent.

**What is lost by removing (a) verbatim:** users who still hold an old default value keep
it instead of being re-baselined. E.g. `ui_font_size = 14` or `editor_font_size = 18`
would stay; the user can change it in Preferences. `default_mode`/`layouts` for an
un-migrated file fall back to `GlobalPreferencesV2::default()`, which is the intended new
default anyway.

## 3. Target architecture

One schema, one version, one migration module:

```
settings/
  store.rs        # load, atomic write, envelope merge, quarantine, read-only future
  upgrade.rs      # the ONLY migration: v<3 → v3, one backup, one pass
  model.rs        # re-export of astrolith_core::preferences
```

1. **Bump `GLOBAL_PREFERENCES_VERSION` to 3** (and `PROJECT_PREFERENCES_VERSION` to 2 when
   the project file changes shape). Replace every key-presence trigger with
   `raw["version"] < CURRENT`.
2. **`upgrade.rs` runs once**, in this order: (a) copy the whole file to
   `global.json.v<old>.bak` with `fs::write` + optional fsync; (b) apply the *final*
   defaults for the four re-baselined values, but only when the stored value equals the
   old default (`ui_font_size ∈ {14,16}`, `editor_font_size == 18.0`,
   `page_width_percent` absent, `editor_line_height` absent); (c) delete the retired keys
   (`light_theme`, `dark_theme`, `editor_width`, `interface_scale`, `layout`); (d) write
   `version: 3` atomically.
3. **After a successful upgrade the module never runs again**, and it produces no
   backups on any later launch (fixes CFG-03's chained snapshots).
4. **Retire `preferences.json` handling**: if it exists and `global.json` does not, import
   `theme` and delete/rename it once; if `global.json` exists, rename it to
   `preferences.v1.ignored.json` with a single warning. No repeated warning (fixes CFG-07).
5. **Drop the dead schema fields** and the tombstone (fixes CFG-08).
6. **Single source for numeric defaults**: `GlobalPreferencesV2::default()` stays
   authoritative; `theme/metrics.rs` keeps only `mono` and derives `ui`/`body` from the
   resolved settings (fixes CFG-09); the Preferences placeholder uses the current default
   (fixes CFG-10).
7. **fsync the parent directory** after rename (fixes CFG-04).
8. **Treat oversized-but-readable JSON as corruption**, i.e. quarantine it, instead of
   read-only defaults (fixes CFG-05).

## 4. Exact deletion list (file / function level)

| # | Remove | Keep |
|---|---|---|
| 1 | `settings/migrate.rs` entirely (`migrate_legacy_file`, `LegacyPreferencesV1`, `rename_legacy`, the two test modules) — **except** the one-shot "import theme if `global.json` is absent" step, which moves into `upgrade.rs` | `preferences.v1.corrupt.json` naming, if corrupt v1 files are still possible |
| 2 | `store.rs::migrate_neutral_palette` (188-235) | — |
| 3 | `store.rs::migrate_page_width` (243-299) | — |
| 4 | `store.rs::migrate_writing_typography` (302-377) | — |
| 5 | `store.rs::migrate_interface_scale` (379-443) | — |
| 6 | `store.rs::migrate_mode_layouts` (445-503) | the `default_mode`/`layouts` defaults themselves |
| 7 | `store.rs::migrate_project_layout` (510-570) | — |
| 8 | The six unreachable `!writable` blocks (G1) | `writable` plumbing in `load` |
| 9 | `interface_scale` tombstone write (`store.rs:414`) | — |
| 10 | `telemetry_enabled` + `ResolvedPreferences::telemetry_enabled` (`core:675,880,937` and their resolver arm) | the privacy statement in `help.rs` |
| 11 | Project-file `display_name`, `last_opened_at` (`core:733,735`; writer at `store.rs:762-763`) | registry `ProjectRecord` equivalents |
| 12 | `theme/typography.rs::PROSE_PAGE_WIDTH_PERCENT` fallback constant → derive from preferences | one fallback for "no settings loaded yet", passed explicitly |
| 13 | `theme/metrics.rs` `type_scale.ui/body/body_line` duplicates | `mono` |
| 14 | The seven migration tests in `store.rs` (`legacy_preferences_migrate_once_are_never_read_again`, `retired_palettes_are_backed_up_and_removed_once`, `pre_writing_defaults_upgrade_once_and_preserve_explicit_values`, `retired_interface_scale_upgrades_once_and_preserves_other_sizes`, `pre_mode_project_layout_is_backed_up_and_dropped`, …) | new tests for `upgrade.rs` failure modes (§5) |

Estimated deletion: ~500 production lines, ~250 test lines, 5 backup filename families,
1 on-disk path.

## 5. Data-safety analysis per deletion

| Deletion | User data at risk | What an existing install experiences | Safe transition |
|---|---|---|---|
| M1 | theme choice in `preferences.json` | theme falls back to `system` if the file is simply deleted without import | Import `theme` when `global.json` is absent; otherwise rename with one warning |
| M2 | none (retired palette *names* have no reader) | nothing changes visually | delete; keys become preserved unknowns |
| M3 | `editor_width` px | keeps stored `page_width_percent` if present; else new 64% default instead of the converted value | keep the px→% conversion inside `upgrade.rs` **only** if `page_width_percent` is absent; otherwise drop |
| M4 | `editor_font_size = 18` | stays 18 (slightly larger prose) | convert in `upgrade.rs` when `version < 3` |
| M5 | `ui_font_size = 14/16` | stays 14/16 | convert in `upgrade.rs` when `version < 3` |
| M6/M7 | old three-column composition | new per-mode defaults (already the documented v0.2 behaviour) | delete; no conversion exists even today |
| G1/G2/G3 | none | none | delete |
| CFG-08 fields | `display_name`/`last_opened_at` duplicated in the project file | registry already drives recent-project labels | delete schema fields; unknown keys remain until the next project-file rewrite, harmless |
| Parent-dir fsync | none | none (durability improvement) | add |

Irreversible consequences: only the loss of an *old default* that the user may have
accepted implicitly; no document, project, collection, preference value or registry entry
is deleted. Every migration backs up before writing, and the backup survives until the
user removes it (which `Reset All` should now also do — see §6).

## 6. Verification plan

All scenarios are **E2E on a real profile directory** (`XDG_CONFIG_HOME` pointed at a
fixture), artifact per `e2e-verification-plan.md`. No unit test may substitute for these.

| Scenario | Fixture | Expected | Evidence |
|---|---|---|---|
| `CFG-E2E-01` clean install | empty config dir | defaults; `global.json` created on first change only; **zero** `*.pre-*.json` / `*.bak`; no warning | manifest + file listing |
| `CFG-E2E-02` v2 with all retired keys | `global.json` containing `light_theme`, `dark_theme`, `editor_width`, `layout`, `interface_scale`, `ui_font_size: 14` | one `global.json.v2.bak`; `version: 3`; theme/size converted; retired keys gone; `layouts` = new defaults; unknown custom key preserved | before/after JSON diff |
| `CFG-E2E-03` v2 with explicit non-default values | `ui_font_size: 22`, `editor_font_size: 20`, `page_width_percent: 55` | values preserved exactly; no conversion | JSON diff |
| `CFG-E2E-04` second launch after upgrade | result of 02 | **no** new backup, no write, no warning | file mtimes + listing |
| `CFG-E2E-05` corrupt file | truncated JSON | `global.json.corrupt`; defaults; exactly one warning; app usable and writable | listing + UI screenshot |
| `CFG-E2E-06` future version | `version: 4` | defaults shown; saving refused; file byte-identical after a session | sha256 before/after |
| `CFG-E2E-07` legacy v1 file | `preferences.json` only | theme imported; file renamed once; no repeated warning on the next launch | listing + log |
| `CFG-E2E-08` reset | any of the above | Reset All removes `global.json`, `projects.json`, `projects/`, `preferences.json`, **and every `*.pre-*` / `*.bak` / `*.corrupt`** | listing |
| `CFG-E2E-09` oversized file | 1.1 MiB valid JSON | quarantined (not read-only), app writable | listing |
| `CFG-E2E-10` unknown envelope key | custom key `"future": {"x": 1}` | preserved across upgrade and across later saves | JSON diff |

## 7. Explicitly out of scope for this cleanup

- **Corruption quarantine** and **future-version read-only**: required, tested, keep.
- **Unknown-envelope merge**: required, keep.
- **Recovery records** and **save-failure artifacts**: not configuration; their format is
  current and their parse failures are already reported. No migration exists or is needed.
- **Platform directory resolution**: not migration code (it merely looks like branching).
- **Per-collection overrides**: current schema, not legacy.
- **The project registry's stable IDs / moved-project rebinding**
  (`core:966-1011`, `recent_projects.rs`): user data, keep.

## 8. Risk register for this cleanup

| Risk | Mitigation |
|---|---|
| A developer machine still holds a format the deleted code would have converted | Run `CFG-E2E-02/03` against a *copy* of the developer's real `~/.config/astrolith` before deleting anything |
| Removing `version` fields from structs changes the on-disk shape | Keep writing `version`; only the *read* path changes |
| Deleting the `ProseStyle` page-width fallback constant changes first-frame prose width before settings load | Pass the resolved default explicitly; verify the first frame in `SHELL_OPEN` workload |
| A future schema change repeats the key-presence mistake | The target design uses one versioned `upgrade.rs`; the roadmap makes "schema changes bump the version" a review rule |
