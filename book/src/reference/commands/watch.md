# watch

`watch` follows directories where an instrument is writing data. It prints one JSON line per new data set, plane, scan or sweep, per completed or stalled data set, per QC finding and per error.

```text
openreadout watch [OPTIONS] [DIR]...
```

`watch` polls. It opens files read-only and never locks them. Sub-directories are walked, and a directory data set (a Zarr store, a Bruker `.d`) counts as one data set.

## Flags

- `--interval SECONDS`: seconds between polls. Each poll re-opens only the files that changed. Default 0.25.
- `--once`: look once, print events for what is there now, and exit.
- `--since WHEN`: only report data sets modified since then: a duration (`90s`, `10m`, `2h`, `1d`), an ISO-8601 time, or `all`. Default: the live window back from now; with `--once`, `all`.
- `--stall-after SECONDS`: report a data set that stops growing for this long as `dataset_stalled`. Default 120.
- `--qc`: evaluate the built-in QC rules on every new plane or scan: saturation, focus drift, dropped frames, TIC drop.
- `--qc-rules FILE`: QC rules from a TOML file instead of the built-in ones. Implies `--qc`.
- `--print-qc-rules`: print the built-in QC rules as TOML and exit.
- `--timeout SECONDS`: stop after this many seconds. Default: run until Ctrl-C.
- `--max-tracked N`: most data sets remembered at once. Finished ones are forgotten first. Default 100000.
- `--json`: accepted for consistency with other commands. The output is always JSON lines.

The global `--live-window` flag sets how recently a file must have changed to count as still being written.

## Examples

```bash
openreadout watch /data/incoming --qc                # JSON lines until Ctrl-C
openreadout watch /data/incoming --once              # what is there now
openreadout watch --print-qc-rules > rules.toml      # start your own rules from the defaults
openreadout watch /data/incoming --qc-rules rules.toml
```

Event types, the in-progress heuristic and the rule format are described in the [lab share guide](../../guides/lab-shares.md), under [QC rules](../../guides/lab-shares.md#qc-rules).

## JSON

[`watch`](../json/watch.md).

Run `openreadout watch --help` for the full help of your installed version.
