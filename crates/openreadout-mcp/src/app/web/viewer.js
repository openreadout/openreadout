// OpenReadout viewer: an MCP App (io.modelcontextprotocol/ui) that draws what openreadout_view
// returns, shows the agent's annotations, and lets the user point at things and send them to
// the agent. It talks to the host with JSON-RPC over postMessage, loads nothing from the network
// and keeps no state between sessions. Plain browser JavaScript with no dependencies, so the
// server embeds this file as it is (see crates/openreadout-mcp/MAINTAINING.md).
"use strict";

const VIEW_TOOL = "openreadout_view";
const PROTOCOL_VERSION = "2026-01-26";
const VIEW_LABELS = {
  image: "Image", nmr: "NMR", chromatogram: "Chromatogram", spectrum: "Spectrum",
  fcs: "Events", plate: "Plate", trace: "Trace", summary: "Summary",
};
// Keys that select a zoomed window; a new selection or view clears them.
const ZOOM_KEYS = ["region", "first_sample", "count", "x_range"];

// The OpenReadout look, the figure engine's: ink on paper, a warm red accent, quiet axes,
// direct labels. theme.json holds openreadout-figure's Theme tokens (a Rust test keeps it
// current); the server puts it in place of the placeholder below. Chrome colours come from the
// host; these are the data colours.
const TOKENS = /*TOKENS*/null;
function themesFrom(t) {
  const one = (m, bandAlpha) => {
    const th = (t && t[m]) || {};
    const [r, g, b] = hexRgb(th.accent || "#a8432f");
    return {
      accent: th.accent, faint: th.faint, ink: th.ink, muted: th.muted, paper: th.paper, panel: th.panel,
      band: `rgba(${r},${g},${b},${bandAlpha})`,
      series: th.series || [], seq: th.sequential || [], div: th.diverging || [],
    };
  };
  return { light: one("light", 0.11), dark: one("dark", 0.16), blanks: (t && t.blank_labels) || [] };
}
const THEMES = themesFrom(TOKENS || (typeof require === "function" ? require("./theme.json") : null));
const MONO = '"OpenReadout Mono", "IBM Plex Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace';
// [family, weight, base64 WOFF]: subsets of IBM Plex, filled in by the server (src/app/mod.rs).
const FONTS = /*FONTS*/[];

// ---------------------------------------------------------------------------------------------
// Pure helpers (also loaded by the Node tests)

/** Tick positions for [lo, hi]: multiples of 1, 2 or 5 × 10^k, about `want` of them. */
function niceTicks(lo, hi, want) {
  if (!(isFinite(lo) && isFinite(hi))) return [];
  if (lo > hi) [lo, hi] = [hi, lo];
  if (hi - lo < 1e-300) return [lo];
  const raw = (hi - lo) / Math.max(1, want);
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = [1, 2, 5, 10].map((m) => m * mag).find((s) => s >= raw) || 10 * mag;
  const out = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) {
    out.push(Math.abs(v) < step * 1e-9 ? 0 : Number(v.toPrecision(12)));
  }
  return out;
}

/** The largest 1, 2 or 5 × 10^k that is at most v (v > 0). */
function niceBelow(v) {
  if (!(v > 0)) return 0;
  const mag = Math.pow(10, Math.floor(Math.log10(v)));
  return [5, 2, 1].map((m) => m * mag).find((x) => x <= v) || mag;
}

/** A short label for a number. */
function fmt(v, digits) {
  if (v === null || v === undefined || !isFinite(v)) return "–";
  const a = Math.abs(v);
  if (a !== 0 && (a >= 1e6 || a < 1e-3)) {
    const [m, e] = v.toExponential(Math.max(1, (digits || 3) - 1)).split("e");
    return `${Number(m)}e${Number(e)}`;
  }
  const s = Number(v.toPrecision(digits || 4));
  return String(s);
}

/** A value to read in a tooltip: `digits` significant digits, SI suffixes from a million up
 * (`28.77 M`), and a power of ten written out below 0.001 (`4.392 × 10⁻⁴`). */
function fmtValue(v, digits) {
  if (v === null || v === undefined || !isFinite(v)) return "–";
  if (v < 0) return "−" + fmtValue(-v, digits);
  const a = Math.abs(v);
  const d = digits || 4;
  if (a >= 1e6 && a < 1e15) {
    const [k, s] = a >= 1e12 ? [1e12, "T"] : a >= 1e9 ? [1e9, "G"] : [1e6, "M"];
    return `${Number((v / k).toPrecision(d))} ${s}`;
  }
  if (a !== 0 && (a < 1e-3 || a >= 1e15)) {
    const e = Math.floor(Math.log10(a));
    const m = Number((v / Math.pow(10, e)).toPrecision(d));
    return `${m} × 10${supers(e)}`;
  }
  return String(Number(v.toPrecision(d)));
}

/** A tick label, as the figure engine writes them (openreadout-figure `format_tick`): as many
 * decimals as the step needs, SI suffixes from 10 000 up (`20k`, `1.5M`), an exponent below
 * 0.001. */
function fmtTick(v, step) {
  if (v === null || v === undefined || !isFinite(v)) return "–";
  if (v === 0) return "0";
  const a = Math.abs(v);
  step = isFinite(step) && step > 0 ? step : a;
  const dec = (s) => {
    if (s >= 1 || s <= 0) return 0;
    const base = Math.max(0, Math.ceil(-Math.log10(s)));
    const scaled = s * Math.pow(10, base);
    return Math.abs(scaled - Math.round(scaled)) > 1e-6 ? base + 1 : base;
  };
  const trim = (x) => (x.includes(".") ? x.replace(/0+$/, "").replace(/\.$/, "") : x);
  if (a >= 1e4) {
    const [div, suffix] = a >= 1e12 ? [1e12, "T"] : a >= 1e9 ? [1e9, "G"] : a >= 1e6 ? [1e6, "M"] : [1e3, "k"];
    return trim((v / div).toFixed(Math.min(3, dec(step / div)))) + suffix;
  }
  if (a < 1e-3) {
    const e = Math.floor(Math.log10(a));
    return `${trim((v / Math.pow(10, e)).toFixed(1))}e${e}`;
  }
  return trim(v.toFixed(Math.min(6, dec(step))));
}

/** Labels for a run of ticks (their step sets the decimals). */
function tickLabels(ticks) {
  const step = ticks.length > 1 ? Math.abs(ticks[1] - ticks[0]) : Math.abs(ticks[0] || 1);
  return ticks.map((t) => fmtTick(t, step));
}

/** A whole number with thousands separators (pixel coordinates, counts). */
function fmtInt(v) {
  if (!isFinite(v)) return "–";
  return Math.round(v).toLocaleString("en-US");
}

/** A length in µm in the unit that reads best (nm, µm, mm). */
function fmtLength(um, digits) {
  if (!isFinite(um)) return "–";
  const a = Math.abs(um);
  if (a >= 1000) return `${fmt(um / 1000, digits || 4)} mm`;
  if (a < 1 && a > 0) return `${fmt(um * 1000, digits || 4)} nm`;
  return `${fmt(um, digits || 4)} µm`;
}

/** A duration in seconds in the unit that reads best for a run lasting `span` seconds. */
function timeUnit(span) {
  if (span >= 3 * 3600) return { unit: "h", k: 3600 };
  if (span >= 180) return { unit: "min", k: 60 };
  return { unit: "s", k: 1 };
}

/** A legend entry: the series name cut to `max` characters. */
function legendLabel(name, max) {
  const s = String(name || "");
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}

/** Label positions near `targets`, at least `gap` apart and within [lo, hi] when they fit,
 * each as close to its target as the others allow (the figure engine's `spread`). */
function spreadLabels(targets, gap, lo, hi) {
  const order = targets.map((t, i) => i).sort((a, b) => targets[a] - targets[b]);
  const pos = order.map((i) => Math.min(hi, Math.max(lo, targets[i])));
  // pool adjacent violators: each block sits at the mean of its shifted targets
  const blocks = [];
  pos.forEach((p, k) => {
    blocks.push({ m: p - gap * k, s: k, n: 1 });
    while (blocks.length > 1 && blocks[blocks.length - 2].m > blocks[blocks.length - 1].m) {
      const b = blocks.pop(), a = blocks.pop();
      blocks.push({ m: (a.m * a.n + b.m * b.n) / (a.n + b.n), s: a.s, n: a.n + b.n });
    }
  });
  for (const b of blocks) for (let k = b.s; k < b.s + b.n; k++) pos[k] = b.m + gap * k;
  const over = pos.length ? pos[pos.length - 1] - hi : 0;
  if (over > 0) for (let k = 0; k < pos.length; k++) pos[k] -= over;
  const under = pos.length ? lo - pos[0] : 0;
  if (under > 0) for (let k = 0; k < pos.length; k++) pos[k] += under;
  const out = new Array(targets.length);
  order.forEach((i, k) => { out[i] = pos[k]; });
  return out;
}

/** "#rrggbb" → [r, g, b]. */
function hexRgb(h) {
  const n = parseInt(String(h).replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** A CSS colour as the browser resolves it (`rgb(r, g, b)`, `rgba(…)`, `#rrggbb`) → "#rrggbb",
 * or null when it is not one of those (or is transparent). */
function cssHex(c) {
  const s = String(c || "").trim();
  if (/^#[0-9a-f]{6}$/i.test(s)) return s.toLowerCase();
  const m = s.match(/^rgba?\(([^)]*)\)$/i);
  if (!m) return null;
  const v = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
  if (v.length < 3 || v.slice(0, 3).some((x) => !isFinite(x)) || (v.length > 3 && v[3] < 0.5)) return null;
  return "#" + v.slice(0, 3).map((x) => Math.max(0, Math.min(255, Math.round(x))).toString(16).padStart(2, "0")).join("");
}

/** Colour at t in [0, 1] along `stops` (hex strings), as [r, g, b]. */
function ramp(stops, t) {
  t = Math.min(1, Math.max(0, isFinite(t) ? t : 0));
  const f = t * (stops.length - 1);
  const i = Math.min(stops.length - 2, Math.floor(f));
  const u = f - i;
  const a = hexRgb(stops[i]), b = hexRgb(stops[i + 1]);
  return [0, 1, 2].map((k) => Math.round(a[k] + (b[k] - a[k]) * u));
}

/** The x value of each point of a trace or NMR plot. */
function plotXs(plot, n) {
  const xs = new Float64Array(n);
  if (Array.isArray(plot.xs)) {
    for (let k = 0; k < n; k++) xs[k] = plot.xs[k] == null ? NaN : plot.xs[k];
    return xs;
  }
  const ax = plot.x || {};
  const first = Number(ax.first ?? 0), step = Number(ax.step ?? 1);
  const spb = plot.samples_per_point > 1 ? plot.samples_per_point : 1;
  const half = spb > 1 ? 0.5 : 0;
  for (let k = 0; k < n; k++) xs[k] = first + step * (plot.first_sample + (k + half) * spb);
  return xs;
}

/** Sample window [first, count] of a trace plot that covers x in [a, b]. */
function sampleWindow(plot, a, b) {
  const ax = plot.x || {};
  const total = plot.sweep_sample_count;
  let i0, i1;
  if (Array.isArray(plot.xs)) {
    const spb = plot.samples_per_point > 1 ? plot.samples_per_point : 1;
    const ks = [];
    plot.xs.forEach((x, k) => {
      if (x != null && x >= Math.min(a, b) && x <= Math.max(a, b)) ks.push(k);
    });
    if (!ks.length) return null;
    i0 = plot.first_sample + ks[0] * spb;
    i1 = plot.first_sample + (ks[ks.length - 1] + 1) * spb;
  } else {
    const first = Number(ax.first ?? 0), step = Number(ax.step ?? 1) || 1;
    const ia = (a - first) / step, ib = (b - first) / step;
    i0 = Math.min(ia, ib);
    i1 = Math.max(ia, ib);
  }
  i0 = Math.max(0, Math.floor(i0));
  i1 = Math.min(total, Math.ceil(i1) + 1);
  if (i1 - i0 < 8) return null;
  return { first_sample: i0, count: i1 - i0 };
}

/** Bytes from base64. */
function bytesFromBase64(b64) {
  if (!b64) return new Uint8Array(0);
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

/** Decode little-endian float32 values from base64. */
function f32FromBase64(b64) {
  const bytes = bytesFromBase64(b64);
  return new Float32Array(bytes.buffer, 0, bytes.length >> 2);
}

/** Display transform of flow-cytometry values, and its inverse. */
function scaleFn(name) {
  if (name === "log") return (v) => (v > 0 ? Math.log10(v) : NaN);
  if (name === "arcsinh") return (v) => Math.asinh(v / 150);
  return (v) => v;
}
function scaleInv(name) {
  if (name === "log") return (u) => Math.pow(10, u);
  if (name === "arcsinh") return (u) => Math.sinh(u) * 150;
  return (u) => u;
}

/** Ticks for a transformed axis: data values (0, ±10^k) placed by the transform. */
function scaleTicks(name, lo, hi, want) {
  if (name !== "log" && name !== "arcsinh") {
    const t = niceTicks(lo, hi, want);
    const labels = tickLabels(t);
    return t.map((v, i) => ({ at: v, label: labels[i] }));
  }
  const f = scaleFn(name), inv = scaleInv(name);
  const a = inv(lo), b = inv(hi);
  const out = [];
  const push = (v) => {
    const u = f(v);
    if (u >= lo && u <= hi) out.push({ at: u, label: v === 0 ? "0" : (v < 0 ? "−" : "") + "10" + supers(Math.round(Math.log10(Math.abs(v)))) });
  };
  if (name === "arcsinh" && a <= 0 && b >= 0) push(0);
  for (let k = -3; k <= 12; k++) {
    const v = Math.pow(10, k);
    if (name === "arcsinh" && v < 100) continue;
    push(v);
    if (name === "arcsinh") push(-v);
  }
  out.sort((p, q) => p.at - q.at);
  return out;
}
function supers(n) {
  const d = "⁰¹²³⁴⁵⁶⁷⁸⁹";
  return (n < 0 ? "⁻" : "") + String(Math.abs(n)).split("").map((c) => d[Number(c)]).join("");
}

/** The pth percentile (0–100) of finite values. */
function percentile(values, p) {
  const v = Array.from(values).filter((x) => isFinite(x)).sort((a, b) => a - b);
  if (!v.length) return NaN;
  const i = Math.min(v.length - 1, Math.max(0, Math.round((p / 100) * (v.length - 1))));
  return v[i];
}

/** Arguments for openreadout_view from a tool call the host showed us. */
function hintFrom(result, input) {
  const meta = result && result._meta && result._meta["openreadout/view"];
  if (meta && typeof meta === "object") return Object.assign({}, meta);
  const args = (input && input.arguments) || input || {};
  if (typeof args.file === "string") return { file: args.file };
  if (args.file && typeof args.file === "object") return { file: args.file };
  return null;
}

/** Plate well name of a zero-based row and column (`A1`, `AA1` past row Z). */
function wellName(r, c) {
  const row = r < 26 ? String.fromCharCode(65 + r) : "A" + String.fromCharCode(65 + r - 26);
  return row + (c + 1);
}

/** `B7` → [1, 6]; null when it is not a well name. */
function parseWell(s) {
  const m = /^\s*([A-Za-z]{1,2})0*(\d{1,2})\s*$/.exec(String(s || ""));
  if (!m) return null;
  const letters = m[1].toUpperCase();
  const r = letters.length === 1 ? letters.charCodeAt(0) - 65 : 26 + letters.charCodeAt(1) - 65;
  const c = Number(m[2]) - 1;
  return c >= 0 ? [r, c] : null;
}

const num = (v) => (v === null || v === undefined || v === "" ? NaN : Number(v));
const pair = (v) => (Array.isArray(v) && v.length >= 2 ? [num(v[0]), num(v[1])] : null);

/**
 * The agent's annotations in one shape, whatever spelling the tool used. Image annotations are
 * in full-resolution pixels, plot annotations in axis units. Unknown kinds are dropped.
 *   point {x, y} · box {x, y, w, h} · polygon {points} · line / arrow {x0, y0, x1, y1}
 *   text {x, y} · band {x0, x1, y0?, y1?} · hline {y} · vline {x} — each with label?, color?,
 *   and wells (plate annotations).
 */
function normalizeAnnotations(list) {
  if (!Array.isArray(list)) return [];
  const out = [];
  for (const a of list.slice(0, 200)) {
    if (!a || typeof a !== "object") continue;
    const kind = String(a.kind || a.type || a.shape || (a.points ? "polygon" : a.width != null || a.w != null ? "box" : "point")).toLowerCase();
    const base = { label: a.label != null ? String(a.label).slice(0, 120) : a.text != null ? String(a.text).slice(0, 120) : "", color: typeof a.color === "string" ? a.color : null };
    const at = pair(a.at) || pair(a.xy) || pair(a.position) || [num(a.x), num(a.y)];
    const wells = a.wells || a.well;
    if (wells) {
      const ws = (Array.isArray(wells) ? wells : [wells]).map(parseWell).filter(Boolean);
      if (ws.length) { out.push(Object.assign(base, { kind: "wells", wells: ws })); continue; }
    }
    if (kind === "point" || kind === "dot" || kind === "marker") {
      if (isFinite(at[0]) && isFinite(at[1])) out.push(Object.assign(base, { kind: "point", x: at[0], y: at[1] }));
    } else if (kind === "text" || kind === "label") {
      if (isFinite(at[0]) && isFinite(at[1])) out.push(Object.assign(base, { kind: "text", x: at[0], y: at[1] }));
    } else if (kind === "box" || kind === "rect" || kind === "rectangle" || kind === "region") {
      const r = a.region || a.rect || a.box;
      let x, y, w, h;
      if (Array.isArray(r) && r.length >= 4) [x, y, w, h] = r.map(num);
      else if (r && typeof r === "object") [x, y, w, h] = [num(r.x), num(r.y), num(r.width ?? r.w), num(r.height ?? r.h)];
      else if (a.x0 != null) [x, y, w, h] = [num(a.x0), num(a.y0), num(a.x1) - num(a.x0), num(a.y1) - num(a.y0)];
      else [x, y, w, h] = [num(a.x), num(a.y), num(a.width ?? a.w), num(a.height ?? a.h)];
      if ([x, y, w, h].every(isFinite)) out.push(Object.assign(base, { kind: "box", x: Math.min(x, x + w), y: Math.min(y, y + h), w: Math.abs(w), h: Math.abs(h) }));
    } else if (kind === "polygon" || kind === "polyline" || kind === "path") {
      const pts = (a.points || a.vertices || []).map((p) => (Array.isArray(p) ? [num(p[0]), num(p[1])] : [num(p.x), num(p.y)])).filter((p) => p.every(isFinite));
      if (pts.length >= 2) out.push(Object.assign(base, { kind: "polygon", points: pts, closed: kind === "polygon" }));
    } else if (kind === "line" || kind === "arrow" || kind === "segment") {
      const p0 = pair(a.from) || (a.points && pair(a.points[0])) || [num(a.x0 ?? a.x1), num(a.y0 ?? a.y1)];
      const p1 = pair(a.to) || (a.points && pair(a.points[1])) || [num(a.x1 != null && a.x0 != null ? a.x1 : a.x2), num(a.y1 != null && a.y0 != null ? a.y1 : a.y2)];
      if (p0.concat(p1).every(isFinite)) out.push(Object.assign(base, { kind: "line", x0: p0[0], y0: p0[1], x1: p1[0], y1: p1[1], arrow: kind === "arrow" || !!a.arrow }));
    } else if (kind === "band" || kind === "span" || kind === "xband" || kind === "range") {
      const xr = pair(a.x_range) || pair(a.range) || pair(a.x) || [num(a.from ?? a.x0 ?? a.start), num(a.to ?? a.x1 ?? a.end)];
      const yr = pair(a.y_range);
      if (xr.every(isFinite)) out.push(Object.assign(base, { kind: "band", x0: Math.min(...xr), x1: Math.max(...xr), y0: yr ? Math.min(...yr) : null, y1: yr ? Math.max(...yr) : null }));
    } else if (kind === "vline" || kind === "xline") {
      const x = num(a.x ?? a.at ?? a.value);
      if (isFinite(x)) out.push(Object.assign(base, { kind: "vline", x }));
    } else if (kind === "hline" || kind === "yline" || kind === "threshold") {
      const y = num(a.y ?? a.at ?? a.value);
      if (isFinite(y)) out.push(Object.assign(base, { kind: "hline", y }));
    }
  }
  return out;
}

/**
 * Place label boxes near their anchors without overlapping each other or `avoid`, inside
 * `bounds` {x, y, w, h}. Items: {ax, ay, w, h}. Returns [{x, y}] (top-left), one per item,
 * in the same order. Greedy: the first free spot among positions around the anchor.
 */
function placeLabels(items, bounds, avoid) {
  const taken = (avoid || []).slice();
  const hit = (r) => taken.some((t) => r.x < t.x + t.w && r.x + r.w > t.x && r.y < t.y + t.h && r.y + r.h > t.y);
  const inside = (r) => r.x >= bounds.x && r.y >= bounds.y && r.x + r.w <= bounds.x + bounds.w && r.y + r.h <= bounds.y + bounds.h;
  const clamp = (r) => ({
    x: Math.min(Math.max(r.x, bounds.x), bounds.x + bounds.w - r.w),
    y: Math.min(Math.max(r.y, bounds.y), bounds.y + bounds.h - r.h),
    w: r.w, h: r.h,
  });
  return items.map((it) => {
    const g = it.gap ?? 8;
    const spots = [];
    for (const d of [1, 2, 3.5]) {
      const dx = g * d, dy = g * d;
      spots.push(
        [it.ax + dx, it.ay - it.h / 2], [it.ax - dx - it.w, it.ay - it.h / 2],
        [it.ax - it.w / 2, it.ay - dy - it.h], [it.ax - it.w / 2, it.ay + dy],
        [it.ax + dx, it.ay - dy - it.h], [it.ax + dx, it.ay + dy],
        [it.ax - dx - it.w, it.ay - dy - it.h], [it.ax - dx - it.w, it.ay + dy],
      );
    }
    let best = null;
    for (const [x, y] of spots) {
      const r = { x, y, w: it.w, h: it.h };
      if (inside(r) && !hit(r)) { best = r; break; }
    }
    if (!best) best = clamp({ x: spots[0][0], y: spots[0][1], w: it.w, h: it.h });
    taken.push(best);
    return { x: best.x, y: best.y };
  });
}

/** Ease in and out, t in [0, 1]. */
function ease(t) {
  t = Math.min(1, Math.max(0, t));
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

/**
 * Camera between two rectangles at time t: the zoom follows a geometric path (equal zoom
 * steps per frame, as a person would zoom), the centre a linear one.
 */
function lerpRect(a, b, t) {
  const u = ease(t);
  const w = a.w * Math.pow(b.w / a.w, u);
  const h = a.h * Math.pow(b.h / a.h, u);
  const cx = a.x + a.w / 2 + (b.x + b.w / 2 - a.x - a.w / 2) * u;
  const cy = a.y + a.h / 2 + (b.y + b.h / 2 - a.y - a.h / 2) * u;
  return { x: cx - w / 2, y: cy - h / 2, w, h };
}

/** `r` grown to `aspect` (width / height) around its centre. */
function fitAspect(r, aspect) {
  let { w, h } = r;
  if (w / h > aspect) h = w / aspect;
  else w = h * aspect;
  return { x: r.x + r.w / 2 - w / 2, y: r.y + r.h / 2 - h / 2, w, h };
}

/** The layout map a plate is coloured by, as the previews choose it: 2 to 8 groups (blanks
 * aside) covering the most wells, a map named like a group winning ties; else none. */
function defaultGroupMap(groups) {
  let best = null, bestScore = null;
  for (const [name, m] of Object.entries(groups || {})) {
    const values = Object.values(m).map(String);
    const n = new Set(values.filter((v) => !isBlankGroup(v))).size;
    if (n < 2 || n > 8) continue;
    const score = [values.length, /group/i.test(name) ? 1 : 0];
    if (!bestScore || score[0] > bestScore[0] || (score[0] === bestScore[0] && score[1] > bestScore[1])) {
      best = name;
      bestScore = score;
    }
  }
  return best;
}

/** Blank wells recede: the figure engine's blank labels (`Blank`, `BL`, `buffer`, `empty`, …). */
function isBlankGroup(name) {
  return THEMES.blanks.includes(String(name == null ? "" : name).trim().toLowerCase());
}

/** The shared category rule (openreadout-figure `category_colors`): categories take the series
 * colours in the order they first appear, blanks are faint and use up no colour. */
function categoryColors(names, palette) {
  const seen = [];
  return names.map((n) => {
    if (isBlankGroup(n)) return palette.faint;
    let i = seen.indexOf(n);
    if (i < 0) { seen.push(n); i = seen.length - 1; }
    return palette.series[i % palette.series.length];
  });
}

/** qPCR channel names `B3 GAPDH` → the target after the well, for colouring by target. */
function seriesGroup(name) {
  const m = /^\s*[A-Pa-p]\d{1,2}[\s:_-]+(.+)$/.exec(String(name || ""));
  return m ? m[1].trim() : null;
}

/** Wells as the plate tools take them: `A1:B3` for a full rectangle, else `A1,B2,C7`. */
function wellSpec(wells) {
  if (!wells.length) return "";
  const rs = wells.map((w) => w[0]), cs = wells.map((w) => w[1]);
  const r0 = Math.min(...rs), r1 = Math.max(...rs), c0 = Math.min(...cs), c1 = Math.max(...cs);
  const uniq = new Set(wells.map(([r, c]) => r * 1000 + c));
  if (wells.length > 1 && uniq.size === (r1 - r0 + 1) * (c1 - c0 + 1)) return `${wellName(r0, c0)}:${wellName(r1, c1)}`;
  return [...uniq].sort((a, b) => a - b).map((k) => wellName(Math.floor(k / 1000), k % 1000)).join(",");
}

/** Arguments of an MCP tool call, as the model would write it. */
function callText(tool, args) {
  return `${tool} ${JSON.stringify(args)}`;
}

// ---------------------------------------------------------------------------------------------
// Host connection (MCP Apps JSON-RPC over postMessage)

function connect(onNotification, onRequest) {
  let nextId = 1;
  const pending = new Map();
  const send = (msg) => window.parent.postMessage(msg, "*");
  window.addEventListener("message", (event) => {
    if (event.source !== window.parent) return;
    const m = event.data;
    if (!m || m.jsonrpc !== "2.0") return;
    if (m.id !== undefined && m.method === undefined) {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      if (m.error) p.reject(Object.assign(new Error(m.error.message || "request failed"), { data: m.error.data }));
      else p.resolve(m.result);
    } else if (m.method && m.id !== undefined) {
      Promise.resolve()
        .then(() => onRequest(m.method, m.params || {}))
        .then((result) => send({ jsonrpc: "2.0", id: m.id, result: result || {} }))
        .catch((e) => send({ jsonrpc: "2.0", id: m.id, error: { code: -32603, message: String(e && e.message || e) } }));
    } else if (m.method) {
      onNotification(m.method, m.params || {});
    }
  });
  return {
    request(method, params, timeoutMs) {
      const id = nextId++;
      send({ jsonrpc: "2.0", id, method, params });
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(method + " timed out"));
        }, timeoutMs || 180000);
        pending.set(id, {
          resolve: (v) => { clearTimeout(timer); resolve(v); },
          reject: (e) => { clearTimeout(timer); reject(e); },
        });
      });
    },
    notify(method, params) {
      send({ jsonrpc: "2.0", method, params: params || {} });
    },
  };
}

// ---------------------------------------------------------------------------------------------
// The app

function main() {
  const $ = (id) => document.getElementById(id);
  const root = document.documentElement;
  const canvas = $("canvas");
  const ctx = canvas.getContext("2d");
  // Everything but the pointer's overlays (hover, selection, popovers) is drawn once into
  // `base`; moving the pointer only copies it back and draws the overlays.
  const base = document.createElement("canvas");
  const bctx = base.getContext("2d");
  let g = bctx; // the context drawing functions use
  const S = {
    host: null,
    hostCaps: {},
    hostContext: {},
    toolInput: null,
    args: null,          // current openreadout_view arguments
    data: null,          // current openreadout_view structuredContent
    picture: null,       // { img } for static pictures
    outline: null,
    file: null,
    seq: 0,
    layout: null,        // geometry of the last drawing, for hit tests
    drag: null,
    hover: null,         // what the pointer is over
    sel: null,           // what the user selected
    ann: null,           // the agent's annotations: { list, view, image }
    fcsScale: {},
    img: null,           // deep-zoom state of an image view
    kin: { mode: "curves", t: -1, colorBy: undefined, playing: false, focus: null },
    fig: null,           // a figure result: { img, data, zoom }
    size: [0, 0],
  };
  window.__animating = false;

  // ---------- host plumbing

  const host = connect(onNotification, async (method) => {
    if (method === "ui/resource-teardown") return {};
    if (method === "ping") return {};
    throw new Error("unsupported: " + method);
  });
  S.host = host;

  function applyContext(c) {
    if (!c) return;
    const key = () => JSON.stringify([S.hostContext.theme, S.hostContext.displayMode, S.hostContext.containerDimensions, S.hostContext.styles]);
    const before = key();
    Object.assign(S.hostContext, c);
    if (before === key() && S.data) return;
    if (c.theme) {
      root.setAttribute("data-theme", c.theme);
      root.style.colorScheme = c.theme;
    }
    const vars = c.styles && c.styles.variables;
    if (vars) for (const [k, v] of Object.entries(vars)) if (v) root.style.setProperty(k, v);
    const fonts = c.styles && c.styles.css && c.styles.css.fonts;
    if (fonts && !document.getElementById("host-fonts")) {
      const st = document.createElement("style");
      st.id = "host-fonts";
      st.textContent = fonts;
      document.head.appendChild(st);
    }
    const dims = S.hostContext.containerDimensions || {};
    const full = S.hostContext.displayMode === "fullscreen" || typeof dims.height === "number";
    root.classList.toggle("fill", full);
    const modes = S.hostContext.availableDisplayModes || [];
    $("fullscreen").hidden = !modes.includes("fullscreen");
    if (c.safeAreaInsets) {
      const i = c.safeAreaInsets;
      document.body.style.padding = `${i.top}px ${i.right}px ${i.bottom}px ${i.left}px`;
    }
    palette = null;
    invalidate();
    if (S.fig) redrawFigure(S.fig.xRange);
  }

  function onNotification(method, p) {
    switch (method) {
      case "ui/notifications/tool-input":
        S.toolInput = p.arguments || {};
        if (!S.data) {
          const f = S.toolInput.file;
          setTitle(typeof f === "string" ? f.split(/[\\/]/).pop() : f && f.name, "");
          overlay("Reading the file…");
        }
        break;
      case "ui/notifications/tool-result":
        onToolResult(p);
        break;
      case "ui/notifications/tool-cancelled":
        if (!S.data) overlay(p.reason ? `The tool call failed: ${p.reason}` : "The tool call was cancelled.", true);
        break;
      case "ui/notifications/host-context-changed":
        applyContext(p);
        break;
      default:
        break;
    }
  }

  const canCallTools = () => !!(S.hostCaps && S.hostCaps.serverTools);
  const canMessage = () => !!(S.hostCaps && S.hostCaps.message);
  const canContext = () => !!(S.hostCaps && S.hostCaps.updateModelContext);

  async function onToolResult(result) {
    const sc = result && result.structuredContent;
    if (result && result.isError) {
      overlay(textOf(result) || "The tool call failed.", true);
      return;
    }
    // The host opened a file with the viewer (file entrypoint): the result is already ours.
    if (sc && sc.view && sc.file && sc.view !== "pending" && sc.view !== "figure" && (sc.plot || sc.image || sc.plate || sc.events || sc.outline)) {
      S.args = hintFrom(null, S.toolInput) || {};
      show(result);
      return;
    }
    // openreadout_plot: show the figure it drew; _meta["openreadout/figure"] has its panels
    if (result && result._meta && result._meta["openreadout/figure"]) {
      showFigure(result);
      return;
    }
    const hint = sc && sc.view === "pending" ? hintFrom(null, S.toolInput) : hintFrom(result, S.toolInput);
    // A finished figure (openreadout_plot): its picture and data are in the result itself.
    if ((hint && hint.view === "figure") || (!hint && sc && sc.figure)) {
      showFigure(result);
      return;
    }
    if (!hint) {
      overlay(textOf(result) || "Nothing to show for this result.");
      return;
    }
    if (!canCallTools()) {
      showStatic(result);
      return;
    }
    const ann = normalizeAnnotations(hint.annotations || (sc && sc.annotations));
    delete hint.annotations;
    S.ann = ann.length ? { list: ann, image: hint.image ?? 0 } : null;
    // A later call in the same frame (hosts that keep one viewer): follow it with a transition.
    const sameImage = S.data && S.data.view === "image" && S.args && S.args.file === hint.file && (hint.view === undefined || hint.view === "image") && (hint.image ?? 0) === (S.args.image ?? 0);
    S.sel = null;
    hideAct();
    S.args = Object.assign({ outline: !S.outline }, hint);
    if (sameImage && S.img) {
      S.img.follow = true;
    } else if (S.img) {
      S.img = null;
    }
    await load();
  }

  function textOf(result) {
    return (result.content || []).filter((c) => c.type === "text").map((c) => c.text).join("\n").slice(0, 2000);
  }

  /** Hosts that cannot proxy tool calls: show the picture the tool returned, if any. */
  function showStatic(result) {
    const im = (result.content || []).find((c) => c.type === "image");
    if (!im) {
      overlay("This host does not let the viewer read the file. The tool's answer is in the chat.");
      return;
    }
    decode(im).then((img) => {
      S.data = { view: "static" };
      S.picture = { img };
      overlay(null);
      invalidate();
    });
  }

  function decode(im) {
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => resolve(img);
      img.onerror = () => reject(new Error("Could not decode the picture."));
      img.src = `data:${im.mimeType};base64,${im.data}`;
    });
  }

  async function callView(args) {
    const r = await host.request("tools/call", { name: VIEW_TOOL, arguments: args });
    if (r && r.isError) throw Object.assign(new Error(textOf(r) || "openreadout_view failed"), { data: r.structuredContent });
    return r;
  }

  async function load(patch, opts) {
    if (patch) {
      if (!opts || !opts.keepZoom) for (const k of ZOOM_KEYS) delete S.args[k];
      for (const [k, v] of Object.entries(patch)) {
        if (v === undefined || v === null) delete S.args[k];
        else S.args[k] = v;
      }
    }
    const seq = ++S.seq;
    const args = Object.assign({}, S.args, { outline: !S.outline });
    // Pictures: as wide as the canvas in device pixels (the server caps it).
    args.max_size = pictureSize();
    overlay("Loading…");
    root.setAttribute("aria-busy", "true");
    try {
      let r = await callView(args);
      if (r.structuredContent && r.structuredContent.view === "pending") r = await callView(args);
      if (seq !== S.seq) return;
      if (r.structuredContent && r.structuredContent.view === "pending") {
        overlay("The host did not give the viewer this file's path. Ask the agent to open it instead.", true);
        return;
      }
      await show(r);
    } catch (e) {
      if (seq !== S.seq) return;
      const hint = e && e.data && e.data.hint;
      overlay((e && e.message ? e.message : String(e)) + (hint ? "\n" + hint : ""), true);
    } finally {
      if (seq === S.seq) root.setAttribute("aria-busy", "false");
    }
  }

  function pictureSize() {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(256, Math.min(1600, Math.max(canvas.clientWidth, 320) * dpr));
    return Math.ceil(w / 64) * 64;
  }

  async function show(result) {
    const d = result.structuredContent || {};
    if (d.outline) S.outline = d.outline;
    if (d.file) S.file = d.file;
    const viewChanged = !S.data || S.data.view !== d.view;
    S.data = d;
    S.args = S.args || {};
    S.hover = null;
    tip(null);
    $("readout").textContent = "";
    if (d.file && !S.args.file) S.args.file = d.file.path;
    S.args.view = d.view;
    if (viewChanged && S.ann && S.ann.view && S.ann.view !== d.view) S.ann = null;
    if (S.ann && !S.ann.view) S.ann.view = d.view;
    setTitle(S.file && S.file.name, S.file ? S.file.format_name : "");
    renderTabs();
    const notes = (d.notes || []).slice(0, 3);
    $("notes").textContent = notes.join(" · ");
    $("notes").title = (d.notes || []).join("\n");
    const im = (result.content || []).find((c) => c.type === "image");
    if (d.view === "image" && im) {
      const img = await decode(im).catch(() => null);
      if (!img) { overlay("Could not decode the picture.", true); return; }
      await showImage(d, img);
    } else {
      S.img = null;
      S.picture = null;
      if (d.plate) setupKinetic(d.plate);
      overlay(null);
      invalidate();
    }
    renderControls();
    setHint();
    tellModel();
  }

  function setTitle(name, format) {
    if (name) $("file-name").textContent = name;
    $("file-format").textContent = format || "";
  }

  function overlay(text, isError) {
    const o = $("overlay");
    if (!text) {
      o.hidden = true;
      return;
    }
    o.hidden = false;
    o.textContent = text;
    o.classList.toggle("error", !!isError);
    // While something is already shown, a reload only gets a small badge.
    o.classList.toggle("busy", !isError && !!S.data && S.data.view !== "pending");
  }

  let toastTimer = 0;
  function toast(text) {
    const t = $("toast");
    t.textContent = text;
    t.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (t.hidden = true), 2200);
  }

  function setHint() {
    const d = S.data;
    let h = "";
    if (!d) h = "";
    else if (d.view === "image") h = "Drag to select · Shift-drag to pan · double-click to zoom";
    else if (d.view === "figure") h = "Drag to zoom · double-click to reset";
    else if (d.plot) h = d.plot.kind === "chromatogram" && d.plot.source === "spectra" ? "Drag to select a range · click for the spectrum" : d.plot.kind === "spectrum" ? "Click a peak · drag to select" : "Drag to select a range · double-click to reset";
    else if (d.plate) h = d.plate.kinetic ? "Click wells to select · hover for the curve" : "Click wells to select";
    else if (d.events) h = d.events.gating ? "Click a population to open it · drag to select" : "Drag to select events";
    $("hint").textContent = h;
  }

  // ---------- telling the model

  // What the user is looking at and what they selected, so "what is this?" has an answer and
  // the agent's next tool call can use the numbers as they are.
  let tellTimer = 0;
  function tellModel() {
    clearTimeout(tellTimer);
    tellTimer = setTimeout(() => {
      if (!canContext()) return;
      const text = describe() + (S.sel ? "\n" + selectionText(S.sel).model : "");
      if (!text.trim()) return;
      const structured = { viewer: { file: S.file && S.file.path, view: S.data && S.data.view, args: viewArgs(), selection: S.sel ? selectionText(S.sel).data : null } };
      host.request("ui/update-model-context", { content: [{ type: "text", text }], structuredContent: structured }, 10000).catch(() => {});
    }, 400);
  }

  function viewArgs() {
    const a = Object.assign({}, S.args || {});
    delete a.outline;
    delete a.max_size;
    return a;
  }

  function describe() {
    const d = S.data;
    if (!d || !S.file) return "";
    const a = S.args || {};
    let what = d.view;
    if (d.view === "image" && d.image) {
      const im = d.image;
      const r = S.img && S.img.settled ? regionOf(S.img.cam) : im.full_res_region;
      const info = imageInfo();
      what = `image ${im.image}` +
        ((info.size_c || 1) > 1 && !im.rgb ? (im.composite ? `, composite of channels ${im.c.join(",")}` : `, channel ${im.c.join(",")}`) : "") +
        ((info.size_z || 1) > 1 ? (im.projection ? ", max projection over z" : `, z ${im.z.join(",")}`) : "") +
        ((info.size_t || 1) > 1 ? `, t ${im.t.join(",")}` : "") +
        (r ? `, showing x ${fmtInt(r.x)}–${fmtInt(r.x + r.width)}, y ${fmtInt(r.y)}–${fmtInt(r.y + r.height)} (full-resolution px)` : "");
    } else if (d.plot && d.plot.kind === "trace") {
      what = `trace ${d.plot.trace}, sweep ${d.plot.sweep}, samples ${d.plot.first_sample}–${d.plot.first_sample + d.plot.count}`;
    } else if (d.plot && d.plot.kind === "chromatogram") {
      what = `chromatogram (${d.plot.series.map((s) => s.name).join(", ")})` + (a.x_range ? `, ${fmt(a.x_range[0])}–${fmt(a.x_range[1])} min` : "");
    } else if (d.plot && d.plot.kind === "spectrum") {
      const s = d.plot.spectrum;
      what = `mass spectrum, scan ${s.scan_number} (index ${s.index}, MS${s.ms_level}, ${fmt(s.rt_min)} min)`;
    } else if (d.plot && d.plot.kind === "nmr") {
      what = `NMR spectrum of trace ${d.plot.trace}`;
    } else if (d.events) {
      const gt = d.events.gating;
      what = `flow-cytometry events, ${d.events.x.name}` + (d.events.y ? ` vs ${d.events.y.name}` : " histogram") + (gt ? `, population ${gt.shown.path} of ${gt.file}` : "");
    } else if (d.plate) {
      const k = d.plate.kinetic;
      what = k ? `kinetic plate, ${k.wells.length} wells, ${S.kin.mode === "heat" ? "heat map at " + fmtTime(kinTime()) : "growth curves"}` : `plate heat map of ${d.plate.value}`;
    }
    return `The user is looking at ${S.file.path} in the OpenReadout viewer: ${what}.`;
  }

  /** The selection in words, for the user (`short`), for the model (`model`) and as data. */
  function selectionText(sel) {
    const file = S.file ? S.file.path : S.args.file;
    const d = S.data || {};
    const calls = [];
    let short = "", what = "";
    const data = Object.assign({}, sel);
    if (sel.kind === "region") {
      const r = sel.region;
      const im = d.image || {};
      const info = imageInfo();
      const plane = [];
      if ((info.size_c || 1) > 1 && !im.rgb) {
        if (im.composite) plane.push(`c=${(im.c || []).join(",")}`);
        else if (im.c && im.c.length) plane.push(`c=${im.c[0]}`);
      }
      if ((info.size_z || 1) > 1 && im.z && im.z.length === 1 && !im.projection) plane.push(`z=${im.z[0]}`);
      if ((info.size_t || 1) > 1 && im.t && im.t.length === 1) plane.push(`t=${im.t[0]}`);
      const args = { file, image: im.image || 0, region: r };
      if (plane.length) args.select = [plane.join(",")];
      const ps = physical();
      const size = ps ? ` (${fmtLength(r.width * ps.x, 3)} × ${fmtLength(r.height * ps.y, 3)})` : "";
      short = `x ${fmtInt(r.x)}–${fmtInt(r.x + r.width)}, y ${fmtInt(r.y)}–${fmtInt(r.y + r.height)} px${size}`;
      what = `a region of image ${args.image}: {x: ${r.x}, y: ${r.y}, width: ${r.width}, height: ${r.height}} in full-resolution pixels${size}`;
      calls.push(callText("openreadout_preview", args), callText("openreadout_stats", args));
    } else if (sel.kind === "point") {
      short = `x ${fmtInt(sel.x)}, y ${fmtInt(sel.y)} px`;
      what = `the point x ${Math.round(sel.x)}, y ${Math.round(sel.y)} (full-resolution pixels) of image ${(d.image || {}).image || 0}`;
      const half = 256;
      calls.push(callText("openreadout_preview", { file, image: (d.image || {}).image || 0, region: { x: Math.max(0, Math.round(sel.x - half)), y: Math.max(0, Math.round(sel.y - half)), width: 2 * half, height: 2 * half } }));
    } else if (sel.kind === "annotation") {
      short = `“${sel.label}”`;
      what = `the agent's annotation ${short} at ${JSON.stringify(sel.at)}`;
    } else if (sel.kind === "xrange") {
      const p = d.plot || {};
      const u = (p.x && p.x.unit) || "";
      short = `${fmt(sel.x0, 5)}–${fmt(sel.x1, 5)} ${u}`.trim();
      what = `the ${(p.x && p.x.quantity) || "x"} range ${short}`;
      if (p.kind === "chromatogram") {
        const args = { file, rt_range: [round(sel.x0), round(sel.x1)] };
        if (S.args.mz && S.args.mz.length) args.mz = S.args.mz;
        if (p.trace != null && p.source === "trace") args.traces = [p.trace];
        calls.push(callText("openreadout_peaks", args), callText("openreadout_chromatogram", args));
      } else if (p.kind === "spectrum") {
        calls.push(callText("openreadout_spectrum", { file, run: p.run, spectrum: p.spectrum.index }) + ` (m/z ${fmt(sel.x0, 7)}–${fmt(sel.x1, 7)})`);
      } else {
        calls.push(callText("openreadout_trace", { file, trace: p.trace, sweep: p.sweep, x_range: [round(sel.x0), round(sel.x1)] }));
      }
    } else if (sel.kind === "peak") {
      const pk = sel.peak;
      short = `peak ${pk.label ?? ""} at ${fmt(pk.x, 5)} min`.replace("  ", " ");
      what = `chromatogram peak ${pk.label ?? ""}: apex ${fmt(pk.x, 6)} min` + (pk.from != null ? `, ${fmt(pk.from, 5)}–${fmt(pk.to, 5)} min` : "") + (pk.area != null ? `, area ${fmt(pk.area, 5)}` : "") + (pk.height != null ? `, height ${fmt(pk.height, 5)}` : "");
      calls.push(callText("openreadout_peaks", { file, rt_range: [round(pk.from ?? pk.x - 0.1), round(pk.to ?? pk.x + 0.1)] }));
      if ((d.plot || {}).source === "spectra") calls.push(`openreadout_view shows the spectrum at ${fmt(pk.x, 6)} min`);
    } else if (sel.kind === "mz") {
      short = `m/z ${fmt(sel.mz, 8)}`;
      const p = d.plot || {};
      what = `the peak at m/z ${fmt(sel.mz, 9)} (intensity ${fmt(sel.y, 4)}) of scan ${p.spectrum && p.spectrum.scan_number}`;
      calls.push(callText("openreadout_chromatogram", { file, mz: [Number(sel.mz.toFixed(5))], ppm: 10, run: p.run }));
    } else if (sel.kind === "xpoint") {
      short = `${fmt(sel.x, 5)} min`;
      what = `retention time ${fmt(sel.x, 6)} min`;
    } else if (sel.kind === "wells") {
      const names = sel.wells.map(([r, c]) => wellName(r, c));
      const pl = d.plate || {};
      const map = S.kin.colorBy && pl.groups ? pl.groups[S.kin.colorBy] : null;
      const groups = map ? [...new Set(names.map((n) => map[n]).filter(Boolean))] : [];
      short = (names.length > 6 ? `${names.length} wells (${names.slice(0, 4).join(", ")}…)` : names.join(", ")) + (groups.length && groups.length <= 2 ? ` · ${groups.join(", ")}` : "");
      what = `well${names.length > 1 ? "s" : ""} ${names.join(", ")}` + (groups.length ? ` (${S.kin.colorBy}: ${groups.join(", ")})` : "") + ` of table ${pl.table}`;
      const vals = names.map((n, i) => {
        const v = wellValue(sel.wells[i]);
        return v == null ? null : `${n} ${fmt(v, 4)}`;
      }).filter(Boolean);
      if (vals.length && vals.length <= 24) what += `; ${pl.kinetic ? "at " + fmtTime(kinTime()) : "values"}: ${vals.join(", ")}`;
      data.wells = names;
      const spec = wellSpec(sel.wells);
      if (pl.kinetic) calls.push(callText("openreadout_assay", { file, analysis: "growth", wells: spec }), callText("openreadout_assay", { file, analysis: "kinetics", wells: spec }));
      else calls.push(callText("openreadout_table", { file, table: pl.table }));
    } else if (sel.kind === "gate") {
      const ev = d.events;
      const conds = [`${ev.x.name} >= ${fmt(sel.x0, 6)}`, `${ev.x.name} < ${fmt(sel.x1, 6)}`];
      if (ev.y && sel.y0 != null) conds.push(`${ev.y.name} >= ${fmt(sel.y0, 6)}`, `${ev.y.name} < ${fmt(sel.y1, 6)}`);
      short = `${sel.count} of ${sel.of} events shown (${fmt(100 * sel.count / Math.max(1, sel.of), 3)} %)`;
      what = `a rectangle of events: ${conds.join(", ")}; about ${fmt(100 * sel.count / Math.max(1, sel.of), 3)} % of the events shown`;
      const args = { file, filter: conds, count: true };
      if (ev.gating) {
        args.workspace = S.args.workspace || undefined;
        args.gatingml = S.args.gatingml || undefined;
        args.compensate = ev.compensated ? "auto" : undefined;
        if (ev.gating.shown.path !== "/") { args.populations = [ev.gating.shown.path]; what += ` of population ${ev.gating.shown.path} (add "gate:${ev.gating.shown.path} == 1" to the filter)`; }
      }
      calls.push(callText("openreadout_table", JSON.parse(JSON.stringify(args))));
      data.filter = conds;
    } else if (sel.kind === "population") {
      const p = sel.population;
      short = `${p.name} · ${fmtInt(p.count)} events`;
      what = `population ${p.path} (${fmtInt(p.count)} events, ${fmt(p.percent_of_parent, 3)} % of its parent)`;
      calls.push(callText("openreadout_gate", { file, workspace: S.args.workspace, gatingml: S.args.gatingml, populations: [p.path] }));
    } else if (sel.kind === "figure") {
      short = sel.text;
      what = `the figure at ${sel.text}`;
    }
    const model = `The user selected ${what}.` + (calls.length ? ` To look closer: ${calls.join(" ; ")}` : "");
    return { short, model, data };
  }

  const round = (x) => Number(Number(x).toPrecision(7));

  // ---------- user → agent: the selection bar

  function showAct(sel, anchor) {
    S.sel = sel;
    tip(null);
    const act = $("act");
    const t = selectionText(sel);
    $("act-label").textContent = t.short;
    $("act-label").title = t.model;
    const box = $("act-buttons");
    box.textContent = "";
    for (const b of actionsFor(sel)) {
      const btn = el("button", { type: "button", textContent: b.label, className: b.primary ? "primary" : "", on: { click: b.run } });
      box.append(btn);
    }
    const ask = $("act-ask");
    ask.hidden = !canMessage();
    if (!canMessage() && !canContext()) {
      box.append(el("button", { type: "button", textContent: "Copy", on: { click: () => copy(t.model) } }));
    }
    $("act-input").value = "";
    $("act-input").placeholder = askPlaceholder(sel);
    act.hidden = false;
    positionAct(anchor);
    invalidateOverlay();
    tellModel();
  }

  function positionAct(anchor) {
    const act = $("act");
    if (anchor) S.actAnchor = anchor;
    else if (S.sel && S.sel.kind === "wells") anchor = actAnchorForWells(S.sel.wells);
    else anchor = S.actAnchor;
    const stage = $("stage").getBoundingClientRect();
    const w = act.offsetWidth, h = act.offsetHeight;
    const a = anchor || { x: 8, y: 8, w: 0, h: 0 };
    let x = Math.min(stage.width - w - 8, Math.max(8, a.x + a.w / 2 - w / 2));
    let y = a.y + a.h + 8;
    if (y + h > stage.height - 4) y = a.y - h - 8;
    if (y < 4) y = Math.max(4, Math.min(stage.height - h - 4, a.y + 8));
    act.style.left = x + "px";
    act.style.top = y + "px";
  }

  function hideAct() {
    $("act").hidden = true;
  }

  function clearSelection() {
    if (!S.sel) return;
    S.sel = null;
    hideAct();
    invalidateOverlay();
    tellModel();
  }

  function askPlaceholder(sel) {
    const k = sel.kind;
    if (k === "region" || k === "point" || k === "annotation") return "Ask about this region… (Enter: “What is this?”)";
    if (k === "wells") return "Ask about these wells…";
    if (k === "gate" || k === "population") return "Ask about these events…";
    if (k === "peak" || k === "mz") return "Ask about this peak…";
    return "Ask about this…";
  }

  function defaultQuestion(sel) {
    const k = sel.kind;
    if (k === "region" || k === "point" || k === "annotation") return "What is in this region?";
    if (k === "wells") return sel.wells.length > 1 ? "What happened in these wells?" : "What happened in this well?";
    if (k === "gate") return "What are these events?";
    if (k === "population") return "Tell me about this population.";
    if (k === "peak" || k === "mz") return "What is this peak?";
    return "What happens here?";
  }

  function actionsFor(sel) {
    const d = S.data || {};
    const out = [];
    if (sel.kind === "region") out.push({ label: "Zoom in", primary: !canMessage(), run: () => { clearSelection(); zoomTo(rectOfRegion(sel.region)); } });
    if (sel.kind === "xrange") out.push({ label: "Zoom", run: () => { clearSelection(); zoomPlot(sel.x0, sel.x1); } });
    if (sel.kind === "xpoint" || (sel.kind === "peak" && d.plot && d.plot.source === "spectra")) {
      const x = sel.kind === "peak" ? sel.peak.x : sel.x;
      out.push({ label: "Spectrum here", run: () => { clearSelection(); S.args = { file: S.args.file, view: "spectrum", rt_min: x, run: d.plot.run ?? S.args.run }; load(); } });
    }
    if (sel.kind === "peak") out.push({ label: "Zoom", run: () => { clearSelection(); const pk = sel.peak; const w = Math.max((pk.to ?? pk.x) - (pk.from ?? pk.x), 0.05); zoomPlot((pk.from ?? pk.x) - w, (pk.to ?? pk.x) + w); } });
    if (sel.kind === "mz") out.push({ label: "Chromatogram of this m/z", run: () => { clearSelection(); S.args = { file: S.args.file, view: "chromatogram", run: d.plot.run, mz: [Number(sel.mz.toFixed(5))] }; load(); } });
    if (sel.kind === "population" && sel.population.path !== (d.events.gating.shown || {}).path) out.push({ label: "Open", run: () => { clearSelection(); load({ population: sel.population.path, x: undefined, y: undefined }); } });
    if (sel.kind === "figure") out.push({ label: "Zoom", run: () => { const z = sel.zoom; clearSelection(); figureZoom(z); } });
    return out;
  }

  async function ask(question) {
    const sel = S.sel;
    if (!sel) return;
    const t = selectionText(sel);
    const q = (question || "").trim() || defaultQuestion(sel);
    const name = S.file ? S.file.name : "";
    // The message is what the user sees in the chat; the exact numbers go with it, and the
    // model context (sent first) carries the full description and tool arguments.
    const text = `${q}\n\n(Selected in the OpenReadout viewer: ${name}${name ? ", " : ""}${t.short}.${canContext() ? "" : " " + t.model})`;
    clearTimeout(tellTimer);
    if (canContext()) {
      await host.request("ui/update-model-context", { content: [{ type: "text", text: describe() + "\n" + t.model }], structuredContent: { viewer: { file: S.file && S.file.path, view: S.data && S.data.view, args: viewArgs(), selection: t.data } } }, 10000).catch(() => {});
    }
    try {
      const r = await host.request("ui/message", { role: "user", content: [{ type: "text", text }] }, 15000);
      if (r && r.isError) throw new Error("declined");
      toast("Sent to the chat");
      $("act-input").value = "";
    } catch (e) {
      toast("The host did not take the message");
    }
  }

  function copy(text) {
    const done = () => toast("Copied");
    if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(done, () => fallbackCopy(text));
    else fallbackCopy(text);
  }
  function fallbackCopy(text) {
    const ta = el("textarea", { value: text });
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.append(ta);
    ta.select();
    try { document.execCommand("copy"); toast("Copied"); } catch (e) { toast("Copy is blocked here"); }
    ta.remove();
  }

  // No <form>: sandboxed frames without allow-forms block form submission.
  $("act-send").addEventListener("click", () => ask($("act-input").value));
  $("act-close").addEventListener("click", clearSelection);
  $("act-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); ask($("act-input").value); }
    if (e.key === "Escape") { e.preventDefault(); clearSelection(); canvas.focus(); }
  });

  // ---------- tabs and controls

  function renderTabs() {
    const tabs = $("tabs");
    tabs.textContent = "";
    const views = (S.outline && S.outline.views) || [];
    if (views.length < 2) return;
    for (const v of views) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = VIEW_LABELS[v] || v;
      b.setAttribute("aria-pressed", String(S.data && S.data.view === v));
      b.onclick = () => {
        if (S.data && S.data.view === v) return;
        const keep = { file: S.args.file, view: v };
        for (const k of ["run", "table", "image", "workspace", "gatingml", "sample"]) if (S.args[k] !== undefined) keep[k] = S.args[k];
        S.args = keep;
        S.img = null;
        clearSelection();
        load();
      };
      tabs.appendChild(b);
    }
  }

  function el(tag, props, ...children) {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(props || {})) {
      if (k === "on") for (const [ev, fn] of Object.entries(v)) e.addEventListener(ev, fn);
      else if (k === "style") e.setAttribute("style", v);
      else if (k in e) e[k] = v;
      else e.setAttribute(k, v);
    }
    for (const c of children) if (c != null) e.append(c);
    return e;
  }

  function select(label, options, value, onChange) {
    const s = el("select", { on: { change: () => onChange(s.value) } });
    for (const [v, text] of options) s.append(el("option", { value: String(v), textContent: text }));
    s.value = String(value);
    return el("label", {}, label, s);
  }

  function slider(label, max, value, onChange) {
    const out = el("span", { className: "value", textContent: `${value + 1}/${max + 1}` });
    const r = el("input", { type: "range", min: 0, max, step: 1, value });
    r.addEventListener("input", () => (out.textContent = `${Number(r.value) + 1}/${max + 1}`));
    r.addEventListener("change", () => onChange(Number(r.value)));
    return el("label", {}, label, r, out);
  }

  function button(text, onClick, disabled, title) {
    return el("button", { type: "button", textContent: text, disabled: !!disabled, title: title || "", on: { click: onClick } });
  }

  function segmented(options, value, onChange) {
    const box = el("span", { className: "seg", role: "group" });
    for (const [v, text] of options) {
      const b = el("button", { type: "button", textContent: text, on: { click: () => onChange(v) } });
      b.setAttribute("aria-pressed", String(v === value));
      box.append(b);
    }
    return box;
  }

  function renderControls() {
    const box = $("controls");
    box.textContent = "";
    const d = S.data;
    if (!d) return;
    const o = S.outline || {};
    const add = (...xs) => xs.forEach((x) => x && box.append(x));
    const zoomed = ZOOM_KEYS.some((k) => S.args[k] !== undefined);
    if (d.view === "image" && d.image) {
      const im = d.image;
      const info = imageInfo();
      if ((o.images || []).length > 1) {
        add(select("Image", o.images.map((x) => [x.index, `${x.index}${x.name ? " · " + x.name : ""}`]), im.image, (v) => { S.img = null; S.ann = null; load({ image: Number(v), c: undefined, z: undefined, t: undefined, composite: undefined }); }));
      }
      const nc = info.size_c || im.channels.length || 1;
      if (nc > 1 && !im.rgb) {
        const opts = [];
        for (let c = 0; c < nc; c++) {
          const ch = (info.channels || [])[c] || {};
          opts.push([c, ch.name ? `${c} · ${ch.name}` : `Channel ${c}`]);
        }
        opts.push(["all", "Composite"]);
        add(select("Channel", opts, im.composite ? "all" : im.c[0], (v) =>
          v === "all" ? reloadPlane({ composite: true, channels: undefined, c: undefined }) : reloadPlane({ composite: undefined, c: Number(v) })));
      }
      if ((info.size_z || 1) > 1) {
        if (!im.projection) add(slider("Z", info.size_z - 1, im.z[0] || 0, (v) => reloadPlane({ z: v })));
        const mip = el("input", { type: "checkbox", checked: !!im.projection, on: { change: () => reloadPlane({ mip: mip.checked || undefined }) } });
        add(el("label", {}, mip, "Max projection"));
      }
      if ((info.size_t || 1) > 1) add(slider("T", info.size_t - 1, im.t[0] || 0, (v) => reloadPlane({ t: v })));
      add(select("Contrast", [["auto", "Auto"], ["min-max", "Min–max"], ["percentile:1,99", "1–99 %"], ["percentile:0.1,99.9", "0.1–99.9 %"], ["raw", "Full range"]],
        S.args.contrast || "auto", (v) => reloadPlane({ contrast: v === "auto" ? undefined : v })));
      add(el("span", { className: "group" },
        button("−", () => zoomBy(1 / 2), false, "Zoom out (−)"),
        button("+", () => zoomBy(2), false, "Zoom in (+)"),
        button("Fit", () => zoomTo(fullRect()), false, "Whole image (0)")));
    } else if (d.plot && d.plot.kind === "trace") {
      const p = d.plot;
      const traces = o.traces || [];
      if (traces.length > 1) add(select("Trace", traces.map((t) => [t.index, `${t.index}${t.name ? " · " + t.name : ""}`]), p.trace, (v) => load({ trace: Number(v), sweep: undefined, channels: undefined })));
      if (p.sweep_count > 1) {
        add(el("span", { className: "group" },
          button("‹", () => load({ sweep: p.sweep - 1 }, { keepZoom: true }), p.sweep <= 0),
          slider("Sweep", p.sweep_count - 1, p.sweep, (v) => load({ sweep: v }, { keepZoom: true })),
          button("›", () => load({ sweep: p.sweep + 1 }, { keepZoom: true }), p.sweep >= p.sweep_count - 1)));
      }
      const t = traces.find((x) => x.index === p.trace);
      if (t && t.channel_count > 1 && !p.overlay) {
        // One chip per channel; the plotted ones are pressed. At most 8 at once.
        const shown = p.series.map((s) => s.channel);
        const chips = el("span", { className: "chips" });
        t.channels.forEach((ch, c) => {
          const on = shown.includes(c);
          const name = ch.name || `ch ${c}`;
          const chip = el("button", { type: "button", className: "chip", title: name },
            el("span", { className: "swatch", style: `background:${seriesColor(shown.indexOf(c) >= 0 ? shown.indexOf(c) : c)}` }), legendLabel(name, 24));
          chip.setAttribute("aria-pressed", String(on));
          chip.addEventListener("click", () => {
            const next = on ? shown.filter((x) => x !== c) : shown.concat([c]).sort((a, b) => a - b);
            if (!next.length || next.length > 8) return;
            load({ channels: next }, { keepZoom: true });
          });
          chips.append(chip);
        });
        add(chips);
      }
      add(button("Reset zoom", () => load({}), !zoomed));
    } else if (d.plot && d.plot.kind === "nmr") {
      const nmrTraces = (o.traces || []).filter((t) => t.nmr);
      if (nmrTraces.length > 1) add(select("Trace", nmrTraces.map((t) => [t.index, `${t.index}${t.name ? " · " + t.name : ""}`]), d.plot.trace, (v) => load({ trace: Number(v) })));
      add(button("Reset zoom", () => load({}), !zoomed));
    } else if (d.plot && d.plot.kind === "chromatogram") {
      const p = d.plot;
      if (p.source === "spectra") {
        const mz = el("input", { type: "text", placeholder: "m/z, m/z…", value: (S.args.mz || []).join(", "), title: "Extracted-ion chromatograms at these m/z (empty: TIC)" });
        const go = () => {
          const list = mz.value.split(/[ ,;]+/).map(Number).filter((x) => isFinite(x) && x > 0);
          load({ mz: list.length ? list : undefined }, { keepZoom: true });
        };
        mz.addEventListener("keydown", (e) => e.key === "Enter" && go());
        add(el("label", {}, "XIC", mz), button("Show", go));
        if ((o.spectra || []).length > 1) add(select("Run", o.spectra.map((s) => [s.index, `${s.index}${s.name ? " · " + s.name : ""}`]), S.args.run || 0, (v) => load({ run: Number(v) })));
      } else {
        const rt = (o.traces || []).filter((t) => t.retention);
        if (rt.length > 1) add(select("Trace", rt.map((t) => [t.index, `${t.index}${t.name ? " · " + t.name : ""}`]), p.trace, (v) => load({ trace: Number(v), channels: undefined })));
        const t = (o.traces || []).find((x) => x.index === p.trace);
        if (t && t.channel_count > 1) add(select("Channel", t.channels.map((c, i) => [i, c.name || `ch ${i}`]), (S.args.channels || [0])[0], (v) => load({ channels: [Number(v)] }, { keepZoom: true })));
      }
      const pk = el("input", { type: "checkbox", checked: (p.peaks || []).length > 0 || S.args.peaks === true, on: { change: () => load({ peaks: pk.checked }, { keepZoom: true }) } });
      add(el("label", {}, pk, "Peaks"));
      add(button("Reset zoom", () => load({}), !zoomed));
    } else if (d.plot && d.plot.kind === "spectrum") {
      const p = d.plot, s = p.spectrum;
      add(el("span", { className: "group" },
        button("‹", () => load({ index: s.index - 1, rt_min: undefined, scan: undefined }, { keepZoom: true }), s.index <= 0, "Previous scan (←)"),
        el("span", { className: "value", textContent: `scan ${s.scan_number} · MS${s.ms_level}${s.rt_min != null ? " · " + fmt(s.rt_min, 4) + " min" : ""}${s.precursor_mz ? " · precursor " + fmt(s.precursor_mz, 7) : ""}` }),
        button("›", () => load({ index: s.index + 1, rt_min: undefined, scan: undefined }, { keepZoom: true }), s.index >= p.scan_count - 1, "Next scan (→)")));
      add(button("Chromatogram", () => { S.args = { file: S.args.file, view: "chromatogram", run: p.run }; load(); }));
      add(button("Reset zoom", () => load({}), !zoomed));
    } else if (d.plate) {
      const pl = d.plate;
      const tables = (o.tables || []).filter((t) => t.plate);
      if (tables.length > 1) add(select("Table", tables.map((t) => [t.index, `${t.index}${t.name ? " · " + t.name : ""}`]), pl.table, (v) => load({ table: Number(v), column: undefined, read: undefined })));
      const t = (o.tables || []).find((x) => x.index === pl.table);
      if (t && pl.layout === "long" && !pl.kinetic) {
        const skip = new Set(["well", "row", "column", "col", "read", "wavelength_nm", "time_s"]);
        const cols = t.columns.filter((c) => !skip.has(c.name.toLowerCase()));
        if (cols.length > 1) add(select("Value", cols.map((c) => [c.name, c.label || c.name]), S.args.column || cols[0].name, (v) => load({ column: v })));
      }
      const k = pl.kinetic;
      if (k) {
        add(segmented([["curves", "Curves"], ["heat", "Heat map"]], S.kin.mode, (m) => { S.kin.mode = m; renderControls(); invalidate(); tellModel(); }));
        if ((k.reads || []).length > 1) add(select("Read", k.reads.map((r) => [r.read, r.label || `read ${r.read}`]), k.read, (v) => load({ read: Number(v) })));
        const play = button(S.kin.playing ? "❚❚" : "▶", () => togglePlay(), false, "Play the time course");
        play.id = "kin-play";
        const out = el("span", { className: "value", id: "kin-time", textContent: fmtTime(kinTime()) });
        const r = el("input", { type: "range", min: 0, max: k.time_s.length - 1, step: 1, value: S.kin.t, "aria-label": "Time" });
        r.id = "kin-range";
        r.addEventListener("input", () => { S.kin.t = Number(r.value); out.textContent = fmtTime(kinTime()); invalidate(); });
        r.addEventListener("change", () => tellModel());
        add(el("span", { className: "group" }, play, r, out));
      }
      const maps = Object.keys(pl.groups || {});
      if (maps.length) {
        const opts = maps.map((m) => [m, m]).concat([["", k ? "value at the time" : "value"]]);
        add(select("Colour", opts, S.kin.colorBy || "", (v) => { S.kin.colorBy = v || null; renderControls(); invalidate(); }));
      }
      if (!k) add(el("span", { className: "muted", textContent: pl.value }));
      const chips = groupChips();
      if (chips) add(chips);
    } else if (d.events) {
      const ev = d.events;
      const gt = ev.gating;
      if (gt) {
        // Breadcrumb from all events down to the population shown.
        const crumbs = el("span", { className: "crumbs" });
        const path = gt.shown.path === "/" ? [] : gt.shown.path.split("/").filter(Boolean);
        const step = (label, target, here) => here ? el("span", { className: "here", textContent: label }) : el("button", { type: "button", textContent: label, on: { click: () => { clearSelection(); load({ population: target, x: undefined, y: undefined }); } } });
        crumbs.append(step("All events", "/", path.length === 0));
        path.forEach((name, i) => {
          crumbs.append(el("span", { className: "muted", textContent: "›" }));
          crumbs.append(step(name, "/" + path.slice(0, i + 1).join("/"), i === path.length - 1));
        });
        add(crumbs);
      }
      const t = (o.tables || []).find((x) => x.index === ev.table);
      const cols = t ? t.columns : [ev.x].concat(ev.y ? [ev.y] : []);
      const opts = cols.map((c) => [c.name, c.label ? `${c.name} · ${c.label}` : c.name]);
      add(select("X", opts, ev.x.name, (v) => load({ x: v })));
      add(select("Y", [["", "Histogram"]].concat(opts), ev.y ? ev.y.name : "", (v) => load({ y: v })));
      for (const axis of ["x", "y"]) {
        const c = ev[axis];
        if (!c) continue;
        add(select(axis.toUpperCase() + " scale", [["linear", "Linear"], ["log", "Log"], ["arcsinh", "Arcsinh"]], fcsScaleOf(c.name), (v) => {
          S.fcsScale[c.name] = v;
          clearSelection();
          invalidate();
        }));
      }
      add(select("Events", [[10000, "10 000"], [25000, "25 000"], [50000, "50 000"]], S.args.events || 10000, (v) => load({ events: Number(v) })));
    } else if (d.view === "figure") {
      add(button("Reset zoom", () => (S.fig && S.fig.xRange ? redrawFigure(null) : figureZoom(null)), !(S.fig && (S.fig.zoom || S.fig.xRange))));
    }
  }

  function reloadPlane(patch) {
    // Same image, other plane: keep the camera, drop the pictures of the old plane.
    load(patch, { keepZoom: true });
  }

  function fcsScaleOf(name) {
    if (S.fcsScale[name]) return S.fcsScale[name];
    return /^(FSC|SSC|Time)/i.test(name) ? "linear" : "arcsinh";
  }

  // ---------- colours

  let palette = null;
  const probe = document.createElement("span");
  probe.style.display = "none";
  document.body.appendChild(probe);
  /** Data and chrome colours for the current host theme (canvas cannot parse light-dark()). */
  function colors() {
    if (palette) return palette;
    const cs = getComputedStyle(root);
    const v = (k, f) => {
      probe.style.color = f;
      probe.style.color = `var(${k}, ${f})`;
      return getComputedStyle(probe).color || f;
    };
    const bg = v("--color-background-primary", "#fbfaf7");
    const rgb = (bg.match(/[\d.]+/g) || [255, 255, 255]).map(Number);
    const dark = S.hostContext.theme ? S.hostContext.theme === "dark" : (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]) < 128;
    const th = dark ? THEMES.dark : THEMES.light;
    const hostSans = (cs.getPropertyValue("--font-sans") || "").trim() || "system-ui, sans-serif";
    palette = Object.assign({}, th, {
      dark,
      ink: v("--color-text-primary", dark ? "#eceae4" : "#1d1f24"),
      muted: v("--color-text-secondary", "#6b6e75"),
      faintText: v("--color-text-tertiary", "#999"),
      rule: v("--color-border-primary", "#d6d1c7"),
      grid: v("--color-border-secondary", "#e9e5dd"),
      bg,
      panel: v("--color-background-secondary", "#f4f2ec"),
      sans: `"OpenReadout Sans", "IBM Plex Sans", ${hostSans}`,
      mono: MONO,
    });
    palette.series = [palette.ink].concat(th.series.slice(1));
    return palette;
  }
  function seriesColor(i) {
    const c = colors();
    return c.series[i % c.series.length];
  }
  function alpha(color, a) {
    const m = String(color).match(/[\d.]+/g);
    if (String(color).startsWith("#")) {
      const [r, gg, b] = hexRgb(color);
      return `rgba(${r},${gg},${b},${a})`;
    }
    if (m && m.length >= 3) return `rgba(${m[0]},${m[1]},${m[2]},${a})`;
    return color;
  }

  // ---------- drawing infrastructure

  /** Size the canvas for its box; returns CSS width and height. */
  function sizeCanvas(aspect, minH) {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(160, canvas.parentElement.clientWidth);
    let h;
    if (root.classList.contains("fill")) {
      const stage = canvas.parentElement.getBoundingClientRect();
      h = Math.max(160, window.innerHeight - stage.top - 34);
    } else {
      const maxH = (S.hostContext.containerDimensions && S.hostContext.containerDimensions.maxHeight) || 0;
      h = aspect ? w / aspect : Math.round(Math.min(520, Math.max(240, w * 0.56)));
      h = Math.max(h, minH || 0);
      h = Math.min(h, maxH ? maxH - 130 : Math.max(680, minH || 0));
      h = Math.max(160, h);
    }
    h = Math.round(h);
    canvas.style.height = h + "px";
    const W = Math.round(w * dpr), H = Math.round(h * dpr);
    if (canvas.width !== W || canvas.height !== H) { canvas.width = W; canvas.height = H; }
    if (base.width !== W || base.height !== H) { base.width = W; base.height = H; }
    bctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    S.size = [w, h];
    return [w, h];
  }

  let frameQueued = false;
  /** Redraw everything (data changed, size or theme changed). */
  function invalidate() {
    render();
    compose();
    if (S.sel && S.sel.kind === "wells" && !$("act").hidden) positionAct(actAnchorForWells(S.sel.wells));
  }
  /** Redraw only the pointer's overlays. */
  function invalidateOverlay() {
    if (frameQueued) return;
    frameQueued = true;
    requestAnimationFrame(() => { frameQueued = false; compose(); });
  }

  function render() {
    const d = S.data;
    S.layout = null;
    g = bctx;
    if (!d) return;
    if (d.view === "image" || d.view === "static") drawImageView();
    else if (d.view === "figure") drawFigure();
    else if (d.plot) drawPlot();
    else if (d.plate) drawPlate();
    else if (d.events) drawEvents();
    else drawSummary();
  }

  function compose() {
    if (!S.data) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(base, 0, 0);
    const dpr = window.devicePixelRatio || 1;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    g = ctx;
    try {
      drawOverlays();
    } finally {
      g = bctx;
    }
  }

  function drawOverlays() {
    const L = S.layout;
    if (!L) return;
    if (L.kind === "image") imageOverlays(L);
    else if (L.kind === "plot") plotOverlays(L);
    else if (L.kind === "plate") plateOverlays(L);
    else if (L.kind === "events") eventOverlays(L);
    else if (L.kind === "figure") figureOverlays(L);
    if (S.drag && S.drag.moved && S.drag.mode === "select") dragOverlay(L);
  }

  function font(size, weight, mono) {
    const c = colors();
    return `${weight || 400} ${size}px ${mono ? c.mono : c.sans}`;
  }

  function txt(s, x, y, o) {
    o = o || {};
    g.font = o.font || font(o.size || 11, o.weight, o.mono);
    if (o.halo) {
      g.save();
      g.strokeStyle = o.halo;
      g.lineWidth = 4;
      g.lineJoin = "round";
      g.textAlign = o.align || "left";
      g.textBaseline = o.baseline || "alphabetic";
      g.strokeText(s, x, y);
      g.restore();
    }
    g.fillStyle = o.color || colors().muted;
    g.textAlign = o.align || "left";
    g.textBaseline = o.baseline || "alphabetic";
    g.fillText(s, x, y);
  }

  /** A label pill: rounded box with text, used for annotations on pictures. */
  function pill(text, x, y, o) {
    o = o || {};
    const c = colors();
    g.font = font(o.size || 12, 500);
    const tw = g.measureText(text).width;
    const h = (o.size || 12) + 9, dot = o.dot ? 12 : 0;
    const w = tw + 14 + dot;
    if (o.measureOnly) return { w, h };
    const r = h / 2;
    g.save();
    g.shadowColor = "rgba(0,0,0,0.22)";
    g.shadowBlur = 6;
    g.shadowOffsetY = 1;
    g.fillStyle = o.bg || "rgba(255,255,255,0.94)";
    roundRect(x, y, w, h, r);
    g.fill();
    g.restore();
    if (o.stroke) {
      g.strokeStyle = o.stroke;
      g.lineWidth = 1.5;
      roundRect(x + 0.75, y + 0.75, w - 1.5, h - 1.5, r);
      g.stroke();
    }
    if (o.dot) {
      g.beginPath();
      g.arc(x + 11, y + h / 2, 3.5, 0, Math.PI * 2);
      g.strokeStyle = o.dotColor || "#1d1f24";
      g.lineWidth = 1.5;
      g.stroke();
    }
    txt(text, x + 7 + dot, y + h / 2 + 0.5, { font: font(o.size || 12, 500), color: o.color || "#1d1f24", baseline: "middle" });
    return { w, h, c };
  }

  function roundRect(x, y, w, h, r) {
    g.beginPath();
    if (g.roundRect) g.roundRect(x, y, w, h, r);
    else g.rect(x, y, w, h);
  }

  function drawSummary() {
    const [w, h] = sizeCanvas(null, 160);
    const c = colors();
    g.clearRect(0, 0, w, h);
    const o = S.outline || {};
    txt("Nothing to draw in this file", 18, 34, { size: 15, weight: 500, color: c.ink });
    const counts = `${o.images_total || 0} images · ${o.traces_total || 0} traces · ${(o.spectra || []).length} spectrum runs · ${o.tables_total || 0} tables`;
    txt(counts, 18, 58, { size: 12, mono: true, color: c.muted });
    txt("Ask the agent about its metadata.", 18, 82, { size: 12, color: c.muted });
    (o.notes || []).slice(0, 4).forEach((l, i) => txt(legendLabel(l, 110), 18, 110 + i * 18, { size: 11.5, color: c.faintText }));
    S.layout = { kind: "summary" };
  }

  // ---------- images: deep zoom with the agent's annotations

  function imageInfo() {
    const d = S.data || {};
    const idx = d.image ? d.image.image : S.args.image || 0;
    return ((S.outline || {}).images || []).find((x) => x.index === idx) || {};
  }

  function physical() {
    const ps = imageInfo().physical_size || {};
    if (!(ps.x > 0)) return null;
    let k = 1;
    const u = String(ps.unit || "µm").toLowerCase();
    if (u === "nm") k = 1e-3;
    else if (u === "mm") k = 1e3;
    else if (u === "m") k = 1e6;
    return { x: ps.x * k, y: (ps.y || ps.x) * k };
  }

  function planeKey(args, im) {
    const a = args || {};
    return JSON.stringify([im ? im.image : a.image || 0, im ? im.c : a.c, im ? im.z : a.z, im ? im.t : a.t, !!(im ? im.composite : a.composite), a.channels || null, !!(im ? im.projection : a.mip), a.contrast || "auto"]);
  }

  function fullRect() {
    const I = S.img;
    return { x: 0, y: 0, w: I.full.w, h: I.full.h };
  }

  function rectOfRegion(r) {
    return { x: r.x, y: r.y, w: r.width, h: r.height };
  }

  /** The camera rectangle as a region the tools take: whole pixels inside the image. */
  function regionOf(cam) {
    const I = S.img;
    const x0 = Math.max(0, Math.floor(cam.x)), y0 = Math.max(0, Math.floor(cam.y));
    const x1 = Math.min(I.full.w, Math.ceil(cam.x + cam.w)), y1 = Math.min(I.full.h, Math.ceil(cam.y + cam.h));
    return { x: x0, y: y0, width: Math.max(1, x1 - x0), height: Math.max(1, y1 - y0) };
  }

  function isFull(cam) {
    const I = S.img;
    return cam.x <= 0.5 && cam.y <= 0.5 && cam.x + cam.w >= I.full.w - 0.5 && cam.y + cam.h >= I.full.h - 0.5;
  }

  async function showImage(d, img) {
    const im = d.image;
    const info = imageInfo();
    const fullW = info.size_x || (im.full_res_region ? im.full_res_region.width : img.width);
    const fullH = info.size_y || (im.full_res_region ? im.full_res_region.height : img.height);
    const key = planeKey(S.args, im);
    const reg = im.full_res_region || { x: 0, y: 0, width: fullW, height: fullH };
    const rect = rectOfRegion(reg);
    const tex = { img, rect, res: rect.w / img.width };
    const prev = S.img;
    const follow = prev && prev.follow;
    if (prev && prev.key === key && prev.full.w === fullW) {
      // Same plane: keep what we have and move the camera if the region changed.
      prev.textures.push(tex);
      prev.textures = prev.textures.slice(-8);
      if (isWhole(rect, prev.full)) prev.overview = tex;
      overlay(null);
      if (follow) {
        // The agent looked somewhere else: follow it there, in a frame shaped like its region
        // (a wide strip after a square would otherwise sit in wide margins).
        prev.follow = false;
        const aspect = frameAspect(rect);
        if (Math.abs(aspect - prev.aspect) > 0.02) {
          prev.aspect = aspect;
          invalidate();
        }
        flyTo(rect, 1100);
      } else {
        invalidate();
      }
      return;
    }
    if (prev && prev.full.w === fullW && prev.full.h === fullH && prev.imageIndex === im.image) {
      // Other plane of the same image (channel, z, contrast): keep the camera.
      S.img = Object.assign(prev, { key, textures: [tex], overview: isWhole(rect, prev.full) ? tex : null, follow: false });
      overlay(null);
      invalidate();
      fetchOverview();
      return;
    }
    const aspect = frameAspect(rect);
    S.img = {
      key, imageIndex: im.image, full: { w: fullW, h: fullH }, aspect,
      textures: [tex], overview: null, cam: null, anim: null, settled: true, follow: false, pending: null,
    };
    S.img.overview = isWhole(rect, S.img.full) ? tex : null;
    if (!S.img.overview) {
      // The agent zoomed in: show the whole picture first, then fly to its region, so the
      // user sees where on the slide it is.
      window.__animating = true;
      const ov = await fetchOverview();
      if (!S.img || S.img.key !== key) return;
      overlay(null);
      if (ov) {
        S.img.cam = null;
        invalidate();
        await sleep(450);
        if (S.img && S.img.key === key) flyTo(rect, 1400);
        return;
      }
      window.__animating = false;
    }
    overlay(null);
    S.img.cam = null;
    S.img.aspectRect = rect;
    invalidate();
  }

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  /** The viewer frame's shape for a picture of `rect`, within limits a chat column can show. */
  function frameAspect(rect) {
    return Math.min(2.4, Math.max(0.55, rect.w / rect.h));
  }

  function isWhole(rect, full) {
    return rect.x <= 0 && rect.y <= 0 && rect.w >= full.w - 1 && rect.h >= full.h - 1;
  }

  function canvasAspect() {
    const [w, h] = S.size;
    return w > 0 && h > 0 ? w / h : (S.img ? S.img.aspect : 1.5);
  }

  async function fetchOverview() {
    const I = S.img;
    if (!I || I.overview) return I && I.overview;
    const key = I.key;
    const args = Object.assign({}, S.args, { outline: false, max_size: 640 });
    delete args.region;
    try {
      const r = await callView(args);
      const im = (r.content || []).find((c) => c.type === "image");
      if (!im || !S.img || S.img.key !== key) return null;
      const img = await decode(im);
      const reg = (r.structuredContent && r.structuredContent.image && r.structuredContent.image.full_res_region) || { x: 0, y: 0, width: I.full.w, height: I.full.h };
      const tex = { img, rect: rectOfRegion(reg), res: reg.width / img.width };
      S.img.overview = tex;
      S.img.textures.unshift(tex);
      invalidate();
      return tex;
    } catch (e) {
      return null;
    }
  }

  /** Move the camera to show `rect`, animated over `ms`. */
  function flyTo(rect, ms) {
    const I = S.img;
    if (!I) return;
    const target = fitAspect(rect, canvasAspect());
    const from = I.cam || target;
    if (I.anim) cancelAnimationFrame(I.anim.raf);
    const reduce = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (!ms || reduce) {
      I.cam = target;
      I.anim = null;
      settle();
      return;
    }
    window.__animating = true;
    I.settled = false;
    const t0 = performance.now();
    const step = (now) => {
      const t = Math.min(1, (now - t0) / ms);
      I.cam = lerpRect(from, target, t);
      invalidate();
      if (t < 1) I.anim = { raf: requestAnimationFrame(step) };
      else { I.anim = null; settle(); }
    };
    I.anim = { raf: requestAnimationFrame(step) };
  }

  function zoomTo(rect) {
    if (!S.img) return;
    flyTo(rect, 650);
  }

  function zoomBy(k, at) {
    const I = S.img;
    if (!I) return;
    const c = I.cam;
    const cx = at ? at.x : c.x + c.w / 2, cy = at ? at.y : c.y + c.h / 2;
    let w = c.w / k, h = c.h / k;
    const maxW = I.full.w * 1.6;
    if (w > maxW) { h *= maxW / w; w = maxW; }
    if (w < 16) { h *= 16 / w; w = 16; }
    const fx = at ? (cx - c.x) / c.w : 0.5, fy = at ? (cy - c.y) / c.h : 0.5;
    flyTo({ x: cx - w * fx, y: cy - h * fy, w, h }, 420);
  }

  function pan(dx, dy) {
    const I = S.img;
    if (!I) return;
    I.cam = clampCam({ x: I.cam.x + dx, y: I.cam.y + dy, w: I.cam.w, h: I.cam.h });
    I.settled = false;
    invalidate();
  }

  function clampCam(c) {
    const I = S.img;
    const mx = Math.max(c.w * 0.5, 0), my = Math.max(c.h * 0.5, 0);
    return {
      x: Math.min(I.full.w - c.w + mx, Math.max(-mx, c.x)),
      y: Math.min(I.full.h - c.h + my, Math.max(-my, c.y)),
      w: c.w, h: c.h,
    };
  }

  let settleTimer = 0;
  function settle() {
    const I = S.img;
    if (!I) return;
    window.__animating = false;
    I.settled = true;
    invalidate();
    clearTimeout(settleTimer);
    settleTimer = setTimeout(() => {
      if (!S.img) return;
      // The region the tools take, for the context and the next control change.
      if (isFull(S.img.cam) || !visibleRegion()) delete S.args.region;
      else S.args.region = visibleRegion();
      fetchDetail();
      tellModel();
    }, 140);
  }

  function visibleRegion() {
    const I = S.img;
    const c = I.cam;
    const x0 = Math.max(0, c.x), y0 = Math.max(0, c.y);
    const x1 = Math.min(I.full.w, c.x + c.w), y1 = Math.min(I.full.h, c.y + c.h);
    if (x1 <= x0 || y1 <= y0) return null;
    return regionOf({ x: x0, y: y0, w: x1 - x0, h: y1 - y0 });
  }

  /** Load a sharper picture of what the camera shows, unless one is already there. */
  async function fetchDetail() {
    const I = S.img;
    if (!I || !I.settled) return;
    const reg = visibleRegion();
    if (!reg) return;
    const [w] = S.size;
    const dpr = window.devicePixelRatio || 1;
    const need = I.cam.w / (w * dpr); // full-res px per device px
    const covers = (t) => t.rect.x <= reg.x + 0.5 && t.rect.y <= reg.y + 0.5 && t.rect.x + t.rect.w >= reg.x + reg.width - 0.5 && t.rect.y + t.rect.h >= reg.y + reg.height - 0.5;
    if (I.textures.some((t) => covers(t) && t.res <= Math.max(1, need * 1.3))) return;
    const key = I.key + JSON.stringify(reg);
    if (I.pending === key) return;
    I.pending = key;
    const args = Object.assign({}, S.args, { outline: false });
    const full = reg.x === 0 && reg.y === 0 && reg.width === I.full.w && reg.height === I.full.h;
    if (full) delete args.region;
    else args.region = reg;
    const vis = reg.width / reg.height;
    const px = Math.ceil(Math.min(1600, Math.max(256, (vis >= canvasAspect() ? w : S.size[1] * vis) * dpr)));
    args.max_size = Math.min(1600, Math.ceil(Math.max(px, px / vis) / 64) * 64);
    const planeAt = I.key;
    root.setAttribute("aria-busy", "true");
    overlay("Loading…");
    try {
      const r = await callView(args);
      const im = (r.content || []).find((c) => c.type === "image");
      if (!im || !S.img || S.img.key !== planeAt) return;
      const img = await decode(im);
      const fr = r.structuredContent && r.structuredContent.image && r.structuredContent.image.full_res_region;
      const rect = fr ? rectOfRegion(fr) : { x: reg.x, y: reg.y, w: reg.width, h: reg.height };
      const keep = S.img.textures.filter((t) => t !== S.img.overview).slice(-9);
      S.img.textures = (S.img.overview ? [S.img.overview] : []).concat(keep, [{ img, rect, res: rect.w / img.width, born: performance.now() }]);
      if (r.structuredContent && r.structuredContent.image) S.data.image = r.structuredContent.image;
      fadeIn();
    } catch (e) {
      toast(String(e && e.message || e).slice(0, 120));
    } finally {
      if (S.img) S.img.pending = null;
      root.setAttribute("aria-busy", "false");
      overlay(null);
    }
  }

  function fadeIn() {
    const t0 = performance.now();
    window.__animating = true;
    const step = (now) => {
      invalidate();
      if (now - t0 < 260) requestAnimationFrame(step);
      else window.__animating = !!(S.img && S.img.anim);
    };
    requestAnimationFrame(step);
  }

  function drawImageView() {
    if (S.data.view === "static") return drawStatic();
    const I = S.img;
    if (!I) return;
    const fill = root.classList.contains("fill");
    let aspect = I.aspect;
    if (!fill) {
      const box = canvas.parentElement.clientWidth;
      const maxH = Math.min(680, ((S.hostContext.containerDimensions || {}).maxHeight || 800) - 130);
      if (box / aspect > maxH) aspect = box / maxH;
      if (box / aspect < 220) aspect = box / 220;
    }
    const [w, h] = sizeCanvas(fill ? null : aspect);
    const c = colors();
    if (!I.cam) {
      I.cam = fitAspect(I.aspectRect || fullRect(), w / h);
      I.aspectRect = null;
    }
    // keep the camera's aspect equal to the canvas's
    if (Math.abs(I.cam.w / I.cam.h - w / h) > 1e-3) I.cam = fitAspect(I.cam, w / h);
    const cam = I.cam;
    const k = w / cam.w;
    const X = (x) => (x - cam.x) * k, Y = (y) => (y - cam.y) * k;
    g.clearRect(0, 0, w, h);
    g.fillStyle = c.panel;
    g.fillRect(0, 0, w, h);
    // image bounds (letterbox outside)
    const bx0 = Math.max(0, X(0)), by0 = Math.max(0, Y(0)), bx1 = Math.min(w, X(I.full.w)), by1 = Math.min(h, Y(I.full.h));
    // coarse to fine
    const tex = I.textures.slice().sort((a, b) => b.res - a.res);
    g.save();
    g.beginPath();
    g.rect(bx0, by0, Math.max(0, bx1 - bx0), Math.max(0, by1 - by0));
    g.clip();
    const now = performance.now();
    for (const t of tex) {
      const dx = X(t.rect.x), dy = Y(t.rect.y), dw = t.rect.w * k, dh = t.rect.h * k;
      if (dx > w || dy > h || dx + dw < 0 || dy + dh < 0) continue;
      g.imageSmoothingEnabled = dw / t.img.width < 2.5;
      g.imageSmoothingQuality = "high";
      g.globalAlpha = t.born ? Math.min(1, (now - t.born) / 240) : 1;
      g.drawImage(t.img, dx, dy, dw, dh);
    }
    g.globalAlpha = 1;
    g.restore();
    S.layout = { kind: "image", w, h, X, Y, k, cam: Object.assign({}, cam), inv: (px, py) => ({ x: cam.x + px / k, y: cam.y + py / k }) };
    S.layout.miniBox = minimapBox(S.layout);
    drawAnnotationsOnImage(S.layout);
    drawScaleBar(S.layout);
    drawMinimap(S.layout);
    drawChannelLegend(S.layout);
  }

  function drawStatic() {
    const img = S.picture && S.picture.img;
    if (!img) return;
    const [w, h] = sizeCanvas(img.width / img.height);
    g.clearRect(0, 0, w, h);
    const s = Math.min(w / img.width, h / img.height);
    g.drawImage(img, (w - img.width * s) / 2, (h - img.height * s) / 2, img.width * s, img.height * s);
    S.layout = { kind: "static" };
  }

  function drawScaleBar(L) {
    const ps = physical();
    if (!ps) return;
    const umPerPx = ps.x / L.k;
    const nice = niceBelow(umPerPx * L.w * 0.18);
    const len = nice / umPerPx;
    if (!(len > 16)) return;
    const label = fmtLength(nice, 3);
    g.font = font(11.5, 500, true);
    const tw = g.measureText(label).width;
    const bw = Math.max(len, tw) + 20, bh = 30;
    const x = L.w - bw - 12, y = L.h - bh - 12;
    g.save();
    g.fillStyle = "rgba(255,255,255,0.9)";
    g.shadowColor = "rgba(0,0,0,0.15)";
    g.shadowBlur = 4;
    roundRect(x, y, bw, bh, 4);
    g.fill();
    g.restore();
    txt(label, x + bw - 10, y + 13, { font: font(11.5, 500, true), color: "#1d1f24", align: "right" });
    g.fillStyle = "#1d1f24";
    g.fillRect(x + bw - 10 - len, y + bh - 9, len, 3);
  }

  /** Where the minimap goes, so labels can stay clear of it (null when there is none). */
  function minimapBox(L) {
    const I = S.img;
    if (!I || !I.overview || isFull(I.cam) || I.cam.w * I.cam.h > I.full.w * I.full.h * 0.8) return null;
    const s = Math.min(Math.min(150, L.w * 0.24) / I.full.w, Math.min(110, L.h * 0.3) / I.full.h);
    const mw = I.full.w * s, mh = I.full.h * s;
    return { x: 9, y: L.h - mh - 15, w: mw + 6, h: mh + 6 };
  }

  function drawMinimap(L) {
    const I = S.img;
    if (!I || !I.overview || isFull(I.cam) || I.cam.w * I.cam.h > I.full.w * I.full.h * 0.8) {
      if (L) L.mini = null;
      return;
    }
    const ov = I.overview;
    const maxW = Math.min(150, L.w * 0.24), maxH = Math.min(110, L.h * 0.3);
    const s = Math.min(maxW / I.full.w, maxH / I.full.h);
    const mw = I.full.w * s, mh = I.full.h * s;
    const x = 12, y = L.h - mh - 12;
    g.save();
    g.fillStyle = "rgba(255,255,255,0.92)";
    g.shadowColor = "rgba(0,0,0,0.2)";
    g.shadowBlur = 6;
    g.fillRect(x - 3, y - 3, mw + 6, mh + 6);
    g.restore();
    g.imageSmoothingEnabled = true;
    g.drawImage(ov.img, x + ov.rect.x * s, y + ov.rect.y * s, ov.rect.w * s, ov.rect.h * s);
    const c = I.cam;
    const rx = x + Math.max(0, c.x) * s, ry = y + Math.max(0, c.y) * s;
    let rw = Math.min(I.full.w, c.x + c.w) * s + x - rx, rh = Math.min(I.full.h, c.y + c.h) * s + y - ry;
    if (rw < 8 || rh < 8) {
      // a small view: a marker of fixed size centred on it
      const cx = rx + rw / 2, cy = ry + rh / 2;
      rw = Math.max(8, rw); rh = Math.max(8, rh);
      L.miniMark = { x: cx - rw / 2, y: cy - rh / 2 };
    }
    g.fillStyle = "rgba(20,20,24,0.28)";
    g.fillRect(x, y, mw, ry - y);
    g.fillRect(x, ry + rh, mw, y + mh - ry - rh);
    g.fillRect(x, ry, rx - x, rh);
    g.fillRect(rx + rw, ry, x + mw - rx - rw, rh);
    const mk = L.miniMark || { x: rx, y: ry };
    g.strokeStyle = colors().accent;
    g.lineWidth = 1.5;
    g.strokeRect(mk.x, mk.y, rw, rh);
    L.mini = { x, y, w: mw, h: mh, s };
  }

  function drawChannelLegend(L) {
    const im = S.data.image;
    if (!im || !im.composite || !im.channels || !im.channels.length) return;
    let y = 12;
    for (const ch of im.channels.slice(0, 8)) {
      const label = `${ch.name || "ch " + ch.index}`;
      g.font = font(11.5, 500);
      const tw = g.measureText(label).width;
      g.fillStyle = "rgba(20,20,24,0.62)";
      roundRect(12, y, tw + 26, 20, 10);
      g.fill();
      g.fillStyle = ch.color;
      g.beginPath();
      g.arc(23, y + 10, 4, 0, Math.PI * 2);
      g.fill();
      txt(label, 31, y + 10.5, { font: font(11.5, 500), color: "#fff", baseline: "middle" });
      y += 24;
    }
  }

  /** The agent's annotations on an image, in full-resolution pixels. */
  function drawAnnotationsOnImage(L) {
    const A = S.ann;
    L.ann = [];
    if (!A || !A.list.length || (A.image ?? 0) !== (S.data.image ? S.data.image.image : 0)) return;
    const c = colors();
    const red = THEMES.light.accent;
    const items = [];
    for (const a of A.list) {
      const col = a.color || red;
      g.lineWidth = 2;
      g.strokeStyle = col;
      if (a.kind === "box") {
        const x = L.X(a.x), y = L.Y(a.y), w = a.w * L.k, h = a.h * L.k;
        halo(() => g.strokeRect(x, y, w, h));
        if (a.label) items.push({ a, ax: x, ay: y, w: 0, h: 0, anchor: "corner", gap: 4 });
        L.ann.push({ a, x, y, w, h });
      } else if (a.kind === "polygon") {
        g.beginPath();
        a.points.forEach(([px, py], i) => (i ? g.lineTo(L.X(px), L.Y(py)) : g.moveTo(L.X(px), L.Y(py))));
        if (a.closed) g.closePath();
        halo(() => g.stroke());
        if (a.closed) { g.fillStyle = alpha(col, 0.12); g.fill(); }
        const xs = a.points.map((p) => L.X(p[0])), ys = a.points.map((p) => L.Y(p[1]));
        const top = ys.indexOf(Math.min(...ys));
        if (a.label) items.push({ a, ax: xs[top], ay: ys[top] });
        L.ann.push({ a, x: Math.min(...xs), y: Math.min(...ys), w: Math.max(...xs) - Math.min(...xs), h: Math.max(...ys) - Math.min(...ys) });
      } else if (a.kind === "line") {
        const x0 = L.X(a.x0), y0 = L.Y(a.y0), x1 = L.X(a.x1), y1 = L.Y(a.y1);
        halo(() => { g.beginPath(); g.moveTo(x0, y0); g.lineTo(x1, y1); g.stroke(); if (a.arrow) arrowHead(x0, y0, x1, y1); });
        if (a.label) items.push({ a, ax: x0, ay: y0 });
        L.ann.push({ a, x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.abs(x1 - x0), h: Math.abs(y1 - y0) });
      } else if (a.kind === "point" || a.kind === "text") {
        const x = L.X(a.x), y = L.Y(a.y);
        if (a.kind === "point" && !a.label) {
          g.beginPath();
          g.arc(x, y, 5, 0, Math.PI * 2);
          g.fillStyle = "#fff";
          g.fill();
          g.lineWidth = 2;
          g.strokeStyle = col;
          g.stroke();
        }
        if (a.label) items.push({ a, ax: x, ay: y, centered: true });
        L.ann.push({ a, x: x - 6, y: y - 6, w: 12, h: 12 });
      }
    }
    // Labels: pills next to their anchors, never on top of each other.
    const measured = items.map((it) => Object.assign(it, pill(it.a.label, 0, 0, { measureOnly: true, dot: it.centered && it.a.kind === "point" })));
    const bounds = { x: 4, y: 4, w: L.w - 8, h: L.h - 8 };
    // Labelled points sit on their anchor (the ring marks the spot), like the launch pictures.
    const placed = placeLabels(measured.map((it) => it.centered
      ? { ax: it.ax, ay: it.ay, w: it.w, h: it.h, gap: 0 }
      : { ax: it.ax, ay: it.ay, w: it.w, h: it.h, gap: it.gap ?? 6 }), bounds, L.miniBox ? [L.miniBox] : []);
    measured.forEach((it, i) => {
      let { x, y } = placed[i];
      if (it.centered) {
        // the ring of a labelled point sits on the point itself
        const want = { x: it.ax - 11, y: it.ay - it.h / 2 };
        if (!measured.some((o, j) => j < i && overlaps(want, it, placed[j], o))) { x = want.x; y = want.y; placed[i] = want; }
      } else if (it.anchor === "corner") {
        const want = { x: it.ax, y: it.ay - it.h - 4 };
        if (want.y > 4 && !measured.some((o, j) => j < i && overlaps(want, it, placed[j], o))) { x = want.x; y = want.y; placed[i] = want; }
      }
      const hot = S.hover && S.hover.ann === it.a;
      const sel = S.sel && S.sel.kind === "annotation" && S.sel.ann === it.a;
      pill(it.a.label, x, y, { dot: it.centered && it.a.kind === "point", dotColor: it.a.color || "#1d1f24", stroke: hot || sel ? (it.a.color || c.accent) : null });
      L.ann.push({ a: it.a, x, y, w: it.w, h: it.h, label: true });
    });
  }

  function overlaps(p, a, q, b) {
    return p.x < q.x + b.w && p.x + a.w > q.x && p.y < q.y + b.h && p.y + a.h > q.y;
  }

  /** Stroke twice: a soft dark halo, then the colour, so marks read on any image. */
  function halo(stroke) {
    const col = g.strokeStyle, lw = g.lineWidth;
    g.save();
    g.strokeStyle = "rgba(255,255,255,0.85)";
    g.lineWidth = lw + 2.5;
    stroke();
    g.restore();
    g.strokeStyle = col;
    g.lineWidth = lw;
    stroke();
  }

  function arrowHead(x0, y0, x1, y1) {
    const a = Math.atan2(y1 - y0, x1 - x0), s = 10;
    g.beginPath();
    g.moveTo(x1, y1);
    g.lineTo(x1 - s * Math.cos(a - 0.4), y1 - s * Math.sin(a - 0.4));
    g.lineTo(x1 - s * Math.cos(a + 0.4), y1 - s * Math.sin(a + 0.4));
    g.closePath();
    g.fillStyle = g.strokeStyle;
    g.fill();
  }

  function imageOverlays(L) {
    const c = colors();
    const sel = S.sel;
    if (sel && sel.kind === "region") {
      const r = sel.region;
      const x = L.X(r.x), y = L.Y(r.y), w = r.width * L.k, h = r.height * L.k;
      g.fillStyle = "rgba(20,20,24,0.18)";
      g.fillRect(0, 0, L.w, y);
      g.fillRect(0, y + h, L.w, L.h - y - h);
      g.fillRect(0, y, x, h);
      g.fillRect(x + w, y, L.w - x - w, h);
      g.setLineDash([5, 4]);
      g.strokeStyle = "#fff";
      g.lineWidth = 1.5;
      g.strokeRect(x, y, w, h);
      g.setLineDash([]);
      g.strokeStyle = c.accent;
      g.lineWidth = 1;
      g.strokeRect(x - 1, y - 1, w + 2, h + 2);
    } else if (sel && sel.kind === "point") {
      const x = L.X(sel.x), y = L.Y(sel.y);
      g.beginPath();
      g.arc(x, y, 7, 0, Math.PI * 2);
      g.strokeStyle = "#fff";
      g.lineWidth = 3;
      g.stroke();
      g.strokeStyle = c.accent;
      g.lineWidth = 1.75;
      g.stroke();
    }
  }

  function dragOverlay(L) {
    const a = S.drag.a, b = S.drag.b;
    if (!a || !b) return;
    const c = colors();
    g.save();
    if (L.kind === "image" || L.kind === "events" || L.kind === "figure" || L.kind === "plate") {
      const x = Math.min(a[0], b[0]), y = Math.min(a[1], b[1]), w = Math.abs(b[0] - a[0]), h = Math.abs(b[1] - a[1]);
      g.fillStyle = alpha(c.accent, 0.1);
      g.fillRect(x, y, w, h);
      g.setLineDash([5, 4]);
      g.strokeStyle = L.kind === "image" ? "#fff" : c.accent;
      g.lineWidth = 1.5;
      g.strokeRect(x + 0.5, y + 0.5, w, h);
      if (L.kind === "image") {
        g.setLineDash([]);
        g.strokeStyle = c.accent;
        g.lineWidth = 1;
        g.strokeRect(x - 0.5, y - 0.5, w + 2, h + 2);
        const p = L.inv(x, y), q = L.inv(x + w, y + h);
        const label = `${fmtInt(q.x - p.x)} × ${fmtInt(q.y - p.y)} px`;
        pill(label, Math.min(L.w - 120, x), Math.max(4, y - 26), { size: 11 });
      }
    } else if (L.kind === "plot") {
      const x = Math.min(a[0], b[0]), w = Math.abs(b[0] - a[0]);
      g.fillStyle = alpha(c.accent, 0.1);
      g.fillRect(x, L.top, w, L.bottom - L.top);
      g.strokeStyle = c.accent;
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(x + 0.5, L.top); g.lineTo(x + 0.5, L.bottom);
      g.moveTo(x + w - 0.5, L.top); g.lineTo(x + w - 0.5, L.bottom);
      g.stroke();
    }
    g.restore();
  }

  // ---------- plots

  function plotModel() {
    const p = S.data.plot;
    const panels = [];
    const series = p.series || [];
    const n0 = series.length ? (series[0].y || series[0].lo || []).length : 0;
    let xs = null;
    const overlay = p.kind !== "trace" || p.overlay;
    // Many overlaid curves (qPCR wells): one colour per target, one label per target.
    let groups = null;
    if (series.length > 8) {
      const names = series.map((s) => seriesGroup(s.name));
      const distinct = [...new Set(names.filter(Boolean))];
      if (names.every(Boolean) && distinct.length <= 8 && distinct.length >= 1) groups = { names, distinct };
    }
    const prepared = series.map((s, i) => {
      const n = (s.y || s.lo || []).length;
      let x;
      if (Array.isArray(s.x)) x = Float64Array.from(s.x, (v) => (v == null ? NaN : v));
      else x = xs && xs.length === n ? xs : (xs = plotXs(p, n || n0));
      let color, group = null;
      if (p.voltammogram) color = alpha(colors().ink, series.length > 1 ? 0.3 + 0.7 * i / (series.length - 1) : 1); // cycles: first faint, last ink
      else if (s.palette != null) color = seriesColor(s.palette); // a base or dye's colour (electrophoresis)
      else if (groups) { group = groups.names[i]; color = seriesColor(groups.distinct.indexOf(group)); }
      else if (series.length > 8) color = colors().ink;
      else color = seriesColor(i);
      return { name: s.name, unit: s.unit, color, group, x, y: s.y, lo: s.lo, hi: s.hi, style: s.style || (p.kind === "spectrum" ? "profile" : "line"), channel: s.channel, index: i };
    });
    if (!overlay) {
      for (const s of prepared) panels.push({ series: [s], label: s.name, unit: s.unit });
    } else {
      const unit = p.y_unit || (prepared[0] && prepared[0].unit);
      panels.push({ series: prepared, label: "", unit: unit || (p.kind === "spectrum" ? "intensity" : p.kind === "nmr" ? "" : "") });
    }
    const ax = p.x || {};
    return { panels, ax, reversed: !!ax.reversed, kind: p.kind, groups, many: series.length > 8 };
  }

  function drawPlot() {
    const m = plotModel();
    const c = colors();
    const p = S.data.plot;
    const np = Math.max(1, m.panels.length);
    const [w, h] = sizeCanvas(null, np > 1 ? np * 86 + 48 : 0);
    g.clearRect(0, 0, w, h);
    // x extent over every series
    let x0 = Infinity, x1 = -Infinity;
    for (const pn of m.panels) for (const s of pn.series) for (const v of s.x) if (isFinite(v)) { if (v < x0) x0 = v; if (v > x1) x1 = v; }
    const xr = p.x_range;
    if (xr && (m.kind === "chromatogram" || m.kind === "spectrum")) { x0 = Math.min(xr[0], xr[1]); x1 = Math.max(xr[0], xr[1]); }
    if (!isFinite(x0)) { x0 = 0; x1 = 1; }
    if (x1 === x0) { x0 -= 0.5; x1 += 0.5; }
    // y extents first, to size the left margin from the tick labels
    const yinfo = m.panels.map((pn) => {
      let y0 = Infinity, y1 = -Infinity;
      for (const s of pn.series) for (const arr of [s.y, s.lo, s.hi]) if (arr) for (const v of arr) if (v != null && isFinite(v)) { if (v < y0) y0 = v; if (v > y1) y1 = v; }
      if (m.kind === "spectrum" || m.kind === "chromatogram") y0 = Math.min(0, y0);
      if (!isFinite(y0)) { y0 = 0; y1 = 1; }
      if (y1 === y0) { y0 -= 1; y1 += 1; }
      const pad = (y1 - y0) * 0.07;
      y1 += pad;
      if (y0 !== 0) y0 -= pad;
      return { y0, y1 };
    });
    const top = np > 1 ? 18 : 24, bottom = 30, gap = np > 1 ? 18 : 0;
    const ph = (h - top - bottom - gap * (np - 1)) / np;
    g.font = font(10.5, 400, true);
    let tickW = 0;
    yinfo.forEach((yi) => { for (const l of tickLabels(niceTicks(yi.y0, yi.y1, Math.max(3, Math.floor(ph / 42))))) tickW = Math.max(tickW, g.measureText(l).width); });
    const left = Math.ceil(tickW + 14);
    // room on the right for direct labels
    const labels = directLabels(m);
    g.font = font(11.5, 500);
    const labelW = labels.length ? Math.min(150, Math.max(...labels.map((l) => g.measureText(l.text).width)) + 12) : 0;
    const right = Math.max(14, labelW);
    const pw = w - left - right;
    const X = (v) => left + (m.reversed ? (x1 - v) : (v - x0)) / (x1 - x0) * pw;
    const panelsGeo = [];
    m.panels.forEach((pn, pi) => {
      const py = top + pi * (ph + gap);
      const { y0, y1 } = yinfo[pi];
      const Y = (v) => py + ph - (v - y0) / (y1 - y0) * ph;
      // quiet grid and y ticks
      const yt = niceTicks(y0, y1, Math.max(3, Math.floor(ph / 42)));
      const ytl = tickLabels(yt);
      g.lineWidth = 1;
      for (const t of yt) {
        const yy = Math.round(Y(t)) + 0.5;
        g.strokeStyle = t === 0 ? c.rule : c.grid;
        g.beginPath(); g.moveTo(left, yy); g.lineTo(left + pw, yy); g.stroke();
        txt(ytl[yt.indexOf(t)], left - 7, yy, { mono: true, size: 10.5, align: "right", baseline: "middle" });
      }
      // panel label (channel name) and y unit, top left
      const head = [pn.label, pn.unit && pn.label !== pn.unit ? pn.unit : ""].filter(Boolean).join("  ·  ") || pn.unit || "";
      if (head) txt(legendLabel(head, 60), left, py - 7, { size: 11, color: c.muted });
      // annotations under the data
      if (pi === 0) drawPlotAnnotations(X, Y, py, ph, left, pw, "under");
      g.save();
      g.beginPath();
      g.rect(left, py - 1, pw, ph + 2);
      g.clip();
      // chromatogram peaks: labelled with their time, at the trace's own top (the picker's
      // height is above its baseline)
      const onTrace = (x) => {
        const s0 = pn.series[0];
        if (!s0 || !s0.x.length) return null;
        let best = -1, bd = Infinity;
        for (let k = 0; k < s0.x.length; k++) { const d = Math.abs(s0.x[k] - x); if (d < bd) { bd = d; best = k; } }
        const ys = s0.y || s0.hi;
        let top = -Infinity;
        for (let k = Math.max(0, best - 2); k <= Math.min(ys.length - 1, best + 2); k++) if (isFinite(ys[k]) && ys[k] > top) top = ys[k];
        return isFinite(top) ? top : null;
      };
      const peaks = (p.peaks || []).map((q) => (m.kind === "chromatogram" ? Object.assign({}, q, { number: q.label, label: Number(q.x).toFixed(2), height: onTrace(q.x) ?? q.height }) : q));
      for (const s of pn.series) drawSeries(s, X, Y, py + ph, m);
      g.restore();
      // only prominent peaks are labelled (the server says which); every peak can be hovered
      const avoid = pi === 0 && labels.length ? directLabelZones(directLabelLayout(labels, { py, ph, Y }, left + pw), left + pw) : [];
      if (pi === 0 && peaks.length) drawPeakLabels(peaks.filter((q) => q.labelled !== false), X, Y, py, left, pw, m.kind === "chromatogram" ? 10 : 14, avoid);
      if (pi === 0 && m.kind === "spectrum" && pn.series[0]) {
        if (!pn.series[0].x.length) txt("This scan holds no peaks.", left + pw / 2, py + ph / 2, { size: 13, color: c.muted, align: "center" });
        else drawPeakLabels(spectrumPeaks(pn.series[0], 12), X, Y, py, left, pw, 12, avoid);
      }
      if (pi === 0) drawPlotAnnotations(X, Y, py, ph, left, pw, "over");
      panelsGeo.push({ py, ph, y0, y1, Y, series: pn.series });
    });
    // x axis: one rule, mono tick labels, the unit after the last one
    const yAxis = top + np * ph + (np - 1) * gap;
    g.strokeStyle = c.rule;
    g.lineWidth = 1;
    g.beginPath(); g.moveTo(left, yAxis + 0.5); g.lineTo(left + pw, yAxis + 0.5); g.stroke();
    const xt = niceTicks(x0, x1, Math.max(2, Math.floor(pw / 90)));
    const xtl = tickLabels(xt);
    const unit = m.ax.unit || (m.kind === "spectrum" ? "m/z" : m.ax.quantity === "sample" ? "samples" : String(m.ax.quantity || "").replace(/_/g, " "));
    xt.forEach((t, i) => {
      const xx = Math.round(X(t)) + 0.5;
      g.beginPath(); g.moveTo(xx, yAxis); g.lineTo(xx, yAxis + 4); g.stroke();
      const last = m.reversed ? i === 0 : i === xt.length - 1;
      const label = xtl[i] + (last && unit ? " " + unit : "");
      const align = last ? (xx + g.measureText(label).width / 2 > w - 4 ? "right" : "center") : "center";
      txt(label, align === "right" ? Math.min(xx + 6, w - 4) : xx, yAxis + 17, { mono: true, size: 10.5, align });
    });
    // direct labels at the right end of each line (or group)
    drawDirectLabels(labels, X, panelsGeo, left + pw, w);
    S.layout = { kind: "plot", left, pw, top, x0, x1, X, reversed: m.reversed, panels: panelsGeo, bottom: yAxis, w, h, model: m };
  }

  /** One label per series (up to 8) or per group of many curves, at their right ends. */
  function directLabels(m) {
    const pn = m.panels[0];
    if (!pn || m.panels.length > 1) return [];
    if (m.groups) {
      return m.groups.distinct.map((name, i) => ({ text: legendLabel(name, 22), color: seriesColor(i), members: pn.series.filter((s) => s.group === name) }));
    }
    // cycles of a voltammogram are loops: their ends are not where a label belongs
    if (S.data.plot && S.data.plot.voltammogram) return [];
    if (pn.series.length < 2 || pn.series.length > 8) return [];
    return pn.series.map((s) => ({ text: legendLabel(s.name, 22), color: s.color, members: [s] }));
  }

  /** Where the direct labels go: each label's line end (`y`) and its spread place (`ty`), and the
   * text's left edge. Peak labels keep off these places (one set of labels, as in the figure
   * engine). */
  function directLabelLayout(labels, P, xEnd) {
    const items = labels.map((l) => {
      // the last finite point of the member with the highest end
      let best = null;
      for (const s of l.members) {
        const arr = s.y || s.hi;
        for (let k = arr.length - 1; k >= 0; k--) {
          if (arr[k] != null && isFinite(arr[k]) && isFinite(s.x[k])) {
            if (!best || arr[k] > best.v) best = { v: arr[k], x: s.x[k] };
            break;
          }
        }
      }
      return best ? { l, y: P.Y(best.v) } : null;
    }).filter(Boolean).sort((a, b) => a.y - b.y);
    // spread apart vertically, as the figure engine does (end_labels): one line of type apart,
    // as close to each line's end as the others allow
    const hgt = 15;
    const ys = spreadLabels(items.map((it) => it.y), hgt, P.py, P.py + P.ph);
    const moved = items.some((it, i) => Math.abs(ys[i] - it.y) > 3);
    items.forEach((it, i) => { it.ty = ys[i]; });
    return { items, moved, tx: xEnd + (moved ? 18 : 6) };
  }

  /** The places the direct labels take inside the plot: the last stretch of each labelled line. */
  function directLabelZones(lay, xEnd) {
    return lay.items.map((it) => ({ x: xEnd - 28, y: Math.min(it.y, it.ty) - 9, w: 40, h: Math.abs(it.y - it.ty) + 18 }));
  }

  function drawDirectLabels(labels, X, panels, xEnd, w) {
    if (!labels.length || !panels[0]) return;
    const { items, moved, tx } = directLabelLayout(labels, panels[0], xEnd);
    const ys = items.map((it) => it.ty);
    const c = colors();
    items.forEach((it, i) => {
      if (moved) {
        // an elbow leader from the line's end to its label
        g.strokeStyle = alpha(it.l.color, 0.55);
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(xEnd + 2, it.y);
        g.lineTo(xEnd + 5, it.y);
        g.lineTo(tx - 5, ys[i]);
        g.lineTo(tx - 2, ys[i]);
        g.stroke();
      }
      txt(it.l.text, tx, ys[i], { size: 11.5, weight: 500, color: it.l.color === c.faint ? c.muted : it.l.color, baseline: "middle" });
    });
  }

  function drawSeries(s, X, Y, baseY, m) {
    const many = m && m.many;
    g.strokeStyle = s.color;
    g.fillStyle = s.color;
    g.lineWidth = many ? 1 : 1.5;
    g.lineJoin = "round";
    g.globalAlpha = many ? (m.groups ? 0.55 : 0.4) : 1;
    const n = s.x.length;
    if (s.style === "sticks") {
      g.lineWidth = 1.25;
      g.beginPath();
      for (let k = 0; k < n; k++) {
        const v = s.y[k];
        if (v == null || !isFinite(s.x[k])) continue;
        const xx = Math.round(X(s.x[k])) + 0.5;
        g.moveTo(xx, Y(Math.max(0, Math.min(v, 0))));
        g.lineTo(xx, Y(v));
      }
      g.stroke();
      g.globalAlpha = 1;
      return;
    }
    if (s.lo && s.hi) {
      // min/max envelope: the band between the extremes of each slice
      g.beginPath();
      const flush = (from, to) => {
        if (to < from) return;
        g.moveTo(X(s.x[from]), Y(s.hi[from]));
        for (let k = from + 1; k <= to; k++) g.lineTo(X(s.x[k]), Y(s.hi[k]));
        for (let k = to; k >= from; k--) g.lineTo(X(s.x[k]), Y(s.lo[k]));
        g.closePath();
      };
      let start = -1;
      for (let k = 0; k <= n; k++) {
        const ok = k < n && s.lo[k] != null && s.hi[k] != null && isFinite(s.x[k]);
        if (ok && start < 0) start = k;
        if (!ok && start >= 0) { flush(start, k - 1); start = -1; }
      }
      g.fill();
      g.lineWidth = 1;
      g.stroke();
      g.globalAlpha = 1;
      return;
    }
    g.beginPath();
    let pen = false;
    for (let k = 0; k < n; k++) {
      const v = s.y[k];
      if (v == null || !isFinite(s.x[k])) { pen = false; continue; }
      const xx = X(s.x[k]), yy = Y(v);
      if (pen) g.lineTo(xx, yy);
      else g.moveTo(xx, yy);
      pen = true;
    }
    g.stroke();
    g.globalAlpha = 1;
  }

  /** The tallest local maxima of a spectrum, labelled with their m/z. */
  function spectrumPeaks(s, max) {
    const out = [];
    const y = s.y || [];
    for (let k = 0; k < y.length; k++) {
      const v = y[k];
      if (v == null || !(v > 0)) continue;
      if (s.style !== "sticks" && ((k > 0 && y[k - 1] > v) || (k + 1 < y.length && y[k + 1] > v))) continue;
      out.push({ x: s.x[k], height: v, label: fmt(s.x[k], 7) });
    }
    return out.sort((a, b) => b.height - a.height).slice(0, max);
  }

  function drawPeakLabels(peaks, X, Y, py, left, pw, max, avoid) {
    const c = colors();
    g.font = font(10.5, 400, true);
    const sorted = peaks.slice().sort((a, b) => (b.height || 0) - (a.height || 0));
    const used = [];
    let n = 0;
    for (const pk of sorted) {
      if (n >= max) break;
      const xx = X(pk.x);
      if (!isFinite(xx)) continue;
      const label = String(pk.label ?? (pk.x != null ? fmt(pk.x, 4) : ""));
      if (!label) continue;
      const half = g.measureText(label).width / 2 + 4;
      if (xx < left - 2 || xx > left + pw + 2) continue;
      const tx = Math.min(left + pw - half, Math.max(left + half, xx));
      if (used.some(([a, b]) => tx + half > a && tx - half < b)) continue;
      let yy = pk.height != null ? Math.max(py + 10, Y(pk.height) - 4) : py + 10;
      // off the direct labels' places: above them when there is room, else left out
      const hits = (avoid || []).filter((z) => tx + half > z.x && tx - half < z.x + z.w && yy > z.y && yy - 12 < z.y + z.h);
      if (hits.length) {
        yy = Math.min(...hits.map((z) => z.y)) - 3;
        if (yy - 12 < py) continue;
      }
      used.push([tx - half, tx + half]);
      txt(label, tx, yy, { mono: true, size: 10.5, align: "center", color: c.ink, halo: c.bg });
      n++;
    }
  }

  /** The agent's annotations on a plot, in axis units. `layer`: "under" or "over" the data. */
  function drawPlotAnnotations(X, Y, py, ph, left, pw, layer) {
    const A = S.ann;
    if (!A || !A.list.length) return;
    const c = colors();
    const inX = (x) => Math.min(left + pw, Math.max(left, x));
    for (const a of A.list) {
      const col = a.color || c.accent;
      if (layer === "under" && a.kind === "band") {
        const xa = inX(X(a.x0)), xb = inX(X(a.x1));
        const ya = a.y1 != null ? Y(a.y1) : py, yb = a.y0 != null ? Y(a.y0) : py + ph;
        g.fillStyle = a.color ? alpha(a.color, 0.14) : c.band;
        g.fillRect(Math.min(xa, xb), ya, Math.abs(xb - xa), yb - ya);
      }
      if (layer !== "over") continue;
      g.strokeStyle = col;
      g.fillStyle = col;
      g.lineWidth = 1.25;
      if (a.kind === "band" && a.label) {
        const xa = Math.min(X(a.x0), X(a.x1));
        txt(a.label, Math.max(left + 2, xa) + 4, py + 13, { size: 11.5, weight: 500, color: col, halo: c.bg });
      } else if (a.kind === "vline") {
        const x = X(a.x);
        g.setLineDash([4, 3]);
        g.beginPath(); g.moveTo(x, py); g.lineTo(x, py + ph); g.stroke();
        g.setLineDash([]);
        if (a.label) txt(a.label, x + 5, py + 13, { size: 11.5, weight: 500, color: col, halo: c.bg });
      } else if (a.kind === "hline") {
        const y = Y(a.y);
        g.setLineDash([4, 3]);
        g.beginPath(); g.moveTo(left, y); g.lineTo(left + pw, y); g.stroke();
        g.setLineDash([]);
        if (a.label) txt(a.label, left + pw - 4, y - 5, { size: 11.5, weight: 500, color: col, align: "right", halo: c.bg });
      } else if (a.kind === "point" || a.kind === "text") {
        const x = X(a.x), y = Y(a.y);
        if (a.kind === "point") {
          g.beginPath();
          g.arc(x, y, 4, 0, Math.PI * 2);
          g.fill();
        }
        if (a.label) txt(a.label, x + 7, y - 6, { size: 11.5, weight: 500, color: col, halo: c.bg });
      } else if (a.kind === "line") {
        g.beginPath(); g.moveTo(X(a.x0), Y(a.y0)); g.lineTo(X(a.x1), Y(a.y1)); g.stroke();
        if (a.arrow) arrowHead(X(a.x0), Y(a.y0), X(a.x1), Y(a.y1));
        if (a.label) txt(a.label, X(a.x0) + 5, Y(a.y0) - 6, { size: 11.5, weight: 500, color: col, halo: c.bg });
      } else if (a.kind === "box") {
        g.strokeRect(X(a.x), Y(a.y + a.h), X(a.x + a.w) - X(a.x), Y(a.y) - Y(a.y + a.h));
        if (a.label) txt(a.label, X(a.x) + 4, Y(a.y + a.h) - 5, { size: 11.5, weight: 500, color: col, halo: c.bg });
      }
    }
  }

  function plotOverlays(L) {
    const c = colors();
    const P = L.panels;
    if (!P.length) return;
    const sel = S.sel;
    if (sel && (sel.kind === "xrange")) {
      const xa = L.X(sel.x0), xb = L.X(sel.x1);
      g.fillStyle = c.band;
      g.fillRect(Math.min(xa, xb), L.top, Math.abs(xb - xa), L.bottom - L.top);
      g.strokeStyle = c.accent;
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(xa, L.top); g.lineTo(xa, L.bottom);
      g.moveTo(xb, L.top); g.lineTo(xb, L.bottom);
      g.stroke();
    }
    if (sel && (sel.kind === "xpoint" || sel.kind === "mz")) {
      const x = L.X(sel.kind === "mz" ? sel.mz : sel.x);
      g.strokeStyle = c.accent;
      g.lineWidth = 1.5;
      g.beginPath(); g.moveTo(x, L.top); g.lineTo(x, L.bottom); g.stroke();
    }
    const pk = (S.hover && S.hover.peak) || (sel && sel.kind === "peak" && sel.peak);
    if (pk && P[0].series[0]) shadePeak(pk, L, P[0]);
    const hv = S.hover;
    if (!hv || hv.px == null) return;
    // crosshair and the value of each series at the pointer
    const x = hv.px;
    g.strokeStyle = alpha(c.ink, 0.25);
    g.lineWidth = 1;
    g.beginPath(); g.moveTo(Math.round(x) + 0.5, L.top); g.lineTo(Math.round(x) + 0.5, L.bottom); g.stroke();
    if (hv.curve) {
      // many curves: the one under the pointer stands out
      const s = hv.curve;
      g.save();
      g.beginPath(); g.rect(L.left, P[0].py, L.pw, P[0].ph); g.clip();
      drawSeries(Object.assign({}, s, { color: c.accent }), L.X, P[0].Y, undefined, { many: false });
      g.restore();
    }
    for (const dot of hv.dots || []) {
      g.beginPath();
      g.arc(dot.x, dot.y, 3.5, 0, Math.PI * 2);
      g.fillStyle = c.bg;
      g.fill();
      g.strokeStyle = dot.color;
      g.lineWidth = 1.75;
      g.stroke();
    }
  }

  function shadePeak(pk, L, P) {
    const c = colors();
    const s = P.series[0];
    if (pk.from == null || pk.to == null || !s.y) return;
    const a = Math.min(pk.from, pk.to), b = Math.max(pk.from, pk.to);
    g.save();
    g.beginPath(); g.rect(L.left, P.py, L.pw, P.ph); g.clip();
    g.beginPath();
    let started = false;
    let lastX = null;
    for (let k = 0; k < s.x.length; k++) {
      const xv = s.x[k];
      if (!(xv >= a && xv <= b) || s.y[k] == null) continue;
      const X = L.X(xv), Y = P.Y(s.y[k]);
      if (!started) { g.moveTo(X, P.Y(0)); started = true; }
      g.lineTo(X, Y);
      lastX = X;
    }
    if (started) {
      g.lineTo(lastX, P.Y(0));
      g.closePath();
      g.fillStyle = alpha(c.accent, 0.22);
      g.fill();
    }
    g.restore();
  }

  function plotInv(L, px) {
    return L.reversed ? L.x1 - (px - L.left) / L.pw * (L.x1 - L.x0) : L.x0 + (px - L.left) / L.pw * (L.x1 - L.x0);
  }

  function zoomPlot(xa, xb) {
    const p = S.data.plot;
    if (p.kind === "trace" || p.kind === "nmr") {
      const win = sampleWindow(p, xa, xb);
      if (win) load(win, { keepZoom: true });
    } else {
      load({ x_range: [Math.min(xa, xb), Math.max(xa, xb)] }, { keepZoom: true });
    }
  }

  // ---------- plates: heat map, and a grid of growth curves for kinetic reads

  function setupKinetic(pl) {
    const k = pl.kinetic;
    if (pl.groups && S.kin.colorBy === undefined) S.kin.colorBy = defaultGroupMap(pl.groups);
    if (S.kin.colorBy && !(pl.groups || {})[S.kin.colorBy]) S.kin.colorBy = defaultGroupMap(pl.groups);
    if (!k) { S.kin.mode = "heat"; return; }
    if (!k.v) {
      k.v = f32FromBase64(k.values);
      k.n = k.time_s.length;
      k.index = new Map(k.wells.map(([r, c], i) => [r * 1000 + c, i]));
      // a shared y scale for every small curve: robust range of all values
      const vals = Array.from(k.v).filter(isFinite);
      vals.sort((a, b) => a - b);
      const q = (p) => vals[Math.min(vals.length - 1, Math.max(0, Math.round(p * (vals.length - 1))))];
      k.lo = Math.min(0, q(0.002));
      k.hi = q(0.998);
      if (!(k.hi > k.lo)) k.hi = k.lo + 1;
    }
    if (S.kin.t < 0 || S.kin.t >= k.n) S.kin.t = k.n - 1;
    if (S.kin.mode !== "heat") S.kin.mode = "curves";
  }

  function kinTime() {
    const k = S.data && S.data.plate && S.data.plate.kinetic;
    return k ? k.time_s[Math.max(0, S.kin.t)] : NaN;
  }

  function fmtTime(s) {
    const k = S.data && S.data.plate && S.data.plate.kinetic;
    if (!isFinite(s)) return "";
    const span = k ? k.time_s[k.n - 1] : s;
    const u = timeUnit(span);
    return `${fmt(s / u.k, u.unit === "s" ? 4 : 3)} ${u.unit}`;
  }

  function wellSeries(r, c) {
    const k = S.data.plate.kinetic;
    const i = k.index.get(r * 1000 + c);
    return i == null ? null : k.v.subarray(i * k.n, (i + 1) * k.n);
  }

  /** A well's value as shown: at the scrubbed time for kinetic plates, else the plate value. */
  function wellValue([r, c]) {
    const pl = S.data.plate;
    if (pl.kinetic) {
      const s = wellSeries(r, c);
      if (!s) return null;
      const v = s[Math.max(0, S.kin.t)];
      return isFinite(v) ? v : null;
    }
    const hit = pl.cells.find(([a, b]) => a === r && b === c);
    return hit ? hit[2] : null;
  }

  /** Group of each well under the colour map, with a colour per group. */
  function groupColoring() {
    const pl = S.data.plate;
    const map = S.kin.colorBy && pl.groups ? pl.groups[S.kin.colorBy] : null;
    if (!map) return null;
    const order = [];
    for (let r = 0; r < pl.rows; r++) for (let c = 0; c < pl.columns; c++) {
      const v = map[wellName(r, c)];
      if (v != null && !order.includes(v)) order.push(v);
    }
    // the shared category rule, so the plate looks as it does in previews and plots
    const cols = categoryColors(order, colors());
    const colorsBy = new Map(order.map((name, i) => [name, cols[i]]));
    const counts = new Map();
    for (const v of Object.values(map)) counts.set(v, (counts.get(v) || 0) + 1);
    return { map, order, colorsBy, counts };
  }

  function groupChips() {
    const pl = S.data.plate;
    const gc = groupColoring();
    if (!gc || gc.order.length > 24) return null;
    const chips = el("span", { className: "chips" });
    for (const name of gc.order) {
      const on = !S.kin.focus || S.kin.focus === name;
      const chip = el("button", { type: "button", className: "chip", title: `${S.kin.colorBy}: ${name}` },
        el("span", { className: "swatch", style: `background:${gc.colorsBy.get(name)}` }), legendLabel(name, 22), el("span", { className: "n", textContent: String(gc.counts.get(name) || 0) }));
      chip.setAttribute("aria-pressed", String(on));
      chip.addEventListener("click", () => {
        S.kin.focus = S.kin.focus === name ? null : name;
        renderControls();
        invalidate();
      });
      chip.addEventListener("dblclick", () => {
        // select every well of the group
        const wells = [];
        for (let r = 0; r < pl.rows; r++) for (let c = 0; c < pl.columns; c++) if (gc.map[wellName(r, c)] === name) wells.push([r, c]);
        if (wells.length) showAct({ kind: "wells", wells }, actAnchorForWells(wells));
      });
      chips.append(chip);
    }
    return chips;
  }

  function drawPlate() {
    const pl = S.data.plate;
    const k = pl.kinetic;
    const curves = k && S.kin.mode === "curves";
    const c = colors();
    const gc = groupColoring();
    // When each block of rows holds one group, the group names head the blocks on the left,
    // as a plate layout is drawn on paper.
    const blocks = gc ? rowBlocks(pl, gc) : null;
    const boxW = Math.max(160, canvas.parentElement.clientWidth);
    let headW = 0;
    if (blocks) {
      g.font = font(12, 500);
      headW = Math.min(boxW * 0.22, Math.max(...blocks.map((b) => g.measureText(b.name).width)) + 14);
      if (headW < 30 || boxW < 420) { headW = 0; }
    }
    const left = 22 + headW, top = 20, bottom = curves ? 18 : 40, right = 6;
    const cellW = Math.max(6, Math.min(curves ? 96 : 56, (boxW - left - right) / pl.columns));
    const cellH = curves ? cellW * 0.7 : cellW;
    const [w, h] = sizeCanvas(root.classList.contains("fill") ? null : boxW / (top + pl.rows * cellH + bottom));
    g.clearRect(0, 0, w, h);
    const fit = Math.min(cellW, (w - left - right) / pl.columns, (h - top - bottom) / pl.rows / (curves ? 0.7 : 1));
    const cw = fit, ch = curves ? fit * 0.7 : fit;
    const gw = cw * pl.columns;
    const ox = left + Math.max(0, (w - left - right - gw) / 2), oy = top;
    // row and column labels
    const every = cw < 14 ? Math.ceil(14 / cw) : 1;
    for (let col = 0; col < pl.columns; col++) if ((col + 1) % every === 0 || col === 0) txt(String(col + 1), ox + (col + 0.5) * cw, top - 7, { mono: true, size: 10, align: "center" });
    for (let r = 0; r < pl.rows; r++) if (r % (ch < 12 ? Math.ceil(12 / ch) : 1) === 0) txt(wellName(r, 0).replace(/\d+$/, ""), ox - 8, oy + (r + 0.5) * ch, { mono: true, size: 10, align: "center", baseline: "middle" });
    if (headW) {
      for (const b of blocks) {
        const y0 = oy + b.from * ch, y1 = oy + (b.to + 1) * ch;
        const words = legendLabel(b.name, 28);
        txt(words, ox - 18, (y0 + y1) / 2, { size: 12, weight: 500, color: gc.colorsBy.get(b.name) === c.faint ? c.muted : c.ink, align: "right", baseline: "middle" });
        g.strokeStyle = c.rule;
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(ox - 14.5, y0 + 3);
        g.lineTo(ox - 14.5, y1 - 3);
        g.stroke();
      }
    }
    const focus = S.kin.focus;
    const dim = (r, col) => focus && gc && gc.map[wellName(r, col)] !== focus;
    if (curves) {
      const pad = Math.max(1, Math.min(3, cw * 0.06));
      for (let r = 0; r < pl.rows; r++) for (let col = 0; col < pl.columns; col++) {
        const x = ox + col * cw + pad / 2, y = oy + r * ch + pad / 2, ww = cw - pad, hh = ch - pad;
        g.fillStyle = c.panel;
        g.fillRect(x, y, ww, hh);
        const s = wellSeries(r, col);
        if (!s) continue;
        const name = wellName(r, col);
        const color = gc ? (gc.colorsBy.get(gc.map[name]) || c.ink) : c.ink;
        g.globalAlpha = dim(r, col) ? 0.18 : 1;
        miniCurve(s, x + 2, y + 2, ww - 4, hh - 4, color, cw > 30 ? 1.25 : 1);
        g.globalAlpha = 1;
      }
      // the scrubbed time: one thin line through every card
      if (S.kin.t < k.n - 1) {
        const u = S.kin.t / Math.max(1, k.n - 1);
        g.strokeStyle = alpha(c.accent, 0.55);
        g.lineWidth = 1;
        g.beginPath();
        for (let col = 0; col < pl.columns; col++) {
          const xx = Math.round(ox + col * cw + pad / 2 + 2 + u * (cw - pad - 4)) + 0.5;
          g.moveTo(xx, oy);
          g.lineTo(xx, oy + pl.rows * ch);
        }
        g.stroke();
      }
      const span = `${fmtTime(kinTime())} of ${fmtTime(k.time_s[k.n - 1])}` + (k.unit ? ` · ${k.unit}` : "") + (k.wavelength_nm ? ` at ${fmt(k.wavelength_nm, 4)} nm` : "");
      txt(span, ox, oy + pl.rows * ch + 13, { size: 11, color: c.muted });
      const scale = `${fmt(k.lo, 3)} – ${fmt(k.hi, 3)}${k.unit ? " " + k.unit : ""} in every well`;
      g.font = font(11, 400);
      const used = g.measureText(span).width;
      g.font = font(11, 400, true);
      if (used + g.measureText(scale).width + 24 < gw) txt(scale, ox + gw, oy + pl.rows * ch + 13, { size: 11, color: c.faintText, align: "right", mono: true });
    } else {
      // heat map: a value per well (the scrubbed time for kinetic plates)
      let lo = Infinity, hi = -Infinity;
      const vals = [];
      for (let r = 0; r < pl.rows; r++) for (let col = 0; col < pl.columns; col++) {
        const v = wellValue([r, col]);
        if (v != null && isFinite(v)) { vals.push([r, col, v]); lo = Math.min(lo, v); hi = Math.max(hi, v); }
      }
      if (k) { lo = k.lo; hi = k.hi; }
      const rad = Math.max(2, cw / 2 - Math.max(1, cw * 0.08));
      for (let r = 0; r < pl.rows; r++) for (let col = 0; col < pl.columns; col++) {
        g.beginPath();
        g.arc(ox + (col + 0.5) * cw, oy + (r + 0.5) * ch, rad, 0, Math.PI * 2);
        g.strokeStyle = c.grid;
        g.lineWidth = 1;
        g.stroke();
      }
      for (const [r, col, v] of vals) {
        const t = hi > lo ? (v - lo) / (hi - lo) : 0.5;
        const [R, G, B] = ramp(c.seq, t);
        g.beginPath();
        g.arc(ox + (col + 0.5) * cw, oy + (r + 0.5) * ch, rad, 0, Math.PI * 2);
        g.fillStyle = `rgb(${R},${G},${B})`;
        g.globalAlpha = dim(r, col) ? 0.2 : 1;
        g.fill();
        g.globalAlpha = 1;
        if (gc) {
          const gcol = gc.colorsBy.get(gc.map[wellName(r, col)]);
          if (gcol && cw > 18) { g.strokeStyle = gcol; g.lineWidth = 2; g.beginPath(); g.arc(ox + (col + 0.5) * cw, oy + (r + 0.5) * ch, rad + 0.5, 0, Math.PI * 2); g.stroke(); }
        }
      }
      // colour scale
      const ly = oy + pl.rows * ch + 12;
      const lw = Math.min(220, gw * 0.5);
      for (let i = 0; i < lw; i++) {
        const [R, G, B] = ramp(c.seq, i / (lw - 1));
        g.fillStyle = `rgb(${R},${G},${B})`;
        g.fillRect(ox + i, ly, 1, 7);
      }
      const unit = k ? k.unit || "" : "";
      txt(fmt(lo, 4), ox, ly + 20, { mono: true, size: 10.5 });
      txt(fmt(hi, 4) + (unit ? " " + unit : ""), ox + lw, ly + 20, { mono: true, size: 10.5, align: "right" });
      txt(k ? `at ${fmtTime(kinTime())}` : legendLabel(pl.value, 60), ox + lw + 14, ly + 7, { size: 11, color: c.muted });
    }
    S.layout = { kind: "plate", ox, oy, cw, ch, rows: pl.rows, columns: pl.columns, w, h, curves };
  }

  /** Runs of rows whose non-blank wells all share one group: [{name, from, to}], or null. */
  function rowBlocks(pl, gc) {
    const rowGroup = [];
    for (let r = 0; r < pl.rows; r++) {
      const names = new Set();
      for (let c = 0; c < pl.columns; c++) {
        const v = gc.map[wellName(r, c)];
        if (v != null && !isBlankGroup(v)) names.add(v);
      }
      if (names.size > 1) return null;
      rowGroup.push(names.size ? [...names][0] : null);
    }
    const out = [];
    for (let r = 0; r < pl.rows; r++) {
      const name = rowGroup[r];
      if (name == null) continue;
      const last = out[out.length - 1];
      if (last && last.name === name && last.to === r - 1) last.to = r;
      else out.push({ name, from: r, to: r });
    }
    if (out.length < 2 || out.length > 8 || new Set(out.map((b) => b.name)).size !== out.length) return null;
    return out;
  }

  function miniCurve(s, x, y, w, h, color, lw) {
    const k = S.data.plate.kinetic;
    const n = s.length;
    g.strokeStyle = color;
    g.lineWidth = lw;
    g.lineJoin = "round";
    g.beginPath();
    let pen = false;
    for (let j = 0; j < n; j++) {
      const v = s[j];
      if (!isFinite(v)) { pen = false; continue; }
      const xx = x + (j / Math.max(1, n - 1)) * w;
      const yy = y + h - Math.min(1.05, Math.max(-0.05, (v - k.lo) / (k.hi - k.lo))) * h;
      if (pen) g.lineTo(xx, yy);
      else g.moveTo(xx, yy);
      pen = true;
    }
    g.stroke();
  }

  function wellAt(L, px, py) {
    const col = Math.floor((px - L.ox) / L.cw), r = Math.floor((py - L.oy) / L.ch);
    if (col < 0 || r < 0 || col >= L.columns || r >= L.rows) return null;
    return [r, col];
  }

  function actAnchorForWells(wells) {
    const L = S.layout;
    if (!L || L.kind !== "plate") return null;
    const rs = wells.map((w) => w[0]), cs = wells.map((w) => w[1]);
    const x0 = L.ox + Math.min(...cs) * L.cw, y0 = L.oy + Math.min(...rs) * L.ch;
    return { x: x0, y: y0, w: (Math.max(...cs) + 1) * L.cw + L.ox - x0, h: (Math.max(...rs) + 1) * L.ch + L.oy - y0 };
  }

  function plateOverlays(L) {
    const c = colors();
    const sel = S.sel;
    const ring = (r, col, color, lw) => {
      g.strokeStyle = color;
      g.lineWidth = lw;
      if (L.curves) g.strokeRect(L.ox + col * L.cw + 0.75, L.oy + r * L.ch + 0.75, L.cw - 1.5, L.ch - 1.5);
      else { g.beginPath(); g.arc(L.ox + (col + 0.5) * L.cw, L.oy + (r + 0.5) * L.ch, L.cw / 2 - 0.5, 0, Math.PI * 2); g.stroke(); }
    };
    // the agent's well annotations
    if (S.ann) for (const a of S.ann.list) if (a.kind === "wells") for (const [r, col] of a.wells) ring(r, col, a.color || c.accent, 2);
    if (sel && sel.kind === "wells") for (const [r, col] of sel.wells) ring(r, col, c.accent, 2);
    const hv = S.hover;
    if (hv && hv.well) {
      ring(hv.well[0], hv.well[1], c.ink, 1.5);
      if (S.data.plate.kinetic) wellPopover(L, hv.well);
    }
  }

  /** The hovered well's curve, large, with the other wells of its group behind it. */
  function wellPopover(L, [r, col]) {
    const c = colors();
    const pl = S.data.plate;
    const k = pl.kinetic;
    const s = wellSeries(r, col);
    if (!s) return;
    const gc = groupColoring();
    const name = wellName(r, col);
    const group = gc ? gc.map[name] : null;
    const color = gc && group != null ? gc.colorsBy.get(group) : c.ink;
    const W = Math.min(300, L.w - 16), H = 176;
    const cx = L.ox + (col + 0.5) * L.cw, cy = L.oy + (r + 0.5) * L.ch;
    let x = cx + L.cw * 0.7, y = cy - H / 2;
    if (x + W > L.w - 6) x = cx - L.cw * 0.7 - W;
    if (x < 6) x = Math.max(6, Math.min(L.w - W - 6, cx - W / 2));
    y = Math.max(6, Math.min(L.h - H - 6, y));
    g.save();
    g.shadowColor = "rgba(0,0,0,0.18)";
    g.shadowBlur = 16;
    g.shadowOffsetY = 3;
    g.fillStyle = c.bg;
    roundRect(x, y, W, H, 8);
    g.fill();
    g.restore();
    g.strokeStyle = c.rule;
    g.lineWidth = 1;
    roundRect(x + 0.5, y + 0.5, W - 1, H - 1, 8);
    g.stroke();
    const sample = pl.groups && pl.groups.Sample ? pl.groups.Sample[name] : null;
    txt(name, x + 12, y + 20, { size: 13, weight: 600, color: c.ink });
    g.font = font(13, 600);
    const nw = g.measureText(name).width;
    const sub = [group, sample && sample !== group ? sample : null].filter(Boolean).join(" · ");
    if (sub) txt(legendLabel(sub, 34), x + 20 + nw, y + 20, { size: 11.5, weight: 500, color });
    const v = s[Math.max(0, S.kin.t)];
    txt(`${fmt(v, 4)}${k.unit ? " " + k.unit : ""} at ${fmtTime(kinTime())}`, x + 12, y + 37, { size: 11, mono: true, color: c.muted });
    const px = x + 12, py = y + 48, pw = W - 24, ph = H - 70;
    // the rest of the group, faint
    if (gc && group != null) {
      for (const [wr, wc] of k.wells) {
        if (wr === r && wc === col) continue;
        if (gc.map[wellName(wr, wc)] !== group) continue;
        g.globalAlpha = 0.28;
        miniCurve(wellSeries(wr, wc), px, py, pw, ph, c.faint, 1);
        g.globalAlpha = 1;
      }
    }
    g.strokeStyle = c.rule;
    g.beginPath(); g.moveTo(px, py + ph + 0.5); g.lineTo(px + pw, py + ph + 0.5); g.stroke();
    miniCurve(s, px, py, pw, ph, color === c.faint ? c.ink : color, 1.75);
    const u = S.kin.t / Math.max(1, k.n - 1);
    g.strokeStyle = alpha(c.accent, 0.6);
    g.beginPath(); g.moveTo(px + u * pw, py); g.lineTo(px + u * pw, py + ph); g.stroke();
    if (isFinite(v)) {
      g.beginPath();
      g.arc(px + u * pw, py + ph - Math.min(1.05, Math.max(-0.05, (v - k.lo) / (k.hi - k.lo))) * ph, 3, 0, Math.PI * 2);
      g.fillStyle = c.accent;
      g.fill();
    }
    txt(fmt(k.hi, 3), px + pw, py - 3, { size: 10, mono: true, align: "right", color: c.faintText });
    txt("0", px, py + ph + 13, { size: 10, mono: true, color: c.faintText });
    txt(fmtTime(k.time_s[k.n - 1]), px + pw, py + ph + 13, { size: 10, mono: true, align: "right" });
  }

  let playTimer = 0;
  function togglePlay() {
    const k = S.data && S.data.plate && S.data.plate.kinetic;
    if (!k) return;
    S.kin.playing = !S.kin.playing;
    if (S.kin.playing && S.kin.t >= k.n - 1) S.kin.t = 0;
    const btn = $("kin-play");
    if (btn) btn.textContent = S.kin.playing ? "❚❚" : "▶";
    cancelAnimationFrame(playTimer);
    if (!S.kin.playing) { tellModel(); return; }
    const t0 = performance.now(), from = S.kin.t;
    const dur = 6000 * (1 - from / k.n);
    const step = (now) => {
      if (!S.kin.playing || !S.data || !S.data.plate) return;
      const u = Math.min(1, (now - t0) / Math.max(400, dur));
      S.kin.t = Math.round(from + u * (k.n - 1 - from));
      const r = $("kin-range"), o = $("kin-time");
      if (r) r.value = S.kin.t;
      if (o) o.textContent = fmtTime(kinTime());
      invalidate();
      if (u < 1) playTimer = requestAnimationFrame(step);
      else { S.kin.playing = false; const b = $("kin-play"); if (b) b.textContent = "▶"; tellModel(); }
    };
    playTimer = requestAnimationFrame(step);
  }

  // ---------- flow-cytometry events

  function drawEvents() {
    const ev = S.data.events;
    const c = colors();
    const [w, h] = sizeCanvas(null, 300);
    g.clearRect(0, 0, w, h);
    if (!ev.xv) {
      ev.xv = f32FromBase64(ev.xs);
      ev.yv = ev.ys ? f32FromBase64(ev.ys) : null;
      ev.bits = ev.gating ? bytesFromBase64(ev.gating.members) : null;
    }
    const gt = ev.gating;
    const left = 52, right = 14, top = 24, bottom = 34;
    const pw = w - left - right, ph = h - top - bottom;
    const sx = fcsScaleOf(ev.x.name);
    const fx = scaleFn(sx);
    const tx = Float64Array.from(ev.xv, fx);
    const xlo = percentile(tx, 0.2), xhi = percentile(tx, 99.8);
    const xr = xhi > xlo ? [xlo - (xhi - xlo) * 0.02, xhi + (xhi - xlo) * 0.03] : [xlo - 1, xlo + 1];
    const X = (v) => left + (v - xr[0]) / (xr[1] - xr[0]) * pw;
    let yr, Y, ty = null, sy = null;
    g.fillStyle = c.panel;
    g.fillRect(left, top, pw, ph);
    if (ev.yv) {
      sy = fcsScaleOf(ev.y.name);
      const fy = scaleFn(sy);
      ty = Float64Array.from(ev.yv, fy);
      const ylo = percentile(ty, 0.2), yhi = percentile(ty, 99.8);
      yr = yhi > ylo ? [ylo - (yhi - ylo) * 0.02, yhi + (yhi - ylo) * 0.03] : [ylo - 1, ylo + 1];
      Y = (v) => top + ph - (v - yr[0]) / (yr[1] - yr[0]) * ph;
      if (gt) drawGatedScatter(ev, tx, ty, X, Y, left, top, pw, ph);
      else drawDensity(tx, ty, xr, yr, left, top, pw, ph);
    } else {
      const bins = Math.max(32, Math.min(256, Math.floor(pw / 3)));
      const counts = new Uint32Array(bins);
      const groups = gt ? gt.children.length : 0;
      const byGroup = groups ? Array.from({ length: groups }, () => new Uint32Array(bins)) : [];
      tx.forEach((v, i) => {
        const u = (v - xr[0]) / (xr[1] - xr[0]);
        if (!(u >= 0 && u < 1)) return;
        const b = Math.floor(u * bins);
        counts[b]++;
        for (let j = 0; j < groups; j++) if (ev.bits[i] & (1 << j)) byGroup[j][b]++;
      });
      const max = Math.max(1, ...counts);
      yr = [0, max * 1.08];
      Y = (v) => top + ph - (v - yr[0]) / (yr[1] - yr[0]) * ph;
      const cw = pw / bins;
      g.fillStyle = alpha(c.ink, groups ? 0.25 : 0.8);
      for (let i = 0; i < bins; i++) if (counts[i]) g.fillRect(left + i * cw, Y(counts[i]), Math.max(1, cw - 0.5), top + ph - Y(counts[i]));
      byGroup.forEach((arr, j) => {
        g.fillStyle = alpha(popColor(j), 0.75);
        for (let i = 0; i < bins; i++) if (arr[i]) g.fillRect(left + i * cw, Y(arr[i]), Math.max(1, cw - 0.5), top + ph - Y(arr[i]));
      });
    }
    // axes
    const yt = ev.yv ? scaleTicks(sy, yr[0], yr[1], Math.max(2, Math.floor(ph / 44))) : scaleTicks("linear", yr[0], yr[1], Math.max(2, Math.floor(ph / 44)));
    for (const t of yt) txt(t.label, left - 7, Y(t.at), { mono: true, size: 10.5, align: "right", baseline: "middle" });
    const xt = scaleTicks(sx, xr[0], xr[1], Math.max(2, Math.floor(pw / 80)));
    g.strokeStyle = c.rule;
    g.lineWidth = 1;
    g.beginPath(); g.moveTo(left, top + ph + 0.5); g.lineTo(left + pw, top + ph + 0.5); g.stroke();
    for (const t of xt) {
      const xx = Math.round(X(t.at)) + 0.5;
      g.beginPath(); g.moveTo(xx, top + ph); g.lineTo(xx, top + ph + 4); g.stroke();
      txt(t.label, xx, top + ph + 16, { mono: true, size: 10.5, align: "center" });
    }
    const xl = `${ev.x.name}${ev.x.label ? " · " + ev.x.label : ""}`;
    txt(legendLabel(xl, 60), left + pw, top + ph + 30, { size: 11, color: c.ink, align: "right", weight: 500 });
    txt(legendLabel(ev.yv ? `${ev.y.name}${ev.y.label ? " · " + ev.y.label : ""}` : "events", 60), left, top - 8, { size: 11, color: c.ink, weight: 500 });
    const L = { kind: "events", left, top, pw, ph, xr, yr, hist: !ev.yv, X, Y, sx, sy, tx, ty, w, h };
    if (gt) L.labels = populationLabels(ev, tx, ty, X, Y, L);
    S.layout = L;
  }

  /** Population colours follow the shared category rule (in the order the gates list them). */
  function popColor(j) {
    return seriesColor(j);
  }

  function drawDensity(tx, ty, xr, yr, left, top, pw, ph) {
    const c = colors();
    const bins = Math.max(60, Math.min(240, Math.floor(pw / 2.5)));
    const by = Math.max(40, Math.floor(bins * ph / pw));
    const grid = new Uint32Array(bins * by);
    let max = 0;
    for (let i = 0; i < tx.length; i++) {
      const u = (tx[i] - xr[0]) / (xr[1] - xr[0]), v = (ty[i] - yr[0]) / (yr[1] - yr[0]);
      if (!(u >= 0 && u < 1 && v >= 0 && v < 1)) continue;
      const k = Math.floor(v * by) * bins + Math.floor(u * bins);
      if (++grid[k] > max) max = grid[k];
    }
    const cw = pw / bins, chh = ph / by;
    const stops = c.dark ? c.seq : c.seq.slice(1);
    for (let j = 0; j < by; j++) for (let i = 0; i < bins; i++) {
      const n = grid[j * bins + i];
      if (!n) continue;
      const [R, G, B] = ramp(stops, 0.08 + 0.92 * Math.log(1 + n) / Math.log(1 + max));
      g.fillStyle = `rgb(${R},${G},${B})`;
      g.fillRect(left + i * cw, top + ph - (j + 1) * chh, Math.ceil(cw), Math.ceil(chh));
    }
  }

  function drawGatedScatter(ev, tx, ty, X, Y, left, top, pw, ph) {
    const c = colors();
    const n = tx.length;
    const sz = n > 20000 ? 1.4 : 2;
    g.save();
    g.beginPath(); g.rect(left, top, pw, ph); g.clip();
    // events outside every child population first, faint
    g.fillStyle = alpha(c.ink, c.dark ? 0.22 : 0.16);
    for (let i = 0; i < n; i++) if (!ev.bits[i]) g.fillRect(X(tx[i]) - sz / 2, Y(ty[i]) - sz / 2, sz, sz);
    const kids = ev.gating.children.length;
    const focus = S.hover && S.hover.pop != null ? S.hover.pop : null;
    for (let j = 0; j < kids; j++) {
      g.fillStyle = alpha(popColor(j), focus == null || focus === j ? 0.55 : 0.15);
      for (let i = 0; i < n; i++) if (ev.bits[i] & (1 << j)) g.fillRect(X(tx[i]) - sz / 2, Y(ty[i]) - sz / 2, sz, sz);
    }
    g.restore();
  }

  /** A label at the middle of each child population's events: name and percent of parent. */
  function populationLabels(ev, tx, ty, X, Y, L) {
    const gt = ev.gating;
    const out = [];
    gt.children.forEach((p, j) => {
      const xs = [], ys = [];
      for (let i = 0; i < tx.length; i++) if (ev.bits[i] & (1 << j)) { xs.push(tx[i]); if (ty) ys.push(ty[i]); }
      if (!xs.length) return;
      const ax = X(percentile(xs, 50)), ay = ty ? Y(percentile(ys, 50)) : L.top + 18;
      const text = `${p.name}  ${p.percent_of_parent != null ? fmt(p.percent_of_parent, 3) + " %" : ""}`.trim();
      out.push({ j, p, text, ax, ay });
    });
    const items = out.map((o) => Object.assign(o, pill(o.text, 0, 0, { measureOnly: true, size: 11.5 })));
    const placed = placeLabels(items.map((o) => ({ ax: o.ax, ay: o.ay, w: o.w, h: o.h, gap: 0 })), { x: L.left + 2, y: L.top + 2, w: L.pw - 4, h: L.ph - 4 }, []);
    items.forEach((o, i) => {
      o.x = placed[i].x;
      o.y = placed[i].y;
      pill(o.text, o.x, o.y, { size: 11.5, color: popColor(o.j), bg: alpha(colors().bg, 0.92) });
    });
    return items;
  }

  function eventOverlays(L) {
    const c = colors();
    const sel = S.sel;
    if (sel && sel.kind === "gate") {
      const xa = L.X(scaleFn(L.sx)(sel.x0)), xb = L.X(scaleFn(L.sx)(sel.x1));
      const ya = sel.y0 != null ? L.Y(scaleFn(L.sy)(sel.y0)) : L.top + L.ph, yb = sel.y1 != null ? L.Y(scaleFn(L.sy)(sel.y1)) : L.top;
      g.fillStyle = alpha(c.accent, 0.08);
      g.fillRect(Math.min(xa, xb), Math.min(ya, yb), Math.abs(xb - xa), Math.abs(yb - ya));
      g.setLineDash([5, 4]);
      g.strokeStyle = c.accent;
      g.lineWidth = 1.5;
      g.strokeRect(Math.min(xa, xb), Math.min(ya, yb), Math.abs(xb - xa), Math.abs(yb - ya));
      g.setLineDash([]);
    }
    for (const o of L.labels || []) {
      const on = (S.hover && S.hover.pop === o.j) || (sel && sel.kind === "population" && sel.population.path === o.p.path);
      if (on) pill(o.text, o.x, o.y, { size: 11.5, color: popColor(o.j), bg: alpha(c.bg, 0.96), stroke: popColor(o.j) });
    }
  }

  // ---------- figures (openreadout_plot results)

  /**
   * A finished figure: the picture from the result, zoomable, with hover from its data when the
   * result carries it. The data may sit in structuredContent.figure or _meta["openreadout/figure"]:
   *   { width, height, panels: [{ area: {x, y, width, height} (picture px),
   *     x: {domain: [a, b], scale: "linear"|"log", unit, reversed}, y: {...},
   *     series: [{ name, x: [...], y: [...], color }] }] }
   * (`plot_area` is taken for `area`; a figure with one panel may put its keys at the top.)
   */
  async function showFigure(result) {
    const im = (result.content || []).find((c) => c.type === "image");
    const sc = result.structuredContent || {};
    const data = sc.figure || (result._meta && result._meta["openreadout/figure"]) || null;
    if (!im) {
      overlay(textOf(result) || "The figure has no picture.");
      return;
    }
    const img = await decode(im).catch(() => null);
    if (!img) { overlay("Could not decode the figure.", true); return; }
    S.data = { view: "figure" };
    S.args = {};
    // the agent's arguments, to draw the figure again in this theme, size and zoom
    const spec = S.toolInput && Object.keys(S.toolInput).length && canCallTools() ? S.toolInput : null;
    S.fig = { img, data: figurePanels(data, img), zoom: null, spec, xRange: null, drawn: null };
    S.img = null;
    S.sel = null;
    hideAct();
    const title = (data && (data.title || data.name)) || (sc.title) || "Figure";
    setTitle(title, (data && data.file) ? String(data.file).split(/[\\/]/).pop() : "");
    $("tabs").textContent = "";
    overlay(null);
    root.setAttribute("aria-busy", "false");
    invalidate();
    renderControls();
    setHint();
    redrawFigure(null);
  }

  /** Draw the figure again (openreadout_view figure): in the host's theme, as wide as the
   * canvas at the screen's pixel density, over `xRange` ({range: [a, b], panel}: the panel
   * of the spec it was dragged on; panels that share x, facets and plate grids zoom together)
   * or the whole range. */
  async function redrawFigure(xRange) {
    const F = S.fig;
    if (!F || !F.spec) return;
    const dpr = Math.min(3, window.devicePixelRatio || 1);
    const width = Math.round(Math.max(320, canvas.parentElement.clientWidth));
    const c = colors();
    const dark = c.dark;
    // drawn on the host's own background, so the figure sits in the page without a card
    const paper = cssHex(c.bg);
    const key = JSON.stringify([width, dpr, dark, paper, xRange]);
    if (F.drawn === key) return;
    F.drawn = key;
    // the viewer's header shows the title, so the picture leaves it out
    const spec = Object.assign({}, F.spec);
    delete spec.title;
    const args = { view: "figure", spec, dark, width, scale: dpr, outline: false };
    if (paper) args.paper = paper;
    if (xRange) {
      args.x_range = xRange.range;
      if (xRange.panel != null) args.panel = xRange.panel;
    }
    root.setAttribute("aria-busy", "true");
    try {
      const r = await callView(args);
      const im = (r.content || []).find((c) => c.type === "image");
      if (!im || S.fig !== F || F.drawn !== key) return;
      const img = await decode(im);
      const fig = r.structuredContent && r.structuredContent.figure;
      F.img = img;
      F.data = figurePanels(fig, img);
      F.zoom = null;
      F.xRange = xRange;
      S.hover = null;
      tip(null);
      for (const n of (r.structuredContent && r.structuredContent.notes) || []) toast(n);
      invalidate();
      renderControls();
    } catch (e) {
      // the picture the tool returned stays
      F.drawn = null;
    } finally {
      root.setAttribute("aria-busy", "false");
    }
  }

  /** A figure the viewer can draw again zooms by drawing it over an x range. */
  function figureRedrawsZoom() {
    const F = S.fig;
    return !!(F && F.spec && F.data.length);
  }

  function figurePanels(data, img) {
    if (!data) return [];
    const list = Array.isArray(data.panels) ? data.panels : [data];
    const sx = data.width ? img.width / data.width : 1, sy = data.height ? img.height / data.height : 1;
    return list.map((p) => {
      const a = p.area || p.plot_area || p.plotArea;
      if (!a) return null;
      const ax = (k) => {
        const v = p[k] || p[k + "_axis"] || {};
        const dom = pair(v.domain) || pair(v.range) || [num(v.min), num(v.max)];
        return { dom, log: v.scale === "log", unit: v.unit || v.label || "", reversed: !!v.reversed };
      };
      const series = (p.series || []).map((s) => ({ name: s.name || s.label || "", x: (s.x || []).map(num), y: (s.y || []).map(num), color: s.color })).filter((s) => s.x.length && s.x.length === s.y.length);
      return { area: { x: num(a.x) * sx, y: num(a.y) * sy, w: num(a.width ?? a.w) * sx, h: num(a.height ?? a.h) * sy }, x: ax("x"), y: ax("y"), series, specPanel: Number.isInteger(p.spec_panel) ? p.spec_panel : null };
    }).filter((p) => p && [p.area.x, p.area.y, p.area.w, p.area.h].every(isFinite) && p.x.dom.every(isFinite) && p.y.dom.every(isFinite));
  }

  /** The x range dragged from `x0` to `x1` (canvas px) at height `ym` over one panel, in that
   * panel's units, with the spec panel it belongs to; null when the drag is not over a panel
   * with a numeric x axis. The ends are clamped to the panel. */
  function figureDragRange(x0, x1, ym) {
    const F = S.fig, L = S.layout;
    if (!F || !L || Math.abs(x1 - x0) <= 8) return null;
    const [ia, iy] = L.inv(x0, ym), [ib] = L.inv(x1, ym);
    // the panel under the drag, or the nearest one above or below it (a drag along a gap)
    const across = F.data.filter((q) => Math.max(ia, ib) >= q.area.x && Math.min(ia, ib) <= q.area.x + q.area.w);
    const gapTo = (q) => (iy < q.area.y ? q.area.y - iy : iy > q.area.y + q.area.h ? iy - q.area.y - q.area.h : 0);
    const p = across.sort((q, r) => gapTo(q) - gapTo(r))[0];
    if (!p || gapTo(p) * L.k > 40) return null;
    const at = (ix) => {
      const tr = (v) => (p.x.log ? Math.log10(v) : v);
      const [xa, xb] = p.x.dom.map(tr);
      let u = Math.min(1, Math.max(0, (ix - p.area.x) / p.area.w));
      if (p.x.reversed) u = 1 - u;
      const v = xa + u * (xb - xa);
      return p.x.log ? Math.pow(10, v) : v;
    };
    const a = at(ia), b = at(ib);
    if (!isFinite(a) || !isFinite(b) || a === b) return null;
    return { range: [Math.min(a, b), Math.max(a, b)], panel: p.specPanel };
  }

  function figureZoom(z) {
    if (!S.fig) return;
    S.fig.zoom = z;
    invalidate();
    renderControls();
  }

  function drawFigure() {
    const F = S.fig;
    if (!F) return;
    const img = F.img;
    const z = F.zoom || { x: 0, y: 0, w: img.width, h: img.height };
    const [w, h] = sizeCanvas(root.classList.contains("fill") ? null : img.width / img.height);
    g.clearRect(0, 0, w, h);
    const k = Math.min(w / z.w, h / z.h);
    const ox = (w - z.w * k) / 2, oy = (h - z.h * k) / 2;
    g.imageSmoothingEnabled = true;
    g.imageSmoothingQuality = "high";
    g.drawImage(img, z.x, z.y, z.w, z.h, ox, oy, z.w * k, z.h * k);
    // picture px → canvas px and back
    const P = (x, y) => [ox + (x - z.x) * k, oy + (y - z.y) * k];
    const inv = (px, py) => [z.x + (px - ox) / k, z.y + (py - oy) / k];
    S.layout = { kind: "figure", w, h, P, inv, k };
  }

  function figureData(px, py) {
    const F = S.fig;
    const L = S.layout;
    if (!F || !L) return null;
    const [ix, iy] = L.inv(px, py);
    for (const p of F.data) {
      const a = p.area;
      if (ix < a.x || ix > a.x + a.w || iy < a.y || iy > a.y + a.h) continue;
      const tr = (ax) => (v) => (ax.log ? Math.log10(v) : v);
      const fx = tr(p.x), fy = tr(p.y);
      const [xa, xb] = p.x.dom.map(fx), [ya, yb] = p.y.dom.map(fy);
      let u = (ix - a.x) / a.w;
      if (p.x.reversed) u = 1 - u;
      const xv = xa + u * (xb - xa), yv = ya + (1 - (iy - a.y) / a.h) * (yb - ya);
      const X = (v) => a.x + ((p.x.reversed ? xb - fx(v) : fx(v) - xa) / (xb - xa)) * a.w;
      const Y = (v) => a.y + (1 - (fy(v) - ya) / (yb - ya)) * a.h;
      const inside = (qx, qy) => qx >= a.x - 0.5 && qx <= a.x + a.w + 0.5 && qy >= a.y - 0.5 && qy <= a.y + a.h + 0.5;
      const xAt = p.x.log ? Math.pow(10, xv) : xv, yAt = p.y.log ? Math.pow(10, yv) : yv;
      // A few lines: every series' value at the nearest x it has, the cursor snapped to it.
      const lines = p.series.length <= 8 && p.series.every((s) => s.x.length > 3);
      if (lines) {
        let snapX = null, bestDx = Infinity;
        for (const s of p.series) for (let i = 0; i < s.x.length; i++) {
          const dx = Math.abs(X(s.x[i]) - ix);
          if (dx < bestDx) { bestDx = dx; snapX = s.x[i]; }
        }
        const rows = [];
        for (const s of p.series) {
          let bi = -1, bd = Infinity;
          for (let i = 0; i < s.x.length; i++) {
            const d = Math.abs(s.x[i] - snapX);
            if (d < bd) { bd = d; bi = i; }
          }
          if (bi < 0 || !isFinite(s.y[bi])) continue;
          const cx = X(s.x[bi]), cy = Y(s.y[bi]);
          if (!inside(cx, cy) || Math.abs(cx - X(snapX)) > a.w * 0.02) continue;
          rows.push({ s, i: bi, cx, cy });
        }
        rows.sort((q, r) => q.cy - r.cy);
        if (rows.length) return { p, x: snapX, y: yAt, rows, rule: X(snapX) };
      }
      // Otherwise the nearest point of any series, in picture pixels.
      let best = null;
      const z = F.zoom;
      for (const s of p.series) {
        for (let i = 0; i < s.x.length; i++) {
          const qx = X(s.x[i]), qy = Y(s.y[i]);
          if (!inside(qx, qy) || (z && (qx < z.x || qx > z.x + z.w || qy < z.y || qy > z.y + z.h))) continue;
          const dx = qx - ix, dy = qy - iy;
          const d = dx * dx + dy * dy * 0.25;
          if (!best || d < best.d) best = { d, s, i, cx: qx, cy: qy };
        }
      }
      return { p, x: xAt, y: yAt, best: best && Math.sqrt(best.d) * L.k < 40 ? best : null };
    }
    return null;
  }

  /** The rich tooltip of a figure readout: the x value as a heading, then each series (with its
   * colour) and its value, aligned; a single point gives its x and y. */
  function figureTip(f) {
    const p = f.p;
    const xu = p.x.unit || "", yu = p.y.unit || "";
    const axisName = (u, fallback) => (u ? legendLabel(u, 22) : fallback);
    if (f.rows) {
      return {
        head: { value: fmtValue(f.x, 5), unit: xu },
        rows: f.rows.map((r) => ({ color: r.s.color, key: legendLabel(r.s.name || "value", 32), value: fmtValue(r.s.y[r.i], 4) })),
      };
    }
    if (f.best) {
      const s = f.best.s, i = f.best.i;
      return {
        head: s.name ? { value: legendLabel(s.name, 32) } : null,
        rows: [
          { color: s.name ? null : s.color, key: axisName(xu, "x"), value: fmtValue(s.x[i], 5) },
          { color: null, key: axisName(yu, "y"), value: fmtValue(s.y[i], 5) },
        ],
      };
    }
    return {
      head: null,
      rows: [
        { color: null, key: axisName(xu, "x"), value: fmtValue(f.x, 4) },
        { color: null, key: axisName(yu, "y"), value: fmtValue(f.y, 4) },
      ],
    };
  }

  function figureOverlays(L) {
    const hv = S.hover;
    const c = colors();
    const dot = (b, col) => {
      const [x, y] = L.P(b.cx, b.cy);
      g.beginPath();
      g.arc(x, y, 4, 0, Math.PI * 2);
      g.fillStyle = c.bg;
      g.fill();
      g.strokeStyle = col || c.accent;
      g.lineWidth = 2;
      g.stroke();
    };
    if (hv && hv.fig && hv.fig.rows) {
      const f = hv.fig, a = f.p.area;
      const [x, y0] = L.P(f.rule, a.y), [, y1] = L.P(f.rule, a.y + a.h);
      g.strokeStyle = alpha(c.ink, 0.25);
      g.lineWidth = 1;
      g.beginPath(); g.moveTo(Math.round(x) + 0.5, y0); g.lineTo(Math.round(x) + 0.5, y1); g.stroke();
      for (const r of f.rows) dot(r, r.s.color);
    } else if (hv && hv.fig && hv.fig.best) {
      dot(hv.fig.best, hv.fig.best.s.color);
    }
  }

  // ---------- pointer: hover readout, selection, zoom and pan

  function pos(e) {
    const r = canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  /** The hover tooltip: `rich` ({head, rows: [{color, key, value}]}) as an aligned key–value
   * list with series swatches, else `text` as lines. */
  function tip(text, x, y, rich) {
    const t = $("tip");
    if (!text) { t.hidden = true; return; }
    t.hidden = false;
    t.classList.toggle("rich", !!rich);
    if (rich) {
      t.textContent = "";
      const el = (cls, s) => { const e = document.createElement("span"); e.className = cls; if (s != null) e.textContent = s; t.appendChild(e); return e; };
      if (rich.head) {
        const h = el("tip-head");
        const v = document.createElement("span");
        v.textContent = rich.head.value;
        h.appendChild(v);
        if (rich.head.unit) { const u = document.createElement("span"); u.className = "tip-unit"; u.textContent = " " + rich.head.unit; h.appendChild(u); }
      }
      const swatches = rich.rows.some((r) => r.color);
      t.classList.toggle("plain", !swatches);
      for (const r of rich.rows) {
        if (swatches) {
          const sw = el("tip-swatch");
          if (r.color) sw.style.background = r.color;
        }
        el("tip-key", r.key);
        const v = el("tip-value", r.value);
        if (r.unit) { const u = document.createElement("span"); u.className = "tip-unit"; u.textContent = " " + r.unit; v.appendChild(u); }
      }
    } else {
      t.textContent = text;
    }
    const w = canvas.clientWidth, h = canvas.clientHeight;
    let left = x + 14, top = y - t.offsetHeight - 10;
    if (left + t.offsetWidth > w - 4) left = x - t.offsetWidth - 14;
    if (top < 2) top = y + 16;
    t.style.left = Math.max(2, left) + "px";
    t.style.top = Math.max(2, Math.min(h - t.offsetHeight - 2, top)) + "px";
  }

  /** What is under the pointer: sets S.hover and returns the readout text. */
  function hover(px, py) {
    const L = S.layout, d = S.data;
    S.hover = null;
    if (!L || !d) return "";
    if (L.kind === "image") {
      if (L.mini && px >= L.mini.x && px <= L.mini.x + L.mini.w && py >= L.mini.y && py <= L.mini.y + L.mini.h) {
        S.hover = { mini: true };
        return "Click to go there";
      }
      const hitAnn = (L.ann || []).slice().reverse().find((a) => px >= a.x - 3 && px <= a.x + a.w + 3 && py >= a.y - 3 && py <= a.y + a.h + 3);
      const p = L.inv(px, py);
      S.hover = { px, py, ann: hitAnn ? hitAnn.a : null };
      const I = S.img;
      if (!I || p.x < 0 || p.y < 0 || p.x > I.full.w || p.y > I.full.h) return "";
      const ps = physical();
      let s = `x ${fmtInt(p.x)}  y ${fmtInt(p.y)} px`;
      if (ps) s += `\n${fmtLength(p.x * ps.x)}, ${fmtLength(p.y * ps.y)}`;
      if (hitAnn && hitAnn.a.label) s = `${hitAnn.a.label}\n` + s;
      const rows = [{ key: "x", value: fmtInt(p.x), unit: "px" }, { key: "y", value: fmtInt(p.y), unit: "px" }];
      if (ps) rows.push({ key: "position", value: `${fmtLength(p.x * ps.x)}, ${fmtLength(p.y * ps.y)}` });
      S.hover.tip = { head: hitAnn && hitAnn.a.label ? { value: legendLabel(hitAnn.a.label, 40) } : null, rows };
      return s;
    }
    if (L.kind === "plot") {
      if (px < L.left || px > L.left + L.pw) return "";
      const panel = L.panels.find((q) => py >= q.py - 6 && py <= q.py + q.ph + 6) || L.panels[0];
      if (!panel) return "";
      const xv = plotInv(L, px);
      const nearest = (s) => {
        let best = -1, bd = Infinity;
        for (let k = 0; k < s.x.length; k++) {
          const dd = Math.abs(s.x[k] - xv);
          if (dd < bd) { bd = dd; best = k; }
        }
        return best;
      };
      const unit = (d.plot.x && (d.plot.x.unit || (d.plot.x.quantity !== "sample" ? d.plot.x.quantity : ""))) || (d.plot.kind === "spectrum" ? "m/z" : "");
      const lines = [];
      // the same readout as an aligned key–value list for the tooltip
      const rows = [];
      let headX = null;
      S.hover = { px, py, dots: [] };
      // a peak under the pointer
      const peaks = d.plot.peaks || [];
      if (peaks.length && panel === L.panels[0]) {
        const pk = peaks.find((q) => q.from != null && xv >= Math.min(q.from, q.to) && xv <= Math.max(q.from, q.to)) ||
          peaks.find((q) => Math.abs(L.X(q.x) - px) < 5);
        if (pk) {
          S.hover.peak = pk;
          lines.push(`peak ${pk.label ?? ""} · apex ${fmt(pk.x, 5)} ${unit}`.trim());
          if (pk.area != null) lines.push(`area ${fmt(pk.area, 5)}${pk.area_percent != null ? ` (${fmt(pk.area_percent, 3)} %)` : ""}`);
          if (pk.height != null) lines.push(`height ${fmt(pk.height, 5)}`);
          rows.push({ key: `peak ${pk.label ?? ""} apex`.replace("  ", " "), value: fmtValue(pk.x, 5), unit });
          if (pk.area != null) rows.push({ key: "area", value: fmtValue(pk.area, 5) + (pk.area_percent != null ? ` (${fmt(pk.area_percent, 3)} %)` : "") });
          if (pk.height != null) rows.push({ key: "height", value: fmtValue(pk.height, 5) });
        }
      }
      if (panel.series.length > 8) {
        // many curves: the closest one to the pointer
        let best = null;
        for (const s of panel.series) {
          const k = nearest(s);
          const v = s.y ? s.y[k] : null;
          if (k < 0 || v == null) continue;
          const dist = Math.abs(panel.Y(v) - py);
          if (!best || dist < best.dist) best = { s, k, v, dist };
        }
        if (best && best.dist < 24) {
          S.hover.curve = best.s;
          S.hover.dots.push({ x: L.X(best.s.x[best.k]), y: panel.Y(best.v), color: colors().accent });
          lines.unshift(`${best.s.name}`, `${fmt(best.s.x[best.k], 5)} ${unit} · ${fmt(best.v, 5)}`);
          S.hover.tip = { head: { value: legendLabel(best.s.name, 32) }, rows: [{ color: best.s.color, key: unit || "x", value: fmtValue(best.s.x[best.k], 5) }, { color: null, key: "value", value: fmtValue(best.v, 5) }].concat(rows) };
        } else {
          lines.unshift(`${fmt(xv, 5)} ${unit}`);
          S.hover.tip = rows.length ? { head: { value: fmtValue(xv, 5), unit }, rows } : null;
        }
        return lines.join("\n");
      }
      const head = [];
      for (const s of panel.series) {
        const k = nearest(s);
        if (k < 0) continue;
        const y = s.y ? s.y[k] : null;
        if (!head.length) head.push(`${fmt(s.x[k], 6)} ${unit}`.trim());
        const yv = y != null ? fmt(y, 5) : `${fmt(s.lo[k], 4)} … ${fmt(s.hi[k], 4)}`;
        const yTip = y != null ? fmtValue(y, 5) : `${fmtValue(s.lo[k], 4)} … ${fmtValue(s.hi[k], 4)}`;
        lines.push(`${panel.series.length > 1 ? legendLabel(s.name, 16) + "  " : ""}${yv}${s.unit ? " " + s.unit : ""}`);
        if (headX == null) headX = s.x[k];
        rows.push({ color: panel.series.length > 1 ? s.color : null, key: panel.series.length > 1 ? legendLabel(s.name, 26) : (s.name ? legendLabel(s.name, 26) : "value"), value: yTip, unit: s.unit || "" });
        const yy = y != null ? y : s.hi[k];
        if (yy != null && isFinite(yy) && s.style !== "sticks") S.hover.dots.push({ x: L.X(s.x[k]), y: panel.Y(yy), color: s.color });
      }
      if (headX != null) S.hover.tip = { head: { value: fmtValue(headX, 6), unit }, rows };
      return head.concat(lines).join("\n");
    }
    if (L.kind === "plate") {
      const well = wellAt(L, px, py);
      if (!well) return "";
      S.hover = { well };
      const pl = d.plate;
      const name = wellName(...well);
      const v = wellValue(well);
      const gc = groupColoring();
      const grp = gc ? gc.map[name] : null;
      if (pl.kinetic) return "";
      S.hover.tip = {
        head: { value: name, unit: grp || "" },
        rows: [{ color: gc && grp ? gc.colorsBy.get(grp) || null : null, key: S.args.column || "value", value: v == null ? "no value" : fmtValue(v, 5) }],
      };
      return `${name}${grp ? " · " + grp : ""}\n${v == null ? "no value" : fmt(v, 5)}`;
    }
    if (L.kind === "events") {
      const lab = (L.labels || []).find((o) => px >= o.x && px <= o.x + o.w && py >= o.y && py <= o.y + o.h);
      if (lab) {
        S.hover = { pop: lab.j };
        const p = lab.p;
        S.hover.tip = {
          head: { value: legendLabel(p.path, 40) },
          rows: [{ key: "events", value: fmtInt(p.count) }, { key: "of parent", value: fmt(p.percent_of_parent, 3), unit: "%" }],
        };
        return `${p.path}\n${fmtInt(p.count)} events · ${fmt(p.percent_of_parent, 3)} % of parent\nclick to select`;
      }
      const u = (px - L.left) / L.pw, v = 1 - (py - L.top) / L.ph;
      if (u < 0 || u > 1 || v < 0 || v > 1) return "";
      S.hover = { px, py };
      const x = scaleInv(L.sx)(L.xr[0] + u * (L.xr[1] - L.xr[0]));
      S.hover.tip = { head: null, rows: [{ key: d.events.x.name, value: fmtValue(x, 4) }] };
      if (L.hist) return `${d.events.x.name} ${fmt(x, 4)}`;
      const y = scaleInv(L.sy)(L.yr[0] + v * (L.yr[1] - L.yr[0]));
      S.hover.tip.rows.push({ key: d.events.y.name, value: fmtValue(y, 4) });
      return `${d.events.x.name} ${fmt(x, 4)}\n${d.events.y.name} ${fmt(y, 4)}`;
    }
    if (L.kind === "figure") {
      const f = figureData(px, py);
      S.hover = { fig: f };
      if (!f) return "";
      const p = f.p;
      const xu = p.x.unit || "", yu = p.y.unit || "";
      const rich = figureTip(f);
      S.hover.tip = rich;
      if (f.rows) {
        // x, then each series at it, top to bottom as they lie on the plot
        const head = `${fmt(f.x, 5)}${xu ? " " + xu : ""}`;
        return [head].concat(f.rows.map((r) => `${r.s.name ? legendLabel(r.s.name, 24) + " " : ""}${fmt(r.s.y[r.i], 4)}`)).join("\n");
      }
      if (f.best) {
        const s = f.best.s, i = f.best.i;
        return `${s.name ? s.name + "\n" : ""}${fmt(s.x[i], 5)}${xu ? " " + xu : ""} · ${fmt(s.y[i], 5)}${yu ? " " + yu : ""}`;
      }
      return `${fmt(f.x, 4)}${xu ? " " + xu : ""} · ${fmt(f.y, 4)}${yu ? " " + yu : ""}`;
    }
    return "";
  }

  function finishSelect(a, b) {
    const L = S.layout;
    if (!L) return;
    const x0 = Math.min(a[0], b[0]), y0 = Math.min(a[1], b[1]), x1 = Math.max(a[0], b[0]), y1 = Math.max(a[1], b[1]);
    const anchor = { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    if (L.kind === "image") {
      const I = S.img;
      const p = L.inv(x0, y0), q = L.inv(x1, y1);
      const rx0 = Math.max(0, Math.floor(p.x)), ry0 = Math.max(0, Math.floor(p.y));
      const rx1 = Math.min(I.full.w, Math.ceil(q.x)), ry1 = Math.min(I.full.h, Math.ceil(q.y));
      if (rx1 - rx0 < 2 || ry1 - ry0 < 2) return;
      showAct({ kind: "region", region: { x: rx0, y: ry0, width: rx1 - rx0, height: ry1 - ry0 } }, anchor);
    } else if (L.kind === "plot") {
      const xa = plotInv(L, Math.max(L.left, x0)), xb = plotInv(L, Math.min(L.left + L.pw, x1));
      showAct({ kind: "xrange", x0: Math.min(xa, xb), x1: Math.max(xa, xb) }, { x: x0, y: L.top + 8, w: x1 - x0, h: 0 });
    } else if (L.kind === "plate") {
      const wa = wellAt(L, Math.max(L.ox, x0), Math.max(L.oy, y0)), wb = wellAt(L, Math.min(L.ox + L.cw * L.columns - 1, x1), Math.min(L.oy + L.ch * L.rows - 1, y1));
      if (!wa || !wb) return;
      const wells = [];
      for (let r = wa[0]; r <= wb[0]; r++) for (let c = wa[1]; c <= wb[1]; c++) if (S.data.plate.kinetic ? wellSeries(r, c) : true) wells.push([r, c]);
      if (wells.length) showAct({ kind: "wells", wells }, actAnchorForWells(wells));
    } else if (L.kind === "events") {
      const invx = (px) => scaleInv(L.sx)(L.xr[0] + (px - L.left) / L.pw * (L.xr[1] - L.xr[0]));
      const invy = (py) => scaleInv(L.sy)(L.yr[0] + (1 - (py - L.top) / L.ph) * (L.yr[1] - L.yr[0]));
      const sel = { kind: "gate", x0: invx(Math.max(L.left, x0)), x1: invx(Math.min(L.left + L.pw, x1)) };
      if (!L.hist) { sel.y0 = invy(Math.min(L.top + L.ph, y1)); sel.y1 = invy(Math.max(L.top, y0)); }
      // count the sampled events inside, in display space
      const ua = L.X(scaleFn(L.sx)(sel.x0)), ub = L.X(scaleFn(L.sx)(sel.x1));
      let n = 0;
      for (let i = 0; i < L.tx.length; i++) {
        const xx = L.X(L.tx[i]);
        if (!(xx >= ua && xx <= ub)) continue;
        if (!L.hist) { const yy = L.Y(L.ty[i]); if (!(yy >= Math.max(L.top, y0) && yy <= Math.min(L.top + L.ph, y1))) continue; }
        n++;
      }
      sel.count = n;
      sel.of = L.tx.length;
      showAct(sel, anchor);
    } else if (L.kind === "figure" && figureRedrawsZoom() && figureDragRange(x0, x1, (y0 + y1) / 2)) {
      // zoom by drawing the figure again over the dragged x range of the panel dragged on
      redrawFigure(figureDragRange(x0, x1, (y0 + y1) / 2));
    } else if (L.kind === "figure") {
      const [ia, ja] = L.inv(x0, y0), [ib, jb] = L.inv(x1, y1);
      const z = { x: Math.max(0, ia), y: Math.max(0, ja), w: Math.min(S.fig.img.width, ib) - Math.max(0, ia), h: Math.min(S.fig.img.height, jb) - Math.max(0, ja) };
      if (z.w > 8 && z.h > 8) figureZoom(z);
    }
  }

  function click(px, py, e) {
    const L = S.layout, d = S.data;
    if (!L || !d) return;
    if (L.kind === "image") {
      if (S.hover && S.hover.mini && L.mini) {
        const I = S.img;
        const x = (px - L.mini.x) / L.mini.s, y = (py - L.mini.y) / L.mini.s;
        flyTo({ x: x - I.cam.w / 2, y: y - I.cam.h / 2, w: I.cam.w, h: I.cam.h }, 500);
        return;
      }
      const hitAnn = S.hover && S.hover.ann;
      if (hitAnn) {
        const at = hitAnn.kind === "box" ? [hitAnn.x, hitAnn.y, hitAnn.w, hitAnn.h] : [hitAnn.x, hitAnn.y];
        const a = (L.ann || []).find((q) => q.a === hitAnn && q.label) || (L.ann || []).find((q) => q.a === hitAnn);
        showAct({ kind: "annotation", ann: hitAnn, label: hitAnn.label || hitAnn.kind, at, region: hitAnn.kind === "box" ? { x: Math.round(hitAnn.x), y: Math.round(hitAnn.y), width: Math.round(hitAnn.w), height: Math.round(hitAnn.h) } : null }, a ? { x: a.x, y: a.y, w: a.w, h: a.h } : null);
        return;
      }
      if (S.sel) { clearSelection(); return; }
      const p = L.inv(px, py);
      if (S.img && p.x >= 0 && p.y >= 0 && p.x <= S.img.full.w && p.y <= S.img.full.h) showAct({ kind: "point", x: p.x, y: p.y }, { x: px - 6, y: py - 6, w: 12, h: 12 });
      return;
    }
    if (L.kind === "plot") {
      if (px < L.left || px > L.left + L.pw) return;
      const p = d.plot;
      const xv = plotInv(L, px);
      if (S.hover && S.hover.peak) {
        const pk = S.hover.peak;
        showAct({ kind: "peak", peak: pk }, { x: L.X(pk.x) - 4, y: L.top + 8, w: 8, h: 0 });
        return;
      }
      if (p.kind === "spectrum") {
        // the tallest stick or local maximum within a few pixels
        const s = L.panels[0] && L.panels[0].series[0];
        if (!s) return;
        let best = null;
        for (let k = 0; k < s.x.length; k++) {
          const dx = Math.abs(L.X(s.x[k]) - px);
          if (dx > 6 || s.y[k] == null) continue;
          if (!best || s.y[k] > best.y) best = { x: s.x[k], y: s.y[k] };
        }
        if (best) showAct({ kind: "mz", mz: best.x, y: best.y }, { x: L.X(best.x) - 4, y: L.panels[0].Y(best.y), w: 8, h: 8 });
        else clearSelection();
        return;
      }
      if (p.kind === "chromatogram" && p.source === "spectra") {
        showAct({ kind: "xpoint", x: xv }, { x: px - 4, y: L.top + 8, w: 8, h: 0 });
        return;
      }
      clearSelection();
      return;
    }
    if (L.kind === "plate") {
      const well = wellAt(L, px, py);
      if (!well) { clearSelection(); return; }
      let wells = [well];
      if (e && (e.shiftKey || e.metaKey || e.ctrlKey) && S.sel && S.sel.kind === "wells") {
        const has = S.sel.wells.some(([r, c]) => r === well[0] && c === well[1]);
        wells = has ? S.sel.wells.filter(([r, c]) => !(r === well[0] && c === well[1])) : S.sel.wells.concat([well]);
        if (!wells.length) { clearSelection(); return; }
      }
      showAct({ kind: "wells", wells }, actAnchorForWells(wells));
      return;
    }
    if (L.kind === "events") {
      const lab = (L.labels || []).find((o) => px >= o.x && px <= o.x + o.w && py >= o.y && py <= o.y + o.h);
      if (lab) { showAct({ kind: "population", population: lab.p }, { x: lab.x, y: lab.y, w: lab.w, h: lab.h }); return; }
      clearSelection();
      return;
    }
  }

  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    canvas.setPointerCapture(e.pointerId);
    const a = pos(e);
    const panMode = S.layout && S.layout.kind === "image" && (e.shiftKey || e.button === 1 || spaceDown);
    S.drag = { a, b: a, moved: false, mode: panMode ? "pan" : "select", cam: S.img && S.img.cam ? Object.assign({}, S.img.cam) : null };
    if (panMode) canvas.classList.add("grab");
  });
  canvas.addEventListener("pointermove", (e) => {
    const p = pos(e);
    if (S.drag) {
      const dr = S.drag;
      if (Math.abs(p[0] - dr.a[0]) + Math.abs(p[1] - dr.a[1]) > 4) dr.moved = true;
      dr.b = p;
      if (dr.moved && dr.mode === "pan" && S.img && S.layout) {
        const k = S.layout.k;
        S.img.cam = clampCam({ x: dr.cam.x - (p[0] - dr.a[0]) / k, y: dr.cam.y - (p[1] - dr.a[1]) / k, w: dr.cam.w, h: dr.cam.h });
        S.img.settled = false;
        invalidate();
      } else if (dr.moved) {
        tip(null);
        invalidateOverlay();
      }
      return;
    }
    const text = hover(p[0], p[1]);
    $("readout").textContent = text.replace(/\n/g, "  ·  ");
    tip(text, p[0], p[1], S.hover && S.hover.tip);
    canvas.classList.toggle("pointer", !!(S.hover && (S.hover.ann || S.hover.mini || S.hover.pop != null || S.hover.peak || S.hover.well)));
    invalidateOverlay();
  });
  canvas.addEventListener("pointerup", (e) => {
    const drag = S.drag;
    S.drag = null;
    canvas.classList.remove("grab");
    if (!drag) return;
    const p = pos(e);
    if (drag.moved && drag.mode === "pan") { settle(); return; }
    if (drag.moved) { finishSelect(drag.a, p); invalidateOverlay(); }
    else click(p[0], p[1], e);
  });
  canvas.addEventListener("pointerleave", () => {
    tip(null);
    S.hover = null;
    $("readout").textContent = "";
    invalidateOverlay();
  });
  canvas.addEventListener("dblclick", (e) => {
    const L = S.layout;
    if (L && L.kind === "image" && S.img) {
      const [px, py] = pos(e);
      clearSelection();
      zoomBy(e.altKey ? 0.5 : 2, L.inv(px, py));
      return;
    }
    if (L && L.kind === "figure") { if (S.fig && S.fig.xRange) redrawFigure(null); else figureZoom(null); return; }
    if (ZOOM_KEYS.some((k) => S.args && S.args[k] !== undefined)) { clearSelection(); load({}); }
  });
  // Pinch (and ctrl/cmd + wheel) zooms images; a plain wheel scrolls the chat as usual.
  canvas.addEventListener("wheel", (e) => {
    const L = S.layout;
    if (!L || L.kind !== "image" || !S.img || !(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const [px, py] = pos(e);
    const at = L.inv(px, py);
    const f = Math.exp(-e.deltaY * 0.01);
    const c = S.img.cam;
    let w = c.w / f;
    w = Math.max(16, Math.min(S.img.full.w * 1.6, w));
    const k = w / c.w;
    S.img.cam = clampCam({ x: at.x - (at.x - c.x) * k, y: at.y - (at.y - c.y) * k, w, h: c.h * k });
    S.img.settled = false;
    invalidate();
    clearTimeout(wheelTimer);
    wheelTimer = setTimeout(settle, 180);
  }, { passive: false });
  let wheelTimer = 0;
  let spaceDown = false;
  window.addEventListener("keyup", (e) => { if (e.key === " ") spaceDown = false; });
  canvas.addEventListener("keydown", (e) => {
    const d = S.data;
    if (!d) return;
    if (e.key === " ") { spaceDown = true; e.preventDefault(); return; }
    if (e.key === "Escape") { clearSelection(); return; }
    if (d.view === "image" && S.img) {
      const c = S.img.cam;
      if (e.key === "+" || e.key === "=") { e.preventDefault(); zoomBy(2); return; }
      if (e.key === "-" || e.key === "_") { e.preventDefault(); zoomBy(0.5); return; }
      if (e.key === "0") { e.preventDefault(); zoomTo(fullRect()); return; }
      if (e.shiftKey && e.key.startsWith("Arrow")) {
        e.preventDefault();
        const dx = e.key === "ArrowLeft" ? -0.2 : e.key === "ArrowRight" ? 0.2 : 0;
        const dy = e.key === "ArrowUp" ? -0.2 : e.key === "ArrowDown" ? 0.2 : 0;
        flyTo({ x: c.x + dx * c.w, y: c.y + dy * c.h, w: c.w, h: c.h }, 250);
        return;
      }
    }
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    if (d.view === "image" && d.image && !d.image.projection) {
      const info = imageInfo();
      const z = (d.image.z[0] || 0) + step;
      if (z >= 0 && z < (info.size_z || 1)) reloadPlane({ z });
    } else if (d.plot && d.plot.kind === "trace") {
      const s = d.plot.sweep + step;
      if (s >= 0 && s < d.plot.sweep_count) load({ sweep: s }, { keepZoom: true });
    } else if (d.plot && d.plot.kind === "spectrum") {
      const i = d.plot.spectrum.index + step;
      if (i >= 0 && i < d.plot.scan_count) load({ index: i, rt_min: undefined, scan: undefined }, { keepZoom: true });
    } else if (d.plate && d.plate.kinetic) {
      const k = d.plate.kinetic;
      S.kin.t = Math.max(0, Math.min(k.n - 1, S.kin.t + step * Math.max(1, Math.round(k.n / 60))));
      const r = $("kin-range"), o = $("kin-time");
      if (r) r.value = S.kin.t;
      if (o) o.textContent = fmtTime(kinTime());
      invalidate();
    }
  });

  $("fullscreen").addEventListener("click", async () => {
    const want = S.hostContext.displayMode === "fullscreen" ? "inline" : "fullscreen";
    try {
      const r = await host.request("ui/request-display-mode", { mode: want }, 10000);
      applyContext({ displayMode: r && r.mode ? r.mode : want });
    } catch (e) {
      /* the host said no */
    }
  });

  // Report our height so hosts that size the frame to its content can do so.
  // Measured as the SDK does: the content's height, the window's width.
  let lastH = 0, lastW = 0, sizeQueued = false;
  const reportSize = () => {
    if (sizeQueued) return;
    sizeQueued = true;
    requestAnimationFrame(() => {
      sizeQueued = false;
      const html = document.documentElement;
      const saved = html.style.height;
      html.style.height = "max-content";
      const h = Math.ceil(html.getBoundingClientRect().height);
      html.style.height = saved;
      const w = Math.ceil(window.innerWidth);
      if (h === lastH && w === lastW) return;
      lastH = h;
      lastW = w;
      host.notify("ui/notifications/size-changed", { width: w, height: h });
    });
  };
  const sizeObserver = new ResizeObserver(reportSize);
  sizeObserver.observe(document.documentElement);
  sizeObserver.observe(document.body);
  let resizeTimer = 0;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      invalidate();
      if (S.img && S.img.settled) fetchDetail();
      if (S.fig) redrawFigure(S.fig.xRange);
      if (S.sel && !$("act").hidden) positionAct(null);
    }, 150);
  });
  if (window.matchMedia) {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onScheme = () => { if (!S.hostContext.theme) { palette = null; invalidate(); } };
    if (mq.addEventListener) mq.addEventListener("change", onScheme);
  }

  // The data labels' fonts, from bytes (no network). Redraw once they are ready.
  if (typeof FontFace !== "undefined" && document.fonts && FONTS.length) {
    Promise.all(FONTS.map(([family, weight, b64]) => {
      const face = new FontFace(family, bytesFromBase64(b64), { weight, style: "normal" });
      document.fonts.add(face);
      return face.load();
    })).then(() => { palette = null; invalidate(); }).catch(() => {});
  }

  // ---------- start

  host
    .request("ui/initialize", {
      appInfo: { name: "openreadout-viewer", version: "2" },
      appCapabilities: { availableDisplayModes: ["inline", "fullscreen"] },
      protocolVersion: PROTOCOL_VERSION,
    }, 20000)
    .then((r) => {
      S.hostCaps = (r && r.hostCapabilities) || {};
      applyContext((r && r.hostContext) || {});
      host.notify("ui/notifications/initialized", {});
      reportSize();
    })
    .catch(() => overlay("This page is the OpenReadout viewer. It runs inside an MCP host.", true));
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    niceTicks, niceBelow, fmt, fmtTick, tickLabels, fmtInt, fmtLength, timeUnit, legendLabel, ramp, hexRgb, cssHex, fmtValue, plotXs, sampleWindow,
    f32FromBase64, bytesFromBase64, scaleFn, scaleInv, scaleTicks, percentile, hintFrom, wellName, parseWell,
    normalizeAnnotations, placeLabels, ease, lerpRect, fitAspect, defaultGroupMap, isBlankGroup, seriesGroup,
    callText, wellSpec, THEMES, categoryColors, spreadLabels,
  };
} else {
  main();
}
