# MicroCal ITC `.itc` provenance

## 2026-09-26 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml` with their licences):

- bayesian-itc (choderalab, GPL-3.0, commit c992735): `itc-bitc-caii-050611a` (VP-ITC, VPViewer2000
  1.30.0, 7 data columns) with its Origin exports `A050611aRAW.TXT` (time, power) and `A050611a.TXT`
  (integrated heats with the cell concentration and injection volume), and the lab notebook
  `README` (446 µM CBS into ~10 µM CA II); `itc-bitc-mg1edta-p1a` (VP-ITC, VPViewer2000 1.4.8, 3
  columns, 23 injections of 12 µl) with `Mg1EDTAp1aRAW.DAT` and `Mg1EDTAp1a.DAT`; `itc-bitc-adh10`
  (VP-ITC 1.4.8) with `ADh10RAW.DAT`; `itc-bitc-ca2cam` (VP-ITC at 6 °C, 50 blocks);
  `itc-bitc-sampl3-p5d091` (ITC200 1.25.5, 7 columns, 5 s sampling); `itc-bitc-sample`
  (ITC200 1.26.1, 9 columns, injection start times on the block lines).
- Zenodo 6608282 (CC0-1.0): `itc-zenodo6608282-hts-tmp-217um` and `itc-zenodo6608282-d25-50um`
  (ITC200 1.26.1); their file names and README give the cell and syringe concentrations
  (217 µM / 5.5 mM; 50 µM / 1000 µM).
- Zenodo 12959756 (CC-BY-4.0): `itc-zenodo12959756-ydat-ol` (VP-ITC, VPViewer2000 1.4.11).
- Zenodo 21479520 (CC-BY-4.0): `itc-zenodo21479520-hsl3-ctnip4` (MicroCal ITC software 1.29.32 on a
  "MICROCALITC" instrument, 9 columns, the run time on the software line).
- One further depositor's files are held out (`docs/benchmark/heldout.md`) and were not opened.

**Prior art consulted:** none. bayesian-itc's data files and their Origin exports were used as
ground truth.

### Inferred from the files (scratch comparisons, not committed)

- Text, CRLF. Lines starting `$`: `$ITC`; the number of injections (equal to the count of the
  four-number lines that follow in every file); `$NOT` (meaning unknown, kept verbatim); the
  target temperature (°C; equals the `#` temperature line and the measured cell temperature);
  the initial delay (s: 60-3000; the first injection block starts after it: 300 s delay, first
  block at 302 s in `050611a`); the stirring speed (rpm: 307 and 298-300 on VP-ITC, 750 and 500 on
  ITC200/MicroCal ITC, the instruments' usual speeds); the reference power (µcal/s: 5-10); a code
  (2 in every file, kept verbatim); `$ADCGainCode: n`; `$False,True,True` (kept verbatim); then one
  line per injection: volume (µl), duration (s), spacing (s), filter period (s) — `050611a`: 10 µl
  in 20 s every 240 s with a 2 s filter, and Origin's heats table lists 10 µl per injection.
- Lines starting `#`: 0; the syringe concentration (mM: 0.446 where the notebook says 446 µM CBS;
  5.47 where the file name says 5.5 mM); the cell concentration (mM: 0.00995, which Origin's heats
  table repeats as `Mt`; 0.217 where the file name says 217 µM); the cell volume (ml: 1.4266,
  1.434 on VP-ITC; 0.2001-0.2064 on ITC200: the nominal volumes of the two cells); the
  temperature; two further numbers (kept verbatim).
- `?` starts the free-text comment (lines until the first `%` line; empty in most files).
- Lines starting `%`: the instrument identifier (`VPITC06.03.471`, `ITC200_12.08.168`,
  `MICROCALITC_MAL1177627`), the cell volume again, calibration numbers (kept verbatim, not
  interpreted), and last the software and version (`VPViewer2000 Ver: 1.4.8`, `ITC200 Ver:
  1.26.1`, `MicroCalITC Ver: 1.29.32 Run time:4/15/2025 5:02:02 PM`).
- `@0` starts the pre-injection baseline; `@n,<volume>[,<duration>[, <start time>]]` starts
  injection n's block (the volume matches the injection line; ITC200 1.26+ adds the time the
  injection started, s).
- Data rows: comma-separated numbers; column 1 is the time (s) and column 2 the differential power
  (µcal/s): both equal Origin's RAW exports of three files to the export's five decimals (1,350,
  3,205 and 6,062 rows); column 3 is the cell temperature (°C; it tracks the target). Further
  columns (7 and 9 in newer files) are not identified and are kept unnamed. Sampling is uniform in
  every file (2 s VP-ITC, 1 or 5 s ITC200), across injection blocks.
- Integrated heats (Origin's `DH`) are not in the `.itc` file; they are the analysis software's.
