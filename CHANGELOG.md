# Changelog

All notable changes to this project are documented here. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- `stats` on large CZI mosaics is fast again. Reading the plane in strips had made it decode the tiles that cross a strip edge twice. Now each tile is decoded once, and `stats` also accumulates the colour components of a strip in parallel.

## [0.2.0] - 2026-10-07

The second release. In short:

- **A new command line and MCP surface.** The command line and the MCP server now use the same names, and each command and tool does one thing: every analysis is its own MCP tool, and `check`, `spectra`, `export --attachment`, `batch summarize` and `search --health`/`--export` are split into separate commands. Many flags are renamed and JSON output is `schema_version` 2. Scripts and prompts written for 0.1 need changes; [Upgrading from 0.1](https://openreadout.github.io/openreadout/getting-started/upgrading.html) maps every old name to its new one.
- **A viewer in the chat.** Clients with MCP Apps (Claude Desktop, claude.ai, ChatGPT, the Codex app, VS Code) show image planes, sweeps, spectra, chromatograms, plates and flow-cytometry plots in an interactive viewer.
- **More mass spectrometry.** Current Thermo instruments (Orbitrap Astral, Ascend, Eclipse, IQ-X, ID-X, TSQ Altis Plus, FAIMS runs) are validated, Waters ion-mobility and SONAR drift bins and Sciex QTRAP scans are read, and Agilent GC/MSD and MassHunter 10 data that used to come back empty or wrong are fixed.
- **Microscopy codecs and whole slides.** CZI 12-bit JPEG and chunked zstd/LZ4 subblocks, 12-bit JPEG TIFF pages, lossless-JPEG VSI tiles, Hamamatsu NDPI sets and OME-Zarr NGFF 0.5 stores are read. `stats` works on whole-slide levels larger than memory.
- **Plates and qPCR.** Applied Biosystems and Bio-Rad CFX results exports are read, `analyze qpcr` can compute Cq itself, and Gen5, SoftMax Pro, Tecan and BMG exports that were refused or returned no values now work.
- **Tested on new data.** New held-out files and bug hunts in imaging, mass spectrometry, chromatography and signals found the errors fixed below. New development files with independent ground truth raise the confidence of several readers.
- **Easier installs.** npm installs the binary from a platform package instead of downloading it, the macOS binaries are signed and notarized, the install scripts check the release checksums, and there is a bioconda recipe for the program and conda-forge recipes for the Python packages.

### Added

- An interactive viewer in the chat for clients with MCP Apps (Claude Desktop, claude.ai, ChatGPT, the Codex app, VS Code): image planes, sweeps, NMR spectra, chromatograms with their spectra, plates and flow-cytometry plots, and a file viewer for vendor files opened in the ChatGPT and Codex desktop apps.
- A Codex plugin and marketplace (`codex plugin marketplace add openreadout/agent-plugins`) and a Gemini CLI extension (`gemini extensions install https://github.com/openreadout/agent-plugins`).
- Thermo `.raw` files from current instruments are validated against their depositors' conversions: Orbitrap Astral (Astral-analyzer `ASTMS` scans, DIA), Ascend, Eclipse, IQ-X and ID-X, FAIMS compensation voltages (`cv=`), TSQ Altis Plus SRM runs, TSQ 9610 and ISQ GC full scans, and file versions 57, 61 and 62 (LTQ, LTQ Orbitrap and LTQ FT runs from 2005–2008).
- Waters ion-mobility and SONAR acquisitions: the drift bins in `_funcNNN.cdt` are read as run 1, one spectrum per bin with its drift time. The 2,000 bins of four test acquisitions equal the vendor library's conversions.
- Sciex QTRAP quadrupole and ion-trap scans: Q1, precursor ion, neutral loss, enhanced MS and enhanced product ion (with precursor charges), validated point for point against the depositors' conversions of five public files.
- CZI: 12-bit JPEG subblocks and chunked compression (id 7, zstd or LZ4 chunks) are decoded.
- TIFF: 12-bit JPEG pages are read as uint16, and OME Modulo sub-dimensions (FLIM bins, lambda, angles, tiles) are listed in `images[].extra.modulo`.
- VSI: ETS tiles with compression code 5 (lossless JPEG, as some VS120 slides store them) are decoded.
- Hamamatsu NDPI sets (`.ndpis`, one NDPI per fluorescence channel) are read as one image with a channel per listed file, named from the file's filter set. A missing file is a `missing_file` finding of `check`, and reading its channel exits 4 with a hint.
- OME-Zarr: two public NGFF 0.5 stores from the Image Data Resource (sharded Zarr v3, with a label image) are development files, so NGFF 0.5 is validated on files OpenReadout did not write. `resolution_levels` gives a sharded array's inner chunk size as its tile size.
- OME-TIFF export writes int8, int16 and int32 planes (MRC micrographs and segmentations), which it used to refuse with exit 6.
- qPCR results exports: the Results tables of Applied Biosystems software (StepOne, 7500, QuantStudio, ViiA 7; `.xls`, `.xlsx`, text, with their amplification and melt curves) and Bio-Rad CFX `Quantification Cq Results` (`.csv`, `.xlsx`) are read as `qpcr-results-export`, so `analyze qpcr` works on them.
- `analyze qpcr --compute-cq` (`openreadout_qpcr` `compute_cq`) can compute Cq by `--cq-method` (`cq_method`) `stored-threshold` or `second-derivative`, and LightCycler 480 `.ixo` files now default to `second-derivative`, which comes much closer to the instrument's own Cp.
- Roche LightCycler 480 `.ixo`: `vendor.export_scale` gives the factor that turns each stored amplification reading into the value the LightCycler 480 software exports. The instrument model is taken from the run's instrument name only when that names a LightCycler, so a lab's serial number is no longer reported as the model.
- Malvern Zetasizer `.dts`: size records now return their Z-average, PdI and intensity peak means and areas, checked against the Zetasizer software's exports of two depositors (software 7.10 and 7.12). Peak widths and the number and volume peaks stay withheld, because no export in the corpus holds them. Sample names whose material block begins with 2 instead of 1 are no longer empty.
- Empower `.arw` exports whose header has one `"name"<TAB>value` field per line are read. They were refused.
- New development files with independent ground truth raise the confidence of imzML, Bruker ESP, FluoView OIB, Sciex `.wiff` and TIA `.ser` to high, and of Empower `.arw`, UNICORN `.res`, Zetasizer `.dts`, LightCycler 480 `.ixo`, FluoView OIF, WinWCP, Rigaku RASX and generic HDF5 to medium.
- The release workflow can sign and notarize the macOS binaries with a Developer ID, and sign the Windows binary with Azure Artifact Signing. `scripts/macos-sign.sh` does the macOS part and also runs on a Mac.
- `cargo binstall openreadout` on Windows on Arm installs the x64 build.
- A bioconda recipe for the program (`integrations/bioconda/`) and conda-forge recipes for the Python packages (`integrations/conda-forge/`).
- `.zenodo.json`, so Zenodo can archive each release with a DOI.
- A privacy policy, `PRIVACY.md`, linked from the README, the docs site and the `.mcpb` manifest.
- [Upgrading from 0.1](https://openreadout.github.io/openreadout/getting-started/upgrading.html), a page in the docs that maps the 0.1 commands, tools and flags to their 0.2 names.

### Changed

- **Breaking: renamed commands and MCP tools.** `check --planes`, `check --against` and `check --report` are now the commands `planes`, `compare` and `report`. `spectra` is split into `scans` (the scan list) and `spectrum` (one spectrum). `export --attachment` is `extract`, `batch summarize` is `summarize`, and `search --health` and `search --export` are `health` and `export-dataset`. `index --health` is gone. The plate-reader analyses are `analyze assay-wells`, `assay-curve`, `dose-response`, `kinetics`, `growth` and `assay-qc`. In MCP, each analysis is its own tool (`openreadout_peaks`, `openreadout_nmr_peaks`, `openreadout_dose_response`, ...) instead of `openreadout_analyze` with `kind`, `openreadout_spectra` is `openreadout_scans` and `openreadout_spectrum`, and `openreadout_compare`, `openreadout_report`, `openreadout_extract`, `openreadout_summarize` and `openreadout_health` are new. The formats list is the `openreadout://formats` resource, no longer a tool. A tool's name is `openreadout_` plus the command, and its arguments are the command's long flags. There are no aliases for the old names.
- **Breaking: renamed flags and arguments.** Among them: `export --to` is `--format`, `scans --rt` is `--rt-range`, `trace --first` is `--first-sample`, `preview --plain` and `--grid` are `--axes`, and the qPCR, NMR, spike, patch-clamp and plate-assay flags spell out their units and roles (`--compute-cq`, `--range-ppm`, `--band-hz`, `--peak-threshold-mv`, `--blank-wells`). `--samples` and `--layout` as spellings of `--sample-sheet` are gone. [Upgrading from 0.1](https://openreadout.github.io/openreadout/getting-started/upgrading.html) lists every rename.
- **Breaking: JSON output is `schema_version` 2.** `info --view full` leaves out the vendor tree unless you add `--vendor`, and `stats` lists planes only with `--per plane`.
- LIF: channels are named after the dye LAS X records for them (`DAPI`, `ALEXA 488`), which is also reported as `fluorophore`. Channels without one keep the display colour's name (`Red`), and the colour stays in `color`.
- The Claude Code plugin, the Codex plugin and the Gemini CLI extension install from their own repository, [openreadout/agent-plugins](https://github.com/openreadout/agent-plugins), which the release workflow updates. Add the Claude Code marketplace again with `/plugin marketplace add openreadout/agent-plugins`.
- The npm package no longer downloads the binary in a postinstall script. The binary comes in a platform package (`@openreadout/cli-darwin-arm64`, `-darwin-x64`, `-linux-arm64`, `-linux-x64`, `-win32-x64`) that the package manager installs as an optional dependency, so it works with pnpm 10, behind proxies and from registry mirrors.
- The install scripts check the download against the release's `SHA256SUMS`. `install.ps1` reads `OPENREADOUT_VERSION` like `install.sh`, works on Windows PowerShell 5.1 with older TLS defaults, and puts the program on the `PATH` of the current terminal as well. `install.sh` prints the line to add to your shell's startup file when `~/.local/bin` is not on your `PATH`.
- Plate exports: an export the reader recognises but whose values it does not read is now `unvalidated` for tables, so `--strict` refuses it. It was `validated`.
- Assurance: Chromeleon's stored peaks confirm only the signals they were found on, not every trace layout in the archive.
- EC-Lab text exports: a column no development file had no longer makes the traces `unvalidated`, since every column is read by the same number parser.
- The documentation site has one page, [How we validate](https://openreadout.github.io/openreadout/project/how-we-validate.html), in place of the Validation, Project health, Evidence per format, m/z agreement and Performance pages.

### Fixed

- `stats` reads large planes of tiled and pyramid images in strips, so a whole-slide level 0 no longer has to fit in memory (2.9 GB to 0.5 GB peak on a 53,760 × 17,664 NDPI, same results). Slides whose level 0 is over 4 GiB, which `stats` refused before, now work.
- TIFF: whole full-resolution planes of NDPI slides between 1 and 4 GiB are read (they exited 4).
- VSI: raw ETS tiles with three samples per pixel (a DSX-2000 texture map written by PRECiV) came out with red and blue exchanged. They are now returned as red, green, blue.
- CZI and VSI: `check` no longer exits 4 on channels stored at only some extra-dimension indices, or on stored tiles that lie just past the image edge.
- Imaris files whose lower resolution levels have fewer z planes failed to export (exit 2), and `preview` failed when it picked such a level. Export no longer copies those levels, and `preview` no longer picks them.
- DM: `check` no longer reports `truncated` when the header's root length counts 4 of the 8 end bytes (30 of the 89 development files).
- mzML: optical spectra (a PDA detector's, with a wavelength array instead of m/z) no longer make `export --format mzml` panic. `spectrum` refuses them with exit 6 and a hint, and the mzML export leaves them out and reports how many in `spectra_skipped`.
- OME-Zarr export streams planes larger than 256 MiB block by block instead of above 4 GiB. A 20563 × 20164 RGB plane now peaks at 0.9 GB instead of 2.9 GB, and the store is byte-identical.
- OME-Zarr export stores chunks that hold only zeros. A fresh export whose last image was blank (an empty label image, a dark frame) read as an acquisition in progress for five minutes, and `planes` returned nothing in that time.
- OME-Zarr: while a store is still being written, the planes of its finished images (earlier wells of a plate) count as complete. `planes` and `check` left them out.
- VSI: reading a whole slide plane holds it once, not twice (3.8 GB to 2.2 GB peak for a 1.9 GB plane, same pixels).
- Exporting a file that holds no images (a VSI without its `.ets` folder) exits 6 and says why. It exited 2 with a hint about `--select`.
- OME-TIFF and OME-Zarr export of 64-bit integer and complex images, and OME-TIFF export of 2- and 4-sample images, exit 6 with a hint that says what works. The hints sent each writer to the other, which refused the same file.
- imzML: spectra that state no MS level are MS1 when the file's `fileContent` names only MS1 spectra, and `check` reports a spectrum whose m/z and intensity arrays differ in length (`bad_array`).
- mzML spectra that carry a text array (`null-terminated ASCII string`) failed as corrupt.
- Sciex `.wiff`: a scheduled MRM file without stored windows returned every transition in every cycle. Each transition now has its expected time ± half the method's detection window; the layout stays unvalidated, because Analyst decides a window's edge cycles in a way the file does not record.
- Sciex `.wiff` files with several samples: samples after the first returned the first sample's scan data. Each sample's scans are now read from its own block of the `.wiff.scan`.
- Sciex MRM `.wiff` files with hundreds of precursors per cycle used memory in proportion to their spectra: a 14.5 MB API 4000 file with 30 million spectra took 1.8 GB to open and 5.8 GB for `analyze chromatogram`. It now takes 33 MB and 300 MB, and the chromatogram takes 2 s instead of 16 s.
- `export --format mzml` refused mzML and Waters MRM files that hold chromatograms and no spectra. It now writes their chromatograms (Waters MRM tables as a TIC and one SRM chromatogram per transition) with no spectrum list. Chromatogram precursors now carry the activation the mzML schema requires, and a run id made from a file name with spaces is a valid XML ID.
- Agilent GC/MSD data directories (5975, 5977) returned every spectrum empty with no error. Their points, kept in `MSPeak.bin`, are now read.
- Agilent 7010C GC triple-quadrupole full scans had wrong abundances (f32 values read as integers).
- An Agilent data directory missing the `MSProfile.bin` its scans point into returned empty spectra. Such scans are now an error, and `info` names the missing file.
- Agilent data directories deposited with an empty `MSProfile.bin` (a 6224 TOF run) failed as corrupt partway through. Their scans now read from `MSPeak.bin`.
- Agilent `.d` directories whose `MSScan.xsd` prefixes its type names (`mstns:`) were refused as corrupt.
- Agilent MassHunter profiles written by MassHunter Acquisition 10 (for example a 6546 Q-TOF) were refused as corrupt LZF; they use the ion-mobility profile encoding and are read.
- Empower `.arw`: channel names no longer keep trailing spaces.
- Waters Empower `.arw` exports without header rows are detected and read. Their layout is `unvalidated`.
- Shimadzu: `check` no longer reports `pda_max_plot_mismatch` on PDA runs whose first spectrum is not zero. LabSolutions takes the max plot after subtracting the first spectrum.
- Bruker timsTOF: negative-ion runs no longer get negative 1/K0 values. The assurance block reports these values as derived until a vendor conversion of a negative run confirms them.
- SoftMax Pro 6/7 documents (`.sda`) that read two wavelengths, as dual-wavelength ELISAs do, were refused. Each wavelength is now a read of the plate table.
- Gen5 experiment files written by Gen5 1.x were refused. Their reads are now decoded, and the reader, serial number and Gen5 version of their plate description are read at the right offsets. A refused Gen5 file now says why.
- Gen5 Excel exports: kinetic tables that start in column B are read, every worksheet that holds a Gen5 export becomes its own table, and `check` reports worksheets that no reader read (`worksheet_not_read`).
- Tecan i-control exports with several reads per well returned no values, and German i-control exports were not recognised. Each well's value is now i-control's `Mean`, and a workbook with one export per sheet gives one plate read per sheet.
- BMG MARS table views of kinetic reads (a `Time` line under the column titles) return their values with each column's time. They returned no values.
- Rigaku `.ras` files edited by hand are read: data rows commented out with `#` are left out with a warning, and a file that lost its `*RAS_INT_END` trailer is read when every declared row is there.
- Rigaku RASX: reciprocal-space maps returned their scans in text order (`Data10` before `Data2`). They are now in scan order.
- JASCO flat `.jws` files with two channels (circular dichroism and HT voltage, J-810) are read. They were refused with exit 6.
- PerkinElmer `.sp` files saved as text (`PE … ASCII PEDS`, e.g. from an LS55) were called corrupt. Their spectra are now read.
- JEOL: a `.jdf` file whose `JEOL.NMR` signature is damaged now exits 4 (corrupt) instead of 3 (unknown format).
- EC-Lab `.mpt` exports whose time column holds dates and times are read, with times in seconds from the first row. They were rejected as corrupt.
- EC-Lab `.mpr` files with a data module of version 0 (EC-Lab 10 and earlier) are read. They were refused.
- `analyze ephys-features` refused every NWB intracellular series, because NWB spells its units `volts` and `amperes`. It now reads them.
- ABF: `info` no longer lists a command trace that cannot be read (epochs that run past the end of the sweep), which also made `export --format nwb` fail.
- ABF: a damaged header that declares millions of sweeps no longer exhausts memory when the file's structure is listed.
- Blackrock: `info` could take a PTP file as one gap-free sweep when a forward jump and a clock reset cancelled out. It now reads every timestamp of files up to 64 MiB and reports the sweep layout as assumed in larger ones.
- `analyze spikes` and `trace` held every channel of a long recording in memory at once (1.7 GB and 830 MB on a 234 MB, 65-channel `.ns6`). They now read in bounded pages (1.0 GB and 190 MB).
- A file that matched a format only by its extension and then failed to open (a JSON file named `.emd`, a Java object file named `.ser`, a library catalogue named `.mrc`) was reported as truncated. The error now says that only the extension matched.
- The Claude Code plugin failed to load because its marketplace entry and `plugin.json` both declared the skill.
- The Homebrew formula and winget manifests attached to a release no longer start with the template's header comment.
- Docker build records (`*.dockerbuild`) no longer end up among the release assets.
- `CITATION.cff` now validates: the dual license is a list of SPDX identifiers.
- The website's home page and *Connect an assistant* no longer say that there is no release yet.
- Thermo: `spectrum --exclude-flagged` on a profile scan zeroed every value of a stored chunk that reached a flagged peak's m/z, including a larger unflagged peak in the same chunk. It now zeroes only the values up to the flagged peak's intensity, which matches conversions made before October 2020 on an Orbitrap Elite run.

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
