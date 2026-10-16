# stats

`stats` computes pixel statistics per plane, channel and image (min, max, mean, standard deviation, percentiles, histogram, zero and saturated fractions), or per well of a screening plate. On one plane it also measures an intensity profile along a line, the statistics inside a polygon, objects such as nuclei, and the lanes and bands of a gel or blot.

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

These read one plane: the selected channels (all by default), the middle z unless `--select z=N`, and t=0 unless `--select t=N`. Coordinates are pixels of `--level`, the same numbers as the rulers of a [`preview`](preview.md) at level 0. Only the rectangle around the shape is read, so a line across a whole slide costs the tiles it crosses. Lengths and areas are also given in µm and µm² when the file records the pixel size (taller-than-wide pixels included).

- `--line X0,Y0,X1,Y1`: an intensity profile between two points on the plane (its edges, 0 and the width or height, included). Values are sampled every pixel by bilinear interpolation, each the mean over `--line-width PX` pixels across the line (default 1). Per channel: `min`, `max`, `mean`, the position of the maximum (`max_at_px`, `max_at_um`), the full width at half maximum of the highest peak above the line's minimum (`fwhm_px`, `fwhm_um`), and `values`: at most 200, evenly spaced from the start to the end of the line, `step_px` apart, each the mean of the samples within half a step.
- `--polygon "X,Y X,Y X,Y …"`: the pixels whose centres are inside (even-odd rule): `area_px`, `area_um2`, and per channel `mean`, `std`, `median`, `min`, `max` and `sum` (the integrated intensity).
- `--objects`, `--objects=cells`, `--objects=spots`: count nuclei (the default), whole cells or small bright spots in `--region`, inside `--polygon`, or in the whole plane. See [Objects](#objects).
- `--lanes`, `--lanes=N`: find the lanes and bands of a gel or blot in `--region` or the whole plane, `N` lanes when given. `--lane-width PX` sets the lane width, `--polarity bright|dark` the bands, `--ladder LANE:SIZES` (with `--ladder-unit`) sizes every band from a marker lane, and `--profiles` adds each lane's profile. See [Lanes](#lanes).

The flags of pixel statistics (`--mip`, `--per`, `--bins`, `--scale`, `--well`) do not apply to these measurements, so giving one with them is a usage error, as are `--region` with only a line or polygon and `--line-width` without `--line`. Pick the plane with `--select` instead.

### Objects

One call answers how many, how big, how bright and where. The settings come from the image, so the defaults usually suffice:

- **What is counted.** Nuclei are found in the channel named like a DNA stain (DAPI, Hoechst, DRAQ5 …), else channel 0; `--select c=N` picks another. On RGB images, stained brightfield (H&E, hematoxylin, IHC) is unmixed into hematoxylin, eosin and DAB by colour deconvolution and nuclei are found in hematoxylin; `--stain` picks another stain or a component. Fluorescence RGB uses the brightest component. Bright objects on a dark background, or dark ones on a bright background, are told apart from the image (`--polarity` overrides).
- **Size.** The expected diameter sets the smoothing, the background scale, how close two objects may be and the smallest object kept. By default it is measured from the image: candidate sizes come from the regions above Otsu's threshold, the scale of the strongest blobs, and a smoothed copy for textured nuclei, and the one whose segmentation gives the most compact objects wins. When the pixel size is known, only sizes a nucleus can have (3 to 30 µm; cells 6 to 150 µm) are considered, and the typical 10 µm (cells 20 µm) competes too. `--diameter UM` (or `--diameter-px PX` for images without a pixel size) sets it; it can be at most the shorter side of the area searched. Spots default to 0.5 µm and are at most 64 px across.
- **Threshold.** The plane is smoothed at a twentieth of the diameter and the slowly varying background under objects of that size is removed (not for stain amounts, which are already relative to empty glass). The threshold is Otsu's, moved 30% of the way towards the background so dim objects are whole, or the upper of Otsu's three-class thresholds when objects stand out from a middle class such as cytoplasm and that gives clearly more compact objects. When a few objects cover a tiny part of a large background, Otsu's threshold falls inside the background noise; the threshold is then half way from the background to the objects' median brightness, and at least five robust standard deviations above the background (`sparse`). `--method otsu|triangle|local` or `--threshold V` override; `threshold_method` says which was used.
- **Touching objects** are cut apart by a seeded watershed: one seed per peak of the distance to the background, at least a quarter diameter apart, with dark seams between objects (valleys narrower than a fifth of a diameter where the signal falls by 40%) counted as background for the distance. `--no-split` keeps them together. Holes smaller than half an object are filled, and fragments under a fifth of the expected area merge into their neighbour (`--min-area UM2` and `--max-area UM2` set the limits in µm²).
- **Spots** are the local maxima of a Laplacian of Gaussian at the spot scale whose height above the local background is at least five robust standard deviations of the background (`--threshold` sets the height). Each spot is measured over a disc of the expected diameter.
- **Large areas.** Regions up to 2896 × 2896 pixels are counted in one piece; larger ones (up to 8192 × 8192) in tiles with a margin of three diameters, so an object is found as it would be in one piece and counted once. For more, pass a smaller region or a coarser `--level`.

The answer gives `count`, `touching_edge`, `density_per_mm2` (objects touching the edge count half), `area_fraction`, `large` (objects over twice the median area, perhaps several together), the spread of areas (`area_px`, `area_um2`) and diameters (`diameter_um`) of the objects off the edge, the expected diameter used and where it came from, and the threshold. `channels` lists every channel with its background (the median outside objects), the spread of the object means and, for markers (other channels than the one counted, and DAB), how many objects are `positive`: those above Otsu's split of the object means when they fall in two clear groups, else above the background by five robust standard deviations. `objects` lists up to `--max-objects` objects (default 100) in reading order, each with its number, centroid, area, diameter, solidity (low values suggest merged objects), mean in every channel and whether it touches the edge. `confidence` is `low` when less than half of the objects' area (`fit`) is in compact objects of the expected size (on brightfield smears, where cytoplasm and debris are counted, and on stains other than H&E such as a trichrome), when most objects are under a third of the expected area, or when more objects were found than the area holds (both signs of a threshold that cut noise or texture into pieces). The count is then a warning, not an answer, and the text output starts with LOW CONFIDENCE. `notes` say what was assumed, why the confidence is low, and when the contrast is too weak for a reliable count.

Check a count against a picture before quoting it: `preview --marks objects` draws the same objects with their numbers.

Methods: Otsu (IEEE SMC 1979) and its multi-level form; the triangle threshold (Zack, Rogers and Latt, J Histochem Cytochem 1977); the exact Euclidean distance transform of Felzenszwalb and Huttenlocher (Theory of Computing 2012); marker-controlled watershed (Beucher and Meyer 1993); scale selection with the normalised Laplacian of Gaussian (Lindeberg, IJCV 1998); colour deconvolution with the stain vectors of Ruifrok and Johnston (Anal Quant Cytol Histol 2001).

### Lanes

`--lanes` answers "how strong is each band, and in which lane" for a gel or blot image (Image Lab `.scn`, Typhoon `.gel`, TIFF exports of imagers), the way densitometry software does it. Lanes run top to bottom; rotate a sideways image first.

- **Signal.** Bright bands on a dark ground (fluorescence, chemiluminescence) are measured as stored. Dark bands on a light ground (Coomassie, silver, colorimetric) are measured as the plane's largest value minus the value. The side of the median that the histogram's long tail lies on decides between them; `--polarity` overrides. Planes over 4 million pixels are binned by block means; coordinates stay full-resolution pixels.
- **Lanes.** Each column's signal is the mean of its brightest tenth above the plane's 5th percentile, smoothed. The autocorrelation of that profile gives the lane pitch. Lanes are its maxima at least 0.6 pitches apart that rise a sixth of the profile's range above the valleys around them, each centred on the signal within half a pitch. A gap of two or more pitches between lanes gets evenly spaced lanes, so an empty lane keeps its number. `--lanes=N` keeps the `N` most prominent. Lanes are 0.7 pitches wide unless `--lane-width` says otherwise. Rows where the lanes' signal stays below a tenth of its upper level (the ground around a membrane) are left out. `lane_contrast` (1 − the gaps' signal over the lanes') says how clearly the lanes stand out: below 0.15 they are a guess, and a note says so.
- **Profile and background.** A lane's profile is the mean across the middle 80% of its width, row by row. Its background is a rolling-ball background: a grey-scale opening by a tenth of the lane's length, smoothed, kept under the profile.
- **Bands** are maxima of profile minus background that stand at least four robust standard deviations of the profile's noise, 5% of the lane's highest band (`min_share` in MCP) and 3% of the image's strongest band above the background, away from the first and last 3% of the lane. A band runs to where it falls to 5% of its height, or to the valley before the next band. A step of the background as large as the band, such as a membrane's bright edge, is not a band. Nor are the wells: bands in the top 10% of the lanes that three quarters of the lanes (at least three) share within 1.5% of the lanes' length, above further bands in most of them, are left out with a note.
- **Volume** is the sum of profile minus background over the band's rows times the lane width: stored units × pixels, as densitometry software reports it. `fraction_of_lane` is the band's share of its lane's band volume, and `relative` its share of the image's strongest band. A band with pixels at the saturation level (the detector's ceiling when the file records one, else the pixel type's) has `saturated_px` above zero and a note: its volume is a lower bound. Each lane's `total_volume` is its whole background-subtracted signal, for normalising to a total-protein stain.
- **Sizes.** `--ladder 1:250,150,100,75,50` pairs the marker lane's bands, top down, with the sizes given, largest first. Every band then gets a size by linear interpolation of log(size) against its row between the neighbouring marker bands, extrapolated from the end pair outside them. When the marker lane has more or fewer bands than sizes, the top ones are paired and a note says so.

Check the lanes against a picture before quoting volumes: `preview --marks lanes` draws the same lanes, profiles and bands.

![A western blot with five lanes boxed and numbered, each lane's profile drawn in it, and the bands marked with their share of the strongest band](../../img/formats/blot-lanes.png)

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
openreadout stats cells.czi --objects                                   # count the nuclei
openreadout stats slide.svs --objects --region 9000,20000,1200,800      # nuclei in H&E tissue
openreadout stats foci.nd2 --objects=spots --select c=2 --diameter 0.4  # puncta in channel 2
openreadout stats cells.czi --line 200,1100,1300,1100 --line-width 5    # a profile across them
openreadout stats blot.scn --lanes                                       # lanes, bands, volumes
openreadout stats gel.scn --lanes=10 --ladder 1:250,150,100,75,50 --ladder-unit kDa
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
