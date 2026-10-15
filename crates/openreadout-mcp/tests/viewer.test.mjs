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
  const seq = v.THEMES.light.seq;
  assert.deepEqual(v.ramp(seq, 0), v.hexRgb(seq[0]));
  assert.deepEqual(v.ramp(seq, 1), v.hexRgb(seq[seq.length - 1]));
  assert.deepEqual(v.ramp(seq, NaN), v.hexRgb(seq[0]));
  assert.ok(Number.isNaN(v.scaleFn("log")(-1)));
  assert.equal(v.scaleFn("arcsinh")(0), 0);
  for (const s of ["log", "arcsinh", "linear"]) assert.ok(Math.abs(v.scaleInv(s)(v.scaleFn(s)(1234)) - 1234) < 1e-6, s);
  assert.equal(v.percentile([5, 1, 3, NaN], 50), 3);
});

test("transformed axes get ticks at round data values", () => {
  const f = v.scaleFn("arcsinh");
  const t = v.scaleTicks("arcsinh", f(-500), f(50000), 5);
  assert.deepEqual(t.map((x) => x.label), ["−10²", "0", "10²", "10³", "10⁴"]);
  assert.ok(t.every((x) => x.at >= f(-500) && x.at <= f(50000)));
  assert.deepEqual(v.scaleTicks("log", 0, 3, 4).map((x) => x.label), ["10⁰", "10¹", "10²", "10³"]);
  assert.deepEqual(v.scaleTicks("linear", 0, 10, 5).map((x) => x.at), [0, 2, 4, 6, 8, 10]);
});

test("direct labels spread like the figure engine's", () => {
  const p = v.spreadLabels([10, 11, 12, 100], 10, 0, 200);
  assert.ok(p[1] - p[0] >= 9.99 && p[2] - p[1] >= 9.99, String(p));
  assert.equal(p[3], 100);
  assert.ok(Math.abs(p[1] - 11) < 1e-9, "centred on the cluster");
  const q = v.spreadLabels([195, 196], 10, 0, 200);
  assert.ok(q[1] <= 200 && q[1] - q[0] >= 9.99, String(q));
});

test("tick labels match the figure engine's", () => {
  assert.deepEqual(v.tickLabels([0, 5e6, 1e7, 1.5e7]), ["0", "5M", "10M", "15M"]);
  assert.equal(v.fmtTick(20000, 5000), "20k");
  assert.equal(v.fmtTick(0.25, 0.05), "0.25");
  assert.equal(v.fmtTick(0.00002, 0.00001), "2e-5");
  assert.equal(v.fmtTick(1234, 1), "1234");
});

test("units read naturally", () => {
  assert.equal(v.fmtLength(0.22), "220 nm");
  assert.equal(v.fmtLength(33), "33 µm");
  assert.equal(v.fmtLength(5000), "5 mm");
  assert.equal(v.fmtInt(41877.4), "41,877");
  assert.equal(v.timeUnit(18 * 3600).unit, "h");
  assert.equal(v.timeUnit(600).unit, "min");
  assert.equal(v.timeUnit(30).unit, "s");
});

test("wells: names, parsing and the spec the plate tools take", () => {
  assert.equal(v.wellName(0, 0), "A1");
  assert.equal(v.wellName(7, 11), "H12");
  assert.equal(v.wellName(27, 47), "AB48");
  assert.deepEqual(v.parseWell("b07"), [1, 6]);
  assert.deepEqual(v.parseWell("AB48"), [27, 47]);
  assert.equal(v.parseWell("FSC-A"), null);
  assert.equal(v.wellSpec([[0, 0], [0, 1], [1, 0], [1, 1]]), "A1:B2");
  assert.equal(v.wellSpec([[2, 3], [0, 0]]), "A1,C4");
  assert.equal(v.wellSpec([[5, 4]]), "F5");
});

test("plates are coloured by the smallest useful layout map", () => {
  const groups = {
    Sample: { A1: "s1", A2: "s2", A3: "s3", A4: "s4" },
    Group: { A1: "phage", A2: "phage", A3: "none", A4: "Blank" },
    Single: { A1: "x", A2: "x" },
  };
  assert.equal(v.defaultGroupMap(groups), "Group");
  assert.equal(v.defaultGroupMap({}), null);
  assert.ok(v.isBlankGroup("Blank") && v.isBlankGroup(" BL ") && !v.isBlankGroup("lambda"));
  // layout groups take the figure engine's series colours in order, blanks faint
  const pal = { series: v.THEMES.light.series, faint: v.THEMES.light.faint };
  assert.deepEqual(v.categoryColors(["lambda", "BL", "HK97", "lambda"], pal), [pal.series[0], pal.faint, pal.series[1], pal.series[0]]);
  assert.equal(pal.series[1], "#A8432F", "the accent comes second");
  assert.equal(v.seriesGroup("B3 GAPDH"), "GAPDH");
  assert.equal(v.seriesGroup("IN 0"), null);
});

test("annotations come in one shape whatever the spelling", () => {
  const a = v.normalizeAnnotations([
    { kind: "point", x: 10, y: 20, label: "Purkinje cells" },
    { type: "box", region: { x: 1, y: 2, width: 30, height: 40 }, label: "fold" },
    { kind: "rect", x0: 50, y0: 60, x1: 40, y1: 80 },
    { kind: "polygon", points: [[0, 0], [10, 0], [5, 5]] },
    { kind: "arrow", from: [0, 0], to: [5, 5], label: "here" },
    { kind: "band", x_range: [8.6, 7.4], label: "came back" },
    { kind: "hline", y: -60 },
    { kind: "vline", at: 3 },
    { kind: "text", at: [1, 2], text: "note" },
    { kind: "point", wells: ["B3", "C4"], label: "outliers" },
    { kind: "unknown", x: 1 },
    { kind: "point", x: "nope" },
    null,
  ]);
  assert.deepEqual(a.map((x) => x.kind), ["point", "box", "box", "polygon", "line", "band", "hline", "vline", "text", "wells"]);
  assert.deepEqual([a[1].x, a[1].y, a[1].w, a[1].h], [1, 2, 30, 40]);
  assert.deepEqual([a[2].x, a[2].y, a[2].w, a[2].h], [40, 60, 10, 20]);
  assert.equal(a[4].arrow, true);
  assert.deepEqual([a[5].x0, a[5].x1], [7.4, 8.6]);
  assert.equal(a[8].label, "note");
  assert.deepEqual(a[9].wells, [[1, 2], [2, 3]]);
  assert.deepEqual(v.normalizeAnnotations("nope"), []);
});

test("labels do not overlap and stay inside", () => {
  const bounds = { x: 0, y: 0, w: 300, h: 200 };
  const items = [0, 1, 2, 3].map(() => ({ ax: 150, ay: 100, w: 80, h: 20 }));
  const out = v.placeLabels(items, bounds, []);
  for (let i = 0; i < out.length; i++) {
    const r = Object.assign({}, out[i], { w: 80, h: 20 });
    assert.ok(r.x >= 0 && r.y >= 0 && r.x + r.w <= 300 && r.y + r.h <= 200, JSON.stringify(r));
    for (let j = 0; j < i; j++) {
      const q = out[j];
      assert.ok(!(r.x < q.x + 80 && r.x + 80 > q.x && r.y < q.y + 20 && r.y + 20 > q.y), `${i} overlaps ${j}`);
    }
  }
  // a label near the edge is kept inside
  const [p] = v.placeLabels([{ ax: 295, ay: 2, w: 80, h: 20 }], bounds, []);
  assert.ok(p.x + 80 <= 300 && p.y >= 0);
});

test("the camera zooms smoothly and keeps the canvas shape", () => {
  const a = { x: 0, y: 0, w: 1000, h: 500 }, b = { x: 400, y: 200, w: 10, h: 5 };
  assert.deepEqual(v.lerpRect(a, b, 0), a);
  const end = v.lerpRect(a, b, 1);
  assert.ok(Math.abs(end.w - 10) < 1e-9 && Math.abs(end.x - 400) < 1e-9);
  // halfway in time is halfway in zoom steps (geometric), not in width
  const mid = v.lerpRect(a, b, 0.5);
  assert.ok(Math.abs(mid.w - 100) < 1e-6, String(mid.w));
  const f = v.fitAspect({ x: 10, y: 10, w: 100, h: 100 }, 2);
  assert.deepEqual(f, { x: -40, y: 10, w: 200, h: 100 });
  assert.equal(v.ease(0), 0);
  assert.equal(v.ease(1), 1);
});

test("the viewer starts from the result's hint, else the tool's file", () => {
  const hint = { file: "/d/a.czi", view: "image", z: 3 };
  assert.deepEqual(v.hintFrom({ _meta: { "openreadout/view": hint } }, { file: "/other" }), hint);
  assert.deepEqual(v.hintFrom({}, { file: "/d/b.abf" }), { file: "/d/b.abf" });
  const opened = { file: { name: "a.czi", resourceUri: "host-resource://1" } };
  assert.deepEqual(v.hintFrom(null, opened), opened);
  assert.equal(v.hintFrom({}, { roots: ["/d"] }), null);
});
