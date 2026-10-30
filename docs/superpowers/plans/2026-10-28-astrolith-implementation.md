# Astrolith — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkboxes for tracking.

**Status:** M00 manually accepted by the user; M01 and all M02 tasks complete; M02.R runtime/responsiveness repairs complete; native IME, scaling and latency manually accepted by the user on 2026-10-29; M03.1 JSON Schema normalization complete on 2026-10-30 (M03 schema generation, forms and validation pending; forms remain blocked on M04.1)
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

**M03.1 update (2026-10-30):** The pure normalizer is implemented and tested against real generated Astro 5.18.2 (draft-07 `$ref`/`definitions`) and 7.4.4 (draft 2020-12) schema output; the two dialects normalize to one identical model. Astro 6 remains unverified (no fixture). Per review R1, the `astro sync` generation service and fixture schema authoring are still required before schemas feed forms; per R3, editable forms (M03.3) remain blocked on the M04.1 lossless document/field-patch contract. See `docs/technical/m03-progress.md`.

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

### Task M03.3 — Frontmatter Controls

**Files**
- Create: `crates/astrolith-gpui/src/frontmatter/mod.rs`
- Create: `crates/astrolith-gpui/src/frontmatter/fields.rs`

**Tests:** Field type mapping, editing, validation feedback, unsupported fields and read-only fallback.

### Task M03.4 — Validation Service

Implement basic field checks and Astro tooling validation as separate operations.

**M03 Gate:** Common Frontmatter types work, invalid data is diagnosed, and unsupported types are not silently serialized as strings.

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

### Task M04.3 — Recovery

**Files**
- Create: `crates/astrolith-document/src/recovery.rs`

**Tests:** Unsaved-content recovery, invalid recovery record, missing original file and recovery cleanup after confirmed successful save.

**M04 Gate:** No known path silently destroys unsaved content or overwrites conflicting disk changes.

---

## 9. M05 — Astro Preview Services

**Goal:** Start and manage the actual Astro development server.

### Task M05.1 — Dev Server Lifecycle

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
