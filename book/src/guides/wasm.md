# WebAssembly

OpenReadout's readers also compile to WebAssembly, so a web page or a Node program can read instrument files without a server. The [demo page](/openreadout/demo/) is built this way.

## The npm package

The package `openreadout-wasm` (in [`packaging/wasm`](https://github.com/openreadout/openreadout/tree/main/packaging/wasm); not yet on npm) wraps the module in promises:

```js
import { open } from "openreadout-wasm";

const f = await open(file);              // a File or Blob, bytes, or { size, read(offset, length) }
const info = await f.info();             // the `data` of `info --json`
const report = await f.check();          // the `data` of `check --json`
const { meta, data, type } = await f.preview(); // the picture, its MIME type and what was drawn
f.close();
```

`preview` takes the options of the [`openreadout_preview`](../reference/mcp.md) tool, plus `dark`, `format` (`png`, `jpeg` or `svg`) and `scale`. `scale` draws a PNG or JPEG with that many pixels per figure pixel (1 to 4), so a page can lay a picture out at `max_size` CSS pixels and keep it sharp on a phone: pass `devicePixelRatio`. Images come without rulers unless you ask for `axes: "rulers"`. Annotations are drawn in the picture's own coordinates, and `meta.annotations` says where each one landed:

```js
const { data, type, meta } = await f.preview({
  axes: "rulers",
  format: "svg",
  annotations: [{ type: "point", x: 812, y: 440, label: "nucleus" }],
});
img.src = URL.createObjectURL(new Blob([data], { type }));
```

`open` takes a `File` or `Blob`, a byte array, or a range reader: an object with a `size` and a `read(offset, length)` method that the page implements, for example with HTTP range requests. Blobs up to 256 MiB are read whole. For larger ones the readers fetch only the blocks they need, so `info` on a multi-gigabyte file reads only a few megabytes.

Data sets that span several files or a directory (Bruker `.d`, ChemStation `.D`, Waters `.raw`, NMR experiment directories, multi-file OME-TIFF, OME-Zarr) need their sibling files. Open them with `openAmong(files, path)`, where `files` is a dropped folder and `path` is the file to open, or the folder itself when the data set is a directory. Files are read only where the readers look:

```js
import { openAmong } from "openreadout-wasm";
const f = await openAmong(droppedFiles, "run.D");
```

Files and folders on a web server, such as an OME-Zarr image on S3 or a file on Zenodo, open with `openStore(name, get, { path })`. You fetch the files yourself: `get(path, offset, length)` is given a path inside the directory and resolves to `{ data, size }`, up to `length` bytes of that file from `offset` and the file's size, or to `null` when there is no such file. Without `path`, the directory itself is opened (an OME-Zarr image). With it, the file at `path` is opened, and any companion files it needs are asked for through `get` too. The readers ask only for the files and blocks they need, and the directory is never listed. The [package README](https://github.com/openreadout/openreadout/tree/main/packaging/wasm#readme) has an example with `fetch` and HTTP range requests, and the demo page's [`web/store.js`](https://github.com/openreadout/openreadout/blob/main/web/store.js) is a complete one.

Each method is named after a command-line command and returns the same JSON:

| method | command |
| --- | --- |
| `formats()`, `version()` | `self formats`, `--version` |
| `info()` | `info` |
| `info({ view: "full", vendor, provenance })` | `info --view full` |
| `info({ view: "structure" })` | `info --view structure` |
| `info({ view: "explain", ask })` | `info --view explain`, `--ask` |
| `info({ view: "format" })` | `info --view format` |
| `check({ headersOnly })` | `check`, `--headers-only` |
| `preview(options)` | `preview` |
| `table(i, { firstRow, maxRows })` | `table` |
| `trace(i, { sweep, firstSample, count, maxSamples })` | `trace` |
| `scans({ ms_level, rt_range, limit, ... })` | `scans` |
| `spectrum({ spectrum, scan, centroid, max_points })` | `spectrum` |

Errors are thrown as `InstrumentError`, with `code`, `exitCode` and `hint` fields that match the command line's [JSON errors](../getting-started/reading-json.md#the-json-wrapper).

## What the WebAssembly build leaves out

- writing files: `export` and the other writers
- `plot`, `stats` and the analyses, which read files by path
- `watch`, `index` and the MCP server
- multi-threaded decoding (it all runs on the calling thread)
- the check for files still being written, because there is no modification time

A bug that makes Rust panic stops the WebAssembly instance. The readers are fuzzed so that malformed files give errors instead, but a page that must survive a panic should load the module again.

## Privacy of the demo page

The demo page in [`web/`](https://github.com/openreadout/openreadout/tree/main/web) can't upload your file. Its Content-Security-Policy is:

```text
default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; font-src 'self'; img-src 'self' blob:; connect-src 'self' https:; worker-src blob:; form-action 'none'; base-uri 'none'
```

`connect-src 'self' https:` lets the page fetch its own files from the site (the WebAssembly module and the sample files) and, when you open a URL, the file or folder it points to. Only [`web/store.js`](https://github.com/openreadout/openreadout/blob/main/web/store.js) fetches from another site, it does so only after you open a URL, and it only reads: GET and HEAD requests with a byte range and no body. The page's scripts never send a file anywhere; a file you open is read into the tab's memory, and `form-action 'none'` forbids posting a form. OpenReadout runs in a Web Worker so the page stays responsive while it draws. The page starts the worker from a `blob:` URL (`worker-src blob:`), and a worker started that way keeps the page's policy. The sample files come from the public test corpus and are credited on the page and in `web/samples/NOTICE.txt`.

## Reading from other sources in Rust

The readers do not read from a path directly. They read from a byte source: a local file, a buffer in memory, or a function the host provides that reads byte ranges. That way the same readers can run in a browser, open Python `bytes` and file objects, and read from S3 or an HTTP server through code the host writes. OpenReadout itself doesn't open network connections.

```rust
use std::sync::Arc;
use openreadout_core::source::{CachedSource, CallbackSource, Input};

// A range reader written by the host (HTTP range requests, S3 GetObject with Range, ...).
let remote = CallbackSource::new("plate1.czi", size, move |offset, buf| my_http_range(offset, buf));
let input = Input::from_source("plate1.czi", Arc::new(CachedSource::with_defaults(Arc::new(remote))));
let (detection, dataset) = registry.open_input(&input)?;
let info = dataset.info()?;
```

A buffer is `Input::from_bytes(name, bytes)`, and a set of files held in memory is a `MemFs`. [`docs/architecture.md`](https://github.com/openreadout/openreadout/blob/main/docs/architecture.md) describes the design.

## Building the module

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version <the wasm-bindgen version in Cargo.lock>
scripts/wasm.sh build        # writes packaging/wasm/pkg/
scripts/wasm.sh test         # tests in Node
scripts/wasm.sh demo         # the demo page, after `npm run build` in book/
```

`scripts/wasm.sh build` runs `wasm-opt` to shrink the module when binaryen is installed (`cd packaging/wasm && npm install` provides it). The module is about 16 MB, 7 MB gzipped. The figure engine that draws previews, with its embedded fonts, is about 1 MB of that.

On macOS, some Rust toolchains ship a `rust-lld` that fails with `Library not loaded: @rpath/libLLVM.dylib`. A small linker wrapper fixes this without changing the toolchain:

```bash
wasm_sysroot="$(rustc --print sysroot)"
wasm_host="$(rustc -vV | sed -n 's/^host: //p')"
cat > /tmp/openreadout-wasm-linker <<EOF
#!/bin/sh
export DYLD_LIBRARY_PATH="$wasm_sysroot/lib"
exec "$wasm_sysroot/lib/rustlib/$wasm_host/bin/rust-lld" "\$@"
EOF
chmod +x /tmp/openreadout-wasm-linker
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=/tmp/openreadout-wasm-linker scripts/wasm.sh build
```
