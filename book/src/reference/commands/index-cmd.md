# index

`index` catalogs every instrument data set under one or more directories into files you own: Parquet tables plus `index.json`. It reads headers only, works in parallel, and can be resumed and rerun to update.

```text
openreadout index [OPTIONS] --output <INDEX_DIR> <DIR>...
```

The index directory holds `index.json`, `experiments.parquet`, `files.parquet`, `problems.parquet` and the crawl state. Query it with [`search`](search.md).

## Flags

- `-o`, `--output INDEX_DIR`: the index directory. Created if missing. It must not be inside a crawled directory.
- `--check headers|full|none`: integrity check per data set. Default `headers`.
- `--follow-symlinks`: follow symbolic links. A directory reached twice is crawled once.
- `--exclude GLOB`: skip entries whose name or relative path matches. Repeatable, for example `--exclude '*.tmp' --exclude 'scratch/**'`.
- `--max-files N`: stop after this many files in this session. Rerun to continue.
- `--max-seconds S`: stop after this many seconds in this session. Rerun to continue.
- `--restart`: discard an interrupted crawl instead of resuming it.
- `--full-rescan`: read every file again instead of reusing unchanged records.
- `--no-pii`: do not look for personal data.
- `--health`: also print the storage health report, as `search --health` does.
- `--json`: print the JSON wrapper instead of text.

The global `--threads` flag sets how many files are read in parallel.

## Examples

```bash
openreadout index /lab/share -o /lab/index                  # rerun to update or to resume
openreadout index /lab/share -o /lab/index --exclude 'scratch/**' --max-seconds 3600
openreadout index /lab/share -o /lab/index --health
```

The [lab share guide](../../guides/lab-shares.md) walks through a crawl and lists every column of the index tables.

## JSON

[`index`](../json/index-manifest.md).

Run `openreadout index --help` for the full help of your installed version.
