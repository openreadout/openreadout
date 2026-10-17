# Changelog

All notable changes to this project are documented here. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-10-17

In short:

- **Figures.** `openreadout plot` and the MCP tool `openreadout_plot` draw finished figures from lab files: growth curves in plate positions, group means, a TIC and the spectrum at its apex, f–I curves, cytometry populations, fits. A few lines of JSON name the data by file and selector, and OpenReadout picks the chart, axes, units and labels.
- **New pictures.** A figure engine with embedded IBM Plex fonts draws every preview and plot, as PNG, JPEG, SVG or PDF, light or dark. Kinetic plates preview as growth curves in plate positions and FCS files as density plots, and an agent can put labelled points, boxes, arrows and bands on any preview.
- **A viewer that follows the agent.** In MCP Apps hosts the viewer flies to the region the agent zoomed into, draws its marks, and lets you select a region, range, wells or events and ask about them.
- **Measurements and answers.** Line profiles, polygon statistics and object counts on images; growth wells flagged as grew, crashed or regrew; the spectrum at a retention time.
- **New instruments.** DNA sequencers (Illumina run folders, nanopore POD5 and FAST5, FASTQ, BAM), Sanger and fragment-analysis files, Bioanalyzer and TapeStation, DICOM, NIfTI and ParaVision, diffraction detectors (CBF, SMV, EIGER NeXus), SerialEM metadata, NanoDrop, Qubit, Incucyte and gel imagers: 122 formats in all.
- **One assay tool.** The six plate-reader tools are one `openreadout_assay`, and the tool list is about a quarter smaller.

### Added

- DNA sequencers (`openreadout-seq`): Illumina run folders (`illumina-run`: RunInfo, RunParameters, InterOp metrics, BCL and CBCL base calls), Oxford Nanopore POD5 and FAST5 raw signal, and FASTQ and BAM read files. `info` gives the run summary per read and lane (% ≥ Q30, density, pass filter, yield, error rate), metrics per cycle and tile and index counts from the metrics alone; nanopore reads come back in pA, one sweep per read; read files give counts, N50 and qualities, estimated from the first 32 MB of very large files. `preview` draws a run-quality page, a read's squiggle or read-length and quality histograms; `export --format fastq` writes FASTQ from an Illumina run's base calls; `plot` bins a table column (`show: histogram`, read lengths by default) and picks a trace channel by name (`y: "percent_q30"`).
- Medical and preclinical imaging (`openreadout-medical`): DICOM files, series and study folders and DICOMDIRs (CT, MR, PET, ultrasound, X-ray, nuclear medicine, enhanced multi-frame, segmentations, whole-slide microscopy as a pyramid; native, deflate, RLE, JPEG, JPEG-LS and JPEG 2000), NIfTI-1, NIfTI-2 and Analyze 7.5, and Bruker ParaVision `2dseq` reconstructions. Values are rescaled (CT in Hounsfield units), voxel sizes and orientation are reported, and patient-identifying data is withheld and flagged. `preview --view axial|coronal|sagittal|orthogonal` and `--contrast window:...` (the file's window or CT presets), `export --format nifti`, and `plot` series with `show: profile`.
- Structural biology: CBF/miniCBF (PILATUS, EIGER, full imgCIF), SMV `.img` (ADSC, Ceta, TimePix) and NeXus NXmx (EIGER master and data files, bitshuffle/LZ4) diffraction images, with the beam and detector geometry in `extra.diffraction`, and SerialEM `.mdoc` metadata on its own or next to its MRC stack. Diffraction previews use log contrast, dark spots on a light ground, masked pixels as no data, overloads in red and resolution rings (`--marks rings`); `preview --sum t` adds a movie's frames (EER events directly) and `--fft` adds a power-spectrum inset; `plot` `show: frames` draws a value per frame or per mdoc section; `export --format mrc` writes MRC2014, with `--group-frames N` for dose fractions.
- Capillary and chip electrophoresis (`openreadout-ce`): Applied Biosystems ABIF (`.ab1`, `.fsa`, `.hid`) Sanger reads with base calls, quality values and the four dye channels, and fragment runs sized from the instrument's size map or a Local Southern fit against the size standard; Agilent Bioanalyzer XML and TapeStation XML with its electropherogram CSV, with RIN, RINe and DIN. `preview` draws a Sanger chromatogram with the called bases, a fragment run per dye on a bp axis and a chip run as small multiples, `export` writes FASTQ from a Sanger read, and trace previews take an `x_range`.
- Gels and blots: `stats --lanes` (MCP `lanes`) finds the lanes of a gel or blot and the bands in each, with volumes, shares and saturation, sized from a ladder lane with `--ladder`, and `preview --marks lanes` draws them. Blot-imager TIFFs (Amersham Imager 680, ImageQuant 800, LAS 4000, Uvitec Alliance, Vilber Fusion, iBright) give their acquisition settings, Syngene GeneSys `.sgd` captures read as images, and Incucyte image exports give pixel size, objective and well.
- NanoDrop and Qubit exports (`openreadout-benchtop`) read as tables of samples, NanoDrop spectra as an absorbance trace, and `preview` draws the spectra beside each sample's concentration and ratios. Incucyte metric exports read as kinetic plates.
- `plot` (MCP `openreadout_plot`): a figure from a JSON spec. Each series names its data by file and selector (a trace, sweep and channel; a chromatogram or the spectrum at a retention time; plate wells by range or layout group; FCS parameters and gate populations; f–I curves and dose-response fits from the analyses; a table file's columns) or gives inline values. Panels, small multiples, plate positions, shared and log axes, colour rules, bands, lines, labelled points and arrows arrange it. Figures are written as PNG, SVG or PDF and verified, and the JSON summary gives each panel's axes and series and the peaks it labelled.
- A figure engine (`openreadout-figure`) draws every picture: vector shapes and real type (IBM Plex Sans and Mono, embedded) rendered in-process to PNG, JPEG or SVG, in the paper-and-ink look of the project, light or dark.
- `preview --annotate` (MCP `annotations`) draws labelled points, boxes, outlines, arrows, text, lines and bands in the picture's own coordinates (file pixels on images, axis units on plots, wells on plates) and reports where each landed.
- Kinetic plate reads preview as a grid of growth curves in plate positions, coloured by the plate layout's groups, with blank wells faint. The heat map of the last time point used to hide wells that crashed and grew back.
- Flow cytometry files preview as a density plot of two parameters (default FSC-A against SSC-A) or a histogram of one; fluorescence axes are asinh.
- Spectra and chromatogram previews label their largest peaks, and `preview --marks spikes` marks the action potentials of a current-clamp sweep.
- NMR data sets preview as their spectrum on a ppm axis with each multiplet labelled (the stored processed spectrum, else the FID processed), mass-spectrometry runs as their total-ion chromatogram, and voltammetry as current against potential.
- The viewer draws `openreadout_plot` figures again in the host's theme and at the viewer's size, redraws them over a dragged x range, and reads out every series' value under the pointer.
- The browser demo opens nine sample files with one tap (a slide scan, a confocal image, a plate-reader run, GC-MS, a neuron recording, flow cytometry, NMR, qPCR and FTIR) and lets you zoom into images, count nuclei, step through sweeps, pick a retention time for its mass spectrum and choose flow axes. The WebAssembly `preview` takes `scale` for sharp pictures on high-density screens.
- `preview` writes SVG (`-o x.svg`), and `--dark` draws in the dark theme. SVG figures come in named groups (series, axes, labels, annotations, panels) that Inkscape and Illustrator open as layers, and their ids are unique to each document, so several figures inlined in one page don't clip each other. Chromatogram and peak plots (`analyze chromatogram -o x.png`, `analyze peaks --plot`) and assay curve plots use the new engine.
- `stats` measures one plane: an intensity profile along a line (`--line`, with FWHM), statistics inside a polygon (`--polygon`), and object counts (`--objects`), so "how many nuclei, how big, how bright, where" has an answer in one call. MCP `line`, `polygon` and `objects` on `openreadout_stats`.
- Object counts take their settings from the image: the nuclear channel by name, the object size measured from the image and bounded by what nuclei measure in µm, background removal, Otsu's two- or three-class threshold, and a seeded watershed that cuts touching nuclei apart at their narrowest and darkest. Stained brightfield (H&E, hematoxylin, IHC) is unmixed by colour deconvolution and nuclei are found in hematoxylin. `--objects=cells` and `--objects=spots` (a Laplacian of Gaussian) count other things. Each object has its mean in every channel, and each marker channel (and DAB) says how many objects are positive. Chromatin texture inside a nucleus no longer reads as a seam between two nuclei, and small pieces left by the watershed join back to their nucleus. Regions of a whole slide up to 8192 × 8192 pixels are counted in tiles. Notes say when the image does not suit the count.
- `preview --marks objects` (MCP `marks: "objects"`, with `objects` for the settings) outlines and numbers the objects with the count, and the viewer outlines them too.
- `analyze growth` says per well whether it grew, crashed or regrew, with the crash and regrowth times, and `growth_summary` lists the wells of each pattern.
- `spectrum --rt MIN` reads the scan at a retention time, and every spectrum lists its most intense peaks with charge, monoisotopic m/z and neutral mass.
- `analyze nmr-peaks --auto-integrate` integrates every multiplet, and `analyze gate --median all` reports the median of every gated parameter.
- The MCP viewer follows the agent: it shows the whole image, then flies to the region the agent previewed, and draws the agent's annotations over the data. Images zoom smoothly with a small map of the whole image. Kinetic plates show a growth curve in each well, with group headings beside their rows. FCS files with a FlowJo workspace or Gating-ML file show populations you can open. A figure from `openreadout_plot` opens in the viewer with hover values.
- In the viewer you can select a region, a range, wells or events and ask the agent about them. The question goes to the chat with the selection in the units the tools take.
- MCP progress notifications carry a message a host can show ("Reading 1,200 of 53,141 spectra"), from exports, chromatograms, peaks, stats, batch and indexing.
- Python: `openreadout.plot()` returns a `Figure` and `File.preview()` (or `openreadout.preview()`) a `Preview`. Both show themselves in Jupyter and save through a verified write, and a `Figure` renders again as PNG, SVG or PDF without reading the files. `File.stats()` takes `line`, `polygon` and `objects`.
- R: `openreadout_plot()` and `openreadout_preview()` return pictures that `plot()` draws and `file =` writes, and `openreadout_stats()` takes `line`, `polygon` and `objects`.
- WebAssembly: `preview()` takes every `openreadout_preview` option, including annotations, marks, `dark` and `format: "svg"`.
- `preview --axes scale-bar` (MCP `axes: "scale-bar"`) frames an image as a figure for people: the picture with a scale bar, a `title` above and a `caption` below, no rulers. `openreadout_export` saves image previews this way unless `axes` says otherwise.
- The website has a [gallery](https://openreadout.github.io/openreadout/gallery.html) of previews and figures drawn by the binary from public files, with the command or spec behind each.

### Changed

- The six plate-reader MCP tools are one, `openreadout_assay`, with `analysis` (`wells`, `curve`, `dose-response`, `kinetics`, `growth`, `qc`). The tool list is about a quarter smaller.
- SoftMax Pro groups are the replicate groups of the plate analyses.
- A spike must rise 15 mV above the trough before it (`--min-rise-mv`, 0 for eFEL's count), and `spectrum --max-points` keeps the most intense points.
- `preview --column` is repeatable (MCP `columns`): a plate's value column, or the parameters of an event preview.
- Previews report `panels` (each plot panel's place and data range), `marks` and `annotations`, so a viewer can map picture pixels to data values.
- WebAssembly: `preview()` resolves to `{ meta, data, type }` (the picture's bytes and MIME type) instead of `{ meta, png }`, previews are 768 px by default, and `axes` replaces the `axes` and `grid` booleans.
- Image previews fill the requested size: `max_size` 768 on a 4000 × 3000 region gives a 768-pixel-wide picture, not 579. `image.downsample` is now the exact (fractional) number of source pixels per picture pixel.
- Previews, plots and the viewer colour categories by one rule: layout groups, populations and series take ink, the accent, blue, ochre, … in the order they first appear, and blank wells are faint. The viewer takes its colours from the figure engine's theme.
- Peak labels on previews, chromatogram plots and the viewer go only on peaks that stand clear of the noise; direct labels at line ends keep a line apart with elbow leaders; flow cytometry axes label decades (`10²`, `10³`, `−10²`); on a log axis, curves that rise from a baseline at zero (qPCR ΔRn) start where they leave the noise.
- In `plot`, an x `range` fits y to the data it shows, end labels, peak labels and annotation labels keep out of each other's way as one set, and the viewer redraws figures on the host's own background, zooms any panel of a multi-panel figure (panels that share x zoom together), and reads out values in an aligned list with series swatches.
- In `plot`, category names under many narrow bars go on two lines, turn 45° or upright, or name every other bar, whichever fits; a long key moves into columns beside the chart, and keys never run off the figure.

### Fixed

- `analyze ephys-features` gave a rheobase for cells that fire during the 0 pA step; it now reports none, with a note and `spontaneous_rate_hz`. Noise near −20 mV in depolarisation block no longer counts as spikes, and the f–I slope stops at the step of highest rate.
- `analyze growth --wells` dropped the layout's blank wells and fell back to each well's own minimum; the blanks are now subtracted.
- The viewer gates every event of a flow cytometry file (up to 2 million, then evenly spread windows), not the first 300,000, and its frame takes the shape of each new region the agent looks at.
- `preview` of an FCS file draws its events again (it said it could not).
- `index` reads symbolic links to files, and `watch` now does too (it skipped them, so a folder of links showed nothing); a skipped link to a directory gets a note naming `--follow-symlinks`, which `watch` and MCP `openreadout_index` and `openreadout_watch` now take. Plane selections such as `c=0,1,z=2` work in every command. MCP tools take a single value where they take a list and a JSON string where they take an object or a list (agents sent the `preview` of `openreadout_export` as a string), and a type error lists the tool's arguments. `preview` and `stats` point FCS and other table-only files at the tools that read them. The website's `/formats/` address works.
- `cargo xtask r-vendor` copies the figure engine's fonts into the R source package.
- A damaged TIFF could make a region read allocate gigabytes, and a damaged OMNIC map could make `info` list millions of image channels. Both now give a clean error (found by the fuzzer).

## [0.3.0] - 2026-10-13

In short:

- **Nine new formats.** Bruker BAF (older Bruker Q-TOF and FT-ICR runs), Philips iSyntax and Hamamatsu VMS slides, TDT, Axona and Multi Channel Systems recordings, Shimadzu UVProbe spectra, Shimadzu GCMSsolution `.qgd` GC-MS runs and OMNIC map files.
- **Checked on about 900 new public files.** Sweeps of microscopy, mass spectrometry, chromatography, flow cytometry, electrophysiology and spectroscopy files from labs the test corpus did not have, compared with independent readers and the depositors' own exports, found the errors fixed below.
- **MRM transitions and lock mass.** `info` states the transition count of MRM runs (MassHunter, Thermo, Sciex) and `table` lists them. Waters spectra can be lock-mass corrected (`--lock-mass`).
- **One breaking change.** `spectrum --nth` (counted from 1) is replaced by `--spectrum` (counted from 0, like every other index).

### Added

- Bruker BAF `.d` directories (micrOTOF, micrOTOF-Q, maXis, impact, compact, timsTOF in BAF mode, apex and solariX FT-ICR): spectra with their centroids and profiles, retention times, precursors and m/z. The m/z follow the recalibration in `Calibrator.ami` when the run has one and agree with the depositors' vendor-library conversions of seven public runs within 0.12 ppm.
- Philips iSyntax slides (data model 5): the wavelet pyramid is decoded tile by tile, with the label, macro and ICC profiles as attachments. Regions and whole levels equal libisyntax's pixels exactly on a public slide.
- Hamamatsu VMS slides: the grid of JPEG images read by restart interval, the map as a coarser level, the macro photo as an attachment, checked against OpenSlide.
- Tucker-Davis Technologies tanks and blocks (`tdt-tank`): stream stores as traces (a tank's blocks as sweeps), snippets as a `spikes` table and epoc onsets with their durations as an `epocs` table. Checked against Neo on six public blocks and tanks.
- Axona DacqUSB sessions (`axona-dacqusb`): the raw `.bin` of the active tetrodes in µV and the tetrode spike files as a `spikes` table. Checked against Neo on three public sessions.
- Multi Channel Systems MC_DataTool binary exports with a header (`mcs-raw`). Checked against Neo on a public file.
- Shimadzu UVProbe `.spc` reader (`shimadzu-uvprobe`): UV-Vis and UV-Vis-NIR spectra (absorbance, reflectance) from the compound file, with the instrument model, serial number, software version and the data set's date. Galactic SPC keeps the extension for its own files. Checked against UVProbe's text exports on 3 files and read on 5 files from 3 public records.
- ABF 1 command waveforms: trace 1 now holds the synthesized DAC command for ABF 1.6 and later episodic files with an epoch table on one DAC, as for ABF 2. Checked against pyABF inside the epochs and against recorded command monitors over whole sweeps.
- Zeiss LSM files report their objective (name, magnification, NA and immersion), the recording's name and the acquisition date. The date is the instrument's local time, without a time zone.
- Harmony archives (the `hs` folder that "export as archive" writes, from Operetta CLS and Opera Phenix) open as plates, from the `hs` folder or the measurement folder.
- MIRAX slides exported with overlaps removed, which have no camera position table, are read. Their pixels used to exit 6.
- Thermo OMNIC map files (`.map`, OMNIC Atlµs): every map spectrum, the background, the stage position of each spectrum (table `positions`), a grid image when the positions form a grid, and the video image. They used to read as a group file of two spectra, losing the rest. No independent reader exists, so their outputs are reported as unvalidated.
- Shimadzu GCMSsolution `.qgd` GC-MS runs: scans are spectra, equal to the vendor's ANDI and mzXML exports on 22,561 scans of 3 files (GCMS-QP2010 Ultra, GCMS-TQ8040).
- MRM and SRM runs: `info` states the number of transitions (`spectra[].extra.mrm_transitions.count`) and `table` lists them, for Agilent MassHunter, Thermo and Sciex. The counts equal the chromatograms of the depositors' conversions.
- Waters: `spectrum` and `analyze chromatogram` take `--lock-mass MZ|auto` (MCP `lock_mass`) to correct each scan's m/z by the lock-spray reference. Values stay raw by default. On a run MassLynx also corrected, the error drops from 141 ppm to under 1 ppm.
- Agilent MassHunter diode-array spectra (`DAD1.sd`/`.sp`) are read as a trace, one channel per wavelength, equal to an export on 520 spectra.
- Thermo file version 60 is validated, and the analog pressure traces equal the pump-pressure chromatograms of five depositors' conversions.
- Sciex ZenoTOF runs are validated against a depositor's conversion. A `.wiff` without its `.wiff.scan` lists every scan header instead of none.
- mzML export builds the TIC and SRM chromatograms from the spectra when the source stores none.

### Changed

- `spectrum --ms-level L --spectrum I` picks the spectrum at zero-based index `I` among the spectra of MS level `L`, and `--nth` is gone. `--nth` counted from 1 while every other index counts from 0. The first MS/MS scan is now `--ms-level 2 --spectrum 0` (MCP: `ms_level: 2, spectrum: 0`).

### Fixed

- Thermo ITQ 900 scans stored as bare peak lists are read (5,184 scans equal the export). Identical scan events are stored once, so `info` on a 3.7 GB Orbitrap Astral run takes 100 MiB instead of 227 MiB and its TIC 108 MB instead of 1.67 GB.
- Thermo: record-length arithmetic on counts read from the file can no longer overflow on a damaged header.
- Shimadzu MRM events with several precursors and negative polarity are read correctly (47,674 records equal the vendor's mzXML). LCMS-9030 Q-TOF data is named as not read instead of reported as no spectra. `info` gives the vendor peak count of each signal.
- Sciex TOF profiles no longer get an extra empty step beside a stored zero, and an incomplete last cycle is left out as the vendor library does.
- ChemStation diode-array spectra keep their recorded times when the spacing is uneven.
- Waters: a data file that ends early (a damaged download) loses only its last scan, not the whole function.
- mzML export keeps an mzML source's spectrum ids.
- Varian NMR: `extra.scans` is the number of scans the FID holds (`ct`), with `extra.requested_scans` (`nt`) and a note when a run stopped early.
- NMR: processing without stored referencing says that 0 ppm is the spectrometer frequency, and `analyze nmr-peaks --range-ppm` keeps only peaks whose shift lies inside the range.
- Ordinary CSV files with a quoted header were taken for Empower `.arw` exports and called corrupt.
- A file whose first bytes match no format, such as a Lumex text file named `.spa` or a macOS `._` AppleDouble file, now exits 3 (unknown format) instead of 4 (corrupt). This covers the readers with a fixed signature.
- `table` gives the text of text columns (`texts` in JSON, the text itself in the plain output). Zetasizer sample names, measurement kinds and dates used to come out as numeric codes.
- A TRIOS run that recorded no data points reads as its facts with no traces and a warning instead of exiting 6.
- `stats` and `export` over a whole Leica file with FALCON FLIM elements used to exit 6 because the raw photon data is not decoded. They now leave those images out, list them in `images_skipped`, and read the rest.
- A CZI whose attachment entries name a file part the document does not have (an Elyra 7 file saved from one scene) failed `check` and lost its time stamps. The attachments are in the file itself and are read from there.
- `check` on an ND2 file now reports a declared frame that has no chunk. It used to pass the file because the chunk count added up, and `stats` then failed on the missing frame.
- An Imaris file that holds only scene objects (spots, surfaces) and no image exits 6 with a hint instead of being called corrupt.
- `stats` and `export` on Imaris files whose chunks span many Z planes used several times the memory with more threads and were no faster. Planes that share a chunk now go to one worker: 313 MB instead of 832 MB on a 205 MB image with 8 threads.
- `info` printed `NA 0` and `0x` for objectives whose numerical aperture or magnification the file does not state. It now prints only what the file says.
- The CZI `info` note said `export` writes full resolution only. It writes the pyramid levels too.
- Claude Code 2.1.292 showed no OpenReadout MCP tools. With MCP protocol version 2026-07-28 the server left `ttlMs` and `cacheScope` off its list results, and the client rejected the whole tool list. The server now sends them.
- DM3/DM4 files now name the microscope entered for the session (`JEM-ARM200F`) instead of DigitalMicrograph's microscope-control plug-in (`JEOL COM`) when the file has both.
- Electron-microscopy pixel and voxel sizes in `experiment.method.parameters` are in nm. A 0.156 nm pixel read as 0.000156 µm was easy to misread.
- FCS: `check` reported `crc_mismatch` on every file that stores a CRC. Writers store CRC-16/CCITT computed from 0x1D0F, not the bit-reflected form the standard describes; both are now accepted and `crc_ok` names the one that matched (14 files from 8 records).
- FCS: acquisition times for `$DATE` written as `13 Mar 2014` (MoFlo), with Chinese month names (Accuri on a Chinese-language PC), as `dd-mm-yyyy` when the day is above 12 (partec), and for BeamCyte files that keep the date in `$BTIM`.
- FCS: the instrument manufacturer is named for Gallios, Navios, Cytomics, DxFLEX, MoFlo, Accuri, FACSVerse, FACSLyric, Attune, NovoCyte, MACSQuant, Guava, partec and Apogee files, and Sony models written with an `LE-` prefix are recognised.
- FCS: `table --compensate` failed on BD Accuri and Apogee files whose `$SPILLOVER` names parameters by number. A matrix that names parameters a file does not have now exits 6 with a hint instead of 2.
- FlowJo: a non-XML file with the `.wsp` extension (CytExpert) is refused with a plain message instead of binary text.
- Open Ephys GUI 0.6 legacy-format recording directories (with a `.timestamps` file) were refused as an unknown format.
- Renishaw WiRE photoluminescence files: the x axis is wavelength in nm, not an unnamed `x`.
- `stats` on large CZI mosaics is fast again. Reading the plane in strips had made it decode the tiles that cross a strip edge twice. Now each tile is decoded once, and `stats` also accumulates the colour components of a strip in parallel.
- BMG SMART Control workbooks of kinetic runs: each cycle is now its own time point. `check` used to report the whole run as one time point with many values per well.
- `export -o x.ome.zarr` without `--format` wrote an OME-TIFF under the `.zarr` name. The output's extension now picks the format, on the command line and in `openreadout_export`, and an extension that names no format or contradicts `--format` exits 2.
- The viewer's composite showed every channel when `select` gave a range such as `c=0-2`. It now reads `select` with the same parser as `--select`.
- The viewer no longer shows a note about the preview byte budget under large images. `openreadout_preview` says "shown at reduced resolution" when it had to shrink a picture.
- The viewer's channel chips for traces with many channels (a 96-well qPCR run) took over the page. Long names are cut and the chips scroll.
- `openreadout_preview` with `channels` on an image file now has a hint pointing to `select`.
- The README said 96 formats while the binary reads 97. It now points to the format list, whose count comes from the binary.
- The bioconda recipe has the checksum of the 0.2.0 source archive, and the conda-forge recipes build 0.2.0.
- OME-Zarr export of RGB images is several times faster and needs much less memory, which matters most for whole slides. Splitting the colour samples into channels and finding each channel's value range ran on one thread, and the copies grew their buffers as they went. The output is the same byte for byte.

## [0.2.0] - 2026-10-07

The second release. In short:

- **A new command line and MCP surface.** The command line and the MCP server now use the same names, and each command and tool does one thing. Every analysis is its own MCP tool, and `check`, `spectra`, `export --attachment`, `batch summarize` and `search --health`/`--export` are split into separate commands. Many flags are renamed and JSON output is `schema_version` 2. Scripts and prompts written for 0.1 need changes. [Upgrading from 0.1](https://openreadout.github.io/openreadout/getting-started/upgrading.html) maps every old name to its new one.
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
