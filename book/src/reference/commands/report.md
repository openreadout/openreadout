# report

`report` writes a privacy-reviewed diagnostic bundle for a file that OpenReadout refuses, fails on, or does not validate. It lets you report such a file without sharing the file.

```text
openreadout report [OPTIONS] <FILE>
```

MCP: `openreadout_report` with `file`, `output` and `include_text`.

## Flags

- `--json`: print the JSON wrapper instead of text.
- `-o`, `--output PATH`: where to write the bundle. Default: `openreadout-report-<hash>.json` in the current directory.
- `--dry-run`: print the bundle and write nothing.
- `--overwrite`: replace an existing bundle.
- `--include-text`: keep free text from the file (sample, image and channel names, comments).
- `--hex N`: add hex excerpts of N bytes (at most 256) of the file head and of up to 32 structure headers.
- `--full-check`: run the full integrity check instead of headers only.
- `--no-hash`: leave out the file's SHA-256.
- `--no-first-read`: do not decode a first plane, sweep, spectrum or table rows.

## What the bundle holds

The bundle holds the file's assurance fingerprint, every decode stage with its error, the structure map, and the metadata with free text replaced. It contains no pixel, spectral, trace or table values and no path. OpenReadout doesn't send it anywhere. Attach it to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml). Read the bundle first with `--dry-run` if you want to see exactly what it contains.

## JSON

[`report`](../json/report.md).

Run `openreadout report --help` for the full help of your installed version.
