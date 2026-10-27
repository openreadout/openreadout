// OpenReadout in WebAssembly: read instrument files in a browser or in Node, locally.
//
//   import { open } from "openreadout-wasm";
//   const f = await open(file);            // a File/Blob, Uint8Array, ArrayBuffer or {size, read}
//   const info = await f.info();           // the `data` of `openreadout info --json`
//   const { data, type } = await f.preview();  // the picture's bytes and MIME type
//
// Nothing is uploaded or fetched: the module has no network code. Bytes come from the object
// you pass. Blobs up to `wholeFileLimit` (default 256 MiB) are read at once; larger ones, the
// files of a dropped folder and the files of a store you fetch yourself (`openStore`) are read
// in blocks, only where the readers look.

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
  constructor(inner, host, options = {}) {
    this._inner = inner;
    // For lazily read files: `read(path, offset, length)` resolves to bytes, and for stores,
    // `lookup(path)` answers whether a file exists (see `_run`).
    this._host = host;
    this._maxSpan = options.maxSpan ?? 32 * 1024 * 1024;
  }

  /** The name the file was given. */
  get name() {
    return this._inner.name;
  }

  async _run(op) {
    let span = 0;
    let last = null;
    for (;;) {
      const out = op(this._inner);
      const needs = this._host ? this._inner.pending() : undefined;
      if (!needs) return out;
      // A pass that only checked for files asks for all of them at once: look them up together.
      await Promise.all(needs.filter((n) => n.lookup).map((n) => this._host.lookup(this._inner, n.path)));
      for (const need of needs.filter((n) => !n.lookup)) {
        // Read ahead: each further miss in the same file doubles the span.
        if (need.path !== last) span = 0;
        last = need.path;
        span = Math.min(Math.max(need.length, span * 2), this._maxSpan);
        const end = Math.min(need.offset + Math.max(span, need.length), need.size);
        const bytes = await this._host.read(need.path, need.offset, end - need.offset);
        this._inner.provide(need.path, need.offset, bytes);
      }
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
   * `preview`: resolves to `{ meta, data, type }`: the `preview --json` data, the picture as a
   * Uint8Array and its MIME type. Options are those of the `openreadout_preview` MCP tool
   * (`image`, `select`, `region`, `max_size`, `axes`, `trace`, `sweep`, `columns`, `marks`,
   * `annotations`, ...), plus `dark`, `format` (`png`, `jpeg` or `svg`) and `scale` (1 to 4
   * pixels per figure pixel, for high-density screens).
   */
  async preview(options = {}) {
    const json = JSON.stringify(options);
    const meta = unwrap(await this._run((f) => f.preview(json)));
    const type = { png: "image/png", jpeg: "image/jpeg", svg: "image/svg+xml" }[meta.encoding];
    return { meta, data: this._inner.image(), type };
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
   * `scans`: the scan headers of run `run` (filters `ms_level`, `polarity`, `rt_range`,
   * `precursor`, `precursor_tol`, `precursor_ppm`, `charge`, `activation`, `scan_filter`;
   * paging `offset`, `limit`).
   */
  scans(options = {}) {
    const json = JSON.stringify(options);
    return this._run((f) => f.scans(json)).then(unwrap);
  }
  /**
   * `spectrum`: one spectrum of run `run`, by `spectrum` (zero-based) or `scan` (the
   * instrument's scan number), with `centroid` and `max_points`.
   */
  spectrum(options = {}) {
    const json = JSON.stringify(options);
    return this._run((f) => f.spectrum(json)).then(unwrap);
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
  const host = { read: (_path, at, len) => fetchRange(at, len) };
  return new InstrumentFile(wasm.InstrumentFile.lazy(name, size), host, options);
}

/**
 * Open a file among several (a dropped folder): `files` is an array of `File`s (with
 * `webkitRelativePath`) or `{ path, data }` pairs (`data` a Blob or bytes); `path` names the
 * one to open, or the folder
 * itself for a data set that is a directory (an OME-Zarr image, a Bruker `.d`). Files are read
 * only where the readers look.
 */
export async function openAmong(files, path, options = {}) {
  await load(options.module);
  const byPath = new Map();
  for (const f of files) {
    const p = f.path ?? (f.webkitRelativePath || f.name);
    const data = f.data ?? f;
    byPath.set(p, isBlob(data) ? data : new Blob([toBytes(data)]));
  }
  const paths = [...byPath.keys()];
  const sizes = paths.map((p) => byPath.get(p).size);
  const host = {
    read: async (p, at, len) => new Uint8Array(await byPath.get(p).slice(at, at + len).arrayBuffer()),
  };
  return new InstrumentFile(wasm.InstrumentFile.lazyFiles(paths, sizes, path), host, options);
}

/**
 * Open a directory whose files you fetch yourself, such as a folder on a web server: the
 * directory itself (an OME-Zarr image), or with `options.path`, a file inside it whose
 * companion files are fetched the same way. `name` is the directory's name (`image.zarr`).
 * `get(path, offset, length)` is given a path inside the directory (`0/.zarray`) and resolves
 * to `{ data, size }`: up to `length` bytes of that file from `offset` (fewer at its end) and
 * the file's size, or to `null` when there is no such file. Files are looked up and read only
 * as the readers ask for them, and the directory is never listed.
 */
export async function openStore(name, get, options = {}) {
  await load(options.module);
  const inside = (p) => p.slice(name.length + 1);
  // A lookup asks for the start of the file too: most Zarr chunks are smaller than this, so
  // one request brings the whole chunk.
  const first = 4 * wasm.InstrumentFile.blockSize();
  const host = {
    lookup: async (inner, p) => {
      const r = await get(inside(p), 0, first);
      if (!r) {
        inner.absent(p);
        return;
      }
      inner.found(p, r.size);
      const data = toBytes(r.data);
      if (data.length > 0) inner.provide(p, 0, data);
    },
    read: async (p, at, len) => {
      const r = await get(inside(p), at, len);
      if (!r) throw new Error(`${p} is no longer there`);
      return toBytes(r.data);
    },
  };
  const path = options.path === undefined ? undefined : `${name}/${options.path}`;
  return new InstrumentFile(wasm.InstrumentFile.lazyStore(name, path), host, options);
}
