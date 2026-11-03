# Development and packaging

This is the developer entry point for Astrolith. The frozen upstream behavior
inventory and per-row audit live in
[astro-editor-parity.md](astro-editor-parity.md); editor API capability status
lives in [editor-capabilities.md](editor-capabilities.md).

## 1. Toolchain and setup

Mise owns every project tool version and task entry point. Do not install
project versions globally or add a competing version file.

```sh
mise install
mise run doctor
mise run build
mise run dev
```

If Mise requests trust, review `mise.toml` and run `mise trust` in this
directory. Tool installation needs network access. Versions are pinned in
`mise.toml`: Rust 1.99.1 with rust-analyzer, rust-src, clippy and rustfmt;
Node.js 26.12.1; Bun 1.4.4; Mise 2026.10.19 is the tested runner and CI
version.

Native prerequisites are not installed by Mise. GPUI Kit needs a C/C++ linker,
clang, cmake, pkg-config, fontconfig, Wayland/X11 development headers, OpenSSL,
zstd and a Vulkan loader plus a working GPU driver. The [README](../../README.md#build-from-source)
lists the Ubuntu and Arch package commands. Launch from a graphical Wayland or
X11 session. No JavaScript fixture installation is needed for the workspace
build; frozen Astro fixtures use project-local tooling with committed
lockfiles.

`Cargo.lock` is committed and every Cargo task uses `--locked`. A dependency
change includes its `Cargo.lock` update.

## 2. Tasks

`mise.toml` is authoritative. Current tasks:

| Task | Purpose |
| --- | --- |
| `doctor` | Check tool executables, lockfile metadata and native prerequisites. |
| `build` | Build the workspace with the lockfile. |
| `test` | Run all headless workspace tests. |
| `test:astro` | Focused static Astro discovery tests. |
| `test:document` | Focused retained-document, projection and formatting tests. |
| `test:gpui` | Focused headless GPUI tests. |
| `test:workspace` | Focused index, watcher and file-operation tests. |
| `test:fixtures` | Verify frozen Astro 5/6/7 fixtures with the real project-local loaders. |
| `fmt` / `fmt:write` | Check or apply Rust formatting. |
| `lint` | Clippy for every workspace target with warnings denied. |
| `check` | `doctor`, `fmt`, `lint`, `build`, `test`, `test:fixtures` in order. |
| `ci` | The same composition in CI. |
| `dev` / `dev:release` | Open the native window in the dev or release profile. |
| `lock:update` | Resolve declared dependencies into `Cargo.lock`. |
| `test:shell-perf[:release]` | Read-only file-to-frame workload. |
| `test:editor-perf[:dev]` | Bounded plain-editor workload. |
| `test:projection-perf[:release]` | Projection diff plus projected-writer typing workload. |
| `test:resize-perf[:release]` | Native window resize frames. |
| `package:metadata` | Generate desktop, AppStream and MIME metadata from the workspace version. |
| `package:validate` | Regenerate and validate metadata with `desktop-file-validate`, `appstreamcli` and the MIME validator. |
| `package:deb`, `package:appimage`, `package:rpm` | Build an artifact into `dist/`. |
| `dev:editor`, `dev:layout` | Retained disposable M00 probes (see §7). |

Never construct an ad-hoc workspace-wide lint, format or check command in place
of these tasks.

## 3. Architecture boundaries

Astrolith is a Rust modular monolith: a GPUI Kit desktop shell over independent
domain and service crates.

| Crate | Responsibility |
| --- | --- |
| `astrolith-core` | Pure domain types: scoped preferences, field mappings, IDE command, Copyedit preferences, theme and layout values, telemetry consent. No I/O. |
| `astrolith-document` | Parsed documents, lossless frontmatter patches, writer projection and visible-edit mapping, formatting transforms, Savepoint/atomic save, recovery store, word tokens, sentences, Copyedit classifier. |
| `astrolith-astro` | Static project inspection, generated-schema normalization, frontmatter validation helpers, MDX component discovery and prop extraction, Starlight catalog, import planning. Never executes project code. |
| `astrolith-workspace` | Project roots and containment, collection index, bounded watcher, entry create/duplicate/rename/trash/reveal/copy-path, asset copy and link arithmetic, filters and sort. Blocking filesystem work. |
| `astrolith-preview` | Project-local Astro dev-server lifecycle, route resolution, preview server generations, preview capability. |
| `astrolith-observability` | Logging bootstrap: stderr and JSON daily files, panic hook, rotation and redaction discipline. |
| `astrolith-gpui` | Native shell, panels, dialogs, command registry, preferences UI, editor presentation, autosave coordination, update client, desktop actions, URL/startup routing. |
| `apps/desktop` | Binary bootstrap: logging, spike dispatch, GPUI application and the main window. |

Binding rules:

- Rendering performs no filesystem work and no blocking I/O. Blocking reads,
  writes, scans and process work run on the service runtime's blocking lane.
- `astrolith-gpui/src/commands.rs` is the sole command registry. Menus, the
  palette, key bindings, toolbar and context menus consume the same command
  IDs, labels, shortcuts and availability rules.
- Domain logic, parsers, persistence and services are written test-first with
  `--locked`.
- Editors keep the complete source in `DocumentBuffer`; the writer displays a
  lossless projection. Saves, recovery and conflict checks operate on complete
  source.
- `gpui-kit = "=0.8.1"` is the only UI facade. File I/O and services stay out of
  the desktop bootstrap.

## 4. Test strategy

- **Domain and service tests:** normal Rust unit/integration tests in the owning
  crate. They cover invariants, boundaries and failure paths, not trivial glue.
- **Headless GPUI tests:** `#[gpui_kit::test]` exercises windows, dialogs,
  command dispatch, editor behaviour and rendering state on the GPUI test
  platform. They do not prove native IME, focus, scaling, pointer behavior,
  compositing or pixels.
- **Frozen fixtures:** `fixtures/` holds Astro 5/6/7 projects, schema oracles
  and real component sources. `mise run test:fixtures` runs the actual loaders.
- **Opt-in real-server gate:** a preview test drives a real `astro dev`, saves
  an edit and observes it in the served HTML. It is not part of the default
  headless suite.
- **Ignored performance workloads:** `--ignored` tests produce the measured
  numbers in [concurrency-and-responsiveness.md](concurrency-and-responsiveness.md)
  and the plan's P4 cost table. They are evidence runs, not pass/fail gates.
- **Documentation checks:** the GPUI help tests keep the in-app guide and
  shortcut reference aligned with the command registry and verify that
  repository documentation links resolve.

`mise run check` is the commit gate. It composes doctor, formatting, lint,
build, headless tests and fixture verification. CI runs the same composition on
Linux, macOS and Windows with the same Mise pins; hosted results are separate
evidence from local runs.

No headless test establishes a native rendering, input-method, focus, display
scaling or packaging result. Do not record those as accepted without a
real-platform run.

## 5. Packaging and update signing

One binary and one metadata source ship. Everything under
`packaging/linux/generated/` is produced from `templates/` plus the Cargo
workspace version; never edit or commit generated files.

```sh
mise run package:metadata
mise run package:validate
mise run package:deb
mise run package:appimage
mise run package:rpm
```

Build prerequisites and exact artifact contents are documented in
[packaging/linux/README.md](../../packaging/linux/README.md). In short: the
Debian package is assembled with `ar` and `tar`; AppImage needs `linuxdeploy`
and `appimagetool`; RPM needs `rpmbuild`. All scripts rebuild the release
binary and refuse to package when `astrolith --version` disagrees with the
generated metadata version.

Update checks are compiled in, never configured at runtime. A release build
needs both:

- `ASTROLITH_UPDATE_MANIFEST_URL` — HTTPS URL of the signed envelope.
- `ASTROLITH_UPDATE_PUBLIC_KEY` — base64 of the raw 32-byte Ed25519 public key.

Without both, the Check for updates command is unavailable and the About/update
surface explains that updates are not configured. `sign-manifest.mjs` signs a
payload with an Ed25519 key from `ASTROLITH_UPDATE_SIGNING_KEY` (base64 PKCS#8
private key or its 32-byte seed). The application verifies the signature, the
SHA-256 digest, the artifact platform and the version before offering an
external download; it never self-replaces.

Release workflow secrets and variables (documented, never committed):

| Name | Kind | Use |
| --- | --- | --- |
| `ASTROLITH_UPDATE_SIGNING_KEY` | secret | Ed25519 private key material. Absent means the workflow skips signing and publishes no manifest. |
| `ASTROLITH_UPDATE_MANIFEST_URL` | repository variable or build env | HTTPS manifest URL baked into the build. |
| `ASTROLITH_UPDATE_PUBLIC_KEY` | repository variable or build env | Base64 raw Ed25519 public key baked into the build. |
| `ASTROLITH_RELEASE_PAGE` | repository variable | HTTPS release page placed in the generated manifest. |

Test keys are generated inside the Rust tests and never persisted.

Packaging prerequisites remain open. There is no signing key or published
signed manifest in this repository, the icons are provisional, the AppStream
homepage is a placeholder, the repository has no license (the RPM spec says
`UNLICENSED`), and no artifact has completed a clean-environment install,
launch, file-association, protocol and uninstall run.

## 6. Real-platform gates

Recorded native evidence is Linux Wayland only. The following must be run on a
real session and recorded before claiming them:

- IME composition, focus, display scaling and rendering for the writer.
- Native menu rendering and every menu action by mouse and keyboard.
- File dialogs, file-manager reveal, context-menu pointer placement and IDE
  launch.
- Drag and drop into the editor and shell; image hover previews.
- Preview start/restart/failure output, switching projects with a running
  server, and mutating entries while the browser holds the preview.
- Close and quit prompts, save conflicts, recovery restore and read-only save
  failures.
- Protocol cold and warm launch, startup-file routing, and single-instance
  forwarding.
- Packaged artifact install, launch, MIME association, protocol registration
  and uninstall in clean Linux environments.
- A real signed manifest check by a packaged build.

Hosted CI results for macOS and Windows are not Linux evidence and do not
authorize delivery claims for those platforms.

## 7. Retained disposable probes

`apps/desktop/src/spikes/` and `mise run dev:editor` / `dev:layout` remain
because the application binary still dispatches them through `--spike=editor`
and `--spike=layout`, and the editor probe still covers the public GPUI facade
used by the writer. They are not part of the user-facing application surface,
their manual acceptance does not establish acceptance for later behavior, and
they should be deleted only together with that dispatch and the corresponding
tests. The P0 plain-editor workload in `astrolith-gpui` is a normal ignored
test, not a spike binary, and stays.

## 8. Documentation sources of truth

- `docs/user/guide.md` is embedded verbatim into the in-app Help → User guide
  surface; editing it changes both the repository page and the bundled text.
- `docs/user/keyboard-shortcuts.md` is checked against the live command
  registry by a GPUI help test.
- `docs/technical/astro-editor-parity.md` records the frozen upstream ledger
  and the P15 per-row audit.
- `docs/technical/editor-capabilities.md` is the only authority for GPUI editor
  command status.
- `packaging/linux/README.md` owns artifact and signing detail; this page
  links rather than restating commands it could drift from.
