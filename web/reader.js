// What the demo page asks of OpenReadout, without the page itself: open a file, describe it,
// choose and draw a view, and turn the JSON into the facts and numbers the page lists. It runs
// in the browser tab and in Node (web/test/demo.test.mjs), on the npm wrapper of the
// WebAssembly build (packaging/wasm/index.js).

import { load, open } from "./openreadout/index.js";

let loading = null;
let loaded = false;

/**
 * Load the WebAssembly module once. In the browser, `module` is the fetched module (a
 * Response); in Node the wrapper reads it from disk.
 */
export function ready(module) {
  if (loading === null) {
    loading = load(module);
    loading.then(() => (loaded = true)).catch(() => {});
  }
  return loading;
}

/** Whether the module has loaded. */
export const isReady = () => loaded;

const settle = (p) =>
  p.then(
    (value) => ({ ok: true, value }),
    (error) => ({
      ok: false,
      error: { message: error.message, code: error.code, hint: error.hint, exitCode: error.exitCode },
    }),
  );

/**
 * Open a file (a File, Blob or bytes) and read what the page shows first: `info`, `info --view
 * explain` and `check`, each settled on its own, so a file the reader half understands still
 * shows what it could read. The returned `file` stays open for the views; close it when done.
 */
export async function openFile(source, name) {
  await ready();
  const started = performance.now();
  const file = await open(source, name ? { name } : {});
  const info = await settle(file.info());
  const explain = await settle(file.info({ view: "explain" }));
  const check = await settle(file.check());
  return {
    file,
    name: file.name,
    size: source.size ?? source.byteLength,
    info,
    explain,
    check,
    ms: Math.round(performance.now() - started),
  };
}

// ----- views ---------------------------------------------------------------------------------

/**
 * The view the page draws first: an image (with its channels blended when there are 2 to 4)
 * or a plot (traces, spectra, plates, events). `preset` (a sample's `view`) overrides it.
 */
export function firstView(info, preset = {}) {
  const v = { kind: info.images?.length ? "image" : "plot", ...preset };
  if (v.kind === "image") {
    v.image ??= 0;
    const im = info.images.find((i) => i.index === v.image) ?? info.images[0];
    if (v.channels === undefined && im.samples_per_pixel === 1 && im.size_c > 1) {
      v.channels = [...Array(Math.min(im.size_c, 4)).keys()];
    }
    if (im.size_z > 1) v.z ??= Math.floor(im.size_z / 2);
    if (im.size_t > 1) v.t ??= 0;
  } else if (v.marks === undefined && isVoltage(info, v.trace ?? 0)) {
    v.marks = "spikes";
  }
  return v;
}

function isVoltage(info, trace) {
  const t = info.traces?.find((x) => x.index === trace);
  return Boolean(t && t.sweep_count > 1 && /^m?V$/.test(t.channels?.[0]?.unit ?? ""));
}

/** The image a view shows, from `info`. */
export function imageOf(info, v) {
  return info.images?.find((i) => i.index === (v.image ?? 0)) ?? info.images?.[0];
}

/**
 * Preview options for a view, drawn `width` CSS pixels wide (images: the longer side fits
 * `width` across, or `height` down) at `scale` device pixels per CSS pixel.
 */
export function previewOptions(info, v, { width, height = Infinity, scale = 1, dark = false }) {
  const s = Math.min(4, Math.max(1, Math.round(scale * 4) / 4));
  if (v.kind === "image") {
    const im = imageOf(info, v);
    const r = v.region ?? { x: 0, y: 0, width: im.size_x, height: im.size_y };
    const fit = Math.min(width / r.width, height / r.height);
    const size = Math.max(16, Math.round(Math.max(r.width, r.height) * fit));
    const select = [];
    if (v.channels?.length) select.push(`c=${v.channels.join(",")}`);
    if (v.z !== undefined) select.push(`z=${v.z}`);
    if (v.t !== undefined) select.push(`t=${v.t}`);
    return {
      image: v.image ?? 0,
      ...(select.length ? { select } : {}),
      ...(v.channels?.length > 1 ? { composite: true } : {}),
      ...(v.region ? { region: v.region } : {}),
      ...(v.objects ? { marks: "objects" } : {}),
      axes: "none",
      max_size: size,
      scale: s,
      format: "jpeg",
      dark,
    };
  }
  const o = { max_size: Math.max(280, Math.round(width)), scale: s, format: "png", dark };
  for (const k of ["trace", "sweep", "run", "spectrum", "table", "columns", "marks", "annotations"]) {
    if (v[k] !== undefined) o[k] = v[k];
  }
  return o;
}

/** Draw a view: `{ meta, data, type, scale }`. */
export async function draw(file, info, v, size) {
  const o = previewOptions(info, v, size);
  const p = await file.preview(o);
  return { ...p, scale: o.scale };
}

/**
 * The full-resolution region that a part of an image preview shows: `box` is that part as
 * fractions of the picture (`{ x0, y0, x1, y1 }`, 0 to 1, possibly beyond when zooming out).
 * The result is kept inside the image, at least `min` pixels across, and undefined when it
 * covers the whole image.
 */
export function regionOf(meta, info, v, box, min = 48) {
  const im = imageOf(info, v);
  const r = meta.image.full_res_region;
  const cx = r.x + ((box.x0 + box.x1) / 2) * r.width;
  const cy = r.y + ((box.y0 + box.y1) / 2) * r.height;
  return fitRegion(im, cx, cy, (box.x1 - box.x0) * r.width, (box.y1 - box.y0) * r.height, min);
}

/** A region of `width` × `height` full-resolution pixels centred on (cx, cy), inside the image. */
export function fitRegion(im, cx, cy, width, height, min = 48) {
  // keep the shape when the region would be smaller than `min` across
  const grow = Math.max(1, min / Math.min(width, height));
  let w = width * grow;
  let h = height * grow;
  // keep the shape when one side hits the image edge
  const k = Math.min(im.size_x / w, im.size_y / h, 1);
  w *= k;
  h *= k;
  if (w >= im.size_x - 1 && h >= im.size_y - 1) return undefined;
  const x = Math.min(Math.max(0, cx - w / 2), im.size_x - w);
  const y = Math.min(Math.max(0, cy - h / 2), im.size_y - h);
  return { x: Math.round(x), y: Math.round(y), width: Math.round(w), height: Math.round(h) };
}

/**
 * The panel of a plot preview under a point (CSS pixels), with the data coordinates there:
 * `{ panel, x, y }`, or undefined between panels.
 */
export function pointAt(meta, px, py) {
  for (const panel of meta.panels ?? []) {
    const a = panel.plot_area;
    if (px < a.x || px > a.x + a.width || py < a.y || py > a.y + a.height) continue;
    const fx = (px - a.x) / a.width;
    const fy = 1 - (py - a.y) / a.height;
    const along = (range, f, scale) =>
      scale === "log" ? 10 ** (Math.log10(range[0]) + f * (Math.log10(range[1]) - Math.log10(range[0]))) : range[0] + f * (range[1] - range[0]);
    return { panel, x: along(panel.x, fx, panel.x_scale), y: along(panel.y, fy, panel.y_scale) };
  }
  return undefined;
}

/**
 * The scan with the most ion current within ±`window` minutes of `rt` (minutes), as the
 * `scans` command lists it.
 */
export async function scanNear(file, run, rt, window = 0.05) {
  const r = await file.scans({ run, rt_range: [Math.max(0, rt - window), rt + window], limit: 200 });
  let best;
  for (const s of r.scans ?? []) if (!best || (s.total_ion_current ?? 0) > (best.total_ion_current ?? 0)) best = s;
  return best;
}

// ----- words and numbers ---------------------------------------------------------------------

const UNITLESS = new Set(["", undefined, null]);

function value(p) {
  if (p == null) return "";
  const v = p.value ?? p;
  if (Array.isArray(v)) {
    if (v.length && typeof v[0] === "object") return v.map((x) => x.fluorophore ?? x.name ?? "").filter(Boolean).join(", ");
    return v.join(", ");
  }
  if (typeof v === "object") return "";
  const n = typeof v === "number" ? round(v) : String(v);
  return UNITLESS.has(p.unit) ? n : `${n} ${p.unit}`;
}

/** A number with at most 4 significant digits, grouped with thin spaces when large. */
export function round(n) {
  if (!Number.isFinite(n)) return String(n);
  const a = Math.abs(n);
  if (a >= 10000) return Math.round(n).toLocaleString("en-US");
  if (a >= 100) return String(Math.round(n * 10) / 10);
  return String(Number(n.toPrecision(4)));
}

/** Bytes as KB/MB. */
export function human(n) {
  if (!Number.isFinite(n)) return "";
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1000 && i < units.length - 1) {
    n /= 1000;
    i++;
  }
  return `${i === 0 ? n : n.toFixed(1)} ${units[i]}`;
}

const sentence = (s) => (s ? s[0].toUpperCase() + s.slice(1) : s);

function when(t) {
  if (!t) return "";
  const m = /^(\d{4}-\d\d-\d\d)[T ](\d\d:\d\d)/.exec(t);
  return m ? `${m[1]} ${m[2]}` : t;
}

function seconds(s) {
  if (!Number.isFinite(s)) return "";
  if (s < 120) return `${round(s)} s`;
  if (s < 7200) return `${round(s / 60)} min`;
  return `${round(s / 3600)} h`;
}

const PARAM_NAMES = {
  objective_magnification: "Objective",
  pixel_size: "Pixel size",
  sample_rate: "Sample rate",
  acquisition_mode: "Mode",
  read_modes: "Read",
  read_type: "Read type",
  temperature: "Temperature",
  ion_source: "Ion source",
  polarity: "Polarity",
  ms_levels: "MS levels",
  nucleus: "Nucleus",
  solvent: "Solvent",
  spectrometer_frequency: "Frequency",
  scans: "Scans",
  resolution: "Resolution",
  cycles: "Cycles",
  targets: "Targets",
  channels: "Channels",
  acquisition_temperature: "Acquisition at",
  cytometry_technology: "Cytometer",
};

/**
 * What the file says about the experiment, as label and value pairs: technique, instrument,
 * when, and the main settings (`info.experiment`, which `info --json` reports for every format).
 */
export function facts(info) {
  const e = info.experiment ?? {};
  const out = [];
  const add = (label, v) => v && out.push([label, String(v)]);
  add("Technique", sentence(e.method?.technique?.label ?? ""));
  const ins = e.instrument ?? {};
  const model = (ins.model ?? "").split(/,\s|\s{2,}/)[0];
  const maker = ins.vendor && !model.includes(ins.vendor) ? ins.vendor : "";
  add("Instrument", [maker, model.length > 60 ? `${model.slice(0, 57)}…` : model].filter(Boolean).join(" "));
  add("Software", [ins.software, ins.software_version].filter(Boolean).join(" ").replace(/\s*\(.*$/, "").slice(0, 60));
  add("Recorded", when(e.acquisition?.started_at));
  add("Duration", seconds(e.acquisition?.duration_s));
  for (const [k, p] of Object.entries(e.method?.parameters ?? {})) {
    if (PARAM_NAMES[k] && out.length < 12) {
      let v = value(p);
      if (k === "objective_magnification" && v) v = `${v}×`;
      add(PARAM_NAMES[k], v);
    }
  }
  return out;
}

/** One line per measurement in the file ("confocal fluorescence, 3 channels, ..."). */
export function measurements(info) {
  return (info.experiment?.measurements ?? []).map((m) => sentence(m.what)).filter(Boolean);
}

/** The short name of a format: "TIFF family (TIFF, BigTIFF, ...)" is "TIFF family". */
export function formatName(info) {
  return (info.format?.name ?? "").replace(/\s*\(.*\)$/, "");
}

/**
 * Numbers about what the picture shows, as `{ label, value }`: the size of an image region,
 * the objects found in it, the labelled peaks of a plot, the action potentials of a sweep.
 */
export function numbers(info, v, meta) {
  const out = [];
  if (meta.image) {
    const im = imageOf(info, v);
    const r = meta.image.full_res_region;
    const um = im?.physical_size?.unit === "µm" ? im.physical_size.x : undefined;
    out.push({ label: "Showing", value: `${r.width.toLocaleString("en-US")} × ${r.height.toLocaleString("en-US")} px` });
    if (um) out.push({ label: "Field of view", value: `${round(r.width * um)} × ${round(r.height * um)} µm` });
    out.push({ label: "Zoom", value: `${round(im.size_x / r.width)}×` });
    const ob = meta.objects;
    if (ob) {
      out.push({ label: `${sentence(ob.kind ?? "objects")} found`, value: ob.count.toLocaleString("en-US"), strong: true });
      if (ob.density_per_mm2) out.push({ label: "Density", value: `${round(ob.density_per_mm2)} per mm²` });
      if (ob.diameter_um?.median) out.push({ label: "Median diameter", value: `${round(ob.diameter_um.median)} µm` });
      if (ob.confidence) out.push({ label: "Confidence", value: ob.confidence });
    }
    return out;
  }
  const spikes = (meta.marks ?? []).filter((m) => m.kind === "spike");
  if (spikes.length || v.marks === "spikes") {
    out.push({ label: "Action potentials", value: String(spikes.length), strong: true });
    if (spikes.length > 1) {
      const t = spikes.map((s) => s.x).sort((a, b) => a - b);
      out.push({ label: "First at", value: `${round(t[0] * 1000)} ms` });
      const peak = Math.max(...spikes.map((s) => s.y));
      out.push({ label: "Highest peak", value: `${round(peak)} mV` });
    }
  }
  if (meta.trace && !spikes.length && v.marks !== "spikes") {
    const t = meta.trace;
    if (t.channels?.length > 1) out.push({ label: "Curves", value: String(t.channels.length), strong: true });
    if (t.sweep_sample_count) out.push({ label: "Points each", value: t.sweep_sample_count.toLocaleString("en-US") });
    if (t.x_axis) out.push({ label: "Range", value: `${round(t.x_axis.first)} to ${round(t.x_axis.last)} ${t.x_axis.unit}` });
  }
  const peaks = (meta.marks ?? []).filter((m) => m.kind === "peak" && m.label);
  if (peaks.length) {
    const unit = meta.spectrum ? "m/z" : (meta.trace?.x_axis?.unit ?? "");
    out.push({
      label: meta.spectrum ? "Largest peaks (m/z)" : `Labelled peaks${unit ? ` (${unit})` : ""}`,
      value: peaks.map((p) => p.label).join(" · "),
      wide: true,
    });
  }
  if (meta.events) {
    out.push({ label: "Events", value: meta.events.events.toLocaleString("en-US"), strong: true });
    if (meta.events.total_events !== meta.events.events) out.push({ label: "Of", value: meta.events.total_events.toLocaleString("en-US") });
  }
  if (meta.plate) {
    const p = meta.plate;
    out.push({ label: "Wells", value: String(p.wells), strong: true });
    if (p.time) out.push({ label: "Read for", value: `${round(p.time[1] - p.time[0])} ${p.time_unit}` });
    if (Number.isFinite(p.min)) out.push({ label: "Values", value: `${round(p.min)} to ${round(p.max)}` });
    if (p.groups?.length) out.push({ label: "Groups", value: String(p.groups.length) });
  }
  if (meta.spectrum) {
    const s = meta.spectrum;
    out.push({ label: "Scan", value: `${s.scan_number ?? s.index} · MS${s.ms_level ?? ""}`.replace(/ · MS$/, "") });
    if (Number.isFinite(s.rt_s)) out.push({ label: "Retention time", value: `${round(s.rt_s / 60)} min` });
  }
  return out;
}

/** The traces of a file, for a picker: `[{ index, name, sweeps }]`. */
export function traceChoices(info) {
  return (info.traces ?? []).map((t) => ({ index: t.index, name: t.name || `Recording ${t.index + 1}`, sweeps: t.sweep_count ?? 1 }));
}

/**
 * The parameters of a flow-cytometry table for the axis pickers: `[{ name, label }]`, the
 * label being the stain when the file names one ("CD4 (PacBlu-A)").
 */
export function parameterChoices(info, table = 0) {
  const t = info.tables?.find((x) => x.index === table);
  return (t?.columns ?? [])
    .filter((c) => c.name !== "Time" && !/^time$/i.test(c.name))
    .map((c) => ({ name: c.name, label: c.label && c.label !== c.name ? `${c.label} (${c.name})` : c.name }));
}

/**
 * Cq values by sample from a qPCR results table (a `cq` column with `sample` codes), mean of
 * the wells that have one: `[{ sample, cq, wells }]`, lowest Cq first. Empty when the file has
 * no such table.
 */
export async function cqBySample(file, info) {
  const t = info.tables?.find((x) => x.columns?.some((c) => c.name === "cq") && x.columns.some((c) => c.name === "sample"));
  if (!t) return [];
  const slice = await file.table(t.index, { maxRows: Math.min(t.row_count, 5000) });
  const col = (n) => slice.columns.indexOf(n);
  const [ci, si] = [col("cq"), col("sample")];
  const names = t.columns.find((c) => c.name === "sample")?.extra?.categories ?? [];
  const by = new Map();
  for (const row of slice.rows) {
    const cq = row[ci];
    const name = names[row[si]] ?? String(row[si]);
    if (!Number.isFinite(cq)) continue;
    const e = by.get(name) ?? { sample: name, sum: 0, wells: 0 };
    e.sum += cq;
    e.wells++;
    by.set(name, e);
  }
  return [...by.values()].map((e) => ({ sample: e.sample, cq: e.sum / e.wells, wells: e.wells })).sort((a, b) => a.cq - b.cq);
}

/**
 * Per-well series of a kinetic plate table (columns `well`, `time_s`, `value`): a map from
 * well name to `{ first, last, max, reads }`.
 */
export async function wellSeries(file, info, table = 0) {
  const t = info.tables?.find((x) => x.index === table);
  if (!t || !t.columns.some((c) => c.name === "time_s")) return new Map();
  const slice = await file.table(table, { maxRows: Math.min(t.row_count, 100000) });
  const col = (n) => slice.columns.indexOf(n);
  const [wi, ti, vi] = [col("well"), col("time_s"), col("value")];
  const names = t.columns.find((c) => c.name === "well")?.extra?.categories ?? [];
  const out = new Map();
  for (const row of slice.rows) {
    const name = names[row[wi]] ?? String(row[wi]);
    const v = row[vi];
    if (!Number.isFinite(v)) continue;
    const e = out.get(name) ?? { first: v, firstT: row[ti], last: v, lastT: row[ti], max: v, reads: 0 };
    if (row[ti] < e.firstT) [e.first, e.firstT] = [v, row[ti]];
    if (row[ti] >= e.lastT) [e.last, e.lastT] = [v, row[ti]];
    e.max = Math.max(e.max, v);
    e.reads++;
    out.set(name, e);
  }
  return out;
}

/** The command that draws the same picture with the command-line tool. */
export function command(name, info, v) {
  const q = (s) => (/^[\w./-]+$/.test(s) ? s : `'${s.replace(/'/g, "'\\''")}'`);
  const a = ["openreadout", "preview", q(name)];
  if (v.kind === "image") {
    if (v.image) a.push("--image", String(v.image));
    if (v.channels?.length) a.push("--select", `c=${v.channels.join(",")}`);
    if (v.channels?.length > 1) a.push("--composite");
    if (v.z !== undefined) a.push("--select", `z=${v.z}`);
    if (v.region) a.push("--region", [v.region.x, v.region.y, v.region.width, v.region.height].join(","));
    if (v.objects) a.push("--marks", "objects");
  } else {
    if (v.trace !== undefined) a.push("--trace", String(v.trace));
    if (v.sweep !== undefined) a.push("--sweep", String(v.sweep));
    if (v.spectrum !== undefined) a.push("--spectrum", String(v.spectrum));
    for (const c of v.columns ?? []) a.push("--column", q(c));
    if (v.marks) a.push("--marks", v.marks);
  }
  return a.join(" ");
}

/**
 * Everything the page shows for a file in one call, as plain data (used by the tests and by
 * the page's first look at a dropped file).
 */
export async function analyze(source, name) {
  const r = await openFile(source, name);
  try {
    const view = r.info.ok ? firstView(r.info.value) : { kind: "plot" };
    const preview = r.info.ok
      ? await settle(draw(r.file, r.info.value, view, { width: 640 }))
      : { ok: false, error: r.info.error };
    return { ...r, file: undefined, view, preview };
  } finally {
    r.file.close();
  }
}
