import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";

function check(command, args, quiet = false) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed: ${result.error?.message ?? result.stderr ?? result.stdout}`,
    );
  }
  const output = result.stdout.trim();
  if (!quiet) console.log(`${command}: ${output.split("\n")[0] ?? "available"}`);
  return output;
}

try {
  const executable = (name) => process.platform === "win32" ? `${name}.exe` : name;
  check(executable("rustc"), ["--version"]);
  check(executable("cargo"), ["--version"]);
  check(executable("rustup"), ["which", "rustfmt"]);
  check(executable("rustup"), ["which", "clippy-driver"]);
  check(executable("rustup"), ["which", "rust-analyzer"]);
  const sysroot = check(executable("rustc"), ["--print", "sysroot"]);
  if (!existsSync(join(sysroot, "lib", "rustlib", "src", "rust", "library", "core", "src", "lib.rs"))) {
    throw new Error("The rust-src component is missing from the selected toolchain.");
  }
  console.log("rust-src: standard-library source found");
  console.log(`node: ${process.version}`);
  check(executable("bun"), ["--version"]);
  const metadata = JSON.parse(check(executable("cargo"), ["metadata", "--locked", "--no-deps", "--format-version", "1"], true));
  console.log(`cargo metadata: ${metadata.workspace_members.length} workspace package(s), lockfile accepted`);

  if (process.platform === "linux") {
    for (const tool of ["cc", "c++", "clang", "cmake", "pkg-config"]) {
      check(tool, ["--version"]);
    }
    for (const library of ["fontconfig", "wayland-client", "xkbcommon", "xkbcommon-x11", "x11-xcb", "openssl", "libzstd", "vulkan"]) {
      check("pkg-config", ["--modversion", library]);
    }
    console.log("A graphical Wayland/X11 session and Vulkan driver are needed for mise run dev.");
  } else if (process.platform === "darwin") {
    check("xcode-select", ["-p"]);
    check("xcrun", ["--find", "clang"]);
  } else if (process.platform === "win32") {
    const compiler = check("rustc.exe", ["-vV"]);
    if (!compiler.includes("-pc-windows-msvc")) {
      throw new Error("Use the MSVC Rust toolchain on Windows.");
    }
    check("cmake.exe", ["--version"]);
    const vswhere = join(process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft Visual Studio", "Installer", "vswhere.exe");
    if (!existsSync(vswhere)) throw new Error("Install Visual Studio 2022 C++ Build Tools and a Windows SDK.");
    const installation = check(vswhere, ["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"]);
    if (!installation) throw new Error("The Visual Studio C++ workload is missing.");
  } else {
    throw new Error(`Unsupported desktop build platform: ${process.platform}`);
  }
  console.log("Build prerequisites found. Run mise run check to compile and verify the workspace.");
} catch (error) {
  console.error(error.message);
  console.error("See README.md for native dependencies and setup instructions.");
  process.exitCode = 1;
}
