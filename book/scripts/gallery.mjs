// Draw the gallery figures with the openreadout binary: every entry of book/gallery/gallery.json
// becomes public/img/gallery/NAME.png and NAME-dark.png, and site/data/gallery.json records what was drawn (the
// command, the picture's size) for site/pages/gallery.astro and the home page.
//
//   node book/scripts/gallery.mjs [NAME ...]
//
// The binary is $OPENREADOUT_BIN, else target/release/openreadout, else target/debug/openreadout.
// The corpus is $OPENREADOUT_CORPUS_DIR, else corpus/files (`cargo xtask corpus fetch --tier
// standard` has every file the gallery uses). Run it again after a change to the figures.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const book = resolve(fileURLToPath(import.meta.url), "../..");
const root = resolve(book, "..");
const bin =
  process.env.OPENREADOUT_BIN ||
  ["release", "debug"].map((p) => join(root, "target", p, "openreadout")).find((p) => existsSync(p));
if (!bin) throw new Error("no openreadout binary: cargo build --release, or set OPENREADOUT_BIN");
const corpus = process.env.OPENREADOUT_CORPUS_DIR || join(root, "corpus", "files");
const manifest = JSON.parse(readFileSync(join(book, "gallery", "gallery.json"), "utf8"));
const outDir = join(book, "public", "img", "gallery");
const dataFile = join(book, "site", "data", "gallery.json");
mkdirSync(outDir, { recursive: true });
const only = process.argv.slice(2);
const work = mkdtempSync(join(tmpdir(), "openreadout-gallery-"));

/** `$CORPUS/x` → the corpus path (`real`) or the file name a reader would type (`shown`). */
const real = (s) => s.replaceAll("$CORPUS", corpus);
const shown = (s) => s.replace(/\$CORPUS\/(\S*?)(?=["\s]|$)/g, (_m, p) => basename(p) || p);
const deep = (v, f) =>
  typeof v === "string" ? f(v) : Array.isArray(v) ? v.map((x) => deep(x, f)) : v && typeof v === "object"
    ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, deep(x, f)])) : v;
const quote = (a) => (/^[\w@%+=:,./-]+$/.test(a) ? a : `'${a.replaceAll("'", "'\\''")}'`);

function run(args) {
  let out;
  try {
    out = execFileSync(bin, [...args, "--json"], { encoding: "utf8", maxBuffer: 1 << 28 });
  } catch (e) {
    out = e.stdout;
    if (!out) throw e;
  }
  const env = JSON.parse(out);
  if (!env.ok) throw new Error(`openreadout ${args.slice(0, 2).join(" ")}: ${env.error.message}\nhint: ${env.error.hint}`);
  return env.data;
}

// Each figure is drawn twice: NAME.png in the light theme and NAME-dark.png in the dark one.
const THEMES = [
  ["", []],
  ["-dark", ["--dark"]],
];

function preview(e) {
  const p = e.preview;
  const args = [...p.args];
  if (p.annotations) args.push("--annotate", JSON.stringify(p.annotations));
  let meta;
  for (const [suffix, flag] of THEMES) {
    meta = run(["preview", real(p.file), ...args, ...flag, "-o", join(outDir, `${e.name}${suffix}.png`), "--overwrite"]);
  }
  const cmd = ["openreadout", "preview", shown(p.file), ...args.map(shown), "-o", `${e.name}.png`];
  return { meta, width: meta.width, height: meta.height, commands: [cmd.map(quote).join(" ")] };
}

function objects(e) {
  const o = e.objects;
  const statsArgs = o.stats_args ?? [];
  const stats = run(["stats", real(o.file), "--select", o.select, "--objects", ...statsArgs]);
  const found = stats.objects;
  const marks = found.objects.map((b, i) => ({ type: "point", x: Math.round(b.x), y: Math.round(b.y), label: String(i + 1) }));
  const args = ["--select", o.select, ...o.args, "--annotate", JSON.stringify(marks)];
  let meta;
  for (const [suffix, flag] of THEMES) {
    meta = run(["preview", real(o.file), ...args, ...flag, "-o", join(outDir, `${e.name}${suffix}.png`), "--overwrite"]);
  }
  return {
    meta,
    width: meta.width,
    height: meta.height,
    facts: { count: found.count, threshold: found.threshold, threshold_method: found.threshold_method },
    commands: [
      ["openreadout", "stats", shown(o.file), "--select", o.select, "--objects", ...statsArgs, "--json"].map(quote).join(" "),
      `openreadout preview ${quote(shown(o.file))} --select ${o.select} ${o.args.join(" ")} --annotate '[{"type": "point", "x": …, "y": …, "label": "1"}, …]' -o ${e.name}.png`,
    ],
  };
}

function plot(e) {
  const spec = join(work, `${e.name}.json`);
  writeFileSync(spec, JSON.stringify(deep(e.plot, real)));
  let meta;
  for (const [suffix, flag] of THEMES) {
    const theme = flag.length ? ["--theme", "dark"] : [];
    meta = run(["plot", spec, ...theme, "-o", join(outDir, `${e.name}${suffix}.png`), "--overwrite"]);
  }
  return {
    meta,
    width: meta.width,
    height: meta.height,
    spec: JSON.stringify(deep(e.plot, shown), null, 2),
    commands: [`openreadout plot ${e.name}.json -o ${e.name}.png`],
  };
}

const previous = existsSync(dataFile) ? JSON.parse(readFileSync(dataFile, "utf8")) : { entries: [] };
const done = [];
try {
  for (const e of manifest.entries) {
    if (only.length && !only.includes(e.name)) {
      const kept = previous.entries.find((x) => x.name === e.name);
      if (kept) done.push(kept);
      continue;
    }
    const t0 = Date.now();
    const r = e.plot ? plot(e) : e.objects ? objects(e) : preview(e);
    const { meta, ...rest } = r;
    done.push({ name: e.name, section: e.section, wide: Boolean(e.wide), title: e.title, caption: e.caption, img: `img/gallery/${e.name}.png`, ...rest });
    console.log(`${e.name}: ${r.width} x ${r.height}, ${Date.now() - t0} ms${meta.notes?.length ? `; notes: ${meta.notes.join("; ")}` : ""}`);
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
const version = execFileSync(bin, ["--version"], { encoding: "utf8" }).trim().split(/\s+/).pop();
writeFileSync(dataFile, JSON.stringify({ version, entries: done }, null, 1) + "\n");
console.log(`wrote ${done.length} figures and ${dataFile}`);
