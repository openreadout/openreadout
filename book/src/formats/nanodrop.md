{{#include ../../../docs/formats/nanodrop.md}}

## Pictures

`openreadout preview` draws an export with spectra as the spectra overlaid, with 230, 260 and 280 nm marked, beside each sample's concentration and ratios. Ratios below the usual range of clean nucleic acid are in red.

![NanoDrop One UV-Vis export: 24 absorbance spectra from 190 to 850 nm, with each measurement's A260/A280 and A260/A230](../img/formats/nanodrop-spectra.png)

An export without spectra is drawn as one bar per sample with its ratios:

![NanoDrop 8000 nucleic-acid report: the concentration of nine samples as bars, with A260/A280 and A260/A230](../img/formats/nanodrop-8000.png)

## Examples

```bash
openreadout info dsDNA.tsv                    # instrument, application, samples, spectra
openreadout table dsDNA.tsv                   # one row per sample, with units
openreadout trace dsDNA.tsv --sweep 2         # the spectrum of the third sample
openreadout batch table nanodrop/ -o all.csv  # every export of a folder as one table
```

---

How this reader was derived, file by file: [provenance log](https://github.com/openreadout/openreadout/blob/main/docs/provenance/nanodrop.md).
