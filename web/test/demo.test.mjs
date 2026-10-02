// The demo page, assembled by web/build.mjs and driven from Node: the module loads from the
// inlined copy, a dropped File is explained, summarised, checked and previewed, and the page
// cannot reach the network. Run `scripts/wasm.sh build` first.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
let out;
let app;

before(async () => {
  out = mkdtempSync(join(tmpdir(), "openreadout-demo-"));
  execFileSync(process.execPath, [join(root, "web/build.mjs"), out], { stdio: "pipe" });
  app = await import(pathToFileURL(join(out, "app.js")).href);
});

after(() => rmSync(out, { recursive: true, force: true }));

const fixture = (rel) => readFileSync(join(root, rel));

test("the module loads from the inlined copy", async () => {
  await app.ready();
});

test("a dropped CZI is explained, summarised, checked and previewed", async () => {
  const file = new File([fixture("crates/openreadout-cli/tests/fixtures/mini.czi")], "mini.czi");
  const r = await app.analyze(file);
  assert.ok(r.info.ok && r.explain.ok && r.check.ok && r.preview.ok, JSON.stringify(r, null, 1));
  const m = app.model(r);
  assert.equal(m.title, "mini.czi");
  assert.match(m.subtitle, /Zeiss CZI/);
  assert.ok(m.explain.summary.length > 0);
  assert.equal(JSON.parse(m.info).format.id, "czi");
  assert.equal(m.check.ok, true);
  assert.deepEqual([...m.preview.png.subarray(0, 4)], [0x89, 0x50, 0x4e, 0x47]);
});

test("an unreadable file shows the CLI's error and hint", async () => {
  const file = new File([new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9])], "notes.bin");
  const m = app.model(await app.analyze(file));
  assert.match(m.subtitle, /unrecognised/);
  assert.equal(m.infoError.code, "unknown_format");
  assert.ok(m.infoError.hint);
});

test("the page states and enforces that nothing is uploaded", () => {
  const html = readFileSync(join(out, "index.html"), "utf8");
  assert.match(html, /Your file stays on your computer\./);
  const csp = html.match(/http-equiv="Content-Security-Policy"\s+content="([^"]+)"/)[1];
  assert.match(csp, /connect-src 'none'/);
  assert.match(csp, /form-action 'none'/);
  assert.match(csp, /default-src 'none'/);
  assert.doesNotMatch(csp, /https?:/);
  // No script refers to another origin, and none calls fetch/XHR/WebSocket/sendBeacon on
  // anything but the (unused, since inlined) module URL of the wasm-bindgen glue.
  const scripts = [];
  const walk = (d) => {
    for (const e of readdirSync(d)) {
      const p = join(d, e);
      if (statSync(p).isDirectory()) walk(p);
      else if (p.endsWith(".js") && !p.endsWith("wasm-inline.js")) scripts.push(p);
    }
  };
  walk(out);
  for (const p of scripts) {
    const src = readFileSync(p, "utf8");
    assert.doesNotMatch(src, /\b(XMLHttpRequest|WebSocket|sendBeacon|EventSource)\b/, p);
    assert.doesNotMatch(src, /["'`]https?:\/\//, p);
  }
});
