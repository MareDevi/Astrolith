import mdx from "@astrojs/mdx";
import { defineConfig } from "astro/config";

// The parity fixture intentionally keeps the supported static subset: a
// project-relative source tree, the verified empty `mdx()` integration, and
// settings that cannot change collection discovery. `@astrojs/starlight` is
// declared as package metadata only so the component catalog can detect it;
// it is not registered as an integration here.
export default defineConfig({
  site: "https://parity.astrolith.example",
  integrations: [mdx()],
  markdown: {
    syntaxHighlight: "shiki",
  },
});
