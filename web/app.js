// The demo page: sample cards, your own file, and the result with its picture and controls.
// OpenReadout runs in a Web Worker (engine.js, core.js), so the page stays responsive while it
// draws. The page's Content-Security-Policy (`connect-src 'self'`) lets it fetch only its own
// files from this site, so a file you open can't leave your device. reader.js turns the JSON
// into what the page lists, and camera.js holds the pan and zoom arithmetic.

import { BUILT } from "./built.js";
import * as C from "./camera.js";
import { call, progress } from "./engine.js";
import * as R from "./reader.js";
import { FIRST, SAMPLES } from "./samples.js";

const $ = (id) => document.getElementById(id);
const touch = matchMedia("(hover: none)").matches;
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
const dpr = () => window.devicePixelRatio || 1;

const frame = $("frame");
const PLOT_MAX = 960; // plots wider than this only grow their type
const CACHE = 40; // pictures kept per file

const S = {
  files: new Map(), // key -> the state of an opened file (kept, so going back is instant)
  cur: null, // the file shown
  gen: 0, // the latest open request; older ones give way
  own: 0, // files of your own opened so far
};

// ----- small DOM helpers -----------------------------------------------------------------------

function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === false || v === null) continue;
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) if (c != null && c !== false) e.append(typeof c === "string" ? document.createTextNode(c) : c);
  return e;
}

function errorText(err) {
  return el("p", { class: "error" }, err?.message ?? "This failed.", err?.hint ? el("span", { class: "why" }, err.hint) : null);
}

const ignore = (e) => {
  if (!e?.superseded) console.error(e);
};

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
  const F = S.cur;
  if (F?.mode === "plot") drawPlot(F).catch(ignore);
  if (F?.second) drawSecond(F).catch(ignore);
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

// ----- the busy state ------------------------------------------------------------------------------

let workId = 0;

/**
 * Show that the page is working: the picture dims under a small spinner with `text` (or, with
 * `corner`, stays as it is with the spinner in a corner), and `control` is marked busy with its
 * buttons disabled. Returns `{ text(t), done() }`.
 */
function startWork(text, { control, corner = false } = {}) {
  const id = ++workId;
  const box = $("busy");
  box.className = corner ? "busy corner" : "busy";
  $("busy-text").textContent = text;
  box.hidden = false;
  frame.classList.toggle("dim", !corner);
  const chips = control ? [...control.querySelectorAll(".chip")] : [];
  if (control) control.setAttribute("aria-busy", "true");
  for (const c of chips) c.disabled = true;
  let ended = false;
  return {
    text(t) {
      if (id === workId) $("busy-text").textContent = t;
    },
    done() {
      if (ended) return;
      ended = true;
      if (control) control.removeAttribute("aria-busy");
      for (const c of chips) c.disabled = false;
      if (id === workId) {
        box.hidden = true;
        frame.classList.remove("dim");
      }
    },
  };
}

// ----- pictures ------------------------------------------------------------------------------------

/**
 * A preview of the file as `{ meta, url }`, drawn in the worker (one per `lane` at a time) and
 * kept per file, so going back to a view is instant.
 */
function picture(F, options, lane, background = false) {
  const key = JSON.stringify(options);
  const hit = F.pics.get(key);
  if (hit) {
    F.pics.delete(key);
    F.pics.set(key, hit); // most recently used last
    return hit.promise;
  }
  const entry = { done: false, pic: null };
  entry.promise = call("preview", { key: F.key, options }, { lane, background }).then(async (r) => {
    const url = URL.createObjectURL(new Blob([r.data], { type: r.type }));
    const im = new Image();
    im.src = url;
    await im.decode().catch(() => {});
    entry.done = true;
    entry.pic = { meta: r.meta, url };
    return entry.pic;
  });
  entry.promise.catch(() => F.pics.get(key) === entry && F.pics.delete(key));
  F.pics.set(key, entry);
  for (const [k, e] of F.pics) {
    if (F.pics.size <= CACHE) break;
    if (!e.done || inUse(F, e.pic)) continue;
    F.pics.delete(k);
    URL.revokeObjectURL(e.pic.url);
  }
  return entry.promise;
}

/** Whether a picture is ready without waiting. */
const cached = (F, options) => F.pics.get(JSON.stringify(options))?.done === true;

const inUse = (F, pic) => F.layers.some((l) => l.pic === pic) || F.plotPic === pic || F.secondPic === pic;

// ----- loading the samples -------------------------------------------------------------------------

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

async function fetchSample(s, onProgress) {
  const b = BUILT.samples[s.id];
  const packed = b.gz !== null && typeof DecompressionStream === "function";
  const res = await fetch(`samples/${encodeURIComponent(s.file)}${packed ? ".gz" : ""}`);
  if (!res.ok) throw new Error(`the sample did not load (HTTP ${res.status})`);
  let body = counted(res, packed ? b.gz : b.bytes, onProgress);
  if (packed) body = body.pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(body).arrayBuffer());
}

let moduleLoaded = false;

// ----- opening files -------------------------------------------------------------------------------

function pressCard(id) {
  for (const c of document.querySelectorAll(".card")) c.setAttribute("aria-pressed", String(c.dataset.sample === id));
}

function cardBusy(id, on) {
  const c = document.querySelector(`.card[data-sample="${id}"]`);
  if (on) c?.setAttribute("aria-busy", "true");
  else c?.removeAttribute("aria-busy");
}

/** Before anything is shown: the card's picture, dimmed, where the result will be. */
function placeholder(s) {
  frame.className = "frame placeholder dim";
  frame.style.cssText = "";
  clearFrame();
  const t = s && BUILT.samples[s.id].thumb[isDark() && BUILT.samples[s.id].thumb.dark ? "dark" : "light"];
  if (t) {
    frame.classList.toggle("plot", !BUILT.samples[s.id].image);
    frame.prepend(el("img", { src: t.src, alt: "" }));
  }
  $("r-title").textContent = s ? s.subject : "";
  $("r-meta").replaceChildren();
}

async function openSample(id, { scroll = true } = {}) {
  const s = SAMPLES.find((x) => x.id === id);
  if (!s) return;
  const gen = ++S.gen;
  pressCard(id);
  if (location.hash !== `#${id}`) history.replaceState(null, "", `#${id}`);
  const key = `sample:${id}`;
  let F = S.files.get(key);
  if (F) {
    await show(F, gen, startWork("Drawing"), scroll);
    return;
  }
  if (!S.cur) placeholder(s);
  cardBusy(id, true);
  const work = startWork(`Opening the ${s.label.toLowerCase()} sample`);
  try {
    const b = BUILT.samples[id];
    const sampleBytes = b.gz ?? b.bytes;
    const moduleBytes = moduleLoaded ? 0 : BUILT.wasm.bytes * 0.42; // about its gzipped size
    let fm = 0;
    let fs = 0;
    const report = () => {
      if (gen !== S.gen) return;
      const f = (fm * moduleBytes + fs * sampleBytes) / (moduleBytes + sampleBytes);
      work.text(`Loading ${Math.round(f * 100)}%`);
    };
    progress((f) => {
      fm = f;
      report();
    });
    const [bytes] = await Promise.all([
      fetchSample(s, (f) => {
        fs = f;
        report();
      }),
      call("load").then(() => (moduleLoaded = true)),
    ]);
    if (gen === S.gen) work.text(`Opening the ${s.label.toLowerCase()} sample`);
    const size = bytes.byteLength; // the bytes move to the worker
    const r = await call("open", { key, source: bytes, name: s.file }, { transfer: [bytes.buffer] });
    F = fileState(key, r, s, size);
    S.files.set(key, F);
    if (gen === S.gen) await show(F, gen, work, scroll);
  } catch (e) {
    if (gen === S.gen) showOpenError(e);
  } finally {
    cardBusy(id, false);
    work.done();
  }
}

async function openOwn(file) {
  if (!file) return;
  const gen = ++S.gen;
  pressCard(null);
  history.replaceState(null, "", location.pathname + location.search);
  if (!S.cur) placeholder(null);
  const work = startWork(`Opening ${file.name}`);
  const key = `own:${++S.own}`;
  try {
    if (!moduleLoaded) {
      progress((f) => gen === S.gen && work.text(`Loading ${Math.round(f * 100)}%`));
      await call("load");
      moduleLoaded = true;
      work.text(`Opening ${file.name}`);
    }
    const r = await call("open", { key, source: file, name: file.name });
    // keep only the latest file of your own
    for (const [k, old] of S.files) {
      if (k.startsWith("own:")) {
        S.files.delete(k);
        forget(old);
      }
    }
    const F = fileState(key, r, null, file.size);
    S.files.set(key, F);
    if (gen === S.gen) await show(F, gen, work, true);
    else forget(F);
  } catch (e) {
    if (gen === S.gen) showOpenError(e);
  } finally {
    work.done();
  }
}

function forget(F) {
  call("close", { key: F.key }).catch(() => {});
  for (const e of F.pics.values()) if (e.done) URL.revokeObjectURL(e.pic.url);
}

function showOpenError(e) {
  S.cur = null;
  frame.className = "frame";
  frame.style.cssText = "";
  clearFrame();
  frame.prepend(errorText({ message: `Could not open the file: ${e.message}`, hint: e.hint }));
  $("controls").replaceChildren();
  $("numbers").replaceChildren();
  $("hint").textContent = "";
}

function fileState(key, r, sample, size) {
  const info = r.info.ok ? r.info.value : null;
  const v = info ? R.firstView(info, sample?.view) : null;
  return {
    key,
    name: r.name,
    size,
    sample,
    info,
    infoError: r.info.ok ? null : r.info.error,
    explain: r.explain,
    check: r.check,
    v,
    mode: v?.kind === "image" ? "viewer" : "plot",
    pics: new Map(),
    layers: [], // the pictures placed in the image viewer
    plotPic: null,
    secondPic: null,
  };
}

// ----- showing a file ------------------------------------------------------------------------------

/**
 * Show a file: draw its first picture while the previous file stays on screen, then swap
 * everything at once.
 */
async function show(F, gen, work, scroll) {
  try {
    if (F.info) {
      if (F.mode === "viewer") await prepareViewer(F);
      else await preparePlot(F);
    }
  } catch (e) {
    if (e?.superseded || gen !== S.gen) return;
    F.drawError = e;
  }
  if (gen !== S.gen) return;
  work.done();
  S.cur = F;
  $("result").removeAttribute("aria-busy");
  renderHead(F);
  renderWords(F);
  renderFacts(F);
  $("json").textContent = JSON.stringify(F.info ?? F.infoError, null, 2);
  $("extra").replaceChildren();
  $("second").hidden = true;
  $("hint2").textContent = "";
  if (!F.info || F.drawError) {
    showFigureError(F.infoError ?? F.drawError);
  } else if (F.mode === "viewer") {
    mountViewer(F);
  } else {
    mountPlot(F);
  }
  // on a phone the result is below the cards: bring its top into view
  const top = $("result").getBoundingClientRect().top;
  if (scroll && top > window.innerHeight * 0.7) $("result").scrollIntoView({ behavior: reducedMotion.matches ? "auto" : "smooth", block: "start" });
}

function clearFrame() {
  for (const c of [...frame.children]) if (!["scalebar", "zoombar", "busy"].includes(c.id)) c.remove();
}

function showFigureError(err) {
  frame.className = "frame";
  frame.style.cssText = "";
  clearFrame();
  frame.prepend(errorText(err));
  $("scalebar").hidden = true;
  $("zoombar").hidden = true;
  $("controls").replaceChildren();
  $("numbers").replaceChildren();
  $("hint").textContent = "";
  $("cmd").textContent = "";
}

function renderHead(F) {
  $("r-title").textContent = F.sample ? F.sample.subject : F.name;
  const format = F.sample?.format ?? (F.info ? R.formatName(F.info) : "Format not recognised");
  const meta = $("r-meta");
  meta.replaceChildren(el("code", {}, F.name), ` · ${format} · ${R.human(F.size)}`);
  if (F.sample) {
    meta.append(" · ", el("a", { href: `samples/${encodeURIComponent(F.sample.file)}`, download: F.sample.file }, "Download"));
  }
}

function renderWords(F) {
  const box = $("words");
  box.replaceChildren();
  // a file that can't be read says so once, where the picture would be
  box.closest(".panel").hidden = !F.info;
  $("h-cli").closest(".cli").hidden = !F.info;
  if (!F.info) return;
  if (!F.explain.ok) {
    box.append(errorText(F.explain.error));
    return;
  }
  const e = F.explain.value;
  const summary = F.info ? e.summary.replace(F.info.format.name, R.formatName(F.info)) : e.summary;
  box.append(el("p", { class: "summary" }, summary));
  const paras = e.paragraphs ?? [];
  if (paras[0]) box.append(el("p", {}, paras[0]));
  const more = [...paras.slice(1), ...(e.caveats ?? [])];
  if (more.length) box.append(el("details", {}, el("summary", {}, "More detail"), ...more.map((p) => el("p", {}, p))));
}

function renderFacts(F) {
  const facts = $("facts");
  const list = $("measured");
  facts.replaceChildren();
  list.replaceChildren();
  const row = (k, v, cls) => facts.append(el("dt", {}, k), el("dd", { class: cls }, v));
  if (!F.info) {
    row("Format", "Not recognised");
  } else {
    for (const m of R.measurements(F.info)) list.append(el("li", {}, m));
    for (const [k, v] of R.facts(F.info)) row(k, v);
  }
  if (!F.check.ok) {
    if (F.info) row("Integrity check", F.check.error.message, "fail");
  } else {
    const findings = F.check.value.findings ?? [];
    if (F.check.value.ok) row("Integrity check", findings.length ? `Passed, ${findings.length} note${findings.length > 1 ? "s" : ""}` : "Passed", "pass");
    else row("Integrity check", findings.map((f) => f.message).join("; ") || "Problems found", "fail");
  }
  if (F.sample) row("Source", el("span", {}, el("a", { href: F.sample.href }, F.sample.credit), ` (${F.sample.licence})`));
}

function renderNumbers(F) {
  const dl = $("numbers");
  dl.replaceChildren();
  let list;
  if (F.mode === "viewer") {
    const meta = F.layers[0]?.pic.meta;
    if (!meta) return;
    let objects = null;
    if (F.v.objects) {
      const d = F.layers.find((l) => l.detail && l.sk === settingsKey(F.v));
      objects = d && d.want === F.want ? d.pic.meta.objects : { kind: "nuclei", count: "Counting…" };
    }
    list = R.numbers(F.info, F.v, meta, { region: C.visible(F.cam, F.vp, F.img), zoom: F.cam.z / C.fitZoom(F.vp, F.img), objects });
  } else {
    if (!F.plotPic) return;
    list = R.numbers(F.info, F.v, F.plotPic.meta);
  }
  for (const n of list) {
    dl.append(el("dt", { class: n.wide ? "wide" : undefined }, n.label), el("dd", { class: [n.strong ? "strong" : "", n.wide ? "wide" : ""].join(" ").trim() || undefined }, n.value));
  }
}

function renderCommand(F) {
  let v = F.v;
  if (F.mode === "viewer") {
    const r = C.visible(F.cam, F.vp, F.img);
    const whole = r.width >= F.img.w - 0.5 && r.height >= F.img.h - 0.5;
    v = { ...v, region: whole ? undefined : { x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) } };
  }
  $("cmd").textContent = R.command(F.name, F.info, v);
}

// ----- the image viewer ----------------------------------------------------------------------------

/**
 * What a picture depends on besides the region: channels and plane, and for the picture of the
 * region in view also whether nuclei are outlined. The whole image (the base) never has them:
 * counting nuclei in all of a large image takes too long.
 */
const settingsKey = (v, detail = true) => JSON.stringify([v.image ?? 0, v.channels, v.z, v.t, detail && Boolean(v.objects)]);

/** The viewport for a file: the stage's width, the shape of the first view, a height limit. */
function viewportFor(F) {
  const W = Math.max(200, $("stage").clientWidth);
  const maxH = Math.round(Math.min(820, Math.max(280, window.innerHeight * (touch ? 0.6 : 0.72))));
  const r = F.home;
  const aspect = r.height / r.width;
  let w = W;
  let h = w * aspect;
  if (h > maxH) {
    h = maxH;
    w = Math.min(W, Math.max(h / aspect, W * 0.5));
  }
  return { w: Math.round(w), h: Math.round(h) };
}

function initViewer(F) {
  const im = R.imageOf(F.info, F.v);
  F.img = { w: im.size_x, h: im.size_y };
  F.home = F.v.region ?? { x: 0, y: 0, width: im.size_x, height: im.size_y };
  F.v.region = undefined; // the camera says what is shown
  F.vp = viewportFor(F);
  F.homeCam = C.showRect(F.home, F.vp, F.img);
  F.cam = F.homeCam;
}

async function prepareViewer(F) {
  if (!F.vp) initViewer(F);
  if (F.layers.length) return;
  await refine(F);
}

function mountViewer(F) {
  frame.className = "frame viewer";
  frame.tabIndex = 0;
  frame.setAttribute("aria-label", "Image. Use + and − to zoom, the arrow keys to move, 0 to reset.");
  clearFrame();
  fitStage(F);
  sizeFrame(F);
  for (const l of F.layers) frame.insertBefore(l.el, $("scalebar"));
  $("zoombar").hidden = false;
  $("hint").textContent = touch ? "Pinch to zoom, drag to move, double-tap to reset." : "Scroll to zoom, drag to move, double-click to reset.";
  setupImageControls(F);
  applyCam(F);
  renderCommand(F);
  F.cq ??= null;
  if (F.layers.length < 2 || F.v.objects) settleSoon(F, 0);
}

/** Fit the viewport to the stage's width again, showing the same part of the image. */
function fitStage(F) {
  const vp = viewportFor(F);
  if (vp.w === F.vp.w && vp.h === F.vp.h) return false;
  const fitOld = C.fitZoom(F.vp, F.img);
  const fit = C.fitZoom(vp, F.img);
  F.homeCam = C.clamp({ ...F.homeCam, z: (F.homeCam.z / fitOld) * fit }, vp, F.img);
  F.cam = C.clamp({ ...F.cam, z: (F.cam.z / fitOld) * fit }, vp, F.img);
  F.vp = vp;
  return true;
}

function sizeFrame(F) {
  frame.style.width = `${F.vp.w}px`;
  frame.style.height = `${F.vp.h}px`;
}

let numbersFrame = 0;

/** Place every picture for the camera, and update what depends on it. */
function applyCam(F) {
  if (S.cur !== F || F.mode !== "viewer") return;
  for (const l of F.layers) {
    const p = C.place(l.rect, F.cam, F.vp);
    l.el.style.transform = `translate(${p.x}px, ${p.y}px) scale(${p.s})`;
    // past one picture pixel per two screen pixels, show the pixels as squares
    l.el.classList.toggle("pixels", l.pic.meta.width / l.rect.width < 0.5 * F.cam.z * dpr());
  }
  const { min, max } = C.limits(F.vp, F.img);
  $("zoom-out").disabled = F.cam.z <= min * 1.0001;
  $("zoom-in").disabled = F.cam.z >= max * 0.9999;
  $("zoom-reset").disabled = C.same(F.cam, F.homeCam);
  frame.classList.toggle("zoomed", F.cam.z > min * 1.0001);
  updateScalebar(F);
  cancelAnimationFrame(numbersFrame);
  numbersFrame = requestAnimationFrame(() => S.cur === F && renderNumbers(F));
}

function setCam(F, cam, { settle = true } = {}) {
  F.cam = cam;
  applyCam(F);
  if (settle) settleSoon(F);
}

let settleTimer;

/** Draw a sharper picture once the camera has been still for a moment. */
function settleSoon(F, ms = 160) {
  clearTimeout(settleTimer);
  settleTimer = setTimeout(() => {
    if (S.cur !== F) return;
    renderCommand(F);
    refine(F, { quiet: true }).catch(ignore);
  }, ms);
}

/**
 * Ask for the pictures the camera needs: the whole image (the base, once per setting) and,
 * when the base is too coarse for the zoom or nuclei are counted, the region in view. The
 * promise resolves when the more important one is placed.
 */
function refine(F, { quiet = false, control } = {}) {
  const sk = settingsKey(F.v);
  const bk = settingsKey(F.v, false);
  const fit = C.fitZoom(F.vp, F.img);
  const scale = dpr();
  const base = F.layers.find((l) => !l.detail && l.sk === bk);
  // nuclei are counted in what is in view only: counting takes time in proportion to the area
  const region = C.regionFor(F.cam, F.vp, F.img, F.v.objects ? 0 : 0.1);
  const coarse = !base || base.pic.meta.width / F.img.w < F.cam.z * scale * 0.9;
  const wantDetail = F.v.objects || (coarse && region !== undefined) || (coarse && base && F.cam.z > fit * 1.01);
  const jobs = [];
  if (wantDetail) {
    const r = region ?? { x: 0, y: 0, width: F.img.w, height: F.img.h };
    const opts = R.imageOptions(F.v, { region, size: Math.max(r.width, r.height) * F.cam.z * scale });
    const current = F.layers.find((l) => l.detail && l.sk === sk && l.key === JSON.stringify(opts));
    F.want = JSON.stringify(opts);
    if (!current) jobs.push({ detail: true, opts });
  }
  if (!base) jobs.push({ detail: false, opts: R.imageOptions({ ...F.v, objects: false }, { size: Math.max(F.img.w, F.img.h) * fit * scale }) });
  if (!jobs.length) {
    if (S.cur === F) renderNumbers(F);
    return Promise.resolve();
  }
  // quiet: the picture in view is only sharpened (nuclei: recounted), so it isn't dimmed
  const slow = jobs.some((j) => !cached(F, j.opts));
  const work = S.cur === F && slow ? startWork(F.v.objects ? "Counting nuclei" : "Drawing", { control, corner: quiet }) : null;
  const done = jobs.map((j) =>
    picture(F, j.opts, j.detail ? "detail" : "base").then((pic) => {
      if (settingsKey(F.v) !== sk) return; // the settings changed meanwhile
      addLayer(F, { detail: j.detail, key: JSON.stringify(j.opts), want: JSON.stringify(j.opts), pic, sk: j.detail ? sk : bk });
    }),
  );
  if (S.cur === F && F.v.objects) renderNumbers(F);
  const first = done[0];
  Promise.allSettled(done).then(() => work?.done());
  return first.finally(() => work?.done());
}

function addLayer(F, l) {
  l.rect = l.pic.meta.image.full_res_region;
  l.el = el("img", { alt: altText(F), decoding: "async", draggable: "false" });
  l.el.src = l.pic.url;
  l.el.style.width = `${l.rect.width}px`;
  l.el.style.height = `${l.rect.height}px`;
  // a picture of other settings goes; a new picture of the same kind replaces the old one
  const keep = F.layers.filter((o) => o.detail !== l.detail && o.sk === settingsKey(F.v, o.detail));
  for (const o of F.layers) if (!keep.includes(o)) o.el.remove();
  F.layers = l.detail ? [...keep, l] : [l, ...keep];
  if (S.cur === F) {
    const after = l.detail ? $("scalebar") : (keep[0]?.el ?? $("scalebar"));
    frame.insertBefore(l.el, after);
    applyCam(F);
    renderNumbers(F);
  }
}

function altText(F) {
  return `${F.sample ? F.sample.subject : F.name}, image`;
}

function updateScalebar(F) {
  const bar = $("scalebar");
  const ps = R.imageOf(F.info, F.v)?.physical_size;
  if (!ps?.x || ps.unit !== "µm") {
    bar.hidden = true;
    return;
  }
  const umPerCss = ps.x / F.cam.z;
  const target = umPerCss * Math.min(110, F.vp.w * 0.3);
  const steps = [0.1, 0.2, 0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000];
  const nice = steps.reduce((a, b) => (b <= target ? b : a), steps[0]);
  bar.hidden = false;
  bar.querySelector("span").style.width = `${nice / umPerCss}px`;
  bar.querySelector("b").textContent = nice >= 1000 ? `${nice / 1000} mm` : `${nice} µm`;
}

/** Glide to a camera (buttons and keys); a gesture moves the camera directly. */
let glide = 0;
let gliding = null; // the camera a glide is going to

/** The camera to build on: where a glide is going, or where the camera is. */
const aim = (F) => gliding ?? F.cam;

function stopGlide() {
  cancelAnimationFrame(glide);
  gliding = null;
}

function animateTo(F, to) {
  cancelAnimationFrame(glide);
  gliding = to;
  const from = F.cam;
  if (reducedMotion.matches || C.same(from, to)) {
    gliding = null;
    setCam(F, to);
    return;
  }
  const t0 = performance.now();
  const step = (now) => {
    const t = Math.min(1, (now - t0) / 200);
    const e = 1 - (1 - t) ** 3;
    const z = from.z * (to.z / from.z) ** e;
    if (t === 1) gliding = null;
    setCam(F, t === 1 ? to : { cx: from.cx + (to.cx - from.cx) * e, cy: from.cy + (to.cy - from.cy) * e, z }, { settle: t === 1 });
    if (t < 1 && S.cur === F) glide = requestAnimationFrame(step);
    else gliding = null;
  };
  glide = requestAnimationFrame(step);
}

const viewer = () => (S.cur?.mode === "viewer" && S.cur.vp && S.cur.layers.length ? S.cur : null);

function zoomButton(f) {
  const F = viewer();
  if (F) animateTo(F, C.zoomAt(aim(F), f, F.vp.w / 2, F.vp.h / 2, F.vp, F.img));
}

function resetView() {
  const F = viewer();
  if (F) animateTo(F, F.homeCam);
}

$("zoom-in").addEventListener("click", () => zoomButton(2));
$("zoom-out").addEventListener("click", () => zoomButton(0.5));
$("zoom-reset").addEventListener("click", resetView);

// Gestures. A drag pans, two fingers pinch, the wheel or a trackpad pinch zooms around the
// pointer, a double-click or double-tap resets. Each move starts again from the camera at the
// start of the gesture, so nothing accumulates.
{
  const pointers = new Map();
  let g = null;
  let lastTap = null;
  let lastScroll = -1e9;
  let lastWheel = -1e9;

  const at = (e) => {
    const r = frame.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const begin = () => {
    const F = viewer();
    g = F ? { cam: F.cam, ps: [...pointers.values()].map((p) => ({ ...p })), moved: g?.moved ?? false } : null;
  };

  frame.addEventListener("pointerdown", (e) => {
    const F = viewer();
    if (!F || e.target.closest(".zoombar") || (e.pointerType === "mouse" && e.button !== 0)) return;
    // no text selection or image drag while panning; focus for the keys
    e.preventDefault();
    frame.focus({ preventScroll: true });
    stopGlide();
    frame.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, at(e));
    g = null;
    begin();
  });

  frame.addEventListener("pointermove", (e) => {
    const F = viewer();
    if (!F || !g || !pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, at(e));
    const ps = [...pointers.values()];
    if (ps.length === 1 && g.ps.length === 1) {
      const dx = ps[0].x - g.ps[0].x;
      const dy = ps[0].y - g.ps[0].y;
      if (!g.moved && Math.hypot(dx, dy) < 4) return;
      g.moved = true;
      frame.classList.add("dragging");
      setCam(F, C.panBy(g.cam, dx, dy, F.vp, F.img));
    } else if (ps.length >= 2 && g.ps.length >= 2) {
      const [a0, b0] = g.ps;
      const [a1, b1] = ps;
      const d0 = Math.hypot(a0.x - b0.x, a0.y - b0.y) || 1;
      const d1 = Math.hypot(a1.x - b1.x, a1.y - b1.y);
      const m0 = { x: (a0.x + b0.x) / 2, y: (a0.y + b0.y) / 2 };
      const m1 = { x: (a1.x + b1.x) / 2, y: (a1.y + b1.y) / 2 };
      g.moved = true;
      const zoomed = C.zoomAt(g.cam, d1 / d0, m0.x, m0.y, F.vp, F.img);
      setCam(F, C.panBy(zoomed, m1.x - m0.x, m1.y - m0.y, F.vp, F.img));
    }
  });

  const end = (e) => {
    if (!pointers.has(e.pointerId)) return;
    const p = at(e);
    pointers.delete(e.pointerId);
    if (pointers.size > 0) {
      begin(); // a finger lifted from a pinch: carry on from here
      return;
    }
    frame.classList.remove("dragging");
    const tap = g && !g.moved && e.type === "pointerup";
    g = null;
    if (tap && e.pointerType !== "mouse") {
      const now = e.timeStamp;
      if (lastTap && now - lastTap.t < 320 && Math.hypot(p.x - lastTap.x, p.y - lastTap.y) < 30) {
        lastTap = null;
        resetView();
      } else lastTap = { t: now, ...p };
    }
  };
  frame.addEventListener("pointerup", end);
  frame.addEventListener("pointercancel", end);
  frame.addEventListener("dblclick", (e) => {
    if (viewer() && !e.target.closest(".zoombar")) resetView();
  });

  // The wheel zooms only when it starts on the image: while the page is scrolling under the
  // pointer, and at the zoom limits, it keeps scrolling the page.
  window.addEventListener("scroll", () => (lastScroll = performance.now()), { passive: true });
  frame.addEventListener(
    "wheel",
    (e) => {
      const F = viewer();
      if (!F) return;
      const now = performance.now();
      const going = now - lastWheel < 300;
      if (!e.ctrlKey && !going && now - lastScroll < 400) return;
      const dy = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 400 : 1);
      const f = Math.exp(-Math.max(-300, Math.min(300, dy)) * (e.ctrlKey ? 0.01 : 0.0022));
      const { min, max } = C.limits(F.vp, F.img);
      const atLimit = (f < 1 && F.cam.z <= min * 1.0001) || (f > 1 && F.cam.z >= max * 0.9999);
      if (!e.ctrlKey && !going && atLimit) return;
      e.preventDefault();
      lastWheel = now;
      stopGlide();
      const p = at(e);
      setCam(F, C.zoomAt(F.cam, f, p.x, p.y, F.vp, F.img));
    },
    { passive: false },
  );

  frame.addEventListener("keydown", (e) => {
    const F = viewer();
    if (!F || e.altKey || e.metaKey || e.ctrlKey) return;
    const step = 0.15;
    const moves = {
      ArrowLeft: [F.vp.w * step, 0],
      ArrowRight: [-F.vp.w * step, 0],
      ArrowUp: [0, F.vp.h * step],
      ArrowDown: [0, -F.vp.h * step],
    };
    if (e.key === "+" || e.key === "=") zoomButton(1.5);
    else if (e.key === "-" || e.key === "_") zoomButton(1 / 1.5);
    else if (e.key === "0" || e.key === "Home") resetView();
    else if (moves[e.key]) animateTo(F, C.panBy(aim(F), ...moves[e.key], F.vp, F.img));
    else return;
    e.preventDefault();
  });
}

// ----- plots ---------------------------------------------------------------------------------------

const plotWidth = () => Math.min($("stage").clientWidth, PLOT_MAX);

function plotOptions(F) {
  return R.previewOptions(F.info, F.v, { width: plotWidth(), scale: dpr(), dark: isDark() });
}

async function preparePlot(F) {
  F.plotPic = await picture(F, plotOptions(F), "main");
}

function mountPlot(F) {
  frame.className = "frame";
  frame.removeAttribute("tabindex");
  frame.removeAttribute("aria-label");
  clearFrame();
  $("scalebar").hidden = true;
  $("zoombar").hidden = true;
  frame.prepend(el("img", { id: "plot", alt: "", decoding: "async" }));
  placePlot(F);
  setupPlotControls(F);
  afterPlot(F);
}

function placePlot(F) {
  const img = $("plot");
  if (!img || !F.plotPic) return;
  const m = F.plotPic.meta;
  const s = m.scale ?? plotOptions(F).scale;
  const cssW = m.width / s;
  const cssH = m.height / s;
  F.figure = { cssW, cssH };
  img.src = F.plotPic.url;
  img.alt = `${F.sample ? F.sample.subject : F.name}, ${m.kind} plot`;
  frame.style.width = `${Math.round(cssW)}px`;
  frame.style.height = "";
  frame.style.aspectRatio = `${cssW} / ${cssH}`;
  renderNumbers(F);
  renderCommand(F);
}

/** Draw the plot again for the current view (an option changed, or the theme or the width). */
async function drawPlot(F, { control } = {}) {
  const opts = plotOptions(F);
  const work = cached(F, opts) ? null : startWork("Drawing", { control });
  try {
    const pic = await picture(F, opts, "main");
    if (S.cur !== F || JSON.stringify(plotOptions(F)) !== JSON.stringify(opts)) return;
    F.plotPic = pic;
    placePlot(F);
    afterPlot(F);
  } finally {
    work?.done();
  }
}

/** Work that follows a drawing: per-kind extras that need the picture. */
function afterPlot(F) {
  const m = F.plotPic.meta;
  if (m.plate && F.wells === undefined) {
    F.wells = call("wellSeries", { key: F.key, table: m.plate.table ?? 0 }, { background: true }).then(
      (entries) => new Map(entries),
      () => new Map(),
    );
  }
  if (F.pick === pickRetentionTime && !F.second) {
    const top = (m.marks ?? []).filter((x) => x.kind === "peak").sort((a, b) => b.y - a.y)[0];
    if (top) selectRetentionTime(F, top.x);
  }
  if (F.second && $("second").hidden) drawSecond(F).catch(ignore);
  if ($("spikebars")) spikeBars(F);
  if (F.cq === undefined) {
    F.cq = null;
    call("cqBySample", { key: F.key }, { background: true }).then((rows) => {
      if (rows.length) F.cq = rows;
      if (S.cur === F && rows.length) $("extra").replaceChildren(cqTable(rows));
    }, ignore);
  } else if (F.cq) $("extra").replaceChildren(cqTable(F.cq));
}

// ----- controls ----------------------------------------------------------------------------------------

function control(label, ...children) {
  return el("div", { class: "control" }, el("span", { class: "c-label" }, label), ...children);
}

function selectControl(label, options, value, onChange) {
  const sel = el("select", { class: "select", "aria-label": label });
  for (const [v, text] of options) sel.append(el("option", { value: v, selected: v === value }, text));
  const box = control(label, sel);
  sel.addEventListener("change", () => onChange(sel.value, box));
  return box;
}

function stepper(label, value, count, onChange) {
  let cur = value ?? 0;
  const out = el("output", {});
  const range = el("input", { type: "range", min: "0", max: String(count - 1), step: "1", value: String(cur), "aria-label": label });
  const prev = el("button", { type: "button", "aria-label": `Previous ${label.toLowerCase()}` }, "‹");
  const next = el("button", { type: "button", "aria-label": `Next ${label.toLowerCase()}` }, "›");
  let wrap;
  const set = (v, fire = true) => {
    cur = Math.max(0, Math.min(count - 1, v));
    range.value = String(cur);
    out.textContent = `${cur} of ${count}`;
    prev.disabled = cur === 0;
    next.disabled = cur === count - 1;
    if (fire) onChange(cur, wrap);
  };
  prev.addEventListener("click", () => set(cur - 1));
  next.addEventListener("click", () => set(cur + 1));
  range.addEventListener("input", () => set(Number(range.value)));
  wrap = control(label, el("div", { class: "stepper" }, prev, out, next), range);
  wrap.setStep = (v) => set(v);
  set(cur, false);
  return wrap;
}

/** Draw the image again with new settings; the old picture stays, dimmed, until then. */
function restyle(F, box) {
  refine(F, { control: box }).catch(ignore);
}

function setupImageControls(F) {
  const box = $("controls");
  box.replaceChildren();
  const im = R.imageOf(F.info, F.v);
  if (F.v.channels && im.size_c > 1) box.append(channelChips(F, im));
  if (im.size_z > 1) box.append(stepper("Focal plane", F.v.z, im.size_z, (z, c) => ((F.v.z = z), restyle(F, c))));
  if (im.size_t > 1) box.append(stepper("Time point", F.v.t, im.size_t, (t, c) => ((F.v.t = t), restyle(F, c))));
  const b = el("button", { type: "button", class: "chip", "aria-pressed": String(Boolean(F.v.objects)) }, "Count nuclei");
  const nuclei = control("Nuclei", el("div", { class: "chips" }, b));
  b.addEventListener("click", () => {
    F.v.objects = !F.v.objects;
    b.setAttribute("aria-pressed", String(F.v.objects));
    restyle(F, nuclei);
  });
  box.append(nuclei);
}

function channelChips(F, im) {
  const chips = el("div", { class: "chips" });
  const box = control("Channels", chips);
  for (const ch of im.channels.slice(0, 4)) {
    const on = () => F.v.channels.includes(ch.index);
    const b = el("button", { type: "button", class: "chip", "aria-pressed": String(on()) }, el("span", { class: "dot" }), ch.fluorophore ?? ch.name ?? `Channel ${ch.index + 1}`);
    b.querySelector(".dot").style.background = ch.color ?? "#888";
    b.addEventListener("click", () => {
      const set = new Set(F.v.channels);
      if (set.has(ch.index)) {
        if (set.size === 1) return;
        set.delete(ch.index);
      } else set.add(ch.index);
      F.v.channels = [...set].sort((a, c) => a - c);
      b.setAttribute("aria-pressed", String(on()));
      restyle(F, box);
    });
    chips.append(b);
  }
  return box;
}

function setupPlotControls(F) {
  const info = F.info;
  const m = F.plotPic.meta;
  const box = $("controls");
  box.replaceChildren();
  frame.classList.remove("pickable");
  F.pick = null;
  let hint = "";
  const traces = R.traceChoices(info);
  if (m.trace && traces.length > 1) {
    box.append(
      selectControl("Recording", traces.map((t) => [String(t.index), t.name]), String(F.v.trace ?? m.trace.trace), (v, c) => {
        F.v.trace = Number(v);
        F.v.sweep = undefined;
        F.v.marks = R.firstView(info, { trace: F.v.trace }).marks;
        drawPlot(F, { control: c })
          .then(() => S.cur === F && setupPlotControls(F))
          .catch(ignore);
      }),
    );
  }
  const t = m.trace && traces.find((x) => x.index === m.trace.trace);
  if (t && t.sweeps > 1) {
    box.append(
      stepper("Sweep", m.trace.sweep, t.sweeps, (s, c) => {
        F.v.sweep = s;
        paintBars(F, s);
        drawPlot(F, { control: c }).catch(ignore);
      }),
    );
    if (F.v.marks === "spikes") box.append(el("div", { class: "control", id: "spikebars" }));
  }
  if (m.events) {
    const params = R.parameterChoices(info, m.events.table ?? 0).map((p) => [p.name, p.label]);
    const axis = (i, label) =>
      selectControl(label, params, [m.events.x, m.events.y][i], (v, c) => {
        const cols = [F.plotPic.meta.events.x, F.plotPic.meta.events.y];
        cols[i] = v;
        F.v.columns = cols;
        drawPlot(F, { control: c }).catch(ignore);
      });
    box.append(axis(0, "X axis"), axis(1, "Y axis"));
  }
  if (m.spectrum) {
    const run = info.spectra?.find((r) => r.index === (m.spectrum.run ?? 0));
    if (run?.scan_count > 1) {
      box.append(
        stepper("Scan", m.spectrum.index, run.scan_count, (i, c) => {
          F.v.spectrum = i;
          drawPlot(F, { control: c }).catch(ignore);
        }),
      );
    }
  }
  const timeAxis = m.trace?.x_axis?.quantity === "retention_time" || m.chromatogram;
  if (timeAxis && info.spectra?.length) {
    frame.classList.add("pickable");
    F.pick = pickRetentionTime;
    hint = `${touch ? "Tap" : "Click"} the chromatogram to show the mass spectrum at that time.`;
  }
  if (m.plate?.view === "curves") {
    frame.classList.add("pickable");
    F.pick = pickWell;
    hint = `${touch ? "Tap" : "Click"} a well to see its values.`;
  }
  $("hint").textContent = hint;
}

function spikeBars(F) {
  const t = F.plotPic.meta.trace;
  if (!t) return;
  const sweeps = R.traceChoices(F.info).find((x) => x.index === t.trace)?.sweeps ?? 1;
  if (!F.spikes || F.spikes.trace !== t.trace) {
    const counts = new Array(sweeps).fill(null);
    F.spikes = { trace: t.trace, counts };
    counts[t.sweep] = (F.plotPic.meta.marks ?? []).filter((m) => m.kind === "spike").length;
    // count the other sweeps in the background
    for (let s = 0; s < sweeps; s++) {
      if (counts[s] !== null) continue;
      call("spikeCount", { key: F.key, trace: t.trace, sweep: s }, { background: true }).then(
        (n) => {
          counts[s] = n;
          if (S.cur === F) paintBars(F);
        },
        () => (counts[s] = 0),
      );
    }
  }
  paintBars(F);
}

function paintBars(F, selected) {
  const box = $("spikebars");
  if (!box || !F.spikes) return;
  const counts = F.spikes.counts;
  const max = Math.max(1, ...counts.map((n) => n ?? 0));
  const cur = selected ?? F.v.sweep ?? F.plotPic.meta.trace.sweep;
  const bars = el("div", { class: "bars", role: "group", "aria-label": "Action potentials per sweep" });
  counts.forEach((n, i) => {
    const label = `Sweep ${i}: ${n ?? "counting"}`;
    const b = el("button", { type: "button", class: n === null ? "unknown" : undefined, "aria-pressed": String(i === cur), "aria-label": label, title: label });
    b.style.height = n === null ? "100%" : `${Math.max(4, (n / max) * 100)}%`;
    b.addEventListener("click", () => $("controls").querySelector(".stepper")?.closest(".control")?.setStep?.(i));
    bars.append(b);
  });
  box.replaceChildren(
    el("span", { class: "c-label" }, "Action potentials per sweep"),
    bars,
    el("div", { class: "bars-caption" }, el("span", {}, "0"), el("span", {}, String(counts.length - 1))),
  );
}

function cqTable(rows) {
  const t = el("table", { class: "cq" }, el("caption", {}, "Mean Cq by sample"));
  for (const r of rows.slice(0, 12)) t.append(el("tr", {}, el("td", {}, r.sample), el("td", {}, `${R.short(r.cq)} (${r.wells} well${r.wells > 1 ? "s" : ""})`)));
  return t;
}

// ----- picking on plots -----------------------------------------------------------------------------

/** The figure's coordinates (CSS pixels of the drawing) under a pointer event. */
function figurePoint(F, e) {
  const r = $("plot").getBoundingClientRect();
  const k = F.figure.cssW / r.width;
  return { x: (e.clientX - r.left) * k, y: (e.clientY - r.top) * k };
}

{
  let down = null;
  frame.addEventListener("pointerdown", (e) => {
    down = S.cur?.mode === "plot" && S.cur.pick ? { x: e.clientX, y: e.clientY } : null;
  });
  frame.addEventListener("pointerup", (e) => {
    const F = S.cur;
    if (down && F?.pick && Math.hypot(e.clientX - down.x, e.clientY - down.y) < 8) F.pick(F, e);
    down = null;
  });
}

function pickRetentionTime(F, e) {
  const m = F.plotPic.meta;
  const pt = figurePoint(F, e);
  const hit = R.pointAt(m, pt.x, pt.y);
  if (!hit) return;
  // snap to a labelled peak within a few pixels
  const perPx = (hit.panel.x[1] - hit.panel.x[0]) / hit.panel.plot_area.width;
  const near = (m.marks ?? [])
    .filter((x) => x.kind === "peak" && Math.abs(x.x - hit.x) < 10 * perPx)
    .sort((a, b) => Math.abs(a.x - hit.x) - Math.abs(b.x - hit.x))[0];
  selectRetentionTime(F, near ? near.x : hit.x);
}

async function selectRetentionTime(F, rt) {
  const run = F.plotPic.meta.chromatogram?.run ?? 0;
  const scan = await call("scanNear", { key: F.key, run, rt, window: 0.03 }, { lane: "scan" }).catch(() => undefined);
  if (S.cur !== F || !scan) return;
  F.second = { run, spectrum: scan.index, rt: scan.rt_s / 60 };
  F.v.annotations = [{ type: "vline", x: F.second.rt, color: "accent" }];
  drawPlot(F).catch(ignore);
  drawSecond(F).catch(ignore);
}

async function drawSecond(F) {
  if (!F.second) return;
  const v = { kind: "plot", run: F.second.run, spectrum: F.second.spectrum };
  const opts = R.previewOptions(F.info, v, { width: plotWidth(), scale: dpr(), dark: isDark() });
  const box = $("second");
  const spin = $("busy2");
  const slow = !cached(F, opts);
  if (slow && !box.hidden) {
    box.classList.add("dim");
    spin.hidden = false;
  }
  try {
    const pic = await picture(F, opts, "second");
    if (S.cur !== F) return;
    F.secondPic = pic;
    const img = $("figure2");
    img.src = pic.url;
    img.alt = `Mass spectrum at ${R.short(F.second.rt)} min`;
    box.style.width = `${Math.round(pic.meta.width / opts.scale)}px`;
    box.hidden = false;
    $("hint2").textContent = `Mass spectrum at ${R.short(F.second.rt)} min (scan ${pic.meta.spectrum?.scan_number ?? F.second.spectrum}).`;
  } catch (e) {
    if (e?.superseded || S.cur !== F) return;
    $("hint2").textContent = e.message;
  } finally {
    box.classList.remove("dim");
    spin.hidden = true;
  }
}

async function pickWell(F, e) {
  const m = F.plotPic.meta;
  const pt = figurePoint(F, e);
  const hit = R.pointAt(m, pt.x, pt.y);
  const well =
    hit?.panel.well ??
    (m.panels ?? []).find((p) => {
      const a = p.plot_area;
      return pt.x >= a.x - 4 && pt.x <= a.x + a.width + 4 && pt.y >= a.y - 4 && pt.y <= a.y + a.height + 4;
    })?.well;
  if (!well) return;
  F.v.annotations = [{ type: "well", well }];
  drawPlot(F).catch(ignore);
  const series = await F.wells;
  if (S.cur !== F) return;
  const s = series?.get(well);
  const unit = m.plate.time_unit ?? "";
  const rows = [["Well", well, "strong"]];
  if (s) {
    rows.push(["Start", R.short(s.first)], ["End", R.short(s.last)], ["Highest", R.short(s.max)]);
    rows.push(["Reads", `${s.reads} over ${R.short(m.plate.time[1] - m.plate.time[0])} ${unit}`]);
  }
  const dl = el("dl", {}, ...rows.flatMap(([k, v, cls]) => [el("dt", {}, k), el("dd", { class: cls }, v)]));
  $("extra").replaceChildren(el("h3", {}, "Selected well"), dl);
}

// ----- wiring ------------------------------------------------------------------------------------------

for (const card of document.querySelectorAll(".card")) {
  card.addEventListener("click", () => openSample(card.dataset.sample));
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

// A wider or narrower stage: the image viewport keeps showing the same part of the image, and
// plots are drawn again for the new width.
{
  let width = 0;
  let timer;
  new ResizeObserver(() => {
    const w = $("stage").clientWidth;
    if (Math.abs(w - width) < 1) return;
    width = w;
    const F = S.cur;
    if (!F?.info || F.drawError) return;
    if (F.mode === "viewer" && F.vp) {
      if (!fitStage(F)) return;
      sizeFrame(F);
      setCam(F, F.cam);
    } else if (F.mode === "plot") {
      clearTimeout(timer);
      timer = setTimeout(() => {
        if (S.cur !== F) return;
        if (Math.abs(plotWidth() - (F.figure?.cssW ?? 0)) > 24 || $("stage").clientWidth < (F.figure?.cssW ?? 0)) drawPlot(F).catch(ignore);
        if (F.second) drawSecond(F).catch(ignore);
      }, 200);
    }
  }).observe($("stage"));
}

window.addEventListener("hashchange", () => {
  const id = location.hash.slice(1);
  if (SAMPLES.some((s) => s.id === id)) openSample(id);
});

syncThumbs();
{
  const id = location.hash.slice(1);
  openSample(SAMPLES.some((s) => s.id === id) ? id : FIRST, { scroll: false });
}
