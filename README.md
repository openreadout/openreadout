# OpenReadout

[![CI](https://github.com/openreadout/openreadout/actions/workflows/ci.yml/badge.svg)](https://github.com/openreadout/openreadout/actions/workflows/ci.yml)
[![Docs](https://github.com/openreadout/openreadout/actions/workflows/docs.yml/badge.svg)](https://openreadout.github.io/openreadout/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

OpenReadout reads raw lab-instrument files without the vendor software, and tells you what is in them as text or JSON.

```console
$ openreadout info mini.nd2
mini.nd2
  format: Nikon ND2 (nd2) v3.0  size: 1.8 KiB  images: 1  planes: 2
  assurance: validated
  [0] -  8x8 z=1 c=1 t=2  uint16 XYCZT  px=0.2500 µm
      ch0 DAPI
```

It is a single binary with no runtime dependencies. It never modifies your files and never uses the network.

## Install

OpenReadout has not had its first release yet. Until then, build it from source with Rust 1.91 or later:

```bash
cargo install --git https://github.com/openreadout/openreadout openreadout
```

From the first release on:

```bash
curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh   # macOS, Linux
brew install openreadout/tap/openreadout
npx openreadout --version
```

Windows, npm, Docker, Nix and the other channels are on the [install page](https://openreadout.github.io/openreadout/getting-started/install.html).

## Examples

```bash
openreadout check run.raw                         # is the file intact? exit code 4 if not
openreadout export slide.lif -o slide.ome.tiff    # convert to an open format, verified on write
openreadout analyze peaks run.D --json            # integrate chromatographic peaks
```

Every command takes `--json`. See the [command reference](https://openreadout.github.io/openreadout/reference/commands/index.html).

## What it reads

OpenReadout reads files from light and electron microscopes, high-content screeners, flow cytometers, electrophysiology rigs, NMR, IR, Raman and UV-Vis spectrometers, mass spectrometers, chromatographs, plate readers, qPCR cyclers and several other bench instruments. It exports to OME-TIFF, OME-Zarr, mzML, Parquet, CSV, NWB and other open formats. Each reader states how well it has been validated. The [format list](https://openreadout.github.io/openreadout/formats/index.html) has the details, and `openreadout self formats` prints the same list.

## Python, R and AI agents

**Python.** The `openreadout` package gives NumPy, dask and xarray access to the same files, with plugins for bioio and napari. Wheels come with each release. See the [Python guide](https://openreadout.github.io/openreadout/guides/python.html).

**R.** The R package returns arrays and data frames with 1-based indices. It builds from source for now. See the [R guide](https://openreadout.github.io/openreadout/guides/r.html).

**AI agents.** `openreadout mcp --install claude-desktop` (or `cursor`, `codex` and others) registers the MCP server. In Claude Code, `/plugin marketplace add openreadout/openreadout` and then `/plugin install openreadout@openreadout` add the skill and the server together. See the [agents guide](https://openreadout.github.io/openreadout/guides/agents.html).

## A file that does not work

Run `openreadout check --report FILE`. It writes a local diagnostic bundle that holds no data values, sample names or paths, and sends nothing. Attach it to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml).

## Links

- Documentation: <https://openreadout.github.io/openreadout/>
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md)
- Security: [SECURITY.md](SECURITY.md)
- Changes: [CHANGELOG.md](CHANGELOG.md)

## License

OpenReadout is licensed under either the [Apache License 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT), at your option. It is not affiliated with any instrument vendor; see [TRADEMARKS.md](TRADEMARKS.md).
