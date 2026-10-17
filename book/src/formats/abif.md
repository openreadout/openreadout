{{#include ../../../docs/formats/abif.md}}

## Pictures

`openreadout preview read.ab1` draws a Sanger read as a chromatogram: the four bases' traces, the
called base over each peak in its colour, the quality of each call underneath (red below QV 20),
and the whole read's quality in a strip at the top with the window marked. It starts where the
quality first holds at 20 or more; `--x-range 300:380` (base positions) shows another window.

![Sanger chromatogram of a 3730xl read: coloured traces, base calls above, quality bars below](../img/formats/abif-sanger.png)

`openreadout plot` draws the same read as a figure, with a title, annotations and other panels
beside it: `{"data": {"file": "read.ab1"}, "x": {"range": [80, 160]}}` draws bases 80 to 160. In
the SVG each base's trace, the base calls and the quality bars are a group of their own
(`data-series="A"`, `"base calls"`, `"quality"`).

A fragment-analysis run is drawn as electropherograms on a base-pair axis, one panel per dye with
its tallest peaks labelled by size, and the size standard at the bottom with each fragment
labelled by its length. `--x-range 180:300` zooms.

![Electropherogram of a 3500 multiplex run: four dye panels with sized peaks and the GS600 LIZ ladder](../img/formats/abif-fragments.png)

## Examples

```bash
openreadout info read.ab1                          # sample, instrument, read length, mean quality
openreadout table read.ab1                         # base calls: position, base, quality, scan
openreadout export read.ab1                        # read.fastq (Phred+33)
openreadout analyze peaks run.fsa --trace 0 --channel 1   # peaks of dye 1, sized in bp
openreadout table run.fsa --table 0                # the size standard's matched fragments
```

---

How this reader was derived, file by file: [provenance log](https://github.com/openreadout/openreadout/blob/main/docs/provenance/abif.md).
