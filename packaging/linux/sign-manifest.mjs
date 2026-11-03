#!/usr/bin/env node
// Build and sign the update manifest envelope consumed by Astrolith.
//
// The signing key never lives in the repository. Release automation reads it
// from ASTROLITH_UPDATE_SIGNING_KEY (base64 of a PKCS#8 Ed25519 private key or
// its raw 32-byte seed) and only runs this script when that secret exists.
import { createHash, createPrivateKey, createPublicKey, sign } from "node:crypto";
import { createReadStream, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const packagingDirectory = dirname(fileURLToPath(import.meta.url));
const workspaceVersion = (() => {
  const manifest = readFileSync(join(packagingDirectory, "..", "..", "Cargo.toml"), "utf8");
  const section = manifest.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/);
  const version = section?.[1].match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error("could not read the Cargo workspace version");
  return version;
})();

function fail(message) {
  console.error(`update manifest: ${message}`);
  process.exit(1);
}

function parseArguments(argv) {
  const options = {
    version: workspaceVersion,
    notes: "",
    page: "",
    artifacts: [],
    output: "dist/astrolith-update-manifest.json",
    printPublicKey: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--version") {
      options.version = argv[++index] ?? "";
    } else if (argument === "--notes") {
      options.notes = argv[++index] ?? "";
    } else if (argument === "--notes-file") {
      options.notes = readFileSync(argv[++index] ?? "", "utf8").trim();
    } else if (argument === "--page") {
      options.page = argv[++index] ?? "";
    } else if (argument === "--out") {
      options.output = argv[++index] ?? "";
    } else if (argument === "--artifact") {
      const platform = argv[++index] ?? "";
      const path = argv[++index] ?? "";
      const url = argv[++index] ?? "";
      if (!platform || !path || !url) {
        fail("--artifact requires <platform> <file> <url>");
      }
      options.artifacts.push({ platform, path, url });
    } else if (argument === "--print-public-key") {
      options.printPublicKey = true;
    } else if (argument === "--help" || argument === "-h") {
      console.log(
        "usage: sign-manifest.mjs --page URL [--version X.Y.Z] [--notes TEXT | --notes-file FILE] " +
          "[--artifact PLATFORM FILE URL ...] [--out FILE] [--print-public-key]",
      );
      process.exit(0);
    } else {
      fail(`unknown argument: ${argument}`);
    }
  }
  return options;
}

function privateKey() {
  const encoded = process.env.ASTROLITH_UPDATE_SIGNING_KEY;
  if (!encoded) {
    fail("ASTROLITH_UPDATE_SIGNING_KEY is not set; refusing to publish an unsigned manifest");
  }
  const bytes = Buffer.from(encoded.trim(), "base64");
  if (bytes.length === 32) {
    const prefix = Buffer.from("302e020100300506032b657004220420", "hex");
    return createPrivateKey({
      key: Buffer.concat([prefix, bytes]),
      format: "der",
      type: "pkcs8",
    });
  }
  try {
    return createPrivateKey({ key: bytes, format: "der", type: "pkcs8" });
  } catch (error) {
    fail(`ASTROLITH_UPDATE_SIGNING_KEY is not a PKCS#8 Ed25519 key: ${error.message}`);
  }
}

function isHttps(text) {
  try {
    return new URL(text).protocol === "https:";
  } catch {
    return false;
  }
}

function sha256(path) {
  return new Promise((resolve, reject) => {
    const hash = createHash("sha256");
    const stream = createReadStream(path);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", () => resolve(hash.digest("hex")));
  });
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const key = privateKey();
  if (options.printPublicKey) {
    const spki = createPublicKey(key).export({ format: "der", type: "spki" });
    console.log(spki.subarray(spki.length - 32).toString("base64"));
    return;
  }
  if (!/^\d+\.\d+\.\d+$/.test(options.version)) {
    fail(`version is not an x.y.z triple: ${options.version}`);
  }
  if (!isHttps(options.page)) fail("the release page must be an HTTPS URL");
  if (options.artifacts.length === 0) fail("at least one --artifact is required");
  const artifacts = [];
  for (const artifact of options.artifacts) {
    if (!isHttps(artifact.url)) {
      fail(`artifact ${artifact.platform} must use an HTTPS URL`);
    }
    artifacts.push({
      platform: artifact.platform,
      url: artifact.url,
      sha256: await sha256(artifact.path),
    });
  }
  const payloadBytes = Buffer.from(
    JSON.stringify({
      version: options.version,
      notes: options.notes,
      page: options.page,
      artifacts,
    }),
  );
  const signature = sign(null, payloadBytes, key);
  const envelope = {
    payload: payloadBytes.toString("base64"),
    signature: signature.toString("base64"),
  };
  writeFileSync(options.output, `${JSON.stringify(envelope, null, 2)}\n`);
  console.log(`update manifest: wrote ${options.output} for ${options.version}`);
}

main().catch((error) => {
  fail(error.message);
});
