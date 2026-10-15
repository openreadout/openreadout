# stats

`stats` computes pixel statistics per plane, channel and image (min, max, mean, standard deviation, percentiles, histogram, zero and saturated fractions), or per well of a screening plate. On one plane it also measures an intensity profile along a line, the statistics inside a polygon, and objects such as nuclei.

```text
openreadout stats [OPTIONS] [FILE]...
```

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--image N`: only this image.
- `--select SEL`: plane selection such as `c=0`, `z=2-5`, `t=0,3`, or several axes in one, `c=0,1,z=2`. Repeatable. The per-image aggregate covers exactly the selected planes.
- `--level N`: pyramid level, 0 = full resolution. Default 0.
- `--region X,Y,W,H`: only this rectangle of each plane, in the pixels of `--level`.
- `--bins N`: histogram bins, 0 = none, at most 65536. Default 64.
- `--scale linear|log`: histogram bin spacing. Default `linear`.
- `--mip z|t`: take the maximum-intensity projection first, then the statistics of the projections.
- `--per channel|image|plane|well|field`: what one row is. `channel` (the default) gives channel and image aggregates, and `plane` adds one entry per plane. `well` gives one row per well and channel of a plate over its fields; `field` one row per well, field and channel.
- `--well WELL`: with `--per well` or `--per field`, only this well (`C05`). Repeatable.

### Measurements

These read one plane: the selected channels (all by default; objects use the first), the middle z unless `--select z=N`, and t=0 unless `--select t=N`. Coordinates are pixels of `--level`, the same numbers as the rulers of a [`preview`](preview.md) at level 0. Only the rectangle around the shape is read, so a line across a whole slide costs the tiles it crosses.

- `--line X0,Y0,X1,Y1`: an intensity profile. Values are sampled every pixel by bilinear interpolation, each the mean over `--line-width PX` pixels across the line (default 1). Per channel: `min`, `max`, `mean`, the position of the maximum (`max_at_px`, `max_at_um`), the full width at half maximum of the highest peak (`fwhm_px`, `fwhm_um`), and `values`: at most 200, each the mean of the samples it covers, `step_px` apart.
- `--polygon "X,Y X,Y X,Y …"`: the pixels whose centres are inside (even-odd rule): `area_px`, `area_um2`, and per channel `mean`, `std`, `median`, `min`, `max` and `sum` (the integrated intensity).
- `--objects`: count objects in `--region` or the whole plane (at most 2896 × 2896 pixels; use a coarser `--level` for more). The plane is smoothed (`--smooth PX`, Gaussian σ, default 1), thresholded (`--threshold V`, default Otsu's), and the pixels above it (`--polarity dark`: below it; the default is bright, and dark for RGB images) become 8-connected objects with holes filled. Touching objects are cut apart where the neck between them is at most three quarters as wide as the smaller one is deep, a distance-transform watershed (`--no-split` keeps them together). Objects smaller than `--min-area PX` (default 10) are dropped. The answer gives `count`, `touching_edge`, `large` (more than twice the median area, likely several objects), `area_fraction`, the spread of areas in px and µm², the median diameter in µm, and the largest `--max-objects` objects (default 100) with centroid, area, mean raw value and bounding box. RGB images are thresholded on the mean of R, G and B, or on one component with `--component 0|1|2`: stained nuclei usually stand out in one component. This is a simple segmentation: check it against a preview before quoting a count.

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
openreadout stats cells.czi --select c=1 --objects --min-area 2000       # count DAPI nuclei
openreadout stats cells.czi --line 200,1100,1300,1100 --line-width 5    # a profile across them
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

[`stats`](../json/stats.md), [`stats --per well`](../json/stats-wells.md), [measurements](../json/stats-measure.md), [batch table](../json/batch-table.md).

Run `openreadout stats --help` for the full help of your installed version.
