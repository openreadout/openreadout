{{#include ../../../docs/formats/qubit.md}}

## Pictures

`openreadout preview` draws each sample's concentration as a bar, with the instrument's text where it wrote no number:

![Qubit 4 export: the original sample concentration of seven tubes as bars, one marked Out of range](../img/formats/qubit.png)

## Examples

```bash
openreadout info run.csv                     # assay, instrument, tubes
openreadout table run.csv                    # one row per tube, with units
openreadout batch table qubit/ -o all.csv    # every export of a folder as one table
```

---

How this reader was derived, file by file: [provenance log](https://github.com/openreadout/openreadout/blob/main/docs/provenance/qubit.md).
