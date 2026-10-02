# Provenance: TA Instruments data files (Universal Analysis, `.001`)

## 2026-09-26: initial reader (`ta-universal-analysis`)

**Corpus files used (development):** Zenodo 17293641 (Bacova, Sanz de León, Molina; CC-BY-4.0):
`PCL_standard.001` and `PCL_MDSC_r.001` (DSC Q20, standard and modulated) with the depositors'
Universal Analysis text exports `PCL_standard.txt`, `PCL_MDSC_r.txt`; GitHub
vincent-uden/dsc-q20 (MIT, commit bd2756f): `Data.079`, `Data.082`, `Data.083`, `Data.085` (DSC
Q20 indium and salt runs) with the depositors' exports `In.tsv`, `IndiumFin-18-37-22-02.tsv`,
`InCp.tsv`, `Fixersalt-10-15-23-02.tsv`, and `Data.074` (a run with no records). The export
headers name their raw file (`File \\...\Data.079`).

No held-out file: two depositors only.

**Prior art consulted:** none (no open reader of these files was found; the layout was read from
the files and their exports).

**What we inferred:**

- The file is UTF-16LE with a byte-order mark. The header is `key value` lines (the first space
  separates them; the exports write a tab) ending in CR LF, starting `CLOSED` (or `OPEN`), with
  `VERSION`, `Instrument`, `Module`, `Operator`, `File`, `InstSerial`, `Sample`, `Size`, `Nsig`,
  `Sig1` … `SigN` (label and unit), `Date`, `Time`, `OrgMethod` (the method steps) and calibration
  lines. A form feed (U+000C) ends the header.
- After the form feed: one byte holding the signal count (equal to `Nsig` in every file), then
  records of `Nsig` little-endian float32 values in `Sig` order. In `PCL_standard.001` all 13 539
  exported rows equal the records in order; in the dsc-q20 files every exported row equals a
  record.
- The records end at a record whose first value is −100.0 (second 20.0, the rest 0); bytes after it
  are not data. Checked in every file; the empty runs hold the end record right after the count.
- The modulated-DSC export (`PCL_MDSC_r.txt`) holds 10 of the 13 signals at its own times (about
  every 1.5 records): our values linearly interpolated there agree within 0.3 % of each column's
  largest magnitude (time exactly).
