# Maintaining `openreadout-ephys`

Formats: `heka-patchmaster`, `ced-spike2`, `winwcp`, `open-ephys`, `tdt-tank`, `axona-dacqusb`, `mcs-raw`, `edf`, `brainvision`, `eeglab`. Start with [docs/maintaining.md](../../docs/maintaining.md) for the project-wide process.

## Decode pipeline

Independent readers share the crate; `src/lib.rs` registers each and its `detect`.

- **HEKA PatchMaster** (`src/heka/`). `bundle.rs` parses the `DAT2` bundle header (writer version, up to twelve items: `.dat` samples, `.pul` pulsed tree, `.pgf` stimulus tree, …). `tree.rs` walks the pulsed tree Root → Group → Series → Sweep → Trace (little-endian `eerT` magic, five levels, record sizes from the tree header, a child count after each record). `dataset.rs` turns every series into a trace (sweeps × channels); channels of one series whose sample interval or point count differ become further traces of that series ("series split by sample grid"). Samples are read lazily per sweep from the `.dat` item at each trace record's offset, in its sample format, and multiplied by the record's data scaler.
- **CED Spike2 `.smr`** (`src/spike2/`). `file.rs` reads the 512-byte file header, the 140-byte channel headers and each channel's chain of data blocks (20-byte block header, then items). `dataset.rs` makes every waveform channel a trace (pauses between blocks split sweeps), puts events, markers and text marks in the `events` table and AdcMark/RealMark spike shapes in the `spikes` table. Times count the file's tick. A file starting `S64` (64-bit `.smrx`) goes to `file64.rs` instead: the header stream (first 64 KiB plus extra header blocks), 272-byte channel records, the string table, and per channel a tree of 4 KiB index blocks over 64 KiB data blocks (runs of samples, u64 times); blocks of an older channel generation are skipped. It fills the same `SmrFile`, so the dataset code is shared.
- **WinWCP `.wcp`** (`src/winwcp/`): the 1024-byte `KEY=value` header, then records of `NBA` analysis sectors and `NBD` data sectors of interleaved int16; one trace, records as sweeps, scaled with each record's VMax.
- **TDT tanks and blocks** (`src/tdt/`). `mod.rs` finds the block files (`.tsq`, `.tev`, `.Tbk`, `.sev`) of a block directory, one of its files, or every block of a tank directory. `tsq.rs` reads the 40-byte index records: stream chunks per store and channel (data offsets into `.tev`), snippets, strobe on/off records (the value is in the offset field), start and stop marks. `dataset.rs` makes every readable stream store a trace (blocks are sweeps, channels end at the last chunk wholly inside the `.tev`), snippets the `spikes` table and strobe onsets the `epocs` table (offsets paired by the onset store named in their channel and sort-code bytes).
- **Axona DacqUSB** (`src/axona/`): the `.set` `key value` lines, the raw `.bin` (432-byte packets, 3 samples × 64 channels in a permuted order, `bin_channel_slot`) as one trace of the active tetrodes' channels in µV, and the tetrode files `.1` … as the `spikes` table (big-endian time stamps, int8 waveforms). `.eeg`, `.egf` and `.pos` are listed only.
- **MC_DataTool `.raw`** (`src/mcsraw/`): the text header up to `EOH` and interleaved uint16 frames, scaled with the `El` gain and `ADC zero`; one trace.
- **EDF, EDF+, BDF and BDF+** (`src/edf/`): the 256-byte header and 256 bytes per signal (stored field by field), then data records holding each signal's samples in turn (2-byte or, in BDF, 3-byte little-endian). Signals with the same samples per record are one trace; the `EDF Annotations` / `BDF Annotations` signals are read at open: their time-keeping TALs give each record's start (EDF+D runs of contiguous records become sweeps) and the other TALs the `annotations` table. TAL times are parsed to exact nanoseconds and count from the first record's start.
- **BrainVision** (`src/brainvision/`): the `.vhdr` INI header (identification line, `[Common Infos]`, `[Binary Infos]` or `[ASCII Infos]`, `[Channel Infos]`, `[Coordinates]`, free text from `[Comment]` on) names the data and marker files; the dataset opens from any of the three. Samples are multiplexed frames or vectorized channel runs of INT_16, UINT_16, INT_32 or IEEE_FLOAT_32 (ASCII multiplexed lines are indexed at open), × each channel's resolution. `MARKERBASED` segments that divide the data evenly are sweeps; the `.vmrk` markers are the `markers` table.
- **EEGLAB** (`src/eeglab/`): `mat.rs` parses a level-5 MAT-file (128-byte header, tagged elements padded to 8, `miCOMPRESSED` variables inflated with zlib as a stream) into arrays, keeping only the place of numeric arrays over `MAX_MAT_INLINE` bytes. `mat73.rs` turns a MAT 7.3 (HDF5) file into the same arrays. `mod.rs` reads the `EEG` structure from the variable `EEG` or from one variable per field, finds the samples (a `.fdt` of float32 frames, or the `data` array, read at its place or inflated once), and makes one trace with the epochs as sweeps and `EEG.event` as the `events` table.
- **Open Ephys** (`src/openephys/`). `mod.rs` claims a recording directory (or its `structure.oebin` / a `.continuous` file) and picks the layout. `binary.rs`: `[Record Node N/]experimentE/recordingR/` with `structure.oebin` (JSON), interleaved int16 `continuous.dat`, `sample_numbers.npy` (GUI 0.6+) or integer `timestamps.npy` (older), and `events/<stream>/*.npy`; `npy.rs` is the minimal `.npy` reader. `legacy.rs`: one `.continuous` file per channel (1024-byte text header, 2070-byte records of 1024 big-endian int16 samples), `.events` and `.spikes`, with `_2`, `_3` … suffixes for later experiments. `dataset.rs` joins both into traces per stream.

Versions branch at: the bundle signature and writer version (`bundle.rs`), the tree magic, level count and record sizes (`tree.rs`), the Spike2 file version in the header (`file.rs`), and the Open Ephys GUI version in `structure.oebin` / the legacy header (`binary.rs`, `legacy.rs`). The TDT reader branches on the event type and data format codes of each store (`tsq.rs`) and on block versus tank (`mod.rs`); Axona on the tetrode file header (`num_chans`, `bytes_per_sample`); MC_DataTool on the stream kinds in `Streams`; EDF on the version field (16- or 24-bit samples) and the reserved field (`EDF+C`, `EDF+D`: annotations and record times); BrainVision on `DataFormat`, `DataOrientation` and `BinaryFormat`; EEGLAB on the MAT-file version and compression, one `EEG` variable or one per field, and `.fdt` or embedded samples. The assurance profile (`src/assurance.rs`) turns each of these into a variant feature.

## Invariants and checks

- Every offset and length from a header is bounds-checked against the file size before it is read; a tree, block chain or item running past the end is exit 4 (corrupt) with the byte offset (`Error::corrupt_at`).
- Tree walks are capped (record count, tree size) so a damaged child count cannot allocate without bound; exceeding a cap is exit 6 with a hint that the file may be damaged.
- Refused (exit 6, with a hint naming a way out): PatchMaster files without a bundle (`DAT1`/`DATA`, trees in separate `.pul`/`.pgf` files), PatchMaster Next v2000 bundles (64-bit offsets; no development file), big-endian (PowerPC) trees, trees that do not have five levels, and image-plane requests on any of the readers.
- Refused (exit 6): TDT stores whose samples are in `.sev` files, TDT files without a `.tsq`, Axona files without their `.set`, MC_DataTool exports without a header or with streams other than `El_`.
- TDT and Axona keep times as the file gives them: TDT seconds from the first block's start mark, Axona tetrode time stamps / `timebase`.
- `check` walks every block chain and tree record and reports damaged blocks and trace records without stopping at the first.
- Nothing is resampled: channels on different sample grids stay separate traces, and each stream keeps its own clock.
- EDF: a header whose record count is −1 or larger than the file is read up to the last whole record (a finding says so); annotation signals are read up to `MAX_EDF_ANNOTATION_BYTES`.

## Debugging a new file

1. `openreadout info FILE --view format` and `openreadout info FILE --json`, then read `assurance` (which variant feature is unseen) and `notes`.
2. `openreadout info FILE --view structure` lists the bundle items, tree levels, Spike2 channels and block counts, or the Open Ephys streams; `openreadout info FILE --view full` prints the parsed headers and the vendor tree.
3. `openreadout check FILE` for structural problems (damaged blocks, records past the end).
4. Compare with the oracles the corpus uses: load-heka-python and pyHEKA for PatchMaster, Neo's `Spike2RawIO`, `OpenEphysBinaryRawIO`/`OpenEphysRawIO`, `WinWcpRawIO`, `TdtRawIO`, `AxonaRawIO` and `RawMCSRawIO` for the others, pyedflib and MNE-Python for EDF, MNE-Python and Neo's `BrainVisionRawIO` for BrainVision, SciPy's `loadmat` and MNE-Python for EEGLAB (run as documented in `oracle/`; see each provenance log for licences).
5. Start from the synthetic fixtures in `src/spike2/tests.rs` and `src/openephys/tests.rs` to reproduce a layout without the corpus, and add the file with `cargo xtask variant intake` (`docs/maintaining.md`).

## Fragile spots

- **PatchMaster units and zero offset.** Values are scaled by each trace record's data scaler as PatchMaster displays them, but not zero-subtracted (PatchMaster subtracts `zero_offset` only when that option is on); the assurance block reports this as an assumption.
- **PatchMaster Next.** The v2000 bundle layout and newer tree record sizes have no development file; a newer writer version may change record layouts silently within the five-level tree, which is why unseen writer versions leave the file `unvalidated`.
- **Open Ephys units.** Channel units come from `structure.oebin`; when it leaves them empty, µV (neural) and V (ADC) are assumed per the Open Ephys documentation (`unit_assumed`, reported in assurance).
- **Open Ephys timestamps.** GUI versions before 0.6 store integer sample numbers in `timestamps.npy`; 0.5 has `synchronized_timestamps.npy`. The file name alone decides the meaning, so a new GUI version that reuses a name with another meaning would read wrong times.
- **Directory claims.** The Open Ephys detector claims a directory only when a recording's marker files are within a few levels; an earlier version claimed whole shares (fixed, see the provenance log). Keep `detect` tests for share-like directories.
- **TDT `.Tbk` point counts.** Snippet lengths come from each record's size field; the `.Tbk` point count can be larger (it is in the corpus), and a reader that trusts it reads into the next snippet.
- **TDT cut-short files.** Two corpus blocks index data past the end of their `.tev`; the trace ends at the first chunk that is not wholly in the file, and the oracle hashes the same samples.
- **Axona channel order.** The `.bin` stores channels in blocks of eight that alternate between the upper and lower half of each sample; a wrong slot table gives plausible but wrong traces, so the corpus hash is the only guard.
- **EDF times.** pyedflib and MNE count annotation onsets from the first record's start (the EDF+ subsecond start), and so does the reader; the parse is exact to the nanosecond so that onsets match pyedflib's bit for bit. pyedflib keeps 100 ns, so a TAL with more decimals would differ in the last digits.
- **EDF signal groups.** A trace is a set of signals with equal samples per record; a writer that changes a signal's count would move it to another trace, and the oracle groups the same way.
- **BrainVision float scaling.** The reader multiplies every format by the channel's resolution, as MNE and the pybv writer do; Neo leaves IEEE_FLOAT_32 values unscaled. Files with float data and a resolution other than 1 are where the two would differ.
- **BrainVision file names.** Renamed recordings often name a data file that no longer exists; the reader then takes the header's own stem (as MNE and Neo do), and finds names that differ only in case.
- **EEGLAB MAT-files.** Large `data` arrays are located, not read, when the `.set` is parsed; in a compressed `.set` reading them inflates the whole variable once (capped at `MAX_EEGLAB_INFLATED`). A writer that stores `data` as a cell or sparse array would be refused, not misread. MAT 7.3 files (`mat73.rs`) are walked with `hdf5-pure`: a group whose datasets are all object references without a `MATLAB_class` is taken as a struct array; a new MATLAB writer that marks those fields would read as a scalar struct of cells.
- **Spike2 pauses and ticks.** Sweep boundaries come from gaps between data blocks; a writer that splits blocks without a pause would create spurious sweeps. Times depend on the file's tick (µs per time unit × base).

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `heka-patchmaster` | [format note](../../docs/formats/heka-patchmaster.md), [provenance log](../../docs/provenance/heka-patchmaster.md) | high | vendor docs | 10 / 10 | 6 | - |
| `ced-spike2` | [format note](../../docs/formats/ced-spike2.md), [provenance log](../../docs/provenance/ced-spike2.md) | high | prior art | 24 / 22 | 11 | - |
| `winwcp` | [format note](../../docs/formats/winwcp.md), [provenance log](../../docs/provenance/winwcp.md) | medium | prior art | 4 / 4 | 3 | - |
| `edf` | [format note](../../docs/formats/edf.md), [provenance log](../../docs/provenance/edf.md) | high | open spec | 28 / 28 | 17 | - |
| `brainvision` | [format note](../../docs/formats/brainvision.md), [provenance log](../../docs/provenance/brainvision.md) | high | prior art | 13 / 13 | 8 | - |
| `eeglab` | [format note](../../docs/formats/eeglab.md), [provenance log](../../docs/provenance/eeglab.md) | high | prior art | 13 / 13 | 11 | - |
| `tdt-tank` | [format note](../../docs/formats/tdt-tank.md), [provenance log](../../docs/provenance/tdt-tank.md) | high | prior art | 11 / 11 | 6 | - |
| `axona-dacqusb` | [format note](../../docs/formats/axona-dacqusb.md), [provenance log](../../docs/provenance/axona-dacqusb.md) | low | prior art | 3 / 3 | 1 | - |
| `mcs-raw` | [format note](../../docs/formats/mcs-raw.md), [provenance log](../../docs/provenance/mcs-raw.md) | low | prior art | 1 / 1 | 1 | - |
| `open-ephys` | [format note](../../docs/formats/open-ephys.md), [provenance log](../../docs/provenance/open-ephys.md) | high | vendor docs | 18 / 18 | 4 | - |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the readers in this crate: the variant features of a file and the feature values the development corpus validates |
| [`src/axona/mod.rs`](src/axona/mod.rs) | Axona DacqUSB recordings: a `.set` file of `key value` lines, the raw `.bin` (432-byte packets of 3 samples × 64 int16 channels) and tetrode spike files `.1` … `.32`, all sharing… |
| [`src/axona/tests.rs`](src/axona/tests.rs) | Synthetic-session tests of the Axona reader |
| [`src/brainvision/mod.rs`](src/brainvision/mod.rs) | BrainVision Core Data Format recordings: a text header (`.vhdr`), a text marker file (`.vmrk`) and the samples (`.eeg`, or `.dat`), as BrainVision Recorder and Analyzer, pybv, |
| [`src/brainvision/tests.rs`](src/brainvision/tests.rs) | Synthetic-recording tests of the BrainVision reader |
| [`src/edf/mod.rs`](src/edf/mod.rs) | European Data Format files: EDF, EDF+ (`.edf`), and BioSemi's 24-bit variants BDF and BDF+ (`.bdf`) |
| [`src/edf/tests.rs`](src/edf/tests.rs) | Synthetic-file tests of the EDF / BDF reader |
| [`src/eeglab/mat.rs`](src/eeglab/mat.rs) | MATLAB level-5 MAT-files (the container of EEGLAB `.set` files): a 128-byte header, then data elements, each a tag (type, byte count) and its bytes padded to 8; top-level variables |
| [`src/eeglab/mat73.rs`](src/eeglab/mat73.rs) | MATLAB 7.3 MAT-files: HDF5 files after a 512-byte userblock |
| [`src/eeglab/mod.rs`](src/eeglab/mod.rs) | EEGLAB datasets (`.set`, with the samples inside it or in a `.fdt` next to it): a MATLAB MAT-file holding the `EEG` structure, either as one variable `EEG` or as one variable per |
| [`src/eeglab/tests.rs`](src/eeglab/tests.rs) | Synthetic-dataset tests of the EEGLAB reader, with MAT-files written here |
| [`src/heka/bundle.rs`](src/heka/bundle.rs) | The PatchMaster "bundle" header: a signature, the writing program's version, and up to twelve items, each the start, length and extension of an embedded file (`.dat` samples… |
| [`src/heka/dataset.rs`](src/heka/dataset.rs) | `Dataset` for a PatchMaster bundle: every series is a trace (sweeps × channels); channels of a series whose sample interval or point counts differ form further traces of that… |
| [`src/heka/mod.rs`](src/heka/mod.rs) | HEKA PatchMaster bundle files (`.dat`): a bundle header, the samples, and the pulsed tree (Root → Group → Series → Sweep → Trace) |
| [`src/heka/tree.rs`](src/heka/tree.rs) | HEKA's "Tree" container and the pulsed tree (`.pul`) inside a bundle: Root → Group → Series → Sweep → Trace records, each followed by its child count |
| [`src/lib.rs`](src/lib.rs) | Readers for electrophysiology recordings that have no other home in OpenReadout: HEKA PatchMaster bundles (`heka-patchmaster`), CED Spike2 `.smr`/`.smrx` files (`ced-spike2`), |
| [`src/mcsraw/mod.rs`](src/mcsraw/mod.rs) | Multi Channel Systems MC_DataTool binary exports (`.raw` written with the header option): a text header ending at `EOH`, then interleaved uint16 frames of every stream |
| [`src/mcsraw/tests.rs`](src/mcsraw/tests.rs) | Synthetic-file tests of the MC_DataTool reader |
| [`src/openephys/binary.rs`](src/openephys/binary.rs) | Open Ephys binary format: `[Record Node N/]experimentE/recordingR/` directories, each with a `structure.oebin` (JSON), `continuous/<stream>/continuous.dat` (interleaved int16) with |
| [`src/openephys/dataset.rs`](src/openephys/dataset.rs) | `Dataset` for Open Ephys recordings (binary and legacy formats) |
| [`src/openephys/legacy.rs`](src/openephys/legacy.rs) | The legacy "Open Ephys format": one `.continuous` file per channel (1024-byte text header, then 2070-byte records of 1024 big-endian int16 samples), `.events` and `.spikes` files; |
| [`src/openephys/mod.rs`](src/openephys/mod.rs) | Open Ephys recordings: the binary format (`structure.oebin` + `continuous.dat` + `.npy`) and the legacy one-file-per-channel format (`.continuous`, `.events`, `.spikes`) |
| [`src/openephys/npy.rs`](src/openephys/npy.rs) | Minimal reader of NumPy `.npy` files (the format NumPy documents: magic `\x93NUMPY`, version, header length, a Python-literal dict with `descr`, `fortran_order` and `shape`) |
| [`src/openephys/tests.rs`](src/openephys/tests.rs) | Synthetic-directory tests of the Open Ephys reader |
| [`src/spike2/dataset.rs`](src/spike2/dataset.rs) | `Dataset` for a Spike2 `.smr` or `.smrx`: every waveform channel is a trace (pauses split sweeps); events, markers and text marks form the `events` table, AdcMark/RealMark… |
| [`src/spike2/file.rs`](src/spike2/file.rs) | The 32-bit Spike2 `.smr` layout: a 512-byte file header, 140-byte channel headers, and per channel a chain of data blocks (20-byte header, then items) |
| [`src/spike2/file64.rs`](src/spike2/file64.rs) | The 64-bit Spike2 `.smrx` layout, derived from the files and the Spike2 software's own exports of them (no CED documentation or library; `docs/provenance/ced-spike2.md`): a header… |
| [`src/spike2/mod.rs`](src/spike2/mod.rs) | CED Spike2 data files: 32-bit `.smr` and 64-bit `.smrx` |
| [`src/spike2/tests.rs`](src/spike2/tests.rs) | Synthetic-file tests of the Spike2 reader |
| [`src/tdt/dataset.rs`](src/tdt/dataset.rs) | `Dataset` for a TDT block or tank: one trace per stream store (channels are the store's channels; blocks are sweeps), a `spikes` table of snippets and an `epocs` table of strobe |
| [`src/tdt/mod.rs`](src/tdt/mod.rs) | Tucker-Davis Technologies (TDT) tanks and blocks: a block is a directory of files sharing a stem (`.tsq` index, `.tev` data, `.Tbk` store headers, `.Tdx`, `.tin`, `.tnt`… |
| [`src/tdt/tests.rs`](src/tdt/tests.rs) | Synthetic-block tests of the TDT reader |
| [`src/tdt/tsq.rs`](src/tdt/tsq.rs) | The `.tsq` index of a TDT block: a run of 40-byte records (size in 4-byte words, event type, four-character store name, channel, sort code, float64 time stamp in seconds since… |
| [`src/winwcp/mod.rs`](src/winwcp/mod.rs) | WinWCP `.wcp` files (Strathclyde Electrophysiology Software): a 1024-byte `KEY=value` text header, then `NR` records of an analysis header and interleaved int16 samples |
| [`src/winwcp/tests.rs`](src/winwcp/tests.rs) | Synthetic-file tests of the WinWCP reader |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature format_version `v`
- feature layout `format!("{} channels", t.channels.len())`
- feature format_version `v, &all`
- feature sample_layout `&c.dtype`
- feature layout `"discontinuous records"`
- feature layout `"several sample rates"`
- feature record `"annotations"`
- feature sample_layout `f`
- feature layout `f`
- feature layout `"marker-based segments"`
- feature record `"markers"`
- feature codec `"zlib (MAT 7)", &all`
- feature sample_layout `format!("{s} {dtype}")`
- feature layout `"epochs"`
- feature record `"events"`
- feature layout `"tank", &all`
- feature layout `"block", &all`
- feature record `"stream"`
- feature record `"snippet"`
- feature record `"strobe"`
- feature record `"strobe-off"`
- feature layout `"raw packets"`
- feature record `"tetrode spikes"`
- feature writer_version `v`
- feature record `k`
- feature layout `"binary"`
- feature format_version `format!("GUI {p}")`
- feature layout `"legacy"`
- feature format_version `heka_version(v)`
- feature layout `format!("pulsed tree {v}")`
- feature sample_layout `d`
- feature record `what`
- feature writer_version `v` (descriptive)
- feature writer `w` (descriptive)
- feature layout `"several sweeps"` (descriptive)
- feature acquisition `m` (descriptive)
- feature layout `"series split by sample grid"` (descriptive)
- undecoded "cut-short files"
- undecoded "unread stores"
- undecoded "scalar records"
- undecoded "data past the end of the .tev"
- undecoded "EEG, position and other session files"
- undecoded "damaged blocks"
- undecoded "irregular channel files"
- undecoded "binary event streams"
- undecoded "spike groups"
- undecoded "missing streams"
- undecoded "unvalidated traces"
- undecoded "damaged trace records"
- assumed "channel units"
- assumed "zero subtraction"
- calibration "ADC counts to channel units"
- calibration "digital to physical values"
- calibration "stored values to channel units"
- calibration "ADC counts to µV"
- calibration "ADC counts to amperes and volts"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `axona-dacqusb` | field | `experiment.acquisition.started_at` | descriptive | 3 | 3 | `gin-axona-multi-modal`, `gin-axona-raw`, `gin-axona-unit-spikes` |
| `axona-dacqusb` | layout | `raw packets` | traces | 2 | 2 | `gin-axona-multi-modal`, `gin-axona-raw` |
| `axona-dacqusb` | record | `tetrode spikes` | tables | 2 | 2 | `gin-axona-multi-modal`, `gin-axona-unit-spikes` |
| `axona-dacqusb` | writer_version | `1.2.2.16` | descriptive | 3 | 3 | `gin-axona-multi-modal`, `gin-axona-raw`, `gin-axona-unit-spikes` |
| `brainvision` | field | `experiment.acquisition.started_at` | descriptive | 8 | 8 | `gin-brainvision-1`, `gin-brainvision-2`, `gin-brainvision-3-float32` |
| `brainvision` | format_version | `1` | metadata | 9 | 9 | `gin-brainvision-1`, `gin-brainvision-2`, `gin-brainvision-4-float32` |
| `brainvision` | format_version | `2` | metadata | 4 | 4 | `gin-brainvision-3-float32`, `gin-brainvision-3-int16`, `gin-brainvision-3-int32` |
| `brainvision` | layout | `MULTIPLEXED` | traces | 12 | 12 | `gin-brainvision-1`, `gin-brainvision-2`, `gin-brainvision-3-float32` |
| `brainvision` | layout | `VECTORIZED` | traces | 1 | 1 | `zenodo-8407716-subject-19-exp-postsleep` |
| `brainvision` | layout | `marker-based segments` | traces | 4 | 4 | `gin-brainvision-2`, `gin-brainvision-3-float32`, `gin-brainvision-3-int16` |
| `brainvision` | record | `markers` | tables | 11 | 11 | `gin-brainvision-1`, `gin-brainvision-2`, `gin-brainvision-3-float32` |
| `brainvision` | sample_layout | `IEEE_FLOAT_32` | traces | 9 | 9 | `gin-brainvision-1`, `gin-brainvision-2`, `gin-brainvision-3-float32` |
| `brainvision` | sample_layout | `INT_16` | traces | 3 | 3 | `gin-brainvision-3-int16`, `mnepython-bv-test`, `openneuro-ds005530-sub-03112021304-task-sleep-run-02` |
| `brainvision` | sample_layout | `INT_32` | traces | 1 | 1 | `gin-brainvision-3-int32` |
| `brainvision` | writer | `BrainVision Analyzer` | descriptive | 6 | 6 | `gin-brainvision-1`, `gin-brainvision-2`, `gin-brainvision-3-float32` |
| `brainvision` | writer | `BrainVision Recorder` | descriptive | 2 | 2 | `mnepython-bv-test`, `openneuro-ds005530-sub-03112021304-task-sleep-run-02` |
| `brainvision` | writer | `FieldTrip` | descriptive | 2 | 2 | `gin-brainvision-4-float32`, `openneuro-ds003190-sub-017-ses-03-task-cnos-run-4` |
| `brainvision` | writer | `pybv` | descriptive | 3 | 3 | `openneuro-ds005262-sub-10-ses-8-task-innerspeech`, `openneuro-ds007541-sub-01-ses-02-task-verification-run-05`, `openneuro-ds007720-sub-p2-ses-5-task-rsvpcopyphrase-run-6` |
| `ced-spike2` | field | `experiment.acquisition.started_at` | descriptive | 21 | 23 | `figshare25112837-fig2-aud-smrx`, `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx` |
| `ced-spike2` | format_version | `3` | metadata, tables, traces | 2 | 2 | `spike2-file-spike2-3-smr`, `zenodo4985334-aa-mvc-smr` |
| `ced-spike2` | format_version | `4` | metadata, tables, traces | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | format_version | `5` | metadata, tables, traces | 6 | 6 | `figshare695021-smr-n80108repre`, `figshare695021-smr-n80117li`, `figshare695021-smr-n90120re` |
| `ced-spike2` | format_version | `6` | metadata, tables, traces | 4 | 4 | `figshare639946-smr-day0-dark`, `figshare639946-smr-day1-light`, `zenodo10624872-21022013-smr` |
| `ced-spike2` | format_version | `7` | metadata, tables, traces | 4 | 4 | `figshare26177569-sssort-singleb-asym03-smr`, `zenodo14706945-smr-07152022-n1`, `zenodo15783208-cpp600-c7e-da-smr` |
| `ced-spike2` | format_version | `9` | metadata, tables, traces | 1 | 1 | `spike2-two-mice-bigfile-test000-smr` |
| `ced-spike2` | format_version | `smrx 0.1` | metadata, tables, traces | 1 | 1 | `figshare25112837-fig2-aud-smrx` |
| `ced-spike2` | format_version | `smrx 1.1` | metadata, tables, traces | 3 | 5 | `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx`, `figshare26177569-sssort-singleb-smrx` |
| `ced-spike2` | record | `adc` | traces | 21 | 23 | `figshare25112837-fig2-aud-smrx`, `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx` |
| `ced-spike2` | record | `adc-mark` | tables | 3 | 3 | `spike2-file-spike2-1-smr`, `spike2-file-spike2-2-smr`, `spike2-multi-sampling-smr` |
| `ced-spike2` | record | `event-falling` | tables | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | record | `event-level` | tables | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | record | `event-rising` | tables | 6 | 6 | `figshare25112837-fig2-aud-smrx`, `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx` |
| `ced-spike2` | record | `marker` | tables | 6 | 7 | `figshare639946-smr-day0-dark`, `figshare695021-smr-n80108repre`, `figshare695021-smr-n80117li` |
| `ced-spike2` | record | `text-mark` | tables | 5 | 6 | `figshare695021-smr-n90120re`, `spike2-130322-1ly-smr`, `spike2-file-spike2-2-smr` |
| `ced-spike2` | writer_version | `00000000` | descriptive | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | writer_version | `S2050123` | descriptive | 1 | 1 | `spike2-file-spike2-3-smr` |
| `ced-spike2` | writer_version | `S2050210` | descriptive | 1 | 1 | `zenodo4437568-fig1-rec-f-control-smr` |
| `ced-spike2` | writer_version | `S2051454` | descriptive | 3 | 3 | `figshare695021-smr-n80108repre`, `figshare695021-smr-n80117li`, `figshare695021-smr-n90120re` |
| `ced-spike2` | writer_version | `S2061895` | descriptive | 1 | 1 | `spike2-130322-1ly-smr` |
| `ced-spike2` | writer_version | `S2071033` | descriptive | 1 | 1 | `spike2-multi-sampling-smr` |
| `ced-spike2` | writer_version | `S2071345` | descriptive | 2 | 2 | `figshare639946-smr-day0-dark`, `figshare639946-smr-day1-light` |
| `ced-spike2` | writer_version | `S2071431` | descriptive | 1 | 1 | `zenodo10624872-21022013-smr` |
| `ced-spike2` | writer_version | `S2071635` | descriptive | 1 | 1 | `spike2-two-mice-bigfile-test000-smr` |
| `ced-spike2` | writer_version | `S2072307` | descriptive | 1 | 1 | `zenodo4985334-aa-mvc-smr` |
| `ced-spike2` | writer_version | `S2083226` | descriptive | 1 | 1 | `figshare25112837-fig2-aud-smrx` |
| `ced-spike2` | writer_version | `S2083331` | descriptive | 1 | 1 | `zenodo15783208-cpp600-c7e-da-smr` |
| `ced-spike2` | writer_version | `S2090130` | descriptive | 2 | 2 | `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singleb-asym03-smr` |
| `ced-spike2` | writer_version | `S2090302` | descriptive | 1 | 2 | `zenodo14706945-smr-07152022-n1` |
| `ced-spike2` | writer_version | `S2091349` | descriptive | 0 | 1 |  |
| `ced-spike2` | writer_version | `S2103724` | descriptive | 1 | 1 | `zenodo20750122-sub3-transpai-smr` |
| `edf` | format_version | `BDF` | metadata, tables, traces | 2 | 2 | `mnepython-test-bdf`, `openneuro-ds005648-sub-03-task-targets` |
| `edf` | format_version | `BDF+C` | metadata, tables, traces | 3 | 3 | `gin-edf-event-markers-bdf`, `openneuro-ds005857-sub-ltp505-ses-2-task-ltpdelayrepfrreadonly`, `pyedflib-test-generator-bdf` |
| `edf` | format_version | `EDF` | metadata, tables, traces | 5 | 5 | `edfio-short-psg`, `gin-edf-cross-kind-offsets-corrupt-header`, `mnepython-test-edf-stim-channel` |
| `edf` | format_version | `EDF+C` | metadata, tables, traces | 16 | 16 | `gin-edf-autoscaled-offsets`, `gin-edf-edf-c`, `gin-edf-electrode-and-analog-data` |
| `edf` | format_version | `EDF+D` | metadata, tables, traces | 2 | 2 | `figshare-28485251-1`, `zenodo-4688758-s65` |
| `edf` | layout | `several sample rates` | traces | 4 | 4 | `edfio-short-psg`, `gin-edf-same-unit-offsets-multirate`, `pyedflib-test-generator-bdf` |
| `edf` | record | `annotations` | tables | 13 | 15 | `gin-edf-electrode-and-analog-data`, `gin-edf-event-edge-cases`, `gin-edf-event-markers-bdf` |
| `edf` | sample_layout | `int16` | traces | 22 | 22 | `edfio-short-psg`, `figshare-28485251-1`, `gin-edf-autoscaled-offsets` |
| `edf` | sample_layout | `int24` | traces | 5 | 5 | `gin-edf-event-markers-bdf`, `mnepython-test-bdf`, `openneuro-ds005648-sub-03-task-targets` |
| `eeglab` | format_version | `MAT 5.0` | metadata, tables, traces | 11 | 11 | `figshare-14459286-311-epochs-before`, `gin-eeglab-continuous-one-file`, `gin-eeglab-continuous-two-files` |
| `eeglab` | format_version | `MAT 7.3` | metadata, tables, traces | 2 | 2 | `openneuro-ds005048-sub-02-task-40hzauditoryentrainment`, `openneuro-ds005628-sub-1-task-edzna-run-1` |
| `eeglab` | layout | `epochs` | traces | 4 | 4 | `figshare-14459286-311-epochs-before`, `gin-eeglab-epoched-ica`, `openneuro-ds006923-sub-1009sg-task-restingstate-acq-epochs` |
| `eeglab` | record | `events` | tables | 10 | 10 | `figshare-14459286-311-epochs-before`, `gin-eeglab-continuous-one-file`, `gin-eeglab-continuous-two-files` |
| `eeglab` | sample_layout | `embedded float32` | traces | 6 | 6 | `gin-eeglab-continuous-one-file`, `openneuro-ds004362-sub-001-task-motion-run-1`, `openneuro-ds005628-sub-1-task-edzna-run-1` |
| `eeglab` | sample_layout | `embedded float64` | traces | 1 | 1 | `openneuro-ds003800-sub-001-task-rest` |
| `eeglab` | sample_layout | `fdt float32` | traces | 6 | 6 | `figshare-14459286-311-epochs-before`, `gin-eeglab-continuous-two-files`, `gin-eeglab-epoched-ica` |
| `eeglab` | writer | `MATLAB MACA64` | descriptive | 2 | 2 | `openneuro-ds006394-sub-01-task-sid`, `openneuro-ds007406-sub-002-task-extremeversustraditionalvideos` |
| `eeglab` | writer | `MATLAB PCWIN64` | descriptive | 8 | 8 | `figshare-14459286-311-epochs-before`, `openneuro-ds003800-sub-001-task-rest`, `openneuro-ds004362-sub-001-task-motion-run-1` |
| `eeglab` | writer | `SciPy` | descriptive | 3 | 3 | `gin-eeglab-continuous-one-file`, `gin-eeglab-continuous-two-files`, `gin-eeglab-epoched-ica` |
| `eeglab` | writer_version | `14.1.2` | descriptive | 1 | 1 | `figshare-14459286-311-epochs-before` |
| `eeglab` | writer_version | `2020.0` | descriptive | 1 | 1 | `openneuro-ds003800-sub-001-task-rest` |
| `eeglab` | writer_version | `2022.0` | descriptive | 1 | 1 | `openneuro-ds006923-sub-1009sg-task-restingstate-acq-epochs` |
| `eeglab` | writer_version | `2022.1` | descriptive | 1 | 1 | `openneuro-ds004362-sub-001-task-motion-run-1` |
| `eeglab` | writer_version | `2023.1` | descriptive | 1 | 1 | `openneuro-ds005048-sub-02-task-40hzauditoryentrainment` |
| `eeglab` | writer_version | `2024.0` | descriptive | 1 | 1 | `openneuro-ds006394-sub-01-task-sid` |
| `eeglab` | writer_version | `2024.2` | descriptive | 1 | 1 | `openneuro-ds005628-sub-1-task-edzna-run-1` |
| `eeglab` | writer_version | `2025.0.0` | descriptive | 2 | 2 | `openneuro-ds006260-sub-mc02-ses-1-task-smartickdataset-run-2`, `openneuro-ds007006-sub-001-task-ivan` |
| `eeglab` | writer_version | `2025.1.0` | descriptive | 1 | 1 | `openneuro-ds007406-sub-002-task-extremeversustraditionalvideos` |
| `eeglab` | writer_version | `synthetic (not written by EEGLAB)` | descriptive | 2 | 2 | `gin-eeglab-continuous-one-file`, `gin-eeglab-continuous-two-files` |
| `heka-patchmaster` | acquisition | `current-clamp` | descriptive | 7 | 7 | `gin-ephy-testing-data-heka-generation-v2x65-two-series-mixed-modes`, `gin-ephy-testing-data-heka-generation-v2x90-2-two-amplifier-channels`, `gin-ephy-testing-data-heka-generation-v2x90-3-eight-headstages` |

… 44 more values: the generated table in `src/assurance.rs` has all of them.

### Tests, fixtures, fuzz targets, snapshots

- integration tests: none (unit tests in `src/`)
- committed fixtures: 3 files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)
- fuzz targets (`fuzz/fuzz_targets/`): `whole_axona`, `whole_brainvision`, `whole_edf`, `whole_eeglab`, `whole_heka`, `whole_mcs_raw`, `whole_openephys`, `whole_spike2`, `whole_tdt`, `whole_winwcp`
- corpus inputs by tier: full 1, heldout 5, smoke 64, standard 60
- golden snapshots: [`corpus/snapshots/heka-patchmaster.jsonl`](../../corpus/snapshots/heka-patchmaster.jsonl), [`corpus/snapshots/ced-spike2.jsonl`](../../corpus/snapshots/ced-spike2.jsonl), [`corpus/snapshots/winwcp.jsonl`](../../corpus/snapshots/winwcp.jsonl), [`corpus/snapshots/edf.jsonl`](../../corpus/snapshots/edf.jsonl), [`corpus/snapshots/brainvision.jsonl`](../../corpus/snapshots/brainvision.jsonl), [`corpus/snapshots/eeglab.jsonl`](../../corpus/snapshots/eeglab.jsonl), [`corpus/snapshots/tdt-tank.jsonl`](../../corpus/snapshots/tdt-tank.jsonl), [`corpus/snapshots/axona-dacqusb.jsonl`](../../corpus/snapshots/axona-dacqusb.jsonl), [`corpus/snapshots/mcs-raw.jsonl`](../../corpus/snapshots/mcs-raw.jsonl), [`corpus/snapshots/open-ephys.jsonl`](../../corpus/snapshots/open-ephys.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
