import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  compareOracleMappings,
  matchesSupportedPattern,
  parseOracleEvents,
  selectSupportedSources,
} from "../scripts/test-astro-fixtures.mjs";

test("array patterns apply negative globs regardless of ordering", () => {
  const patterns = ["!**/drafts/**", "**/*.{md,mdx}"];
  assert.equal(matchesSupportedPattern("guides/entry.md", patterns), true);
  assert.equal(matchesSupportedPattern("drafts/entry.md", patterns), false);
  assert.equal(matchesSupportedPattern("drafts/nested/entry.mdx", patterns), false);
  assert.equal(
    matchesSupportedPattern("introduction.md", ["**/*.md", "!**/introduction.md"]),
    false,
  );
  assert.equal(
    matchesSupportedPattern("chapter-3.md", ["**/*.md", "!**/chapter-[0-9].md"]),
    false,
  );
  assert.equal(
    matchesSupportedPattern("template.md", ["**/*.md", "!**/{draft,template}.md"]),
    false,
  );
  assert.throws(() => matchesSupportedPattern("entry.md", []), /must not be empty/);
  assert.throws(
    () => matchesSupportedPattern("entry.md", ["**/*.md", "!../outside/**"]),
    /traverse parents|relative/,
  );
});

test("supported patterns match root and nested Markdown sources", () => {
  assert.equal(matchesSupportedPattern("root.md", "**/*.md"), true);
  assert.equal(matchesSupportedPattern("nested/deep/entry.md", "**/*.md"), true);
  assert.equal(matchesSupportedPattern("root.mdx", "**/*.mdx"), true);
  assert.equal(matchesSupportedPattern("nested/deep/entry.mdx", "**/*.mdx"), true);
  assert.equal(matchesSupportedPattern("root.md", "**/*.{md,mdx}"), true);
  assert.equal(matchesSupportedPattern("nested/deep/entry.mdx", "**/*.{md,mdx}"), true);
  assert.equal(matchesSupportedPattern("nested/entry.mdx", "**/*.md"), false);
  assert.equal(matchesSupportedPattern("nested/entry.mdx", "**/*.{md,mdx}"), true);
  assert.equal(matchesSupportedPattern("nested/notes.txt", "**/*.{md,mdx}"), false);
});

test("oracle JSONL is parsed, validated, and deterministically de-duplicated", () => {
  const input = [
    JSON.stringify({ collection: "docs", id: "nested/entry", filePath: "content/a.md" }),
    "",
    JSON.stringify({ collection: "docs", id: "nested/entry", filePath: "content/a.md" }),
    JSON.stringify({ collection: "notes", id: "entry", filePath: "content/a.md" }),
  ].join("\n");

  assert.deepEqual(parseOracleEvents(input), [
    { collection: "docs", id: "nested/entry", filePath: "content/a.md" },
    { collection: "notes", id: "entry", filePath: "content/a.md" },
  ]);
  assert.throws(
    () => parseOracleEvents('{"collection":"docs","id":1,"filePath":"a.md"}'),
    /id must be a nonempty string/,
  );
});

test("mapping comparison reports missing and unexpected source files", () => {
  const actual = [
    { collection: "docs", id: "guide", filePath: "content/guide.md" },
    { collection: "docs", id: "extra", filePath: "content/extra.mdx" },
  ];
  const expected = new Map([
    ["docs", ["content/guide.md", "content/missing.md"]],
    ["notes", ["content/note.md"]],
  ]);

  assert.deepEqual(compareOracleMappings(actual, expected), {
    ok: false,
    missing: [
      { collection: "docs", filePath: "content/missing.md" },
      { collection: "notes", filePath: "content/note.md" },
    ],
    unexpected: [{ collection: "docs", filePath: "content/extra.mdx" }],
  });
});

test("source selection stays inside the literal base and supported pattern", () => {
  const root = mkdtempSync(join(tmpdir(), "astrolith-selection-test-"));
  try {
    mkdirSync(join(root, "content/docs/nested"), { recursive: true });
    mkdirSync(join(root, "other"), { recursive: true });
    writeFileSync(join(root, "content/docs/root.md"), "# Root\n");
    writeFileSync(join(root, "content/docs/nested/guide.mdx"), "# Guide\n");
    writeFileSync(join(root, "content/docs/nested/ignored.txt"), "ignored\n");
    writeFileSync(join(root, "other/outside.md"), "# Outside\n");

    assert.deepEqual(
      selectSupportedSources(root, "content/docs", "**/*.{md,mdx}"),
      ["content/docs/nested/guide.mdx", "content/docs/root.md"],
    );
    assert.deepEqual(
      selectSupportedSources(root, "content/docs", "**/*.md"),
      ["content/docs/root.md"],
    );
    assert.deepEqual(
      selectSupportedSources(root, "content/docs", ["**/*.{md,mdx}", "!**/nested/**"]),
      ["content/docs/root.md"],
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("production mappings must match real Astro IDs and source paths", async () => {
  const { assertProductionMappings } = await import("../scripts/test-astro-fixtures.mjs");
  const rows = [{collection:"docs",id:"custom-slug",filePath:"content/docs/a.md"}];
  assertProductionMappings(rows, [...rows, {collection:"customIds",id:"other",filePath:"content/custom/x.md"}]);
  assert.throws(() => assertProductionMappings([{...rows[0],filePath:"wrong.md"}], rows), /mapping/);
  assert.throws(() => assertProductionMappings([], rows), /mapping/);
});
