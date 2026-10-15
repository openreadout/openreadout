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

/** A mark drawn on a preview, in its own coordinates (file pixels, axis units or wells). */
export interface Annotation {
  type: "point" | "box" | "polygon" | "line" | "arrow" | "text" | "vline" | "hline" | "vband" | "hband" | "well";
  x?: number;
  y?: number;
  x2?: number;
  y2?: number;
  width?: number;
  height?: number;
  points?: Array<[number, number]>;
  label?: string;
  /** `accent` (default), `ink`, `muted`, `white` or `#RRGGBB`. */
  color?: string;
  channel?: number;
  well?: string;
}

export interface PreviewOptions {
  image?: number;
  /** Plane selection, e.g. `"c=1"` or `["c=0,1", "z=3"]`. */
  select?: string | string[];
  mip?: "z" | "t";
  composite?: boolean;
  level?: number;
  region?: { x: number; y: number; width: number; height: number };
  /** Longest side in pixels (default 768). */
  max_size?: number;
  /** What frames an image (default `none` here: the bare plane). */
  axes?: "rulers" | "grid" | "none";
  /** `auto`, `min-max`, `percentile:LO,HI` or `raw`. */
  contrast?: string;
  lut?: "gray" | "channel-color";
  trace?: number;
  sweep?: number;
  channels?: number[];
  run?: number;
  spectrum?: number;
  scan?: number;
  centroid?: boolean;
  table?: number;
  columns?: string | string[];
  marks?: "auto" | "peaks" | "spikes" | "objects" | "none";
  /** With `marks: "objects"`: how objects are found, as the `objects` option of `openreadout_stats`. */
  objects?: Record<string, any>;
  annotations?: Annotation[];
  dark?: boolean;
  format?: "png" | "jpeg" | "svg";
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
  /** A picture: options as for the `openreadout_preview` MCP tool, plus `dark` and `format`. */
  preview(options?: PreviewOptions): Promise<{
    meta: Record<string, any>;
    data: Uint8Array;
    type: "image/png" | "image/jpeg" | "image/svg+xml";
  }>;
  table(table?: number, options?: { firstRow?: number; maxRows?: number }): Promise<Record<string, any>>;
  trace(
    trace?: number,
    options?: { sweep?: number; firstSample?: number; count?: number; maxSamples?: number },
  ): Promise<Record<string, any>>;
  /** Scan headers of a mass-spectrometry run. */
  scans(options?: {
    run?: number;
    ms_level?: number;
    polarity?: "positive" | "negative";
    rt_range?: [number, number];
    precursor?: number;
    precursor_tol?: number;
    precursor_ppm?: number;
    charge?: number;
    activation?: string;
    scan_filter?: string;
    offset?: number;
    limit?: number;
  }): Promise<Record<string, any>>;
  /** One mass spectrum, by `spectrum` (zero-based index) or `scan` (scan number). */
  spectrum(options: {
    run?: number;
    spectrum?: number;
    scan?: number;
    centroid?: boolean;
    max_points?: number;
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
