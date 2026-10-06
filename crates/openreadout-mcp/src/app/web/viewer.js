// OpenReadout viewer: an MCP App (io.modelcontextprotocol/ui) that draws what openreadout_view
// returns. It talks to the host with JSON-RPC over postMessage, loads nothing from the network
// and keeps no state between sessions. Plain browser JavaScript with no dependencies, so the
// server embeds this file as it is (see crates/openreadout-mcp/MAINTAINING.md).
"use strict";

const VIEW_TOOL = "openreadout_view";
const PROTOCOL_VERSION = "2026-01-26";
const VIEW_LABELS = {
  image: "Image", nmr: "NMR", chromatogram: "Chromatogram", spectrum: "Spectrum",
  fcs: "Events", plate: "Plate", trace: "Trace", summary: "Summary",
};
// Series colours that read on light and dark backgrounds.
const PALETTE = ["#2f6fdf", "#e0612f", "#22a06b", "#b5479f", "#c79a17", "#3aa0c8", "#7a5bd6", "#d14a5a"];
// Viridis, sampled at nine stops.
const VIRIDIS = [
  [68, 1, 84], [71, 44, 122], [59, 81, 139], [44, 113, 142], [33, 144, 141],
  [39, 173, 129], [92, 200, 99], [170, 220, 50], [253, 231, 37],
];
// Keys that select a zoomed window; a new selection or view clears them.
const ZOOM_KEYS = ["region", "first_sample", "count", "x_range"];

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
    out.push(Math.abs(v) < step * 1e-9 ? 0 : v);
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

/** Viridis colour for t in [0, 1]. */
function viridis(t) {
  t = Math.min(1, Math.max(0, isFinite(t) ? t : 0));
  const f = t * (VIRIDIS.length - 1);
  const i = Math.min(VIRIDIS.length - 2, Math.floor(f));
  const u = f - i;
  const a = VIRIDIS[i], b = VIRIDIS[i + 1];
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

/** Decode little-endian float32 values from base64. */
function f32FromBase64(b64) {
  if (!b64) return new Float32Array(0);
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Float32Array(bytes.buffer, 0, bytes.length >> 2);
}

/** Display transform of flow-cytometry values. */
function scaleFn(name) {
  if (name === "log") return (v) => (v > 0 ? Math.log10(v) : NaN);
  if (name === "arcsinh") return (v) => Math.asinh(v / 150);
  return (v) => v;
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
  const ctx2d = canvas.getContext("2d");
  const S = {
    host: null,          // connection
    hostCaps: {},
    hostContext: {},
    toolInput: null,
    args: null,          // current openreadout_view arguments
    data: null,          // current openreadout_view structuredContent
    picture: null,       // { img, mime } for image views
    outline: null,
    file: null,
    seq: 0,
    layout: null,        // geometry of the last drawing, for hit tests
    drag: null,
    fcsScale: {},        // FCS display scale per parameter
  };

  // ---------- host plumbing

  const host = connect(onNotification, async (method) => {
    if (method === "ui/resource-teardown") return {};
    if (method === "ping") return {};
    throw new Error("unsupported: " + method);
  });
  S.host = host;

  function applyContext(c) {
    if (!c) return;
    const before = JSON.stringify([S.hostContext.theme, S.hostContext.displayMode, S.hostContext.containerDimensions, S.hostContext.styles]);
    Object.assign(S.hostContext, c);
    const changed = before !== JSON.stringify([S.hostContext.theme, S.hostContext.displayMode, S.hostContext.containerDimensions, S.hostContext.styles]);
    if (!changed && S.data) return;
    if (c.theme) {
      root.setAttribute("data-theme", c.theme);
      root.style.colorScheme = c.theme;
    }
    const vars = c.styles && c.styles.variables;
    if (vars) {
      for (const [k, v] of Object.entries(vars)) if (v) root.style.setProperty(k, v);
    }
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
    redraw();
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

  function canCallTools() {
    return !!(S.hostCaps && S.hostCaps.serverTools);
  }

  async function onToolResult(result) {
    const sc = result && result.structuredContent;
    if (result && result.isError) {
      overlay(textOf(result) || "The tool call failed.", true);
      return;
    }
    // The host opened a file with the viewer (file entrypoint): the result is already ours.
    if (sc && sc.view && sc.file && sc.view !== "pending" && (sc.plot || sc.image || sc.plate || sc.events || sc.outline)) {
      S.args = hintFrom(null, S.toolInput) || {};
      show(result);
      return;
    }
    const hint = sc && sc.view === "pending" ? hintFrom(null, S.toolInput) : hintFrom(result, S.toolInput);
    if (!hint) {
      overlay(textOf(result) || "Nothing to show for this result.");
      return;
    }
    if (!canCallTools()) {
      showStatic(result);
      return;
    }
    S.args = Object.assign({ outline: true }, hint);
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
    const img = new Image();
    img.onload = () => {
      S.data = { view: "static" };
      S.picture = { img };
      overlay(null);
      redraw();
    };
    img.src = `data:${im.mimeType};base64,${im.data}`;
  }

  async function callView(args) {
    const r = await host.request("tools/call", { name: VIEW_TOOL, arguments: args });
    if (r && r.isError) throw new Error(textOf(r) || "openreadout_view failed");
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
    const w = Math.round(Math.max(256, Math.min(1600, canvas.clientWidth * (window.devicePixelRatio || 1))));
    args.max_size = Math.ceil(w / 64) * 64;
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
      show(r);
    } catch (e) {
      if (seq !== S.seq) return;
      const hint = e && e.data && e.data.hint;
      overlay((e && e.message ? e.message : String(e)) + (hint ? "\n" + hint : ""), true);
    } finally {
      if (seq === S.seq) root.setAttribute("aria-busy", "false");
    }
  }

  function show(result) {
    const d = result.structuredContent || {};
    if (d.outline) S.outline = d.outline;
    if (d.file) S.file = d.file;
    S.data = d;
    S.args = S.args || {};
    tip(null);
    $("readout").textContent = "";
    if (d.file && !S.args.file) S.args.file = d.file.path;
    S.args.view = d.view;
    setTitle(S.file && S.file.name, S.file ? S.file.format_name : "");
    renderTabs();
    renderControls();
    const notes = (d.notes || []).slice(0, 3);
    $("notes").textContent = notes.join(" · ");
    $("notes").title = (d.notes || []).join("\n");
    const im = (result.content || []).find((c) => c.type === "image");
    if (im) {
      const img = new Image();
      img.onload = () => {
        S.picture = { img };
        overlay(null);
        redraw();
        tellModel();
      };
      img.onerror = () => overlay("Could not decode the picture.", true);
      img.src = `data:${im.mimeType};base64,${im.data}`;
    } else {
      S.picture = null;
      overlay(null);
      redraw();
      tellModel();
    }
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
  }

  // Tell the model what the user is looking at, so "what is this?" has an answer.
  let tellTimer = 0;
  function tellModel() {
    clearTimeout(tellTimer);
    tellTimer = setTimeout(() => {
      const text = describe();
      if (!text) return;
      host.request("ui/update-model-context", { content: [{ type: "text", text }] }, 10000).catch(() => {});
    }, 800);
  }

  function describe() {
    const d = S.data;
    if (!d || !S.file) return "";
    const a = S.args || {};
    let what = d.view;
    if (d.view === "image" && d.image) {
      const im = d.image;
      const r = im.full_res_region;
      what = `image ${im.image}, ${im.composite ? "composite of channels " + im.c.join(",") : "channel " + im.c.join(",")}` +
        `, z ${im.projection ? "max projection" : im.z.join(",")}, t ${im.t.join(",")}, level ${im.level}` +
        (r ? `, region x ${r.x}–${r.x + r.width}, y ${r.y}–${r.y + r.height} (full-resolution px)` : "");
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
      what = `flow-cytometry events, ${d.events.x.name}` + (d.events.y ? ` vs ${d.events.y.name}` : " histogram");
    } else if (d.plate) {
      what = `plate heat map of ${d.plate.value}`;
    }
    return `The user is looking at ${S.file.path} in the OpenReadout viewer: ${what}.`;
  }

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
        for (const k of ["run", "table", "image"]) if (S.args[k] !== undefined) keep[k] = S.args[k];
        S.args = keep;
        load();
      };
      tabs.appendChild(b);
    }
  }

  function el(tag, props, ...children) {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(props || {})) {
      if (k === "on") for (const [ev, fn] of Object.entries(v)) e.addEventListener(ev, fn);
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

  function button(text, onClick, disabled) {
    return el("button", { type: "button", textContent: text, disabled: !!disabled, on: { click: onClick } });
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
      const info = (o.images || []).find((x) => x.index === im.image) || {};
      if ((o.images || []).length > 1) {
        add(select("Image", o.images.map((x) => [x.index, `${x.index}${x.name ? " · " + x.name : ""}`]), im.image, (v) => load({ image: Number(v), c: undefined, z: undefined, t: undefined, level: undefined, composite: undefined })));
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
          v === "all" ? load({ composite: true, channels: undefined, c: undefined }, { keepZoom: true }) : load({ composite: undefined, c: Number(v) }, { keepZoom: true })));
      }
      if ((info.size_z || 1) > 1) {
        if (!im.projection) add(slider("Z", info.size_z - 1, im.z[0] || 0, (v) => load({ z: v }, { keepZoom: true })));
        const mip = el("input", { type: "checkbox", checked: !!im.projection, on: { change: () => load({ mip: mip.checked || undefined }, { keepZoom: true }) } });
        add(el("label", {}, mip, "Max projection"));
      }
      if ((info.size_t || 1) > 1) add(slider("T", info.size_t - 1, im.t[0] || 0, (v) => load({ t: v }, { keepZoom: true })));
      if ((info.levels || []).length > 1) {
        const opts = [["", "Auto"]].concat(info.levels.map((l, i) => [i, `${i} · ${l[0]}×${l[1]}`]));
        add(select("Level", opts, S.args.level ?? "", (v) => load({ level: v === "" ? undefined : Number(v) }, { keepZoom: true })));
      }
      add(select("Contrast", [["auto", "Auto"], ["min-max", "Min–max"], ["percentile:1,99", "1–99 %"], ["percentile:0.1,99.9", "0.1–99.9 %"], ["raw", "Full range"]],
        S.args.contrast || "auto", (v) => load({ contrast: v === "auto" ? undefined : v }, { keepZoom: true })));
      add(button("Reset zoom", () => load({}), !zoomed));
    } else if (d.plot && d.plot.kind === "trace") {
      const p = d.plot;
      const traces = o.traces || [];
      if (traces.length > 1) add(select("Trace", traces.map((t) => [t.index, `${t.index}${t.name ? " · " + t.name : ""}`]), p.trace, (v) => load({ trace: Number(v), sweep: undefined, channels: undefined })));
      if (p.sweep_count > 1) {
        add(button("‹", () => load({ sweep: p.sweep - 1 }), p.sweep <= 0));
        add(slider("Sweep", p.sweep_count - 1, p.sweep, (v) => load({ sweep: v })));
        add(button("›", () => load({ sweep: p.sweep + 1 }), p.sweep >= p.sweep_count - 1));
      }
      const t = traces.find((x) => x.index === p.trace);
      if (t && t.channel_count > 1) {
        // One chip per channel; the plotted ones are pressed. At most 8 at once.
        const shown = p.series.map((s) => s.channel);
        const chips = el("span", { className: "chips" });
        t.channels.forEach((ch, c) => {
          const on = shown.includes(c);
          const chip = el("span", { className: "chip", role: "button", tabIndex: 0 },
            el("span", { className: "swatch", style: `background:${PALETTE[c % PALETTE.length]}` }), ch.name || `ch ${c}`);
          chip.setAttribute("aria-pressed", String(on));
          const toggle = () => {
            const next = on ? shown.filter((x) => x !== c) : shown.concat([c]).sort((a, b) => a - b);
            if (!next.length || next.length > 8) return;
            load({ channels: next }, { keepZoom: true });
          };
          chip.addEventListener("click", toggle);
          chip.addEventListener("keydown", (e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), toggle()));
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
      if (p.source === "spectra") add(el("span", { className: "muted", textContent: "Click a point for its spectrum" }));
    } else if (d.plot && d.plot.kind === "spectrum") {
      const p = d.plot, s = p.spectrum;
      add(button("‹", () => load({ index: s.index - 1, rt_min: undefined, scan: undefined }), s.index <= 0));
      add(el("span", { className: "value", textContent: `scan ${s.scan_number} · MS${s.ms_level}${s.rt_min != null ? " · " + fmt(s.rt_min, 4) + " min" : ""}${s.precursor_mz ? " · precursor " + fmt(s.precursor_mz, 7) : ""}` }));
      add(button("›", () => load({ index: s.index + 1, rt_min: undefined, scan: undefined }), s.index >= p.scan_count - 1));
      add(button("Chromatogram", () => { S.args = { file: S.args.file, view: "chromatogram", run: p.run }; load(); }));
      add(button("Reset zoom", () => load({}, { keepZoom: false }), !zoomed));
    } else if (d.plate) {
      const tables = (o.tables || []).filter((t) => t.plate);
      if (tables.length > 1) add(select("Table", tables.map((t) => [t.index, `${t.index}${t.name ? " · " + t.name : ""}`]), d.plate.table, (v) => load({ table: Number(v), column: undefined })));
      const t = (o.tables || []).find((x) => x.index === d.plate.table);
      if (t && d.plate.layout === "long") {
        const skip = new Set(["well", "row", "column", "col", "read", "wavelength_nm", "time_s"]);
        const cols = t.columns.filter((c) => !skip.has(c.name.toLowerCase()));
        if (cols.length > 1) add(select("Value", cols.map((c) => [c.name, c.label || c.name]), S.args.column || cols[0].name, (v) => load({ column: v })));
      }
      add(el("span", { className: "muted", textContent: d.plate.value }));
    } else if (d.events) {
      const ev = d.events;
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
          redraw();
        }));
      }
      add(select("Events", [[10000, "10 000"], [25000, "25 000"], [50000, "50 000"]], S.args.events || 10000, (v) => load({ events: Number(v) })));
    }
  }

  function fcsScaleOf(name) {
    if (S.fcsScale[name]) return S.fcsScale[name];
    return /^(FSC|SSC|Time)/i.test(name) ? "linear" : "arcsinh";
  }

  // ---------- drawing

  // Canvas cannot parse light-dark() or var(): let the browser resolve each colour.
  const probe = document.createElement("span");
  probe.style.display = "none";
  document.body.appendChild(probe);
  function colors() {
    const cs = getComputedStyle(root);
    const v = (k, f) => {
      if (k.startsWith("--font")) return (cs.getPropertyValue(k) || "").trim() || f;
      probe.style.color = f;
      probe.style.color = `var(${k}, ${f})`;
      return getComputedStyle(probe).color || f;
    };
    return {
      text: v("--color-text-primary", "#222"),
      muted: v("--color-text-secondary", "#666"),
      faint: v("--color-text-tertiary", "#999"),
      grid: v("--color-border-secondary", "#e5e5e5"),
      axis: v("--color-border-primary", "#ccc"),
      bg: v("--color-background-secondary", "#f5f5f5"),
      font: v("--font-sans", "system-ui, sans-serif"),
    };
  }

  /** Size the canvas for its box; returns CSS width and height. */
  function sizeCanvas(aspect) {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(160, canvas.parentElement.clientWidth);
    let h;
    if (root.classList.contains("fill")) {
      const stage = canvas.parentElement.getBoundingClientRect();
      h = Math.max(160, window.innerHeight - stage.top - 34);
    } else {
      const maxH = (S.hostContext.containerDimensions && S.hostContext.containerDimensions.maxHeight) || 0;
      h = aspect ? w / aspect : Math.round(Math.min(520, Math.max(220, w * 0.55)));
      h = Math.min(h, maxH ? maxH - 120 : 640);
      h = Math.max(160, h);
    }
    canvas.style.height = h + "px";
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    ctx2d.setTransform(dpr, 0, 0, dpr, 0, 0);
    return [w, h];
  }

  function redraw() {
    const d = S.data;
    S.layout = null;
    if (!d) return;
    if (d.view === "image" || d.view === "static") return drawImage();
    if (d.plot) return drawPlot();
    if (d.plate) return drawPlate();
    if (d.events) return drawEvents();
    return drawSummary();
  }

  function drawSummary() {
    const [w, h] = sizeCanvas();
    const c = colors();
    ctx2d.clearRect(0, 0, w, h);
    ctx2d.fillStyle = c.muted;
    ctx2d.font = `13px ${c.font}`;
    const o = S.outline || {};
    const lines = [
      `${(o.images_total || 0)} images · ${(o.traces_total || 0)} traces · ${(o.spectra || []).length} spectrum runs · ${(o.tables_total || 0)} tables`,
      "This file holds nothing the viewer draws. Ask the agent for its metadata.",
    ].concat((o.notes || []).slice(0, 4));
    lines.forEach((l, i) => ctx2d.fillText(l, 16, 28 + i * 20));
  }

  // ----- image

  function drawImage() {
    const pic = S.picture;
    if (!pic) return;
    const img = pic.img;
    const box = canvas.parentElement.clientWidth;
    const fill = root.classList.contains("fill");
    let aspect = img.width / img.height;
    if (!fill) {
      const maxH = Math.min(640, ((S.hostContext.containerDimensions || {}).maxHeight || 760) - 120);
      if (box / aspect > maxH) aspect = box / maxH;
    }
    const [w, h] = sizeCanvas(fill ? null : aspect);
    const c = colors();
    ctx2d.clearRect(0, 0, w, h);
    const scale = Math.min(w / img.width, h / img.height);
    const dw = img.width * scale, dh = img.height * scale;
    const ox = (w - dw) / 2, oy = (h - dh) / 2;
    ctx2d.imageSmoothingEnabled = scale < 1.5;
    ctx2d.drawImage(img, ox, oy, dw, dh);
    S.layout = { kind: "image", ox, oy, dw, dh, iw: img.width, ih: img.height };
    const im = S.data.image;
    if (!im) return;
    // Scale bar from the physical pixel size.
    const info = ((S.outline || {}).images || []).find((x) => x.index === im.image) || {};
    const ps = info.physical_size || {};
    const reg = im.full_res_region;
    if (ps.x && reg) {
      const umPerCss = (reg.width * ps.x) / dw;
      const nice = niceBelow(umPerCss * dw * 0.22);
      const len = nice / umPerCss;
      if (len > 12) {
        const unit = ps.unit && ps.unit !== "µm" && ps.unit !== "um" ? ps.unit : "µm";
        const label = `${fmt(nice, 3)} ${unit}`;
        const x1 = ox + dw - 12, x0 = x1 - len, y = oy + dh - 14;
        ctx2d.font = `12px ${c.font}`;
        ctx2d.lineWidth = 4;
        ctx2d.strokeStyle = "rgba(0,0,0,0.55)";
        ctx2d.beginPath(); ctx2d.moveTo(x0, y); ctx2d.lineTo(x1, y); ctx2d.stroke();
        ctx2d.lineWidth = 2;
        ctx2d.strokeStyle = "#fff";
        ctx2d.beginPath(); ctx2d.moveTo(x0, y); ctx2d.lineTo(x1, y); ctx2d.stroke();
        ctx2d.textAlign = "right";
        ctx2d.lineWidth = 3;
        ctx2d.strokeStyle = "rgba(0,0,0,0.6)";
        ctx2d.strokeText(label, x1, y - 6);
        ctx2d.fillStyle = "#fff";
        ctx2d.fillText(label, x1, y - 6);
        ctx2d.textAlign = "left";
      }
    }
    // Channel legend for composites.
    if (im.composite && im.channels && im.channels.length) {
      ctx2d.font = `12px ${c.font}`;
      let y = oy + 16;
      for (const ch of im.channels.slice(0, 8)) {
        const label = `${ch.name || "ch " + ch.index}`;
        ctx2d.fillStyle = "rgba(0,0,0,0.5)";
        ctx2d.fillRect(ox + 8, y - 11, ctx2d.measureText(label).width + 22, 16);
        ctx2d.fillStyle = ch.color;
        ctx2d.fillRect(ox + 12, y - 7, 8, 8);
        ctx2d.fillStyle = "#fff";
        ctx2d.fillText(label, ox + 25, y + 1);
        y += 19;
      }
    }
  }

  function imageCoords(px, py) {
    const L = S.layout, im = S.data && S.data.image;
    if (!L || L.kind !== "image" || !im) return null;
    const u = (px - L.ox) / L.dw, v = (py - L.oy) / L.dh;
    if (u < 0 || v < 0 || u > 1 || v > 1) return null;
    const r = im.full_res_region;
    return { x: r.x + u * r.width, y: r.y + v * r.height };
  }

  // ----- plots

  function plotModel() {
    const p = S.data.plot;
    const panels = [];
    const series = p.series || [];
    const n0 = series.length ? (series[0].y || series[0].lo || []).length : 0;
    let xs = null;
    const prepared = series.map((s, i) => {
      const n = (s.y || s.lo || []).length;
      let x;
      if (Array.isArray(s.x)) x = Float64Array.from(s.x, (v) => (v == null ? NaN : v));
      else x = xs && xs.length === n ? xs : (xs = plotXs(p, n || n0));
      const color = PALETTE[(s.channel ?? i) % PALETTE.length];
      return { name: s.name, unit: s.unit, color, x, y: s.y, lo: s.lo, hi: s.hi, style: s.style || (p.kind === "spectrum" ? "profile" : "line"), channel: s.channel };
    });
    if (p.kind === "trace" && !p.overlay) {
      for (const s of prepared) panels.push({ series: [s], label: s.name + (s.unit ? ` (${s.unit})` : "") });
    } else {
      const unit = p.y_unit || (prepared[0] && prepared[0].unit);
      panels.push({ series: prepared, label: unit || (p.kind === "spectrum" ? "intensity" : "") });
    }
    const ax = p.x || {};
    const xLabel = [ax.quantity && String(ax.quantity).replace(/_/g, " "), ax.unit && `(${ax.unit})`].filter(Boolean).join(" ");
    return { panels, xLabel, reversed: !!ax.reversed, kind: p.kind };
  }

  function drawPlot() {
    const [w, h] = sizeCanvas();
    const c = colors();
    const m = plotModel();
    ctx2d.clearRect(0, 0, w, h);
    ctx2d.font = `11px ${c.font}`;
    const left = 62, right = 14, top = 10, bottom = 38, gap = 10;
    const np = Math.max(1, m.panels.length);
    const ph = (h - top - bottom - gap * (np - 1)) / np;
    // x extent over every series
    let x0 = Infinity, x1 = -Infinity;
    for (const pn of m.panels) for (const s of pn.series) for (const v of s.x) if (isFinite(v)) { if (v < x0) x0 = v; if (v > x1) x1 = v; }
    const xr = S.data.plot.x_range;
    if (xr && (m.kind === "chromatogram" || m.kind === "spectrum")) { x0 = Math.min(xr[0], xr[1]); x1 = Math.max(xr[0], xr[1]); }
    if (!isFinite(x0)) { x0 = 0; x1 = 1; }
    if (x1 === x0) { x0 -= 0.5; x1 += 0.5; }
    const pw = w - left - right;
    const X = (v) => left + (m.reversed ? (x1 - v) : (v - x0)) / (x1 - x0) * pw;
    const panelsGeo = [];
    m.panels.forEach((pn, pi) => {
      const py = top + pi * (ph + gap);
      let y0 = Infinity, y1 = -Infinity;
      for (const s of pn.series) {
        for (const arr of [s.y, s.lo, s.hi]) if (arr) for (const v of arr) if (v != null && isFinite(v)) { if (v < y0) y0 = v; if (v > y1) y1 = v; }
      }
      if (m.kind === "spectrum" || m.kind === "chromatogram") y0 = Math.min(0, y0);
      if (!isFinite(y0)) { y0 = 0; y1 = 1; }
      if (y1 === y0) { y0 -= 1; y1 += 1; }
      const pad = (y1 - y0) * 0.06;
      y1 += pad;
      if (y0 !== 0) y0 -= pad;
      const Y = (v) => py + ph - (v - y0) / (y1 - y0) * ph;
      // frame and grid
      ctx2d.strokeStyle = c.grid;
      ctx2d.lineWidth = 1;
      ctx2d.fillStyle = c.muted;
      ctx2d.textAlign = "right";
      ctx2d.textBaseline = "middle";
      const yt = niceTicks(y0, y1, Math.max(3, Math.floor(ph / 40)));
      for (const t of yt) {
        const yy = Math.round(Y(t)) + 0.5;
        ctx2d.beginPath(); ctx2d.moveTo(left, yy); ctx2d.lineTo(left + pw, yy); ctx2d.stroke();
        ctx2d.fillText(fmt(t, 3), left - 6, yy);
      }
      ctx2d.save();
      ctx2d.translate(12, py + ph / 2);
      ctx2d.rotate(-Math.PI / 2);
      ctx2d.textAlign = "center";
      ctx2d.fillStyle = c.muted;
      ctx2d.fillText(trimLabel(pn.label, ph), 0, 0);
      ctx2d.restore();
      ctx2d.save();
      ctx2d.beginPath();
      ctx2d.rect(left, py, pw, ph);
      ctx2d.clip();
      // peaks under the data
      const peaks = (S.data.plot.peaks || []);
      if (pi === 0 && m.kind === "chromatogram" && peaks.length <= 15) {
        ctx2d.fillStyle = "rgba(127,127,127,0.13)";
        for (const pk of peaks) if (pk.from != null && pk.to != null) {
          const a = X(pk.from), b = X(pk.to);
          ctx2d.fillRect(Math.min(a, b), py, Math.abs(b - a), ph);
        }
      }
      for (const s of pn.series) drawSeries(s, X, Y, py + ph);
      ctx2d.restore();
      // peak labels
      if (pi === 0 && peaks.length) drawPeakLabels(peaks, X, Y, py, c);
      if (pi === 0 && m.kind === "spectrum" && pn.series[0]) drawPeakLabels(spectrumPeaks(pn.series[0], 12), X, Y, py, c);
      panelsGeo.push({ py, ph, y0, y1, Y, series: pn.series });
    });
    // x axis
    const yAxis = top + np * ph + (np - 1) * gap;
    ctx2d.strokeStyle = c.axis;
    ctx2d.beginPath(); ctx2d.moveTo(left, yAxis + 0.5); ctx2d.lineTo(left + pw, yAxis + 0.5); ctx2d.stroke();
    ctx2d.fillStyle = c.muted;
    ctx2d.textAlign = "center";
    ctx2d.textBaseline = "top";
    for (const t of niceTicks(x0, x1, Math.max(2, Math.floor(pw / 90)))) {
      const xx = Math.round(X(t)) + 0.5;
      ctx2d.beginPath(); ctx2d.moveTo(xx, yAxis); ctx2d.lineTo(xx, yAxis + 4); ctx2d.stroke();
      ctx2d.fillText(fmt(t, 5), xx, yAxis + 6);
    }
    ctx2d.fillText(m.xLabel, left + pw / 2, yAxis + 21);
    // legend for several series in one panel
    const first = m.panels[0];
    if (first && first.series.length > 1) {
      ctx2d.textAlign = "left";
      ctx2d.textBaseline = "middle";
      let lx = left + 8, ly = top + 9;
      for (const s of first.series.slice(0, 8)) {
        const label = s.name || "";
        const lw = ctx2d.measureText(label).width + 22;
        if (lx + lw > left + pw) { lx = left + 8; ly += 15; }
        ctx2d.fillStyle = s.color;
        ctx2d.fillRect(lx, ly - 1, 10, 3);
        ctx2d.fillStyle = c.text;
        ctx2d.fillText(label, lx + 14, ly);
        lx += lw + 6;
      }
    }
    S.layout = { kind: "plot", left, pw, top, x0, x1, X, reversed: m.reversed, panels: panelsGeo, bottom: yAxis };
  }

  function trimLabel(s, room) {
    s = s || "";
    const max = Math.max(4, Math.floor(room / 6.5));
    return s.length > max ? s.slice(0, max - 1) + "…" : s;
  }

  function drawSeries(s, X, Y, base) {
    ctx2d.strokeStyle = s.color;
    ctx2d.fillStyle = s.color;
    ctx2d.lineWidth = 1.25;
    ctx2d.lineJoin = "round";
    const n = s.x.length;
    if (s.style === "sticks") {
      ctx2d.beginPath();
      for (let k = 0; k < n; k++) {
        const v = s.y[k];
        if (v == null || !isFinite(s.x[k])) continue;
        const xx = Math.round(X(s.x[k])) + 0.5;
        ctx2d.moveTo(xx, Y(Math.max(0, Math.min(v, 0))));
        ctx2d.lineTo(xx, Y(v));
      }
      ctx2d.stroke();
      return;
    }
    if (s.lo && s.hi) {
      // min/max envelope: the band between the extremes of each slice
      ctx2d.beginPath();
      let open = false;
      const flush = (from, to) => {
        if (to < from) return;
        ctx2d.moveTo(X(s.x[from]), Y(s.hi[from]));
        for (let k = from + 1; k <= to; k++) ctx2d.lineTo(X(s.x[k]), Y(s.hi[k]));
        for (let k = to; k >= from; k--) ctx2d.lineTo(X(s.x[k]), Y(s.lo[k]));
        ctx2d.closePath();
      };
      let start = -1;
      for (let k = 0; k <= n; k++) {
        const ok = k < n && s.lo[k] != null && s.hi[k] != null && isFinite(s.x[k]);
        if (ok && start < 0) start = k;
        if (!ok && start >= 0) { flush(start, k - 1); start = -1; open = true; }
      }
      ctx2d.globalAlpha = 0.9;
      ctx2d.fill();
      ctx2d.lineWidth = 1;
      ctx2d.stroke();
      ctx2d.globalAlpha = 1;
      return open;
    }
    ctx2d.beginPath();
    let pen = false;
    for (let k = 0; k < n; k++) {
      const v = s.y[k];
      if (v == null || !isFinite(s.x[k])) { pen = false; continue; }
      const xx = X(s.x[k]), yy = Y(v);
      if (pen) ctx2d.lineTo(xx, yy);
      else ctx2d.moveTo(xx, yy);
      pen = true;
    }
    ctx2d.stroke();
    if (base !== undefined && S.data.plot.kind === "chromatogram" && s.y) {
      // light fill under a single chromatogram
      if ((S.data.plot.series || []).length === 1) {
        ctx2d.lineTo(X(s.x[n - 1]), base);
        ctx2d.lineTo(X(s.x[0]), base);
        ctx2d.globalAlpha = 0.08;
        ctx2d.fill();
        ctx2d.globalAlpha = 1;
      }
    }
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

  function drawPeakLabels(peaks, X, Y, py, c) {
    ctx2d.font = `10px ${colors().font}`;
    ctx2d.textAlign = "center";
    ctx2d.textBaseline = "bottom";
    const sorted = peaks.slice().sort((a, b) => (b.height || 0) - (a.height || 0));
    const used = [];
    for (const pk of sorted) {
      const xx = X(pk.x);
      if (!isFinite(xx)) continue;
      const label = S.data.plot.kind === "nmr" ? fmt(pk.x, 4) : String(pk.label ?? "");
      if (!label) continue;
      const half = ctx2d.measureText(label).width / 2 + 3;
      const L = { lo: 62 + half, hi: canvas.clientWidth - 14 - half };
      if (xx < L.lo - half || xx > L.hi + half) continue;
      const tx = Math.min(L.hi, Math.max(L.lo, xx));
      if (used.some(([a, b]) => xx + half > a && xx - half < b)) continue;
      used.push([xx - half, xx + half]);
      const yy = pk.height != null ? Math.max(py + 12, Y(pk.height) - 3) : py + 12;
      ctx2d.fillStyle = c.text;
      ctx2d.fillText(label, tx, yy);
    }
  }

  // ----- plate

  function drawPlate() {
    const pl = S.data.plate;
    const left = 26, top = 22, legend = 34;
    const boxW = Math.max(160, canvas.parentElement.clientWidth);
    const fitCell = Math.max(4, Math.min(56, (boxW - left - 10) / pl.columns));
    const [w, h] = sizeCanvas(root.classList.contains("fill") ? null : boxW / (top + pl.rows * fitCell + legend + 10));
    const c = colors();
    ctx2d.clearRect(0, 0, w, h);
    const cell = Math.max(4, Math.min(fitCell, (h - top - legend - 8) / pl.rows));
    const gw = cell * pl.columns;
    const ox = left + Math.max(0, (w - left - 10 - gw) / 2);
    let lo = Infinity, hi = -Infinity;
    for (const [, , v] of pl.cells) if (isFinite(v)) { lo = Math.min(lo, v); hi = Math.max(hi, v); }
    ctx2d.font = `${Math.min(12, Math.max(8, cell * 0.45))}px ${c.font}`;
    ctx2d.fillStyle = c.muted;
    ctx2d.textAlign = "center";
    ctx2d.textBaseline = "middle";
    const every = cell < 14 ? Math.ceil(14 / cell) : 1;
    for (let col = 0; col < pl.columns; col++) if ((col + 1) % every === 0 || col === 0) ctx2d.fillText(String(col + 1), ox + (col + 0.5) * cell, top - 10);
    for (let r = 0; r < pl.rows; r++) if (r % every === 0) ctx2d.fillText(rowName(r), left - 13 + (ox - left), top + (r + 0.5) * cell);
    ctx2d.fillStyle = c.grid;
    for (let r = 0; r < pl.rows; r++) for (let col = 0; col < pl.columns; col++) ctx2d.fillRect(ox + col * cell + 0.5, top + r * cell + 0.5, cell - 1, cell - 1);
    for (const [r, col, v] of pl.cells) {
      const t = hi > lo ? (v - lo) / (hi - lo) : 0.5;
      const [R, G, B] = viridis(t);
      ctx2d.fillStyle = `rgb(${R},${G},${B})`;
      ctx2d.fillRect(ox + col * cell + 0.5, top + r * cell + 0.5, cell - 1, cell - 1);
    }
    // colour scale
    const ly = top + pl.rows * cell + 12;
    const lw = Math.min(260, gw * 0.6);
    for (let i = 0; i < lw; i++) {
      const [R, G, B] = viridis(i / (lw - 1));
      ctx2d.fillStyle = `rgb(${R},${G},${B})`;
      ctx2d.fillRect(ox + i, ly, 1, 8);
    }
    ctx2d.fillStyle = c.muted;
    ctx2d.font = `11px ${c.font}`;
    ctx2d.textAlign = "left";
    ctx2d.textBaseline = "top";
    ctx2d.fillText(fmt(lo, 4), ox, ly + 11);
    ctx2d.textAlign = "right";
    ctx2d.fillText(fmt(hi, 4), ox + lw, ly + 11);
    S.layout = { kind: "plate", ox, top, cell, rows: pl.rows, columns: pl.columns };
  }

  function rowName(r) {
    return r < 26 ? String.fromCharCode(65 + r) : "A" + String.fromCharCode(65 + r - 26);
  }

  // ----- flow-cytometry events

  function drawEvents() {
    const ev = S.data.events;
    const [w, h] = sizeCanvas();
    const c = colors();
    ctx2d.clearRect(0, 0, w, h);
    const left = 58, right = 14, top = 10, bottom = 38;
    const pw = w - left - right, ph = h - top - bottom;
    if (!ev.xv) {
      ev.xv = f32FromBase64(ev.xs);
      ev.yv = ev.ys ? f32FromBase64(ev.ys) : null;
    }
    const fx = scaleFn(fcsScaleOf(ev.x.name));
    const tx = Float64Array.from(ev.xv, fx);
    const xlo = percentile(tx, 0.2), xhi = percentile(tx, 99.8);
    const xr = xhi > xlo ? [xlo, xhi + (xhi - xlo) * 0.02] : [xlo - 1, xlo + 1];
    const X = (v) => left + (v - xr[0]) / (xr[1] - xr[0]) * pw;
    let yr, Y;
    ctx2d.font = `11px ${c.font}`;
    if (ev.yv) {
      const fy = scaleFn(fcsScaleOf(ev.y.name));
      const ty = Float64Array.from(ev.yv, fy);
      const ylo = percentile(ty, 0.2), yhi = percentile(ty, 99.8);
      yr = yhi > ylo ? [ylo, yhi + (yhi - ylo) * 0.02] : [ylo - 1, ylo + 1];
      Y = (v) => top + ph - (v - yr[0]) / (yr[1] - yr[0]) * ph;
      // density: counts on a grid, coloured on a log scale
      const bins = Math.max(60, Math.min(220, Math.floor(pw / 3)));
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
      for (let j = 0; j < by; j++) for (let i = 0; i < bins; i++) {
        const n = grid[j * bins + i];
        if (!n) continue;
        const [R, G, B] = viridis(Math.log(1 + n) / Math.log(1 + max));
        ctx2d.fillStyle = `rgb(${R},${G},${B})`;
        ctx2d.fillRect(left + i * cw, top + ph - (j + 1) * chh, Math.ceil(cw), Math.ceil(chh));
      }
      axisLeft(yr, Y, top, ph, left, c, ev.y.name + " · " + fcsScaleOf(ev.y.name));
    } else {
      const bins = Math.max(32, Math.min(256, Math.floor(pw / 3)));
      const counts = new Uint32Array(bins);
      for (const v of tx) {
        const u = (v - xr[0]) / (xr[1] - xr[0]);
        if (u >= 0 && u < 1) counts[Math.floor(u * bins)]++;
      }
      const max = Math.max(1, ...counts);
      yr = [0, max * 1.08];
      Y = (v) => top + ph - (v - yr[0]) / (yr[1] - yr[0]) * ph;
      ctx2d.fillStyle = PALETTE[0];
      ctx2d.globalAlpha = 0.8;
      const cw = pw / bins;
      for (let i = 0; i < bins; i++) if (counts[i]) ctx2d.fillRect(left + i * cw, Y(counts[i]), Math.max(1, cw - 0.5), top + ph - Y(counts[i]));
      ctx2d.globalAlpha = 1;
      axisLeft(yr, Y, top, ph, left, c, "events");
    }
    // x axis
    ctx2d.strokeStyle = c.axis;
    ctx2d.beginPath(); ctx2d.moveTo(left, top + ph + 0.5); ctx2d.lineTo(left + pw, top + ph + 0.5); ctx2d.stroke();
    ctx2d.fillStyle = c.muted;
    ctx2d.textAlign = "center";
    ctx2d.textBaseline = "top";
    for (const t of niceTicks(xr[0], xr[1], Math.max(2, Math.floor(pw / 90)))) ctx2d.fillText(fmt(t, 3), X(t), top + ph + 6);
    ctx2d.fillText(`${ev.x.name}${ev.x.label ? " · " + ev.x.label : ""} · ${fcsScaleOf(ev.x.name)}`, left + pw / 2, top + ph + 21);
    S.layout = { kind: "events", left, top, pw, ph, xr, yr, hist: !ev.yv };
  }

  function axisLeft(yr, Y, top, ph, left, c, label) {
    ctx2d.fillStyle = c.muted;
    ctx2d.textAlign = "right";
    ctx2d.textBaseline = "middle";
    for (const t of niceTicks(yr[0], yr[1], Math.max(2, Math.floor(ph / 42)))) ctx2d.fillText(fmt(t, 3), left - 6, Y(t));
    ctx2d.save();
    ctx2d.translate(12, top + ph / 2);
    ctx2d.rotate(-Math.PI / 2);
    ctx2d.textAlign = "center";
    ctx2d.fillText(trimLabel(label, ph), 0, 0);
    ctx2d.restore();
  }

  // ---------- pointer: hover readout, drag to zoom, click

  function pos(e) {
    const r = canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  function tip(text, x, y) {
    const t = $("tip");
    if (!text) { t.hidden = true; return; }
    t.hidden = false;
    t.textContent = text;
    const w = canvas.clientWidth;
    t.style.left = Math.min(w - t.offsetWidth - 4, x + 12) + "px";
    t.style.top = Math.max(2, y - 28) + "px";
  }

  function hover(px, py) {
    const L = S.layout, d = S.data;
    if (!L || !d) return "";
    if (L.kind === "image") {
      const p = imageCoords(px, py);
      if (!p) return "";
      const info = ((S.outline || {}).images || []).find((x) => d.image && x.index === d.image.image) || {};
      const ps = info.physical_size || {};
      let s = `x ${Math.floor(p.x)}, y ${Math.floor(p.y)} px`;
      if (ps.x && ps.y) s += ` · ${fmt(p.x * ps.x, 4)}, ${fmt(p.y * ps.y, 4)} ${ps.unit || "µm"}`;
      return s;
    }
    if (L.kind === "plot") {
      if (px < L.left || px > L.left + L.pw) return "";
      const panel = L.panels.find((g) => py >= g.py && py <= g.py + g.ph) || L.panels[0];
      if (!panel) return "";
      const s = panel.series[0];
      if (!s) return "";
      let best = -1, bd = Infinity;
      for (let k = 0; k < s.x.length; k++) {
        const dd = Math.abs(L.X(s.x[k]) - px);
        if (dd < bd) { bd = dd; best = k; }
      }
      if (best < 0) return "";
      const x = s.x[best];
      const y = s.y ? s.y[best] : null;
      const yv = y != null ? fmt(y, 5) : `${fmt(s.lo[best], 4)} … ${fmt(s.hi[best], 4)}`;
      const unit = (d.plot.x && d.plot.x.unit) || "";
      return `${fmt(x, 6)} ${unit} · ${yv}${s.unit ? " " + s.unit : ""}`;
    }
    if (L.kind === "plate") {
      const col = Math.floor((px - L.ox) / L.cell), r = Math.floor((py - L.top) / L.cell);
      if (col < 0 || r < 0 || col >= L.columns || r >= L.rows) return "";
      const hit = d.plate.cells.find(([a, b]) => a === r && b === col);
      return `${rowName(r)}${col + 1}: ${hit ? fmt(hit[2], 5) : "no value"}`;
    }
    if (L.kind === "events") {
      const u = (px - L.left) / L.pw, v = 1 - (py - L.top) / L.ph;
      if (u < 0 || u > 1 || v < 0 || v > 1) return "";
      const x = L.xr[0] + u * (L.xr[1] - L.xr[0]);
      return L.hist ? `${fmt(x, 4)}` : `${fmt(x, 4)}, ${fmt(L.yr[0] + v * (L.yr[1] - L.yr[0]), 4)} (display scale)`;
    }
    return "";
  }

  function drawSelection(a, b) {
    redraw();
    const L = S.layout;
    if (!L) return;
    ctx2d.save();
    ctx2d.fillStyle = "rgba(47,111,223,0.15)";
    ctx2d.strokeStyle = "rgba(47,111,223,0.9)";
    ctx2d.lineWidth = 1;
    if (L.kind === "image") {
      const x = Math.min(a[0], b[0]), y = Math.min(a[1], b[1]);
      ctx2d.fillRect(x, y, Math.abs(b[0] - a[0]), Math.abs(b[1] - a[1]));
      ctx2d.strokeRect(x + 0.5, y + 0.5, Math.abs(b[0] - a[0]), Math.abs(b[1] - a[1]));
    } else if (L.kind === "plot") {
      const x = Math.min(a[0], b[0]);
      ctx2d.fillRect(x, L.top, Math.abs(b[0] - a[0]), L.bottom - L.top);
    }
    ctx2d.restore();
  }

  function finishDrag(a, b) {
    const L = S.layout;
    if (!L) return;
    if (L.kind === "image") {
      const p = imageCoords(Math.min(a[0], b[0]), Math.min(a[1], b[1])) || imageCoords(L.ox, L.oy);
      const q = imageCoords(Math.max(a[0], b[0]), Math.max(a[1], b[1])) || imageCoords(L.ox + L.dw - 0.01, L.oy + L.dh - 0.01);
      if (!p || !q) return;
      const x = Math.max(0, Math.floor(p.x)), y = Math.max(0, Math.floor(p.y));
      const width = Math.ceil(q.x) - x, height = Math.ceil(q.y) - y;
      if (width < 8 || height < 8) return;
      load({ region: { x, y, width, height } }, { keepZoom: true });
    } else if (L.kind === "plot") {
      const inv = (px) => L.reversed ? L.x1 - (px - L.left) / L.pw * (L.x1 - L.x0) : L.x0 + (px - L.left) / L.pw * (L.x1 - L.x0);
      const xa = inv(Math.max(L.left, Math.min(a[0], b[0]))), xb = inv(Math.min(L.left + L.pw, Math.max(a[0], b[0])));
      const p = S.data.plot;
      if (p.kind === "trace" || p.kind === "nmr") {
        const win = sampleWindow(p, xa, xb);
        if (win) load(win, { keepZoom: true });
      } else {
        load({ x_range: [Math.min(xa, xb), Math.max(xa, xb)] }, { keepZoom: true });
      }
    }
  }

  function click(px) {
    const L = S.layout, d = S.data;
    if (!L || L.kind !== "plot" || !d.plot || d.plot.kind !== "chromatogram" || d.plot.source !== "spectra") return;
    const x = L.reversed ? L.x1 - (px - L.left) / L.pw * (L.x1 - L.x0) : L.x0 + (px - L.left) / L.pw * (L.x1 - L.x0);
    S.args = { file: S.args.file, view: "spectrum", rt_min: x, run: d.plot.run ?? S.args.run };
    load();
  }

  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    canvas.setPointerCapture(e.pointerId);
    S.drag = { a: pos(e), moved: false };
  });
  canvas.addEventListener("pointermove", (e) => {
    const p = pos(e);
    if (S.drag) {
      if (Math.abs(p[0] - S.drag.a[0]) + Math.abs(p[1] - S.drag.a[1]) > 4) S.drag.moved = true;
      if (S.drag.moved) drawSelection(S.drag.a, p);
      return;
    }
    const text = hover(p[0], p[1]);
    $("readout").textContent = text;
    tip(text, p[0], p[1]);
  });
  canvas.addEventListener("pointerup", (e) => {
    const drag = S.drag;
    S.drag = null;
    if (!drag) return;
    const p = pos(e);
    if (drag.moved) finishDrag(drag.a, p);
    else click(p[0]);
  });
  canvas.addEventListener("pointerleave", () => tip(null));
  canvas.addEventListener("dblclick", () => {
    if (ZOOM_KEYS.some((k) => S.args && S.args[k] !== undefined)) load({});
  });
  canvas.addEventListener("keydown", (e) => {
    const d = S.data;
    if (!d) return;
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    if (d.view === "image" && d.image && !d.image.projection) {
      const info = ((S.outline || {}).images || []).find((x) => x.index === d.image.image) || {};
      const z = (d.image.z[0] || 0) + step;
      if (z >= 0 && z < (info.size_z || 1)) load({ z }, { keepZoom: true });
    } else if (d.plot && d.plot.kind === "trace") {
      const s = d.plot.sweep + step;
      if (s >= 0 && s < d.plot.sweep_count) load({ sweep: s });
    } else if (d.plot && d.plot.kind === "spectrum") {
      const i = d.plot.spectrum.index + step;
      if (i >= 0 && i < d.plot.scan_count) load({ index: i, rt_min: undefined, scan: undefined });
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
  let lastWidth = 0;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      const w = canvas.parentElement.clientWidth;
      redraw();
      // A much wider image view deserves a sharper picture.
      if (S.data && S.data.view === "image" && S.data.width && w * (window.devicePixelRatio || 1) > S.data.width * 1.4 && S.data.width < 1600 && w !== lastWidth) {
        lastWidth = w;
        load({}, { keepZoom: true });
      }
    }, 150);
  });

  // ---------- start

  host
    .request("ui/initialize", {
      appInfo: { name: "openreadout-viewer", version: "1" },
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
  module.exports = { niceTicks, niceBelow, fmt, viridis, plotXs, sampleWindow, f32FromBase64, scaleFn, percentile, hintFrom };
} else {
  main();
}
