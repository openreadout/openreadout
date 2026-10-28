// The page's side of core.js: `call(op, args, { lane, background, transfer })` returns a
// promise of the result. The worker is started from a blob: URL so that it inherits this
// page's Content-Security-Policy, as a worker loaded from its own URL would not. Where module
// workers are unavailable, core.js runs in the page instead.

import { fetchOnce } from "./store.js";

const pending = new Map();
let nextId = 1;
let send = null;
let onProgress = () => {};
let onDownloading = () => {};

function receive(m) {
  if (m.progress !== undefined) return onProgress(m.progress);
  if (m.downloading !== undefined) return onDownloading(m.downloading);
  const p = pending.get(m.id);
  if (!p) return;
  pending.delete(m.id);
  if (m.ok) p.resolve(m.value);
  else p.reject(m.superseded ? Object.assign(new Error("superseded"), { superseded: true }) : Object.assign(new Error(m.error.message), m.error));
}

async function inPage() {
  const { createCore } = await import("./core.js");
  const core = createCore((m) => queueMicrotask(() => receive(m)));
  return (m) => core.receive(m);
}

function inWorker() {
  return new Promise((resolve, reject) => {
    let w;
    try {
      // a dynamic import: Chrome checks a module worker's static imports against worker-src
      const url = JSON.stringify(new URL("worker.js", import.meta.url).href);
      const src = `import(${url}).catch((e) => postMessage({ failed: String(e) }));`;
      w = new Worker(URL.createObjectURL(new Blob([src], { type: "text/javascript" })), { type: "module" });
    } catch (e) {
      reject(e);
      return;
    }
    const timer = setTimeout(() => reject(new Error("the worker did not start")), 8000);
    w.addEventListener("error", (e) => {
      clearTimeout(timer);
      reject(e);
    });
    w.addEventListener("message", function first(e) {
      if (e.data?.failed) {
        clearTimeout(timer);
        reject(new Error(e.data.failed));
        return;
      }
      if (!e.data?.hello) return;
      clearTimeout(timer);
      w.removeEventListener("message", first);
      w.addEventListener("message", (ev) => (ev.data.fetch ? fetchFor(w, ev.data.fetch) : receive(ev.data)));
      resolve((m, transfer) => w.postMessage(m, transfer));
    });
  });
}

/** Make a request for the worker (worker.js), and send it what store.js reads of the answer. */
async function fetchFor(w, { id, at, options, body }) {
  try {
    const res = await fetchOnce(at, options, body);
    const headers = {};
    for (const h of ["content-type", "content-length", "content-range"]) if (res.headers.has(h)) headers[h] = res.headers.get(h);
    const response = { ok: res.ok, status: res.status, url: res.url, headers, data: res.data };
    w.postMessage({ fetched: { id, response } }, res.data ? [res.data.buffer] : []);
  } catch (e) {
    w.postMessage({ fetched: { id, error: { name: e?.name, message: e?.message ?? String(e) } } });
  }
}

const started = inWorker().catch(() => inPage());

/** Run `op` in the worker. */
export async function call(op, args = {}, { lane, background, transfer = [] } = {}) {
  send ??= await started;
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    send({ id, op, args, lane, background }, transfer);
  });
}

/** Report the module's download progress (0 to 1) to `f`. */
export function progress(f) {
  onProgress = f;
}

/** Tell `f(on)` when the worker starts and stops downloading parts of a file from an address. */
export function downloading(f) {
  onDownloading = f;
}
