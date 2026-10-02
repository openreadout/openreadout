# Maintaining `openreadout-ephys`

Formats: `heka-patchmaster`, `ced-spike2`, `open-ephys`. Start with [docs/maintaining.md](../../docs/maintaining.md) for the project-wide process.

## Decode pipeline

Three independent readers share the crate; `src/lib.rs` registers each and its `detect`.

- **HEKA PatchMaster** (`src/heka/`). `bundle.rs` parses the `DAT2` bundle header (writer version, up to twelve items: `.dat` samples, `.pul` pulsed tree, `.pgf` stimulus tree, …). `tree.rs` walks the pulsed tree Root → Group → Series → Sweep → Trace (little-endian `eerT` magic, five levels, record sizes from the tree header, a child count after each record). `dataset.rs` turns every series into a trace (sweeps × channels); channels of one series whose sample interval or point count differ become further traces of that series ("series split by sample grid"). Samples are read lazily per sweep from the `.dat` item at each trace record's offset, in its sample format, and multiplied by the record's data scaler.
- **CED Spike2 `.smr`** (`src/spike2/`). `file.rs` reads the 512-byte file header, the 140-byte channel headers and each channel's chain of data blocks (20-byte block header, then items). `dataset.rs` makes every waveform channel a trace (pauses between blocks split sweeps), puts events, markers and text marks in the `events` table and AdcMark/RealMark spike shapes in the `spikes` table. Times count the file's tick. A file starting `S64` (64-bit `.smrx`) goes to `file64.rs` instead: the header stream (first 64 KiB plus extra header blocks), 272-byte channel records, the string table, and per channel a tree of 4 KiB index blocks over 64 KiB data blocks (runs of samples, u64 times); blocks of an older channel generation are skipped. It fills the same `SmrFile`, so the dataset code is shared.
- **WinWCP `.wcp`** (`src/winwcp/`): the 1024-byte `KEY=value` header, then records of `NBA` analysis sectors and `NBD` data sectors of interleaved int16; one trace, records as sweeps, scaled with each record's VMax.
- **Open Ephys** (`src/openephys/`). `mod.rs` claims a recording directory (or its `structure.oebin` / a `.continuous` file) and picks the layout. `binary.rs`: `[Record Node N/]experimentE/recordingR/` with `structure.oebin` (JSON), interleaved int16 `continuous.dat`, `sample_numbers.npy` (GUI 0.6+) or integer `timestamps.npy` (older), and `events/<stream>/*.npy`; `npy.rs` is the minimal `.npy` reader. `legacy.rs`: one `.continuous` file per channel (1024-byte text header, 2070-byte records of 1024 big-endian int16 samples), `.events` and `.spikes`, with `_2`, `_3` … suffixes for later experiments. `dataset.rs` joins both into traces per stream.

Versions branch at: the bundle signature and writer version (`bundle.rs`), the tree magic, level count and record sizes (`tree.rs`), the Spike2 file version in the header (`file.rs`), and the Open Ephys GUI version in `structure.oebin` / the legacy header (`binary.rs`, `legacy.rs`). The assurance profile (`src/assurance.rs`) turns each of these into a variant feature.

## Invariants and checks

- Every offset and length from a header is bounds-checked against the file size before it is read; a tree, block chain or item running past the end is exit 4 (corrupt) with the byte offset (`Error::corrupt_at`).
- Tree walks are capped (record count, tree size) so a damaged child count cannot allocate without bound; exceeding a cap is exit 6 with a hint that the file may be damaged.
- Refused (exit 6, with a hint naming a way out): PatchMaster files without a bundle (`DAT1`/`DATA`, trees in separate `.pul`/`.pgf` files), PatchMaster Next v2000 bundles (64-bit offsets; no development file), big-endian (PowerPC) trees, trees that do not have five levels, and image-plane requests on any of the readers.
- `check` walks every block chain and tree record and reports damaged blocks and trace records without stopping at the first.
- Nothing is resampled: channels on different sample grids stay separate traces, and each stream keeps its own clock.

## Debugging a new file

1. `openreadout info FILE --view format` and `openreadout info FILE --json`, then read `assurance` (which variant feature is unseen) and `notes`.
2. `openreadout info FILE --view structure` lists the bundle items, tree levels, Spike2 channels and block counts, or the Open Ephys streams; `openreadout info FILE --view full` prints the parsed headers and the vendor tree.
3. `openreadout check FILE` for structural problems (damaged blocks, records past the end).
4. Compare with the oracles the corpus uses: load-heka-python and pyHEKA for PatchMaster, Neo's `Spike2RawIO` and `OpenEphysBinaryRawIO`/`OpenEphysRawIO` for the others (run as documented in `oracle/`; see each provenance log for licences).
5. Start from the synthetic fixtures in `src/spike2/tests.rs` and `src/openephys/tests.rs` to reproduce a layout without the corpus, and add the file with `cargo xtask variant intake` (`docs/maintaining.md`).

## Fragile spots

- **PatchMaster units and zero offset.** Values are scaled by each trace record's data scaler as PatchMaster displays them, but not zero-subtracted (PatchMaster subtracts `zero_offset` only when that option is on); the assurance block reports this as an assumption.
- **PatchMaster Next.** The v2000 bundle layout and newer tree record sizes have no development file; a newer writer version may change record layouts silently within the five-level tree, which is why unseen writer versions leave the file `unvalidated`.
- **Open Ephys units.** Channel units come from `structure.oebin`; when it leaves them empty, µV (neural) and V (ADC) are assumed per the Open Ephys documentation (`unit_assumed`, reported in assurance).
- **Open Ephys timestamps.** GUI versions before 0.6 store integer sample numbers in `timestamps.npy`; 0.5 has `synchronized_timestamps.npy`. The file name alone decides the meaning, so a new GUI version that reuses a name with another meaning would read wrong times.
- **Directory claims.** The Open Ephys detector claims a directory only when a recording's marker files are within a few levels; an earlier version claimed whole shares (fixed, see the provenance log). Keep `detect` tests for share-like directories.
- **Spike2 pauses and ticks.** Sweep boundaries come from gaps between data blocks; a writer that splits blocks without a pause would create spurious sweeps. Times depend on the file's tick (µs per time unit × base).

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `heka-patchmaster` | [format note](../../docs/formats/heka-patchmaster.md), [provenance log](../../docs/provenance/heka-patchmaster.md) | high | vendor docs | 6 / 6 | 5 | - |
| `ced-spike2` | [format note](../../docs/formats/ced-spike2.md), [provenance log](../../docs/provenance/ced-spike2.md) | high | prior art | 17 / 16 | 8 | - |
| `winwcp` | [format note](../../docs/formats/winwcp.md), [provenance log](../../docs/provenance/winwcp.md) | low | prior art | 2 / 2 | 1 | - |
| `open-ephys` | [format note](../../docs/formats/open-ephys.md), [provenance log](../../docs/provenance/open-ephys.md) | high | vendor docs | 17 / 17 | 3 | - |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the readers in this crate: the variant features of a file and the feature values the development corpus validates |
| [`src/heka/bundle.rs`](src/heka/bundle.rs) | The PatchMaster "bundle" header: a signature, the writing program's version, and up to twelve items, each the start, length and extension of an embedded file (`.dat` samples… |
| [`src/heka/dataset.rs`](src/heka/dataset.rs) | `Dataset` for a PatchMaster bundle: every series is a trace (sweeps × channels); channels of a series whose sample interval or point counts differ form further traces of that… |
| [`src/heka/mod.rs`](src/heka/mod.rs) | HEKA PatchMaster bundle files (`.dat`): a bundle header, the samples, and the pulsed tree (Root → Group → Series → Sweep → Trace) |
| [`src/heka/tree.rs`](src/heka/tree.rs) | HEKA's "Tree" container and the pulsed tree (`.pul`) inside a bundle: Root → Group → Series → Sweep → Trace records, each followed by its child count |
| [`src/lib.rs`](src/lib.rs) | Readers for electrophysiology recordings that have no other home in OpenReadout: HEKA PatchMaster bundles (`heka-patchmaster`), CED Spike2 `.smr`/`.smrx` files (`ced-spike2`), |
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
| [`src/winwcp/mod.rs`](src/winwcp/mod.rs) | WinWCP `.wcp` files (Strathclyde Electrophysiology Software): a 1024-byte `KEY=value` text header, then `NR` records of an analysis header and interleaved int16 samples |
| [`src/winwcp/tests.rs`](src/winwcp/tests.rs) | Synthetic-file tests of the WinWCP reader |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature format_version `v`
- feature layout `format!("{} channels", t.channels.len())`
- feature record `k`
- feature layout `"binary"`
- feature format_version `format!("GUI {p}")`
- feature layout `"legacy"`
- feature format_version `heka_version(v)`
- feature layout `format!("pulsed tree {v}")`
- feature sample_layout `d`
- feature record `what`
- feature writer_version `v` (descriptive)
- feature layout `"several sweeps"` (descriptive)
- feature acquisition `m` (descriptive)
- feature layout `"series split by sample grid"` (descriptive)
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
- calibration "ADC counts to amperes and volts"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `ced-spike2` | field | `experiment.acquisition.started_at` | descriptive | 15 | 16 | `figshare25112837-fig2-aud-smrx`, `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx` |
| `ced-spike2` | format_version | `3` | metadata, tables, traces | 2 | 2 | `spike2-file-spike2-3-smr`, `zenodo4985334-aa-mvc-smr` |
| `ced-spike2` | format_version | `4` | metadata, tables, traces | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | format_version | `5` | metadata, tables, traces | 3 | 3 | `spike2-130322-1ly-smr`, `spike2-file-spike2-2-smr`, `spike2-multi-sampling-smr` |
| `ced-spike2` | format_version | `6` | metadata, tables, traces | 2 | 2 | `zenodo10624872-21022013-smr`, `zenodo4437568-fig1-rec-f-control-smr` |
| `ced-spike2` | format_version | `7` | metadata, tables, traces | 3 | 3 | `figshare26177569-sssort-singleb-asym03-smr`, `zenodo15783208-cpp600-c7e-da-smr`, `zenodo20750122-sub3-transpai-smr` |
| `ced-spike2` | format_version | `9` | metadata, tables, traces | 1 | 1 | `spike2-two-mice-bigfile-test000-smr` |
| `ced-spike2` | format_version | `smrx 0.1` | metadata, tables, traces | 1 | 1 | `figshare25112837-fig2-aud-smrx` |
| `ced-spike2` | format_version | `smrx 1.1` | metadata, tables, traces | 3 | 4 | `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx`, `figshare26177569-sssort-singleb-smrx` |
| `ced-spike2` | record | `adc` | traces | 15 | 16 | `figshare25112837-fig2-aud-smrx`, `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx` |
| `ced-spike2` | record | `adc-mark` | tables | 3 | 3 | `spike2-file-spike2-1-smr`, `spike2-file-spike2-2-smr`, `spike2-multi-sampling-smr` |
| `ced-spike2` | record | `event-falling` | tables | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | record | `event-level` | tables | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | record | `event-rising` | tables | 6 | 6 | `figshare25112837-fig2-aud-smrx`, `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singlea-smrx` |
| `ced-spike2` | record | `marker` | tables | 2 | 2 | `spike2-file-spike2-1-smr`, `spike2-multi-sampling-smr` |
| `ced-spike2` | record | `text-mark` | tables | 3 | 3 | `spike2-130322-1ly-smr`, `spike2-file-spike2-2-smr`, `spike2-multi-sampling-smr` |
| `ced-spike2` | writer_version | `00000000` | descriptive | 1 | 1 | `spike2-file-spike2-1-smr` |
| `ced-spike2` | writer_version | `S2050123` | descriptive | 1 | 1 | `spike2-file-spike2-3-smr` |
| `ced-spike2` | writer_version | `S2050210` | descriptive | 1 | 1 | `zenodo4437568-fig1-rec-f-control-smr` |
| `ced-spike2` | writer_version | `S2061895` | descriptive | 1 | 1 | `spike2-130322-1ly-smr` |
| `ced-spike2` | writer_version | `S2071033` | descriptive | 1 | 1 | `spike2-multi-sampling-smr` |
| `ced-spike2` | writer_version | `S2071431` | descriptive | 1 | 1 | `zenodo10624872-21022013-smr` |
| `ced-spike2` | writer_version | `S2071635` | descriptive | 1 | 1 | `spike2-two-mice-bigfile-test000-smr` |
| `ced-spike2` | writer_version | `S2072307` | descriptive | 1 | 1 | `zenodo4985334-aa-mvc-smr` |
| `ced-spike2` | writer_version | `S2083226` | descriptive | 1 | 1 | `figshare25112837-fig2-aud-smrx` |
| `ced-spike2` | writer_version | `S2083331` | descriptive | 1 | 1 | `zenodo15783208-cpp600-c7e-da-smr` |
| `ced-spike2` | writer_version | `S2090130` | descriptive | 2 | 2 | `figshare26177569-sssort-doubleab-smrx`, `figshare26177569-sssort-singleb-asym03-smr` |
| `ced-spike2` | writer_version | `S2091349` | descriptive | 0 | 1 |  |
| `ced-spike2` | writer_version | `S2103724` | descriptive | 1 | 1 | `zenodo20750122-sub3-transpai-smr` |
| `heka-patchmaster` | acquisition | `current-clamp` | descriptive | 3 | 3 | `hekareader-180514s1c1r1`, `zenodo3827171-w2019-07-08b`, `zenodo4311847-feb0821c` |
| `heka-patchmaster` | acquisition | `no-mode` | descriptive | 1 | 1 | `zenodo4311847-feb0821c` |
| `heka-patchmaster` | acquisition | `whole-cell` | descriptive | 4 | 4 | `zenodo3827171-w2019-07-08b`, `zenodo4992914-04-11-12-hek-prestin`, `zenodo7530512-2018-12-21c4` |
| `heka-patchmaster` | format_version | `2.11` | metadata, traces | 1 | 1 | `zenodo4992914-04-11-12-hek-prestin` |
| `heka-patchmaster` | format_version | `2x60` | metadata, traces | 2 | 2 | `zenodo7530512-2018-12-21c4`, `zenodo7530512-2019-10-02c1` |
| `heka-patchmaster` | format_version | `2x65` | metadata, traces | 1 | 1 | `zenodo4311847-feb0821c` |
| `heka-patchmaster` | format_version | `2x90.2` | metadata, traces | 1 | 1 | `zenodo3827171-w2019-07-08b` |
| `heka-patchmaster` | format_version | `2x90.3` | metadata, traces | 1 | 1 | `hekareader-180514s1c1r1` |
| `heka-patchmaster` | layout | `pulsed tree 1000` | metadata, traces | 1 | 1 | `hekareader-180514s1c1r1` |
| `heka-patchmaster` | layout | `pulsed tree 9` | metadata, traces | 5 | 5 | `zenodo3827171-w2019-07-08b`, `zenodo4311847-feb0821c`, `zenodo4992914-04-11-12-hek-prestin` |
| `heka-patchmaster` | layout | `series split by sample grid` | descriptive | 1 | 1 | `zenodo4992914-04-11-12-hek-prestin` |
| `heka-patchmaster` | record | `virtual traces` | traces | 1 | 1 | `zenodo4992914-04-11-12-hek-prestin` |
| `heka-patchmaster` | sample_layout | `float32` | traces | 1 | 1 | `zenodo4992914-04-11-12-hek-prestin` |
| `heka-patchmaster` | sample_layout | `int16` | traces | 6 | 6 | `hekareader-180514s1c1r1`, `zenodo3827171-w2019-07-08b`, `zenodo4311847-feb0821c` |
| `open-ephys` | format_version | `GUI 0.4` | metadata, tables, traces | 3 | 3 | `oe-bin-neural-and-non-neural-data-mixed`, `oe-bin-v0-4-4-1-with-spikes`, `oe-bin-v0-4-4-1-with-video-tracking` |
| `open-ephys` | format_version | `GUI 0.5` | metadata, tables, traces | 2 | 2 | `oe-bin-v0-5-3-two-neuropixels-stream`, `oe-bin-v0-5-x-two-nodes` |
| `open-ephys` | format_version | `GUI 0.6` | metadata, tables, traces | 5 | 5 | `oe-bin-v0-6-x-neuropixels-missing-folders`, `oe-bin-v0-6-x-neuropixels-multiexp-multistream`, `oe-bin-v0-6-x-neuropixels-with-sync` |
| `open-ephys` | format_version | `GUI 1.0` | metadata, tables, traces | 1 | 1 | `oe-bin-v1-0-x-onix-source-neuropixels-sync-timestamps` |
| `open-ephys` | format_version | `Open Ephys format 0.4` | metadata, tables, traces | 5 | 5 | `oe-legacy-sampledata-1`, `oe-legacy-sampledata-2-multiple-starts`, `oe-legacy-sampledata-3` |
| `open-ephys` | format_version | `Open Ephys format 0.6` | metadata, tables, traces | 1 | 1 | `oe-legacy-rhythmdata-test-nodes` |
| `open-ephys` | layout | `binary` | metadata, tables, traces | 11 | 11 | `oe-bin-neural-and-non-neural-data-mixed`, `oe-bin-v0-4-4-1-with-spikes`, `oe-bin-v0-4-4-1-with-video-tracking` |
| `open-ephys` | layout | `legacy` | metadata, tables, traces | 6 | 6 | `oe-legacy-rhythmdata-test-nodes`, `oe-legacy-sampledata-1`, `oe-legacy-sampledata-2-multiple-starts` |
| `open-ephys` | layout | `several sweeps` | descriptive | 2 | 2 | `oe-bin-v0-5-x-two-nodes`, `oe-legacy-sampledata-2-multiple-starts` |
| `winwcp` | field | `experiment.acquisition.started_at` | descriptive | 1 | 1 | `winwcp-file-winwcp-2` |
| `winwcp` | format_version | `8` | metadata, traces | 1 | 1 | `winwcp-file-winwcp-1` |
| `winwcp` | format_version | `9` | metadata, traces | 1 | 1 | `winwcp-file-winwcp-2` |
| `winwcp` | layout | `2 channels` | traces | 2 | 2 | `winwcp-file-winwcp-1`, `winwcp-file-winwcp-2` |
| `winwcp` | writer_version | `V5.3.7` | descriptive | 1 | 1 | `winwcp-file-winwcp-2` |

### Tests, fixtures, fuzz targets, snapshots

- integration tests: none (unit tests in `src/`)
- fuzz targets (`fuzz/fuzz_targets/`): `whole_heka`, `whole_openephys`, `whole_spike2`, `whole_winwcp`
- corpus inputs by tier: full 1, smoke 23, standard 18
- golden snapshots: [`corpus/snapshots/heka-patchmaster.jsonl`](../../corpus/snapshots/heka-patchmaster.jsonl), [`corpus/snapshots/ced-spike2.jsonl`](../../corpus/snapshots/ced-spike2.jsonl), [`corpus/snapshots/winwcp.jsonl`](../../corpus/snapshots/winwcp.jsonl), [`corpus/snapshots/open-ephys.jsonl`](../../corpus/snapshots/open-ephys.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
