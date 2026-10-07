# summarize

`summarize` computes group statistics of a table written earlier by [`batch`](batch.md) `-o` or by `--tidy -o`, or of any CSV, TSV, JSON Lines, JSON or Parquet table: n, mean, sd, sem, median, min, max and CV % per group.

```text
openreadout summarize [OPTIONS] --by <COLUMNS> <TABLE>
```

MCP: `openreadout_summarize`.

## Flags

- `--by COLUMNS`: summarize by these columns, comma-separated or repeated. Channels, parameters and populations stay apart unless `--exact-by`.
- `--value COLUMNS`: value columns to summarize. Default: the measure's main values.
- `--replicate COLUMN`: average the rows of each replicate before summarizing; n then counts replicates.
- `--test welch|mann-whitney`: compare each group with the control.
- `--control VALUE`: the control group, a value of the first `--by` column.
- `--where COLUMN=VALUE`: keep only matching rows (`!=` to leave them out). Repeatable.
- `--exact-by`: group by exactly `--by`, without adding the measurement columns that vary.
- `-o`, `--output FILE`: also write the summary as `.csv`, `.tsv`, `.jsonl`, `.json` or `.parquet`. The write is verified.
- `--overwrite`: replace an existing output file.
- `--csv`: print the summary as CSV.
- `--json`: print the JSON wrapper instead of text.

## Example

```bash
openreadout batch peaks runs/ -r --sample-sheet samples.csv -o rows.parquet
openreadout summarize rows.parquet --by condition --test welch --control DMSO
```

The [Batch tables guide](../../guides/batch.md) explains group summaries and tests.

## JSON

[`summarize`](../json/summarize.md).

Run `openreadout summarize --help` for the full help of your installed version.
