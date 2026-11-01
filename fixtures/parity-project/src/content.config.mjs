import {
  defineCollection as defineCol,
  reference as ref,
  z as zod,
} from "astro:content";
import { glob as loadGlob } from "astro/loaders";

// Import aliases exercise the statically verified `defineCollection` and
// `glob` bindings. Every schema is inlined because top-level schema constants
// are outside the supported discovery subset.
const blog = defineCol({
  loader: loadGlob({ base: "./src/content/blog", pattern: "**/*.{md,mdx}" }),
  schema: ({ image }) =>
    zod.object({
      title: zod.string().min(1).max(120).describe("Display title for the entry"),
      description: zod.string().max(280).optional(),
      slug: zod.string().optional(),
      date: zod.coerce.date(),
      updated: zod.coerce.date().optional(),
      draft: zod.boolean().default(false),
      featured: zod.boolean().default(false),
      order: zod.number().int().min(0).max(999).default(0),
      rating: zod
        .number()
        .min(0)
        .max(5)
        .optional()
        .describe("Editorial rating from 0 to 5"),
      tags: zod.array(zod.string()).max(8).default([]),
      category: zod.enum(["news", "blog", "docs"]),
      status: zod.literal("published"),
      author: ref("authors"),
      related: zod.array(ref("blog")).default([]),
      cover: image().optional(),
      gallery: zod.array(image()).default([]),
      seo: zod
        .object({
          title: zod.string(),
          description: zod.string().optional(),
          socialImage: image().optional(),
        })
        .optional(),
      meta: zod.object({ views: zod.number().nullable() }).optional(),
      attributes: zod.record(zod.string(), zod.string()).optional(),
      // Heterogeneous unions are intentionally preserved as unsupported so the
      // normalizer never coerces them; entries still carry valid values.
      legacy: zod.union([zod.string(), zod.number()]).optional(),
    }),
});

const docs = defineCol({
  loader: loadGlob({ base: "./src/content/docs", pattern: "**/*.{md,mdx}" }),
  schema: ({ image }) =>
    zod.object({
      title: zod.string().min(1).max(120),
      description: zod.string().max(280).optional(),
      slug: zod.string().optional(),
      date: zod.coerce.date(),
      draft: zod.boolean().default(false),
      order: zod.number().int().min(0).max(999).default(0),
      tags: zod.array(zod.string()).max(8).default([]),
      category: zod.enum(["guide", "reference"]),
      cover: image().optional(),
      parent: ref("docs").optional(),
      sidebar: zod
        .object({
          label: zod.string().optional(),
          hidden: zod.boolean().default(false),
        })
        .optional(),
    }),
});

const authors = defineCol({
  loader: loadGlob({ base: "./src/content/authors", pattern: "**/*.md" }),
  schema: zod.object({
    name: zod.string().min(1).max(120),
    role: zod.string(),
    email: zod.string().email().optional(),
    pronouns: zod.string().optional(),
  }),
});

const archived = defineCol({
  loader: loadGlob({ base: "./src/content/archive", pattern: "**/*.{md,mdx}" }),
});

// The array form proves nested content directories plus an exclusion. Drafts
// stay in `blog` but never appear in `published`.
const published = defineCol({
  loader: loadGlob({
    base: "./src/content/blog",
    pattern: ["**/*.{md,mdx}", "!**/drafts/**"],
  }),
});

export const collections = {
  blog,
  docs,
  authors,
  archive: archived,
  published,
};
