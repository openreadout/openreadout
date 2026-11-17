# 2D NMR

OpenReadout reads 2D and 3D NMR data sets the way it reads 1D ones: the processed spectrum the vendor software stored (Bruker `2rr`, JEOL, JCAMP-DX `nD NMR SPECTRUM`), or the raw `ser` transformed in every dimension. This page shows what you can do with them from the command line; every command is also an MCP tool with the same arguments (`analyze nmr-peaks` is `openreadout_nmr_peaks`). The examples use public nmrXiv and BMRB data sets from the [corpus](../project/how-we-validate.md).

## Look at the spectrum

```bash
openreadout preview HSQC -o hsqc.png
```

A 2D spectrum is drawn as contours: F2 along the bottom and F1 up the right side, both from high to low shift, so the diagonal of a COSY runs from the bottom left to the top right. The skyline projections run along the top and the left. Contour levels start at 6 noise SDs and rise by a constant factor; negative contours are drawn in the accent colour.

![Edited HSQC: positive CH and CH₃ contours in ink, negative CH₂ contours in red, with the ¹H projection on top and the ¹³C projection on the left](../img/nmr2d/hsqc.png)

| COSY | HMBC |
| --- | --- |
| ![COSY of the same sample: the diagonal from bottom left to top right and its cross peaks](../img/nmr2d/cosy.png) | ![HMBC: long-range ¹H–¹³C correlations](../img/nmr2d/hmbc.png) |

| NOESY | J-resolved |
| --- | --- |
| ![Phase-sensitive NOESY with negative diagonal peaks and t1 noise stripes](../img/nmr2d/noesy.png) | ![J-resolved spectrum with the F1 axis in Hz](../img/nmr2d/jres.png) |

Zoom with `--x-range` (F2) and `--y-range` (F1), number the cross peaks with `--marks peaks`, and raise `--contour-floor` when noise fills the picture:

```bash
openreadout preview HSQC --x-range 8.2:6.5 --y-range 140:100 --marks peaks -o zoom.png
```

![The aromatic region of the HSQC with the picked cross peaks circled and numbered](../img/nmr2d/hsqc-peaks.png)

Axes that are not chemical shifts are labelled from the file: the F1 of a J-resolved spectrum is a coupling in Hz, and a DOSY spectrum's F1 has the unit TopSpin stored (`log(m²/s)`). With `--process`, a pseudo-2D series (DOSY, T1, T2) is drawn as a stack of its rows, labelled with the gradient or delay of each.

![DOSY spectrum: the F1 axis is log of the diffusion coefficient](../img/nmr2d/dosy.png)

## Pick cross peaks

```bash
openreadout analyze nmr-peaks HSQC
```

```text
2D spectrum pdata/1 (processed, hsqcedetgpsisp2.3), 1024 × 1024 points; noise SD 3.0941e7; 287 cross peaks (left out: 18 maxima on noise ridges, 4 isolated spikes)
      F1 13C (ppm)       F2 1H (ppm)        height       S/N        volume  flags
          123.0475            8.1088     -3.1287e8      10.1     -1.7744e7
          127.7752            8.0287      4.2966e8      13.9      1.6593e7
```

Each cross peak has its position per dimension, height, sign, volume and flags: `diagonal` and `partner_missing` on homonuclear spectra, `ridge` for a peak on a line of t1 noise, `axial` at an indirect carrier. The method and its options are in the [processing notes](nmr.md#2d-and-nd-peak-picking-and-volume-integration-peaks2d-source2d-analyze_2d).

- `--range-ppm f1=140:100 --range-ppm f2=8:6` picks in a window and reads only that part of the spectrum.
- `--integrate f1=130:120,f2=7.5:7` integrates a box (repeatable); `--integral-reference 0=1` normalises.
- `-o peaks.csv` (or `.parquet`) writes one row per cross peak with `f1_ppm`, `f2_ppm`, `height`, `sign`, `volume` and `flags`.
- `--plot peaks.png` draws the contours with the cross peaks numbered as the list counts them.
- `--from fid` transforms the `ser` here instead of using the stored spectrum (a data set without one is transformed anyway). Processing flags take a dimension: `--lb f1=2 --size f1=1024`.
- `--no-negative` leaves negative peaks out; they are reported by default in 2D.

A spectrum larger than the memory limit is refused with a hint to narrow the read with `--range-ppm`, or to raise `--max-points`.

## Rows, columns and projections

`--slice` reads one 1-D trace out of a 2D spectrum, on `trace`, `export`, `plot` and `analyze nmr-peaks`:

| `--slice` | what |
| --- | --- |
| `row:110` | the F2 trace at F1 = 110 ppm |
| `column:7.26` | the F1 trace at F2 = 7.26 ppm |
| `skyline:f2`, `skyline:f1` | the largest value along the other dimension (the usual projection) |
| `min:f2` | the smallest value (negative peaks) |
| `sum:f2` | the sum |

```bash
openreadout trace HSQC --slice skyline:f2 --json
openreadout analyze nmr-peaks HSQC --slice row:122
```

`--sweep N` picks a stored row by its index instead.

## Export

```bash
openreadout export HSQC --format parquet -o hsqc.parquet
```

CSV, Parquet and Arrow exports of a 2D spectrum write its matrix with both shift axes, one row per point (`f1_ppm`, `f2_ppm`, `intensity`). `--format jcamp --trace 1` writes a JCAMP-DX `nD NMR SPECTRUM`, and `--slice` exports one slice as a 1-D spectrum. Like every export, the file is written under a temporary name, read back and checked before it is renamed into place.

## In a figure

`plot` draws a 2D spectrum as a contour panel next to other panels:

```bash
openreadout plot '{"panels": [{"data": {"file": "HMBC"}}, {"data": {"file": "HMBC", "slice": "skyline:f2"}}], "columns": 1}' -o hmbc.png
```

![An HMBC contour panel above its ¹H skyline projection](../img/nmr2d/hmbc-plot.png)
