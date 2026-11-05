# Configuration, Migration and Persistence Audit

Repo: `/home/maredevi/Projects/Astrolith` @ `326eeec`, branch `dev`. Read-only audit.

## Scope inspected

| File | Lines | What was read |
|---|---|---|
| `crates/astrolith-core/src/preferences.rs` | 1604 | all schema types, defaults, resolver, registry, tests |
| `crates/astrolith-gpui/src/settings/store.rs` | 1324 | `Stored<T>`, all 6 migrations, store API, atomic write, all tests |
| `crates/astrolith-gpui/src/settings/migrate.rs` | 127 | v1 file import (whole file) |
| `crates/astrolith-gpui/src/settings/{mod,model,resolve}.rs` | 14/11/3 | re-export surface (whole files) |
| `crates/astrolith-gpui/src/preferences/{mod,advanced}.rs` | 1970/253 | draft state, `edits`, `reset_field`, `reset_paths`, versions pane |
| `crates/astrolith-gpui/src/preferences/tests.rs` | 365 | reset/validation/version tests |
| `crates/astrolith-gpui/src/theme/{mod,palettes,metrics,typography}.rs` | 243/169/189/163 | palette boundary, `apply`, token defaults |
| `crates/astrolith-gpui/src/recent_projects.rs` | 118 | label/ordering over the registry |
| `crates/astrolith-gpui/src/shell.rs` | 7363 | persistence call sites: 100-123, 590-650, 1429-1600, 2540-2660, 3120-3180, 3500-3545 |
| `crates/astrolith-document/src/recovery.rs` | 529 | record format, key, load/parse/clear |
| `crates/astrolith-gpui/src/editor/save_failure.rs` | 100 | artifact format |
| `docs/refactor/ARCHITECTURE.md` | §5-§6 | authoritative migration policy (lines 100-115) |

Git archaeology: `git log -S` / `git show` on `preferences.json`, `light_theme`, `editor_width`, `interface_scale`,
`mode_layouts`, plus `5ef6b23^:crates/astrolith-gpui/src/settings.rs`. No tags exist (`git tag -l` empty).

## Findings

Levels: **Confirmed** (control-flow/measured), **Strong** (direct source evidence), **Hypothesis** (needs runtime).

| ID | Title | Evidence | Level | Impact |
|---|---|---|---|---|
| F1 | Six in-store migrations plus a v1 import all target formats that existed only between 2026-11-01 and 2026-11-04; no release exists | `settings/store.rs:188-570`, `settings/migrate.rs:46-114`, `git tag -l` empty | Strong | ≈500 lines of one-shot code, five backup filenames, seven tests, and a second on-disk path kept alive for a format no user can hold |
| F2 | Every `!writable` guard inside the migrations is unreachable | `store.rs:194,250,307,390,450,515` vs `load` paths `:57,84,113` | Confirmed | six dead branches and six dead warning strings |
| F3 | `telemetry_enabled` is persisted and resolved but never read, and the app has no telemetry | `core:675,880,937`; `help.rs:283-286` | Strong | dead schema weight in the public file |
| F4 | Project-file `display_name`/`last_opened_at` duplicate the registry and are never read from the project file | `core:733,735`; writes `store.rs:762-763`; readers use `ProjectRecord` `core:966-979` | Strong | two authorities for the same metadata (ARCHITECTURE §5 line 105) |
| F5 | `version` is a coarse family marker; every post-2.0 change happened inside `version: 2` and migrations trigger on key presence | `core:14-18`; `store.rs:191,245,304,381,447,512` | Strong | version cannot be used to detect schema; hand-deleting a key re-runs a migration |
| F6 | Migration backups are incremental, not "the exact previous file", and `Reset All` leaves them behind | `store.rs:201-222,241-242,457-462`; `advanced.rs:57-69` | Strong | misleading guarantee; stale copies of retired settings survive reset |
| F7 | Downgrade→upgrade silently discards a re-created v1 `preferences.json` (rename without import) | `migrate.rs:52-60` | Strong | theme choice lost without a warning |
| F8 | An unsupported v1 version is preserved but re-warned on every launch | `migrate.rs:70-79` | Strong | permanent unactionable startup warning |
| F9 | A >1 MiB preferences file is a read error, not a corruption quarantine, so the app stays read-only | `store.rs:27,53-63,799-805` | Strong | user cannot recover through the UI; unknown-field growth can trigger it |
| F10 | `write_atomic` fsyncs the temp file but not the parent directory | `store.rs:812-823` | Strong | rename can be lost on power failure; no test covers it |
| F11 | Core defaults are duplicated as design tokens and fallbacks (`18`, `16`, `2`, `64`) | `core:696-700`; `theme/metrics.rs:122-129`; `theme/typography.rs:14` | Strong | silent divergence between design tokens and persisted defaults |
| F12 | The interface-size placeholder says `14` while the default and hint say `18` | `preferences/mod.rs:339` vs `appearance.rs:71-73` | Strong | user sees the retired scale as the example value |
| F13 | The shell holds effective, global and project layout copies and merges them by hand | `shell.rs:267-273,1485-1491,2583-2622` | Strong | project layout can leak into `global.json` if the guard is bypassed |
| F14 | `interface_scale` is written as a tombstone but is not part of the schema | `store.rs:381,414` | Strong | unexplained key in every upgraded file, re-read as an unknown field forever |

## Current schema (Q3)

### Locations

| Artifact | Path | Source |
|---|---|---|
| global scope | `<config>/astrolith/preferences/global.json` | `settings/store.rs:879-893` |
| project registry | `<config>/astrolith/preferences/projects.json` | `settings/store.rs:591-592` |
| project scope | `<config>/astrolith/preferences/projects/<id>.json` | `settings/store.rs:770-779`, `self.projects_dir.join(...)` |
| collection scope | **not a file** – `collections` map inside the project file | `crates/astrolith-core/src/preferences.rs:755-756`, `809-840` |
| legacy v1 file | `<config>/astrolith/preferences.json` | `settings/migrate.rs:31-42` |
| resurrection backup | `preferences.v1.migrated.json`, `preferences.v1.corrupt.json` | `settings/migrate.rs:51,117` |
| migration backups | `global.json.pre-{neutral,page-width,writing,interface-scale,mode}.json`, `<id>.pre-mode.json` | `settings/store.rs:201,258,314,400,457,522` |
| corruption quarantine | `global.json.corrupt`, `projects.json.corrupt`, `<id>.json.corrupt` (`-2..` suffix) | `settings/store.rs:825-840,842-862` |
| unsaved-edit recovery | `<config>/astrolith/recovery/<fnv16>.json` | `settings/store.rs:909-921`, `crates/astrolith-document/src/recovery.rs:311-326` |
| save-failure artifacts | `<config>/astrolith/save-failures/save-failure-*.json` | `settings/store.rs:894-906`, `editor/save_failure.rs:62-84` |

`<config>` resolution: `%APPDATA%` (Windows), `$HOME/Library/Application Support` (macOS), else absolute
`$XDG_CONFIG_HOME` or `$HOME/.config` (`settings/store.rs:864-877`). `ASTROLITH_SETTINGS_PATH` overrides the
global path only, must be absolute, and disables legacy discovery (`store.rs:880-889`, `migrate.rs:35-42`).
Two more env overrides exist for the other directories: `ASTROLITH_SAVE_FAILURE_DIR`, `ASTROLITH_RECOVERY_DIR`.

### Versioning

Three independent constants: `GLOBAL_PREFERENCES_VERSION = 2`, `PROJECT_PREFERENCES_VERSION = 1`,
`PROJECT_REGISTRY_VERSION = 1` (`core/preferences.rs:14-18`). The store reads `map["version"]` from the raw JSON
object, not from the deserialized struct (`store.rs:72-95`); the struct `version` fields are write-only (never read
outside `migrate.rs:71` for the v1 file).

Consequences (Strong):
- `version` identifies only a *coarse family*. Every post-2.0 schema change (`layouts`, `page_width_percent`,
  `editor_line_height`, `interface_scale`) landed inside the same `version: 2`; there is no migration ledger.
- Migration triggers are **key presence/absence** in the preserved envelope, not the version number
  (`store.rs:191,245,304,381,447,512`). Those keys are the de facto schema markers, and a migration cannot be
  distinguished from a user deleting a field by hand.
- An older build's file is never treated as an error, so "future version readonly" fires only for numeric
  versions `!= 2` (`store.rs:82-91`).

### On-disk format

Pretty JSON object. `version` is always written; every schema field is always serialized (no
`skip_serializing_if`), so optional values appear as `null` (`core/preferences.rs:635-707,725-758`). All
`PanelLayout`/`ModeLayouts`/`FieldMappings`/`ModeLayoutOverrides` derive `#[serde(default)]` and tolerate unknown
fields; only the v1 import struct uses `deny_unknown_fields` (`migrate.rs:20-29`).

Save merges the typed object over the previously loaded raw object, so **unknown top-level fields survive**
(`store.rs:150-156`). Fields removed from the schema therefore stay on disk forever; `retire_legacy_palettes` and
`migrate_mode_layouts` intentionally delete two such families.

Key serialization details:
- Enum values are `snake_case` (`theme: "light"|"dark"|"system"`, `default_mode: "writing"|"build"|"preview"`,
  `image_path_mode: "relative"|"absolute"`, `presentation: "external"|"embedded"`), `default_extension` is
  `"md"|"mdx"` (`core:24-110,296-311,492-521`).
- `AutosaveDelay` is an integer via `#[serde(into="u64", try_from="u64")]`; anything other than 1/2/5/10 fails
  deserialization of the whole file → quarantine (`core:132-171`).
- Copyedit colors are stored as `#rrggbb` strings; validation happens only at the theme boundary
  (`core:392-410`, `theme/mod.rs:36-47`).

### Field inventory (`global.json`)

`GlobalPreferencesV2` (`core:635-678`, defaults `681-706`); all keys always written.

| Key | Type / default | Consumer |
|---|---|---|
| `version` | u32, 2 | envelope check only (`store.rs:72-95`) |
| `theme` | `system` | `theme::apply` (`theme/mod.rs:99`) |
| `default_mode` | `writing` | shell mode restore (`shell.rs:1588-1620`) |
| `layouts.{writing,build,preview}` | `ModeLayouts` defaults (writing/preview closed, build sidebar 224 wide) | `resolve_with`, shell (`core:250-313`) |
| `window` | `null` / `WindowGeometry{x,y,width,height,maximized}` | `restored_bounds` (`shell.rs:188-196`), `capture_window_geometry` (`:1461-1477`) |
| `heading_colors.{light,dark}` | `null` hex strings | `ProseHeadingColor` (`theme/mod.rs:230-240`) |
| `ui_font_family` / `ui_font_size` | `""` / `18.0` | `theme::apply` (`theme/mod.rs:101-104`) |
| `prose_font_family` / `editor_font_size` / `editor_line_height` | `""` / `16.0` / `2.0` | `ProseStyle::new` (`shell.rs:3157-3162`) |
| `page_width_percent` | `64.0` | `ProseStyle` (`shell.rs:3161`) |
| `focus_mode` / `typewriter_mode` | `false` | `apply_writing_preferences` (`shell.rs:2613-2616`) |
| `autosave_delay` | `2` (seconds) | autosave policy |
| `default_extension` | `md` | new-entry creation |
| `ide.{executable,arguments}` | `""`, `[]` | IDE launch |
| `copyedit.{nouns,…,conjunctions,colors}` | false + 5 hex colors | command labels/menus (`commands.rs:253-272`) |
| `preview.{presentation,browser,open_on_enter}` | `external`, empty argv, false | preview surface |
| `telemetry_enabled` | `false` | **no reader** |
| `skipped_update_version` | `null` | update check (`shell.rs:1103,1248`) |

`projects.json` is `{version:1, projects:[{id,path,display_name,package_name,last_opened_at}]}`
(`core:965-993`); no reader of the envelope `version` exists, and no shape change has ever occurred, so no
migration is required.

Project file (`core:725-757`): `version`, `project_id`, `path`, `display_name`, `last_opened_at`, `last_mode`,
`layout_overrides.{writing,build,preview}` (each `null` or full `PanelLayout`), `last_entry`, `content_directory`,
`assets_directory`, `mdx_component_directory`, `image_path_mode`, `default_extension`, `field_mappings.{title,date,
description,draft}`, `insert_mdx_imports`, `collections.<name>` (`CollectionPreferencesV1`). Inheritance is by
`Option::None` (`null`), which is why `Option<bool>`/`Option<f32>` are used instead of sentinels
(`core:1-7,900-960`).

### Atomic write and durability

`write_atomic` (`store.rs:812-823`): create parent dirs, `NamedTempFile::new_in(parent)`, `write_all`,
`sync_all()` on the file descriptor, `persist(path)` (rename/replace). No directory fsync follows the rename, so a
power loss can lose the rename even though the temp file was durable. Writes are serialized per process by
`Mutex` (`store.rs:23-25,782-786`), not by a file lock. Cross-process safety comes from (a) the single-instance
socket and (b) the baseline comparison: if the file on disk differs from the last accepted bytes, the save is
refused with "Preferences changed outside Astrolith; restart before saving settings"
(`store.rs:135-148`). If the single-instance bind itself fails the app continues (`shell.rs:112-118`), so a second
instance degrades to *no persistence plus a warning*, not to corruption (Strong).

`Stored::save` also implements monotonic revisions: `revision <= self.revision` returns `Ok(false)`
(`store.rs:141-143`). Shell revisions are per scope (`shell.rs:1484-1489,1517-1520`), and debounced writes are
250 ms (`shell.rs:3504-3512`).

### Read/load failure behavior (`store.rs:47-133`)

| Condition | Result | Data |
|---|---|---|
| missing file | `T::default()`, writable | none |
| read error (perms, >1 MiB) | defaults, **writable = false**, warning; file untouched (`store.rs:53-63,799-805`) | preserved |
| invalid JSON / non-object | rename to `*.corrupt`, defaults, writable | preserved |
| `version` missing/string/negative/fractional | quarantine (`store.rs:93-98`) | preserved |
| `version` numeric but `!= 2` | defaults, **read-only**, warning (`store.rs:82-91`) | preserved |
| body fails `T` deserialize | quarantine (`store.rs:105-109`) | preserved |
| quarantine rename fails | defaults, read-only, warning (`store.rs:111-121`) | preserved |

`MAX_PREFERENCE_BYTES = 1 MiB` (`store.rs:27`). An oversized file is a *read* error, so it is never quarantined and
the app stays read-only until the user intervenes — including all migration code paths.

Warning aggregation: `combine_warnings` joins all migration + load warnings with spaces (`store.rs:788-791`), and
`PreferencesStore::load` surfaces them through `Shell::warning` (`shell.rs:120-137`). All migrations run
synchronously inside `runtime::blocking` before the first window opens (`shell.rs:119-122`).

### Backup semantics (important nuance)

Every `migrate_*` writes a backup with `fs::write` (not atomic, no fsync) **before** rewriting the file
(`store.rs:201-206,258-263,314-319,396-403,457-462,522-534`). The backups are *incremental*: each one captures the
file as left by the previous migration in the chain, because `self.baseline` is refreshed after each successful
write (`store.rs:220,222`). For a file needing `layout` + `editor_width` + `editor_line_height`, the first backup is
the pristine original but the later ones are intermediate states. The doc comments claiming "the exact previous
file is kept" (`store.rs:241-242,299-301`) are therefore true only for the first migration that fires.

`Reset All` deletes exactly `global.json`, `projects.json`, `projects/`, `preferences.json`,
`preferences.v1.migrated.json` (`preferences/advanced.rs:57-69`). It does **not** delete any
`global.json.pre-*.json`, `*.corrupt`, or `projects/<id>.pre-mode.json`; those stay on disk after a reset
(Strong; the reset test only creates the five listed paths, `preferences/tests.rs:305-336`).

## Migration / legacy inventory (Q1)

Every row below runs on the same thread, in the order listed, from `PreferencesStore::load`
(`store.rs:573-611`); project rows run lazily on the first `store.project(id)` (`store.rs:666-694`).

| # | Path:line | Trigger | Converts | On-disk result | Can the legacy state exist on disk today? |
|---|---|---|---|---|---|
| M1 | `settings/migrate.rs:46-114` | `<config>/astrolith/preferences.json` exists | v1 `{version:1,theme,layout}` → `GlobalPreferencesV2{version:2,theme}`; layout **discarded**; all other fields default | `global.json` written; legacy renamed `preferences.v1.migrated.json`; corrupt → `preferences.v1.corrupt.json` | Only if a build from 2026-11-01 15:11 or earlier ran. If `global.json` already exists the file is renamed **without importing the theme** (`migrate.rs:52-60`) |
| M2 | `store.rs:188-235` | `light_theme` or `dark_theme` present | both keys deleted; values (`nord_light`, `ink`, …) have no reader anywhere | backup `global.json.pre-neutral.json`, then rewrite | Only for a build between `e9e6402` (11-03) and `e75b15f` (11-04) |
| M3 | `store.rs:243-299` | `page_width_percent` absent **and** `editor_width` numeric | px → percent: `px/1200*100` clamped 30-100, 1 decimal; key removed | backup `global.json.pre-page-width.json`; `page_width_percent` written | Only for builds before `bf3b12b` (2026-11-04); `editor_width` default was 720 at `b84c1ef` (11-03), then 768, so both 60% and 64% convert |
| M4 | `store.rs:302-377` | `editor_line_height` absent | `editor_font_size == 18.0` → `16.0`; `editor_line_height = 2.0`; materializes `prose_font_family`, `focus_mode`, `typewriter_mode` | backup `global.json.pre-writing.json`; fields written | Only for builds before `021e542` (2026-11-04) |
| M5 | `store.rs:379-443` | `interface_scale` absent **and** `ui_font_size` ∈ {14,16} | `ui_font_size → 18`; writes tombstone `interface_scale: 1` | backup `global.json.pre-interface-scale.json` | Only for builds before `d6df349` (2026-11-04) |
| M6 | `store.rs:445-503` | global envelope has `layout` | deletes `layout` (old `{sidebar_visible,inspector_visible,preview_visible,sidebar_width,inspector_width}`); writes `default_mode` + `layouts` from defaults; old composition **never restored** (per ARCHITECTURE §6 line 113) | backup `global.json.pre-mode.json` | Only for builds between `5ef6b23` (11-01) and `264aa51` (11-04) |
| M7 | `store.rs:510-570` | project envelope has `layout` | deletes project `layout`, writes nothing in its place; project inherits global mode layouts | backup `projects/<id>.pre-mode.json` | same window as M6 |
| C1 | `store.rs:47-133` | any malformed/future file | corruption quarantine / read-only future version | see table above | any time (current-schema safety) |
| C2 | `core/preferences.rs:900-960` | – | inheritance resolver (`collection > project > global`) | in-memory | n/a |
| C3 | `core/preferences.rs:966-1011` | registry load | `ProjectRecord` missing `package_name` etc. → serde defaults; no migration needed (registry never changed shape since v1) | – | a v1 registry from 11-01 is read as-is |

Git dates from `git log -1 --format=%ad`: `5ef6b23` 2026-11-01 15:11; `264aa51`, `e75b15f`, `021e542`, `d6df349`,
`bf3b12b` all 2026-11-04. There is no tag, no installer, no published release (`git tag -l` empty), so the only
possible legacy files are on the developer's own machine and have already been migrated by the builds that
introduced each migration.

Not migrations, but adjacent persistence with no version field:
- recovery records: flat JSON `{project,generation,source,revision,content,modified}` written atomically,
  strict parse, corrupt files reported as diagnostics and never deleted (`document/recovery.rs:270-315,160-205`).
- save-failure artifacts: flat JSON, content-free, never auto-deleted (`editor/save_failure.rs:27-84`).

### Migration failure matrix

| Migration | Failure point | Returned warning / effect | File state |
|---|---|---|---|
| M1 | read fails | `Could not read legacy preferences at …` | legacy kept; retried next launch |
| M1 | parse/`deny_unknown_fields` fails | `Could not migrate legacy preferences: … The invalid file was moved to preferences.v1.corrupt.json` (or "preserved" if rename fails) | legacy preserved/quarantined |
| M1 | `version != 1` | `Unsupported legacy preference version N at …; the file is preserved.` | file untouched, so **warning repeats every launch** until the user deletes it |
| M1 | `write_atomic(global.json)` fails | `Could not write migrated preferences` | legacy kept, no import |
| M1 | final rename fails | `Preferences were migrated, but the legacy file … could not be retained` | `global.json` written; next launch takes the "global exists" rename-only path |
| M2–M7 | backup `fs::write` fails | `Could not back up … The settings file is preserved.` | returns **before** any mutation; retried next launch |
| M2–M7 | encode fails | `Could not encode …` | realistically unreachable: all values came from parsing valid JSON, and non-finite floats cannot appear in JSON |
| M2–M7 | `write_atomic` fails | `Could not … in <path>: <err>. The file is preserved.` | file on disk unchanged, but `self.value`/`self.envelope` were already mutated in memory (M3/M4/M5 set the value before writing), so the session runs with the migrated value and a later save persists the new shape |
| M2–M7 | `!writable` | unreachable (see Q4) | – |

There is no migration that can destroy a document; the worst outcome in the matrix is a stale warning or a
not-yet-applied visual default.

## Classification (Q2)

Legend: (a) obsolete migration, (b) current-schema defaults, (c) corrupted-config recovery,
(d) platform compatibility, (e) document/database migration, (f) still-required compat, (g) dead code.

| ID | Classification | Justification |
|---|---|---|
| M1 | **obsolete migration** | one-shot path move on the same day the scoped store was introduced; no release predates it; the only user file has been imported already |
| M2 | **obsolete migration** | retired palette *names* have no reader (`theme/palettes.rs` only exposes `light()`/`dark()`; no name registry); the keys are inert unknown fields that the envelope already preserves |
| M3 | **obsolete migration** | pure re-defaulting of a UI measure; old value is not user content and stays in the file as an unknown key |
| M4 | **obsolete migration** | its entire effect is `18→16` plus materializing fields that serde already defaults (`store.rs:320-345` vs `core:696-700` defaults) |
| M5 | **obsolete migration** | same class as M4; the tombstone `interface_scale` is not even part of the schema |
| M6 | **obsolete migration** (specified as a deliberate one-way drop in ARCHITECTURE §6) | old composition is explicitly not restored; deleting the code is behaviourally identical because `layout` is an unknown key for `GlobalPreferencesV2` |
| M7 | **obsolete migration** | same as M6 for `ProjectPreferencesV1` |
| M1 corrupt branch | **corrupted-config recovery** (C) | distinct from the version mapping; quarantine is genuinely useful |
| C1 | **corrupted-config recovery** | current-schema safety, must stay |
| C1 future-version branch | **still-required compat (f)** | protects a newer build's file from being clobbered by an older one; required by ARCHITECTURE §6 line 105 and tested at `store.rs:1147-1163` |
| C1 unknown-envelope merge | **still-required compat (f)** | also the mechanism that makes M2/M3/M6/M7 deletable without data loss |
| registry defaults (C3) | **current-schema defaults (b)** | pure serde defaults, no code |
| recovery record parse | **corrupted-config recovery (c)** for corrupt files, otherwise current format; **no migration exists or is needed** |
| `!self.writable` guards in migrations (`194,250,307,390,450,515`) | **dead code (g)** | unreachable, see Q4 |
| `interface_scale` tombstone (`store.rs:414`) | **dead code (g)** | never read by any schema type or consumer |
| `LegacyPreferencesV1.layout` (`migrate.rs:24-28`) | **dead code (g)** | `#[allow(dead_code)]`, only parsed so `deny_unknown_fields` accepts the old file |

No row qualifies as (d) platform compatibility or (e) document/database migration; the only platform-specific code
is directory resolution (`store.rs:864-877`) and no database exists.

## Dead code and dead data (Q4, Q6)

### Unreachable branches (Strong, by control-flow proof)

`Stored::load` sets `writable = false` only on three paths, and all three return **before** `envelope` is populated
(`store.rs:57-58,84-85,113-114`). `envelope` becomes `Some` only on the success path, where `writable` is still
`true`. Every migration starts with `let envelope = self.envelope.as_mut()?` (`store.rs:190,246,…`), so inside a
migration `!writable` is impossible. The six blocks `store.rs:194-198,250-254,307-311,390-394,450-454,515-519` can
never execute, and their user-facing strings ("remains in {} because the file is read-only") are unreachable.
Corollary: the read-only warning for a future-version file is produced once by `load`, not by migrations.

### Fields written but never read

| Field | Line | Evidence |
|---|---|---|
| `GlobalPreferencesV2::telemetry_enabled` | `core/preferences.rs:675` | zero consumers outside the struct/default/resolver; `help.rs:283-286` says the app has no telemetry; `ResolvedPreferences::telemetry_enabled` (`:880`) is likewise unread |
| `ProjectPreferencesV1::last_opened_at` | `:735` | written at `store.rs:763`; no reader (registry's `ProjectRecord::last_opened_at` at `:976` is what `recent()` uses, `core:1111-1120`) |
| `ProjectPreferencesV1::display_name` | `:733` | written at `store.rs:762`; no reader (registry copy at `:972` drives `recent_projects::entries`) |
| `version` struct fields | `:637,727,811,984` | only the v1 import reads a version (`migrate.rs:71`); the store parses the raw envelope instead |
| `ModeLayouts` / `ProjectRegistry` version | as above | write-only |

`ProjectPreferencesV1::path` is read only as a non-empty guard (`project_discovery.rs:3022-3025`); the authoritative
path is the registry's. Three project-file metadata fields therefore duplicate the registry (`core:729-735` vs
`:966-979`), which is the "two authorities" pattern ARCHITECTURE §5 line 105 warns against.

### Duplicated defaults / resolver logic (Q6)

There is **one** settings model (`astrolith_core::preferences`) and **one** resolver (`resolve`, `core:900-960`),
re-exported unchanged (`settings/resolve.rs:1-3`). `resolve` is used in production by
`project_discovery::resolved_for` (`project_discovery.rs:4610-4624`). No second resolver exists. The duplication is
in *defaults and derived state*:

1. `GlobalPreferencesV2::default()` (`core:681-706`) is authoritative: `ui_font_size 18`, `editor_font_size 16`,
   `editor_line_height 2`, `page_width_percent 64`.
   - `theme/metrics.rs:122-129` repeats `type_scale.ui = 18`, `body = 16`, `body_line = 32`. Only `mono` is consumed
     in production (`theme/mod.rs:227`, `terminal/mod.rs:27`); `ui` is asserted against the preference-derived
     `theme.font_size` in `theme/tests.rs:176`, so the two can diverge while the test still passes only by
     coincidence of the 18 default (Strong).
   - `theme/typography.rs:14` `PROSE_PAGE_WIDTH_PERCENT = 64.` duplicates `page_width_percent`; it is used as a
     fallback when a renderer has no style yet (`editor/session.rs:196`, `editor/rendered.rs:134`,
     `layout/panels.rs:387-390`).
2. Validation bounds are duplicated, sometimes twice in the same file: `8..=28` in `preferences/mod.rs:544`
   (parse fallback) and `:1755` (inline error) and `theme/mod.rs:101` (clamp); `30..=100` in
   `preferences/mod.rs:577`, `:1743`, `writing_modes.rs:60`; `1.2..=3.0` in `preferences/mod.rs:559`, `:1731`.
   The render path clamps all four (`theme::apply:101`, `ProseStyle::new:56-62`), so a hand-edited value is not a
   correctness bug — but any bound change must be made in three or four places that have no shared constant.
3. Stale UI placeholder: `preferences/mod.rs:339` shows `"14"` for the interface size, while the hint
   (`appearance.rs:71-73`) and the default (`core:696`) say 18. The placeholder is not the default, but it is the
   only number the field shows when empty.
4. `Shell` keeps three copies of layout data: effective `settings.layouts`, global-only `global_layouts`, and
   `project_scope.preferences.layout_overrides` (`shell.rs:267-273,320`). `install_project_scope` correctly
   rebuilds a global snapshot (`layouts: self.global_layouts`, `shell.rs:1606-1610`) and calls core `resolve`, but
   `apply_preferences` re-does the same merge by hand (`shell.rs:2583-2622`), and `persist_global` has to restore
   `settings.layouts = global_layouts` before writing to keep the project layout out of `global.json`
   (`shell.rs:1485-1491`). `sync_project_preferences` then passes the *merged* `settings` as the "global" scope to
   `ProjectWorkspace::set_preferences` (`shell.rs:1645-1659` → `project_discovery.rs:4610-4624`); harmless today
   only because `ResolvedPreferences::layouts` is never consumed there (no `.layouts` read in
   `project_discovery.rs`), but the scope names are wrong (Strong).
5. `theme::palette(ThemeMode::System)` silently returns the light palette (`theme/mod.rs:75-80`); every caller must
   resolve `System` first (`ResolveTheme`, `:63-72`). One call site does (`apply`, `:99`), tests pass explicit
   modes.

**Verdict (Q6).** There *is* one source of truth for the model, the scope order and the serialized shape:
`astrolith_core::preferences`; `settings/{model,resolve}.rs` only re-export it, and no GPUI-side resolver exists.
There is **no** single source for default *values* or accepted *ranges*: the same four numbers live in the core
`Default` impls, `theme/metrics.rs`, `theme/typography.rs`, the preferences sheet (placeholders + two validator
copies), and the shell's merged snapshot. The cheapest consolidation is to make `GlobalPreferencesV2::default()`
the only place the four typography numbers exist and have `METRICS`/`PROSE_*` reference it (or delete them), then
expose the ranges as `const` pairs next to the fields and use them in the sheet and the render clamps.

### Leftover assertions / comments

- `store.rs:961` asserts `envelope.get("mode_layouts").is_none()`; `mode_layouts` was never an on-disk key
  (`git log -S'mode_layouts'` only finds commit messages/docs). The assertion documents nothing.
- `store.rs:241-242,299-301` claim "the exact previous file is kept" — false for chained migrations (see above).
- `store.rs:186-187` claims selection "has moved out of the user's configuration"; in fact `theme` remains a
  user-facing preference (`appearance.rs:41-50`). The comment is about *palette selection*, but reads as if theme
  were retired.
- `migrate.rs:1-4` says the legacy file is "retained as `preferences.v1.migrated.json`. Later launches never read
  it again" — true, but a downgrade that rewrites `preferences.json` is silently discarded when `global.json`
  exists (`migrate.rs:52-60`), so "never read again" is also "never merged again".

## Deletion candidates (Q5)

General property that makes most deletions safe: `GlobalPreferencesV2`/`ProjectPreferencesV1` are
`#[serde(default)]` without `deny_unknown_fields`, and `Stored::save` merges the raw envelope, so removing a
migration never deletes data — the old keys stay on disk and are ignored (Strong; verified by
`core:1518-1537` and `store.rs:934-964`).

| ID | Deletion candidate (path:line) | Classification | Risk | Verification after deletion |
|---|---|---|---|---|
| D1 | `settings/migrate.rs:1-127`; call `store.rs:574-578`; module decl `settings/mod.rs:7` | obsolete migration | **low** — a pre-2026-11-01 install loses only the theme import; file itself remains | delete `store.rs:1004-1033`; keep `advanced.rs:66-68` or drop the two reset paths; manual: place `{version:1,theme:"dark"}` at `preferences.json` with no `global.json` → app starts `System` and leaves the file untouched |
| D2 | `store.rs:188-235`; call `:586` | obsolete migration | **none** — palette names have no reader | delete `store.rs:966-1003`; grep `light_theme\|dark_theme` returns only the core test `:1518-1537` |
| D3 | `store.rs:243-299`; call `:589` | obsolete migration | **low** — a hand-tuned pixel column silently becomes 64% | delete `store.rs:1034-1089` (writing/page-width asserts); manual: `{"version":2,"editor_width":900}` → 64%, key retained |
| D4 | `store.rs:302-377`; call `:590` | obsolete migration | **low** — an explicit 18 px document size is kept instead of reset to 16 | delete `store.rs:1034-1089`; manual: file without `editor_line_height` → 2.0 via serde |
| D5 | `store.rs:379-443`; call `:588` | obsolete migration | **low/benign** — explicit 14/16 px interface stays; this is the least destructive outcome | delete `store.rs:1089-1130`; no `interface_scale` key is ever written |
| D6 | `store.rs:445-503`; call `:587` | obsolete migration | **none** — old composition is already discarded by design | keep the `saved_theme_and_layout_restart…` test but drop the `layout` fixture at `:940` and the three migration asserts at `:960-963`; it is the only coverage of unknown-field preservation, so replace `layout` with an arbitrary key |
| D7 | `store.rs:510-570`; call `:677-685` | obsolete migration | **none** — project `layout` is an ignored unknown key | delete `store.rs:1240-1266`; manual: project file with `layout` loads with `layout_overrides == default` and keeps the key |
| D8 | `store.rs:194-198,250-254,307-311,390-394,450-454,515-519` | dead code | **none** — provably unreachable | compiles unchanged; no test exercises them today (they have no coverage) |
| D9 | `store.rs:414` (`interface_scale` tombstone) | dead code | **none** — never read | grep `interface_scale` → only D5 and the dead guard at `:381` |
| D10 | `migrate.rs:24-28` (`LegacyPreferencesV1.layout`) | dead code | **none** — `#[allow(dead_code)]` parse-only filler | removing it and the `deny_unknown_fields` attribute are equivalent; keep the attribute if the field goes |
| D11 | `store.rs:961` (`mode_layouts` assertion) | dead assertion | **none** | `mode_layouts` never existed as a key (`git log -S` finds docs only) |
| D12 | `preferences/mod.rs:339` (`input!("14")`) | stale default | **low** — misleading empty-field placeholder | change to `"18"` or read `GlobalPreferencesV2::default().ui_font_size` |
| D13 | `core:675,880` (`telemetry_enabled`) | dead schema field | **medium to remove** — changing a persisted struct requires acknowledging that the value stays in the envelope; removal is additive-safe because unknown fields survive | no consumer to update; grep `telemetry` → `help.rs:285` only |
| D14 | `core:733,735` project `display_name`/`last_opened_at` | duplicated authority | **medium** — the registry already owns both; removing changes the project-file shape and `store.rs:761-763` | `recent_projects`/`recent()` tests (`store.rs:1192-1239`) prove the registry copy is authoritative; no migration needed because extra keys are preserved |

Do **not** delete: corruption quarantine (`store.rs:109-133`), the numeric future-version read-only branch
(`:82-91`), the unknown-envelope merge (`:150-156`), baseline external-change detection (`:145-148`), the recovery
record parser, or `write_atomic`.

Failure modes that deletion also removes (currently real, not hypothetical):
- M1 can *silently drop* a v1 theme on a downgrade→upgrade cycle (`migrate.rs:52-60` renames without merging).
- M2–M7 do up to 5 sequential whole-file rewrites on the first launch after an upgrade. Each rewrite refreshes
  `baseline`; if the process dies between backup and rewrite the file is still consistent, but the user is left
  with a pile of backups that `Reset All` does not clean up (`advanced.rs:57-69`).
- Each migration can return a warning that is concatenated into one long sentence in the UI
  (`store.rs:788-791`, `shell.rs:133-136`), which is unactionable noise for a user who cannot act on "the fixed
  page width remains in …".
- `migrate_legacy` repeats the "Unsupported legacy preference version" warning on every launch because that path
  never moves the file (`migrate.rs:70-79`).

Deletion order that keeps the tree compiling after each step: D7 → D6 → D5 → D4 → D3 → D2 → D1; D8–D11 can go in
any order. The shared helpers (`unique_destination`, `write_atomic`) are used by `Stored::save`/`quarantine` and
must stay. After D1–D7 only four store tests remain relevant: `store.rs:934-964` (rewrite without the migration),
`:1131-1178` (quarantine/envelope), `:1180-1190` (revision ordering), `:1192-1239`/`:1267-1324` (registry).

## Open questions / not inspected

- Whether any published build exists outside the repo (installers, CI artifacts). Nothing in-repo indicates one;
  the conclusion "no real user can still hold a legacy file" rests on that.
- `runtime::blocking` / `flush_on_quit` ordering guarantees at process exit were not inspected beyond the call
  sites (`shell.rs:604-640`); a quit during a migration was not exercised.
- Filesystem semantics of `NamedTempFile::persist` on Windows were not executed; the code relies on its
  replace-existing behaviour.
- Whether `page_width_percent` should be 60 or 64 for upgraders is a product decision, not a code fact.
- `astrolith-web-mock/` (untracked React prototype) was not inspected for a parallel settings model.
- The recovery/save-failure modules were checked for versioning only; their full lifecycle is another audit's
  scope.
