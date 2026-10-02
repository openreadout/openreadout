# Gamry Framework `.DTA` provenance

## 2026-09-26 — before the reader was written (Richard Zimring with Claude as assistant)

**Permissive prior art consulted (read as documentation):** gamry-parser (Brad Liang, MIT licence
checked with the GitHub licence API), `gamry_parser/gamryparser.py` on its default branch
(2026-09-26): the file is Gamry's `EXPLAIN` text format — a header of tab-separated
`KEY  TYPE  value…` lines (types `LABEL`, `QUANT`, `IQUANT`, `POTEN`, `SELECTOR`, `TOGGLE`,
`TWOPARAM`, `PSTAT`, `NOTES` followed by as many note lines as its count), then tables introduced
by `KEY  TABLE  [points]` with a row of column names, a row of units and tab-indented data rows;
numbers may be written in the PC's locale. gamry-parser (installed from PyPI) is also run as an
independent reader in the oracle.

**Corpus files used:** listed below as they are added (all in `corpus/manifest.toml`).

### Inferred from the files (scratch reading, not committed)

- The first line is `EXPLAIN`, the second `TAG  <experiment>` (`CV`, `EISPOT`, …). `DATE` and
  `TIME` are `LABEL` values in the PC's format (`3/6/2019`, month/day/year in the files seen).
- Tables: `CURVE`, `CURVE1`…`CURVEn` (one per cycle of a CV, same columns), `ZCURVE` (impedance),
  `OCVCURVE` (open-circuit record before the run), others by name. Column names: `Pt`, `T`/`Time`
  (s), `Vf` (V vs. Ref.), `Im` (A), `Vu`, `Sig`, `Ach`, `IERange`, `Over` (a text of flags),
  `Cycle`, `Freq`, `Zreal`, `Zimag`, `Zsig`, `Zmod`, `Zphz`, `Idc`, `Vdc`, `Vm`, `Temp`.
