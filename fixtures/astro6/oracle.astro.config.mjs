import mdx from "@astrojs/mdx";
import { defineConfig } from "astro/config";

export default defineConfig({
  srcDir: "./oracle",
  integrations: [mdx()],
});
