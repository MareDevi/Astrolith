# Astrolith — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkboxes for tracking.

**Status:** M00 manually accepted by the user; M01 and all M02 tasks complete; M02.R runtime/responsiveness repairs complete; native IME, scaling and latency manually accepted by the user on 2026-10-29; M03.1 JSON Schema normalization and the R1 schema generation service complete on 2026-10-30; M04.1 lossless document model, M04.2 conflict-aware save, M04.3 recovery, and shell save/recovery integration complete on 2026-10-31; on 2026-10-31 the remaining backlog — fixture schema oracles, M03.2 Astro helper resolution, M03.4 frontmatter validation, M03.3 schema-driven forms, and recovery lifecycle polish — is implemented and headless-verified (`mise run check` PASS). Both independent-review P1s are closed (`b676845`, `fddbd34`). **M05 (Astro preview services) is implemented and headless-verified on 2026-10-31:** M05.1 dev-server lifecycle (`fca3fcd`), M05.2 entry route resolution (`59e8544`), M05.3 external browser surface plus the Preview-panel wiring (`2c8c562`, `1ba5ed3`), and the M05 gate — an opt-in test that drives the real `astro dev`, saves an edit and observes it in the served HTML (`365bcbe`). M05 has **not** been independently reviewed: the implementing team hit a provider rate limit and was halted, so the remaining work was completed directly; that review is owed. Native browser/rendering acceptance for preview, and native save/recovery/form acceptance, remain **pending human execution**; headless tests do not establish native IME/focus/scaling/rendering behaviour. Remaining open items are the P2s listed in `docs/technical/m03-progress.md` and the limits in `docs/technical/m05-progress.md`. On 2026-10-31 Astro **6.4.8** / MDX **6.0.3** joined the verified release matrix with a committed frozen fixture, schema oracle, and array/negative-glob loader coverage. **M06.1 (preview capability detection) is implemented on 2026-10-31** in `crates/astrolith-preview/src/capabilities.rs`: embedded preview is classified from injectable host facts, gated behind the default-off `embedded-webview` feature, and never offered on Linux Wayland/XWayland, headless or unknown hosts; 14 focused tests pass, full `mise run check` recorded in `docs/technical/m06-progress.md`. M06.2 (WebView spike) remains next.
**Version:** 1.1
**Date:** 2026-10-28  
**Project:** Astrolith  
**Methodology:** Superpowers — TDD, task-based execution, review gates

**Goal:** Build a cross-platform, native Astro Content Collections editor with reliable Markdown/MDX editing, schema-driven metadata forms, safe document persistence, and real Astro live preview.

**Architecture:** Rust modular monolith with a GPUI Kit desktop shell, independent domain and document services, an Astro tooling adapter, and a replaceable preview surface.

**Tech Stack:** Rust, GPUI Kit 0.8.1 (pinned), Tokio service runtime, GPUI background executor, Cargo, Mise, Node.js, Bun, Tree-sitter, Astro CLI, Wry-based experimental WebView.

**Spec:** `docs/superpowers/specs/2026-10-28-astrolith-design.md` (still absent). On 2026-10-28, the user explicitly authorized the existing M01 plan as this stage's design authority after reporting all M00 manual acceptance complete. See `docs/technical/m01-authority.md`.

---

## 1. Global Constraints

### 1.1 Mandatory Technologies

- Application name: **Astrolith**.
- Desktop framework: **GPUI**.
- UI components: **GPUI Kit**.
- Design language: **Zed-inspired native desktop UI**.
- Environment management: **Mise**.
- Rust dependencies and builds: **Cargo**.
- JavaScript fixture management: **Bun**.
- Astro integration: use project-local tooling.
- Linux Wayland: native GPUI plus external browser preview.
- Experimental embedded preview must not be required for normal editor operation.

### 1.2 Environment Policy

1. `mise.toml` is the authoritative source for project development tool versions and task entry points.
2. Rust, Node.js and Bun must be managed through Mise.
3. `Cargo.lock` must be committed.
4. Astro test projects must use committed lockfiles.
5. CI must use the same Mise configuration as local development.
6. No unpinned `latest` versions in the finalized reproducible toolchain.
7. Native OS dependencies, such as system graphics libraries, must be listed separately because Mise does not automatically manage all system packages.
8. Do not add `.nvmrc`, `.tool-versions` or other competing project toolchain version files without a documented compatibility requirement.
9. If Rust tooling requires `rust-toolchain.toml`, its version must match the Rust version selected in Mise.

### 1.3 Development Rules

- TDD for domain logic, parsers, persistence and services.
- One independently testable outcome per implementation task.
- Every task must end with verification and a commit.
- No GUI component directly performs filesystem operations.
- No blocking I/O on the GPUI UI thread.
- No silent document data loss or unsafe type conversion.
- Favor GPUI Kit components over custom replacements.
- Preserve the ability to run the core test suites without launching the GUI.

### 1.4 Concurrency and Responsiveness Amendment

The user authorized this amendment on 2026-10-28 after reporting sluggish development UI and slow file opening. The binding execution boundaries, Zed source references, task ownership, queue limits and performance procedure are in [Concurrency and responsiveness](../../technical/concurrency-and-responsiveness.md). This authority covers the amendment and confirmed responsiveness repairs; it does not fill the missing original specification.

- GPUI owns UI entities and presentation. One application-owned Tokio runtime (two async workers) owns service I/O; synchronous filesystem operations use a lane capped at two running jobs. GPUI's existing background executor remains available for bounded CPU work.
- No UI `block_on`, synchronous service wait, filesystem operation, or large derived-data rebuild in `render`. An `async fn` declaration is not proof of background execution.
- Retain cancellable task handles. Carry project generation, document identity and revision across asynchronous boundaries; reject obsolete results even after cancellation.
- Bound admission and output queues. Merge watcher bursts, debounce settings writes, and serialize document saves independently of settings.
- Explicitly coordinate task draining, document integrity, child-process termination and reaping on project switch and application exit.
- Native responsiveness requires measured file-to-present and interaction latency; headless CPU timing and successful build are separate evidence.

---

## 2. Repository Structure

```text
astrolith/
├── Cargo.toml
├── Cargo.lock
├── mise.toml
├── .github/
│   └── workflows/
│       └── ci.yml
├── apps/
│   └── desktop/
│       ├── Cargo.toml
│       └── src/
│           └── main.rs
├── crates/
│   ├── astrolith-core/
│   ├── astrolith-astro/
│   ├── astrolith-document/
│   ├── astrolith-workspace/
│   ├── astrolith-preview/
│   └── astrolith-gpui/
├── fixtures/
│   ├── astro5/
│   ├── astro6/
│   ├── astro7/
│   └── edge-cases/
├── tests/
├── docs/
│   └── superpowers/
│       ├── specs/
│       └── plans/
├── AGENTS.md
└── README.md
```

Crates may start smaller than this target layout, but the dependency direction must remain unchanged.

---

## 3. Mise Configuration

### 3.1 Initial `mise.toml`

The following is the intended bootstrap template. Version aliases must be replaced with tested, concrete versions at M00 completion.

```toml
min_version = "2026.1.0"

[tools]
rust = "stable"
node = "24"
bun = "1.3"

[env]
RUST_BACKTRACE = "1"

[tasks.doctor]
description = "Verify development toolchain"
run = """
rustc --version
cargo --version
node --version
bun --version
"""

[tasks.dev]
description = "Run Astrolith desktop"
run = "cargo run -p astrolith-desktop"

[tasks.build]
description = "Build all Rust crates"
run = "cargo build --workspace --locked"

[tasks.test]
description = "Run Rust tests"
run = "cargo test --workspace --locked"

[tasks.fmt]
description = "Verify Rust formatting"
run = "cargo fmt --all -- --check"

[tasks.lint]
description = "Run Clippy"
run = "cargo clippy --workspace --all-targets --locked -- -D warnings"
```

The exact Rust toolchain must be selected after verifying GPUI Kit compatibility. The sample `bun = "1.3"` is likewise an initial series target, not a claim that its complete compatibility matrix has been verified.

### 3.2 Required Developer Commands

```bash
mise install
mise run doctor
mise run dev
mise run build
mise run test
mise run fmt
mise run lint
```

Additional tasks will be introduced only when their associated subsystems exist:

- `mise run test:schema`
- `mise run test:document`
- `mise run test:preview`
- `mise run test:ui`
- `mise run check`
- `mise run ci`

### 3.3 CI Policy

GitHub Actions must:

- Install Mise.
- Install tools from `mise.toml`.
- Run `mise run check`.
- Use `cargo --locked` for reproducible builds.
- Build on Linux, macOS and Windows.
- Separate unit tests from platform-specific GUI tests.
- Cache dependencies without making caches authoritative.
- Test Astro fixtures using their own declared dependency versions.

---

## 4. M00 — Environment and Technical Feasibility

**Goal:** Validate the toolchain, GPUI Kit and editing primitives before committing to the application architecture.

**Estimated effort:** 1–2 weeks.

### Task M00.1 — Mise and Rust Workspace

**Files**
- Create: `mise.toml`
- Create: `Cargo.toml`
- Create: `apps/desktop/Cargo.toml`
- Create: `apps/desktop/src/main.rs`
- Create: `.github/workflows/ci.yml`
- Create: `AGENTS.md`

**Interfaces**
- Produces: `astrolith-desktop` binary.
- Produces: `mise run doctor`, `build`, `test`, `fmt`, `lint`.

- [x] Create the repository and Rust Workspace.
- [x] Define and install the Mise toolchain.
- [x] Verify GPUI Kit resolves with the selected Rust version.
- [x] Pin exact tool versions following compatibility verification.
- [x] Verify that `mise run doctor` succeeds.
- [x] Verify `mise run build` and `mise run test`.
- [x] Commit the bootstrap environment.

**Acceptance:** A new developer or CI runner can recreate the Rust development environment using Mise and the documented native system dependencies.

### Task M00.2 — GPUI Kit Editor Spike

**Files**
- Create: `apps/desktop/src/spikes/editor.rs`
- Create: `docs/technical/editor-spike.md`

**Interfaces**
- Produces: a temporary editor prototype and an API capability report.

- [x] Open a GPUI window containing the GPUI Kit Editor.
- [x] Verify Markdown syntax highlighting.
- [x] Verify selection and cursor APIs.
- [x] Verify undo/redo and programmatic content updates.
- [x] Verify Chinese and Japanese IME composition.
- [x] Verify clipboard handling and keyboard shortcuts.
- [x] Test large-document responsiveness.
- [x] Record API limitations and a go/no-go recommendation.
- [x] Commit the results without promoting the spike to product architecture.

**Acceptance:** Critical editing operations work on Linux Wayland, or a documented replacement strategy is approved before M01.

### Task M00.3 — GPUI Kit Theme and Dock Spike

**Files**
- Create: `apps/desktop/src/spikes/layout.rs`
- Create: `docs/technical/design-system-spike.md`

- [x] Create a three-panel layout using GPUI Kit.
- [x] Test resizing and hiding panels.
- [x] Test centralized theme customization.
- [x] Test overlay/dialog behavior.
- [x] Test HiDPI and fractional scaling.
- [x] Record theme and component API limitations.
- [x] Commit verification results.

**Acceptance:** GPUI Kit can reproduce the intended Zed-inspired application shell without replacing major foundational controls.

**M00 acceptance update (2026-10-28):** All manual checks accepted by the user; original automated/manual evidence remains separately identified in `docs/technical/m00-progress.md`.

**M00 Gate:** Do not proceed to the main application implementation until the editor, layout and toolchain risks have explicit resolutions.

---

## 5. M01 — Application Shell and Design System

**Goal:** Build the first stable, navigable Astrolith window.

### Task M01.1 — Theme System

**Files**
- Create: `crates/astrolith-gpui/src/theme/mod.rs`
- Create: `crates/astrolith-gpui/src/theme/tokens.rs`
- Create: `crates/astrolith-gpui/src/theme/dark.rs`
- Create: `crates/astrolith-gpui/src/theme/light.rs`
- Test: `crates/astrolith-gpui/src/theme/tests.rs`

**Interface**

```rust
pub enum ThemeMode {
    Light,
    Dark,
    System,
}
```

- [x] Test semantic token completeness.
- [x] Test system theme resolution.
- [x] Implement the GPUI Kit theme adapter.
- [x] Verify both light and dark renderings.
- [x] Commit.

### Task M01.2 — Native Layout

**Files**
- Create: `crates/astrolith-gpui/src/layout/mod.rs`
- Create: `crates/astrolith-gpui/src/layout/state.rs`
- Create: `crates/astrolith-gpui/src/layout/panels.rs`

**Interface**

```rust
pub struct LayoutState {
    pub sidebar_visible: bool,
    pub inspector_visible: bool,
    pub preview_visible: bool,
    pub sidebar_width: f32,
    pub inspector_width: f32,
}
```

- [x] Test panel visibility state transitions.
- [x] Implement GPUI Kit docking layout.
- [x] Add persistence for layout settings.
- [x] Verify restoration after restart.
- [x] Commit.

Restoration evidence covers disk round trips and reconstructed headless windows. Native graceful close/reopen persistence remains separate platform acceptance; see `docs/technical/m01-progress.md`.

### Task M01.3 — Commands and Navigation

Implement an application command registry and Command Palette.

- [x] Share command identities between toolbar, keyboard shortcuts, and searchable palette.
- [x] Open local UTF-8 files as readonly snapshots using a native picker and background I/O.
- [x] Verify command filtering/confirmation, panel toggles, document focus, cancellation and focus restoration.
- [x] Run `mise run check` and commit.

M01 file opening is readonly. Editable document sessions and Astro discovery remain M02 responsibilities. See `docs/technical/m01-authority.md`.

**Acceptance:** Keyboard navigation opens files, toggles panels and invokes registered application commands without coupling commands to individual widgets.

---

## 6. M02 — Astro Workspace and Markdown Editor

**Goal:** Open a real Astro project and edit its local Markdown content.

### Task M02.1 — Domain IDs and Models

**Files**
- Create: `crates/astrolith-core/src/project.rs`
- Create: `crates/astrolith-core/src/entry.rs`
- Create: `crates/astrolith-core/src/collection.rs`

**Interfaces**

```rust
pub struct EntryId {
    pub collection: String,
    pub id: String,
}

pub struct ProjectId(pub String);
```

**Tests:** Stable ID handling, invalid paths and collection identity.

- [x] Add a dependency-free core crate with validated project, collection and entry identities.
- [x] Preserve logical entry identity separately from explicit source-file mappings (review R2).
- [x] Test custom IDs, overlapping collections, invalid paths and stable identity.
- [x] Run `mise run check`, complete independent review and commit.

See `docs/technical/m02-progress.md` for the model contract, verification and remaining M02 scope.

### Task M02.2 — Project Discovery

**Prerequisite: M02.R — Runtime and Responsiveness**

- [x] Add and test the application-owned Tokio/GPUI bridge, including queued cancellation and blocking concurrency after cancellation.
- [x] Measure the existing readonly file-to-frame path and debug/release editor workloads before changing behavior.
- [x] Remove redundant readonly full-buffer replacement work, coalesce preference writes, and optimize measured development dependency hotspots.
- [x] Record before/after CPU measurements, run `mise run check`, and perform a bounded native smoke. Native IME, scaling and latency were explicitly accepted by the user on 2026-10-29; numerical native measurements remain separate evidence.
- [x] Commit the repair and retain the original review documents.

M02.2 discovery must use the runtime/executor contract above and expose cancellation and obsolete-project rejection.

**Files**
- Create: `crates/astrolith-astro/src/discovery.rs`
- Create: `crates/astrolith-astro/src/config.rs`

**Interface**

```rust
pub async fn discover_project(
    root: &Path,
) -> Result<ProjectManifest, AstroError>;
```

**Tests:** Valid Astro project, absent config, legacy config, unsupported loader and malformed package metadata.

### Task M02.3 — Collection Index

**Files**
- Create: `crates/astrolith-workspace/src/index.rs`
- Create: `crates/astrolith-workspace/src/watcher.rs`

**Interfaces**

```rust
pub async fn scan_collections(
    project: &ProjectManifest,
) -> Result<Vec<CollectionEntry>, WorkspaceError>;
```

**Tests:** Nested directories, duplicate paths, ignored files, create/delete/rename events and path traversal.

Also test bounded scan admission, watcher burst merging/overflow rescan, duplicate source-load coalescing and results arriving after project switch. Limits and ownership are defined in the concurrency amendment.

### Task M02.4 — Document Editing Integration

**Files**
- Create: `crates/astrolith-gpui/src/editor/view.rs`
- Create: `crates/astrolith-gpui/src/editor/session.rs`
- Create: `crates/astrolith-document/src/buffer.rs`

**Interface**

```rust
pub struct DocumentRevision(pub u64);
```

**Tests:** Text edits increase revision, opening a different document restores the intended buffer, and undo/redo does not corrupt dirty-state tracking.

Keep mutable sessions retained across tab switches. Test out-of-order load/parse results and project/document/revision validation. The M01 readonly snapshot initialization optimization is not an editable-session refresh strategy.

**M02 Gate: complete (2026-10-29).** Real installed Astro 5.18.2 and 7.4.4 fixtures open, index and edit through the retained GPUI workflow. Saving remains M04. See `docs/technical/m02-progress.md` for actual checks, compatibility limits and separate native acceptance.

---

## 7. M03 — Schema-Driven Frontmatter

**Goal:** Use Astro-generated schemas to produce editable and validated native forms.

### Task M03.1 — JSON Schema Normalization

**Files**
- Create: `crates/astrolith-astro/src/schema/parser.rs`
- Create: `crates/astrolith-astro/src/schema/model.rs`
- Create: `crates/astrolith-astro/src/schema/tests.rs`

**Interface**

```rust
pub fn normalize_schema(
    source: &serde_json::Value,
) -> Result<CollectionSchema, SchemaError>;
```

**Tests:** Astro 5/6/7 generated schemas; nullable fields, date, defaults, enum, nested objects, arrays and unsupported unions.

**M03.1 update (2026-10-30):** The pure normalizer is implemented and tested against real generated Astro 5.18.2 (draft-07 `$ref`/`definitions`) and 7.4.4 (draft 2020-12) schema output; the two dialects normalize to one identical model. Astro 6 remains unverified (no fixture). The review-R1 generation service is now implemented in `schema/generation.rs`: it resolves the project-local Astro CLI, runs `astro sync` under a timeout/cancellation/bounded-output policy, and discovers, loads and normalizes `.astro/collections/*.schema.json` with a freshness check; it is verified end-to-end against both installed fixtures. Fixture schema authoring and shell/form wiring remain pending; per R3, editable forms (M03.3) remain blocked on the M04.2 save coordinator (the M04.1 lossless contract is complete). See `docs/technical/m03-progress.md`.

**Fixture schema oracles (2026-10-31):** Both `fixtures/astro5` and `fixtures/astro7` now declare a schema-bearing `docs` collection (Zod) whose sample entries satisfy the schema, and the `astro sync` output captured from each install is committed unchanged as a regression oracle at `crates/astrolith-astro/src/schema/fixtures/{astro5,astro7}-docs.schema.json`. The schema exercises strings, an optional string, a coerced date with default, a boolean, a string array, an enum, a nested object, a nullable nested number, a heterogeneous union, a literal and a record across both fixtures. The two oracles normalize to the identical dialect-independent model asserted in `schema/tests.rs`. `fixtures/README.md` documents regeneration. Committed as `b647a88`.

### Task M03.2 — Astro Helper Resolution

**Files**
- Create: `crates/astrolith-astro/src/schema/helpers.rs`

**Interface**

```rust
pub fn enhance_astro_fields(
    schema: &mut CollectionSchema,
    config_source: &str,
) -> Vec<SchemaDiagnostic>;
```

**Tests:** `image()`, `reference()`, nested fields, arrays and helper parsing failures.

**M03.2 update (2026-10-31):** `crates/astrolith-astro/src/schema/helpers.rs` implements `enhance_astro_fields(&mut CollectionSchema, config_source) -> Vec<SchemaDiagnostic>`. It parses the content config with the vendored tree-sitter TypeScript grammar (never executes project code) and refines `image()` → `FieldKind::Image` (Astro emits a plain string) and `reference("collection")` → `FieldKind::Reference { collections }` (Astro emits a heterogeneous union that would otherwise be `Unsupported`). It handles nested `object()`/`array()`/`optional()`/`nullable()`/`default()` wrappers, imported aliases, the `({ image }) => z.object(...)` schema-function form and local `const` schemas; it selects the matching collection by top-level field names, surfaces malformed configs as non-fatal diagnostics, and never silently string-coerces a field. Twelve unit tests pass. **Wired (P1 repair, `b676845`):** `load_generated_schemas` now reads the project content config (honouring `srcDir`) and applies `enhance_astro_fields` to every generated collection before the shell injects schemas, so `image()`/`reference()` refine end-to-end. Missing/malformed config is non-fatal (schemas still returned, a diagnostic added, no panic or string coercion); a wiring/end-to-end test (`generate_applies_astro_helper_resolution_end_to_end`) proves `Image` and `Reference` reach `generate_collection_schemas`. See `docs/technical/m03-progress.md`.

### Task M03.3 — Frontmatter Controls

**Files**
- Create: `crates/astrolith-gpui/src/frontmatter/mod.rs`
- Create: `crates/astrolith-gpui/src/frontmatter/fields.rs`

**Tests:** Field type mapping, editing, validation feedback, unsupported fields and read-only fallback.

**M03.3 update (2026-10-31):** `crates/astrolith-gpui/src/frontmatter/{fields.rs,mod.rs}` implements schema-driven forms. `fields.rs` maps every `FieldKind` to a native control family and applies only supported scalar-leaf edits through the M04.1 `ParsedDocument::set_field` patch contract (byte-exact preservation of untouched regions). `mod.rs` provides the GUI-free `FrontmatterEditor` (parse, edit, re-serialize, re-parse, local validation feedback) and the `FrontmatterForm` GPUI entity (GPUI Kit `Editor`/`Checkbox`/`Button`), with explicit read-only refusal for arrays, records, literals, objects-as-containers and unsupported unions (never coerced to text). The Inspector panel mounts the form; schema-driven edits flow through the existing save coordinator with no filesystem work on the UI thread. Fifteen form tests plus a shell wiring test pass. **P1 fix (`fddbd34`):** a field edit now emits a targeted `(path, value)` change and the shell patches the *live* session text via `set_field`, so a concurrent unsaved body edit is preserved byte-for-byte; the regression test `frontmatter_form_field_edit_preserves_concurrent_body_edit` edits the body then a field and asserts the body survives and the combined result saves. Native form acceptance is still pending.

### Task M03.4 — Validation Service

Implement basic field checks and Astro tooling validation as separate operations.

**M03.4 update (2026-10-31):** `crates/astrolith-astro/src/schema/{validation.rs,validation_tooling.rs}` implements two deliberately separate operations. (1) `validate_frontmatter(schema, &Value) -> Vec<ValidationDiagnostic>` is pure/fs-free/GUI-free and checks required, string/email format, number/integer/boolean, date, enum, literal, nested objects, arrays, records, nullable, image, reference, and warns on unsupported fields; diagnostics are structured (path, message, severity, source) and sorted. Date checking now rejects impossible calendar dates (days-per-month + Gregorian leap years; `b676845`), while accepting valid leap days. (2) `validate_with_astro_tooling(request)` reuses the R1 executable resolution and `run_astro_tool` to run `astro sync`, maps stderr error blocks to revision-tagged diagnostics with project-relative locations, and treats missing install as `Unavailable` and timeouts/cancellation as errors; a stale result is rejected via `reject_if_stale`/`accept_for_revision`, and `validate_with_astro_tooling_async` runs it on the blocking lane. Limits documented: `astro sync` validates on-disk content (not unsaved buffers), reference existence is deferred to `getEntry`, and `.astro` type checking needs `astro check`. The local operation is wired into the form's live validation; the tooling operation is implemented but not yet surfaced in the UI. Twenty-two new tests plus a calendar-date test pass.

**M03 Gate:** Common Frontmatter types work, invalid data is diagnosed, and unsupported types are not silently serialized as strings.

**M03 Gate status (2026-10-31, headless — both P1s closed):** Met in the model and form layers — common types map to native controls, invalid data is diagnosed locally, and unsupported unions/arrays/records are surfaced read-only with an explicit refusal rather than coerced to strings. After the repairs the helper pass is wired (`b676845`), so `image()`/`reference()` refine end-to-end, and the form-overwrite P1 is fixed (`fddbd34`), so editing a field preserves concurrent unsaved body edits. Still open (P2, not gate-blocking): Astro-tooling validation is not surfaced in the UI; the helper collection-selection heuristic and per-keystroke whole-document re-parse remain (see `docs/technical/m03-progress.md`). Native acceptance of the form remains **pending human execution**.


---

## 8. M04 — Persistence and Data Safety

**Goal:** Make the editor safe for real writing.

### Task M04.1 — Lossless Document Model

**Files**
- Create: `crates/astrolith-document/src/parse.rs`
- Create: `crates/astrolith-document/src/serialize.rs`
- Create: `crates/astrolith-document/src/tests.rs`

**Interfaces**

```rust
pub fn parse_document(
    source: &str,
) -> Result<ParsedDocument, DocumentError>;

pub fn serialize_document(
    document: &ParsedDocument,
) -> Result<String, DocumentError>;
```

**Tests:** Empty files, CRLF/LF, YAML comments, unknown fields, Unicode, trailing newlines and unchanged round-trip equality.

**M04.1 update (2026-10-31):** Lossless model implemented in `crates/astrolith-document/src/{parse.rs,serialize.rs}`. `parse_document` keeps the exact source and records byte spans for scalable block-mapping plain/quoted scalars (including nested mappings); `serialize_document` returns the original bytes unchanged when there are no patches and splices only patched spans otherwise, so comments, unknown keys, block scalars, Unicode, CRLF/LF and trailing newlines are preserved by construction. Flow collections and block scalars are readable but not patchable; anchors/aliases/tags, sequences, quoted/complex keys, tab indentation, duplicate keys and malformed YAML are refused (never destructively re-serialized). `set_field(path, value)` provides the scalar field-patch contract required by review R3, with plain-string safety validated against a real YAML re-parse so a patch cannot corrupt the file. 22 new tests pass; still pending before forms: the M04.2 save coordinator and M04.3 recovery. See `docs/technical/m04-progress.md`.

### Task M04.2 — Save Coordinator

**Files**
- Create: `crates/astrolith-document/src/save.rs`
- Create: `crates/astrolith-document/src/version.rs`

**Interfaces**

```rust
pub enum SaveOutcome {
    Saved { revision: u64 },
    Conflict,
}

pub async fn save_snapshot(
    snapshot: &DocumentSnapshot,
) -> Result<SaveOutcome, DocumentError>;
```

**Tests:** Concurrent edits, external modification, interrupted writes, stale revisions, symlink replacement and I/O failure.

Use one writer per document and immutable snapshots. Test exit while save N is running and revision N+1 remains dirty, cancellation during admission, and bounded shutdown without abandoning document-integrity operations.

**M04.2 update (2026-10-31):** Save coordinator implemented in `crates/astrolith-document/src/{save.rs,version.rs}`. `DocumentSnapshot` carries path/identity/revision/content/`DiskBaseline`; `save_snapshot` compares on-disk bytes to the baseline (changed or deleted file → `SaveOutcome::Conflict`, untouched) and otherwise writes atomically (same-dir `create_new` temp, `sync_all`, permission preserve, `rename`, best-effort dir fsync). Saves for one path are serialized process-wide (one writer), and I/O runs in `spawn_blocking`. `DocumentBuffer::confirm_saved` clears dirty only when the saved revision is still current, so saving N while editing N+1 keeps N+1 dirty. The residual check-to-replace race is documented. Eleven new tests pass. The broader dirty-lifecycle coordinator (close/switch/exit/reload) and unsaved-content recovery remain M04.3 / shell scope. See `docs/technical/m04-progress.md`.

### Task M04.3 — Recovery

**Files**
- Create: `crates/astrolith-document/src/recovery.rs`

**Tests:** Unsaved-content recovery, invalid recovery record, missing original file and recovery cleanup after confirmed successful save.

**M04.3 update (2026-10-31):** Recovery store implemented in `crates/astrolith-document/src/recovery.rs`. `RecoveryRecord` captures identity (project + generation + canonical source), revision, unsaved content and timestamp; it stores no absolute path (recovery resolves the validated relative source against a caller project root). `RecoveryStore` persists records atomically to a stable, generation-independent key with per-document write serialization, loads them tolerantly (corrupt files become diagnostics, missing originals remain recoverable), and `clear_after_save` removes a record only when its durable content equals the saved content (so newer unsaved edits are kept across restarts). Ten new tests pass. When to write/prompt recovery remains shell/lifecycle scope. See `docs/technical/m04-progress.md`.

**Shell save/recovery integration (2026-10-31):** `astrolith-gpui` now exposes a Save command (Ctrl/Cmd+S, palette, button) that snapshots the active buffer, runs `save_snapshot` off the UI thread, confirms the saved revision and clears the recovery record on success, and reports a conflict without overwriting on failure. Dirty edits are captured to the `RecoveryStore` after a 500 ms debounce and cleared when clean/saved/discarded; a `Recover unsaved changes…` command restores matching records as dirty sessions with disk reads off the UI thread. Four headless tests pass. Remaining lifecycle polish (startup auto-apply, exit flush) is shell scope. See `docs/technical/m04-progress.md`.

**Recovery lifecycle polish (2026-10-31):** On project open `load_pending_recovery` filters records by project id + generation and now **auto-applies** them via `recover_pending` as dirty sessions (disk is only read as the buffer baseline; nothing is written; the manual "Recover unsaved changes" command stays idempotent and reports nothing to restore on a repeat). `start()` clears stale pending records when switching projects. On native exit, `ProjectWorkspace::flush_recovery_on_quit` snapshots every dirty session's editor text on the UI thread and submits a durable write through `runtime::flush_on_quit` (spawn_blocking outside ordinary file admission); `shell.rs` registers an `App::on_app_quit` observer that runs before windows are cleared, so content survives exit even when the 500 ms debounce has not fired. Reads/writes stay off the UI thread and conflict/discard semantics are unchanged. Two headless tests (`startup_auto_applies_pending_recovery_and_never_overwrites_disk`, `quitting_flushes_unsaved_debounced_content_to_recovery`) pass. Native exit-flush evidence is pending (see `docs/technical/native-acceptance-save-recovery.md`). Committed as `9692c94`.

**M04 Gate:** No known path silently destroys unsaved content or overwrites conflicting disk changes.

**M04 Gate status (2026-10-31, headless):** The document core (lossless patch, conflict-aware atomic save, recovery store) and the shell save/recovery/exit-flush paths are covered by headless tests and `mise run check`. The previously-open cross-surface risk — the M03.3 form overwriting an unsaved body edit — is now **fixed (`fddbd34`)**: field edits patch the live document text and a regression test proves a concurrent unsaved body edit survives. No known P1 path silently destroys unsaved content or overwrites conflicting disk changes remains headlessly. **Native save/conflict/recovery/exit evidence remains pending human execution** (`docs/technical/native-acceptance-save-recovery.md`); headless tests do not substitute for it.

---

## 9. M05 — Astro Preview Services

**Goal:** Start and manage the actual Astro development server.

### Task M05.1 — Dev Server Lifecycle

**M05.1 update (2026-10-31):** Implemented in `crates/astrolith-preview/src/{server.rs,process.rs}` (commit `fca3fcd`): `ServerStatus`, project-local Astro CLI resolution, 30 s startup bound, 3 s graceful stop then terminate+reap, both drains capped at 1 MiB per stream, Unix process-group kill and a `cfg`-gated Windows Job Object. 20 tests including a grandchild-in-group kill and a 4 MiB flood. Windows ownership is unverified on this host. See `docs/technical/m05-progress.md`.

**Files**
- Create: `crates/astrolith-preview/src/server.rs`
- Create: `crates/astrolith-preview/src/process.rs`

**Interfaces**

```rust
pub enum ServerStatus {
    Stopped,
    Starting,
    Ready(url::Url),
    Stopping,
    Failed(String),
}
```

**Tests:** Startup, failed command, port conflict, cancellation, process exit and child-process ownership.

Implement runtime-owned stdout/stderr draining with capped retention, startup/stop deadlines and platform process-tree cleanup. Test output flooding and project switch/exit without orphaned descendants; see the concurrency amendment for limits.

### Task M05.2 — Entry Route Resolver

**M05.2 update (2026-10-31):** Implemented as `resolve_entry_url(&EntryId, &RouteMapping, &Url)` with the `/<collection>/<entry id>` convention, explicit per-entry overrides, `index` folding, percent-encoded segments and typed errors for unmapped/invalid routes (commit `59e8544`, 20 tests).

**Files**
- Create: `crates/astrolith-preview/src/routes.rs`
- Create: `crates/astrolith-preview/src/config.rs`

**Interface**

```rust
pub fn resolve_entry_url(
    entry: &EntryId,
    mapping: &RouteMapping,
    base_url: &url::Url,
) -> Result<url::Url, RouteError>;
```

**Tests:** Slugs, nested paths, URL escaping, absent mapping and invalid routes.

### Task M05.3 — External Browser Preview

**M05.3 update (2026-10-31):** `PreviewSurface`/`RecordingSurface`, a detached platform-opener `ExternalBrowser`, and `PreviewSession` server reuse (commit `2c8c562`); fixtures gained a `docs` page so entry routes render; an opt-in gate test starts the real `astro dev`, saves an edit and observes it in the served HTML (commit `365bcbe`); the GPUI Preview panel is wired with Start/Stop/Open commands, status, project-switch detach and quit abort (commit `1ba5ed3`). Native browser launch and rendering remain manual evidence.

**Files**
- Create: `crates/astrolith-preview/src/surface.rs`
- Create: `crates/astrolith-preview/src/external.rs`

**Tests:** Browser launch, invalid URL, failure feedback, external server reuse and saving-triggered HMR integration.

**M05 Gate:** Editing an article and saving it results in an updated real Astro preview page.

---

## 10. M06 — Hybrid Preview and Platform Integration

**Goal:** Enable platform-dependent embedded preview without making it mandatory.

### Task M06.1 — Preview Capability Detection

**Files**
- Create: `crates/astrolith-preview/src/capabilities.rs`

**Interface**

```rust
pub enum PreviewCapability {
    Embedded,
    ExternalOnly,
}
```

**Tests:** Linux Wayland, X11, macOS, Windows and an unavailable WebView dependency.

**M06.1 update (2026-10-31):** Implemented in `crates/astrolith-preview/src/capabilities.rs` (commit pending in `docs/technical/m06-progress.md`). `PreviewCapability::{Embedded, ExternalOnly}` is classified from an injectable `HostEnvironment { platform, webview_available }`, so every branch is tested without mutating process state or the target triple. Linux Wayland (including XWayland, where `WAYLAND_DISPLAY` and `DISPLAY` are both set), headless Linux, unknown targets and builds without the `embedded-webview` feature are external-only; Linux X11, macOS and Windows offer embedded only when that feature is compiled in. Session detection reads `WAYLAND_DISPLAY`, `DISPLAY` and `XDG_SESSION_TYPE` through a lookup function. `PreviewCapability::detect()` is the startup entry point. 14 focused tests pass.

### Task M06.2 — Embedded WebView Spike

**Files**
- Create: `crates/astrolith-gpui/src/preview/embedded.rs`
- Create: `docs/technical/webview-spike.md`

**Tests:** Startup, navigation, resizing, scaling, overlays, renderer compatibility and graceful failure.

This task remains experimental until real-platform tests pass. It must not delay the external-preview MVP.

### Task M06.3 — Platform Integration

**Files**
- Create: `crates/astrolith-gpui/src/platform/mod.rs`
- Create: platform-specific modules where required.

**Tests:** Native menus, shortcuts, file dialogs, system browser opening, display scaling and window lifecycle.

---

## 11. M07 — Alpha Release

**Goal:** Deliver a reproducible, usable Alpha build.

### Task M07.1 — CI Matrix

- [ ] Linux build and unit tests.
- [ ] Windows build and unit tests.
- [ ] macOS build and unit tests.
- [ ] Native GUI smoke tests on supported runners.
- [ ] Cargo formatting and Clippy checks.
- [ ] Astro fixture integration tests.
- [ ] Artifact packaging validation.

### Task M07.2 — Documentation

- [ ] Write installation instructions.
- [ ] Document Mise setup and task commands.
- [ ] Document Astro compatibility limits.
- [ ] Document data safety and recovery behavior.
- [ ] Document preview configuration.
- [ ] Publish known platform limitations.

### Task M07.3 — Release Gate

- [ ] All P0 tests pass.
- [ ] No known silent data loss.
- [ ] Linux Wayland smoke test passes.
- [ ] Astro project fixture suite passes.
- [ ] External browser preview works.
- [ ] Installable artifact starts on supported target systems.
- [ ] Document unsupported features and remaining defects.

---

## 12. Review Focus

The following user-visible failure cases require explicit regression tests in their owning milestones.

1. **External file change during an unsaved edit:** the user's buffer remains intact and a conflict is reported. Covered by M04.2.
2. **Astro schema with an unsupported union or nullable object:** editing must not silently coerce the value into a string. Covered by M03.1 and M03.3.
3. **IME composition while editor content refreshes:** Chinese/Japanese composition must not be prematurely committed or erased. Covered by M00.2 and M02.4.
4. **Preview server startup failure or occupied port:** the editor remains usable and reports a recoverable error. Covered by M05.1.
5. **Linux Wayland with no embedded WebView:** external preview works without switching the entire application to XWayland. Covered by M06.1 and M05.3.

---

## 13. Superpowers Execution Protocol

Each implementation task follows:

1. Read the relevant design specification.
2. Inspect current interfaces and existing tests.
3. Write the smallest meaningful failing test.
4. Run the test and verify the intended failure.
5. Implement the minimal change.
6. Run the focused test and relevant regression tests.
7. Run formatting and lint checks.
8. Request independent code review.
9. Fix verified review findings.
10. Commit the completed task.

At each milestone boundary:

- Verify all tasks are completed.
- Run `mise run check`.
- Review architectural boundary violations.
- Record new technical debt.
- Update the design specification when a meaningful decision changes.
- Proceed only if the milestone acceptance gate passes.

## 14. Execution Recommendation

Use **Superpowers subagent-driven development** for M01 onward where independent tasks and review gates provide value.

Use focused technical spikes for GPUI Editor, Dock, IME and Embedded WebView feasibility.

Start implementation with **M00.1 — Mise and Rust Workspace**, then M00.2 and M00.3.

No application code should be implemented before this plan is approved.

---

**End of Implementation Plan v1.1**
