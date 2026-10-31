import { defineCollection, z } from "astro:content";
import { glob } from "astro/loaders";

// The `docs` collection carries a schema so `bun run sync` emits
// `.astro/collections/docs.schema.json`. The captured output is the committed
// regression oracle at crates/astrolith-astro/src/schema/fixtures/astro6-docs.schema.json.
// Every sample entry under `content/docs` satisfies the schema, so the whole
// fixture pipeline keeps passing. Field types cover strings, an optional string,
// a coerced date with default, a boolean, a string array, an enum, a nested
// object, a nullable nested number, a heterogeneous union, a literal and a
// record; the union is intentional so normalizer tests keep exercising the
// "unsupported, never silently coerced" path. Astro 6's Zod records use the
// `z.record(key, value)` form, which emits `propertyNames` as well.
const docs = defineCollection({
  loader: glob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
  schema: z.object({
    title: z.string(),
    description: z.string().optional(),
    date: z.coerce.date().default(new Date("2024-01-01T00:00:00.000Z")),
    draft: z.boolean().default(false),
    tags: z.array(z.string()).default([]),
    category: z.enum(["news", "blog", "docs"]),
    author: z.object({
      name: z.string(),
      email: z.string().email().optional(),
    }),
    nested: z.object({
      deep: z.object({
        value: z.number().nullable(),
      }),
    }),
    union: z.union([z.string(), z.number()]),
    literal: z.literal("fixed"),
    record: z.record(z.string(), z.number()),
  }),
});

const markdownDocs = defineCollection({
  loader: glob({ base: "./content/docs", pattern: "**/*.md" }),
});

const archivedDocs = defineCollection({
  loader: glob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
});

// Astro 6 accepts an array of glob patterns. Negative entries always exclude,
// regardless of ordering; a later positive entry cannot re-include a path.
const publishedDocs = defineCollection({
  loader: glob({
    base: "./content/docs",
    pattern: ["**/*.{md,mdx}", "!**/drafts/**"],
  }),
});

const selectedDocs = defineCollection({
  loader: glob({
    base: "./content/docs",
    pattern: ["**/*.md", "!**/guides/**", "!**/introduction.md"],
  }),
});

const negativeFirstDocs = defineCollection({
  loader: glob({
    base: "./content/docs",
    pattern: ["!**/drafts/**", "**/*.{md,mdx}"],
  }),
});

export const collections = {
  docs,
  markdownDocs,
  archive: archivedDocs,
  publishedDocs,
  selectedDocs,
  negativeFirstDocs,
};
