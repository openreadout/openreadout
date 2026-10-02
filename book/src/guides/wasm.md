# WebAssembly

OpenReadout's readers also compile to WebAssembly, so a web page or a Node program can read instrument files without a server. The [demo page](../getting-started/browser.md) is built this way.

## The npm package

The package `openreadout-wasm` (in [`packaging/wasm`](https://github.com/openreadout/openreadout/tree/main/packaging/wasm); not yet on npm) wraps the module in promises:

```js
import { open } from "openreadout-wasm";

const f = await open(file);              // a File or Blob, bytes, or { size, read(offset, length) }
const info = await f.info();             // the `data` of `info --json`
const report = await f.check();          // the `data` of `check --json`
const { meta, png } = await f.preview(); // a preview image and what was drawn
f.close();
```

`open` takes a `File` or `Blob`, a byte array, or a range reader: an object with a `size` and a `read(offset, length)` method that the page implements, for example with HTTP range requests. Blobs up to 256 MiB are read whole. Larger ones are read in blocks, only where the readers look, so `info` on a multi-gigabyte file reads only a few megabytes.

Data sets that span several files or a directory (Bruker `.d`, ChemStation `.D`, Waters `.raw`, NMR experiment directories, multi-file OME-TIFF) need their sibling files. Open them with `openAmong(files, path)`, where `files` is a dropped folder:

```js
import { openAmong } from "openreadout-wasm";
const f = await openAmong(droppedFiles, "run.D");
```

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
| `spectra({ ms_level, rt_range, limit, ... })` | `spectra` (scan headers) |
| `spectra({ index, scan, centroid, max_points })` | `spectra --index`, `--scan` |

Errors are thrown as `InstrumentError`, with `code`, `exitCode` and `hint` fields that match the command line's [JSON errors](../getting-started/reading-json.md#the-json-wrapper).

## What the WebAssembly build leaves out

- the OME-Zarr reader
- writing files: `export` and the other writers
- `watch`, `index` and the MCP server
- multi-threaded decoding (everything runs on the calling thread)
- the check for files still being written, because there is no modification time

A bug that makes Rust panic stops the WebAssembly instance. The readers are fuzzed so that malformed files give errors instead, but a page that must survive a panic should load the module again.

## Privacy of the demo page

The demo page in [`web/`](https://github.com/openreadout/openreadout/tree/main/web) cannot upload anything. Its Content-Security-Policy is:

```text
default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob:; connect-src 'none'; form-action 'none'; base-uri 'none'
```

so the browser refuses any request the page makes after loading its own files. Because the policy forbids fetching even the WebAssembly module, the build inlines the module into the page.

## Reading from other sources in Rust

The readers do not read from a path directly. They read from a byte source: a local file, a buffer in memory, or a function the host provides that reads byte ranges. This is what lets the same readers run in a browser, open Python `bytes` and file objects, and read from S3 or an HTTP server through code the host writes. OpenReadout itself never opens a network connection.

```rust
use std::sync::Arc;
use openreadout_core::source::{CachedSource, CallbackSource, Input};

// A range reader written by the host (HTTP range requests, S3 GetObject with Range, ...).
let remote = CallbackSource::new("plate1.czi", size, move |offset, buf| my_http_range(offset, buf));
let input = Input::from_source("plate1.czi", Arc::new(CachedSource::with_defaults(Arc::new(remote))));
let (detection, dataset) = registry.open_input(&input)?;
let info = dataset.info()?;
```

A buffer is `Input::from_bytes(name, bytes)`, and a set of files held in memory is a `MemFs`. The design is described in [`docs/architecture.md`](https://github.com/openreadout/openreadout/blob/main/docs/architecture.md).

## Building the module

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version <the wasm-bindgen version in Cargo.lock>
scripts/wasm.sh build        # writes packaging/wasm/pkg/
scripts/wasm.sh test         # tests in Node
scripts/wasm.sh demo         # the demo page, after `mdbook build book`
```

`scripts/wasm.sh build` runs `wasm-opt` to shrink the module when binaryen is installed (`cd packaging/wasm && npm install` provides it).

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
