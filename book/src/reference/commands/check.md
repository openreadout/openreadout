# check

`check` validates a file's integrity and exits 4 if it is corrupt or truncated. To hash every plane, see [`planes`](planes.md). To compare a file with its export, see [`compare`](compare.md). To report a file OpenReadout cannot read, see [`report`](report.md).

```text
openreadout check [OPTIONS] <FILE>...
```

MCP: `openreadout_check` with `file` and `headers_only`.

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--headers-only`: check headers and structure only (offsets, declared sizes, missing parts), without decoding data. This is what `index` runs on every data set.

`check` takes the flags in [Several inputs](index.md#several-inputs).

## Examples

```console
$ openreadout check doctor.tif
doctor.tif (tiff): OK
  checks performed:
    - TIFF/BigTIFF header: byte order, version 42/43, first IFD offset
    - IFD chain: every directory lies inside the file, no loops, chain terminates
    …
```

Check a whole run, one JSON line per file. The exit code is the worst code of any file.

```bash
openreadout check -r --jsonl /data/run42 > report.jsonl
```

## Files still being written

On an OME-TIFF, OME-Zarr, ND2 or CZI that an instrument is still writing, `check` covers the complete planes, reports an `acquisition` block, and exits 0. See [Live acquisition](../../guides/lab-shares.md).

A copy that was cut short can look the same while it is new. A file modified within the live window (300 seconds by default) counts as still being written, and an ND2 has no pointer that tells a truncated copy from a growing file. So checking a copy right after making it can exit 0 with `acquisition.state: in_progress` instead of 4. To check a finished copy, turn detection off:

```bash
openreadout check copy.nd2 --live-window 0    # exits 4 if the copy is truncated
```

## JSON

[`check`](../json/check.md).

Run `openreadout check --help` for the full help of your installed version.
