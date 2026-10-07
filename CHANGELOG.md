# Changelog

All notable changes to this project are documented here. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- An interactive viewer in the chat for clients with MCP Apps (Claude Desktop, claude.ai, ChatGPT, the Codex app, VS Code): image planes, sweeps, NMR spectra, chromatograms with their spectra, plates and flow-cytometry plots, and a file viewer for vendor files opened in the ChatGPT and Codex desktop apps.
- A Codex plugin and marketplace (`codex plugin marketplace add openreadout/agent-plugins`) and a Gemini CLI extension (`gemini extensions install https://github.com/openreadout/agent-plugins`).
- Thermo `.raw` files from current instruments are validated against their depositors' conversions: Orbitrap Astral (Astral-analyzer `ASTMS` scans, DIA), Ascend, Eclipse, IQ-X and ID-X, FAIMS compensation voltages (`cv=`), TSQ Altis Plus SRM runs, TSQ 9610 and ISQ GC full scans, and file versions 57, 61 and 62 (LTQ, LTQ Orbitrap and LTQ FT runs from 2005–2008).
- The release workflow can sign and notarize the macOS binaries with a Developer ID, and sign the Windows binary with Azure Artifact Signing. `scripts/macos-sign.sh` does the macOS part and also runs on a Mac.
- `.zenodo.json`, so Zenodo can archive each release with a DOI.
- `cargo binstall openreadout` on Windows on Arm installs the x64 build.
- qPCR results exports: the Results tables of Applied Biosystems software (StepOne, 7500, QuantStudio, ViiA 7; `.xls`, `.xlsx`, text, with their amplification and melt curves) and Bio-Rad CFX `Quantification Cq Results` (`.csv`, `.xlsx`) are read as `qpcr-results-export`, so `analyze qpcr` works on them.
- CZI: 12-bit JPEG subblocks and chunked compression (id 7, zstd or LZ4 chunks) are decoded.
- TIFF: 12-bit JPEG pages are read as uint16, and OME Modulo sub-dimensions (FLIM bins, lambda, angles, tiles) are listed in `images[].extra.modulo`.
- VSI: ETS tiles with compression code 5 (lossless JPEG, as some VS120 slides store them) are decoded.
- Malvern Zetasizer `.dts`: size records now return their Z-average, PdI and intensity peak means and areas, checked against the Zetasizer software's exports of two depositors (software 7.10 and 7.12). Peak widths and the number and volume peaks stay withheld, because no export in the corpus holds them. Sample names whose material block begins with 2 instead of 1 are no longer empty.
- Roche LightCycler 480 `.ixo`: `vendor.export_scale` gives the factor that turns each stored amplification reading into the value the LightCycler 480 software exports. The instrument model is taken from the run's instrument name only when that names a LightCycler, so a lab's serial number is no longer reported as the model.
- OME-TIFF export writes int8, int16 and int32 planes (MRC micrographs and segmentations), which it used to refuse with exit 6.
- Sciex QTRAP quadrupole and ion-trap scans: Q1, precursor ion, neutral loss, enhanced MS and enhanced product ion (with precursor charges), validated point for point against the depositors' conversions of five public files.
- Waters ion-mobility and SONAR acquisitions: the drift bins in `_funcNNN.cdt` are read as run 1, one spectrum per bin with its drift time; the 2,000 bins of four test acquisitions equal the vendor library's conversions.
- Empower `.arw` exports whose header has one `"name"<TAB>value` field per line are read. They were refused.
- New development files with independent ground truth raise the confidence of imzML, Bruker ESP and FluoView OIB to high, and of Empower `.arw`, UNICORN `.res`, Zetasizer `.dts`, LightCycler 480 `.ixo`, FluoView OIF, WinWCP, Rigaku RASX and generic HDF5 to medium (`docs/benchmark/gaps-2026-10.md`).

### Changed

- The command line and the MCP server share one set of names, and each command does one thing. Each analysis is its own MCP tool (`openreadout_peaks`, `openreadout_nmr_peaks`, `openreadout_dose_response`, ...) instead of `openreadout_analyze` with `kind`. `check` is split into `check`, `planes`, `compare` and `report`, `spectra` into `scans` and `spectrum`, `export --attachment` into `extract`, `batch summarize` into `summarize`, and `search --health` and `--export` into `health` and `export-dataset`. The formats list is the `openreadout://formats` resource, no longer a tool. Many flags and arguments are renamed (`export --format`, `scans --rt-range`, `trace --first-sample`, ...). `info --view full` leaves out the vendor tree unless `--vendor`, and `stats` lists planes only with `--per plane`. JSON output is `schema_version` 2. The [MCP tools page](https://openreadout.github.io/openreadout/reference/mcp.html) has the names, and `docs/surface-2026-10.md` the full old-to-new mapping.
- The Claude Code plugin, the Codex plugin and the Gemini CLI extension install from their own repository, [openreadout/agent-plugins](https://github.com/openreadout/agent-plugins), which the release workflow updates. Add the Claude Code marketplace again with `/plugin marketplace add openreadout/agent-plugins`.
- A privacy policy, `PRIVACY.md`, linked from the README, the docs site and the `.mcpb` manifest.
- The npm package no longer downloads the binary in a postinstall script. The binary comes in a platform package (`@openreadout/cli-darwin-arm64`, `-darwin-x64`, `-linux-arm64`, `-linux-x64`, `-win32-x64`) that the package manager installs as an optional dependency, so it works with pnpm 10, behind proxies and from registry mirrors.
- The install scripts check the download against the release's `SHA256SUMS`. `install.ps1` reads `OPENREADOUT_VERSION` like `install.sh`, works on Windows PowerShell 5.1 with older TLS defaults, and puts the program on the `PATH` of the current terminal as well. `install.sh` prints the line to add to your shell's startup file when `~/.local/bin` is not on your `PATH`.

### Fixed

- imzML: spectra that state no MS level are MS1 when the file's `fileContent` names only MS1 spectra, and `check` reports a spectrum whose m/z and intensity arrays differ in length (`bad_array`).
- Rigaku RASX: reciprocal-space maps returned their scans in text order (`Data10` before `Data2`). They are now in scan order.
- Sciex `.wiff`: a scheduled MRM file without stored windows returned every transition in every cycle. Each transition now has its expected time ± half the method's detection window; the layout stays unvalidated, because Analyst decides a window's edge cycles in a way the file does not record.
- Empower `.arw`: channel names no longer keep trailing spaces.
- Assurance: Chromeleon's stored peaks confirm only the signals they were found on, not every trace layout in the archive.
- VSI: raw ETS tiles with three samples per pixel (a DSX-2000 texture map written by PRECiV) came out with red and blue exchanged. They are now returned as red, green, blue.
- Imaris files whose lower resolution levels have fewer z planes failed to export (exit 2), and `preview` failed when it picked such a level. Export no longer copies those levels, and `preview` no longer picks them.
- A file that matched a format only by its extension and then failed to open (a JSON file named `.emd`, a Java object file named `.ser`, a library catalogue named `.mrc`) was reported as truncated. The error now says that only the extension matched.
- SoftMax Pro 6/7 documents (`.sda`) that read two wavelengths, as dual-wavelength ELISAs do, were refused. Each wavelength is now a read of the plate table.
- Gen5 experiment files written by Gen5 1.x were refused. Their reads are now decoded, and the reader, serial number and Gen5 version of their plate description are read at the right offsets. A refused Gen5 file now says why.
- Tecan i-control exports with several reads per well returned no values, and German i-control exports were not recognised. Each well's value is now i-control's `Mean`, and a workbook with one export per sheet gives one plate read per sheet.
- Sciex `.wiff` files with several samples: samples after the first returned the first sample's scan data. Each sample's scans are now read from its own block of the `.wiff.scan`.
- Agilent MassHunter profiles written by MassHunter Acquisition 10 (for example a 6546 Q-TOF) were refused as corrupt LZF; they use the ion-mobility profile encoding and are read.
- The Claude Code plugin failed to load because its marketplace entry and `plugin.json` both declared the skill.
- The Homebrew formula and winget manifests attached to a release no longer start with the template's header comment.
- Docker build records (`*.dockerbuild`) no longer end up among the release assets.
- `CITATION.cff` now validates: the dual license is a list of SPDX identifiers.
- The website's home page and *Connect an assistant* no longer say that there is no release yet.
- Shimadzu: `check` no longer reports `pda_max_plot_mismatch` on PDA runs whose first spectrum is not zero. LabSolutions takes the max plot after subtracting the first spectrum.
- Plate exports: an export the reader recognises but whose values it does not read is now `unvalidated` for tables, so `--strict` refuses it. It was `validated`.
- BMG MARS table views of kinetic reads (a `Time` line under the column titles) return their values with each column's time. They returned no values.
- EC-Lab `.mpt` exports whose time column holds dates and times are read, with times in seconds from the first row. They were rejected as corrupt.
- EC-Lab `.mpr` files with a data module of version 0 (EC-Lab 10 and earlier) are read. They were refused.
- Bruker timsTOF: negative-ion runs no longer get negative 1/K0 values. The assurance block reports these values as derived until a vendor conversion of a negative run confirms them.
- Waters Empower `.arw` exports without header rows are detected and read; their layout is `unvalidated`.
- EC-Lab text exports: a column no development file had no longer makes the traces `unvalidated`, since every column is read by the same number parser.
- TIFF: whole full-resolution planes of NDPI slides between 1 and 4 GiB are read (they exited 4).
- CZI and VSI: `check` no longer exits 4 on channels stored at only some extra-dimension indices, or on stored tiles that lie just past the image edge.
- DM: `check` no longer reports `truncated` when the header's root length counts 4 of the 8 end bytes (30 of the 89 development files).

## [0.1.0] - 2026-10-02

The first public release.

### Added

- Readers for raw files from microscopes, screening systems, flow cytometers, electrophysiology rigs, NMR and optical spectrometers, mass spectrometers, chromatographs, plate readers, qPCR cyclers and other bench instruments. `openreadout self formats` lists them with their validation level.
- Sixteen commands: `info`, `check`, `preview`, `stats`, `trace`, `table`, `spectra`, `analyze`, `export`, `batch`, `link`, `index`, `search`, `watch`, `self` and `mcp`.
- JSON output on every command, with published JSON Schemas and documented exit codes.
- Verified export to OME-TIFF, OME-Zarr, mzML, CSV, Parquet, Arrow, NWB, JCAMP-DX, Allotrope ASM and RDML.
- Analyses of chromatograms, peaks, plate assays, qPCR, NMR spectra, patch-clamp recordings, extracellular spikes and flow-cytometry gates.
- An MCP server, an agent skill and a Claude Code plugin.
- A Python package with bioio and napari plugins, an R package, a WebAssembly build, and Nextflow, Galaxy and Snakemake integrations.
- A documentation website: task recipes with real output, a searchable format list taken from `self formats`, a JSON reference rendered from the published schemas, and a browser demo.
