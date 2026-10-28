# Astrolith — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkboxes for tracking.

**Status:** In progress — M00 feasibility; M01 gate remains open
**Version:** 1.0  
**Date:** 2026-10-28  
**Project:** Astrolith  
**Methodology:** Superpowers — TDD, task-based execution, review gates

**Goal:** Build a cross-platform, native Astro Content Collections editor with reliable Markdown/MDX editing, schema-driven metadata forms, safe document persistence, and real Astro live preview.

**Architecture:** Rust modular monolith with a GPUI Kit desktop shell, independent domain and document services, an Astro tooling adapter, and a replaceable preview surface.

**Tech Stack:** Rust, GPUI Kit 0.8.1 (initial candidate), Cargo, Mise, Node.js, Bun, Tree-sitter, Astro CLI, Wry-based experimental WebView.

**Spec:** `docs/superpowers/specs/2026-10-28-astrolith-design.md`

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
- [ ] Verify Markdown syntax highlighting.
- [x] Verify selection and cursor APIs.
- [x] Verify undo/redo and programmatic content updates.
- [ ] Verify Chinese and Japanese IME composition.
- [ ] Verify clipboard handling and keyboard shortcuts.
- [ ] Test large-document responsiveness.
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
- [ ] Test HiDPI and fractional scaling.
- [x] Record theme and component API limitations.
- [x] Commit verification results.

**Acceptance:** GPUI Kit can reproduce the intended Zed-inspired application shell without replacing major foundational controls.

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

- [ ] Test semantic token completeness.
- [ ] Test system theme resolution.
- [ ] Implement the GPUI Kit theme adapter.
- [ ] Verify both light and dark renderings.
- [ ] Commit.

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

- [ ] Test panel visibility state transitions.
- [ ] Implement GPUI Kit docking layout.
- [ ] Add persistence for layout settings.
- [ ] Verify restoration after restart.
- [ ] Commit.

### Task M01.3 — Commands and Navigation

Implement an application command registry and Command Palette.

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

### Task M02.2 — Project Discovery

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

**M02 Gate:** A real Astro project can be opened; its local Markdown files can be discovered and edited.

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

**End of Implementation Plan v1.0**
