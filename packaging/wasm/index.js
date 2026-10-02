// OpenReadout in WebAssembly: read instrument files in a browser or in Node, locally.
//
//   import { open } from "openreadout-wasm";
//   const f = await open(file);            // a File/Blob, Uint8Array, ArrayBuffer or {size, read}
//   const info = await f.info();           // the `data` of `openreadout info --json`
//   const { png } = await f.preview();     // PNG bytes
//
// Nothing is uploaded or fetched: the module has no network code. Bytes come from the object
// you pass. Blobs up to `wholeFileLimit` (default 256 MiB) are read at once; larger ones are
// read in blocks, only where the readers look.

import init, * as wasm from "./pkg/openreadout.js";

let ready = null;

const isNode =
  typeof process !== "undefined" && process.versions != null && process.versions.node != null;

/**
 * Load the WebAssembly module (once). In a browser the module is fetched next to this file
 * unless `module` is given (bytes, a URL, a Response or a compiled WebAssembly.Module).
 */
export function load(module) {
  if (ready === null) {
    ready = (async () => {
      let m = module;
      if (m === undefined && isNode) {
        const { readFile } = await import("node:fs/promises");
        m = await readFile(new URL("./pkg/openreadout_bg.wasm", import.meta.url));
      }
      await init(m === undefined ? undefined : { module_or_path: m });
    })();
  }
  return ready;
}

/** An operation failed; `code`, `exitCode` and `hint` are those of the CLI's JSON envelope. */
export class InstrumentError extends Error {
  constructor(body) {
    super(body.message);
    this.name = "InstrumentError";
    this.code = body.code;
    this.exitCode = body.exit_code;
    this.hint = body.hint;
  }
}

function unwrap(json) {
  const env = JSON.parse(json);
  if (env.ok) return env.data;
  throw new InstrumentError(env.error);
}

/** The formats this build reads (`openreadout self formats --json`). */
export async function formats() {
  await load();
  return unwrap(wasm.formats()).formats;
}

/** The library version. */
export async function version() {
  await load();
  return wasm.version();
}

function toBytes(x) {
  if (x instanceof Uint8Array) return x;
  if (x instanceof ArrayBuffer) return new Uint8Array(x);
  if (ArrayBuffer.isView(x)) return new Uint8Array(x.buffer, x.byteOffset, x.byteLength);
  throw new TypeError("expected a Uint8Array or ArrayBuffer");
}

const isBlob = (x) => typeof Blob !== "undefined" && x instanceof Blob;

/**
 * An opened file. Every method resolves to the `data` of the CLI command of the same name
 * (`--json`) or rejects with an {@link InstrumentError}.
 */
export class InstrumentFile {
  constructor(inner, fetchRange, options = {}) {
    this._inner = inner;
    this._fetch = fetchRange; // (offset, length) => Promise<Uint8Array>, for lazy files
    this._maxSpan = options.maxSpan ?? 32 * 1024 * 1024;
  }

  /** The name the file was given. */
  get name() {
    return this._inner.name;
  }

  async _run(op) {
    let span = 0;
    for (;;) {
      const out = op(this._inner);
      const pending = this._fetch ? this._inner.pending() : undefined;
      if (!pending) return out;
      // Read ahead: each further miss in the same operation doubles the span.
      const [at, len] = pending;
      span = Math.min(Math.max(len, span * 2), this._maxSpan);
      const size = this._size;
      const end = Math.min(at + Math.max(span, len), size);
      const bytes = await this._fetch(at, end - at);
      this._inner.provide(at, bytes);
    }
  }

  /**
   * `info --view VIEW`: `summary` (default), `full` (plus the vendor tree, unless
   * `vendor: false`, and field provenance, unless `provenance: false`), `structure`, `explain`
   * (optionally answering `ask`: "which channel is DAPI?") or `format`.
   */
  info({ view = "summary", ...options } = {}) {
    const json = JSON.stringify(options);
    return this._run((f) => f.info(view, json)).then(unwrap);
  }
  check({ headersOnly = false } = {}) {
    return this._run((f) => f.check(headersOnly)).then(unwrap);
  }
  /**
   * `preview`: resolves to `{ meta, png }` where `png` is a Uint8Array. Options: `image`,
   * `select` (e.g. `["c=1","z=3"]`), `composite`, `max_size`, `trace`, `sweep`, `run`,
   * `spectrum`, `table`.
   */
  async preview(options = {}) {
    const json = JSON.stringify(options);
    const meta = unwrap(await this._run((f) => f.preview(json)));
    return { meta, png: this._inner.image() };
  }
  /** Rows of table `table` (FCS events, plate-reader values). */
  table(table = 0, { firstRow = 0, maxRows = 100 } = {}) {
    return this._run((f) => f.table(table, firstRow, maxRows)).then(unwrap);
  }
  /** A window of one sweep of trace `trace`: statistics and the first `maxSamples` values. */
  trace(trace = 0, { sweep = 0, firstSample = 0, count = -1, maxSamples = 1000 } = {}) {
    return this._run((f) => f.trace(trace, sweep, firstSample, count, maxSamples)).then(unwrap);
  }
  /**
   * `spectra`: the scan headers of run `run` (filters `ms_level`, `polarity`, `rt_range`,
   * `precursor_mz`, `ppm`, `charge`, `activation`, `scan_filter`; paging `offset`, `limit`), or
   * with `index` (zero-based) or `scan` (the instrument's scan number) one spectrum
   * (`centroid`, `max_points`).
   */
  spectra(options = {}) {
    const json = JSON.stringify(options);
    return this._run((f) => f.spectra(json)).then(unwrap);
  }
  /** Bytes of a lazily read file held in memory so far. */
  get bytesHeld() {
    return this._inner.bytesHeld();
  }
  /** Release the WebAssembly memory held for this file. */
  close() {
    this._inner.free();
  }
}

/**
 * Open a file. `source` is a `File`/`Blob`, a `Uint8Array`/`ArrayBuffer` (the whole file), or
 * an object `{ size, read(offset, length) }` whose `read` returns (a promise of) a Uint8Array
 * (an HTTP range reader, an S3 client: implemented by the page, never by this module).
 * Options: `name` (defaults to `File.name`; the extension helps detection), `wholeFileLimit`.
 */
export async function open(source, options = {}) {
  await load(options.module);
  const name = options.name ?? source?.name ?? "memory";
  const limit = options.wholeFileLimit ?? 256 * 1024 * 1024;
  if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
    return new InstrumentFile(wasm.InstrumentFile.fromBytes(name, toBytes(source)));
  }
  let size;
  let fetchRange;
  if (isBlob(source)) {
    size = source.size;
    if (size <= limit) {
      const bytes = new Uint8Array(await source.arrayBuffer());
      return new InstrumentFile(wasm.InstrumentFile.fromBytes(name, bytes));
    }
    fetchRange = async (at, len) => new Uint8Array(await source.slice(at, at + len).arrayBuffer());
  } else if (source && typeof source.read === "function" && Number.isFinite(source.size)) {
    size = source.size;
    fetchRange = async (at, len) => toBytes(await source.read(at, len));
  } else {
    throw new TypeError("open() takes a File/Blob, bytes, or { size, read(offset, length) }");
  }
  const f = new InstrumentFile(wasm.InstrumentFile.lazy(name, size), fetchRange, options);
  f._size = size;
  return f;
}

/**
 * Open a file among several (a dropped folder): `files` is an array of `File`s (with
 * `webkitRelativePath`) or `{ path, data }` pairs; `path` names the one to open. Every file is
 * read into memory.
 */
export async function openAmong(files, path, options = {}) {
  await load(options.module);
  const set = new wasm.Files();
  for (const f of files) {
    const p = f.path ?? (f.webkitRelativePath || f.name);
    const data = f.data !== undefined ? toBytes(f.data) : new Uint8Array(await f.arrayBuffer());
    set.add(p, data);
  }
  return new InstrumentFile(set.open(path));
}
