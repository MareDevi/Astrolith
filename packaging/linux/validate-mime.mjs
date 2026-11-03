#!/usr/bin/env node
// Validate the generated shared-mime-info file with the real database compiler.
//
// `update-mime-database` parses every mime-type, glob and subclass declaration;
// a malformed or conflicting entry fails here before packaging.
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packagingDirectory = dirname(fileURLToPath(import.meta.url));
const source = resolve(packagingDirectory, "generated", "astrolith.xml");
const directory = mkdtempSync(join(tmpdir(), "astrolith-mime-"));
try {
  mkdirSync(join(directory, "packages"), { recursive: true });
  copyFileSync(source, join(directory, "packages", "astrolith.xml"));
  const result = spawnSync("update-mime-database", [directory], { encoding: "utf8" });
  if (result.error) {
    throw new Error(
      `update-mime-database is unavailable: ${result.error.message}. Install shared-mime-info.`,
    );
  }
  if (result.status !== 0) {
    throw new Error(result.stderr || `update-mime-database exited with ${result.status}`);
  }
  console.log("MIME database entry is valid.");
} catch (error) {
  console.error(`packaging metadata: ${error.message}`);
  process.exit(1);
} finally {
  rmSync(directory, { recursive: true, force: true });
}
