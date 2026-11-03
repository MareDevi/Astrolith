# Astrolith

Astrolith is a native Rust/GPUI Kit editor for [Astro](https://astro.build)
content collections on Linux. It opens an Astro project, lists its collections
and entries, edits Markdown, MDX and YAML sources, renders schema-driven
frontmatter forms, builds MDX component tags from static prop metadata, and
saves with conflict detection and durable recovery.

**Status: Linux alpha.** The feature-parity implementation is complete with
headless evidence. Native Wayland evidence for some interactions, packaged
artifact checks and release signing are still outstanding; see
[Linux limits and known gaps](docs/user/guide.md#14-linux-limits-and-known-gaps)
before relying on a feature for production work. macOS and Windows code paths
exist but are neither delivered nor supported.

## Features

- **Projects and collections** — recent-project registry and reopen, Astro 5/6/7
  discovery for Markdown, MDX and YAML collections, generated-schema
  integration, nested collections, diagnostics for configuration it cannot
  prove statically, and a bounded filesystem watcher.
- **Entries** — create with schema-required frontmatter, duplicate byte-exact,
  rename/move with retargeting, move to the desktop trash, reveal the containing
  directory, and copy absolute or project-relative paths.
- **Navigation** — collection counts, breadcrumbs, draft filter, case-insensitive
  title/filename filter, schema-aware sort modes, and a Content Finder that can
  open entries or insert links.
- **Writer** — lossless frontmatter/MDX-import projection over complete source,
  native find/replace, go-to-line, focus and typewriter modes, Markdown
  presentation, Copyedit highlighting, formatting commands, and word/character
  counts.
- **Frontmatter** — schema-driven typed controls, references, images, validated
  raw-YAML fallback, lossless edits that preserve comments, unknown fields,
  quoting and CRLF, and refresh when the schema changes.
- **MDX components** — static discovery of Astro/React/Vue/Svelte components,
  typed prop forms, tracked placeholders, Starlight scaffolds, and opt-in
  import insertion with collision-safe planning.
- **Safe persistence** — atomic conflict-aware saves, autosave (1/2/5/10 s,
  default 2 s) with a 10 s continuous-typing checkpoint, dirty close/switch/quit
  prompts, and in-app recovery of unsaved edits.
- **Preview** — project-local Astro dev server with start/stop/restart/retry,
  route tracking and external-browser presentation (the only Linux surface).
- **Preferences** — global/project/collection overrides with one resolver,
  advanced versions and scoped Reset All.
- **Desktop integration** — application menus from one command registry,
  keyboard shortcuts, command palette, IDE argv launch, `astrolith://open?path=`
  scheme handling, MIME associations in packages, and single-instance launch.
- **Privacy** — no telemetry. The only Astrolith network request is the signed
  update manifest in builds configured with a manifest URL and public key.

## Documentation

- [User guide](docs/user/guide.md)
- [Keyboard shortcuts](docs/user/keyboard-shortcuts.md)
- [Privacy and local data](docs/user/privacy.md)
- [Development and packaging](docs/technical/development.md)
- [Packaging notes](packaging/linux/README.md)
- [Astro Editor parity ledger and final audit](docs/technical/astro-editor-parity.md)
- [Editor capability contract](docs/technical/editor-capabilities.md)

The user guide, a shortcut reference generated from the live command registry,
and the local-paths inventory are also available in the application's **Help**
menu.

## Build from source

Mise owns the toolchain and task entry points.

```sh
mise install
mise run doctor
mise run build
mise run dev
```

If Mise requests trust, review `mise.toml` and run `mise trust` in this
directory. Installation needs network access. Pinned versions: Rust 1.99.1
(with rust-analyzer, rust-src, clippy and rustfmt), Node.js 26.12.1 and Bun
1.4.4. `Cargo.lock` is committed; Cargo tasks use `--locked`.

### Linux prerequisites

Mise does not install system development packages or GPU drivers. GPUI Kit
needs a C/C++ linker, clang, cmake, pkg-config, fontconfig, Wayland/X11
headers, OpenSSL, zstd and a Vulkan loader.

Ubuntu 24.04:

```sh
sudo apt-get update
sudo apt-get install -y gcc g++ clang cmake pkg-config libfontconfig-dev \
  libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev \
  libvulkan-dev mesa-vulkan-drivers
```

Arch/CachyOS:

```sh
sudo pacman -S --needed base-devel clang cmake pkgconf fontconfig wayland \
  libxkbcommon libxkbcommon-x11 libx11 libxcb openssl zstd vulkan-headers \
  vulkan-icd-loader
```

Install the Vulkan driver for your GPU (for example `vulkan-radeon`,
`vulkan-intel`, or the distribution's NVIDIA driver) and launch from a
graphical Wayland or X11 session. No WebKitGTK or ALSA development packages are
required; preview uses your external browser.

### Packages

AppImage, `.deb` and `.rpm` recipes exist:

```sh
mise run package:metadata
mise run package:validate
mise run package:deb       # or package:appimage / package:rpm
```

Artifacts land in `dist/`. No artifact has completed a clean-environment
install, launch, association, protocol and uninstall run yet, and no signed
release manifest exists. See [packaging notes](packaging/linux/README.md) and
[development and packaging](docs/technical/development.md#5-packaging-and-update-signing).

## Development

`mise run check` composes doctor, formatting, lint, build, headless tests and
Astro fixture verification; it is the commit gate. `mise run ci` runs the same
composition in CI. Architecture boundaries, focused tasks, performance workloads,
test strategy and the real-platform gate list are in
[development and packaging](docs/technical/development.md).

Headless checks do not establish native IME, focus, display scaling, rendering,
drag-and-drop, browser-launch or packaging behavior. Those remain explicit
real-platform evidence and are tracked in the parity audit.

## License

This repository declares no license yet. The RPM metadata therefore says
`UNLICENSED`, and the AppStream metadata omits a project license until one is
chosen.
