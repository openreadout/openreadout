// Types of the OpenReadout WebAssembly wrapper (index.js). The JSON shapes are those of the
// CLI's `--json` output: docs/schema/*.json in the repository.

/** Load the WebAssembly module once (fetched next to index.js unless `module` is given). */
export function load(
  module?: BufferSource | WebAssembly.Module | URL | string | Response,
): Promise<void>;

/** A failed operation, with the CLI envelope's error fields. */
export class InstrumentError extends Error {
  code: string;
  exitCode: number;
  hint?: string;
}

/** Formats this build reads (`self formats --json` → `formats`). */
export function formats(): Promise<Array<Record<string, unknown>>>;

/** Library version. */
export function version(): Promise<string>;

/** A byte range reader implemented by the page (HTTP range requests, S3, ...). */
export interface RangeReader {
  size: number;
  read(offset: number, length: number): Uint8Array | ArrayBuffer | Promise<Uint8Array | ArrayBuffer>;
  name?: string;
}

export interface OpenOptions {
  /** File name (the extension helps detection); defaults to `File.name`. */
  name?: string;
  /** Blobs up to this size are read whole (default 256 MiB); larger ones block by block. */
  wholeFileLimit?: number;
  /** Largest read-ahead span of a lazily read file (default 32 MiB). */
  maxSpan?: number;
  /** The WebAssembly module, when it is not next to index.js. */
  module?: BufferSource | WebAssembly.Module | URL | string | Response;
}

export class InstrumentFile {
  readonly name: string;
  readonly bytesHeld: number;
  /** `info --view VIEW` (default `summary`); `vendor`/`provenance` for `full`, `ask` for `explain`. */
  info(options?: {
    view?: "summary" | "full" | "structure" | "explain" | "format";
    vendor?: boolean;
    provenance?: boolean;
    ask?: string;
  }): Promise<Record<string, any>>;
  check(options?: { headersOnly?: boolean }): Promise<Record<string, any>>;
  preview(options?: {
    image?: number;
    select?: string[];
    composite?: boolean;
    max_size?: number;
    trace?: number;
    sweep?: number;
    run?: number;
    spectrum?: number;
    table?: number;
  }): Promise<{ meta: Record<string, any>; png: Uint8Array }>;
  table(table?: number, options?: { firstRow?: number; maxRows?: number }): Promise<Record<string, any>>;
  trace(
    trace?: number,
    options?: { sweep?: number; firstSample?: number; count?: number; maxSamples?: number },
  ): Promise<Record<string, any>>;
  /** Scan headers of a run, or one spectrum with `index` or `scan`. */
  spectra(options?: {
    run?: number;
    index?: number;
    scan?: number;
    centroid?: boolean;
    max_points?: number;
    ms_level?: number;
    polarity?: "positive" | "negative";
    rt_range?: [number, number];
    precursor_mz?: number;
    ppm?: number;
    charge?: number;
    activation?: string;
    scan_filter?: string;
    offset?: number;
    limit?: number;
  }): Promise<Record<string, any>>;
  close(): void;
}

/** Open a File/Blob, bytes, or a range reader. */
export function open(
  source: Blob | Uint8Array | ArrayBuffer | ArrayBufferView | RangeReader,
  options?: OpenOptions,
): Promise<InstrumentFile>;

/** Open `path` among several files (a dropped folder). */
export function openAmong(
  files: Array<File | { path: string; data: Uint8Array | ArrayBuffer }>,
  path: string,
  options?: OpenOptions,
): Promise<InstrumentFile>;
