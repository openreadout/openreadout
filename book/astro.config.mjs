// The OpenReadout website: the home page and "How it works" (site/pages/), the documentation
// (Markdown under src/, read by site/loader.ts) and the format list. `npm run build` writes the
// static site to book/book/; the docs workflow publishes it to GitHub Pages.
import { defineConfig, passthroughImageService } from "astro/config";
import starlight from "@astrojs/starlight";

const cmd = (name, slug = name) => ({ label: name, slug: `reference/commands/${slug}` });

export default defineConfig({
  site: "https://openreadout.github.io",
  base: "/openreadout",
  srcDir: "./site",
  outDir: "./book",
  trailingSlash: "never",
  // guides/python.html, not guides/python/: the form the CLI's hints and the code link to.
  build: { format: "file" },
  image: { service: passthroughImageService() },
  devToolbar: { enabled: false },
  // Keep the whitespace between a line of text and an inline element on the next line.
  compressHTML: false,
  integrations: [
    starlight({
      title: "OpenReadout",
      description:
        "OpenReadout reads raw lab-instrument files without vendor software, prints JSON, exports to open formats and runs as an MCP server.",
      logo: { light: "./site/assets/logo-light.svg", dark: "./site/assets/logo-dark.svg" },
      favicon: "/favicon.svg",
      head: [
        { tag: "meta", attrs: { property: "og:image", content: "https://openreadout.github.io/openreadout/img/og.png" } },
        { tag: "meta", attrs: { name: "twitter:card", content: "summary_large_image" } },
      ],
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/openreadout/openreadout" }],
      editLink: { baseUrl: "https://github.com/openreadout/openreadout/edit/main/book/" },
      lastUpdated: false,
      pagination: true,
      customCss: ["./site/styles/tokens.css", "./site/styles/docs.css"],
      components: {
        SocialIcons: "./site/components/SocialIcons.astro",
        Footer: "./site/components/Footer.astro",
      },
      expressiveCode: {
        themes: ["github-light", "github-dark"],
        // Commands wrap; transcripts and tables keep their columns and scroll instead.
        defaultProps: { wrap: true, overridesByLang: { "text,txt,console,json,csv,tsv": { wrap: false } } },
        styleOverrides: {
          borderRadius: "6px",
          borderColor: "var(--rule)",
          codeFontFamily: "var(--mono)",
          codeFontSize: "0.84rem",
          codeLineHeight: "1.6",
          uiFontFamily: "var(--sans)",
          frames: { shadowColor: "transparent", editorTabBarBackground: "var(--panel)", terminalTitlebarBackground: "var(--panel)" },
        },
      },
      tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 3 },
      sidebar: [
        {
          label: "Start",
          items: [
            { label: "Install", slug: "getting-started/install" },
            { label: "Your first file", slug: "getting-started/first-file" },
            { label: "Connect an assistant", slug: "getting-started/assistant" },
            { label: "Try it in the browser", link: "https://openreadout.github.io/openreadout/demo/" },
          ],
        },
        {
          label: "Recipes",
          items: [
            { label: "All recipes", slug: "recipes" },
            { label: "Is this file intact?", slug: "recipes/check-files" },
            { label: "Write the methods paragraph", slug: "recipes/methods-text" },
            { label: "Convert to an open format", slug: "recipes/export" },
            { label: "Integrate chromatogram peaks", slug: "recipes/peaks" },
            { label: "Fit a dose–response curve", slug: "recipes/plate-assay" },
            { label: "Check a qPCR run", slug: "recipes/qpcr" },
            { label: "Spike features for a folder", slug: "recipes/ephys" },
            { label: "Index and search a lab share", slug: "recipes/index-share" },
            { label: "Load planes in Python", slug: "recipes/python" },
          ],
        },
        {
          label: "Formats",
          items: [
            { label: "All formats", link: "/formats.html" },
            { label: "High-content screening", slug: "formats/hcs" },
            { label: "Electron microscopy", slug: "formats/em" },
          ],
        },
        {
          label: "Guides",
          collapsed: true,
          items: [
            { label: "Chromatograms and peaks", slug: "guides/quantitation" },
            { label: "Plate-reader assays", slug: "guides/plate-analysis" },
            { label: "NMR processing", slug: "guides/nmr" },
            { label: "Electrophysiology", slug: "guides/ephys" },
            { label: "Batch tables and sample sheets", slug: "guides/batch" },
            { label: "Lab shares, indexes and live acquisitions", slug: "guides/lab-shares" },
            { label: "Metadata conventions", slug: "guides/metadata" },
            { label: "AI agents in depth", slug: "guides/agents" },
            { label: "Python", slug: "guides/python" },
            { label: "R", slug: "guides/r" },
            { label: "Fiji and ImageJ", slug: "guides/fiji" },
            { label: "napari", slug: "guides/napari" },
            { label: "Nextflow, Galaxy, Snakemake", slug: "guides/pipelines" },
            { label: "WebAssembly", slug: "guides/wasm" },
          ],
        },
        {
          label: "Reference",
          collapsed: true,
          items: [
            {
              label: "Commands",
              items: [
                { label: "Overview and exit codes", slug: "reference/commands" },
                cmd("info"),
                cmd("check"),
                cmd("planes"),
                cmd("compare"),
                cmd("report", "report-cmd"),
                cmd("export"),
                cmd("preview"),
                cmd("stats"),
                cmd("trace"),
                cmd("table"),
                cmd("spectra"),
                cmd("analyze"),
                cmd("batch"),
                cmd("link"),
                cmd("index", "index-cmd"),
                cmd("search"),
                cmd("watch"),
                cmd("self"),
                cmd("mcp"),
              ],
            },
            { label: "MCP tools", slug: "reference/mcp" },
            { label: "JSON output schemas", link: "/reference/json.html" },
            { label: "Reading the JSON", slug: "getting-started/reading-json" },
            { label: "Assurance and strict mode", slug: "reference/assurance" },
          ],
        },
        {
          label: "Trust",
          collapsed: true,
          items: [
            { label: "Validation", slug: "project/validation" },
            { label: "Comparison with other tools", slug: "project/comparison" },
            { label: "FAQ", slug: "project/faq" },
            { label: "Clean-room policy", slug: "project/clean-room" },
            { label: "Scope and roadmap", slug: "project/roadmap" },
          ],
        },
      ],
    }),
  ],
});
