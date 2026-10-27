# openreadout-wasm

OpenReadout compiled to WebAssembly. It reads raw lab-instrument files in a browser or in Node, and nothing is uploaded: the module has no network code and reads only the bytes you hand it. The formats it reads are listed at <https://openreadout.github.io/openreadout/formats.html>.

```js
import { open } from "openreadout-wasm";

const f = await open(fileFromAnInput);      // File/Blob, Uint8Array, ArrayBuffer, or {size, read}
const info = await f.info();                // same JSON as `openreadout info --json`
const words = await f.info({ view: "explain", ask: "Which objective?" });  // `info --view explain`
const report = await f.check();             // integrity report
const { meta, data, type } = await f.preview();   // a picture of image 0 (or a trace, spectrum or plate)
const marked = await f.preview({ axes: "rulers", format: "svg",
  annotations: [{ type: "point", x: 812, y: 440, label: "nucleus" }] });
const rows = await f.table(0, { maxRows: 10 });
const ms2 = await f.scans({ ms_level: 2 });     // scan headers; f.spectrum({ spectrum: 0 }) for one
```

Each method is named after a CLI command and returns the same JSON as that command with `--json`; `info({ view })` takes the views of `info --view` (`summary`, `full`, `structure`, `explain`, `format`). On failure it rejects with an `InstrumentError` that carries the CLI's `code`, `exitCode` and `hint`. The types are in `index.d.ts`.

How bytes are read:

- `Uint8Array` or `ArrayBuffer`: the whole file, in memory.
- `File` or `Blob` up to `wholeFileLimit` (256 MiB by default): read once with `arrayBuffer()`. Larger blobs are read block by block, only where the readers look, so `info` on a multi-gigabyte file reads a few megabytes.
- `{ size, read(offset, length) }`: your own range reader, for example HTTP range requests or a File System Access handle. The module never makes a request itself.
- `openAmong(files, path)`: a dropped folder, so data sets spread over several files (OIF folders, OME-TIFF sets, Bruker `.d` directories) find their siblings, and folders that are data sets themselves (an OME-Zarr `.zarr` directory) open as one. Files are read only where the readers look.
- `openStore(name, get, { path })`: a directory whose files you fetch yourself, such as an OME-Zarr image on S3 or any HTTP server, or with `path`, a file inside it whose companion files are fetched the same way. `get(path, offset, length)` resolves to `{ data, size }` for a file inside the directory, or `null` when there is none. Only the files the readers ask for are fetched, and the directory is never listed:

```js
const base = "https://uk1s3.embassy.ebi.ac.uk/idr/zarr/v0.4/idr0062A/6001240.zarr";
const image = await openStore("6001240.zarr", async (path, offset, length) => {
  const res = await fetch(`${base}/${path}`, { headers: { Range: `bytes=${offset}-${offset + length - 1}` } });
  if (res.status === 404 || res.status === 403) return null;
  const data = new Uint8Array(await res.arrayBuffer());
  // A short answer is the rest of the file; otherwise ask for the size.
  const size = data.length < length ? offset + data.length
    : Number((await fetch(`${base}/${path}`, { method: "HEAD" })).headers.get("Content-Length"));
  return { data, size };
});
```

This build does not export, and has no `watch` or `index`.

Guide: <https://openreadout.github.io/openreadout/guides/wasm.html>. Licensed MIT OR Apache-2.0; the IBM Plex fonts in the module are under the SIL Open Font License 1.1.
