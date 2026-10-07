# Architecture

OpenReadout is a Cargo workspace. Format readers know nothing about the command line, JSON or MCP, and front ends know nothing about any format. The contract between them is two traits and one normalized data model in `openreadout-core`.

```text
   front ends:  openreadout (the binary) · openreadout-mcp · openreadout-py · openreadout-r · openreadout-wasm
                         │
                         │  Registry::open(input) → Box<dyn Dataset>
                         ▼
   openreadout-core:  FormatReader · Dataset · FileInfo and the normalized model · Source tags
                      JSON envelope · Error (code, exit code, hint) · byte sources (Input, Fs)
                         ▲                                         │
   readers: one crate per format family                writers: openreadout-ometiff, -omezarr,
   (openreadout-czi, -nd2, -tiff, -thermo, ...)                 -mzml-writer, -arrow, ...
                         ▲
   openreadout-codecs, openreadout-jpegxr: pure-Rust decoders
```

## Crates

| crates | role | published |
| --- | --- | --- |
| `openreadout-core` | traits, normalized model (OME-style names), experiment model, provenance tags, JSON envelope, error type and exit codes, byte sources, plane selection | crates.io |
| `openreadout-codecs`, `openreadout-jpegxr` | pure-Rust decoders, each behind a feature | crates.io |
| one crate per format family, such as `openreadout-czi`, `-tiff`, `-thermo`, `-nmr`, `-spectro`, `-ephys` | clean-room readers, `#![forbid(unsafe_code)]`; each crate's `MAINTAINING.md` says which formats it holds | crates.io |
| `openreadout-ometiff`, `-omezarr`, `-mzml-writer`, `-arrow` | writers; every output is read back and verified | crates.io |
| `openreadout-ops` | command-level operations shared by the front ends: `compare`, `info --view explain`, `extract`, `--only` | crates.io |
| `openreadout-quant`, `-assay`, `-qpcr`, `-signal`, `-batch`, `-preview` | analyses and previews built on the readers | crates.io |
| `openreadout-index`, `-live` | lab-share catalog and live acquisitions | crates.io |
| `openreadout-mcp` | MCP server over stdio (`rmcp`) | crates.io |
| `openreadout` (`crates/openreadout-cli`) | the binary; embeds `SKILL.md` | crates.io |
| `openreadout-py` | Python bindings (PyO3, abi3 wheels via maturin) | PyPI |
| `openreadout-r` | native part of the R package in `r/openreadout` (extendr) | no |
| `openreadout-wasm` | WebAssembly build (`wasm-bindgen`); npm package in `packaging/wasm`, demo page in `web/` | npm |
| `openreadout-corpus-tests` | the [validation](../book/src/project/validation.md) tests against reference readers | no |
| `openreadout-bench` | benchmarks and memory-ceiling tests ([performance](../book/src/project/performance.md)) | no |
| `xtask` | repository automation (corpus, schemas, vocabulary check, skill parity, packaging); the only crate allowed network access | no |

`cargo deny` checks that nothing linked into the binary can open a network connection, and that every dependency's license is compatible with MIT OR Apache-2.0.

## The two traits

```rust
pub trait FormatReader: Send + Sync {
    fn descriptor(&self) -> FormatDescriptor;                           // id, name, extensions, confidence, known gaps
    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection>;     // first 64 KiB + path
    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>>;
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection>;
    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>>;    // a path, a buffer, a host source
}

pub trait Dataset: Send {
    fn info(&self) -> Result<FileInfo>;                                  // headers only, must stay cheap
    fn vendor_metadata(&self) -> Result<serde_json::Value>;             // the vendor's tree, names untouched
    fn provenance(&self) -> ProvenanceMap;                               // JSON path → Source
    fn entries(&self) -> Result<Vec<LsEntry>>;                           // for `info --view structure`
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane>;
    fn check(&mut self) -> Result<CheckReport>;
    // read_table, read_trace, read_spectrum and others default to `unsupported`
}
```

Readers do not open files with `std::fs`. They open an `Input` through `SourceFile`, `Fs` and `DirSource`, which behave like `std::fs::File` and `std::fs` but can read a local file (with positional reads), a buffer, or a callback the host provides: a Python file object, a browser `Blob`, or a range reader for S3 or HTTP written by the host. Readers of multi-file data sets (directory formats, recordings split over several files) find their member files through the same interface, so they also work from memory. `open(path)` is a thin wrapper over `open_input(&Input::local(path))`. See the [WebAssembly guide](../book/src/guides/wasm.md).

`Registry` holds the readers in priority order. Detection hands each reader the first 64 KiB; a definite match wins, otherwise the most confident one. The same registry (`crates/openreadout-cli/src/registry.rs`) backs the command line, the MCP server and the Python package, so they cannot disagree about a file.

`read_plane` returns one full-resolution plane as little-endian bytes, with its geometry and pixel type. Mosaics are stitched on read. `read_plane_level(image, index, level)` reads a downsampled pyramid level; formats without pyramids keep the default, which serves level 0 only. Everything above the plane (selections, export, hashing) is generic.

`FileInfo` also has `tables` (flow cytometry, plate readers), `spectra` (mass spectrometry) and `traces` (electrophysiology, chromatography, spectroscopy), so new format families extend the model instead of forking it.

## Provenance tags

Every normalized field a reader fills is tagged with how its meaning was established, and `info --view full` publishes the map:

| `Source` | JSON | meaning |
| --- | --- | --- |
| `Spec` | `spec` | a published specification or open standard (OME-XML, FCS 3.1) |
| `VendorImpl` | `vendor-impl` | the vendor's own published implementation or documentation (for example ZEISS libCZI concept pages) |
| `PriorArt` | `prior-art` | documentation of a permissively licensed community reader (czifile, liffile, nd2) |
| `Inferred` | `inferred` | our own analysis of corpus files |

Each reader also has a `Confidence` (`high`, `medium` or `low`) that summarizes the same idea for the whole reader. It appears in `self formats` and `info`.

## Errors and exit codes

One error type, `openreadout_core::Error`, maps every failure to a stable string code, an exit code and an optional hint. The command line prints it as a JSON error (or `error:` and `hint:` on stderr), the MCP server as a JSON-RPC error with `{code, exit_code, hint}` in `data`, and the Python package as an exception with the same three attributes. The exit codes are listed in the [command reference](../book/src/reference/commands/index.md#exit-codes).

## Rules every reader follows

- **Headers only for `info`.** Pixel data is read lazily, per plane. `info` on a 3.7 GB CZI takes about 20 ms ([performance](../book/src/project/performance.md)).
- **Never write to the source.** Writers go to a temporary file next to the destination, read every plane back and compare hashes, then rename the file into place.
- **No panics on bad input.** Checked arithmetic, bounds-checked slices and no `unsafe`; a malformed file becomes `corrupt_file` (exit 4).
- **Bounded memory.** Every size read from a file is checked before it is allocated; the limits are in [the memory model](architecture-memory.md).
- **Own vocabulary.** Every public identifier in a format crate appears in that format's vocabulary table (`docs/formats/<fmt>.md`); `cargo xtask vocab-check` enforces it in CI. See the [clean-room policy](legal/clean-room-policy.md).
- **Stable JSON.** Payload types derive `JsonSchema`. `cargo xtask schema gen` writes `docs/schema/*.json`, CI fails on drift, and the website's [JSON reference](https://openreadout.github.io/openreadout/reference/json.html) is rendered from those files at build time (`book/site/pages/reference/json.astro`).

## Adding a format

The full steps are in [`maintaining.md`](maintaining.md#adding-a-format). In short:

1. Write down what you know in `docs/formats/<fmt>.md` and start `docs/provenance/<fmt>.md`.
2. Add public files to the corpus and generate reference values with `oracle/gen.py`.
3. Implement `FormatReader` (with `open_input`, reading through `Input`) and `Dataset` in a format crate.
4. Register the reader in `crates/openreadout-cli/src/registry.rs`.
5. Make the corpus tests pass.
