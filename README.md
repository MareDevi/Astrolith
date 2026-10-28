# Astrolith

Astrolith is a planned native desktop application built with Rust, GPUI, and GPUI Kit. The repository currently contains the M00 feasibility bootstrap: one real native window. Product architecture and editor features are pending the feasibility gate. The implementation plan's referenced design specification is missing; the plan is provisional for M00.

## Development environment

Install [Mise](https://mise.jdx.dev/getting-started.html) and the native prerequisites below, then run from this repository:

```sh
mise install
mise run doctor
mise run check
mise run dev
```

If Mise requests trust, review `mise.toml` and use `mise trust` in this directory. Tool installation needs network access. The exact project versions are Rust **1.99.1** (with `rust-analyzer`, `rust-src`, `clippy`, and `rustfmt`), Node.js **26.12.1**, and Bun **1.4.4**. `mise.toml` is authoritative; tools are scoped to this project. There is no competing Rust/Node version file. Mise **2026.10.19** is the tested task runner and the CI version.

`Cargo.lock` fixes the full Rust dependency graph. The only direct application dependency is `gpui-kit = "=0.8.1"`; its default assets/components and matching GPUI 0.3.10 platform snapshot are enabled. No JavaScript fixtures or package installation are needed yet. Future Astro fixtures must use project-local Bun tooling and committed lockfiles.

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
| `mise run test` | Run headless workspace tests. Bootstrap has no domain tests yet. |
| `mise run fmt` | Check formatting without changing files. |
| `mise run lint` | Run Clippy for every workspace target with warnings denied. |
| `mise run check` | Run doctor, fmt, lint, build, and test in order; later milestones extend this gate. |
| `mise run ci` | Run the same check composition in CI. |
| `mise run dev` | Open the native desktop window. |

The CI workflow configures Linux, macOS, and Windows builds/tests with the same Mise pins. Hosted runner results are separate evidence from local verification. Headless checks do not establish native IME, visual layout, focus, theme, or display scaling acceptance; those remain explicit M00 spike checks.
