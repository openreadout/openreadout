# batch

`batch` computes any measure over many files as one tidy table, joined to sample sheets and summarized by group.

```text
openreadout batch [OPTIONS] <MEASURE> [INPUT]...
openreadout batch summarize [OPTIONS] <TABLE>
```

`MEASURE` is one of:

- `stats`, `trace`, `table`, `info`: the same rows as those commands in table mode.
- `spectra`: one row per mass-spectrometry scan header.
- `peaks`, `chromatogram`, `assay`, `nmr-peaks`, `ephys-features`, `spikes`, `qpcr`, `gate`: the [`analyze`](analyze.md) subcommands.
- `summarize`: group statistics of one table written earlier.

Failures are rows with an `error` column, so one bad file does not stop the run, and the command exits 0 once the table is built. `--fail-fast` stops at the first failure and exits with its code. The [Batch tables guide](../../guides/batch.md) explains the row grain of each measure, sample-sheet keys and group summaries.

## Flags

- `--set KEY=VALUE`: one measure option. Repeatable. The names are the option names of the MCP tools (`mz=[195.0877]`, `ppm=10`, `analysis=curve`, `rows=compound`). VALUE is read as JSON when it parses, else as text.
- `--options JSON`: all options as one JSON object. `--set` values are merged over it.
- `--json`: print the JSON wrapper instead of text.

`batch` also takes the flags in [Several inputs](index.md#several-inputs).

## Batch table flags

`batch` and the table mode of `info`, `stats`, `trace`, `table` and `analyze gate` share these flags.

### Table and inputs

- `--tidy`: one tidy table for all inputs. Implied by the other flags in this section, and by several inputs to `trace`, `table` and `analyze gate`.
- `--formats IDS`: only data sets of these format ids (`fcs,czi`). Others are skipped and counted.
- `--from-index INDEX_DIR`: add the data sets that an [index](index-cmd.md) selects.
- `--query QUERY`: the index query for `--from-index`, in [`search`](search.md) syntax.
- `--where COLUMN=VALUE`: keep only rows where COLUMN equals (or with `!=`, differs from) VALUE. Repeatable.
- `--fields COLUMNS`: columns to output. Identity and error columns are always kept.

### Sample sheets

- `--sample-sheet FILE` (alias `--samples`): join a sample sheet (CSV, TSV, XLSX) or plate layout onto the rows. The join key is chosen from the data and reported. Repeatable.
- `--worksheet NAME`: worksheet of an XLSX sample sheet.
- `--key SHEET_COLUMN=FIELD`: set the join key yourself. Fields: `path`, `file`, `stem`, `sample_id`, `sample_name`, `barcode`, `well`, `position`, `run_order`, `column:NAME`. Repeat for a composite key.

### Group summaries

- `--by COLUMNS`: summarize by these columns: n, mean, sd, sem, median, min, max and CV % per group.
- `--value COLUMNS`: value columns to summarize. Default: the measure's main values.
- `--replicate COLUMN`: average the rows of each replicate before summarizing; n then counts replicates.
- `--test welch|mann-whitney`: compare each group with the control.
- `--control VALUE`: the control group, a value of the first `--by` column.
- `--exact-by`: group by exactly `--by`, without adding the measurement columns that vary.

### Output

- `-o`, `--output FILE`: write the table (or, with `--by`, the summary) as `.csv`, `.tsv`, `.jsonl`, `.json` or `.parquet`. The write is verified, and an interrupted run resumes.
- `--overwrite`: replace an existing output file.
- `--csv`: print the table as CSV on stdout.
- `--limit N`: JSON: return at most N rows. Summaries and counts are always complete.
- `--offset N`: JSON: first row returned. Default 0.

## Examples

```console
$ openreadout table doctor.fcs doctor.fcs --csv --fields parameter,median
path,format,parameter,median
doctor.fcs,fcs,FSC-A,45.5
doctor.fcs,fcs,SSC-A,46.5
doctor.fcs,fcs,Time,47.5
…
```

```bash
openreadout batch peaks runs/ -r --set rows=chromatogram --sample-sheet samples.csv -o rows.parquet
openreadout batch assay plates/ --set analysis=curve --set layout=layout.csv -o curves.parquet
openreadout batch summarize rows.parquet --by condition
```

## JSON

[Batch table](../json/batch-table.md), [batch summary](../json/batch-summary.md).

Run `openreadout batch --help` for the full help of your installed version.
