import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";

const docs = defineCollection({
  loader: glob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
});

const markdownDocs = defineCollection({
  loader: glob({ base: "./content/docs", pattern: "**/*.md" }),
});

const archivedDocs = defineCollection({
  loader: glob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
});

export const collections = { docs, markdownDocs, archive: archivedDocs };
