// Assemble the demo page into a directory: node web/build.mjs <out-dir>
//
// Copies the page (index.html, style.css and the scripts), the npm wrapper
// (packaging/wasm/index.js) and the wasm-bindgen glue and module into <out>/openreadout/, the
// fonts into <out>/fonts/ and the sample files into <out>/samples/, each also gzipped when
// that saves space (the page unpacks them with DecompressionStream). It then opens every
// sample with the module, in Node, to draw the card pictures and check that each one reads.
// Run `scripts/wasm.sh build` first.
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const out = resolve(process.argv[2] ?? join(root, "book/book/demo"));
const pkg = join(root, "packaging/wasm/pkg");
const wasm = join(pkg, "openreadout_bg.wasm");

if (!existsSync(wasm)) {
  console.error(`error: ${wasm} is missing; run scripts/wasm.sh build`);
  process.exit(1);
}

for (const d of ["openreadout/pkg", "fonts", "samples/thumbs"]) mkdirSync(join(out, d), { recursive: true });

// The page and its scripts.
for (const f of ["style.css", "theme.js", "app.js", "camera.js", "engine.js", "core.js", "worker.js", "reader.js", "store.js", "samples.js"]) copyFileSync(join(here, f), join(out, f));
copyFileSync(join(root, "packaging/wasm/index.js"), join(out, "openreadout/index.js"));
copyFileSync(join(pkg, "openreadout.js"), join(out, "openreadout/pkg/openreadout.js"));
copyFileSync(wasm, join(out, "openreadout/pkg/openreadout_bg.wasm"));

// Fonts: IBM Plex from the MCP viewer (committed), Newsreader from the site's packages when
// they are installed (`npm ci` in book/); without it the headings use the system's serif.
const plex = join(root, "crates/openreadout-mcp/src/app/web/fonts");
for (const f of ["sans-regular.woff", "sans-medium.woff", "mono-regular.woff", "OFL.txt"]) copyFileSync(join(plex, f), join(out, "fonts", f));
const newsreader = join(root, "book/node_modules/@fontsource-variable/newsreader/files/newsreader-latin-opsz-normal.woff2");
if (existsSync(newsreader)) copyFileSync(newsreader, join(out, "fonts/newsreader.woff2"));
else console.warn("note: Newsreader not found (npm ci in book/); the demo's headings use the system serif");

// The samples: the files, gzipped copies, and a picture for each card drawn by OpenReadout.
const { SAMPLES } = await import(pathToFileURL(join(here, "samples.js")).href);
const { load, open } = await import(pathToFileURL(join(root, "packaging/wasm/index.js")).href);
await load();
copyFileSync(join(here, "samples/NOTICE.txt"), join(out, "samples/NOTICE.txt"));

const built = { wasm: { bytes: readFileSync(wasm).length }, samples: {} };
const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
const cards = [];
const credits = [];
for (const s of SAMPLES) {
  const bytes = readFileSync(join(here, "samples", s.file));
  copyFileSync(join(here, "samples", s.file), join(out, "samples", s.file));
  const gz = gzipSync(bytes, { level: 9 });
  const packed = gz.length < bytes.length * 0.9;
  if (packed) writeFileSync(join(out, "samples", `${s.file}.gz`), gz);

  const f = await open(bytes, { name: s.file });
  const info = await f.info();
  const image = info.images.length > 0;
  const thumb = {};
  for (const dark of image ? [false] : [false, true]) {
    const o = image
      ? { ...s.thumb, composite: info.images[0].samples_per_pixel === 1 && info.images[0].size_c > 1, axes: "none", max_size: 304, format: "jpeg" }
      : { ...s.thumb, max_size: 300, format: "png", dark };
    const p = await f.preview(o);
    const name = `${s.id}${dark ? "-dark" : ""}.${image ? "jpg" : "png"}`;
    writeFileSync(join(out, "samples/thumbs", name), p.data);
    thumb[dark ? "dark" : "light"] = { src: `samples/thumbs/${name}`, width: p.meta.width, height: p.meta.height };
  }
  f.close();
  built.samples[s.id] = { bytes: bytes.length, gz: packed ? gz.length : null, image, thumb };

  const t = thumb.light;
  const img = `<img src="${t.src}" alt="" width="${t.width}" height="${t.height}" loading="lazy" decoding="async" />`;
  const pic = thumb.dark ? `<picture><source srcset="${thumb.dark.src}" media="(prefers-color-scheme: dark)" />${img}</picture>` : img;
  cards.push(
    `<li><button type="button" class="card" data-sample="${s.id}" aria-pressed="false" aria-label="${esc(`${s.label}: ${s.subject} (${s.format})`)}">` +
      `<span class="thumb${image ? "" : " plot"}">${pic}</span>` +
      `<span class="label">${esc(s.label)}</span><span class="format">${esc(s.format)}</span></button></li>`,
  );
  credits.push(`<a href="${esc(s.href)}">${esc(s.label.toLowerCase())}</a>, ${esc(s.credit)} (${esc(s.licence)});`);
}

writeFileSync(
  join(out, "built.js"),
  `// Written by web/build.mjs: sizes of the module and the samples, and the card pictures.\nexport const BUILT = ${JSON.stringify(built, null, 1)};\n`,
);
const html = readFileSync(join(here, "index.html"), "utf8")
  .replace("<!-- CARDS -->", cards.join("\n          "))
  .replace("<!-- CREDITS -->", credits.join("\n        "));
writeFileSync(join(out, "index.html"), html);

const total = Object.values(built.samples).reduce((n, s) => n + (s.gz ?? s.bytes), 0);
console.log(
  `demo: ${out} (module ${built.wasm.bytes} bytes; ${SAMPLES.length} samples, ${total} bytes to download in all)`,
);
