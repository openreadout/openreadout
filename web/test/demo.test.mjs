// The demo page, assembled by web/build.mjs and driven from Node: the module loads, every
// sample opens and draws with the view the page shows first, a dropped file still works, and
// the page can't reach any other site. Run `scripts/wasm.sh build` first.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
let out;
let R;
let SAMPLES;
let BUILT;

before(async () => {
  out = mkdtempSync(join(tmpdir(), "openreadout-demo-"));
  execFileSync(process.execPath, [join(root, "web/build.mjs"), out], { stdio: "pipe" });
  R = await import(pathToFileURL(join(out, "reader.js")).href);
  ({ SAMPLES } = await import(pathToFileURL(join(out, "samples.js")).href));
  ({ BUILT } = await import(pathToFileURL(join(out, "built.js")).href));
});

after(() => rmSync(out, { recursive: true, force: true }));

const fixture = (rel) => readFileSync(join(root, rel));

test("the module loads", async () => {
  await R.ready();
  assert.ok(R.isReady());
});

test("every sample opens, checks clean and draws its first view", async () => {
  const kinds = {};
  for (const s of SAMPLES) {
    const bytes = readFileSync(join(out, "samples", s.file));
    assert.equal(bytes.length, BUILT.samples[s.id].bytes, s.file);
    const r = await R.openFile(bytes, s.file);
    try {
      assert.ok(r.info.ok && r.explain.ok && r.check.ok, `${s.file}: ${JSON.stringify([r.info.error, r.explain.error, r.check.error])}`);
      assert.equal(r.check.value.ok, true, s.file);
      assert.ok(R.facts(r.info.value).length >= 2, `${s.file}: facts`);
      const view = R.firstView(r.info.value, s.view);
      const p = await R.draw(r.file, r.info.value, view, { width: 360, scale: 2 });
      // drawn 360 CSS pixels across (images: at most) at two device pixels per CSS pixel
      if (p.meta.image) assert.ok(p.meta.width <= 722 && p.meta.width > 300, `${s.file}: ${p.meta.width}`);
      else assert.equal(p.meta.width, 720, s.file);
      kinds[s.id] = p.meta.kind;
      assert.ok(R.numbers(r.info.value, view, p.meta).length > 0, `${s.file}: numbers`);
      assert.match(R.command(s.file, r.info.value, view), /^openreadout preview /);
    } finally {
      r.file.close();
    }
  }
  assert.equal(kinds.slide, "image");
  assert.equal(kinds.confocal, "image");
  assert.equal(kinds.plate, "plate");
  assert.equal(kinds.flow, "events");
});

test("the neuron sample marks its action potentials, the GC-MS run finds a spectrum", async () => {
  const neuron = SAMPLES.find((s) => s.id === "neuron");
  let r = await R.openFile(readFileSync(join(out, "samples", neuron.file)), neuron.file);
  const view = R.firstView(r.info.value, neuron.view);
  assert.equal(view.marks, "spikes");
  const p = await R.draw(r.file, r.info.value, view, { width: 400 });
  assert.equal(p.meta.marks.filter((m) => m.kind === "spike").length, 42);
  r.file.close();

  const gcms = SAMPLES.find((s) => s.id === "gcms");
  r = await R.openFile(readFileSync(join(out, "samples", gcms.file)), gcms.file);
  const scan = await R.scanNear(r.file, 0, 6.65, 0.03);
  assert.ok(scan && Math.abs(scan.rt_s / 60 - 6.65) < 0.05);
  const sp = await R.draw(r.file, r.info.value, { kind: "plot", run: 0, spectrum: scan.index }, { width: 400 });
  assert.equal(sp.meta.kind, "spectrum");
  r.file.close();
});

test("zooming keeps the region inside the image and its shape", () => {
  const im = { size_x: 2000, size_y: 1000 };
  assert.equal(R.fitRegion(im, 1000, 500, 2000, 1000), undefined);
  assert.deepEqual(R.fitRegion(im, 1990, 10, 400, 200), { x: 1600, y: 0, width: 400, height: 200 });
  assert.deepEqual(R.fitRegion(im, 1000, 500, 10, 5, 48), { x: 952, y: 476, width: 96, height: 48 });
});

test("a dropped CZI is explained, checked and previewed", async () => {
  const r = await R.analyze(new File([fixture("crates/openreadout-cli/tests/fixtures/mini.czi")], "mini.czi"));
  assert.ok(r.info.ok && r.explain.ok && r.check.ok && r.preview.ok, JSON.stringify(r, null, 1));
  assert.equal(r.info.value.format.id, "czi");
  assert.ok(r.explain.value.summary.length > 0);
  assert.equal(r.preview.value.type, "image/jpeg");
});

test("an unreadable file shows the CLI's error and hint", async () => {
  const r = await R.analyze(new File([new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9])], "notes.bin"));
  assert.equal(r.info.ok, false);
  assert.equal(r.info.error.code, "unknown_format");
  assert.ok(r.info.error.hint);
});

test("every sample has a card, a credit and a notice", () => {
  const html = readFileSync(join(out, "index.html"), "utf8");
  const notice = readFileSync(join(out, "samples/NOTICE.txt"), "utf8");
  for (const s of SAMPLES) {
    assert.match(html, new RegExp(`data-sample="${s.id}"`));
    assert.ok(html.includes(s.href), `${s.id}: credit link`);
    assert.ok(notice.includes(s.file), `${s.id}: notice`);
    for (const t of Object.values(BUILT.samples[s.id].thumb)) assert.ok(existsSync(join(out, t.src)), t.src);
    if (BUILT.samples[s.id].gz) assert.ok(existsSync(join(out, "samples", `${s.file}.gz`)));
  }
});

test("the page states and enforces that it reaches no other site", () => {
  const html = readFileSync(join(out, "index.html"), "utf8");
  assert.match(html, /Your file stays on your device\./);
  const csp = html.match(/http-equiv="Content-Security-Policy"\s+content="([^"]+)"/)[1];
  assert.match(csp, /default-src 'none'/);
  assert.match(csp, /connect-src 'self';/);
  assert.match(csp, /form-action 'none'/);
  assert.doesNotMatch(csp, /https?:|\*|data:/);
  // No script talks to another origin: fetch only takes relative paths, and nothing uses
  // XHR, WebSocket, beacons or server-sent events.
  const scripts = [];
  const walk = (d) => {
    for (const e of readdirSync(d)) {
      const p = join(d, e);
      if (statSync(p).isDirectory()) walk(p);
      else if (p.endsWith(".js")) scripts.push(p);
    }
  };
  walk(out);
  for (const p of scripts) {
    const src = readFileSync(p, "utf8");
    assert.doesNotMatch(src, /\b(XMLHttpRequest|WebSocket|sendBeacon|EventSource)\b/, p);
    // samples.js holds the credit links, which the page shows as links to follow
    if (!p.endsWith("samples.js")) assert.doesNotMatch(src, /["'`]https?:\/\//, p);
    assert.doesNotMatch(src, /fetch\(\s*["'`](https?:|\/\/)/, p);
  }
});
