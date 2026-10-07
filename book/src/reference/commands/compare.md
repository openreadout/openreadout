# compare

`compare A B` answers "is B the same data as A?", for example a raw file and its export. It exits 0 when the files hold the same data and 1 when they differ. The JSON wrapper has `ok: true` in both cases.

```text
openreadout compare [OPTIONS] <FILE> <AGAINST>
```

MCP: `openreadout_compare` with `file` and `against`.

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--image N`: only this image, in both files.
- `--select SEL`: plane selection such as `c=0`, `z=2-5`, `t=0,3`. Repeatable.
- `--level N`: pyramid level, 0 = full resolution.
- `--tolerance T`: largest absolute sample difference that still counts as equal. Default: planes must be bit-identical.
- `--ignore POINTER`: leave this JSON pointer out of the metadata diff (`*` matches one segment). Repeatable.
- `--include-extra`: also diff the format-specific `extra` objects.
- `--no-metadata`: compare data only.
- `--no-pixels`: compare metadata and geometry only.

## What is compared

`compare` diffs the two files' `info` metadata, compares geometry, channel names and physical sizes per image, and hashes every selected plane of images with equal geometry. Numbers equal within a relative 1e-9 count as equal. `/path`, `/size_bytes`, `/format`, `/format_version`, `/notes` and `/images/*/dimension_order` (the order a container stores planes in; OME-Zarr is always `t, c, z`) are always left out of the diff.

With `--select`, the planes compared are narrowed to the selection. When B holds only those planes of an image, as an export made with the same `--select` does, B's planes are matched to A's selected planes in order, the metadata diff sees A narrowed the same way, and the image is reported with `selected: true`.

## Examples

Confirm that an export holds the same data as its source:

```console
$ openreadout compare doctor.tif doctor.ome.tiff
=> identical
metadata: same (0 differences)
image 0: geometry same, channel names same, physical size same
planes: 2 compared, 2 identical, 0 within tolerance, 0 mismatched
```

An export of one channel:

```bash
openreadout export a.nd2 --select c=1 -o c1.ome.tiff
openreadout compare a.nd2 c1.ome.tiff --select c=1    # identical
```

## JSON

[`compare`](../json/compare.md).

Run `openreadout compare --help` for the full help of your installed version.
