import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const SUPPORTED_PATTERNS = new Set(["**/*.md", "**/*.mdx", "**/*.{md,mdx}"]);
const MAX_BRACE_EXPANSIONS = 64;
const COMMAND_TIMEOUT_MS = 60_000;
const MAX_COMMAND_OUTPUT_BYTES = 1024 * 1024;
const DEFAULT_JSON_OUTPUT = join(tmpdir(), "astrolith-fixtures-oracle.json");
const DEFAULT_REPORT_OUTPUT = join(tmpdir(), "astrolith-fixtures-report.md");

const FIXTURES = [
  {
    id: "astro5",
    astro: "5.18.2",
    mdx: "4.3.14",
    identity: {
      defaultId: "Truthy frontmatter data.slug is returned unchanged; otherwise strip the final source extension, github-slugger each path segment, join with '/', and remove a trailing '/index'.",
      generatedIdCoercion: "No additional coercion is applied by glob() after generateId.",
    },
    identityMarkers: ["return data.slug;"],
    sourceSha256: {
      "node_modules/astro/dist/content/loaders/glob.js": "2e29927e76def4619417d54abe8404fc09041e9a176cdd3e40a789e2c37451ef",
      "node_modules/astro/dist/content/utils.js": "ddb8a456a78c4f39fa9b66fc8625902e800828421f6d1e258af1c1ca875cb22c",
    },
    collections: [
      { name: "docs", base: "content/docs", pattern: "**/*.{md,mdx}" },
      { name: "markdownDocs", base: "content/docs", pattern: "**/*.md" },
      { name: "archive", base: "content/docs", pattern: "**/*.{md,mdx}" },
      { name: "customIds", base: "content/custom", pattern: "**/*.md" },
    ],
  },
  {
    id: "astro6",
    astro: "6.4.8",
    mdx: "6.0.3",
    identity: {
      defaultId: "Truthy frontmatter data.slug is returned unchanged; otherwise strip the final source extension, github-slugger each path segment, join with '/', and remove a trailing '/index'.",
      generatedIdCoercion: "No additional coercion is applied by glob() after generateId.",
    },
    identityMarkers: ["return data.slug;"],
    sourceSha256: {
      "node_modules/astro/dist/content/loaders/glob.js": "7d83443a719ee64b53f120116cc61941ea6aa56ead17ddfacdee396cb8fc815a",
      "node_modules/astro/dist/content/utils.js": "72dc845283b43d4dcf332dd7984cb09492c3fd62062a7c6a81e9b66dca75cfad",
    },
    collections: [
      { name: "docs", base: "content/docs", pattern: "**/*.{md,mdx}" },
      { name: "markdownDocs", base: "content/docs", pattern: "**/*.md" },
      { name: "archive", base: "content/docs", pattern: "**/*.{md,mdx}" },
      { name: "customIds", base: "content/custom", pattern: "**/*.md" },
      { name: "publishedDocs", base: "content/docs", pattern: ["**/*.{md,mdx}", "!**/drafts/**"] },
      { name: "selectedDocs", base: "content/docs", pattern: ["**/*.md", "!**/guides/**", "!**/introduction.md"] },
      { name: "negativeFirstDocs", base: "content/docs", pattern: ["!**/drafts/**", "**/*.{md,mdx}"] },
    ],
  },
  {
    id: "astro7",
    astro: "7.4.4",
    mdx: "8.1.2",
    identity: {
      defaultId: "Truthy frontmatter data.slug returns String(data.slug); otherwise strip the final source extension, github-slugger each path segment, join with '/', and remove a trailing '/index'.",
      generatedIdCoercion: "glob() converts every generateId result to String() before store operations.",
    },
    identityMarkers: ["return String(data.slug);", "const generateId = (opts) => String(userGenerateId(opts));"],
    sourceSha256: {
      "node_modules/astro/dist/content/loaders/glob.js": "ceccd1ae70a146b22222206db1a39cd1db65b0f8d6fa4c3debae40d541ad0818",
      "node_modules/astro/dist/content/utils.js": "6d42b7bf1ec4a9fbc6ff6099e3f131a616d55e2398c2e6ed1403b1c9f2863dc7",
    },
    collections: [
      { name: "docs", base: "content/docs", pattern: "**/*.{md,mdx}" },
      { name: "markdownDocs", base: "content/docs", pattern: "**/*.md" },
      { name: "archive", base: "content/docs", pattern: "**/*.{md,mdx}" },
      { name: "customIds", base: "content/custom", pattern: "**/*.md" },
    ],
  },
];

export function assertProductionMappings(production, oracle) {
  const normalize = (rows) => rows.map(({ collection, id, filePath }) => {
    if (![collection, id, filePath].every((value) => typeof value === "string" && value.length)) {
      throw new Error("Invalid production mapping record");
    }
    return JSON.stringify([collection, id, filePath]);
  }).sort();
  const expected = normalize(oracle.filter((row) => row.collection !== "customIds"));
  const actual = normalize(production);
  if (JSON.stringify(expected) !== JSON.stringify(actual)) {
    throw new Error(`Production mapping mismatch: expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`);
  }
}

function expandBraces(pattern) {
  const open = pattern.indexOf("{");
  if (open === -1) return [pattern];
  const close = pattern.indexOf("}", open + 1);
  if (close === -1) throw new Error(`Unterminated brace expansion in ${pattern}`);
  if (pattern.slice(close + 1).includes("{") || pattern.slice(close + 1).includes("}")) {
    throw new Error(`Nested brace expansions are unsupported in ${pattern}`);
  }
  const prefix = pattern.slice(0, open);
  const suffix = pattern.slice(close + 1);
  const body = pattern.slice(open + 1, close);
  const expanded = [];
  for (const alternative of body.split(",")) {
    if (alternative.length === 0) throw new Error(`Empty brace alternative in ${pattern}`);
    for (const value of expandBraces(`${prefix}${alternative}${suffix}`)) {
      if (expanded.length >= MAX_BRACE_EXPANSIONS) {
        throw new Error(`Brace expansion exceeds ${MAX_BRACE_EXPANSIONS} patterns`);
      }
      expanded.push(value);
    }
  }
  return expanded;
}

function parseCharacterClass(segment, start) {
  const close = segment.indexOf("]", start + 1);
  if (close === -1) throw new Error(`Unterminated character class in ${segment}`);
  let index = start + 1;
  const negated = segment[index] === "!" || segment[index] === "^";
  if (negated) index += 1;
  const ranges = [];
  while (index < close) {
    const first = segment[index];
    if (index + 2 < close && segment[index + 1] === "-") {
      const last = segment[index + 2];
      if (last < first) throw new Error(`Descending character class range in ${segment}`);
      ranges.push([first, last]);
      index += 3;
    } else {
      ranges.push([first, first]);
      index += 1;
    }
  }
  if (ranges.length === 0) throw new Error(`Empty character class in ${segment}`);
  return [{ type: "class", negated, ranges }, close + 1];
}

function tokenizeSegment(segment) {
  if (segment.length === 0) throw new Error("Glob patterns must not contain empty segments");
  const tokens = [];
  for (let index = 0; index < segment.length;) {
    const character = segment[index];
    if (character === "*") {
      if (segment[index + 1] === "*") throw new Error("`**` must form a complete glob segment");
      tokens.push({ type: "star" });
      index += 1;
    } else if (character === "?") {
      tokens.push({ type: "any" });
      index += 1;
    } else if (character === "[") {
      const [token, next] = parseCharacterClass(segment, index);
      tokens.push(token);
      index = next;
    } else if (character === "\\" || character === "{" || character === "}" || character === "]") {
      throw new Error(`Unsupported character ${character} in glob pattern`);
    } else {
      tokens.push({ type: "literal", value: character });
      index += 1;
    }
  }
  return tokens;
}

function segmentMatches(tokens, text) {
  const characters = [...text];
  let next = new Array(characters.length + 1).fill(false);
  next[0] = true;
  for (const token of tokens) {
    const current = new Array(characters.length + 1).fill(false);
    if (token.type === "star") {
      current[0] = next[0];
      for (let index = 0; index < characters.length; index += 1) {
        current[index + 1] = next[index + 1] || current[index];
      }
    } else {
      for (let index = 0; index < characters.length; index += 1) {
        const character = characters[index];
        const accepts = token.type === "any"
          || (token.type === "literal" && character === token.value)
          || (token.type === "class"
            && ((token.ranges.some(([first, last]) => first <= character && character <= last))
              !== token.negated));
        current[index + 1] = next[index] && accepts;
      }
    }
    next = current;
  }
  return next[characters.length];
}

function globMatchesPath(filePath, pattern) {
  if (pattern.length === 0) throw new Error("Glob exclusion must not be empty");
  if (pattern.startsWith("/") || pattern.startsWith("\\") || pattern.startsWith("!")) {
    throw new Error(`Glob exclusion must be relative: ${pattern}`);
  }
  if (pattern.includes("\\")) throw new Error(`Glob exclusion must use '/': ${pattern}`);
  if (pattern.split("/").some((segment) => segment === "..")) {
    throw new Error(`Glob exclusion must not traverse parents: ${pattern}`);
  }
  const pathSegments = filePath.replaceAll("\\", "/").split("/");
  if (pathSegments.some((segment) => segment === "..")) {
    throw new Error(`Glob exclusion must not traverse parents: ${pattern}`);
  }
  return expandBraces(pattern).some((alternative) => {
    const patternSegments = alternative.split("/").map((segment) =>
      segment === "**" ? "**" : tokenizeSegment(segment));
    const usable = pathSegments.length;
    let next = new Array(usable + 1).fill(false);
    next[usable] = true;
    for (let patternIndex = patternSegments.length - 1; patternIndex >= 0; patternIndex -= 1) {
      const segment = patternSegments[patternIndex];
      const current = new Array(usable + 1).fill(false);
      for (let pathIndex = usable; pathIndex >= 0; pathIndex -= 1) {
        if (segment === "**") {
          current[pathIndex] = next[pathIndex]
            || (pathIndex < usable && current[pathIndex + 1]);
        } else {
          current[pathIndex] = pathIndex < usable
            && segmentMatches(segment, pathSegments[pathIndex])
            && next[pathIndex + 1];
        }
      }
      next = current;
    }
    return next[0];
  });
}

export function matchesSupportedPattern(filePath, pattern) {
  const patterns = Array.isArray(pattern) ? pattern : [pattern];
  if (patterns.length === 0) throw new Error("Fixture pattern arrays must not be empty");
  const normalized = filePath.replaceAll("\\", "/");

  for (const entry of patterns) {
    if (!entry.startsWith("!")) continue;
    const exclusion = entry.slice(1);
    if (exclusion.length === 0 || exclusion.startsWith("!")) {
      throw new Error(`Unsupported fixture exclusion: ${entry}`);
    }
    if (globMatchesPath(normalized, exclusion)) return false;
  }

  let included = false;
  for (const entry of patterns) {
    if (entry.startsWith("!")) continue;
    if (!SUPPORTED_PATTERNS.has(entry)) {
      throw new Error(`Unsupported fixture pattern: ${entry}`);
    }
    if (entry === "**/*.md") included ||= normalized.endsWith(".md");
    else if (entry === "**/*.mdx") included ||= normalized.endsWith(".mdx");
    else included ||= normalized.endsWith(".md") || normalized.endsWith(".mdx");
  }
  return included;
}

function walkFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const absolute = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...walkFiles(absolute));
    else if (entry.isFile()) files.push(absolute);
  }
  return files;
}

export function selectSupportedSources(projectRoot, base, pattern) {
  const absoluteRoot = resolve(projectRoot);
  const absoluteBase = resolve(absoluteRoot, base);
  const relativeBase = relative(absoluteRoot, absoluteBase);
  if (relativeBase.startsWith("..") || !existsSync(absoluteBase) || !statSync(absoluteBase).isDirectory()) {
    throw new Error(`Fixture base must be a project directory: ${base}`);
  }

  return walkFiles(absoluteBase)
    .map((file) => relative(absoluteRoot, file).replaceAll("\\", "/"))
    .filter((file) => matchesSupportedPattern(relative(absoluteBase, resolve(absoluteRoot, file)).replaceAll("\\", "/"), pattern))
    .sort((left, right) => left.localeCompare(right));
}

function validateEvent(value, lineNumber) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`oracle event line ${lineNumber} must be an object`);
  }
  for (const key of ["collection", "id", "filePath"]) {
    if (typeof value[key] !== "string" || value[key].length === 0) {
      throw new Error(`oracle event line ${lineNumber} ${key} must be a nonempty string`);
    }
  }
  return {
    collection: value.collection,
    id: value.id,
    filePath: value.filePath.replaceAll("\\", "/"),
  };
}

export function parseOracleEvents(source) {
  const events = new Map();
  source.split(/\r?\n/).forEach((line, index) => {
    if (line.trim().length === 0) return;
    let value;
    try {
      value = JSON.parse(line);
    } catch (error) {
      throw new Error(`oracle event line ${index + 1} is not JSON: ${error.message}`);
    }
    const event = validateEvent(value, index + 1);
    events.set(`${event.collection}\0${event.id}\0${event.filePath}`, event);
  });
  return [...events.values()].sort((left, right) =>
    left.collection.localeCompare(right.collection)
      || left.id.localeCompare(right.id)
      || left.filePath.localeCompare(right.filePath));
}

function sortedUnique(values) {
  return [...new Set(values)].sort((left, right) => left.localeCompare(right));
}

export function compareOracleMappings(events, expectedByCollection) {
  const missing = [];
  const unexpected = [];
  const collections = new Set([
    ...expectedByCollection.keys(),
    ...events.map((event) => event.collection),
  ]);

  for (const collection of [...collections].sort((left, right) => left.localeCompare(right))) {
    const expected = sortedUnique(expectedByCollection.get(collection) ?? []);
    const actual = sortedUnique(
      events.filter((event) => event.collection === collection).map((event) => event.filePath),
    );
    const expectedSet = new Set(expected);
    const actualSet = new Set(actual);
    for (const filePath of expected) {
      if (!actualSet.has(filePath)) missing.push({ collection, filePath });
    }
    for (const filePath of actual) {
      if (!expectedSet.has(filePath)) unexpected.push({ collection, filePath });
    }
  }

  return { ok: missing.length === 0 && unexpected.length === 0, missing, unexpected };
}

function parseOptions(argv) {
  const options = {
    jsonOutput: DEFAULT_JSON_OUTPUT,
    reportOutput: DEFAULT_REPORT_OUTPUT,
    keepTemp: false,
    timeoutMs: COMMAND_TIMEOUT_MS,
  };
  for (const argument of argv) {
    if (argument === "--keep-temp") options.keepTemp = true;
    else if (argument.startsWith("--json-output=")) options.jsonOutput = argument.slice("--json-output=".length);
    else if (argument.startsWith("--report-output=")) options.reportOutput = argument.slice("--report-output=".length);
    else if (argument.startsWith("--timeout-ms=")) options.timeoutMs = Number(argument.slice("--timeout-ms=".length));
    else throw new Error(`Unknown argument: ${argument}`);
  }
  if (!Number.isInteger(options.timeoutMs) || options.timeoutMs < 1_000 || options.timeoutMs > COMMAND_TIMEOUT_MS) {
    throw new Error(`--timeout-ms must be an integer from 1000 through ${COMMAND_TIMEOUT_MS}`);
  }
  return options;
}

function trimCommandOutput(value) {
  const text = value.trim();
  return text.length <= 2_000 ? text : `…${text.slice(-2_000)}`;
}

function runCommand(command, args, { cwd, env = {}, timeoutMs }) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...env },
    maxBuffer: MAX_COMMAND_OUTPUT_BYTES,
    timeout: timeoutMs,
    windowsHide: true,
  });
  const commandLine = [command, ...args].join(" ");
  if (result.error) {
    throw new Error(`${commandLine} failed: ${result.error.message}\n${trimCommandOutput(result.stderr ?? result.stdout ?? "")}`);
  }
  if (result.status !== 0) {
    throw new Error(`${commandLine} exited ${result.status}\n${trimCommandOutput(result.stderr || result.stdout)}`);
  }
  return {
    command: commandLine,
    stdout: result.stdout,
    stderr: trimCommandOutput(result.stderr),
  };
}

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, "utf8"));
}

function sha256(filePath) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function copyFixture(source, destination) {
  cpSync(source, destination, {
    recursive: true,
    filter(from) {
      return !["node_modules", ".astro"].includes(basename(from));
    },
  });
}

function inspectInstalledIdentitySource(projectRoot, fixture) {
  const files = [
    "node_modules/astro/dist/content/loaders/glob.js",
    "node_modules/astro/dist/content/utils.js",
  ];
  const globSource = readFileSync(join(projectRoot, files[0]), "utf8");
  const utilsSource = readFileSync(join(projectRoot, files[1]), "utf8");
  const requiredGlobMarkers = ["data.slug", "getContentEntryIdAndSlug", "store.set({"];
  const requiredUtilsMarkers = ["githubSlug(segment)", ".replace(/\\/index$/, \"\")"];
  for (const marker of requiredGlobMarkers) {
    if (!globSource.includes(marker)) throw new Error(`${fixture.id} Astro glob source lacks marker ${marker}`);
  }
  for (const marker of fixture.identityMarkers) {
    if (!globSource.includes(marker)) throw new Error(`${fixture.id} Astro glob source lacks identity marker ${marker}`);
  }
  for (const marker of requiredUtilsMarkers) {
    if (!utilsSource.includes(marker)) throw new Error(`${fixture.id} Astro content utils source lacks marker ${marker}`);
  }
  const sourceFiles = files.map((file) => ({ path: file, sha256: sha256(join(projectRoot, file)) }));
  for (const source of sourceFiles) {
    if (source.sha256 !== fixture.sourceSha256[source.path]) {
      throw new Error(`${fixture.id} installed source hash changed for ${source.path}`);
    }
  }
  return {
    ...fixture.identity,
    sourceFiles,
  };
}

function addAssertion(assertions, name, condition, detail) {
  assertions.push({ name, ok: Boolean(condition), detail });
  if (!condition) throw new Error(`${name}: ${detail}`);
}

function assertFixtureMappings(projectRoot, fixture, events) {
  const expectedByCollection = new Map(
    fixture.collections.map((collection) => [
      collection.name,
      selectSupportedSources(projectRoot, collection.base, collection.pattern),
    ]),
  );
  const comparison = compareOracleMappings(events, expectedByCollection);
  if (!comparison.ok) {
    throw new Error(`${fixture.id} oracle mappings differ: ${JSON.stringify(comparison)}`);
  }

  const assertions = [];
  addAssertion(
    assertions,
    "literal loader selections match store.set filePaths",
    true,
    `${events.length} delegated events across ${expectedByCollection.size} collections`,
  );
  const docs = events.filter((event) => event.collection === "docs");
  const notes = events.filter((event) => event.collection === "markdownDocs");
  const archive = events.filter((event) => event.collection === "archive");
  addAssertion(
    assertions,
    "markdown collections overlap without merging identities",
    sortedUnique(notes.map((event) => event.filePath)).every((filePath) => docs.some((event) => event.filePath === filePath)),
    "markdownDocs sources are a subset of docs sources",
  );
  addAssertion(
    assertions,
    "identical overlapping glob bases remain separate collection events",
    JSON.stringify(docs.map((event) => [event.id, event.filePath])) === JSON.stringify(archive.map((event) => [event.id, event.filePath])),
    "docs and archive emit the same ordered ID/path pairs",
  );

  if (fixture.collections.some((collection) => collection.name === "publishedDocs")) {
    const published = events.filter((event) => event.collection === "publishedDocs");
    const selected = events.filter((event) => event.collection === "selectedDocs");
    const negativeFirst = events.filter((event) => event.collection === "negativeFirstDocs");
    addAssertion(
      assertions,
      "negative directory glob excludes nested drafts",
      published.length > 0 && published.every((event) => !event.filePath.includes("/drafts/")),
      `${published.length} publishedDocs events omit content/docs/drafts`,
    );
    addAssertion(
      assertions,
      "negative file and directory globs leave only the intended source",
      selected.length === 1 && selected[0].filePath === "content/docs/drafts/wip.md",
      `selectedDocs -> ${selected.map((event) => event.filePath).join(", ") || "none"}`,
    );
    addAssertion(
      assertions,
      "negative globs win regardless of declared order",
      JSON.stringify(published.map((event) => event.filePath)) === JSON.stringify(negativeFirst.map((event) => event.filePath)),
      "publishedDocs and negativeFirstDocs select the same sources",
    );
  }

  const findEvent = (collection, filePath) => events.find((event) => event.collection === collection && event.filePath === filePath);
  const introduction = findEvent("docs", "content/docs/introduction.md");
  addAssertion(
    assertions,
    "truthy frontmatter slug supplies the logical ID",
    introduction?.id === "stable-source-slug",
    `introduction.md -> ${introduction?.id ?? "missing"}`,
  );
  addAssertion(
    assertions,
    "nested default IDs retain path segments",
    findEvent("docs", "content/docs/guides/nested.md")?.id === "guides/nested",
    "guides/nested.md -> guides/nested",
  );
  addAssertion(
    assertions,
    "a nested index ID drops only the trailing index segment",
    findEvent("docs", "content/docs/guides/index.md")?.id === "guides",
    "guides/index.md -> guides",
  );
  addAssertion(
    assertions,
    "Unicode nested MDX IDs preserve github-slugger output",
    findEvent("docs", "content/docs/深い/日本語.mdx")?.id === "深い/日本語",
    "content/docs/深い/日本語.mdx -> 深い/日本語",
  );
  const custom = findEvent("customIds", "content/custom/source-only.md");
  addAssertion(
    assertions,
    "custom generateId keeps logical identity separate from filePath",
    custom?.id === "logical/custom-id",
    `source-only.md -> ${custom?.id ?? "missing"}`,
  );
  addAssertion(
    assertions,
    "MDX source is selected and recorded",
    docs.some((event) => event.filePath.endsWith(".mdx")),
    "docs contains a real MDX source mapping",
  );

  return {
    expectedSources: Object.fromEntries(expectedByCollection),
    comparison,
    assertions,
  };
}

function formatMarkdown(report, jsonOutput) {
  const lines = [
    "# Astro fixture compatibility report",
    "",
    `Generated: ${report.generatedAt}`,
    `Result: **${report.ok ? "PASS" : "FAIL"}**`,
    `Project Mise Bun: \`${report.bunVersion ?? "unavailable"}\``,
    `Machine-readable oracle: \`${jsonOutput}\``,
    "",
    "Registry-selected exact releases on 2026-10-31:",
    "",
    "| Fixture | Astro | @astrojs/mdx |",
    "| --- | --- | --- |",
    ...FIXTURES.map((fixture) => `| ${fixture.id} | \`${fixture.astro}\` | \`${fixture.mdx}\` |`),
    "",
  ];
  if (report.error) lines.push("## Failure", "", "```text", report.error, "```", "");
  for (const fixture of report.fixtures ?? []) {
    lines.push(`## ${fixture.id}`, "");
    if (fixture.error) lines.push("Error:", "", "```text", fixture.error, "```", "");
    lines.push(`Installed Astro: \`${fixture.installedAstro ?? "unknown"}\``);
    lines.push(`Installed @astrojs/mdx: \`${fixture.installedMdx ?? "unknown"}\``);
    if (fixture.identity) {
      lines.push("", `Identity: ${fixture.identity.defaultId}`);
      lines.push(`${fixture.identity.generatedIdCoercion}`);
      lines.push("Installed source:", "");
      for (const source of fixture.identity.sourceFiles ?? []) {
        lines.push(`- \`${source.path}\`: \`${source.sha256}\``);
      }
    }
    if (fixture.assertions?.length) {
      lines.push("", "Checks:", "");
      for (const assertion of fixture.assertions) {
        lines.push(`- ${assertion.ok ? "PASS" : "FAIL"}: ${assertion.name} (${assertion.detail})`);
      }
    }
    if (fixture.mappings?.length) {
      lines.push("", "| Collection | Logical ID | Source filePath |", "| --- | --- | --- |");
      for (const event of fixture.mappings) {
        lines.push(`| ${event.collection} | \`${event.id}\` | \`${event.filePath}\` |`);
      }
    }
    lines.push("");
  }
  lines.push("## Compatibility claim", "");
  lines.push("Only the exact installed releases above are covered. No version range is claimed.", "");
  return lines.join("\n");
}

function writeOutputs(report, options) {
  mkdirSync(dirname(options.jsonOutput), { recursive: true });
  mkdirSync(dirname(options.reportOutput), { recursive: true });
  writeFileSync(options.jsonOutput, `${JSON.stringify(report, null, 2)}\n`);
  writeFileSync(options.reportOutput, formatMarkdown(report, options.jsonOutput));
}

async function main() {
  const options = parseOptions(process.argv.slice(2));
  const report = {
    schemaVersion: 1,
    generatedAt: new Date().toISOString(),
    registrySource: "https://registry.npmjs.org/astro",
    registryVerifiedDate: "2026-10-31",
    bunVersion: null,
    ok: false,
    fixtures: [],
  };
  let tempRoot;
  try {
    report.bunVersion = runCommand("bun", ["--version"], {
      cwd: process.cwd(),
      timeoutMs: options.timeoutMs,
    }).stdout.trim();
    tempRoot = mkdtempSync(join(tmpdir(), "astrolith-astro-fixtures-"));
    report.tempRoot = tempRoot;

    const harnessTest = runCommand(process.execPath, ["--test", "fixtures/test-astro-fixtures.test.mjs"], {
      cwd: process.cwd(),
      timeoutMs: options.timeoutMs,
    });
    report.harnessTest = harnessTest.stdout || "Node test runner exited successfully";

    for (const fixture of FIXTURES) {
      const record = { id: fixture.id, error: null };
      report.fixtures.push(record);
      try {
        const source = join(process.cwd(), "fixtures", fixture.id);
        const projectRoot = join(tempRoot, fixture.id);
        copyFixture(source, projectRoot);
        record.commands = [];
        record.commands.push(runCommand("bun", ["install", "--frozen-lockfile"], {
          cwd: projectRoot,
          timeoutMs: options.timeoutMs,
        }));

        const installedAstro = readJson(join(projectRoot, "node_modules/astro/package.json"));
        const installedMdx = readJson(join(projectRoot, "node_modules/@astrojs/mdx/package.json"));
        record.installedAstro = installedAstro.version;
        record.installedMdx = installedMdx.version;
        if (installedAstro.version !== fixture.astro || installedMdx.version !== fixture.mdx) {
          throw new Error(`${fixture.id} installed versions do not match the frozen fixture lockfile`);
        }

        record.identity = inspectInstalledIdentitySource(projectRoot, fixture);
        record.commands.push(runCommand("bun", ["run", "sync", "--", "--force"], {
          cwd: projectRoot,
          timeoutMs: options.timeoutMs,
        }));

        const oracleEventsFile = join(projectRoot, "oracle-events.jsonl");
        rmSync(oracleEventsFile, { force: true });
        record.commands.push(runCommand("bun", ["run", "sync:oracle", "--", "--force"], {
          cwd: projectRoot,
          env: { ASTROLITH_ORACLE_OUTPUT: oracleEventsFile },
          timeoutMs: options.timeoutMs,
        }));
        record.mappings = parseOracleEvents(readFileSync(oracleEventsFile, "utf8"));
        Object.assign(record, assertFixtureMappings(projectRoot, fixture, record.mappings));
        const production = runCommand("cargo", ["run", "--quiet", "--locked", "--package", "astrolith-workspace", "--example", "fixture-index", "--", projectRoot], {
          cwd: process.cwd(), timeoutMs: options.timeoutMs,
        });
        record.productionMappings = JSON.parse(production.stdout);
        assertProductionMappings(record.productionMappings, record.mappings);
        record.assertions.push({ name: "production Rust index matches real Astro IDs and filePaths", ok: true, detail: `${record.productionMappings.length} mappings compared` });
        const expectedEntries = record.productionMappings.length;
        const editing = runCommand("cargo", ["test", "--quiet", "--locked", "--package", "astrolith-gpui", "installed_astro_fixture_editing_workflow", "--", "--ignored"], {
          cwd: process.cwd(), timeoutMs: options.timeoutMs, env: {
            ASTROLITH_FIXTURE_PROJECT: projectRoot,
            ASTROLITH_FIXTURE_EXPECTED_ENTRIES: String(expectedEntries),
          },
        });
        if (!/1 passed; 0 failed/.test(editing.stdout)) throw new Error("Installed-project editor workflow did not run exactly one passing test");
        record.assertions.push({ name: "real installed project opens in retained GPUI editor", ok: true, detail: `picker, ${expectedEntries} indexed entries, keyboard editing, MDX body and tab retention; source unchanged` });

      } catch (error) {
        record.error = error.stack ?? error.message;
        throw error;
      }
    }
    report.ok = true;
  } catch (error) {
    report.error = error.stack ?? error.message;
  } finally {
    writeOutputs(report, options);
    if (tempRoot && !options.keepTemp) {
      rmSync(tempRoot, { recursive: true, force: true });
      delete report.tempRoot;
      writeOutputs(report, options);
    }
  }

  console.log(`Astro fixture compatibility: ${report.ok ? "PASS" : "FAIL"}`);
  console.log(`Oracle JSON: ${options.jsonOutput}`);
  console.log(`Report: ${options.reportOutput}`);
  if (!report.ok) process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  });
}
