# link

`link` groups files that measured the same sample across instruments and formats, and gives the evidence and a confidence for every link. It reads headers only.

```text
openreadout link [OPTIONS] [PATH]...
```

Evidence includes sample ids, barcodes, plate wells, conversions that name their source file, and the same acquisition stored twice.

## Flags

- `--no-recursive`: only the top level of directory arguments. By default directories are walked.
- `--min-confidence low|medium|high`: links below this confidence are listed but do not join groups. Default `medium`.
- `--max-shared N`: an identifier shared by more data sets than this counts as weak evidence. Default 12.
- `--formats IDS`: only data sets of these format ids.
- `--from-index INDEX_DIR`: add the data sets an [index](index-cmd.md) selects.
- `--query QUERY`: the index query for `--from-index`.
- `--json`: print the JSON wrapper instead of text.

## Examples

```bash
openreadout link share/
openreadout link share/ --min-confidence high --json
openreadout link --from-index /lab/index --query "sample~P0042"
```

How the evidence is weighed: [Batch tables guide](../../guides/batch.md).

## JSON

[`link`](../json/link.md).

Run `openreadout link --help` for the full help of your installed version.
