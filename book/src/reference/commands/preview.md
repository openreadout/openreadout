# preview

`preview` renders a PNG or JPEG picture of the data: an image plane, a channel composite or a maximum projection; a trace; a mass spectrum; or a plate heat map.

```text
openreadout preview [OPTIONS] <FILE>
```

The kind of picture follows the file: images first, then traces, spectra and plate tables. The trace, spectrum and plate flags pick another. The same request on the same file always gives the same bytes.

## Flags

### Output

- `-o`, `--output FILE`: `.png` or `.jpg`; `-` writes the image to stdout. Default: `<input stem>.preview.png` next to the input.
- `--format png|jpeg`: default from the output extension, else PNG.
- `--quality N`: JPEG quality, 1–100. Default 90.
- `--max-size N`: longest side of the picture in pixels (16–8192), rulers included. Default 1024.
- `--overwrite`: replace an existing output file.
- `--json`: print the report as JSON.

### Images

- `--image N`: image index. Default 0.
- `--select SEL`: planes, such as `c=1`, `z=4` or `c=0,1,z=2`. Repeatable. Default: channel 0, the middle z, t 0. Several channels imply `--composite`.
- `--mip z|t`: maximum-intensity projection over z or t, limited to the selected range.
- `--composite`: blend all (or the selected) channels in their colours.
- `--level N`: pyramid level. Default: the level nearest `--max-size`.
- `--region X,Y,W,H`: only this rectangle, in full-resolution pixels (or in the pixels of `--level` when one is given).
- `--contrast auto|min-max|percentile:LO,HI|raw`: `auto` stretches the 0.1–99.9 percentiles. Default `auto`.
- `--lut gray|channel-color`: default gray for one channel, channel colours for composites.
- `--axes rulers|grid|none`: `rulers` (default) frames the picture with rulers in full-resolution pixels and a scale bar, `grid` adds faint grid lines at the ruler ticks, `none` gives the bare plane.

### Traces, spectra and plates

- `--trace N`, `--sweep N`, `--channel N`: the trace, sweep and channels to draw (`--channel` is repeatable; default the first 8).
- `--run N`, `--spectrum N`, `--scan N`: the mass spectrum to draw, by zero-based index or instrument scan number.
- `--centroid`: draw the instrument's centroid list instead of the profile.
- `--table N`, `--column NAME`: the plate table, and the value column of a long layout.

### NMR

- `--process`: draw FIDs as spectra processed by OpenReadout.
- `--process-phase MODE`, `--process-lb HZ`, `--process-size N`, `--process-baseline MODE`: processing settings. See [`analyze nmr-peaks`](analyze.md#nmr-peaks).

## Examples

```console
$ openreadout preview doctor.tif -o doctor.png
wrote doctor.png (47x49 png, 203 bytes, verified=true): image 0 level 0 c=[0] z=[1] t=[0] contrast percentile:0.1,99.9
view it: open (or Read) doctor.png; zoom with --region X,Y,W,H in full-res px read off the rulers (now showing 0,0,16,8)
```

```bash
openreadout preview cells.czi --select c=0,1,2 --mip z -o cells.png
openreadout preview run.raw --scan 1200 -o scan1200.png
```

## Zooming into large images

Image previews have rulers along the top and left edges in full-resolution pixel coordinates, whatever level was drawn. Read coordinates off the picture and pass them back as `--region X,Y,W,H` to zoom in. `preview` then reads the coarsest pyramid level that still shows the region at `--max-size`, so zooming into any part of a whole-slide image reads only a few tiles. The JSON report maps picture pixels back to source pixels (`image.plot_area`, `source_origin`, `source_per_pixel`).

## JSON

[`preview`](../json/preview.md).

Run `openreadout preview --help` for the full help of your installed version.
