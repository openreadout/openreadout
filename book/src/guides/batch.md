# Many files: batch tables and sample sheets

Most questions span a folder: mean intensity per condition, median CD4 per donor, which wells failed. OpenReadout answers them in one command. It measures every file, joins the result to your sample sheet or plate layout, and summarizes it by group.

Here are four images, two per condition, and a sample sheet that names them:

```text
file,condition
well_A1.nd2,DMSO
well_A2.nd2,DMSO
well_B1.czi,drug
well_B2.czi,drug
```

```bash
openreadout stats plate/ --sample-sheet samples.csv --by condition
```

```text
4 data sets (4 ok, 0 failed)
summary by condition, channel_name of mean, median (4 rows used, 0 left out)
condition  channel_name  value   n   mean  sd  sem  median    min    max  cv_percent
─────────  ────────────  ──────  ─  ─────  ──  ───  ──────  ─────  ─────  ──────────
DMSO       DAPI          mean    2  12.75   0    0   12.75  12.75  12.75           0
DMSO       DAPI          median  2    8.5   0    0     8.5    8.5    8.5           0
drug       c0            mean    2     35   0    0      35     35     35           0
drug       c0            median  2     35   0    0      35     35     35           0
samples.csv joined on `file` = path: 4/4 rows, 4/4 data sets annotated (added: condition)
```

OpenReadout found the join key itself and reported it on the last line: the sheet's `file` column matches the file names. The channel name became a group column on its own. The two conditions were imaged with differently named channels, and a mean never mixes two channels.

## Several files at once

`info`, `check`, `stats` and `export` accept several files, directories and glob patterns:

```bash
openreadout check -r /data/2026-09-21                          # one report per file, then a summary
openreadout info -r --jsonl --skip-unknown /data > info.jsonl  # one JSON object per line
openreadout info --json "/data/**/*.nd2"                       # quoted: openreadout expands the glob
openreadout export -r --skip-unknown raw/ -o ome/              # mirrors raw/ under ome/
```

- Directories are walked in name order. `-r` also walks sub-directories. Hidden entries are skipped.
- A directory that is itself a data set, such as a ChemStation `.D`, a Waters `.raw` or a Bruker `.d`, is one input.
- A failed file does not stop the run. `--fail-fast` stops at the first failure.
- The exit code is the worst code of any input, so `check -r DIR` exits 4 when any file is corrupt. See [Exit codes](../reference/commands/index.md#exit-codes).
- `--skip-unknown` leaves out files that are not instrument data, such as READMEs and spreadsheets.

```text
path                         format       contents     status
───────────────────────────  ───────────  ───────────  ───────────
/data/2026-09-21/a.nd2       nd2          intact       ok
/data/2026-09-21/cut.czi     czi          4 findings   corrupt (4)
/data/2026-09-21/run.D       chemstation  intact       ok
3 inputs: 2 ok, 1 failed
```

Each line of the `--jsonl` output is one [JSON wrapper](../getting-started/reading-json.md#the-json-wrapper) with a `path` field. It loads into pandas directly:

```python
import json, pandas as pd
rows = [json.loads(line) for line in open("info.jsonl")]
df = pd.json_normalize([
    {"path": r["path"], **img} for r in rows if r["ok"] for img in r["data"]["images"]
])
```

## One table from many files

`info`, `stats`, `trace`, `table` and `analyze gate` can return one tidy table for all inputs instead of one report per file. Ask for it with `--tidy`. Any of `--sample-sheet`, `--by`, `--where`, `--fields`, `-o`, `--csv` or `--from-index` turns it on as well. For `trace`, `table` and `analyze gate`, several inputs, a directory or a glob is enough.

```bash
# median FITC-A per condition
openreadout table runs/ --sample-sheet samples.csv --where parameter=FITC-A --by condition --value median

# mean intensity per condition and channel; wells averaged first; Welch's t-test against DMSO
openreadout stats plate/ -r --sample-sheet layout.xlsx --by condition --replicate well \
    --test welch --control DMSO -o per_well.parquet

# population counts and median fluorescence per FCS file
openreadout analyze gate fcs/*.fcs --workspace analysis.wsp --median Comp-FITC-A -o populations.csv

# header metadata of a whole share, one row per data set
openreadout info -r share/ --tidy --fields sample_id,instrument_model,started_at --csv
```

### Rows and columns

Each row is one data set times the measure's grain:

| command | one row per | values |
| --- | --- | --- |
| `stats` | image × channel; `--per image`, `plane`, `well` or `field` changes it | `mean`, `median`, `min`, `max`, `std`, percentiles, saturated fraction |
| `trace` | trace × sweep × channel | `mean`, `max`, `min`, `std`, `duration_s` |
| `table` | FCS parameter, or plate well × read | FCS: `median`, `mean`, `sd`; plates: `value` |
| `analyze gate` | population | `count`, `percent_of_parent`, `median:<parameter>` |
| `info` | data set | the columns of the [index](lab-shares.md#experimentsparquet) |

Columns come in a fixed order. `path`, `format` and `files` are first. Then come the keys (image, channel, parameter, well), what the file records about the sample, your sample sheet's columns, the values, and finally `error` and `error_code`. Names are lower case and stable. A unit that never changes is part of the name (`duration_s`, `wavelength_nm`). The JSON output lists every column with its type, unit and description.

A file that cannot be read becomes a row with `error` and `error_code` set, and the run carries on. Files that belong together, such as a multi-file OME-TIFF or a SpikeGLX `.meta` and `.bin`, are measured once, and the `files` column counts them. Your sample sheet and the output file are never treated as inputs.

Narrow the table with these flags:

- `--where COLUMN=VALUE` (or `!=`, repeatable) keeps matching rows.
- `--fields` keeps only some columns.
- `--formats fcs,czi` keeps only those formats.
- `--from-index INDEX --query "..."` takes the inputs from a [lab-share index](lab-shares.md).

### Output

- No flag: a text table of the first 40 rows, the join report and notes.
- `--csv`: CSV on standard output.
- `--jsonl`: one object per row and line.
- `--json`: the JSON wrapper with `columns`, `rows`, `summary` and `joins`. `--limit` and `--offset` page the rows.
- `-o FILE`: `.csv`, `.tsv`, `.jsonl`, `.json` or `.parquet`. With `--by`, the file holds the summary. Parquet keeps each column's unit and description.

`-o` writes to a temporary name, reads the file back, then renames it into place. While it runs it keeps a journal next to the output. If a run is interrupted, run the same command again: files that have not changed are not measured a second time.

## Analyses over a folder

`openreadout batch MEASURE INPUTS…` runs any measure over many files with the same table flags. The measures are `stats`, `trace`, `table`, `info`, `spectra` and the analyses `peaks`, `chromatogram`, `assay`, `nmr-peaks`, `ephys-features`, `spikes`, `qpcr` and `gate`.

Pass options by name with `--set KEY=VALUE`, or all at once with `--options '{...}'`. A value is read as JSON when it parses. The option names are those of the matching MCP tool. The `rows` option picks which records become rows: for example `peak`, `compound` or `chromatogram` for `peaks`, and `sweep`, `cell` or `spike` for `ephys-features`. Each row holds the same numbers as the single-file `analyze` output.

```bash
openreadout batch peaks runs/ -r --set rows=chromatogram --sample-sheet samples.csv --by condition
openreadout batch ephys-features cells/ -r --set rows=cell --sample-sheet genotypes.csv --by genotype
openreadout batch qpcr plates/ --set ddcq=true --set 'reference_targets=["18s"]' --by treatment
openreadout batch stats plate/ --set per=well --sample-sheet layout.csv
```

The analyses themselves are described in [Recipes](../recipes/index.md).

## Sample sheets and plate layouts

`--sample-sheet FILE` (also `--samples` or `--layout`, repeatable) reads two kinds of file.

A **sample sheet** is a table with a header row. It can be CSV, TSV or another delimited text file, or a worksheet of an XLSX, XLS, XLSB or ODS workbook. `--worksheet NAME` picks the worksheet. A sheet with `Row` and `Column` number columns gets a `well` column.

A **plate layout** (plate map) holds one grid per annotation. The header row numbers the plate columns from 1, and each following row starts with a plate-row letter. The grid's top-left cell names the annotation; if it is empty, the line above the grid or the worksheet name does. Several grids can sit one under another or on separate worksheets. 96-, 384- and 1536-well plates are read.

```text
condition,1,2,3
A,DMSO,DMSO,drug
B,DMSO,drug,drug
```

### How rows are matched

OpenReadout tries each sheet column against several keys of every data set:

- the path, file name or file stem
- the sample id, name, barcode or vial the file records (see the [experiment block](metadata.md))
- the plate well (`A1`, `A01` and `R1C1` are the same well)
- the run order, from the acquisition start time

The pair that matches the most rows wins. When rows still match several sheet rows, a second column joins the key; plate barcode plus well is the usual case. Sheet values with `*` or `?` are patterns (`ctrl_*.fcs`). To set the key yourself, use `--key SHEET_COLUMN=FIELD`, and repeat it for a composite key.

Nothing is dropped silently. The join report names:

- the key chosen and the runners-up
- the columns added
- data sets that no sheet row matched
- sheet rows that matched no data set, which is often a misnamed file
- rows that matched conflicting sheet rows; these are left without annotations

A sheet that matches nothing is a usage error (exit 2). Its message shows the sheet's columns and the first file names.

Sheet columns whose values are all numbers are typed as numbers, so `dose` sorts numerically.

Wells of high-content screening images need no extra work. `stats` rows carry the well when the reader records one, and a plate layout joins onto it.

## Group summaries

`--by condition[,dose]` adds one summary row per group and value column. Each row has `n`, `mean`, `sd`, `sem`, `median`, `min`, `max` and `cv_percent`. `--value` picks the value columns; the default is the measure's main values.

Different measurements are never pooled. Columns that say *what* was measured, such as channel, FCS parameter, population, trace or wavelength, are added to the groups whenever they vary. `--exact-by` turns this off. Images, wells, sweeps and files are pooled.

- `--replicate COLUMN` averages each replicate (a well, a biological replicate) first, so `n` counts replicates.
- `--test welch` or `--test mann-whitney`, with `--control VALUE`, compares every group with the control group. It adds `diff`, `ratio`, `statistic`, `df` and a two-sided `p_value`. The tests match SciPy's `ttest_ind(equal_var=False)` and `mannwhitneyu`. P-values are not corrected for multiple comparisons.

Rows with an error, rows filtered out and rows without a value are left out and counted.

To regroup a table you saved earlier (CSV, TSV, JSON Lines, JSON or Parquet), use `batch summarize`:

```bash
openreadout batch summarize per_well.parquet --by condition,dose --value mean
```

## Finding the same sample across instruments

`openreadout link DIR…` reads the headers of every data set and groups the files that measured the same sample. Typical pairs are a Thermo `.raw` file and its mzML conversion, or two instruments' files with the same plate barcode.

```bash
openreadout link share/ --json
```

Every link names its evidence and a confidence:

- High: the same barcode; the same plate barcode and well; the same recorded sample id; one file names the other as its source; the same acquisition start, run length and instrument; the same sequence of mass spectra.
- Medium: the same instrument serial, scan count and run length; the same sample name where no id is recorded; the same file stem in two formats.
- Medium or low: one file's sample id appears in the other's file name.

A well alone never links two files, because wells repeat on every plate. Identifiers that name controls (`blank`, `QC`, `pool`, `standard`) count as weak evidence. So do identifiers that more than `--max-shared` data sets share (default 12). Links at or above `--min-confidence` (default `medium`) form groups; weaker ones are listed separately. A group whose files record different sample ids is reported as a conflict.

## Shell loops

Branch on the exit code, not on the text:

```bash
for f in /data/*; do
  openreadout check "$f" --json > "reports/$(basename "$f").json"
  case $? in
    0) ;;                                        # intact
    3) rm "reports/$(basename "$f").json" ;;     # not an instrument file
    4) echo "CORRUPT $f" ;;
    6) echo "cannot read yet: $f" ;;
    *) echo "failed: $f" ;;
  esac
done
```

The repository has three ready-made scripts in [`examples/shell/`](https://github.com/openreadout/openreadout/tree/main/examples/shell). They need `jq` and use `$OPENREADOUT` as the binary when it is set.

- `batch_summarize.sh DIR` prints one tab-separated line per file: format, image count, dimensions, pixel type, pixel size and acquisition time.
- `triage_corruption.sh DIR` prints `OK`, `CORRUPT` or `UNSUPPORTED` per file with the findings, and exits 1 when anything is damaged. Run it on a copy before deleting the original from the acquisition PC.
- `export_planes.sh OUT FILES…` exports one plane per image (channel 0, middle z, first time point) and checks that each export was verified. `CHANNEL=1 TO=ome-zarr` changes the channel and the format.

From Python, `openreadout.batch()` (with `"summarize"` for `batch summarize`) and `openreadout.link()` take the same options as the commands. See the [Python guide](python.md).

## Validation

The table measures, sample-sheet reading, group statistics and `link` are checked against independent readers and SciPy on the test corpus. The method and results are on the [validation page](../project/validation.md).
