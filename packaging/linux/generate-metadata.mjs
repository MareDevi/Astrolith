#!/usr/bin/env node
// Generate every Linux packaging metadata file from the Cargo workspace version.
//
// The templates and this script are the single source of truth. Build scripts
// call the generator, then compare the generated metadata version with the
// built binary's `--version` output before producing an artifact.
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packagingDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(packagingDirectory, "..", "..");
const templatesDirectory = join(packagingDirectory, "templates");

/** Reverse-DNS identity shared by the desktop entry and AppStream component. */
const DESKTOP_ID = "dev.astrolith.Astrolith";

const outputs = [
  ["astrolith.desktop.in", "astrolith.desktop"],
  ["astrolith.metainfo.xml.in", "astrolith.metainfo.xml"],
  ["astrolith.xml.in", "astrolith.xml"],
];

function parseArguments(argv) {
  const options = {
    output: join(packagingDirectory, "generated"),
    printVersion: false,
    checkBinary: null,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--print-version") {
      options.printVersion = true;
    } else if (argument === "--check-binary") {
      options.checkBinary = argv[index + 1] ?? "";
      index += 1;
      if (!options.checkBinary) throw new Error("--check-binary requires a path");
    } else if (argument === "--output") {
      options.output = argv[index + 1] ?? "";
      index += 1;
      if (!options.output) throw new Error("--output requires a directory");
    } else if (argument === "--help" || argument === "-h") {
      console.log(
        "usage: generate-metadata.mjs [--output DIR] [--print-version] [--check-binary PATH]",
      );
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }
  return options;
}

function workspaceVersion() {
  const manifest = readFileSync(join(repositoryRoot, "Cargo.toml"), "utf8");
  const section = manifest.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/);
  if (!section) throw new Error("Cargo.toml has no [workspace.package] table");
  const version = section[1].match(/^\s*version\s*=\s*"([^"]+)"/m);
  if (!version) throw new Error("[workspace.package] has no version");
  if (!/^\d+\.\d+\.\d+$/.test(version[1])) {
    throw new Error(`workspace version is not an x.y.z triple: ${version[1]}`);
  }
  return version[1];
}

function releaseDate() {
  const epoch = process.env.SOURCE_DATE_EPOCH;
  const date = epoch ? new Date(Number(epoch) * 1000) : new Date();
  if (Number.isNaN(date.getTime())) throw new Error("SOURCE_DATE_EPOCH is not a timestamp");
  return date.toISOString().slice(0, 10);
}

function binaryVersion(path) {
  const result = spawnSync(path, ["--version"], { encoding: "utf8" });
  if (result.error) {
    throw new Error(`could not run ${path} --version: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`${path} --version exited with ${result.status}: ${result.stderr}`);
  }
  const match = result.stdout.trim().match(/(\d+\.\d+\.\d+)$/);
  if (!match) {
    throw new Error(`${path} --version did not report an x.y.z version: ${result.stdout}`);
  }
  return match[1];
}

function main() {
  const options = parseArguments(process.argv.slice(2));
  const version = workspaceVersion();
  if (options.printVersion) {
    console.log(version);
    return;
  }
  const values = {
    "@VERSION@": version,
    "@DESKTOP_ID@": DESKTOP_ID,
    "@RELEASE_DATE@": releaseDate(),
  };
  mkdirSync(options.output, { recursive: true });
  for (const [template, name] of outputs) {
    let content = readFileSync(join(templatesDirectory, template), "utf8");
    for (const [placeholder, value] of Object.entries(values)) {
      content = content.replaceAll(placeholder, value);
    }
    const leftover = content.match(/@[A-Z_]+@/);
    if (leftover) throw new Error(`${name} has an unresolved placeholder ${leftover[0]}`);
    writeFileSync(join(options.output, name), content);
  }
  console.log(`generated packaging metadata ${version} in ${options.output}`);
  if (options.checkBinary) {
    const built = binaryVersion(options.checkBinary);
    if (built !== version) {
      throw new Error(
        `binary version ${built} disagrees with Cargo workspace version ${version}`,
      );
    }
    console.log(`binary version matches generated metadata: ${built}`);
  }
}

try {
  main();
} catch (error) {
  console.error(`packaging metadata: ${error.message}`);
  process.exit(1);
}
