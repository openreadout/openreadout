# health

`health` reports on the storage of a share indexed by [`index`](index-cmd.md): truncated or corrupt files, unreadable formats, duplicates, the same experiment stored twice, files at risk, totals and personal data.

```text
openreadout health [OPTIONS] <INDEX_DIR>
```

MCP: `openreadout_health`.

## Flags

- `--no-hash`: do not confirm duplicate candidates by hashing their content. No file is read.
- `--max-list N`: most entries listed per section. Default 100. Counts are always complete.
- `-o`, `--output FILE`: also write the report as Markdown.
- `--json`: print the JSON wrapper instead of text.

## Example

```bash
openreadout index /lab/share -o /lab/index
openreadout health /lab/index -o health.md
```

The [lab share guide](../../guides/lab-shares.md) explains each section of the report.

## JSON

[`health`](../json/health.md).

Run `openreadout health --help` for the full help of your installed version.
