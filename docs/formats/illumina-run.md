# Illumina run folders (RunInfo.xml, RunParameters.xml, InterOp, BCL and CBCL)

OpenReadout reads the folder an Illumina sequencer (MiSeq, NextSeq, NovaSeq, HiSeq, iSeq,
MiniSeq) writes for a run: the run's layout and settings, the InterOp metrics the instrument's
real-time analysis writes while it runs, and the base calls. `info` reads the two XML files and
the metrics, never the base calls, so it stays fast on runs of terabytes. The base calls are
decoded by `check` and by the FASTQ export.

Derived from Illumina's published InterOp binary-format pages (documentation of the GPL-3.0
InterOp project; its source was not read), illuminate (MIT) and Picard (MIT) read as
documentation, and two public run folders. Illumina's `interop` package (run as a black box)
and Picard `IlluminaBasecallsToFastq` (MIT) are the ground truth. Provenance:
`docs/provenance/illumina-run.md`. Crate: `openreadout-seq` (`src/illumina/`).

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `illumina-run` | a run folder (or its `RunInfo.xml`) | run layout, settings, InterOp metrics, base calls | evidence rubric (`docs/assurance.md`) |

## Detection

A directory that holds `RunInfo.xml`, or a path to `RunInfo.xml` or `RunParameters.xml` inside
one (the folder is opened).

## RunInfo.xml and RunParameters.xml

`RunInfo/Run`: `Id` (the run id, also the folder's name), `Number`, `Flowcell`, `Instrument`,
`Date` (`220422` on older instruments, `6/24/2020 4:30:24 PM` on newer ones), `Reads/Read`
(`Number`, `NumCycles`, `IsIndexedRead`, `IsReverseComplement`), `FlowcellLayout` (`LaneCount`,
`SurfaceCount`, `SwathCount`, `TileCount`, `TileSet/Tiles/Tile`), `ImageChannels/Name`
(`RED`, `GREEN` on two-colour instruments). Cycles are numbered from 1 over the whole run, read
after read. `RunParameters.xml` (MiSeq: `runParameters.xml`) varies by instrument; the first
element of each of these names is read, wherever it sits: `ApplicationName`,
`ApplicationVersion`, `RtaVersion`/`RTAVersion`, `ExperimentName`, `InstrumentType`,
`FlowCellMode`/`FlowCellType`, `Chemistry`/`ReagentKitVersion`, `LibraryTubeSerialBarcode`,
`RunStartDate`, `Username`, `WorkflowType`. The instrument model is the control software's name
without "Control Software"; when that names a family (`NovaSeq`) or there is no
`RunParameters.xml`, the leading letters of the instrument id refine it (`A` NovaSeq 6000, `M`
MiSeq, `VH` NextSeq 1000/2000, `LH` NovaSeq X, `NB`/`NS` NextSeq 500/550, `FS` iSeq 100, `MN`
MiniSeq, `SH` MiSeq i100, `K`/`J` HiSeq 3000/4000, `E` HiSeq X, `D` HiSeq 2500, `SN` HiSeq
2000), tagged `inferred`.

## InterOp

Every file starts with a version byte and a record-size byte (index metrics: the version only);
some versions add an extended header; fixed-size records follow. Records start with lane (u16),
tile (u16; u32 in the versions marked 32) and, for per-cycle metrics, cycle (u16). Read from
`InterOp/<name>MetricsOut.bin` (or the older name without `Out`); a run still being written has
only per-cycle copies in `InterOp/C<n>.1/`, which are read instead. A trailing partial record
(a file being written) is left out and reported.

| file | versions | record after lane, tile, cycle |
| --- | --- | --- |
| Tile | 2: lane, tile, code u16, value f32. 3 (32): header f32 tile area (mm²); lane, tile, code byte, `t` cluster count and PF count (f32), `r` read u32 and % aligned f32 | codes 100 density, 101 PF density (clusters/mm²), 102 clusters, 103 PF clusters, 200 + 2(N−1) phasing and 201 + 2(N−1) prephasing of read N (fractions; ×100 for %), 300 + N − 1 % aligned of read N |
| Q | 4 (record 206), 5 (has-bins flag, bins as low/high/value arrays), 6 (histogram of one count per bin), 7 (32, bins as low, high, value triples) | u32 count per quality score (50) or per bin |
| Error | 3 (rate f32 and four u32 counts), 4 (32, rate), 5 (32, rate, adapter fraction), 6 (32, header of adapters) | error rate (%) |
| Extraction | 2 (four channels), 3 (32, header channel count) | focus (FWHM) f32 per channel, 90th-percentile intensity u16 per channel |
| CorrectedInt | 2, 3, 4 (32) | base calls per base: no call, A, C, G, T (u32) |
| ExtendedTile | 1 (code, value), 2 and 3 (32, occupied clusters f32) | occupied wells (patterned flow cells) |
| Index | 1, 2 (32, u64 counts) | per read: index sequence, clusters, sample and project names (length-prefixed text) |

Not decoded (listed in `info --view structure` and `check`): control, event, image,
registration, FWHM-grid, PF-grid, optical-model, basecalling and empirical-phasing metrics, the
collapsed Q metrics (`QMetrics2030Out.bin`) and Q metrics by lane (the full Q metrics hold the
same counts).

A record repeated for the same lane, tile and cycle counts once. Records are summed while they
are read (per lane and cycle, and per tile and read), so memory stays small on large runs.

## What the reader returns

- **Table 0, run summary**: one row per read and lane: `read`, `indexed`, `lane`, `tiles`,
  `density` and `density_sd` (K clusters/mm², mean and sample standard deviation over tiles;
  tile metrics v3: clusters / tile area), `percent_pf` and `percent_pf_sd` (per tile, then mean
  and spread), `reads` and `reads_pf` (millions of clusters), `percent_q30` (base calls at Q30 or
  above over the read's cycles except its last, as the instrument's run summary counts them),
  `yield` (Gbp: base calls over the same cycles), `percent_aligned`, `error_rate` (per tile the
  mean over the read's cycles, then the mean over tiles), `phasing`, `prephasing` (tile
  metrics v2), `percent_occupied`, `intensity_cycle_1` (first channel at the read's first
  cycle). `extra`: run id, flow cell, instrument and model, reads, control software and
  version, RTA version, experiment, flow-cell mode, chemistry, the InterOp file versions, the
  completion markers present (`RTAComplete.txt`, `CopyComplete.txt`) and `totals` (yield, %
  ≥ Q30, reads, reads PF, % PF, lanes, cycles, read structure such as `151T8B8B151T`).
- **Table 1, cycles**: one row per lane and cycle: `percent_q30`, `mean_qscore`, `error_rate`,
  `percent_a`…`percent_n` (base calls), `intensity_<channel>` (A, C, G, T, or red and green).
- **Table 2, tiles**: per lane and tile: `surface`, `swath` (the tile number's first two
  digits), density, PF density, clusters, PF clusters, % PF, % occupied, % ≥ Q30, error rate,
  % aligned.
- **Table 3, qscore histogram**: `lane`, `cycle`, `qscore`, `base_calls` (non-zero bins;
  `extra.bins` for binned instruments).
- **Table 4, index**: `lane`, `index`, `sample`, `project`, `clusters`, `percent_of_pf`.
- **Table 5, base calls**: one row per lane and tile with a filter file; `clusters` and
  `clusters_pf` are counted from the filter files when the table is read.
- **Trace 0, run quality by cycle**: one sweep per lane over the cycle axis, channels
  `percent_q30`, `mean_qscore`, `error_rate`, the intensities and the base percentages.
  `extra.reads` gives each read's cycles.
- **Experiment**: vendor Illumina, model, serial (instrument id), software; DNA sequencer
  (OBI:0400103), DNA sequencing by synthesis (OBI:0000734); parameters read structure, cycles,
  lanes, flow cell, RTA version, flow-cell mode, chemistry; started at (RunInfo date); the
  library tube barcode as the sample barcode.

## Base calls

`Data/Intensities/BaseCalls/L<lane>/C<cycle>.1/`:

- **BCL** (`s_<lane>_<tile>.bcl`, or `.bcl.gz`): u32 cluster count, then one byte per cluster:
  bits 0–1 the base (A, C, G, T), bits 2–7 the quality; 0 is a no-call (`N`, quality 2).
- **CBCL** (`L<lane>_<surface>.cbcl`): u16 version, u32 header size, bits per base and per
  quality (2 and 2 are read; others are refused), u32 bin count and (bin, quality) u32 pairs,
  u32 tile count and per tile u32 tile number, clusters, uncompressed and compressed block size,
  then a byte that is 1 when clusters that failed the filter were left out. The tile blocks are
  gzip members in that order; a byte holds two clusters, low nibble first: base in bits 0–1,
  quality bin in bits 2–3; nibble 0 is a no-call.
- **Filter** (`BaseCalls/L<lane>/s_<lane>_<tile>.filter`): u32 0, u32 version 3, u32 cluster
  count, one byte per cluster (bit 0: passed).
- **Locations**: `Intensities/L<lane>/s_<lane>_<tile>.locs` per tile, or one `Intensities/s.locs`
  for a patterned flow cell: u32 1, f32 1.0, u32 count, then f32 x, y per cluster (pixels).

NextSeq 500/550 per-lane `.bcl.bgzf` files and `.clocs` are not decoded (`check` reports them).

## FASTQ export

`export --format fastq` (`openreadout_export` `format: "fastq"`) writes a directory
(`<run folder>_fastq` by default) with one gzip FASTQ per lane and read: `<run>_L001_R1.fastq.gz`,
`_R2`, and the index reads as `_I1`, `_I2`. Clusters that failed the filter are left out.
Records follow tile and cluster order; names are
`@<instrument>:<run number>:<flowcell>:<lane>:<tile>:<x>:<y> <read>:N:0:<index>`, with
`x = round(10 × x_px + 1000)` in single precision (as Picard computes it) and the index reads'
bases joined by `+`. `--rows 0-N` writes the first N+1 clusters of each lane. The export does
not demultiplex, trim adapters or apply a sample sheet. Each file is written under a temporary
name, read back (records counted and hashed) and then renamed into place.

## Validation

`tests/corpus/seq_oracle/mod.rs` with `oracle/seq_oracle.py`: every run-summary value per read
and lane against Illumina's `interop` (relative 1e-5; phasing of runs that record it only in
empirical-phasing metrics is not compared), the totals, and the FASTQ of lane 1 against Picard
`IlluminaBasecallsToFastq` (pass-filter clusters, no EAMSS masking, file order): the first 2000
records of every read byte for byte (name, bases, qualities) and the pass-filter count. A
MiSeq (BCL) and a NovaSeq 6000 (CBCL) run folder. During development the whole MiSeq export
(617,038 records) and the first 100,000 records of each NovaSeq read were also equal to
Picard's.

## Vocabulary (public API of `openreadout-seq`: `src/illumina/`, `lib.rs`, `table.rs`)

| identifier | meaning |
| --- | --- |
| `add_corrected` | Corrected-intensity metrics (`CorrectedIntMetricsOut.bin`), versions 2 to 4: base calls per base |
| `add_error` | Error metrics (`ErrorMetricsOut.bin`), versions 3 to 6 |
| `add_extended_tile` | Extended tile metrics (`ExtendedTileMetricsOut.bin`): occupied wells |
| `add_extraction` | Extraction metrics (`ExtractionMetricsOut.bin`), versions 2 and 3: focus and the 90th percentile intensity per channel |
| `add_index` | Index metrics (`IndexMetricsOut.bin`), versions 1 and 2 |
| `add_q` | Quality metrics (`QMetricsOut.bin`), versions 4 to 7 |
| `add_tile` | Tile metrics (`TileMetricsOut.bin`), versions 2 and 3 |
| `aligned` | Percent of clusters aligned to PhiX per read |
| `application` | Control software (`MiSeq Control Software`, `NovaSeq Control Software`) |
| `application_version` | Its version |
| `BAM_FORMAT_ID` | Format id of BAM files |
| `base` | The base letter of a base-call byte |
| `BaseCalls` | The base-call files of a run folder, found by listing directories (cheap: cycle directories and one cycle's files, not every file) |
| `bases` | Bases written |
| `bins` | Quality score of each bin index |
| `bits_per_base` | Bits per base call |
| `bits_per_quality` | Bits per quality score |
| `calls` | Base calls per base: no call, A, C, G, T (corrected-intensity metrics) |
| `CallStorage` | How the base calls of a run are stored |
| `categories` | Labels of a text column |
| `cbcl_header` | The CBCL header of one cycle of a lane and surface (for `check` and the structure view) |
| `CbclHeader` | A CBCL header |
| `CbclTile` | A tile block inside a CBCL file |
| `channels` | Intensity channel count (4 for A, C, G, T on four-colour instruments) |
| `chemistry` | Chemistry or reagent kit version |
| `clusters` | Clusters in the block |
| `clusters_pf` | Clusters passing filter |
| `Col` | One column |
| `col` | The column called `name` |
| `cols` | Columns |
| `cycle_file` | The file holding cycle `cycle` of `tile` in `lane` |
| `cycles` | Lane → cycles that have a `C<n>.1` directory |
| `CycleTotals` | Totals for one lane and cycle, summed over tiles |
| `date` | Run date as written (`220422` or `6/24/2020 4:30:24 PM`) |
| `default_output` | The default output directory of a run's FASTQ: `<run folder>_fastq` beside it |
| `density` | Cluster density, clusters per mm² |
| `density_pf` | Density of clusters passing filter, clusters per mm² |
| `dir` | `Data/Intensities/BaseCalls` |
| `discover` | Find the base calls of the run folder at `root`; `None` when it has none |
| `dtype` | dtype reported for the column |
| `error` | Sum of error rates (percent) and the number of cycles, per read |
| `error_sum` | Sum of per-tile error rates (percent) and the number of tiles |
| `error_tiles` | Tiles with an error rate |
| `experiment` | Experiment name the operator entered |
| `export_fastq` | Write the run's base calls as FASTQ into `out_dir` (created when missing) |
| `export_run_fastq` | Export the run folder at `input` (the folder or its `RunInfo.xml`) as FASTQ: into `output` (default [`default_output`]), the clusters `rows` (zero-based, inclusive; `None` for every cluster) of each lane |
| `extra` | `extra` of the table's `TableInfo` |
| `FAST5_FORMAT_ID` | Format id of FAST5 files |
| `fastq_coordinate` | The FASTQ cluster coordinate of a location, as the conversion tools write it: `round(10 * position + 1000)`, computed in single precision like the stored position (a position such as 1418.95 lands on .5 in single precision and rounds up) |
| `FASTQ_FORMAT_ID` | Format id of FASTQ files |
| `FastqFile` | One FASTQ file written |
| `FastqOptions` | Options of [`export_fastq`] |
| `FastqReport` | What [`export_fastq`] wrote |
| `files` | Files written, in lane and read order |
| `filter_path` | The filter file of a tile |
| `filtered_out` | Clusters skipped because they failed the filter |
| `flowcell` | Flow-cell id |
| `flowcell_mode` | Flow-cell mode or type (`SP`, `S4`, `P3`) |
| `focus_sum` | Sum of per-tile focus (FWHM) per channel |
| `format` | `fastq` |
| `FORMAT_ID` | Format id of Illumina run folders |
| `fs` | The file namespace the run folder is read through |
| `header_len` | Header length in bytes |
| `high` | Highest score of the bin |
| `ILLUMINA_RUN_FORMAT_ID` | Format id of Illumina run folders |
| `IlluminaRun` | An open Illumina run folder |
| `IlluminaRunReader` | Reader of Illumina run folders |
| `image_channels` | Image channels (`RED`, `GREEN`; two-colour chemistry), when listed |
| `include_filtered` | Also write clusters that failed the filter (their names say `Y`) |
| `index` | Index sequence (`I7+I5`) |
| `IndexCount` | Clusters assigned to one index sequence (index metrics) |
| `indexed` | True for an index (barcode) read |
| `info` | `TableInfo` for `info` |
| `input` | The run folder |
| `instrument` | Instrument id (serial) |
| `instrument_model` | The instrument model from the control software's name, else from the leading letters of the instrument id (inferred: the convention seen across public run folders) |
| `instrument_type` | Instrument type, when the file names it (`NextSeq 2000`, `NovaSeqXPlus`) |
| `int` | An integer column (`int64`; values are whole numbers) |
| `intensity_sum` | Sum of per-tile 90th-percentile intensities per channel |
| `intensity_tiles` | Tiles with extraction records |
| `InterOp` | Every InterOp file of a run, aggregated |
| `lane` | Lane |
| `lane_count` | Lanes |
| `lanes` | Lanes to write (empty: every lane with base calls) |
| `layout` | `(read number, first cycle, last cycle)` per read, cycles numbered from 1 over the run |
| `library_tube` | Library tube or sample-loading barcode |
| `locs_path` | The locations file of a tile: per-tile `.locs`, else the flow cell's `s.locs` |
| `low` | Lowest score of the bin |
| `MAX_INTEROP_FILE` | Largest InterOp file read (a NovaSeq S4 Q-metrics file is a few hundred MB at most) |
| `max_reads` | Stop after this many clusters per lane (a sample of the run) |
| `mean_sd` | Mean and population standard deviation of finite values |
| `MemTable` | A table held in memory |
| `metrics` | The InterOp metrics, aggregated |
| `METRICS_FORMAT` | The format id, for errors raised while reading metrics |
| `name` | Column name |
| `new` | A named table with these columns |
| `notes` | Things worth knowing (no cluster locations, a tile without base calls, …) |
| `num` | A numeric column (`float64`) |
| `number` | Read number as `RunInfo.xml` numbers it (from 1) |
| `occupied` | Occupied wells (patterned flow cells; extended tile metrics) |
| `offset` | Offset of the gzip block in the file |
| `open` | Open the run folder named by `input` (the folder, or its `RunInfo.xml`) |
| `operator` | Operator / user name |
| `output` | The directory the files were written to |
| `overwrite` | Replace existing output files |
| `params` | What `RunParameters.xml` says, when the folder has one |
| `parse_cbcl_header` | Parse a CBCL header from the first bytes of the file |
| `parse_filter` | Parse a filter file |
| `parse_locs` | Parse a `.locs` file into `(x, y)` pixel positions |
| `parse_runinfo` | Parse `RunInfo.xml` |
| `parse_runparameters` | Parse `RunParameters.xml` |
| `path` | Path of the file |
| `pf_excluded` | True when clusters that failed the filter were left out of the blocks |
| `phasing` | Phasing per read (percent), keyed by read number |
| `POD5_FORMAT_ID` | Format id of POD5 files |
| `prephasing` | Prephasing per read (percent) |
| `problems` | Problems met (truncated files, unknown versions): one line each |
| `project` | Project name |
| `push` | Push a number (`None` becomes NaN) |
| `push_text` | Push a text value |
| `q30` | Base calls at Q30 or above and in total, per read (from the quality metrics) |
| `q_bins` | Quality bins, when the instrument bins scores |
| `q_hist` | Base calls per quality score (index = score, 0..=50) |
| `q_tiles` | Tiles with quality records |
| `QBin` | A quality-score bin: scores `low..=high` are reported as `value` |
| `qual` | The Phred+33 quality character of a base-call byte (a no-call is quality 2) |
| `raw_len` | Uncompressed bytes |
| `read` | `R1`, `R2`, `I1`, … |
| `read_capped` | Read a whole file of at most `max` bytes |
| `read_cycle` | Base calls of clusters `[first, first + count)` of a tile at one cycle, one byte each (see the module documentation), every cluster of the tile including those that failed the filter |
| `read_filter` | Read a tile's filter: one bool per cluster, true when it passed |
| `read_locs` | Read cluster positions `(x, y)` in pixels; `None` when the run folder has no locations |
| `read_of` | The read number of `cycle`, when it falls in one |
| `read_structure` | The read structure as the conversion tools write it (`151T8B8B151T`) |
| `ReadInfo` | One read of the run (a sequencing read or an index read) |
| `ReadLayout` | Which read a cycle belongs to: `reads[i] = (read number, first cycle, last cycle)` |
| `reads` | Reads in order |
| `records` | Records written |
| `reverse_complement` | True when the read is sequenced as the reverse complement |
| `root` | The run folder |
| `rows` | Rows |
| `rta_version` | Real-time analysis version |
| `run` | What `RunInfo.xml` says |
| `run_id` | Run id (the run folder's name) |
| `run_number` | Run number on the instrument |
| `run_start` | Run start date as written |
| `RunInfo` | What `RunInfo.xml` says |
| `RunParameters` | What `RunParameters.xml` (or MiSeq's `runParameters.xml`) says, as far as it is read |
| `sample` | Sample name |
| `skipped` | Files present but not decoded (name → why) |
| `slice` | Rows `[first, first + max)` |
| `storage` | BCL or CBCL |
| `stored_len` | Compressed bytes |
| `surface_count` | Surfaces per lane |
| `surface_of_tile` | The surface digit of a tile number (`2101` → 2, `11101` → 1) |
| `swath_count` | Swaths per surface |
| `text` | A text column |
| `tidy` | Round to a sensible number of significant digits for JSON summaries |
| `tile` | Tile number |
| `tile_area_mm2` | Tile area in mm² (tile metrics v3) |
| `tile_count` | Tiles per swath |
| `tile_naming` | Tile naming convention (`FourDigit`, `FiveDigit`) |
| `tiles` | Tile blocks in file order |
| `TileTotals` | Per-tile totals (tile metrics plus quality and error totals over cycles) |
| `total_cycles` | Total cycles over every read |
| `unit` | Unit, when the values have one |
| `value` | The score reported for the bin |
| `values` | Values (codes into `categories` for text columns) |
| `verified` | Every file was read back and matched what was written |
| `version` | Format version |
| `versions` | File name → format version, for every file read |
| `workflow` | Workflow or application type (`GenerateFASTQ`, `Resequencing`) |
| `xxh3` | xxh3 of the uncompressed records |
