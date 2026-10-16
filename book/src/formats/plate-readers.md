{{#include ../../../docs/formats/plate-readers.md}}

## Pictures

A kinetic read is drawn as one small curve per well in plate positions. An Incucyte export of confluence over 280 hours:

![Incucyte confluence export: one growth curve per well, B2 to G11, in plate positions](../img/formats/incucyte.png)

`openreadout analyze growth` fits each well's growth rate, lag and doubling time from the same table.

---

How this reader was derived, file by file: [provenance log](https://github.com/openreadout/openreadout/blob/main/docs/provenance/plate-readers.md).
