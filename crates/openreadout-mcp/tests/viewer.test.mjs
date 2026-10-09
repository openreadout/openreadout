// The viewer app's helpers (src/app/web/viewer.js), run with `node --test`. The page itself is
// checked in a host by hand (see MAINTAINING.md); these cover the arithmetic it relies on.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

const require = createRequire(import.meta.url);
const v = require("../src/app/web/viewer.js");

test("ticks are round numbers inside the range", () => {
  assert.deepEqual(v.niceTicks(0, 10, 5), [0, 2, 4, 6, 8, 10]);
  assert.deepEqual(v.niceTicks(10, 0, 5), [0, 2, 4, 6, 8, 10]);
  const t = v.niceTicks(-0.46, -0.44, 3);
  assert.ok(t.length >= 2 && t.every((x) => x >= -0.46 && x <= -0.44), String(t));
  assert.deepEqual(v.niceTicks(NaN, 1, 4), []);
  assert.equal(v.niceBelow(7.3), 5);
  assert.equal(v.niceBelow(0.23), 0.2);
  assert.equal(v.niceBelow(100), 100);
});

test("numbers are short", () => {
  assert.equal(v.fmt(1234.5678), "1235");
  assert.equal(v.fmt(30000000), "3e7");
  assert.equal(v.fmt(0.00012), "1.2e-4");
  assert.equal(v.fmt(null), "–");
});

test("legend labels are short", () => {
  assert.equal(v.legendLabel("A1 FAM", 18), "A1 FAM");
  assert.equal(v.legendLabel("A1 FAM@30116ec1-44f6-4c9c-9c69-5d6f00226d4e", 18), "A1 FAM@30116ec1-4…");
  assert.equal(v.legendLabel(null, 18), "");
});

test("trace x values follow the axis and the window", () => {
  // 4 points of 10 samples each from sample 100, 1 ms per sample
  const plot = { x: { first: 0, step: 0.001 }, first_sample: 100, samples_per_point: 10 };
  const xs = v.plotXs(plot, 4);
  assert.deepEqual(Array.from(xs, (x) => Math.round(x * 1e4) / 1e4), [0.105, 0.115, 0.125, 0.135]);
  // samples plotted one by one
  const raw = v.plotXs({ x: { first: 10, step: -0.5 }, first_sample: 2, samples_per_point: 1 }, 3);
  assert.deepEqual(Array.from(raw), [9, 8.5, 8]);
});

test("a dragged x range becomes a sample window, either axis direction", () => {
  const plot = { x: { first: 0, step: 0.001 }, first_sample: 0, sweep_sample_count: 10000, samples_per_point: 10 };
  assert.deepEqual(v.sampleWindow(plot, 1.0, 2.0), { first_sample: 1000, count: 1001 });
  assert.deepEqual(v.sampleWindow(plot, 2.0, 1.0), { first_sample: 1000, count: 1001 });
  assert.equal(v.sampleWindow(plot, 1.0, 1.001), null, "too narrow to zoom");
  const ppm = { x: { first: 14, step: -0.01 }, first_sample: 0, sweep_sample_count: 1600, samples_per_point: 1 };
  assert.deepEqual(v.sampleWindow(ppm, 8, 7), { first_sample: 600, count: 101 });
});

test("events decode from little-endian float32", () => {
  const f = new Float32Array([1.5, -2, 1e6]);
  const b64 = Buffer.from(f.buffer).toString("base64");
  assert.deepEqual(Array.from(v.f32FromBase64(b64)), [1.5, -2, 1e6]);
  assert.equal(v.f32FromBase64("").length, 0);
});

test("colour scales and display transforms", () => {
  assert.deepEqual(v.viridis(0), [68, 1, 84]);
  assert.deepEqual(v.viridis(1), [253, 231, 37]);
  assert.deepEqual(v.viridis(NaN), [68, 1, 84]);
  assert.ok(Number.isNaN(v.scaleFn("log")(-1)));
  assert.equal(v.scaleFn("arcsinh")(0), 0);
  assert.equal(v.percentile([5, 1, 3, NaN], 50), 3);
});

test("the viewer starts from the result's hint, else the tool's file", () => {
  const hint = { file: "/d/a.czi", view: "image", z: 3 };
  assert.deepEqual(v.hintFrom({ _meta: { "openreadout/view": hint } }, { file: "/other" }), hint);
  assert.deepEqual(v.hintFrom({}, { file: "/d/b.abf" }), { file: "/d/b.abf" });
  const opened = { file: { name: "a.czi", resourceUri: "host-resource://1" } };
  assert.deepEqual(v.hintFrom(null, opened), opened);
  assert.equal(v.hintFrom({}, { roots: ["/d"] }), null);
});
