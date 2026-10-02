// Content loader for the pages under book/src/.
//
// The pages are plain Markdown that also reads well on GitHub: a `# Title` heading instead of
// frontmatter, relative links to other `.md` files, and mdBook-style includes of files outside
// book/src (`{{#include ../../../docs/formats/czi.md}}`, `{{#include ../../../examples/x.sh:2:13}}`).
// This loader turns them into Starlight entries:
//
// - expands `{{#include path}}` and `{{#include path:FIRST:LAST}}` (1-based, inclusive lines);
// - takes the title from the first `# ` heading (frontmatter `title:` wins) and drops the heading;
// - rewrites relative links: a link into book/src becomes the page's URL on the site, a link to
//   any other repository file becomes its GitHub URL. Links in included files are resolved
//   against the included file, as on GitHub.
//
// Code blocks are left alone, except that an include inside one is expanded.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, posix, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import yaml from "js-yaml";
import type { Loader } from "astro/loaders";

export const REPO = "https://github.com/openreadout/openreadout";
const BRANCH = "main";

/** Page id (Starlight slug) of a Markdown file under the content directory. */
export function pageId(rel: string): string {
  let id = rel.replace(/\\/g, "/").replace(/\.mdx?$/, "");
  id = id.replace(/(^|\/)(index|README)$/i, "");
  return id;
}

function listMarkdown(dir: string, base = dir): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name.startsWith("_") || name.startsWith(".")) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...listMarkdown(p, base));
    else if (/\.mdx?$/.test(name)) out.push(relative(base, p));
  }
  return out.sort();
}

interface Ctx {
  root: string; // repository root
  content: string; // book/src
  base: string; // site base path, e.g. "/openreadout/"
}

/** Rewrite one link target found in a file at `fromFile`. */
function rewriteTarget(target: string, fromFile: string, ctx: Ctx): string {
  if (/^([a-z][a-z0-9+.-]*:|#|\/)/i.test(target)) return target; // absolute, fragment or root
  const m = target.match(/^([^#?]*)(\?[^#]*)?(#.*)?$/);
  if (!m) return target;
  const [, path, query = "", frag = ""] = m;
  if (!path) return target;
  const abs = resolve(dirname(fromFile), decodeURI(path));
  const inContent = abs === ctx.content || abs.startsWith(ctx.content + "/");
  const inRepo = abs === ctx.root || abs.startsWith(ctx.root + "/");
  if (inContent && /\.mdx?$/.test(abs)) {
    const id = pageId(relative(ctx.content, abs));
    // The JSON reference is one generated page (site/pages/reference/json.astro), one section per schema.
    const schema = id.match(/^reference\/json(?:\/(.+))?$/);
    if (schema) return `${ctx.base}reference/json.html${frag || (schema[1] ? "#" + schema[1] : "")}`;
    return `${ctx.base}${id ? id + ".html" : ""}${query}${frag}`;
  }
  if (inContent) {
    // a static file next to the pages (book/src/x.png) is served from public/ under the same path
    return `${ctx.base}${relative(ctx.content, abs).replace(/\\/g, "/")}${query}${frag}`;
  }
  if (inRepo) {
    const rel = relative(ctx.root, abs).replace(/\\/g, "/");
    let kind = "blob";
    try {
      if (statSync(abs).isDirectory()) kind = "tree";
    } catch {
      /* a link to a file that does not exist: the link checker reports it */
    }
    return `${REPO}/${kind}/${BRANCH}/${rel}${query}${frag}`;
  }
  return target;
}

const FENCE = /^\s{0,3}(```+|~~~+)/;

/** Rewrite the inline and reference links of Markdown text outside code fences. */
function rewriteLinks(text: string, fromFile: string, ctx: Ctx): string {
  let fence: string | null = null;
  return text
    .split("\n")
    .map((line) => {
      const f = line.match(FENCE);
      if (f) {
        if (fence === null) fence = f[1][0];
        else if (f[1][0] === fence) fence = null;
        return line;
      }
      if (fence !== null) return line;
      // keep inline code spans untouched
      return line
        .split(/(`+[^`]*`+)/)
        .map((part, i) => {
          if (i % 2) return part;
          return part
            .replace(/(\]\()(<[^>]*>|[^)\s]+)((?:\s+"[^"]*")?\))/g, (_all, a, t, b) => {
              const raw = t.startsWith("<") ? t.slice(1, -1) : t;
              return a + rewriteTarget(raw, fromFile, ctx) + b;
            })
            .replace(/^(\s*\[[^\]]+\]:\s+)(\S+)/, (_all, a, t) => a + rewriteTarget(t, fromFile, ctx))
            .replace(/((?:href|src)=")([^"]+)(")/g, (_all, a, t, b) => a + rewriteTarget(t, fromFile, ctx) + b);
        })
        .join("");
    })
    .join("\n");
}

/** Expand mdBook includes, recursively; links of included Markdown are resolved against it. */
function expand(text: string, fromFile: string, ctx: Ctx, depth = 0): string {
  if (depth > 5) throw new Error(`${fromFile}: includes nested too deeply`);
  return text.replace(/\{\{#include\s+([^}\s]+)\s*\}\}/g, (_all, spec: string) => {
    const [p, first, last] = spec.split(":");
    const file = resolve(dirname(fromFile), p);
    let body = readFileSync(file, "utf8");
    if (first !== undefined) {
      const lines = body.split("\n");
      const a = Math.max(1, Number(first) || 1);
      const b = last ? Number(last) : lines.length;
      body = lines.slice(a - 1, b).join("\n");
      return body; // a line range is code: no link rewriting
    }
    body = expand(body, file, ctx, depth + 1);
    return /\.mdx?$/.test(file) ? rewriteLinks(body, file, ctx) : body;
  });
}

function splitFrontmatter(raw: string): [Record<string, unknown>, string] {
  const m = raw.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/);
  if (!m) return [{}, raw];
  return [(yaml.load(m[1]) as Record<string, unknown>) ?? {}, raw.slice(m[0].length)];
}

/** Take the first level-1 heading as the title, and remove it. */
function takeTitle(body: string): [string | undefined, string] {
  const lines = body.split("\n");
  let fence: string | null = null;
  for (let i = 0; i < lines.length; i++) {
    const f = lines[i].match(FENCE);
    if (f) {
      fence = fence === null ? f[1][0] : f[1][0] === fence ? null : fence;
      continue;
    }
    if (fence !== null) continue;
    const h = lines[i].match(/^#\s+(.+?)\s*#*\s*$/);
    if (h) {
      lines.splice(i, 1);
      return [h[1].replace(/`/g, ""), lines.join("\n")];
    }
    if (lines[i].trim() && !lines[i].startsWith("<!--")) break; // text before any heading
  }
  return [undefined, body];
}

/** First sentence of the first paragraph, for the page's meta description. */
function firstSentence(body: string): string | undefined {
  for (const para of body.split(/\n\s*\n/)) {
    const t = para.trim();
    if (!t || /^(#|```|~~~|\||<|-|\*|>|\d+\.|!\[)/.test(t)) continue;
    const plain = t
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/[`*_]/g, "")
      .replace(/\s+/g, " ");
    const s = plain.match(/^.*?[.!?](\s|$)/);
    return (s ? s[0] : plain).trim().slice(0, 300);
  }
  return undefined;
}

export function prepare(file: string, ctx: Ctx) {
  const raw = readFileSync(file, "utf8");
  const [fm, rest] = splitFrontmatter(raw);
  let body = expand(rest, file, ctx);
  body = rewriteLinks(body, file, ctx);
  const [h1, without] = takeTitle(body);
  body = without;
  const data: Record<string, unknown> = { ...fm };
  if (!data.title) data.title = h1 ?? pageId(relative(ctx.content, file));
  if (!data.description) {
    const d = firstSentence(body);
    if (d) data.description = d;
  }
  return { data, body };
}

export function mdbookLoader(opts: { base: string; dir?: string }): Loader {
  return {
    name: "openreadout-mdbook-loader",
    load: async ({ config, store, parseData, renderMarkdown, generateDigest, watcher, logger }) => {
      const root = resolve(new URL(".", config.root).pathname, "..");
      const content = resolve(new URL(".", config.root).pathname, opts.dir ?? "src");
      const ctx: Ctx = { root, content, base: opts.base.endsWith("/") ? opts.base : opts.base + "/" };
      const sync = async () => {
        store.clear();
        for (const rel of listMarkdown(content)) {
          const file = join(content, rel);
          const id = pageId(rel);
          const { data, body } = prepare(file, ctx);
          const parsed = await parseData({ id, data, filePath: file });
          const rendered = await renderMarkdown(body, { fileURL: pathToFileURL(file) });
          store.set({
            id,
            data: parsed,
            body,
            rendered,
            filePath: posix.join("src", rel.replace(/\\/g, "/")),
            digest: generateDigest(body + JSON.stringify(data)),
          });
        }
        logger.info(`loaded ${store.keys().length} pages from book/src`);
      };
      await sync();
      watcher?.add([content, join(root, "docs"), join(root, "examples")]);
      watcher?.on("change", async (p) => {
        if (/\.(md|sh|py)$/.test(p)) await sync();
      });
    },
  };
}
