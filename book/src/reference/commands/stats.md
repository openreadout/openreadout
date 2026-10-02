# stats

`stats` computes pixel statistics per plane, channel and image (min, max, mean, standard deviation, percentiles, histogram, zero and saturated fractions), or per well of a screening plate.

```text
openreadout stats [OPTIONS] [FILE]...
```

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--image N`: only this image.
- `--select SEL`: plane selection such as `c=0`, `z=2-5`, `t=0,3`. Repeatable. The per-image aggregate covers exactly the selected planes.
- `--level N`: pyramid level, 0 = full resolution. Default 0.
- `--region X,Y,W,H`: only this rectangle of each plane, in the pixels of `--level`.
- `--bins N`: histogram bins, 0 = none, at most 65536. Default 64.
- `--scale linear|log`: histogram bin spacing. Default `linear`.
- `--no-planes`: leave out the per-plane entries.
- `--mip z|t`: take the maximum-intensity projection first, then the statistics of the projections.
- `--per channel|image|plane|well|field`: what one row is. `well` gives one row per well and channel of a plate over its fields; `field` one row per well, field and channel. Default `channel`.
- `--well WELL`: with `--per well` or `--per field`, only this well (`C05`). Repeatable.

`stats` takes the flags in [Several inputs](index.md#several-inputs) and the [batch table flags](batch.md#batch-table-flags).

## Examples

```console
$ openreadout stats doctor.tif --bins 0
doctor.tif (tiff)
                            min   max     mean      std     p1      p50       p99   zero  saturated
image 0 (2 planes, uint16)    0  1127  563.500  501.363  2.550  563.500  1124.450  0.39%          0
  c=0                         0  1127  563.500  501.363  2.550  563.500  1124.450  0.39%          0
…
```

```bash
openreadout stats stack.nd2 --mip z --select c=1 --json      # the z-MIP of channel 1
openreadout stats slide.svs --level 2 --region 0,0,2048,2048
```

## What the numbers mean

- Statistics cover finite samples; NaN and infinities are counted in `non_finite`. `std` is the population standard deviation.
- Percentiles use NumPy's default linear interpolation. They are exact for 8- and 16-bit data (`exact: true`).
- RGB planes are summarized over all samples, and per colour component under `components[]`.
- `saturated_fraction` counts samples at the detector's maximum. When the file records how many bits the detector fills (for example 12-bit data in uint16), the maximum is 2^bits − 1; otherwise it is the pixel type's maximum. `saturation_basis` says which.
- Files without images exit 6 with a hint towards [`trace`](trace.md).

## High-content screening plates

A plate (Harmony, ImageXpress, CellVoyager, OME-Zarr HCS) is one data set whose images are its fields of view. `--per well` gives one tidy row per well and channel, merged across fields. Planes the instrument never acquired are left out; planes whose files are missing are skipped and counted in `planes_missing`.

```bash
openreadout stats MeasurementData.mlf --per well --select c=0 --csv > hoechst_by_well.csv
openreadout stats plate/ --per field --well C05 --json
```

Plate layouts, replicate averaging and group tests are covered in [Plate analysis](../../guides/plate-analysis.md) and [Batch tables](../../guides/batch.md).

## JSON

[`stats`](../json/stats.md), [`stats --per well`](../json/stats-wells.md), [batch table](../json/batch-table.md).

Run `openreadout stats --help` for the full help of your installed version.
