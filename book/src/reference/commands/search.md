# search

`search` queries an index made by [`index`](index-cmd.md). For the storage health of the indexed share, see [`health`](health.md); to export what a query selects as a dataset, see [`export-dataset`](export-dataset.md).

```text
openreadout search [OPTIONS] <INDEX_DIR> [QUERY]
```

MCP: `openreadout_search`.

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
- `--limit N`: most results, 0 = all. Default 50.
- `--fields FIELDS`: fields to return, comma-separated, or `all`.
- `--jsonl`: one JSON object per result and line, without the wrapper.
- `--json`: print the JSON wrapper instead of text.

## Examples

```bash
openreadout search /lab/index "format=czi objective=63x acquired<2020" --sort -size
openreadout search /lab/index "status=truncated|corrupt" --jsonl --fields path,format,check_codes
```

## JSON

[`search`](../json/search.md).

Run `openreadout search --help` for the full help of your installed version.
