import { defineCollection } from "astro:content";

import { recordingGlob } from "./recording-glob.mjs";

const docs = defineCollection({
  loader: recordingGlob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
});

const markdownDocs = defineCollection({
  loader: recordingGlob({ base: "./content/docs", pattern: "**/*.md" }),
});

const archivedDocs = defineCollection({
  loader: recordingGlob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
});

const customIds = defineCollection({
  loader: recordingGlob({
    base: "./content/custom",
    pattern: "**/*.md",
    generateId: () => "logical/custom-id",
  }),
});

export const collections = { docs, markdownDocs, archive: archivedDocs, customIds };
