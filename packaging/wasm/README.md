# openreadout-wasm

OpenReadout compiled to WebAssembly. It reads raw lab-instrument files in a browser or in Node, and nothing is uploaded: the module has no network code and reads only the bytes you hand it. The formats it reads are listed at <https://openreadout.github.io/openreadout/formats.html>.

```js
import { open } from "openreadout-wasm";

const f = await open(fileFromAnInput);      // File/Blob, Uint8Array, ArrayBuffer, or {size, read}
const info = await f.info();                // same JSON as `openreadout info --json`
const words = await f.info({ view: "explain", ask: "Which objective?" });  // `info --view explain`
const report = await f.check();             // integrity report
const { meta, png } = await f.preview();    // PNG bytes of image 0 (or a trace, spectrum or plate)
const rows = await f.table(0, { maxRows: 10 });
const ms2 = await f.spectra({ ms_level: 2 });   // scan headers; { index: 0 } for one spectrum
```

Each method is named after a CLI command and returns the same JSON as that command with `--json`; `info({ view })` takes the views of `info --view` (`summary`, `full`, `structure`, `explain`, `format`). On failure it rejects with an `InstrumentError` that carries the CLI's `code`, `exitCode` and `hint`. The types are in `index.d.ts`.

How bytes are read:

- `Uint8Array` or `ArrayBuffer`: the whole file, in memory.
- `File` or `Blob` up to `wholeFileLimit` (256 MiB by default): read once with `arrayBuffer()`. Larger blobs are read block by block, only where the readers look, so `info` on a multi-gigabyte file reads a few megabytes.
- `{ size, read(offset, length) }`: your own range reader, for example HTTP range requests or a File System Access handle. The module never makes a request itself.
- `openAmong(files, path)`: a dropped folder, so data sets spread over several files (OIF folders, OME-TIFF sets, Bruker `.d` directories) find their siblings.

This build does not read OME-Zarr, does not export, and has no `watch` or `index`.

Guide: <https://openreadout.github.io/openreadout/guides/wasm.html>. Licensed MIT OR Apache-2.0.
