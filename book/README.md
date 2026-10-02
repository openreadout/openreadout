# The OpenReadout website

The site at <https://openreadout.github.io/openreadout/>: the home page, the documentation, the format
list and the JSON reference. It is built with [Astro](https://astro.build) and
[Starlight](https://starlight.astro.build) and published by `.github/workflows/docs.yml`.

```bash
cd book
npm ci
node scripts/gen-data.mjs      # format list from the CLI (needs target/{release,debug}/openreadout)
npm run dev                    # http://localhost:4321/openreadout/, reloads on edits
npm run build                  # static site in book/book/
python3 check_links.py         # every in-site link and #fragment resolves
node scripts/check-commands.mjs   # every flag in `<command> --help` is on its reference page
```

## Where things are

| Path | What |
| --- | --- |
| `src/**/*.md` | The documentation pages. Plain Markdown that also reads well on GitHub: a `# Title` heading, relative links to other `.md` files, and mdBook-style includes (`{{#include ../../../docs/formats/czi.md}}`, `{{#include ../../../examples/x.sh:2:13}}`). Code and `docs/` cite these paths, so keep them. |
| `site/loader.ts` | Reads `src/` for Starlight: expands includes, takes the title from the heading, turns `.md` links into site URLs and links to other repository files into GitHub URLs. |
| `site/pages/index.astro`, `how-it-works.astro` | The editorial pages, with `site/layouts/Editorial.astro`, `site/styles/editorial.css` and the figures in `site/scripts/`. |
| `site/pages/formats.astro` | The format list, from `site/data/formats.json` (`scripts/gen-data.mjs`, which runs `openreadout self formats --json`). |
| `site/pages/reference/json.astro` | The JSON reference, rendered from `docs/schema/*.schema.json` at build time. |
| `site/styles/tokens.css` | Colours and type shared by every page; `docs.css` maps them onto Starlight. `web/style.css` (the browser demo) repeats them. |
| `site/data/home.json`, `how.json`, `public/img/` | Figure data and images, taken from public corpus files (`corpus/manifest.toml`). |
| `astro.config.mjs` | Site settings and the sidebar. A new page appears in the sidebar only when it is listed there. |

## Conventions

- URLs end in `.html` (`build.format: "file"`): `src/guides/python.md` is `guides/python.html`. The CLI's
  error hints, package READMEs and the code link to these URLs, so when you rename or remove a page,
  grep the repository for its URL and update the links (`check_links.py` only checks the site itself).
- `<dir>/index.md` is served as `<dir>.html` (Starlight drops `index`).
- The site makes no requests to other origins: fonts are bundled (`@fontsource`), search is
  Pagefind's local index, and the demo page forbids network access entirely.
- Pages follow the voice of the existing docs: plain, exact, second person, real output from public
  files.
