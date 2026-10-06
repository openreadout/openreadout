# Lab shares, indexes and live acquisitions

A core facility's file share holds years of instrument data: CZI and ND2 stacks, FCS tubes, Thermo runs, ChemStation folders, half-copied files and much that is not instrument data at all. This page shows how to build a catalog of such a share, search it, check its health and export a slice. It then covers files that an instrument is still writing.

The catalog is a directory of Parquet tables and one JSON manifest. There is no server and no database. DuckDB, pandas, polars or Excel's Power Query read the tables directly.

## Index a share

```bash
openreadout index /lab/share -o /lab/index
```

```text
indexed /lab/index: 19 data sets, 19 files (17.9 MB) in 19 item(s) of /lab/share
crawl: 0.0 s, 731 items/s, 19 new, 0 changed, 0 unchanged, 0 removed, 0 unrecognised, 2 unreadable; 2.0 MB read by the crawler

format  data sets     bytes
──────  ─────────  ────────
abf             2  655.4 kB
czi             2    3.1 MB
fcs            10    2.6 MB
mzml            3    2.3 MB
nd2             2    9.2 MB

check: corrupt 2, ok 10, truncated 3, warning 4
personal data: 10 flags in 6 data sets (free_text 2, person_name 8)
```

The crawl reads headers only. For each data set it runs what `info` runs, the integrity check in its headers-only mode (`check --headers-only`), and the [personal-data rules](#personal-data). It also reads the first and last 64 KiB of each file for a content fingerprint. It doesn't read pixel planes, table rows, trace samples or spectra.

- **Resumable.** Stop the crawl at any time, with Ctrl-C or a crash, and run the same command again: it continues from its last checkpoint. `--restart` discards the interrupted crawl instead.
- **Incremental.** Run it again next week and only new or changed files are read. A file with the same size, modification time and OpenReadout version is not opened again. `--full-rescan` reads everything.
- **Moves.** A file that moved is recognised by its size and fingerprint and marked `change = moved`.
- **Capped sessions.** `--max-files N` and `--max-seconds S` stop after a checkpoint and write what was read so far, with `complete: false`. Rerun the same command to continue. This lets a scheduler crawl a very large share an hour at a time.
- **Parallel.** `--threads N` sets the number of worker threads. The output does not depend on it.

Other options:

- `--check headers|full|none` sets the integrity check. `full` runs the complete `check`, which may decode data.
- `--exclude GLOB` skips names or root-relative paths (repeatable).
- `--follow-symlinks` follows symbolic links. By default they are skipped and counted.
- `--no-pii` skips the personal-data rules.
- `--health` prints the [storage health report](#storage-health) after the crawl.

Hidden entries are skipped. The index directory must not be inside a crawled directory. A lock keeps two crawls from writing the same index.

### Data sets made of several files

Some data sets are a directory or several files. Each becomes one record in `experiments.parquet`, and its other files are `member` rows of `files.parquet`.

- **Directory data sets**: Bruker `.d`, ChemStation `.D`, Waters `.raw`, Bruker TopSpin experiments, OME-Zarr stores. The crawl does not walk into them. A TopSpin project folder is walked into, so each experiment is its own record.
- **Files that name each other**: multi-file OME-TIFF, CZI file parts, OIR continuation files, OIF folders, VSI `.ets` stacks, imzML `.ibd`, SER/EMI pairs, SpikeGLX `.bin` and `.meta`.
- **Stream series**: SpikeGLX runs with several probes, and Blackrock `.nsX` with `.nev`. The other streams' counts are added to the first stream's record.

Files that no reader recognises are kept as `role = unknown` rows of `files.parquet`, and `index.json` counts them per extension under `unknown_extensions`.

### What the index directory holds

| file | contents |
| --- | --- |
| `index.json` | the manifest: roots, settings, `complete`, crawl counters, totals, and every table's columns |
| `experiments.parquet` | one row per data set |
| `files.parquet` | one row per file seen, and per directory data set |
| `problems.parquet` | one row per problem: integrity findings, files that could not be opened, walk errors, personal-data flags |
| `state/` | crawl state used to resume and to update; it may be deleted, and the next crawl is then a full one |

`openreadout self schema index` prints the JSON Schema of `index.json`. Timestamps in the tables are UTC microseconds. Rows are in walk order: within a directory, its files and data-set directories by name, then its sub-directories by name. `schema_version` in `index.json` changes only when a column is renamed, removed or retyped; new columns may be added.

The tables are ordinary Parquet:

```python
import duckdb
duckdb.sql("""
    select instrument_model, count(*) as runs, sum(size_bytes) / 1e9 as gb
    from '/lab/index/experiments.parquet'
    where family = 'mass-spectrometry'
    group by 1 order by 3 desc
""")
```

## Columns

The same lists are in `index.json` under `tables.*.columns`, with each column's type and unit class. Types are `string`, `uint64`, `int64`, `float64`, `timestamp` (UTC microseconds), `string list` and `int64 list`. A unit class in parentheses is what query units convert to.

### `experiments.parquet`

| column | type | description |
| --- | --- | --- |
| `path` | string | Absolute path of the data set (a file, or a directory data set such as a Bruker .d). |
| `name` | string | File or directory name. |
| `dir` | string | Parent directory. |
| `root` | string | The crawl root it was found under. |
| `kind` | string | `file` or `directory` (a directory data set). |
| `ext` | string | Lower-case extension (`czi`, `tiff`, `d`). |
| `format` | string | Format id (`czi`, `nd2`, `thermo-raw`, ...; `openreadout self formats`). |
| `format_name` | string | Format name. |
| `family` | string | Format family: microscopy, electron-microscopy, flow-cytometry, electrophysiology, mass-spectrometry, chromatography, nmr, plate-reader, spectroscopy, container. |
| `format_vendor` | string | Vendor or standards body of the format. |
| `preservation` | string | `open` (an open, documented format), `vendor` (proprietary, maintained vendor software) or `legacy` (proprietary; the vendor's software is discontinued). |
| `confidence` | string | Detection confidence: `definite`, `likely` or `extension_only`. |
| `format_version` | string | Format version, when the reader reports one. |
| `assurance` | string | `validated`, `partially_validated` or `unvalidated`: whether the file lies inside what its reader was validated on ([Assurance](../reference/assurance.md)). |
| `variant` | string | Variant fingerprint (`info` → assurance.fingerprint): format id and every feature that decides how the file is decoded. |
| `size_bytes` | uint64 (bytes) | Bytes of the whole data set (the file plus its member files, or every file of a directory data set). |
| `file_count` | uint64 | Files in the data set. |
| `mtime` | timestamp | Modification time (the newest file of the data set). |
| `fingerprint` | string | xxh3-128 of the size and the first and last 64 KiB (directory data sets: member names and sizes plus the largest file's fingerprint). |
| `sample_id` | string | `experiment.sample.id`: the sample identifier the file records. |
| `sample_name` | string | `experiment.sample.name`. |
| `sample_well` | string | `experiment.sample.well`: plate well. |
| `sample_barcode` | string | `experiment.sample.barcode`. |
| `sample_position` | string | `experiment.sample.sequence_position`: vial or autosampler position. |
| `sample_source_field` | string | Field the sample id came from. |
| `instrument_vendor` | string | `experiment.instrument.vendor`. |
| `instrument_model` | string | `experiment.instrument.model`. |
| `instrument_serial` | string | `experiment.instrument.serial`. |
| `instrument_software` | string | `experiment.instrument.software`. |
| `instrument_software_version` | string | `experiment.instrument.software_version`. |
| `instrument_kind_id` | string | OBI device term id (`OBI:0400169` microscope). |
| `instrument_kind_label` | string | OBI device term label. |
| `method_name` | string | `experiment.method.name`: method, protocol or experiment name. |
| `technique_id` | string | Technique term id (CHMO, FBbi or OBI: `FBbi:00000246`). |
| `technique_label` | string | Technique term label (`fluorescence microscopy`). |
| `assay_id` | string | OBI assay term id. |
| `assay_label` | string | OBI assay term label. |
| `terms` | string list | Every term id in the experiment (technique, assay, instrument kind, measurement techniques and terms). |
| `term_labels` | string list | Labels of `terms`, same order. |
| `operator` | string | `experiment.acquisition.operator` (personal data: see `pii_*`). |
| `started_at` | timestamp | Acquisition start (`experiment.acquisition.started_at`, else the first image's `acquired_at`), UTC. |
| `started_at_text` | string | Acquisition start as recorded (time zone as the file gives it). |
| `ended_at` | timestamp | Acquisition end, UTC. |
| `duration_s` | float64 (seconds) | Length of the recorded data, seconds. |
| `acquired_year` | int64 | Year of `started_at`. |
| `what` | string list | Each measurement in scientific words (`experiment.measurements[].what`). |
| `parameters_json` | string | `experiment.method.parameters` as JSON (`{"nucleus": {"value": "1H"}}`); queried as `param.<name>`. |
| `image_count` | uint64 | Images. |
| `plane_count` | uint64 | 2-D planes over all images. |
| `size_x` | uint64 | Width of the largest image, pixels. |
| `size_y` | uint64 | Height of the largest image, pixels. |
| `size_z` | uint64 | Z-slices of the largest image. |
| `size_c` | uint64 | Channels of the largest image. |
| `size_t` | uint64 | Time points of the largest image. |
| `pixel_type` | string | Pixel type of the largest image (`uint16`, ...). |
| `physical_size_x_um` | float64 (micrometres) | Pixel width, µm. |
| `physical_size_y_um` | float64 (micrometres) | Pixel height, µm. |
| `physical_size_z_um` | float64 (micrometres) | Z-step, µm. |
| `time_increment_s` | float64 (seconds) | Time between time points, seconds. |
| `channels` | string list | Channel names and fluorophores of every image; FCS `$PnS` labels. |
| `objective` | string | Objective model. |
| `objective_magnification` | float64 (magnification) | Objective nominal magnification. |
| `objective_na` | float64 | Objective numerical aperture. |
| `immersion` | string | Objective immersion. |
| `mosaic_tiles` | uint64 | Tiles of a mosaic image. |
| `table_count` | uint64 | Tables (FCS data sets, plate reads, event tables). |
| `table_rows` | uint64 | Rows over all tables (events, wells). |
| `table_columns` | string list | Column names (FCS `$PnN`, ...), first 64. |
| `trace_count` | uint64 | Traces (sweeps, chromatograms, FIDs, spectra). |
| `trace_channels` | uint64 | Signal channels over all traces. |
| `trace_sweeps` | uint64 | Sweeps over all traces. |
| `trace_samples` | uint64 | Samples per channel over all traces and sweeps. |
| `sample_rate_hz` | float64 (hertz) | Highest sample rate, Hz. |
| `spectra_runs` | uint64 | Mass-spectrometry runs. |
| `scan_count` | uint64 | Mass spectra over all runs. |
| `ms_levels` | int64 list | MS levels present. |
| `rt_min_s` | float64 (seconds) | First retention time, seconds. |
| `rt_max_s` | float64 (seconds) | Last retention time, seconds. |
| `check_status` | string | `ok`, `warning`, `truncated`, `corrupt`, `unreadable`, `unsupported` or `not_checked`. |
| `check_mode` | string | `headers` (structure only; the default) or `full` (`check`). |
| `check_findings` | uint64 | Findings of the integrity check. |
| `check_codes` | string list | Codes of the findings (`truncated`, `missing_part`, ...). |
| `error_code` | string | Why the data set could not be opened (`corrupt_file`, `unsupported_feature`, `io`). |
| `error_message` | string | The error message. |
| `pii_count` | uint64 | Personal-data flags. |
| `pii_kinds` | string list | Kinds flagged (`person_name`, `email`, `phone`, `patient_id`, `date_of_birth`, `free_text`). |
| `pii_fields` | string list | Field paths flagged (values are never stored). |
| `notes` | string list | Reader notes (first 8). |
| `change` | string | `new`, `changed`, `unchanged` or `moved` relative to the previous run. |
| `moved_from` | string | Previous path of a moved data set. |
| `indexed_at` | timestamp | When the data set was read. |
| `tool_version` | string | openreadout version that read it. |
| `elapsed_ms` | float64 | Time spent reading it, milliseconds. |
| `experiment_json` | string | The whole `experiment` object as JSON (with provenance). |

### `files.parquet`

| column | type | description |
| --- | --- | --- |
| `path` | string | Absolute path. |
| `dataset_path` | string | The data set it belongs to (itself for a data set; null for unrecognised files). |
| `role` | string | `dataset` (the file or directory opened), `member` (another file of a data set) or `unknown` (no reader recognises it). |
| `kind` | string | `file` or `directory`. |
| `size_bytes` | uint64 (bytes) | Bytes. |
| `mtime` | timestamp | Modification time. |
| `ext` | string | Lower-case extension. |
| `format` | string | Format id of the data set it belongs to. |
| `fingerprint` | string | Content fingerprint (data sets and files up to 64 KiB). |
| `change` | string | `new`, `changed`, `unchanged` or `moved`. |

### `problems.parquet`

| column | type | description |
| --- | --- | --- |
| `path` | string | The file or data set. |
| `format` | string | Format id, when detected. |
| `category` | string | `integrity` (check findings), `readability` (could not be opened or summarized), `walk` (a directory or entry that could not be read) or `pii` (a personal-data flag). |
| `severity` | string | `error`, `warning`, `info` or `review` (personal data). |
| `code` | string | Stable code: a check finding code, an error code, or the personal-data kind. |
| `message` | string | Human-readable message (never a personal-data value). |
| `field` | string | Personal data: the field path. |
| `rule` | string | Personal data: the rule that fired. |

## Search

```bash
openreadout search /lab/index "format=fcs events>10000" --fields path,sample_id,events
```

```text
path                          sample_id  table_rows
────────────────────────────  ─────────  ──────────
archive/old_runs/run_004.fcs             20000
flow/run_005.fcs              A1         11585
flow/run_021.fcs                         13367
scope_a/run_005.fcs                      13367
4 of 4 matching data sets (paths under /lab/share)
```

More queries:

```bash
openreadout search /lab/index "objective=63x channel~GFP acquired<2020"
openreadout search /lab/index "technique:FBbi_00000246 size>1GB" --sort -size --limit 20
openreadout search /lab/index "status=truncated|corrupt|unreadable" --fields path,format,check_codes
openreadout search /lab/index "format=thermo-raw param.polarity=negative" --jsonl > negative-mode.jsonl
openreadout search /lab/index "risk=legacy OR risk=vendor -format=czi" --sort -size
```

Output options:

- `--json` prints the [JSON wrapper](../getting-started/reading-json.md#the-json-wrapper). Its `total` counts every match, which answers "how many" questions.
- `--jsonl` prints one object per result and line.
- `--sort FIELD` sorts; `-FIELD` sorts descending. Missing values go last. Without it, results are in walk order.
- `--limit N` returns at most N results (default 50; `0` for all).
- `--fields a,b,c` picks the fields; aliases are allowed, and `all` gives every column.

### Query language

A query is a list of terms separated by spaces, and every term must match. `OR` between terms splits the query into alternatives: `a=1 b=2 OR c=3` means `(a=1 AND b=2) OR c=3`. Quotes keep spaces in a value (`sample~"plate 3"`).

| term | meaning |
| --- | --- |
| `field=value` | equal (text ignores case; a list matches when any element is equal) |
| `field!=value` | not equal |
| `field~text` | contains, ignoring case |
| `field!~text` | does not contain |
| `field<v`, `<=`, `>`, `>=` | compare numbers with units, or dates |
| `field:value` | has the term: `technique:FBbi_00000246`, or a word of its label: `technique:confocal` |
| `-term` | negates any term: `-status=ok` |
| `a\|b` | alternatives within one term: `format=czi\|nd2\|lif` |
| `word` | a bare word matches the path, sample, method, channels, instrument, technique, format or operator |

Comparisons take units: `size>1GB`, `rate>=20kHz`, `pixel<0.2um`, `duration>30min`, `objective>=40x`. The units are:

- bytes: `B`, `KB` to `PB` (powers of 1000), `KiB` to `PiB` (powers of 1024)
- seconds: `us`, `ms`, `s`, `min`, `h`, `d`
- hertz: `Hz`, `kHz`, `MHz`, `GHz`
- micrometres: `nm`, `um` or `µm`, `mm`, `Å`
- magnification: `x`

A number without a unit is in the column's unit. Dates may be partial: `acquired<2020` means before 2020-01-01, `acquired>2020` means from 2021-01-01, and `acquired>=2021-06` works too. `=` on a date matches that year, month or day.

Fields are the column names of `experiments.parquet` and these aliases:

| alias | searches | meaning |
| --- | --- | --- |
| `sample` | `sample_id`, `sample_name`, `sample_well`, `sample_barcode`, `sample_position` | sample id, name, well, barcode or position |
| `well`, `barcode` | `sample_well`, `sample_barcode` | plate well, barcode |
| `instrument` | `instrument_vendor`, `instrument_model`, `instrument_serial` | instrument vendor, model or serial |
| `vendor` | `instrument_vendor`, `format_vendor` | instrument or format vendor |
| `model`, `serial` | `instrument_model`, `instrument_serial` | |
| `software` | `instrument_software`, `instrument_software_version` | acquisition software and version |
| `device` | `instrument_kind_id`, `instrument_kind_label` | OBI device term (`device:OBI_0400169`) |
| `method` | `method_name` | method, protocol or experiment name |
| `technique` | `technique_id`, `technique_label` | technique term (CHMO, FBbi, OBI) |
| `assay` | `assay_id`, `assay_label` | OBI assay term |
| `term` | `terms`, `term_labels` | any term in the experiment |
| `acquired`, `date` | `started_at` | acquisition start (dates or partial dates) |
| `year` | `acquired_year` | |
| `duration` | `duration_s` | length of the recorded data |
| `what` | `what` | measurement descriptions |
| `channel` | `channels`, `table_columns` | channel names, fluorophores, FCS `$PnN` names and `$PnS` labels |
| `objective` | `objective_magnification`, `objective` | the magnification when the value is a number (`63x`), else the objective model |
| `na` | `objective_na` | numerical aperture |
| `pixel`, `z_step` | `physical_size_x_um`, `physical_size_z_um` | pixel size, z-step |
| `x`, `y`, `z`, `c`, `t` | `size_x` … `size_t` | dimensions of the largest image |
| `images`, `planes` | `image_count`, `plane_count` | |
| `rows`, `events` | `table_rows` | table rows (FCS events, wells) |
| `columns` | `table_columns` | table column names |
| `traces`, `sweeps`, `rate` | `trace_count`, `trace_sweeps`, `sample_rate_hz` | |
| `scans`, `ms_level`, `rt` | `scan_count`, `ms_levels`, `rt_max_s` | |
| `size` | `size_bytes` | bytes of the data set |
| `modified` | `mtime` | modification time |
| `status`, `check` | `check_status` | `ok`, `warning`, `truncated`, `corrupt`, `unreadable`, `unsupported`, `not_checked` |
| `problem` | `check_codes`, `error_code` | a finding or error code |
| `pii` | `pii_kinds` | a personal-data kind (`pii=email`); `pii=true` / `pii=false` for any |
| `risk` | `preservation` | `open`, `vendor` or `legacy` |
| `param.<name>` | `parameters_json` | an experiment parameter: `param.nucleus=1H`, `param.polarity=negative`, `param.pulse_program~zg` |

An unknown field is a usage error (exit 2) that lists the valid fields.

## Storage health

```bash
openreadout search /lab/index --health -o health.md
```

```text
| data sets | 19 (17.9 MB) |
| files seen | 19 |
| unrecognised files | 0 (0 B) |
| truncated, corrupt or unreadable | 5 |
| duplicate groups | 1 (216.4 kB redundant, confirmed by SHA-256) |
| same experiment stored twice | 0 (0 exported twice) |
| at risk (vendor-only, no open export) | 6 (13.0 MB; 0 legacy) |
| data sets with possible personal data | 6 (10 flags) |
```

The report is Markdown. `-o FILE` also writes it to a file, and `--json` gives it as JSON. It has these sections:

- **Totals** per format, family and acquisition year.
- **Integrity**: data sets per check status, and the damaged ones with their finding codes.
- **Unreadable**: recognised formats that could not be opened, and unrecognised files per extension.
- **Duplicates**: byte-identical data sets. Only candidates with the same size and fingerprint are hashed in full (SHA-256) to confirm. `--no-hash` lists the candidates without reading anything.
- **Same experiment stored twice**: data sets that are not byte-identical but record the same acquisition, such as a CZI and its OME-TIFF export, or a Thermo `.raw` and its mzML.
- **Files at risk**: data sets in vendor-only formats with no open-format file next to them. Zeiss AxioVision and FEI TIA files are marked `legacy` because their vendor software is discontinued.
- **Personal data**: flags per kind, rule and field. The values themselves are not printed.

Lists are capped at `--max-list N` (default 100); counts are not capped.

## Export a slice

`search --export` writes the data sets a query selects to open formats, with a datasheet that describes them:

```bash
openreadout search /lab/index "family=microscopy status=ok" --export dataset/ \
    --redact --salt-file ~/.openreadout-salt
```

```text
dataset/DATASHEET.md, dataset/datasheet.json   the datasheet
dataset/data/<id>.json                         info and experiment of each source
dataset/data/<id>.ome.zarr/                    images (OME-Zarr)
dataset/data/<id>.table<N>.parquet             tables (FCS events, plate reads)
dataset/data/<id>.trace<N>.parquet             traces, one row per sample
dataset/data/<id>.spectra<N>.parquet           mass spectra, one row per point
dataset/data/<id>.spectra<N>.scans.parquet     one row per spectrum
```

`<id>` is the source's file stem plus 8 hex digits of a hash of its path.

- `--tables csv` writes CSV instead of Parquet.
- `--no-images` leaves out images.
- `--limit N` exports at most N data sets.
- `--license SPDX` sets the licence of the whole export. Without it, each source gets the licence of the nearest `LICENSE`, `LICENCE` or `COPYING` file at or above it, or `unknown`.
- `--redact` replaces personal data; see [Redaction](#redaction).

The datasheet records the query, counts, formats, techniques, instruments, the date range, every source with its fingerprint and the files written from it, licences and personal-data flags. `openreadout self schema search-export` prints its JSON Schema.

Each output is written under a temporary name, read back and compared, then renamed. The sources are not modified. A rerun resumes where the last one stopped. The exit code is 1 when a data set could not be exported or an output could not be verified; the datasheet lists it under `skipped` with the reason.

## Files still being written

You can read data while the instrument is still writing it. `info`, `info --view structure`, `check` and `planes` read every complete plane of a growing file. They report the unfinished part in an `acquisition` block instead of failing as corrupt. None of these commands write to the data or lock it.

| format | read while growing | unit read |
| --- | --- | --- |
| OME-TIFF and TIFF (single file) | yes | page |
| OME-Zarr (directory store) | yes | plane whose chunks are all stored |
| Nikon ND2 | yes | frame, with all its channels |
| Zeiss CZI | yes | subblock (plane) |
| everything else | no | – |

Other formats, Thermo RAW included, fail as corrupt (exit 4) while they grow. When such a file changed within the live window, the error message says it may still be being written. Multi-file OME-TIFF sets, multi-file CZI documents and LIF files are read as they are, with no growth reported.

```bash
openreadout info run.nd2 --json | jq .data.acquisition
```

```json
{
  "state": "in_progress",
  "complete_planes": 14,
  "expected_planes": 30,
  "modified_ago_s": 0.4,
  "window_s": 300.0,
  "missing": ["chunk map"],
  "tail_bytes": 4096,
  "evidence": [
    "no chunk map at the end of the file (NIS-Elements writes it last); chunks recovered by scanning",
    "frame chunk 7 is still being written",
    "modified 0.4 s ago, within the 300 s live window"
  ]
}
```

- `state` is `in_progress` while the file is being written. It is `interrupted` when the file shows the same signs but has not changed for longer than the live window: an acquisition or copy that stopped.
- `complete_planes` counts planes whose data is entirely on disk. `expected_planes` is what the finished file will hold, when the metadata written so far says so.
- `missing` lists the end-of-file structures not written yet. `tail_bytes` counts the bytes after the last complete plane.

The block appears only when a file is not finished.

A file in progress is not corrupt, so these commands exit 0. `check` turns its findings about the unfinished tail into warnings, and its first finding is `acquisition_in_progress`. Its `ok` then covers only the complete part, so run `check` again after the acquisition ends and before you archive the file. An `interrupted` file keeps its errors and exits 4.

### How a growing file is recognised

A file counts as in progress when all three hold:

1. Only the structures the instrument software writes last are missing, and everything before them is intact. Damage before the last plane means damage, not growth.
2. The bytes after the last complete plane fit one write in progress.
3. The file was modified within the live window: 300 seconds by default. Change it with `--live-window SECONDS` on any command or the `OPENREADOUT_LIVE_WINDOW` environment variable; `0` turns detection off. Up to 60 seconds in the future is accepted, for clock skew between the instrument PC and a share.

When 1 and 2 hold but 3 does not, the file is `interrupted`. A finished file that was cut short, such as a truncated copy, still points past its own end at a directory or page. Where the format records such pointers (CZI, OME-TIFF), this tells a truncated copy apart from a growing file.

Known limits:

- A truncated copy of an ND2 looks in progress for the length of the window, because ND2 has no pointer that would give it away. After the window it is `interrupted`.
- A time-lapse with frames further apart than the window looks `interrupted` between frames. Raise `--live-window` for such acquisitions.
- A writer that fills in pointers before the data they point to makes a growing file look truncated. This errs on the side of calling a file damaged.
- Until an OME-TIFF's OME-XML is written, its pages are read as the Z planes of one image.
- A CZI mosaic plane counts as complete once one of its tiles is written.
- A Zarr array counts as growing only while its stored chunks are a prefix in C order. Downsampled pyramid levels are not tracked.
- A growing ND2 has no chunk map yet, so each look at it scans the whole file. On files of tens of gigabytes this is slow.

## Watch a folder

`openreadout watch` prints one JSON line for each new data set, plane, scan or sweep under a directory:

```bash
openreadout watch /data/incoming                      # until Ctrl-C
openreadout watch /data/incoming --once               # what is there now, then exit
openreadout watch /data/incoming --qc --since 1h      # with QC; data sets touched in the last hour
openreadout watch /data/incoming --qc-rules lab.toml --stall-after 300 --timeout 3600
```

Each line is a [JSON wrapper](../getting-started/reading-json.md#the-json-wrapper) whose `data` is one event. `openreadout self schema watch` prints its JSON Schema.

| event | when |
| --- | --- |
| `dataset_new` | a data set can be read for the first time |
| `plane_new` | a new complete image plane (`image`, `c`, `z`, `t`) |
| `scan_new` | a new mass spectrum |
| `frame_new` | a new sweep of a trace (electrophysiology sweeps, NMR FIDs) |
| `dataset_complete` | nothing is missing, and the file did not change for one poll |
| `dataset_stalled` | in progress but not grown for `--stall-after` seconds (default 120) |
| `qc` | a [QC rule](#qc-rules) fired |
| `error` | a recognised data set cannot be read |

Every event also has `seq` (increasing by one), `ts` (UTC time), `path` and `format`.

- `watch` polls; it does not rely on file-system notifications. Notifications do not see writes that another machine makes to an SMB or NFS share. `--interval` sets the time between polls (default 0.25 seconds).
- Each poll checks the size and modification time of every file. Only data sets that changed, or that are still in progress, are opened, read-only, and closed again. No file stays open or locked between polls.
- `--since` takes `all`, a duration (`90s`, `10m`, `2h`, `1d`) or an ISO-8601 time. The default is the live window, so running acquisitions are picked up and old files stay quiet. With `--once` the default is `all`.
- `--max-tracked N` bounds memory (default 100,000 data sets); finished data sets are forgotten first.
- `--once`, `--timeout SECONDS`, Ctrl-C or a closed pipe stop it. Each line printed is complete, and the exit code is 0.

On a network share, add the share's attribute-cache time to the delay; SMB and NFS clients often cache file sizes for about a second.

## QC rules

`watch --qc` checks the built-in rules on every new plane or scan. `--qc-rules FILE` uses your own rules instead; `watch --print-qc-rules` prints the defaults to start from. A rule names a metric and a bound. A value outside the bound gives a `qc` event with the rule and the measured value.

```toml
[[rule]]
name = "saturation"
technique = "imaging"            # imaging, mass_spectrometry or any
metric = "saturated_fraction"
max = 0.01
description = "more than 1% of the plane's samples sit at the detector's maximum"

[[rule]]
name = "focus_drift"
technique = "imaging"
metric = "sharpness_ratio"
min = 0.5
baseline = 3                     # planes per image and channel that form the baseline

[[rule]]
name = "dropped_frames"
technique = "imaging"
metric = "interval_ratio"
max = 1.8
baseline = 3                     # intervals before the rule is armed

[[rule]]
name = "tic_drop"
technique = "mass_spectrometry"
metric = "tic_ratio"
min = 0.2
baseline = 5                     # MS1 scans that form the baseline
```

The metrics:

- `saturated_fraction`, per plane: the fraction of samples at the detector's maximum. The maximum is 2^bits − 1 when the file records its bit depth, else the pixel type's maximum. Not computed for floating-point images.
- `sharpness_ratio`, per plane: the variance of the Laplacian of the plane, divided by the median of the first `baseline` planes of the same image and channel. A drop suggests focus drift, but it also drops when the sample moves out of view or the signal fades.
- `interval_ratio`, per time point: the time between the arrival of this time point and the last, divided by the median of the earlier gaps. It catches dropped or delayed frames. It uses arrival time as `watch` sees it, so a pause in the writer or the share counts too.
- `tic_ratio`, per MS1 scan: the total ion current divided by the median of the first `baseline` MS1 scans.

QC reads each new plane or spectrum, so `--qc` costs one plane read per new plane.

## Personal data

An institution's storage holds personal data in many places: operator names in headers, patient-like identifiers typed into sample names, e-mail addresses in comments. `index` checks every string of the `info` output and records a flag per field with its kind and rule, but not the value. Flags are in `experiments.parquet` (`pii_count`, `pii_kinds`, `pii_fields`), in `problems.parquet` (`category = pii`), in `index.json` and in the health report.

The rules favour precision over recall. Each needs a field whose name says it holds a person, a keyword next to the value, or a pattern that is unambiguous on its own. A name typed into a neutrally named field, such as `notes`, with no keyword, is not found.

Kind `person_name`:

- `operator_field`: a field named like a person (operator, owner, experimenter, user name, login, author, technician, acquired by, created by, …) that holds a plausible name or login. Placeholders and shared accounts (`admin`, `user`, `guest`), values with role or group words (`TOF-User`, `Neumann_Group`), the instrument's own name, values of one or two letters and short codes with digits are not flagged.
- `operator_name_in_sample`: a sample field (sample id, name, well, barcode, image or table name) that contains a word of an operator name flagged in the same file.
- `operator_name_in_field`: any other field that contains every part of a flagged operator name, such as an experiment named `Jane Roe 2013-02-28 Fortessa` when the operator is `JaneRoe`.
- `patient_name`: `patient`, `patient name` or `subject name` followed by two capitalised words, in a sample or patient field.

Other kinds:

- `email_address` (kind `email`): `local@domain.tld` anywhere.
- `phone_number` (kind `phone`): `+` followed by 10 to 15 digits, North American forms such as `(555) 019-9922`, or 7 to 15 digits in a field named phone, tel, fax or mobile. Dates, times, versions and IP addresses are not phone numbers.
- `mrn_keyword` (kind `patient_id`): `MRN`, `patient id`, `medical record number`, `NHS number` and similar keywords followed by an identifier with at least 3 digits, or such an identifier in a field named like a patient or record id.
- `dob_keyword` (kind `date_of_birth`): `DOB`, `date of birth`, `born`, `Geburtsdatum` and similar keywords followed by a date, or a date in a field named like a birth date.
- `comment_field` (kind `free_text`): a comment, description, remark or memo field (and FCS `$COM`) with three or more words. It may contain anything, so it is flagged for review.

Strings that OpenReadout writes itself, such as paths, format ids, reader notes and term labels, are not checked.

### Redaction

`search --export --redact` and `info --view full --redact` replace every flagged value, and every other string that contains it, with `redacted:` and 16 hex digits of a keyed hash (HMAC-SHA256) of the value. The same value gets the same replacement in every file and every run, so records stay linkable without the name.

The key is a salt that you provide, either with `--salt-file FILE` (at least 8 bytes) or the `OPENREADOUT_REDACT_SALT` environment variable. There is no option that takes the salt itself on the command line, so it can't end up in your shell history. OpenReadout doesn't print, log or store it. `--redact` without a salt is a usage error (exit 2).

```bash
head -c 32 /dev/urandom | base64 > ~/.openreadout-salt && chmod 600 ~/.openreadout-salt
```

In an export, redaction covers the metadata JSON, the datasheet and the OME-Zarr metadata files. Pixel data is not touched.

## Agents

The MCP tools follow the same steps. `openreadout_index` crawls in capped sessions; call it again with the same arguments while it answers `complete: false`. `openreadout_search` returns the search results, and its `total` answers questions such as "how many CZI files in this share were acquired with a 63x objective?". `openreadout_watch` returns the events since a cursor; pass the returned `cursor` on the next call. See [MCP tools](../reference/mcp.md).

## Limits

- Paths are stored as UTF-8. Names that are not valid UTF-8 are converted lossily, the same way in every run.
- A directory data set lists at most 10,000 member files. Its size and file count cover all of them.
- The unchanged test uses size and modification time. A file rewritten in place within the same microsecond at the same size is not read again; use `--full-rescan`.
- "Same experiment stored twice" needs an acquisition time or matching file stems, plus identical dimensions. Two exports resampled to different sizes are not grouped.
- Remote storage (S3, HTTP) is not read directly. Mount the share instead.
- `watch` QC does not yet cover stage drift, lock-mass drift, spray instability, pressure excursions or plate edge effects.
