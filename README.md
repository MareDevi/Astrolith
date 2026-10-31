# Astrolith

Astrolith is a native Rust/GPUI Kit application for the planned Astro Content Collections editor. M01 provides a persistent dock layout, system/light/dark appearance, a searchable command palette, and readonly local UTF-8 file navigation. M02 adds static Astro project discovery, a watched collection index, and retained editable Markdown/MDX sessions. Safe document saving and preview remain later milestones.

M00 manual acceptance was confirmed by the user. The original design specification remains absent; the user authorized the existing M01 plan as this stage's authority. See [the decision record](docs/technical/m01-authority.md) and [M01 verification](docs/technical/m01-progress.md).

## Development environment

Install [Mise](https://mise.jdx.dev/getting-started.html) and the native prerequisites below, then run from this repository:

```sh
mise install
mise run doctor
mise run check
mise run dev
```

If Mise requests trust, review `mise.toml` and use `mise trust` in this directory. Tool installation needs network access. The exact project versions are Rust **1.99.1** (with `rust-analyzer`, `rust-src`, `clippy`, and `rustfmt`), Node.js **26.12.1**, and Bun **1.4.4**. `mise.toml` is authoritative; tools are scoped to this project. There is no competing Rust/Node version file. Mise **2026.10.19** is the tested task runner and the CI version.

`Cargo.lock` fixes the full Rust dependency graph. The UI depends on the pinned `gpui-kit = "=0.8.1"` facade; its default assets/components and matching GPUI 0.3.10 platform snapshot are enabled. The shell also uses Serde/JSON and Tempfile for application preferences. No JavaScript fixtures or package installation are needed yet. Future Astro fixtures must use project-local Bun tooling and committed lockfiles.

The shell owns one Tokio service runtime; GPUI retains native UI and background CPU execution. Filesystem jobs use a bounded blocking lane, and preference changes coalesce for 250 ms before writing. Selected editor/layout/parser dependencies are optimized in development builds while application code keeps debug information and normal development compilation. See [concurrency boundaries and performance evidence](docs/technical/concurrency-and-responsiveness.md).

Logging writes human-readable records to stderr and structured JSON to daily-rotated files, from a non-blocking writer. See [logging configuration and event levels](docs/technical/logging.md).

### Native prerequisites

Mise does not install operating system development packages or GPU drivers. GPUI Kit's [installation guide](https://gpui-kit.com/docs/installation) describes platform requirements; the bootstrap uses the following build dependencies.

**Linux (Ubuntu 24.04):**

```sh
sudo apt-get update
sudo apt-get install -y gcc g++ clang cmake pkg-config libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev libvulkan-dev mesa-vulkan-drivers
```

On Arch/CachyOS, install the corresponding packages:

```sh
sudo pacman -S --needed base-devel clang cmake pkgconf fontconfig wayland libxkbcommon libxkbcommon-x11 libx11 libxcb openssl zstd vulkan-headers vulkan-icd-loader
```

Install the Vulkan driver for your GPU as well (for example `vulkan-radeon`, `vulkan-intel`, or the distribution's NVIDIA driver). Launch from a graphical Wayland or X11 session. A loader alone does not provide a working renderer. The default bootstrap does not enable a webview or speech; WebKitGTK and ALSA development packages are not required for this dependency graph. Add their native dependencies if those features are introduced. Normal Wayland preview will use an external browser.

**macOS 15 or newer:** install Xcode Command Line Tools with `xcode-select --install`, then check `xcode-select -p`. GPUI Kit enables `font-kit` and runtime shaders for native text/rendering. No embedded preview is required.

**Windows 10 or newer:** install Visual Studio 2022 Build Tools (or Community) with **Desktop development with C++**, MSVC, and a Windows SDK. Install CMake and make it available on `PATH`. Use the MSVC Rust toolchain. If Cargo cannot discover the compiler/linker, run Mise from a Visual Studio Developer PowerShell. Release builds also need the Windows SDK's `fxc.exe` shader compiler; the GPUI snapshot discovers it from the SDK or `GPUI_FXC_PATH`.

### Tasks and verification

| Command | Purpose |
| --- | --- |
| `mise run doctor` | Check tool executables, lockfile metadata, and platform build prerequisites; a compilation is still required to prove compatibility. |
| `mise run build` | Build `astrolith-desktop` with the lockfile. |
| `mise run test:fixtures` | Run the frozen Astro 5/6/7 fixtures and compare real loader IDs/source paths with the Rust index. |
| `mise run test` | Run headless workspace tests. Includes preference, command, readonly-source, and UI interaction tests. |
| `mise run fmt` | Check formatting without changing files. |
| `mise run lint` | Run Clippy for every workspace target with warnings denied. |
| `mise run check` | Run doctor, fmt, lint, build, and test in order; later milestones extend this gate. |
| `mise run ci` | Run the same check composition in CI. |
| `mise run dev` | Open the native desktop window. |
| `mise run test:shell-perf` | Measure readonly file read/validation, editor installation and first headless frame in the dev profile. |
| `mise run test:shell-perf:release` | Measure the same shell workloads in release. |
| `mise run test:editor-perf:dev` | Measure editor load/input/frame CPU work with dev dependency optimizations. |
| `mise run test:editor-perf` | Measure the disposable editor workloads in release. |

The CI workflow configures Linux, macOS, and Windows builds/tests with the same Mise pins. Hosted runner results are separate evidence from local verification. Headless checks do not establish native IME, visual layout, focus, theme, or display scaling acceptance; those remain explicit M00 spike checks.
## Workspace shell

Run `mise run dev`. Use **Open project…** (`Ctrl+Shift+O`, `Cmd+Shift+O` on macOS) to select an Astro project. Select a collection entry to edit its source. Open-document tabs preserve text, selection and undo history. Dirty documents block project switching; closing them requires explicit discard. Changes stay in memory: saving is not available until M04.

Static discovery supports any installed release in Astro majors **5, 6, and 7**, literal local `glob()` collections and the three documented Markdown/MDX patterns. MDX patterns require the configured `@astrojs/mdx` integration at the matching major (4.x for Astro 5, 6.x for Astro 6, 8.x for Astro 7). Dynamic configs, custom IDs, legacy/experimental flags, unknown integrations, MDX options and local/imported schema constants receive diagnostics. Watcher refreshes the index and retains open buffers; close and reopen an unchanged document to load its disk version. See [M02 evidence and limits](docs/technical/m02-progress.md).

Use **Open file…** to view a local UTF-8 source file (up to 8 MiB and 50,000 lines). The document is readonly: selection and copy work; edits and saving are unavailable. MDX bodies remain intact as source; MDX-aware highlighting is not promised. File errors and picker cancellation preserve the current document. Reads run off the UI thread.

**Commands…** searches the shared registry, including panel toggles, document focus, theme selection, and reset layout. Arrow keys select a result; Enter invokes it. Escape first clears a nonempty search, then dismisses the palette and restores focus.

| Command | Linux / Windows | macOS |
| --- | --- | --- |
| Open file | Ctrl+O | Command+O |
| Commands | Ctrl+Shift+P | Command+Shift+P |
| Toggle files | Ctrl+B | Command+B |
| Toggle inspector | Ctrl+Alt+I | Command+Option+I |
| Toggle preview region | Ctrl+Alt+P | Command+Option+P |
| Focus document | Ctrl+1 | Command+1 |

The preview region currently reports that no preview is running. It does not start a development server. Theme mode, side widths, and panel visibility restore at startup. Preferences use `astrolith/preferences.json` under `$XDG_CONFIG_HOME` (or `~/.config`) on Linux, `~/Library/Application Support` on macOS, and `%APPDATA%` on Windows. An absolute `ASTROLITH_SETTINGS_PATH` can isolate a smoke run. Invalid, unsupported, or externally changed settings are preserved and reported; fix the settings file and restart to enable persistence again.

`mise run dev:editor` and `mise run dev:layout` retain the disposable M00 probes. Their manual acceptance does not automatically establish native acceptance for new M01 behavior. Local native evidence is Linux Wayland only; hosted CI and macOS/Windows native results require separate validation.
