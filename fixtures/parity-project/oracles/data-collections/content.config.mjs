import {
  defineCollection,
  reference,
  z,
} from "astro:content";
import { file, glob } from "astro/loaders";

// P3 companion oracle. The active fixture config remains inside Astrolith's
// current glob-only discovery subset; P3 copies this file to
// `src/content.config.mjs` when exercising JSON data-collection references.
const posts = defineCollection({
  loader: glob({ base: "./src/content/blog", pattern: "**/*.{md,mdx}" }),
  schema: ({ image }) =>
    z.object({
      title: z.string(),
      date: z.coerce.date(),
      category: z.enum(["news", "blog", "docs"]),
      status: z.literal("published"),
      author: reference("dataAuthors"),
      cover: image().optional(),
    }),
});

const dataAuthors = defineCollection({
  loader: file("./oracles/data-collections/authors.json"),
  schema: z.object({
    id: z.string(),
    name: z.string(),
    role: z.string(),
    slug: z.string(),
  }),
});

export const collections = { posts, dataAuthors };
