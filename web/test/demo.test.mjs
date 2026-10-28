// The demo page, assembled by web/build.mjs and driven from Node: the module loads, every
// sample opens and draws with the view the page shows first, a dropped file or folder still
// works, an OME-Zarr image opens from a URL, and the page reaches no other site
// unless you open one. Run `scripts/wasm.sh build` first.
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

test("readouts are short numbers", () => {
  assert.deepEqual([798.4, 36.19, 0.04746, 4087, 12345.6, 2].map(R.short), ["798", "36.2", "0.0475", "4,087", "12,346", "2"]);
  const meta = { image: { full_res_region: { x: 0, y: 0, width: 1600, height: 1500 } } };
  const info = { images: [{ index: 0, size_x: 2220, size_y: 2967, physical_size: { x: 0.499, unit: "µm" } }] };
  const zoom = (z) => R.numbers(info, { kind: "image" }, meta, { zoom: z }).find((n) => n.label === "Zoom").value;
  assert.deepEqual([1.977, 7.43, 1, 12.6].map(zoom), ["2×", "7.4×", "1×", "13×"]);
});

test("the image viewer's camera neither drifts nor shrinks", async () => {
  const C = await import(pathToFileURL(join(out, "camera.js")).href);
  const view = { w: 690, h: 648 };
  const img = { w: 2220, h: 2967 };
  const home = C.showRect({ x: 480, y: 700, width: 1600, height: 1500 }, view, img);
  const { min, max } = C.limits(view, img);
  assert.ok(Math.abs(home.z - Math.min(690 / 1600, 648 / 1500)) < 1e-9);
  // zooming in and out by the same amount around the same point comes back exactly
  let cam = home;
  for (let i = 0; i < 50; i++) cam = C.zoomAt(cam, 1.3, 200, 300, view, img);
  assert.equal(cam.z, max);
  for (let i = 0; i < 200; i++) cam = C.zoomAt(cam, 1 / 1.3, 200, 300, view, img);
  assert.equal(cam.z, min);
  // the image point under the pointer stays there while zooming within the limits
  const under = (c, px, py) => [c.cx + (px - view.w / 2) / c.z, c.cy + (py - view.h / 2) / c.z];
  const a = under(home, 100, 500);
  const b = under(C.zoomAt(home, 1.7, 100, 500, view, img), 100, 500);
  assert.ok(Math.hypot(a[0] - b[0], a[1] - b[1]) < 1e-6);
  // random gestures keep the camera in its limits, with the image covering the viewport
  // (or centred along a side that is smaller than it)
  let seed = 1;
  const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  cam = home;
  for (let i = 0; i < 2000; i++) {
    cam = rnd() < 0.5 ? C.zoomAt(cam, 0.5 + rnd() * 1.5, rnd() * view.w, rnd() * view.h, view, img) : C.panBy(cam, (rnd() - 0.5) * 900, (rnd() - 0.5) * 900, view, img);
    assert.ok(cam.z >= min && cam.z <= max, `zoom ${cam.z}`);
    const v = C.visible(cam, view, img);
    const across = (span, size, c, x0, w) => (span >= size ? Math.abs(c - size / 2) < 1e-9 : x0 >= 0 && x0 + w <= size + 1e-9 && Math.abs(w - span) < 1e-6);
    assert.ok(across(view.w / cam.z, img.w, cam.cx, v.x, v.width), `x at ${i}`);
    assert.ok(across(view.h / cam.z, img.h, cam.cy, v.y, v.height), `y at ${i}`);
    const r = C.regionFor(cam, view, img);
    if (r) assert.ok(r.x >= 0 && r.y >= 0 && r.x + r.width <= img.w && r.y + r.height <= img.h && Number.isInteger(r.width));
  }
  // the whole image needs no region; a picture of a region is placed where the camera puts it
  assert.equal(C.regionFor(C.clamp({ cx: 0, cy: 0, z: min }, view, img), view, img), undefined);
  const p = C.place({ x: home.cx, y: home.cy, width: 10, height: 10 }, home, view);
  assert.deepEqual([p.x, p.y], [view.w / 2, view.h / 2]);
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

test("the page reaches another site only to read a URL you open", () => {
  const html = readFileSync(join(out, "index.html"), "utf8");
  assert.match(html, /Your file stays on your device\./);
  const csp = html.match(/http-equiv="Content-Security-Policy"\s+content="([^"]+)"/)[1];
  assert.match(csp, /default-src 'none'/);
  assert.match(csp, /connect-src 'self' https:;/);
  // the worker starts from a blob: URL, so it keeps this policy
  assert.match(csp, /worker-src blob:;/);
  assert.match(csp, /form-action 'none'/);
  assert.doesNotMatch(csp, /http:|\*|data:/);
  // Only store.js fetches from another site, with GET and HEAD and no body. Every other fetch
  // takes a path on this site, and nothing uses XHR, WebSocket, beacons or server-sent events.
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
    if (p.endsWith("store.js")) {
      assert.doesNotMatch(src, /method:\s*["'](POST|PUT|PATCH|DELETE)|body:/, p);
      continue;
    }
    // samples.js holds the credit links, which the page shows as links to follow
    if (!p.endsWith("samples.js")) assert.doesNotMatch(src, /["'`]https?:\/\//, p);
    assert.doesNotMatch(src, /fetch\(\s*["'`](https?:|\/\/)/, p);
  }
});

/** The files of the zipped OME-Zarr fixture, by path inside the image's folder. */
function zarrFixture() {
  const dir = mkdtempSync(join(tmpdir(), "openreadout-zarr-"));
  execFileSync("unzip", ["-q", join(root, "crates/openreadout-zarr/tests/fixtures/ngff04-v2-tczyx-blosc-lz4.zip"), "-d", dir]);
  const files = new Map();
  const walk = (d, rel) => {
    for (const e of readdirSync(d)) {
      const p = join(d, e);
      if (statSync(p).isDirectory()) walk(p, `${rel}${e}/`);
      else files.set(`${rel}${e}`, readFileSync(p));
    }
  };
  walk(dir, "");
  rmSync(dir, { recursive: true, force: true });
  return files;
}

test("an OME-Zarr image opens from a URL, fetching only what it reads", async () => {
  const files = zarrFixture();
  const base = "https://data.example.org/images/image.zarr";
  const asked = [];
  const real = globalThis.fetch;
  // A server that answers range requests and, like S3, hides Content-Range from the page.
  globalThis.fetch = async (url, options = {}) => {
    const path = String(url).slice(base.length + 1);
    asked.push(`${options.method ?? "GET"} ${path}`);
    const body = files.get(path);
    if (!String(url).startsWith(base) || !body) return new Response(null, { status: 404 });
    if (options.method === "HEAD") return new Response(null, { headers: { "content-length": String(body.length) } });
    const [, from, to] = /bytes=(\d+)-(\d+)/.exec(options.headers.Range);
    return new Response(body.subarray(Number(from), Number(to) + 1), { status: 206 });
  };
  try {
    const r = await R.analyze({ url: base });
    assert.ok(r.info.ok && r.explain.ok && r.preview.ok, JSON.stringify(r, null, 1));
    assert.equal(r.info.value.format.id, "ome-zarr");
    assert.equal(r.name, "image.zarr");
  } finally {
    globalThis.fetch = real;
  }
  // Nothing is listed, and nothing is fetched twice.
  assert.ok(asked.every((a) => /^(GET|HEAD) /.test(a)));
  assert.equal(new Set(asked).size, asked.length, "a file was fetched twice");
});

test("a dropped .zarr folder opens as one image", async () => {
  const folder = [...zarrFixture()].map(([path, bytes]) => ({ path: `image.zarr/${path}`, data: new Blob([bytes]) }));
  const r = await R.analyze({ folder, root: "image.zarr" });
  assert.ok(r.info.ok && r.preview.ok, JSON.stringify(r, null, 1));
  assert.equal(r.info.value.format.id, "ome-zarr");
});

test("a file opens from a URL, finding its companion files next to it", async () => {
  // A Bruker EPR spectrum: the descriptor (.DSC) names the data file (.DTA) beside it.
  const dsc = "#DESC\t1.2\nDSRC\tEXP\nBSEQ\tBIG\nIKKF\tREAL\nXTYP\tIDX\nYTYP\tNODATA\nZTYP\tNODATA\nIRFMT\tD\nXPTS\t3\nXMIN\t3300.0\nXWID\t100.0\nTITL\t'test'\nXNAM\t'Field'\nXUNI\t'G'\n";
  const dta = new Uint8Array(24);
  [1.5, -2.0, 4.25].forEach((x, i) => new DataView(dta.buffer).setFloat64(i * 8, x));
  const files = new Map([["a.DSC", new TextEncoder().encode(dsc)], ["a.DTA", dta]]);
  const asked = [];
  const real = globalThis.fetch;
  // Zenodo's file addresses end in /content, after the file's name.
  globalThis.fetch = async (url, options = {}) => {
    asked.push(`${options.method ?? "GET"} ${url}`);
    const m = /^https:\/\/data\.example\.org\/records\/1\/files\/([^/]+)\/content$/.exec(String(url));
    const body = m && files.get(decodeURIComponent(m[1]));
    if (!body) return new Response(null, { status: 404 });
    if (options.method === "HEAD") return new Response(null, { headers: { "content-length": String(body.length) } });
    const [, from, to] = /bytes=(\d+)-(\d+)/.exec(options.headers.Range);
    return new Response(body.subarray(Number(from), Number(to) + 1), { status: 206 });
  };
  try {
    const r = await R.analyze({ url: "https://data.example.org/records/1/files/a.DSC/content" });
    assert.ok(r.info.ok, JSON.stringify(r, null, 1));
    assert.equal(r.info.value.format.id, "bruker-bes3t");
    assert.equal(r.name, "a.DSC");
  } finally {
    globalThis.fetch = real;
  }
  assert.ok(asked.some((a) => a.includes("/a.DTA/content")), "the data file was not looked up");
});

test("a URL without the file's name takes it from where it redirects", async () => {
  const czi = readFileSync(join(root, "crates/openreadout-cli/tests/fixtures/mini.czi"));
  const real = globalThis.fetch;
  // Figshare's URLs are numbers that redirect to an address ending in the file's name.
  globalThis.fetch = async (url, options = {}) => {
    if (String(url) !== "https://files.example.org/files/123") return new Response(null, { status: 404 });
    const res =
      options.method === "HEAD"
        ? new Response(null, { headers: { "content-length": String(czi.length) } })
        : (() => {
            const [, from, to] = /bytes=(\d+)-(\d+)/.exec(options.headers.Range);
            return new Response(czi.subarray(Number(from), Number(to) + 1), { status: 206 });
          })();
    Object.defineProperty(res, "url", { value: "https://bucket.example.org/123/mini.czi?signature=x" });
    return res;
  };
  try {
    const r = await R.analyze({ url: "https://files.example.org/files/123" });
    assert.ok(r.info.ok, JSON.stringify(r, null, 1));
    assert.equal(r.info.value.format.id, "czi");
    assert.equal(r.name, "mini.czi");
  } finally {
    globalThis.fetch = real;
  }
});

test("URLs are read as the files and folders they name", async () => {
  const { webAddress, addressName } = await import(pathToFileURL(join(out, "store.js")).href);
  assert.equal(webAddress(" https://host.org/a/image.zarr/ "), "https://host.org/a/image.zarr");
  assert.equal(webAddress("https://host.org/a/image.zarr/zarr.json"), "https://host.org/a/image.zarr");
  assert.equal(webAddress("https://host.org/a/image.zarr/.zattrs"), "https://host.org/a/image.zarr");
  assert.equal(webAddress("s3://bucket/a/image.zarr"), "https://bucket.s3.amazonaws.com/a/image.zarr");
  assert.equal(webAddress("https://host.org/run.czi?sig=1"), "https://host.org/run.czi?sig=1");
  assert.equal(webAddress("https://zenodo.org/records/7015307/files/a%3D1.czi?download=1"), "https://zenodo.org/api/records/7015307/files/a%3D1.czi/content");
  assert.equal(webAddress("https://github.com/org/repo/blob/main/data/x.fcs"), "https://raw.githubusercontent.com/org/repo/main/data/x.fcs");
  assert.equal(addressName("https://host.org/a/image%20one.zarr"), "image one.zarr");
  assert.equal(addressName("https://zenodo.org/api/records/1/files/a%3D1.czi/content"), "a=1.czi");
  assert.throws(() => webAddress("http://host.org/image.zarr"), /https/);
  assert.throws(() => webAddress("not an address"), /not a URL/);
});
