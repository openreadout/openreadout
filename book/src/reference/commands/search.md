# search

`search` queries an index made by [`index`](index-cmd.md), reports its storage health, or exports what a query selects as a dataset.

```text
openreadout search [OPTIONS] <INDEX_DIR> [QUERY]
```

## Query syntax

Terms are combined with AND; `OR` separates alternatives.

- `field=value`, `field!=value`: equal, not equal.
- `field~text`: contains.
- `field<v`, `<=`, `>`, `>=`: compare numbers, sizes and dates (`size>1GB`, `acquired<2020`).
- `field:TERM_ID`: match an ontology term, such as `technique:FBbi_00000246`.
- `a|b`: either value.
- `-term`: negate a term.
- Bare words search paths, samples, channels and descriptions.

An empty query selects everything. The fields are the index columns listed in the [lab share guide](../../guides/lab-shares.md).

## Flags

### Results

- `--sort FIELD`: sort by a field; `-size` sorts descending. Default: path order.
- `--limit N`: most results, 0 = all. Default 50. With `--export`, most data sets exported (default all).
- `--fields FIELDS`: fields to return, comma-separated, or `all`.
- `--jsonl`: one JSON object per result and line, without the wrapper.
- `--json`: print the JSON wrapper instead of text.

### Storage health (`--health`)

- `--health`: report on the whole index instead of results: truncated or corrupt files, unreadable formats, duplicates, the same experiment stored twice, files at risk, totals and personal data.
- `--no-hash`: do not confirm duplicate candidates by hashing their content.
- `--max-list N`: most entries listed per section. Default 100. Counts are always complete.
- `-o`, `--output FILE`: also write the report as Markdown.

### Dataset export (`--export`)

- `--export OUT`: export the selected data sets into this directory: images as OME-Zarr, tables, traces and spectra as Parquet (or CSV), metadata JSON and a datasheet. Resumable and verified.
- `--tables parquet|csv`: file format for tables, traces and spectra. Default `parquet`.
- `--redact`: replace values flagged as personal data with stable salted hashes.
- `--salt-file FILE`: file holding the redaction salt (or set `OPENREADOUT_REDACT_SALT`).
- `--license SPDX`: license of the whole export. Default: the nearest LICENSE file of each source.
- `--no-images`: leave images out.

## Examples

```bash
openreadout search /lab/index "format=czi objective=63x acquired<2020" --sort -size
openreadout search /lab/index "status=truncated|corrupt" --jsonl --fields path,format,check_codes
openreadout search /lab/index --health -o health.md
openreadout search /lab/index "sample~A12" --export dataset/ --redact --salt-file ~/.salt
```

Redaction and the personal-data rules: [Personal data](../../guides/lab-shares.md#personal-data).

## JSON

[`search`](../json/search.md), [`search --health`](../json/search-health.md), [`search --export`](../json/search-export.md).

Run `openreadout search --help` for the full help of your installed version.
