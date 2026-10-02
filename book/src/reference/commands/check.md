# check

`check` validates a file's integrity and exits 4 if it is corrupt or truncated. It can also hash every plane, compare the file with a second file, or write a diagnostic bundle for a bug report.

```text
openreadout check [OPTIONS] <FILE>...
```

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--headers-only`: check headers and structure only (offsets, declared sizes, missing parts), without decoding data. This is what `index` runs on every data set.

`check` takes the flags in [Several inputs](index.md#several-inputs), except with `--against` or `--report`, which take one file.

### Plane hashes (`--planes`)

- `--planes`: read every plane and print its dimensions and xxh3-128 hash.
- `--dump-dir DIR`: also write each plane's raw little-endian samples to `DIR/image<i>_c<c>_z<z>_t<t>.bin`.
- `--region X,Y,W,H`: only this rectangle of each plane, in the pixels of `--level`.
- `--image N`: only this image (also for `--against`).
- `--select SEL`: plane selection such as `c=0`, `z=2-5`, `t=0,3`. Repeatable (also for `--against`).
- `--level N`: pyramid level, 0 = full resolution (also for `--against`).

### Compare with a second file (`--against`)

- `--against FILE`: compare with this file, for example its OME-TIFF export.
- `--tolerance T`: largest absolute sample difference that still counts as equal. Default: planes must be bit-identical.
- `--ignore POINTER`: leave this JSON pointer out of the metadata diff (`*` matches one segment). Repeatable.
- `--include-extra`: also diff the format-specific `extra` objects.
- `--no-metadata`: compare data only.
- `--no-pixels`: compare metadata and geometry only.

### Diagnostic bundle (`--report`)

- `--report`: write a privacy-reviewed diagnostic bundle for a file that was refused, failed, or is not validated.
- `-o`, `--output PATH`: where to write it. Default: `openreadout-report-<hash>.json` in the current directory.
- `--dry-run`: print the bundle and write nothing.
- `--overwrite`: replace an existing bundle.
- `--include-text`: keep free text from the file (sample, image and channel names, comments).
- `--hex N`: add hex excerpts of N bytes (at most 256) of the file head and of up to 32 structure headers.
- `--full-check`: run the full integrity check instead of headers only.
- `--no-hash`: leave out the file's SHA-256.
- `--no-first-read`: do not decode a first plane, sweep, spectrum or table rows.

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

Confirm that an export holds the same data as its source:

```console
$ openreadout check doctor.tif --against doctor.ome.tiff
=> identical
metadata: same (0 differences)
image 0: geometry same, channel names same, physical size same
planes: 2 compared, 2 identical, 0 within tolerance, 0 mismatched
```

## `--against`

`check A --against B` answers "is B the same data as A?". It diffs the two files' `info` metadata, compares geometry, channel names and physical sizes per image, and hashes every selected plane of images with equal geometry. Numbers equal within a relative 1e-9 count as equal. `/path`, `/size_bytes`, `/format`, `/format_version` and `/notes` are always left out of the diff. It exits 0 when the files are identical and 1 when they differ; the JSON wrapper has `ok: true` in both cases.

## `--report`

The bundle lets you report a file that OpenReadout cannot read without sharing the file. It holds the file's assurance fingerprint, every decode stage with its error, the structure map, and the metadata with free text replaced. It contains no pixel, spectral, trace or table values and no path. OpenReadout doesn't send it anywhere. Attach it to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml). Read the bundle first with `--dry-run` if you want to see exactly what it contains.

## Files still being written

On an OME-TIFF, OME-Zarr, ND2 or CZI that an instrument is still writing, `check` covers the complete planes, reports an `acquisition` block, and exits 0. See [Live acquisition](../../guides/lab-shares.md).

## JSON

[`check`](../json/check.md), [`check --planes`](../json/check-planes.md), [`check --against`](../json/check-against.md), [`check --report`](../json/check-report.md).

Run `openreadout check --help` for the full help of your installed version.
