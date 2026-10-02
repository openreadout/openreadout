# Provenance log — Magritek Spinsolve benchtop NMR experiment directories

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- nmrglue 0.12, BSD-3-Clause, https://github.com/jjhelmus/nmrglue (`nmrglue/fileio/spinsolve.py`,
  the copy installed in `oracle/.venv`): the directory's file names (`data.1d` FID,
  `spectrum.1d`, `spectrum_processed.1d`, `fid.1d`, `nmr_fid.dx`, `acqu.par`, `proc.par`,
  `processing.script`, `.pt1` plot files "cannot be read"), the 32-byte header of `.1d` files as
  eight little-endian u32 (owner, format, version, data type, x/y/z/q sizes) followed by float32
  values, whose first third nmrglue takes as the x axis and the rest as interleaved
  real/imaginary pairs; the `acqu.par` syntax (`name = value`, quoted strings); the axis rule
  spectral width = `bandwidth` × 1000 Hz, observe frequency = `b1Freq` MHz, carrier offset =
  `lowestFrequency` + width/2 Hz, label = `rxChannel`.
- nmrglue is also run as the oracle (`oracle/spinsolve.py`), a black box for the header, the
  parameters and the axis; its data split is not used for files without an x-axis block (below).

**Corpus files used:** `zenodo15131439-sir-swape` (85 Spinsolve 43 MHz 1H FIDs, software
2.01.19, CC-BY-4.0, Jopa, Gołowicz, Kazimierczuk; with MestReNova-processed stacked spectra in
`fidy.csv`), `zenodo20597567-slic` (10 Spinsolve Expert 60 MHz relaxation/long-lived-state
series `data.2d`, software 2.02.16, CC-BY-4.0, with the software's own processed spectra in
`*-Spectra.pt1`), the example data of spinsolveproc (GPL-3.0 repository, data files only:
Spinsolve Expert 1.41.20 at 80 MHz and 2.00.27 at 60 MHz — a 1D proton FID with the software's
`spectrum.pt1` and `proc.par`, a CPMG echo train, T1/T2 series, a PGSTE diffusion series with
`DiffusionSpectrumStacked.1d`/`.csv` and `DiffusionPlot.1d`/`.csv`).

**What was inferred from the files:**
- The header's first three words read as ASCII `PROS`, `DATA`, `V1.1` (stored byte-reversed
  as little-endian u32: `SORPATAD1.1V`); plot files start `PROS` `PLD1`/`PLD2` instead.
- Data type codes, from the file lengths against the x/y/z/q sizes and from the depositors'
  CSV exports of the same arrays: 501 = interleaved complex float32 with **no** x axis (every
  `data.1d`/`data.2d` of the 2.0x software: length 32 + 8·n; nmrglue's third split does not apply);
  503 = n float32 x values then n float32 real values (`DiffusionPlot.1d` equals
  `DiffusionPlot.csv` column by column); 504 = n float32 x values then n interleaved complex
  pairs (`DiffusionSpectrumStacked.1d` equals `DiffusionSpectrumStacked.csv`: x, real, imag).
  Other codes (500, 502, …) are not in any public file and are listed, not decoded.
- The x block's unit is not stored. In `data.1d` of type 504 it is milliseconds (0, 0.2, … with
  `dwellTime` 200 µs); in `DiffusionSpectrumStacked.1d` it is ppm (the span equals
  `bandwidth`/`b1Freq`). The reader names the unit only when the values say so: a linear x
  block whose step equals `dwellTime` in ms (time) or `bandwidth`·1000/(n·`b1Freq`) (ppm).
- A file is the FID when it is `data.1d`, `fid.1d`, `data.2d` (…`.4d`) with complex values and
  x size = `nrPnts`; its sweeps are the y·z·q rows (2.0x relaxation, SLIC and PGSTE series: y =
  `nrSteps`). `data.1d` of the CPMG protocol `T2Bulk` holds `nrEchoes` echo amplitudes (x
  size 2000 ≠ `nrPnts` 128) and is not an FID.
- Frequency sense and phase, from the software's own processed spectra (`spectrum.pt1`, the 16
  and 10 rows of two `*-Spectra.pt1`): the plotted spectrum at ascending point k (ppm
  (`lowestFrequency` + k·width/n)/`b1Freq`) is the FFT of the complex-conjugated FID (first point
  × 0.5, no apodization when `filter = "no"`) at frequency index k − n/2, times e^{+i·p0} where p0
  is the first argument of `Phase(p0, p1)` in `processing.script` (= −`p0Phase` of
  `proc.par`); residual 2·10⁻⁷ of the maximum on the 1D proton spectrum (float32 rounding) and
  ≤ 1.2·10⁻² on the relaxation rows. So `b1Freq` is the 0 ppm frequency, and in this project's
  pipeline (index 0 = high ppm, spectrum × e^{−iφ0}) the FID is conjugated and φ0 = −p0.
- `expName` is `<yymmdd>-<hhmmss> <protocol> (<suffix>)` (the macros' own folder rule, visible
  as text inside the plot files): the acquisition start time (local clock, no zone) and the
  user's suffix (a sample label in every public file).
- `specType` (`C43`, `C60Ultra`, `C80Ultra`, `P60Grad`) names the instrument model and
  `specID` (`SPA…`) its serial number: the values track the `b1Freq` of each file (43, 60,
  62, 80 MHz).
