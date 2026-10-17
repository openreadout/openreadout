// The demo's work queue: the WebAssembly module, the open files and every request the page
// makes of them. It runs in a Web Worker (worker.js), so the page stays responsive while a
// picture is drawn or nuclei are counted, or in the page itself when workers are unavailable
// (engine.js). Requests run one at a time, the page's before background ones. A request in a
// `lane` replaces a request in the same lane that hasn't started yet: when you step quickly
// through sweeps, only the last one is drawn.

import { BUILT } from "./built.js";
import * as R from "./reader.js";

/** `post(message, transfer)` sends a message to the page. */
export function createCore(post) {
  const files = new Map(); // key -> { file, info }
  const queue = [];
  let running = false;
  let module = null;

  const loadModule = () => {
    module ??= (async () => {
      const res = await fetch(new URL("openreadout/pkg/openreadout_bg.wasm", import.meta.url));
      if (!res.ok) throw new Error(`the WebAssembly module did not load (HTTP ${res.status})`);
      let body = res.body;
      if (body && typeof TransformStream === "function") {
        let got = 0;
        body = body.pipeThrough(
          new TransformStream({
            transform(chunk, c) {
              got += chunk.byteLength;
              post({ progress: Math.min(1, got / BUILT.wasm.bytes) });
              c.enqueue(chunk);
            },
          }),
        );
      }
      await R.ready(new Response(body, { headers: { "content-type": "application/wasm" } }));
    })();
    module.catch(() => (module = null));
    return module;
  };

  const entry = (key) => {
    const e = files.get(key);
    if (!e) throw new Error("the file is no longer open");
    return e;
  };

  const ops = {
    load: () => loadModule(),
    async open({ key, source, name }) {
      await loadModule();
      const r = await R.openFile(source, name);
      files.get(key)?.file.close();
      files.set(key, { file: r.file, info: r.info.ok ? r.info.value : null });
      return { result: { ...r, file: undefined } };
    },
    close({ key }) {
      files.get(key)?.file.close();
      files.delete(key);
    },
    async preview({ key, options }) {
      const p = await entry(key).file.preview(options);
      const data = p.data.slice();
      return { result: { meta: p.meta, type: p.type, data }, transfer: [data.buffer] };
    },
    async scanNear({ key, run, rt, window }) {
      return { result: await R.scanNear(entry(key).file, run, rt, window) };
    },
    async wellSeries({ key, table }) {
      const e = entry(key);
      return { result: [...(await R.wellSeries(e.file, e.info, table))] };
    },
    async cqBySample({ key }) {
      const e = entry(key);
      return { result: await R.cqBySample(e.file, e.info) };
    },
    async spikeCount({ key, trace, sweep }) {
      const p = await entry(key).file.preview({ trace, sweep, marks: "spikes", max_size: 200 });
      return { result: (p.meta.marks ?? []).filter((m) => m.kind === "spike").length };
    },
  };

  const pump = async () => {
    if (running) return;
    running = true;
    while (queue.length) {
      const job = queue.shift();
      try {
        const out = (await ops[job.op](job.args)) ?? {};
        post({ id: job.id, ok: true, value: out.result }, out.transfer ?? []);
      } catch (e) {
        post({ id: job.id, ok: false, error: { message: e?.message ?? String(e), code: e?.code, hint: e?.hint } });
      }
    }
    running = false;
  };

  return {
    receive({ id, op, args, lane, background }) {
      if (!(op in ops)) {
        post({ id, ok: false, error: { message: `unknown request ${op}` } });
        return;
      }
      if (lane) {
        for (let i = queue.length - 1; i >= 0; i--) {
          if (queue[i].lane === lane) {
            post({ id: queue[i].id, ok: false, superseded: true });
            queue.splice(i, 1);
          }
        }
      }
      const job = { id, op, args, lane, background: Boolean(background) };
      // the page's requests go before background ones, in the order they came
      const at = background ? queue.length : queue.findIndex((j) => j.background);
      queue.splice(at === -1 ? queue.length : at, 0, job);
      pump();
    },
  };
}
