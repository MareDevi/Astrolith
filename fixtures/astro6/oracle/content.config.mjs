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

const publishedDocs = defineCollection({
  loader: recordingGlob({
    base: "./content/docs",
    pattern: ["**/*.{md,mdx}", "!**/drafts/**"],
  }),
});

const selectedDocs = defineCollection({
  loader: recordingGlob({
    base: "./content/docs",
    pattern: ["**/*.md", "!**/guides/**", "!**/introduction.md"],
  }),
});

const negativeFirstDocs = defineCollection({
  loader: recordingGlob({
    base: "./content/docs",
    pattern: ["!**/drafts/**", "**/*.{md,mdx}"],
  }),
});

export const collections = {
  docs,
  markdownDocs,
  archive: archivedDocs,
  customIds,
  publishedDocs,
  selectedDocs,
  negativeFirstDocs,
};
