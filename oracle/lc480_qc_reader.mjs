// Ground truth for LightCycler 480 `.ixo` files from a reader we did not write: the depositor's
// own offline QC reader, "LightCycler480_Instrument_QC_Verification_English.html" (Zenodo
// 22121623, Mihai Ionita, CC-BY-4.0). The page's script is run unchanged in a Node `vm` context
// with a stand-in DOM, as a black box: we call its `decodeIxo(file)` and record what it returns.
//
// Recorded per file (corpus/oracle/qpcr-roche-qcreader/<id>.json):
// - the instrument name and software version its `meta()` finds, and the run date;
// - the stored crossing point of each PCR well (its `extractCrossingPoints`, which keeps only the
//   40 positive-control wells of the QC plate layout and values between 5 and 40);
// - the acquisition store, as it decodes it: per channel and cycle number, the count and sum of
//   the per-well fluorescence. Its decoder keys readings by `Cycle` and `Channel` only, so the
//   melting programs' readings (all stored as cycle 0) overwrite amplification cycle 1 with the
//   last melting reading. Cycle 1 is left out for that reason; cycles 2 to the last are the
//   amplification program's;
// - the stored melting analyses: per set (program number, channel) the temperature axis
//   (`MeltTemp`), every well's melt curve (`MeltCurve`) and its derivative (`DiffMeltCurve`),
//   as count, first, last and sum, and the full arrays of well 0;
// - its own Tm metrics (computed by the page from the melt curves, not stored in the file).
//
// Usage: node lc480_qc_reader.mjs READER.html ID FILE.ixo [ID FILE.ixo ...]
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.join(HERE, '..', 'corpus', 'oracle', 'qpcr-roche-qcreader');

const [html, ...rest] = process.argv.slice(2);
if (!html || rest.length < 2 || rest.length % 2) {
  console.error('usage: node lc480_qc_reader.mjs READER.html ID FILE.ixo [ID FILE.ixo ...]');
  process.exit(2);
}
const page = fs.readFileSync(html, 'utf8');
const ld = page.match(/<script id="machine-metadata" type="application\/ld\+json">([\s\S]*?)<\/script>/)[1];
const script = page.match(/<script>([\s\S]*?)<\/script>/)[1];

// A stand-in DOM: every element is a stub that accepts any property and method call.
function stub() {
  const f = function () { return stub(); };
  return new Proxy(f, {
    get(t, k) {
      if (k === Symbol.toPrimitive) return () => '';
      if (k === 'textContent' || k === 'value' || k === 'innerHTML') return t[k] ?? '';
      if (k === 'getAttribute') return () => null;
      if (k === 'querySelectorAll') return () => [];
      if (k === 'getContext') return () => stub();
      if (k in t) return t[k];
      return stub();
    },
    set(t, k, v) { t[k] = v; return true; },
    apply() { return stub(); },
  });
}
const elements = {};
const document = {
  documentElement: stub(),
  getElementById(id) {
    if (id === 'machine-metadata') return { textContent: ld };
    if (id === 'profileMode') return { value: 'auto', onchange: null };
    return (elements[id] ??= stub());
  },
  querySelectorAll() { return []; },
  createElement() { return stub(); },
  addEventListener() {},
  body: stub(),
};
const ctx = {
  document, window: {}, navigator: { language: 'en' }, console,
  atob, btoa, TextDecoder, TextEncoder, DecompressionStream, Response, Blob,
  Uint8Array, Float32Array, Float64Array, DataView, ArrayBuffer, Math, JSON, Date, Promise,
  crypto: globalThis.crypto, setTimeout, clearTimeout, requestAnimationFrame: () => 0,
  localStorage: { getItem: () => null, setItem() {} }, URL, Set, Map, Object, Array, Number, String, RegExp, Error,
  isFinite, isNaN, parseInt, parseFloat,
};
ctx.window = ctx;
vm.createContext(ctx);
// `const`/`let` declarations stay in the script's scope, so expose what we call.
vm.runInContext(script + '\n;globalThis.__api={decodeIxo};', ctx, { filename: path.basename(html) });
const { decodeIxo } = ctx.__api;

const summary = (a) => {
  const v = Array.from(a ?? []);
  return { n: v.length, first: v[0] ?? null, last: v.at(-1) ?? null, sum: v.reduce((s, x) => s + x, 0) };
};

fs.mkdirSync(OUT, { recursive: true });
for (let i = 0; i < rest.length; i += 2) {
  const [id, file] = [rest[i], rest[i + 1]];
  const buf = fs.readFileSync(file);
  const fileLike = {
    name: path.basename(file),
    arrayBuffer: async () => buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength),
  };
  const r = await decodeIxo(fileLike);
  const amplification = {};
  for (const ch of Object.keys(r.ampWell)) {
    const wells = r.ampWell[ch];
    const cycles = wells.__cycles;
    amplification[ch] = cycles
      .map((c, k) => {
        const vals = Object.keys(wells).filter((w) => w !== '__cycles').map((w) => wells[w][k]);
        return { cycle: c, n: vals.length, sum: vals.reduce((s, x) => s + x, 0), well0: vals[0] };
      })
      .filter((r) => r.cycle > 1);
  }
  const melt = r.meltMeta.map((m) => ({
    set: m.set,
    program: m.progNo,
    channel: m.channel,
    temperatures: summary(r.temps[m.set]),
    curves: (r.curves[m.set] ?? []).map(summary),
    derivatives: (r.diffs[m.set] ?? []).map(summary),
    well0: {
      temperatures: Array.from(r.temps[m.set] ?? []),
      curve: Array.from((r.curves[m.set] ?? [])[0] ?? []),
      derivative: Array.from((r.diffs[m.set] ?? [])[0] ?? []),
    },
  }));
  const out = {
    id,
    reader: 'LightCycler480_Instrument_QC_Verification_English.html (Zenodo 22121623, CC-BY-4.0), decodeIxo, run in Node ' + process.version,
    file: fileLike.name,
    sha256: r.sha,
    instrument: r.md.instrName,
    software: r.md.sw,
    date: r.date ? r.date.toISOString().slice(0, 10) : null,
    crossing_points: Object.fromEntries(Object.entries(r.cp).map(([k, v]) => [k, v])),
    amplification,
    melt,
    metrics: r.metrics,
  };
  fs.writeFileSync(path.join(OUT, `${id}.json`), JSON.stringify(out, null, 1) + '\n');
  console.log(id, Object.keys(r.cp).length, 'crossing points;', melt.length, 'melt sets;', Object.keys(amplification).join(','));
}
