# OpenReadout

OpenReadout lets AI agents open and analyze raw files from lab instruments: microscopes, mass spectrometers, flow cytometers, electrophysiology rigs, plate readers, NMR and more, 120+ formats in all. It's one open-source binary that needs no vendor software, and it runs as an MCP server, a command-line tool, and Python and R packages.

[![CI](https://github.com/openreadout/openreadout/actions/workflows/ci.yml/badge.svg)](https://github.com/openreadout/openreadout/actions/workflows/ci.yml)
[![Docs](https://github.com/openreadout/openreadout/actions/workflows/docs.yml/badge.svg)](https://openreadout.github.io/openreadout/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

<p align="center">
  <strong>🧪 Try it:</strong> <a href="https://openreadout.github.io/openreadout/demo/">browser demo</a> &nbsp;|&nbsp; <strong>📖 Docs:</strong> <a href="https://openreadout.github.io/openreadout/">openreadout.github.io/openreadout</a>
</p>

https://github.com/user-attachments/assets/ac587ab7-3388-44a2-9fc2-b58cea75a29f

<p align="center"><em>An agent answers "Did my GMDS knockout work?" from 31 files made by five instruments. Data: Edyanto et al., figshare <a href="https://doi.org/10.6084/m9.figshare.33265719">10.6084/m9.figshare.33265719</a> (CC BY 4.0).</em></p>

<details>
<summary>Another example: the agent finds the cerebellum in a 3.7 GB slide scan and labels the layers</summary>

https://github.com/user-attachments/assets/7ee0f9fe-e6e0-4378-a46d-5cdc04d57151

</details>

## Get Started

Install OpenReadout, then connect it to your agent:

```bash
brew install openreadout/tap/openreadout    # other ways under Installation below
openreadout mcp --install claude-desktop    # or claude, codex, cursor, vscode, gemini
```

Then ask your agent about a file, for example "What's in cells.czi?" or "Plot the growth curves in run1.pda by well." In Claude Code you can install the MCP server and the skill together as a plugin (see [AI Integration](#ai-integration)).

To try it without installing anything, open the [browser demo](https://openreadout.github.io/openreadout/demo/) and tap one of its sample files, or drop in a file of your own. It runs OpenReadout compiled to WebAssembly inside the page, and nothing is uploaded.

## What It Reads

<p align="center">
  <img src=".github/assets/mouse-czi.jpg" alt="Sagittal section of a whole mouse, trichrome stained, decoded from a Zeiss CZI slide scan" width="100%">
</p>

<p align="center"><em>A whole-mouse section from a 3.7 GB Zeiss slide scan: 190,309 × 69,378 pixels at 0.22 µm.</em></p>

<p align="center">
  <img src="book/public/img/gallery/slide-fold.png" alt="One fold of the mouse cerebellum at full resolution, with the layers labelled at file-pixel coordinates">
</p>

<p align="center"><em>One fold of its cerebellum at full resolution, labelled the way an agent does it: the rulers are file pixels, so labels go where the agent sees each layer.</em></p>

<p align="center">—</p>
<p align="center"><strong>Microscopy</strong></p>

<table>
<tr>
<td width="33%"><img src=".github/assets/convallaria-lif.jpg" alt="Convallaria rhizome cross-section from a Leica LIF lambda scan"></td>
<td width="33%"><img src=".github/assets/batio3-dm3.jpg" alt="Atomic-resolution STEM image of barium titanate from a Gatan DM3 file"></td>
<td width="33%"><img src=".github/assets/he-qptiff.jpg" alt="H&amp;E-stained tissue section from a PerkinElmer QPTIFF whole-slide scan"></td>
</tr>
<tr>
<td align="center"><sub>Leica LIF · confocal lambda scan</sub></td>
<td align="center"><sub>Gatan DM3 · atomic-resolution STEM</sub></td>
<td align="center"><sub>PerkinElmer QPTIFF · H&amp;E whole slide</sub></td>
</tr>
</table>

<p align="center">—</p>
<p align="center"><strong>Figures, Drawn From the Raw Files</strong></p>

<table>
<tr>
<td width="50%"><img src="book/public/img/gallery/plate-grid.png" alt="96 growth curves from a SoftMax plate read, drawn in plate positions"></td>
<td width="50%"><img src="book/public/img/gallery/fcs-gates.png" alt="CD4 and CD8 T-cell populations gated from a FlowJo workspace"></td>
</tr>
<tr>
<td align="center"><sub>SoftMax .pda · 96 growth curves in plate positions</sub></td>
<td align="center"><sub>BD FCS and FlowJo · gated T cells</sub></td>
</tr>
<tr>
<td width="50%"><img src="book/public/img/gallery/fi.png" alt="Firing rate against injected current, with example sweeps, from an Axon ABF recording"></td>
<td width="50%"><img src="book/public/img/gallery/qpcr.png" alt="qPCR amplification curves on a log axis from an Applied Biosystems run"></td>
</tr>
<tr>
<td align="center"><sub>Axon ABF · firing against current</sub></td>
<td align="center"><sub>Applied Biosystems .eds · qPCR amplification</sub></td>
</tr>
<tr>
<td width="50%"><img src="book/public/img/gallery/ir-overlay.png" alt="Four ATR-FTIR spectra stacked with their bands labelled"></td>
<td width="50%"><img src="book/public/img/gallery/nmr.png" alt="A proton NMR spectrum of sucrose processed from the raw FID"></td>
</tr>
<tr>
<td align="center"><sub>Thermo OMNIC .spa · FTIR spectra</sub></td>
<td align="center"><sub>Bruker TopSpin · NMR from the raw FID</sub></td>
</tr>
</table>

<p align="center"><em>Everything above was read by OpenReadout from raw files, with no vendor software. The <a href="https://openreadout.github.io/openreadout/gallery.html">gallery</a> has more, with the command behind each picture.</em></p>

## What It Does

- **Inspect** images, channels, traces, spectra, tables, and metadata, in plain text or structured JSON
- **Check** files for truncation, missing planes, and damaged structure, with exit code 4 when a file is corrupt
- **Export** to OME-TIFF, OME-Zarr, mzML, NWB, CSV, Parquet, Arrow, JCAMP-DX, Allotrope ASM, and RDML, reading every export back to verify it
- **Preview** image planes, traces, spectra, and plate heat maps as PNG
- **Analyze** chromatographic peaks, plate assays (IC50, standard curves), qPCR (Cq, ΔΔCq), NMR peaks, patch-clamp features, spikes, and flow-cytometry gates, with documented methods
- **Batch** over whole directories, index lab shares, and watch running acquisitions

## On the Command Line

```bash
# Install (macOS / Linux; other ways under Installation)
curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh

# What is in the file?
openreadout info cells.lif
# → format: Leica LIF (lif) v2  size: 16.0 MiB  images: 1  planes: 2
# →   [0] PEI_laminin_35k  2048x2048 z=1 c=2 t=1  uint16  px=0.3250 µm
# →       objective: HC PL FLUOTAR L 20x/0.40 DRY

# Is it complete?
openreadout check partial-copy.lif
# → error  truncated       block chain runs past end of file
# → error  missing_planes  geometry needs 16777216 bytes but only 8969789 are stored

# Integrate the peaks of a chromatogram
openreadout analyze peaks gc-run.ch --min-height 1
# → 4 peaks, area in pA·min
# → 1   4.852 min  area 0.2779  25.08 %
# → ...

# Structured JSON for scripts and agents
openreadout info cells.lif --json
```

```json
{
  "ok": true,
  "schema_version": "2",
  "data": {
    "format": { "id": "lif", "name": "Leica LIF", "vendor": "Leica Microsystems" },
    "images": [
      {
        "size_x": 2048, "size_y": 2048, "size_c": 2,
        "pixel_type": "uint16",
        "physical_size": { "x": 0.325, "y": 0.325, "unit": "µm" }
      }
    ]
  }
}
```

<p align="center">
  <img src=".github/assets/demo.gif" alt="Terminal session: openreadout info describes a Leica LIF file, check reports that a truncated copy is incomplete and exits with code 4, export writes a verified OME-TIFF, analyze peaks lists four peaks in a GC chromatogram, and info --json piped to jq prints the pixel size." width="100%">
</p>

## Formats

| Area | Formats | Export to |
| --- | --- | --- |
| Light microscopy | Zeiss CZI, Nikon ND2, Leica LIF, Olympus OIR/VSI/OIB, Imaris, OME-TIFF and other TIFF variants, OME-Zarr, whole-slide images (including Philips iSyntax, Hamamatsu VMS and MIRAX) | OME-TIFF, OME-Zarr |
| High-content screening | Harmony (Opera Phenix, Operetta), ImageXpress, CellVoyager | OME-Zarr plate, OME-TIFF |
| Electron microscopy | MRC, Gatan DM3/DM4, FEI SER/EMI, Velox EMD, EER movies, SerialEM mdoc | OME-TIFF, OME-Zarr, MRC |
| Structural biology | CBF/miniCBF, SMV, EIGER NeXus (NXmx) diffraction images | MRC, OME-TIFF |
| Medical imaging | DICOM (files, series, studies, DICOMDIR), NIfTI, Analyze, Bruker ParaVision | NIfTI, OME-TIFF |
| Mass spectrometry | Thermo RAW, Bruker timsTOF and BAF, Agilent MassHunter, Waters MassLynx, Sciex WIFF, mzML | mzML, Parquet, Arrow |
| Chromatography | Agilent ChemStation and OpenLab, Shimadzu, Chromeleon, AIA/ANDI | CSV, JCAMP-DX, Parquet |
| Electrophysiology | Axon ABF, Intan, SpikeGLX, Open Ephys, Neuralynx, Blackrock, Plexon, HEKA, Spike2, TDT, Axona, NWB | NWB, CSV, Parquet |
| NMR and spectroscopy | Bruker TopSpin and OPUS, Varian, JEOL, Thermo OMNIC, Renishaw, JCAMP-DX, SPC | JCAMP-DX, CSV |
| Flow cytometry | FCS, FlowJo workspaces, Gating-ML | CSV, Parquet, Arrow |
| Plate readers and qPCR | Plate-reader exports, RDML, Applied Biosystems, LightCycler, Rotor-Gene | Allotrope ASM, RDML, CSV |
| DNA sequencing and electrophoresis | Illumina run folders, Oxford Nanopore POD5 and FAST5, FASTQ, BAM, ABIF Sanger and fragment-analysis files, Agilent Bioanalyzer and TapeStation | FASTQ, CSV, Parquet |
| Other | NanoDrop, Qubit, Incucyte, gel and blot imagers, ÄKTA, ITC, Biacore, Seahorse, Octet, Zetasizer, XRD, EPR, electrochemistry, thermal analysis | CSV, Parquet |

The [format list](https://openreadout.github.io/openreadout/formats.html) lists every format OpenReadout reads, with its known gaps.

## Use Cases

**For Researchers:**
- Open instrument files on any computer, without the acquisition software
- Convert a folder of raw files to OME-Zarr, mzML, or NWB for analysis and sharing
- Verify that files copied off an instrument PC are complete

**For AI Agents:**
- Answer questions about a file: channels, pixel size, objective, acquisition time, scan count
- Extract metadata, traces, spectra, and tables as JSON
- Run documented analyses (peak areas, IC50s, Cq values) and report the method used

**For Core Facilities and Pipelines:**
- Index a lab share into searchable Parquet tables with `index` and `search`
- Watch instrument directories and flag stalled or damaged acquisitions with `watch`
- Run in Nextflow, Snakemake, and Galaxy pipelines ([`integrations/`](integrations))

## Installation

Ships as a single self-contained binary. No Java, no Python, no vendor DLLs -- nothing else to install.

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.ps1 | iex

# Homebrew (macOS / Linux)
brew install openreadout/tap/openreadout

# npm (all platforms; installs the native binary for your platform as a dependency)
npm install -g openreadout

# Rust toolchain
cargo install openreadout --locked
```

Docker, Nix, cargo-binstall, and the other channels are on the [install page](https://openreadout.github.io/openreadout/getting-started/install.html).

Verify installation: `openreadout --version`

## AI Integration

### MCP Server

Built-in [MCP](https://modelcontextprotocol.io) server — register with one command:

```bash
openreadout mcp --install claude          # Claude Code
openreadout mcp --install claude-desktop  # Claude Desktop
openreadout mcp --install codex           # OpenAI Codex
openreadout mcp --install cursor          # Cursor
openreadout mcp --install vscode          # VS Code / Copilot
openreadout mcp --install gemini          # Gemini CLI
```

Windsurf, Zed, Continue, and Cline are supported too. The server exposes 28 tools (`openreadout_info`, `openreadout_check`, `openreadout_preview`, `openreadout_export`, `openreadout_peaks`, ...) over JSON-RPC, so the client doesn't need shell access.

### Claude Code Plugin

Installs the MCP server and the skill together:

```text
/plugin marketplace add openreadout/agent-plugins
/plugin install openreadout@openreadout
```

### Gemini CLI Extension

```bash
gemini extensions install https://github.com/openreadout/agent-plugins
```

### Codex Plugin

```bash
codex plugin marketplace add openreadout/agent-plugins
codex plugin add openreadout@openreadout
```

Each of these adds the skill and the MCP server from the [openreadout/agent-plugins](https://github.com/openreadout/agent-plugins) repository. The server runs the `openreadout` binary from your `PATH`, so [install](#installation) it first.

### Agent Skill

```bash
openreadout self skill --install claude   # ~/.claude/skills/openreadout
openreadout self skill --install agents   # ~/.agents/skills/openreadout (Codex, Cursor, Copilot, Gemini CLI)
```

The skill source is in [`skills/openreadout`](skills/openreadout).

### Built for Agents

- **JSON output.** Every command supports `--json`, with [published schemas](docs/schema).
- **Fixed exit codes.** `0` ok, `1` error, `2` usage, `3` unknown format, `4` corrupt file, `5` I/O, `6` unsupported feature. An agent can branch on the code instead of parsing the message.
- **Errors that say what to try next.** Every error carries a `hint`.
- **Assurance on every answer.** Each result says whether files like it were checked against another reader, and `--strict` refuses values that weren't.
- **Previews the agent can look at.** `preview` writes a PNG of the image, trace or plate the agent is reasoning about.
- **Figures in one call.** `plot` (`openreadout_plot`) draws the figure a scientist would ask for, from the files themselves: plate wells in plate positions, group means with bands, a TIC beside the spectrum at its apex, f–I curves, gated dot plots, fits. PNG for the chat, SVG or PDF for the paper.
- **Cheap metadata.** `info` reads headers only, so a 100 GB file costs about the same as a small one. `--only` returns just the fields asked for.
- **Read-only and offline.** Inputs are opened read-only, and the binary makes no network connections.

### Error Recovery

```bash
# Agent asks for an image that does not exist
openreadout preview cells.lif --image 3 --json
```

```json
{
  "ok": false,
  "error": {
    "code": "usage",
    "message": "usage error: image 3 not found (file has 1 images)",
    "hint": "Indices are zero-based; `openreadout info FILE --json` lists the images, traces (sweep_count, sample_count), tables (row_count) and spectra the file holds.",
    "exit_code": 2
  }
}
```

The agent follows the hint, lists the images, and picks the right index.

## Python and R

**Python** — `pip install openreadout` returns metadata as dicts and pixels as NumPy, dask, or xarray arrays, with plugins for [bioio](https://github.com/bioio-devs/bioio) and napari. See the [Python guide](https://openreadout.github.io/openreadout/guides/python.html).

```python
import openreadout

with openreadout.File("cells.lif") as f:
    f.images[0]["channels"]     # same keys as `info --json`
    stack = f.to_xarray(0)      # labelled with channel names and µm
    f.preview(composite=True)   # shows itself in Jupyter

openreadout.plot(data={"file": "growth.pda"}, facet="plate").save("plate.pdf")
```

**R** — the R package returns arrays and data frames, and draws the same previews and figures. See the [R guide](https://openreadout.github.io/openreadout/guides/r.html).

## Comparison

| | OpenReadout | Bio-Formats | bioio | czifile / nd2 / liffile | msconvert |
|---|---|---|---|---|---|
| Open source & free | ✓ (MIT / Apache-2.0) | ✓ (GPL) | ✓ (plugins vary) | ✓ (BSD) | ✓ (vendor DLLs are not) |
| AI-native CLI + JSON + MCP | ✓ | ✗ | ✗ | ✗ | ✗ |
| Zero install (single binary) | ✓ | ✗ (JVM) | ✗ (Python) | ✗ (Python) | ✗ |
| No vendor DLLs | ✓ | ✓ | ✓ | ✓ | ✗ |
| Integrity check | ✓ | ✗ | ✗ | ✗ | ✗ |
| Microscopy | ✓ | ✓ | ✓ | ✓ (one format each) | ✗ |
| Mass spectrometry | ✓ | ✗ | ✗ | ✗ | ✓ |
| Ephys, flow, NMR, chromatography, plates, qPCR | ✓ | ✗ | ✗ | ✗ | ✗ |
| Cross-platform | ✓ | ✓ | ✓ | ✓ | Windows (or Wine) |

## Validation

Each reader is tested on public instrument files and compared with other readers where one exists, as [How we validate](https://openreadout.github.io/openreadout/project/how-we-validate.html) explains. Each result reports how far its format variant was confirmed.

The readers were written from public files, published specifications and the source of permissively licensed readers, which are credited in [NOTICE](NOTICE). No vendor SDKs, headers or DLLs were used, and GPL readers such as Bio-Formats are only run as black boxes in the tests. Each format keeps a provenance log in [`docs/provenance/`](docs/provenance), and the rules are in the [clean-room policy](docs/legal/clean-room-policy.md).

## Documentation

The [documentation](https://openreadout.github.io/openreadout/) has guides for every command and format:

- **Getting started:** [Install](https://openreadout.github.io/openreadout/getting-started/install.html) | [Your first file](https://openreadout.github.io/openreadout/getting-started/first-file.html) | [Reading the JSON output](https://openreadout.github.io/openreadout/getting-started/reading-json.html)
- **Reference:** [Commands](https://openreadout.github.io/openreadout/reference/commands.html) | [MCP tools](https://openreadout.github.io/openreadout/reference/mcp.html) | [Formats](https://openreadout.github.io/openreadout/formats.html)
- **Gallery:** [previews and figures drawn from public files](https://openreadout.github.io/openreadout/gallery.html)
- **Guides:** [AI agents](https://openreadout.github.io/openreadout/guides/agents.html) | [Python](https://openreadout.github.io/openreadout/guides/python.html) | [R](https://openreadout.github.io/openreadout/guides/r.html) | [Recipes](https://openreadout.github.io/openreadout/recipes.html) | [Batch tables](https://openreadout.github.io/openreadout/guides/batch.html)
- **A file that does not work:** run `openreadout report FILE` and attach the bundle to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml). The bundle contains no data values, sample names, or paths.

## Privacy

OpenReadout runs on your computer, makes no network connections and sends no telemetry. See [PRIVACY.md](PRIVACY.md).

## License

Licensed under either the [Apache License 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT), at your option. OpenReadout is not affiliated with any instrument vendor; see [TRADEMARKS.md](TRADEMARKS.md).

Bug reports and contributions are welcome on [GitHub Issues](https://github.com/openreadout/openreadout/issues). See [CONTRIBUTING.md](CONTRIBUTING.md), and read the [clean-room policy](docs/legal/clean-room-policy.md) before working on a reader.

Images and demo files come from the public test corpus ([`corpus/manifest.toml`](corpus/manifest.toml)), used under their licences: mouse section, Zeiss sample images for Bio-Formats ([Zenodo 10577621](https://zenodo.org/records/10577621), CC-BY-4.0); Convallaria lambda scan, Maria Manuela Azevedo ([Zenodo 14976703](https://zenodo.org/records/14976703), CC-BY-4.0); BaTiO3 STEM, Rama Vasudevan and Gerd Duscher ([Zenodo 8190744](https://zenodo.org/records/8190744), CC-BY-4.0); H&E QPTIFF, PerkinElmer via the OME sample images (CC-BY-4.0); terminal demo, Allen Institute for Cell Science (BSD-3-Clause); knockout chat, Edyanto et al. ([figshare](https://doi.org/10.6084/m9.figshare.33265719), CC-BY-4.0). The figures come from the docs gallery, which lists each source file. [`demo.tape`](.github/assets/demo.tape) regenerates the demo.

---

If you find OpenReadout useful, please [give it a star on GitHub](https://github.com/openreadout/openreadout) — it helps others discover the project.
