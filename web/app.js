// The demo page: sample cards, your own file, and the result with its picture and controls.
// Everything runs in this tab. The page's Content-Security-Policy (`connect-src 'self'`) lets
// it fetch only its own files from this site, the WebAssembly module and the samples, so a file
// you open cannot leave your device. reader.js holds the parts that don't touch the page.

import { BUILT } from "./built.js";
import * as R from "./reader.js";
import { FIRST, SAMPLES } from "./samples.js";

const $ = (id) => document.getElementById(id);
const touch = matchMedia("(hover: none)").matches;
const nextFrame = () => new Promise((r) => requestAnimationFrame(() => setTimeout(r, 0)));

let lastWidth = 0; // the stage width of the last drawing

const S = {
  module: null, // promise of the loaded module
  bytes: new Map(), // sample id -> Uint8Array
  gen: 0, // the latest open request; older ones give way
  cur: null, // the open file: { file, name, size, info, explain, check, ms, sample, view, meta, ... }
};

// ----- small DOM helpers -----------------------------------------------------------------------

function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === false) continue;
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) if (c != null && c !== false) e.append(typeof c === "string" ? document.createTextNode(c) : c);
  return e;
}

function errorText(err) {
  return el("p", { class: "error" }, err?.message ?? "This failed.", err?.hint ? el("span", { class: "hint" }, err.hint) : null);
}

// ----- theme ---------------------------------------------------------------------------------------

const systemDark = matchMedia("(prefers-color-scheme: dark)");

function isDark() {
  const t = document.documentElement.dataset.theme;
  return t ? t === "dark" : systemDark.matches;
}

/** Card pictures follow the page's theme, also when it differs from the system's. */
function syncThumbs() {
  const forced = document.documentElement.dataset.theme;
  for (const pic of document.querySelectorAll(".card picture")) {
    const src = pic.querySelector("source");
    const img = pic.querySelector("img");
    if (!src.dataset.dark) src.dataset.dark = src.getAttribute("srcset");
    if (!img.dataset.light) img.dataset.light = img.getAttribute("src");
    if (forced) {
      src.media = "not all";
      img.src = forced === "dark" ? src.dataset.dark : img.dataset.light;
    } else {
      src.media = "(prefers-color-scheme: dark)";
      img.src = img.dataset.light;
    }
  }
}

function themeChanged() {
  syncThumbs();
  if (S.cur?.meta && !S.cur.meta.image) redraw();
  if (S.cur?.second) drawSecond();
}

$("theme").addEventListener("click", () => {
  const next = isDark() ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  try {
    localStorage.setItem("starlight-theme", next);
  } catch {
    // the choice lasts for this visit only
  }
  themeChanged();
});
systemDark.addEventListener("change", () => {
  if (!document.documentElement.dataset.theme) themeChanged();
});

// ----- status line -------------------------------------------------------------------------------

function status(text, fraction) {
  const box = $("status");
  box.hidden = !text;
  box.classList.remove("error");
  $("status-text").textContent = text ?? "";
  $("status-bar").style.width = `${Math.round(Math.min(1, fraction ?? 0) * 100)}%`;
}

function statusError(text) {
  status(text);
  $("status").classList.add("error");
}

// ----- loading the module and the samples ---------------------------------------------------------

/** Read a response body, reporting progress against `total` bytes. */
function counted(res, total, onProgress) {
  if (!res.body || typeof TransformStream !== "function") return res.body;
  let got = 0;
  return res.body.pipeThrough(
    new TransformStream({
      transform(chunk, c) {
        got += chunk.byteLength;
        onProgress(Math.min(1, got / total));
        c.enqueue(chunk);
      },
    }),
  );
}

let moduleShare = 0; // how much of the status bar the module is worth, while a sample also loads

function loadModule(onProgress = () => {}) {
  if (S.module === null) {
    S.module = (async () => {
      const res = await fetch("openreadout/pkg/openreadout_bg.wasm");
      if (!res.ok) throw new Error(`the WebAssembly module did not load (HTTP ${res.status})`);
      const body = counted(res, BUILT.wasm.bytes, (f) => S.onModuleProgress?.(f));
      await R.ready(new Response(body, { headers: { "content-type": "application/wasm" } }));
    })();
    S.module.catch(() => {
      S.module = null;
    });
  }
  S.onModuleProgress = onProgress;
  return S.module;
}

async function fetchSample(s, onProgress) {
  if (S.bytes.has(s.id)) return S.bytes.get(s.id);
  const b = BUILT.samples[s.id];
  const packed = b.gz !== null && typeof DecompressionStream === "function";
  const res = await fetch(`samples/${encodeURIComponent(s.file)}${packed ? ".gz" : ""}`);
  if (!res.ok) throw new Error(`the sample did not load (HTTP ${res.status})`);
  let body = counted(res, packed ? b.gz : b.bytes, onProgress);
  if (packed) body = body.pipeThrough(new DecompressionStream("gzip"));
  const bytes = new Uint8Array(await new Response(body).arrayBuffer());
  S.bytes.set(s.id, bytes);
  return bytes;
}

const mb = (n) => `${(n / 1e6).toFixed(1)} MB`;

// ----- opening files -------------------------------------------------------------------------------

function pressCard(id) {
  for (const c of document.querySelectorAll(".card")) c.setAttribute("aria-pressed", String(c.dataset.sample === id));
}

async function openSample(id, { scroll = false } = {}) {
  const s = SAMPLES.find((x) => x.id === id);
  if (!s) return;
  const gen = ++S.gen;
  pressCard(id);
  if (location.hash !== `#${id}`) history.replaceState(null, "", `#${id}`);
  const b = BUILT.samples[id];
  const sampleBytes = b.gz ?? b.bytes;
  let modF = 0;
  let smpF = S.bytes.has(id) ? 1 : 0;
  const needModule = !R.isReady();
  const total = (needModule ? BUILT.wasm.bytes * 0.45 : 0) + sampleBytes;
  const show = () => {
    if (gen !== S.gen) return;
    const done = (needModule ? modF * BUILT.wasm.bytes * 0.45 : 0) + smpF * sampleBytes;
    const what = needModule && modF < 1 ? `Loading OpenReadout and the ${s.label.toLowerCase()} sample` : `Loading the ${s.label.toLowerCase()} sample`;
    status(`${what} · ${mb(done)} of ${mb(total)}`, done / total);
  };
  show();
  try {
    const [bytes] = await Promise.all([
      fetchSample(s, (f) => {
        smpF = f;
        show();
      }),
      loadModule((f) => {
        modF = f;
        show();
      }),
    ]);
    if (gen !== S.gen) return;
    await show_(bytes, s.file, s, gen, scroll);
  } catch (e) {
    if (gen === S.gen) statusError(`Could not open the sample: ${e.message}`);
  }
}

async function openOwn(file) {
  if (!file) return;
  const gen = ++S.gen;
  pressCard(null);
  history.replaceState(null, "", location.pathname + location.search);
  try {
    if (!R.isReady()) {
      status(`Loading OpenReadout`, 0);
      await loadModule((f) => gen === S.gen && status(`Loading OpenReadout · ${mb(f * BUILT.wasm.bytes)} of ${mb(BUILT.wasm.bytes)}`, f));
    }
    if (gen !== S.gen) return;
    await show_(file, file.name, null, gen, true);
  } catch (e) {
    if (gen === S.gen) statusError(`Could not read ${file.name}: ${e.message}`);
  }
}

async function show_(source, name, sample, gen, scroll) {
  status(`Reading ${name}…`, 1);
  await nextFrame();
  const r = await R.openFile(source, name);
  if (gen !== S.gen) {
    r.file.close();
    return;
  }
  S.cur?.file.close();
  S.cur = { ...r, sample, view: r.info.ok ? R.firstView(r.info.value, sample?.view) : null, draws: 0 };
  renderHead();
  renderWords();
  renderFacts();
  $("json").textContent = r.info.ok ? JSON.stringify(r.info.value, null, 2) : JSON.stringify(r.info.error, null, 2);
  $("controls").replaceChildren();
  $("extra").replaceChildren();
  $("numbers").replaceChildren();
  $("second").hidden = true;
  S.cur.second = null;
  $("result").hidden = false;
  status(null);
  if (r.info.ok) {
    await redraw({ first: true });
  } else {
    showFigureError(r.info.error);
  }
  if (scroll) $("result").scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "start" });
}

// ----- the result: header, words, facts ---------------------------------------------------------

function renderHead() {
  const c = S.cur;
  const info = c.info.ok ? c.info.value : null;
  $("r-kind").textContent = c.sample ? `${c.sample.label} sample` : "Your file";
  $("r-title").textContent = c.sample ? c.sample.subject : c.name;
  const meta = $("r-meta");
  meta.replaceChildren(
    el("code", {}, c.name),
    ` · ${c.sample?.format ?? (info ? R.formatName(info) : "format not recognised")} · ${R.human(c.size)} · read in ${c.ms} ms in this tab`,
  );
  if (c.sample) {
    meta.append(el("br"), "Sample: ", el("a", { href: c.sample.href }, c.sample.credit), ` (${c.sample.licence})`);
  }
  const dl = $("download");
  dl.hidden = !c.sample;
  if (c.sample) {
    dl.href = `samples/${encodeURIComponent(c.sample.file)}`;
    dl.setAttribute("download", c.sample.file);
    dl.textContent = `Download ${c.sample.file}`;
  }
}

function renderWords() {
  const c = S.cur;
  const box = $("words");
  box.replaceChildren();
  if (!c.explain.ok) {
    box.append(errorText(c.explain.error));
  } else {
    const e = c.explain.value;
    const info = c.info.ok ? c.info.value : null;
    const summary = info ? e.summary.replace(info.format.name, R.formatName(info)) : e.summary;
    box.append(el("p", { class: "summary" }, summary));
    const paras = e.paragraphs ?? [];
    for (const p of paras.slice(0, 2)) box.append(el("p", {}, p));
    const more = [...paras.slice(2), ...(e.caveats ?? [])];
    if (more.length) box.append(el("details", {}, el("summary", {}, "More detail"), ...more.map((p) => el("p", {}, p))));
  }
  const check = $("check");
  if (!c.check.ok) {
    check.className = "check fail";
    check.textContent = c.check.error.message;
  } else {
    const ok = c.check.value.ok;
    const n = (c.check.value.findings ?? []).length;
    check.className = `check ${ok ? "pass" : "fail"}`;
    check.textContent = ok
      ? `Integrity check passed${n ? ` with ${n} note${n > 1 ? "s" : ""}` : ""}: the file is complete and its structure is sound.`
      : `Integrity check found problems: ${(c.check.value.findings ?? []).map((f) => f.message).join("; ")}`;
  }
}

function renderFacts() {
  const c = S.cur;
  const facts = $("facts");
  const list = $("measured");
  facts.replaceChildren();
  list.replaceChildren();
  if (!c.info.ok) {
    facts.append(el("dt", {}, "Format"), el("dd", {}, "Not recognised"));
    return;
  }
  for (const m of R.measurements(c.info.value)) list.append(el("li", {}, m));
  for (const [k, v] of R.facts(c.info.value)) facts.append(el("dt", {}, k), el("dd", {}, v));
}

function renderNumbers() {
  const c = S.cur;
  const dl = $("numbers");
  dl.replaceChildren();
  for (const n of R.numbers(c.info.value, c.view, c.meta)) {
    dl.append(el("dt", { class: n.wide ? "wide" : undefined }, n.label), el("dd", { class: [n.strong ? "strong" : "", n.wide ? "wide" : ""].join(" ").trim() || undefined }, n.value));
  }
  $("cmd").textContent = R.command(c.name, c.info.value, c.view);
}

// ----- drawing ---------------------------------------------------------------------------------------

const frame = $("frame");
const img = $("figure");

function busy(on, text = "Drawing…") {
  $("busy").hidden = !on;
  $("busy-text").textContent = text;
}

function showFigureError(err) {
  img.removeAttribute("src");
  img.hidden = true;
  $("scalebar").hidden = true;
  frame.className = "frame";
  const old = frame.querySelector(".error");
  old?.remove();
  frame.append(errorText(err));
  $("hint").textContent = "";
}

const PLOT_MAX = 960; // plots wider than this only grow their type

function stageSize() {
  const w = Math.min($("stage").clientWidth, S.cur?.meta?.image || S.cur?.view?.kind === "image" ? Infinity : PLOT_MAX);
  const h = Math.max(320, Math.round(window.innerHeight * (touch ? 0.62 : 0.72)));
  return { width: w, height: h };
}

/** Draw the current view into the main figure. */
async function redraw({ first = false } = {}) {
  const c = S.cur;
  if (!c?.info.ok) return;
  const token = ++c.draws;
  const slow = c.view.objects ? "Finding nuclei…" : "Drawing…";
  const timer = setTimeout(() => token === c.draws && busy(true, slow), 120);
  await nextFrame();
  const { width, height } = stageSize();
  lastWidth = width;
  let p;
  try {
    p = await R.draw(c.file, c.info.value, c.view, { width, height, scale: window.devicePixelRatio || 1, dark: isDark() });
  } catch (e) {
    clearTimeout(timer);
    if (token !== c.draws) return;
    busy(false);
    if (first) showFigureError(e);
    else $("hint").textContent = `${e.message}${e.hint ? ` ${e.hint}` : ""}`;
    return;
  }
  if (token !== c.draws || S.cur !== c) return;
  const url = URL.createObjectURL(new Blob([p.data], { type: p.type }));
  const pre = new Image();
  pre.src = url;
  try {
    await pre.decode();
  } catch {
    // shown anyway
  }
  if (token !== c.draws || S.cur !== c) {
    URL.revokeObjectURL(url);
    return;
  }
  clearTimeout(timer);
  busy(false);
  frame.querySelector(".error")?.remove();
  const old = img.src;
  const cssW = p.meta.width / p.scale;
  const cssH = p.meta.height / p.scale;
  img.hidden = false;
  img.classList.remove("settling");
  img.style.transform = "";
  img.src = url;
  img.alt = altText(c, p.meta);
  frame.style.width = `${Math.round(cssW)}px`;
  frame.style.maxWidth = "100%";
  frame.style.marginInline = "auto";
  frame.style.aspectRatio = `${cssW} / ${cssH}`;
  if (old.startsWith("blob:")) URL.revokeObjectURL(old);
  c.meta = p.meta;
  c.figure = { cssW, cssH };
  zoom.reset();
  if (first) setupKind();
  updateScalebar();
  renderNumbers();
  afterDraw();
}

function altText(c, meta) {
  const what = c.sample ? c.sample.subject : c.name;
  if (meta.image) return `${what}: image, ${meta.image.full_res_region.width} × ${meta.image.full_res_region.height} pixels shown`;
  return `${what}: ${meta.kind} plot`;
}

function updateScalebar() {
  const c = S.cur;
  const bar = $("scalebar");
  const im = c.meta?.image ? R.imageOf(c.info.value, c.view) : null;
  const ps = im?.physical_size;
  if (!im || !ps?.x || ps.unit !== "µm") {
    bar.hidden = true;
    return;
  }
  const umPerCss = (c.meta.image.full_res_region.width * ps.x) / c.figure.cssW;
  const target = umPerCss * Math.min(110, c.figure.cssW * 0.3);
  const nice = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000].reduce((a, b) => (b <= target ? b : a), 1);
  bar.hidden = false;
  bar.querySelector("span").style.width = `${nice / umPerCss}px`;
  bar.querySelector("b").textContent = nice >= 1000 ? `${nice / 1000} mm` : `${nice} µm`;
}

// ----- per-kind setup: controls and what a tap does --------------------------------------------------

function setupKind() {
  const c = S.cur;
  const info = c.info.value;
  const box = $("controls");
  box.replaceChildren();
  frame.classList.remove("zoomable", "pickable");
  frame.querySelector(".zoombar")?.remove();
  c.pick = null;
  let hint = "";

  if (c.meta.image) {
    frame.classList.add("zoomable");
    frame.append(zoomBar());
    hint = touch ? "Tap to zoom in, drag to move, pinch to zoom in or out." : "Click to zoom in, drag to move, scroll to zoom.";
    const im = R.imageOf(info, c.view);
    if (c.view.channels && im.size_c > 1) box.append(channelChips(im));
    if (im.size_z > 1) box.append(stepper("Focal plane (z)", c.view.z, im.size_z, (z) => ((c.view.z = z), redraw())));
    if (im.size_t > 1) box.append(stepper("Time point (t)", c.view.t, im.size_t, (t) => ((c.view.t = t), redraw())));
    box.append(nucleiToggle());
  } else {
    const traces = R.traceChoices(info);
    if (c.meta.trace && traces.length > 1) {
      box.append(
        selectControl("Recording", traces.map((t) => [String(t.index), t.name]), String(c.view.trace ?? c.meta.trace.trace), (v) => {
          c.view.trace = Number(v);
          c.view.sweep = undefined;
          c.view.marks = R.firstView(info, { trace: c.view.trace }).marks;
          redraw({ first: true });
        }),
      );
    }
    const t = c.meta.trace && traces.find((x) => x.index === c.meta.trace.trace);
    if (t && t.sweeps > 1) {
      const cur = c.meta.trace.sweep;
      box.append(
        stepper("Sweep", cur, t.sweeps, (s) => {
          c.view.sweep = s;
          redraw();
        }),
      );
      if (c.view.marks === "spikes") box.append(el("div", { class: "control", id: "spikebars" }));
      hint = c.view.marks === "spikes" ? "Step through the sweeps: each injects more current. The bars count the action potentials per sweep." : "";
    }
    if (c.meta.events) {
      box.append(axisPickers(info));
      hint = "Choose the two parameters to plot against each other.";
    }
    if (c.meta.spectrum) {
      const run = info.spectra?.find((r) => r.index === (c.meta.spectrum.run ?? 0));
      if (run?.scan_count > 1) {
        box.append(
          stepper("Scan", c.meta.spectrum.index, run.scan_count, (i) => {
            c.view.spectrum = i;
            redraw();
          }),
        );
      }
    }
    const timeAxis = c.meta.trace?.x_axis?.quantity === "retention_time" || c.meta.chromatogram;
    if (timeAxis && info.spectra?.length) {
      frame.classList.add("pickable");
      c.pick = pickRetentionTime;
      hint = touch ? "Tap the chromatogram to see the mass spectrum at that time." : "Click the chromatogram to see the mass spectrum at that time.";
    }
    if (c.meta.plate?.view === "curves") {
      frame.classList.add("pickable");
      c.pick = pickWell;
      hint = touch ? "Tap a well to see its numbers." : "Click a well to see its numbers.";
    }
  }
  $("hint").textContent = hint;
}

/** Work that follows a drawing: per-kind extras that need the picture's layout. */
function afterDraw() {
  const c = S.cur;
  if (c.meta.plate && c.wellsPromise === undefined) {
    c.wellsPromise = R.wellSeries(c.file, c.info.value, c.meta.plate.table ?? 0).catch(() => new Map());
  }
  if (c.pick === pickRetentionTime && !c.second) {
    const peaks = (c.meta.marks ?? []).filter((m) => m.kind === "peak");
    const top = peaks.sort((a, b) => b.y - a.y)[0];
    if (top) selectRetentionTime(top.x);
  }
  if (document.getElementById("spikebars")) spikeBars();
  if (c.cq === undefined) {
    c.cq = null;
    R.cqBySample(c.file, c.info.value).then(
      (rows) => {
        if (S.cur === c && rows.length) {
          c.cq = rows;
          $("extra").replaceChildren(cqTable(rows));
        }
      },
      () => {},
    );
  }
}

function control(label, ...children) {
  return el("div", { class: "control" }, el("span", { class: "c-label" }, label), ...children);
}

function selectControl(label, options, value, onChange) {
  const sel = el("select", { class: "select", "aria-label": label, onchange: (e) => onChange(e.target.value) });
  for (const [v, text] of options) sel.append(el("option", { value: v, selected: v === value }, text));
  return control(label, sel);
}

function stepper(label, value, count, onChange) {
  let cur = value ?? 0;
  const out = el("output", {});
  const range = el("input", { type: "range", min: "0", max: String(count - 1), step: "1", value: String(cur), "aria-label": label });
  const prev = el("button", { type: "button", "aria-label": `Previous ${label.toLowerCase()}` }, "‹");
  const next = el("button", { type: "button", "aria-label": `Next ${label.toLowerCase()}` }, "›");
  const set = (v, fire = true) => {
    cur = Math.max(0, Math.min(count - 1, v));
    range.value = String(cur);
    out.textContent = `${cur} of ${count}`;
    prev.disabled = cur === 0;
    next.disabled = cur === count - 1;
    if (fire) onChange(cur);
  };
  prev.addEventListener("click", () => set(cur - 1));
  next.addEventListener("click", () => set(cur + 1));
  let t;
  range.addEventListener("input", () => {
    clearTimeout(t);
    out.textContent = `${Number(range.value)} of ${count}`;
    t = setTimeout(() => set(Number(range.value)), 60);
  });
  set(cur, false);
  const wrap = control(label, el("div", { class: "stepper" }, prev, out, next), range);
  wrap.setStep = (v) => set(v);
  return wrap;
}

function channelChips(im) {
  const c = S.cur;
  const chips = el("div", { class: "chips" });
  for (const ch of im.channels.slice(0, 4)) {
    const on = () => c.view.channels.includes(ch.index);
    const b = el(
      "button",
      { type: "button", class: "chip", "aria-pressed": String(on()) },
      el("span", { class: "dot" }),
      ch.fluorophore ?? ch.name ?? `Channel ${ch.index + 1}`,
    );
    b.querySelector(".dot").style.background = ch.color ?? "#888";
    b.addEventListener("click", () => {
      const set = new Set(c.view.channels);
      if (set.has(ch.index)) {
        if (set.size === 1) return;
        set.delete(ch.index);
      } else set.add(ch.index);
      c.view.channels = [...set].sort((a, b) => a - b);
      b.setAttribute("aria-pressed", String(on()));
      redraw();
    });
    chips.append(b);
  }
  return control("Channels", chips);
}

function nucleiToggle() {
  const c = S.cur;
  const b = el("button", { type: "button", class: "chip", "aria-pressed": String(Boolean(c.view.objects)) }, "Outline and count nuclei");
  b.addEventListener("click", () => {
    c.view.objects = !c.view.objects;
    b.setAttribute("aria-pressed", String(c.view.objects));
    redraw();
  });
  return control("Analysis", el("div", { class: "chips" }, b), el("p", { class: "note" }, "Finds nuclei in the area shown, as openreadout stats does."));
}

function axisPickers(info) {
  const c = S.cur;
  const params = R.parameterChoices(info, c.meta.events.table ?? 0);
  const opts = params.map((p) => [p.name, p.label]);
  const cur = [c.meta.events.x, c.meta.events.y];
  const make = (i, label) => {
    const sel = el("select", { class: "select", "aria-label": `${label} axis` });
    for (const [v, text] of opts) sel.append(el("option", { value: v, selected: v === cur[i] }, text));
    sel.addEventListener("change", () => {
      const cols = [c.meta.events.x, c.meta.events.y];
      cols[i] = sel.value;
      c.view.columns = cols;
      redraw();
    });
    return sel;
  };
  return control("Axes", el("div", { class: "axes" }, "Across", make(0, "Horizontal"), "Up", make(1, "Vertical")));
}

async function spikeBars() {
  const c = S.cur;
  const box = document.getElementById("spikebars");
  const t = c.meta.trace;
  if (!box || !t) return;
  const sweeps = R.traceChoices(c.info.value).find((x) => x.index === t.trace)?.sweeps ?? 1;
  if (!c.spikes || c.spikes.trace !== t.trace) {
    c.spikes = { trace: t.trace, counts: new Array(sweeps).fill(null) };
    c.spikes.counts[t.sweep] = (c.meta.marks ?? []).filter((m) => m.kind === "spike").length;
    // count the other sweeps in the background, a few at a time
    (async () => {
      for (let s = 0; s < sweeps; s++) {
        if (S.cur !== c || c.spikes.trace !== t.trace) return;
        if (c.spikes.counts[s] !== null) continue;
        await new Promise((r) => setTimeout(r, 16));
        try {
          const p = await c.file.preview({ trace: t.trace, sweep: s, marks: "spikes", max_size: 200 });
          c.spikes.counts[s] = (p.meta.marks ?? []).filter((m) => m.kind === "spike").length;
        } catch {
          c.spikes.counts[s] = 0;
        }
        if (s % 4 === 3 || s === sweeps - 1) paintBars();
      }
      paintBars();
    })();
  }
  paintBars();
}

function paintBars() {
  const c = S.cur;
  const box = document.getElementById("spikebars");
  if (!box || !c.spikes) return;
  const counts = c.spikes.counts;
  const max = Math.max(1, ...counts.map((n) => n ?? 0));
  const cur = c.meta.trace.sweep;
  const bars = el("div", { class: "bars", role: "group", "aria-label": "Action potentials per sweep" });
  counts.forEach((n, i) => {
    const b = el("button", { type: "button", "aria-pressed": String(i === cur), "aria-label": `Sweep ${i}: ${n ?? "…"} action potentials`, title: `Sweep ${i}: ${n ?? "…"}` });
    b.style.height = `${Math.max(4, ((n ?? 0) / max) * 100)}%`;
    b.addEventListener("click", () => {
      c.view.sweep = i;
      $("controls").querySelector(".stepper")?.closest(".control")?.setStep?.(i);
    });
    bars.append(b);
  });
  box.replaceChildren(
    el("span", { class: "c-label" }, "Action potentials per sweep"),
    bars,
    el("div", { class: "bars-caption" }, el("span", {}, "sweep 0"), el("span", {}, `${counts.length - 1}`)),
  );
}

function cqTable(rows) {
  const t = el("table", { class: "cq" }, el("caption", {}, "Cq by sample (mean of wells)"));
  for (const r of rows.slice(0, 12)) t.append(el("tr", {}, el("td", {}, r.sample), el("td", {}, `${R.round(r.cq)} (${r.wells} well${r.wells > 1 ? "s" : ""})`)));
  return t;
}

// ----- picking on plots -----------------------------------------------------------------------------

/** The figure's coordinates (CSS pixels of the drawing) under a pointer event. */
function figurePoint(e) {
  const r = img.getBoundingClientRect();
  const k = S.cur.figure.cssW / r.width;
  return { x: (e.clientX - r.left) * k, y: (e.clientY - r.top) * k };
}

function pickRetentionTime(e) {
  const c = S.cur;
  const pt = figurePoint(e);
  const hit = R.pointAt(c.meta, pt.x, pt.y);
  if (!hit) return;
  // snap to a labelled peak within a few pixels
  const a = hit.panel.plot_area;
  const perPx = (hit.panel.x[1] - hit.panel.x[0]) / a.width;
  const near = (c.meta.marks ?? []).filter((m) => m.kind === "peak" && Math.abs(m.x - hit.x) < 10 * perPx).sort((m, n) => Math.abs(m.x - hit.x) - Math.abs(n.x - hit.x))[0];
  selectRetentionTime(near ? near.x : hit.x);
}

async function selectRetentionTime(rt) {
  const c = S.cur;
  const run = c.meta.chromatogram?.run ?? 0;
  const scan = await R.scanNear(c.file, run, rt, 0.03).catch(() => undefined);
  if (S.cur !== c || !scan) return;
  c.second = { run, spectrum: scan.index, rt: scan.rt_s / 60 };
  c.view.annotations = [{ type: "vline", x: c.second.rt, color: "accent" }];
  redraw();
  drawSecond();
}

async function drawSecond() {
  const c = S.cur;
  if (!c?.second) return;
  const box = $("second");
  const v = { kind: "plot", run: c.second.run, spectrum: c.second.spectrum };
  try {
    const p = await R.draw(c.file, c.info.value, v, { width: Math.min($("stage").clientWidth, PLOT_MAX), scale: window.devicePixelRatio || 1, dark: isDark() });
    if (S.cur !== c) return;
    const im2 = $("figure2");
    const old = im2.src;
    im2.src = URL.createObjectURL(new Blob([p.data], { type: p.type }));
    im2.alt = `Mass spectrum at ${R.round(c.second.rt)} min`;
    if (old.startsWith("blob:")) URL.revokeObjectURL(old);
    box.hidden = false;
    const peaks = (p.meta.marks ?? []).filter((m) => m.kind === "peak").map((m) => m.label);
    $("hint2").textContent = `Mass spectrum of scan ${p.meta.spectrum?.scan_number ?? ""} at ${R.round(c.second.rt)} min${peaks.length ? `: largest peaks at m/z ${peaks.join(", ")}` : ""}.`;
  } catch (e) {
    $("hint2").textContent = e.message;
    box.hidden = false;
  }
}

async function pickWell(e) {
  const c = S.cur;
  const pt = figurePoint(e);
  const hit = R.pointAt(c.meta, pt.x, pt.y);
  const well = hit?.panel.well ?? (c.meta.panels ?? []).find((p) => {
    const a = p.plot_area;
    return pt.x >= a.x - 4 && pt.x <= a.x + a.width + 4 && pt.y >= a.y - 4 && pt.y <= a.y + a.height + 4;
  })?.well;
  if (!well) return;
  c.view.annotations = [{ type: "well", well }];
  redraw();
  const series = await c.wellsPromise;
  if (S.cur !== c) return;
  const s = series?.get(well);
  const unit = c.meta.plate.time_unit ?? "";
  const dl = el(
    "dl",
    {},
    el("dt", {}, "Well"),
    el("dd", { class: "strong" }, well),
    ...(s
      ? [
          el("dt", {}, "Start"),
          el("dd", {}, R.round(s.first)),
          el("dt", {}, "End"),
          el("dd", {}, R.round(s.last)),
          el("dt", {}, "Highest"),
          el("dd", {}, R.round(s.max)),
          el("dt", {}, "Reads"),
          el("dd", {}, `${s.reads} over ${R.round(c.meta.plate.time[1] - c.meta.plate.time[0])} ${unit}`),
        ]
      : []),
  );
  $("extra").replaceChildren(el("h3", {}, "Selected well"), dl);
}

// ----- zooming images --------------------------------------------------------------------------------

function zoomBar() {
  const b = (label, text, f) => el("button", { type: "button", "aria-label": label, title: label, onclick: (e) => (e.stopPropagation(), f()) }, text);
  return el(
    "div",
    { class: "zoombar", onpointerdown: (e) => e.stopPropagation() },
    b("Zoom out", "−", () => zoom.by(0.5)),
    b("Zoom in", "+", () => zoom.by(2)),
    b("Whole image", "⤢", () => zoom.fit()),
  );
}

/**
 * Zooming feels immediate: the picture already shown is scaled and moved with CSS while the
 * pointer moves, and OpenReadout draws the new region when the gesture ends.
 */
const zoom = (() => {
  let t = { x: 0, y: 0, k: 1 }; // CSS transform of the shown picture
  const pointers = new Map();
  let start = null;
  let wheelTimer;

  const apply = (animate = false) => {
    img.classList.toggle("settling", animate);
    img.style.transform = `translate(${t.x}px, ${t.y}px) scale(${t.k})`;
  };

  /** Ask for the region the transformed picture shows. */
  const commit = () => {
    const c = S.cur;
    if (!c?.meta?.image) return;
    const { cssW, cssH } = c.figure;
    const box = { x0: -t.x / t.k / cssW, y0: -t.y / t.k / cssH, x1: (cssW - t.x) / t.k / cssW, y1: (cssH - t.y) / t.k / cssH };
    const im = R.imageOf(c.info.value, c.view);
    const minPx = Math.max(48, Math.round(Math.min(cssW, cssH) / 3));
    const region = R.regionOf(c.meta, c.info.value, c.view, box, minPx);
    const same = JSON.stringify(region) === JSON.stringify(c.view.region);
    if (same) {
      t = { x: 0, y: 0, k: 1 };
      apply(true);
      return;
    }
    c.view.region = region;
    if (!region && im) c.view.region = undefined;
    redraw();
  };

  const at = (e) => {
    const r = frame.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const scaleAround = (p, f) => {
    t = { x: p.x - (p.x - t.x) * f, y: p.y - (p.y - t.y) * f, k: t.k * f };
  };

  frame.addEventListener("pointerdown", (e) => {
    const c = S.cur;
    if (!c?.meta) return;
    if (!c.meta.image) {
      if (c.pick) start = { p: at(e), time: e.timeStamp, pick: true };
      return;
    }
    frame.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, at(e));
    if (pointers.size === 1) start = { p: at(e), t: { ...t }, time: e.timeStamp, moved: false };
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      start = { pinch: Math.hypot(a.x - b.x, a.y - b.y), mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, t: { ...t }, moved: true };
    }
  });

  frame.addEventListener("pointermove", (e) => {
    if (!pointers.has(e.pointerId) || !start) return;
    pointers.set(e.pointerId, at(e));
    if (pointers.size === 2 && start.pinch) {
      const [a, b] = [...pointers.values()];
      const d = Math.hypot(a.x - b.x, a.y - b.y);
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      t = { ...start.t };
      scaleAround(start.mid, d / start.pinch);
      t.x += mid.x - start.mid.x;
      t.y += mid.y - start.mid.y;
      apply();
    } else if (pointers.size === 1 && !start.pinch) {
      const p = at(e);
      const dx = p.x - start.p.x;
      const dy = p.y - start.p.y;
      if (!start.moved && Math.hypot(dx, dy) > 6) {
        start.moved = true;
        frame.classList.add("dragging");
      }
      if (start.moved) {
        t = { ...start.t, x: start.t.x + dx, y: start.t.y + dy };
        apply();
      }
    }
  });

  const end = (e) => {
    if (start?.pick && !S.cur?.meta?.image) {
      const p = at(e);
      if (Math.hypot(p.x - start.p.x, p.y - start.p.y) < 8) S.cur.pick(e);
      start = null;
      return;
    }
    if (!pointers.has(e.pointerId)) return;
    pointers.delete(e.pointerId);
    frame.classList.remove("dragging");
    if (pointers.size > 0) {
      // one finger of a pinch lifted: carry on as a drag from here
      const [p] = [...pointers.values()];
      start = { p, t: { ...t }, time: e.timeStamp, moved: true };
      return;
    }
    const s = start;
    start = null;
    if (!s) return;
    if (!s.moved && e.type === "pointerup") {
      // a tap: zoom in around it
      scaleAround(at(e), 2);
      apply(true);
      setTimeout(commit, 60);
      return;
    }
    if (s.moved) commit();
  };
  frame.addEventListener("pointerup", end);
  frame.addEventListener("pointercancel", end);

  frame.addEventListener(
    "wheel",
    (e) => {
      if (!S.cur?.meta?.image) return;
      e.preventDefault();
      const f = Math.exp(-e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.0025));
      scaleAround(at(e), f);
      apply();
      clearTimeout(wheelTimer);
      wheelTimer = setTimeout(commit, 220);
    },
    { passive: false },
  );

  return {
    reset() {
      t = { x: 0, y: 0, k: 1 };
      pointers.clear();
      start = null;
    },
    by(f) {
      const c = S.cur;
      if (!c?.figure) return;
      scaleAround({ x: c.figure.cssW / 2, y: c.figure.cssH / 2 }, f);
      apply(true);
      setTimeout(commit, 120);
    },
    fit() {
      const c = S.cur;
      if (!c?.view.region) return;
      c.view.region = undefined;
      redraw();
    },
  };
})();

// ----- wiring ------------------------------------------------------------------------------------------

for (const card of document.querySelectorAll(".card")) {
  card.addEventListener("click", () => openSample(card.dataset.sample, { scroll: true }));
}

$("pick").addEventListener("change", (e) => {
  openOwn(e.target.files?.[0]);
  e.target.value = "";
});

let dragDepth = 0;
const hasFiles = (e) => [...(e.dataTransfer?.types ?? [])].includes("Files");
window.addEventListener("dragenter", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth++;
  $("dropzone").hidden = false;
});
window.addEventListener("dragover", (e) => {
  if (hasFiles(e)) e.preventDefault();
});
window.addEventListener("dragleave", () => {
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) $("dropzone").hidden = true;
});
window.addEventListener("drop", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth = 0;
  $("dropzone").hidden = true;
  openOwn(e.dataTransfer.files[0]);
});

new ResizeObserver(() => {
  const w = $("stage").clientWidth;
  if (S.cur?.meta && Math.abs(w - lastWidth) > 24) {
    lastWidth = w;
    clearTimeout(S.resizeTimer);
    S.resizeTimer = setTimeout(() => {
      redraw();
      if (S.cur?.second) drawSecond();
    }, 200);
  }
}).observe($("stage"));

window.addEventListener("hashchange", () => {
  const id = location.hash.slice(1);
  if (SAMPLES.some((s) => s.id === id)) openSample(id, { scroll: true });
});

syncThumbs();
{
  const id = location.hash.slice(1);
  openSample(SAMPLES.some((s) => s.id === id) ? id : FIRST, { scroll: false });
}
