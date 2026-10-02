import { defineCollection } from "astro:content";
import { docsSchema } from "@astrojs/starlight/schema";
import { mdbookLoader } from "./loader";

// The pages are the Markdown files under book/src/ (the paths the code and docs/ cite), read by
// site/loader.ts, not Starlight's default src/content/docs/.
export const collections = {
  docs: defineCollection({ loader: mdbookLoader({ base: "/openreadout/" }), schema: docsSchema() }),
};
