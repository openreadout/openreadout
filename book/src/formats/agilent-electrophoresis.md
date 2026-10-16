{{#include ../../../docs/formats/agilent-electrophoresis.md}}

## Pictures

`openreadout preview chip.xml` draws every well as a small electropherogram on a log size axis,
titled with the well, the sample and its RIN or DIN, with the tallest peaks labelled by size;
`--trace N` draws one well large and `--x-range 100:1000` zooms.

![Bioanalyzer Eukaryote Total RNA Nano chip: one small electropherogram per well with its RIN](../img/formats/agilent-bioanalyzer.png)

## Examples

```bash
openreadout info chip.xml                       # assay, instrument, wells
openreadout table chip.xml --table 0            # samples: name, RIN, concentration
openreadout table chip.xml --table 1            # the instrument's peaks
openreadout analyze peaks chip.xml --trace 3 --channel 1   # peaks of one well, sized here
openreadout info run.xml                        # TapeStation: reads run_Electropherogram.csv too
```

---

How these readers were derived, file by file: [provenance log](https://github.com/openreadout/openreadout/blob/main/docs/provenance/agilent-electrophoresis.md).
