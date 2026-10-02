// The drop-a-file demo: everything runs in this tab. The page's Content-Security-Policy
// (`connect-src 'none'`) forbids network requests, so a dropped file cannot leave the machine,
// and the WebAssembly module is inlined (wasm-inline.js) because it may not be fetched either.

import { load, open } from "./openreadout/index.js";
import wasmBase64 from "./openreadout/wasm-inline.js";

let loaded = null;

function decodeBase64(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Instantiate the module from the inlined bytes (once). */
export function ready() {
  if (loaded === null) loaded = load(decodeBase64(wasmBase64));
  return loaded;
}

const settled = (p) =>
  p.then(
    (value) => ({ ok: true, value }),
    (error) => ({ ok: false, error: { message: error.message, code: error.code, hint: error.hint, exitCode: error.exitCode } }),
  );

/**
 * Read a dropped file: explain, info, check and a preview, each settled on its own so that a
 * file whose pixels cannot be decoded still shows its metadata.
 */
export async function analyze(file) {
  await ready();
  const started = performance.now();
  const f = await open(file);
  try {
    const info = await settled(f.info());
    const explain = await settled(f.info({ view: "explain" }));
    const check = await settled(f.check());
    const preview = await settled(f.preview({ max_size: 768 }));
    return {
      name: f.name,
      size: file.size ?? file.byteLength,
      info,
      explain,
      check,
      preview,
      bytesRead: f.bytesHeld || (file.size ?? file.byteLength),
      ms: Math.round(performance.now() - started),
    };
  } finally {
    f.close();
  }
}

function human(n) {
  if (!Number.isFinite(n)) return "";
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1000 && i < units.length - 1) {
    n /= 1000;
    i++;
  }
  return `${i === 0 ? n : n.toFixed(1)} ${units[i]}`;
}

/** What the page shows for a result, as plain data (the DOM code below only lays it out). */
export function model(r) {
  const fmt = r.info.ok ? r.info.value.format : null;
  const findings = r.check.ok ? r.check.value.findings ?? [] : [];
  return {
    title: r.name,
    subtitle: [fmt ? fmt.name : "unrecognised", human(r.size), `${r.ms} ms`].join(" · "),
    explain: r.explain.ok
      ? { summary: r.explain.value.summary, paragraphs: r.explain.value.paragraphs ?? [], caveats: r.explain.value.caveats ?? [] }
      : { error: r.explain.error },
    check: r.check.ok
      ? {
          ok: r.check.value.ok,
          label: r.check.value.ok ? "Integrity check passed" : "Integrity check found problems",
          findings: findings.map((x) => `${x.severity ?? ""} ${x.code ?? ""}: ${x.message ?? ""}`.trim()),
        }
      : { error: r.check.error },
    info: r.info.ok ? JSON.stringify(r.info.value, null, 2) : null,
    infoError: r.info.ok ? null : r.info.error,
    preview: r.preview.ok ? { png: r.preview.value.png, meta: r.preview.value.meta } : { error: r.preview.error },
  };
}

// ----- DOM ---------------------------------------------------------------------------------------

function el(doc, tag, attrs = {}, ...children) {
  const e = doc.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else e.setAttribute(k, v);
  }
  for (const c of children) {
    if (c == null) continue;
    e.append(typeof c === "string" ? doc.createTextNode(c) : c);
  }
  return e;
}

function errorBox(doc, err) {
  return el(
    doc,
    "p",
    { class: "error" },
    err.message ?? "failed",
    err.hint ? el(doc, "span", { class: "hint" }, ` ${err.hint}`) : null,
  );
}

function render(doc, out, m) {
  out.replaceChildren();
  out.append(el(doc, "h2", {}, m.title), el(doc, "p", { class: "meta" }, m.subtitle));

  const explain = el(doc, "section", { class: "card", "aria-labelledby": "h-explain" }, el(doc, "h3", { id: "h-explain" }, "What is in it"));
  if (m.explain.error) explain.append(errorBox(doc, m.explain.error));
  else {
    explain.append(el(doc, "p", { class: "summary" }, m.explain.summary));
    for (const p of m.explain.paragraphs) explain.append(el(doc, "p", {}, p));
    if (m.explain.caveats.length) {
      explain.append(el(doc, "ul", { class: "caveats" }, ...m.explain.caveats.map((c) => el(doc, "li", {}, c))));
    }
  }

  const preview = el(doc, "section", { class: "card", "aria-labelledby": "h-preview" }, el(doc, "h3", { id: "h-preview" }, "Preview"));
  if (m.preview.error) preview.append(errorBox(doc, m.preview.error));
  else {
    const url = URL.createObjectURL(new Blob([m.preview.png], { type: "image/png" }));
    preview.append(el(doc, "img", { src: url, alt: `Preview of ${m.title}`, width: String(m.preview.meta.width), height: String(m.preview.meta.height) }));
  }

  const check = el(doc, "section", { class: "card", "aria-labelledby": "h-check" }, el(doc, "h3", { id: "h-check" }, "Check"));
  if (m.check.error) check.append(errorBox(doc, m.check.error));
  else {
    check.append(el(doc, "p", { class: m.check.ok ? "pass" : "fail" }, m.check.label));
    if (m.check.findings.length) check.append(el(doc, "ul", {}, ...m.check.findings.map((f) => el(doc, "li", {}, f))));
  }

  const info = el(doc, "section", { class: "card wide", "aria-labelledby": "h-info" }, el(doc, "h3", { id: "h-info" }, "info --json"));
  if (m.info) info.append(el(doc, "pre", { tabindex: "0" }, m.info));
  else info.append(errorBox(doc, m.infoError));

  out.append(explain, preview, check, info);
}

/** Wire up the drop zone and the file picker. */
export function mount(doc) {
  const zone = doc.getElementById("drop");
  const input = doc.getElementById("pick");
  const out = doc.getElementById("out");
  const status = doc.getElementById("status");
  const show = async (file) => {
    if (!file) return;
    status.textContent = `Reading ${file.name}…`;
    try {
      const r = await analyze(file);
      render(doc, out, model(r));
      status.textContent = "";
    } catch (e) {
      status.textContent = `Could not read ${file.name}: ${e.message}`;
    }
  };
  zone.addEventListener("dragover", (e) => {
    e.preventDefault();
    zone.classList.add("over");
  });
  zone.addEventListener("dragleave", () => zone.classList.remove("over"));
  zone.addEventListener("drop", (e) => {
    e.preventDefault();
    zone.classList.remove("over");
    show(e.dataTransfer?.files?.[0]);
  });
  input.addEventListener("change", () => show(input.files?.[0]));
  ready().then(
    () => (status.textContent = ""),
    (e) => (status.textContent = `The WebAssembly module did not load: ${e.message}`),
  );
}

if (typeof document !== "undefined" && document.getElementById("drop")) mount(document);
