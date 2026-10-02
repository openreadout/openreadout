# Provenance: quantitation (`openreadout-quant`)

No file format is parsed here: the crate reads chromatograms and spectra through the existing
readers. This log records where the methods and the validation data came from.

## 2026-09-23 — chromatograms, peak detection and integration

- **Methods** come from textbook definitions and public standards, not from any vendor software:
  Savitzky–Golay smoothing (Savitzky & Golay, Anal. Chem. 36, 1627 (1964); closed-form quadratic
  weights), trapezoidal integration, the USP tailing factor `W₀.₀₅/2f` and plate count
  `5.54 (t_R/W½)²`, the EP/JP half-height resolution `1.18 Δt/(W½₁+W½₂)`, the 10 % asymmetry
  factor, short-term noise measured on detrended segments (the idea of ASTM E685, not its text),
  drop-line / valley / tangent-skim baselines and the `B`/`V`/`T` baseline codes as they appear in
  chromatography textbooks and in the corpus vendor reports themselves.
- **Prior art read as documentation** (permissive licences): the pyOpenMS documentation of
  `PeakPickerChromatogram` (formerly `PeakPickerMRM`) and `PeakIntegrator`
  (https://pyopenms.readthedocs.io, BSD-3) for their parameters and outputs, and SciPy's
  `find_peaks`/`peak_widths` documentation (BSD-3). Both are *run* as oracles in `oracle/quant.py`;
  no code was copied.
- **Vendor results used as ground truth** (read as text/XML/netCDF, never through vendor software):
  ANDI peak tables in `mtbls1892-*-pda-*-cdf` (Shimadzu LabSolutions, MetaboLights MTBLS1892,
  EMBL-EBI terms) and `cheminfo-agilent-hplc-cdf` (ChemStation, MIT); `Report.TXT` in
  `chromhandler-001f010{1..4}-d` and `RESULTS.CSV` in `chromhandler-rau-r505-*-d`
  (FAIRChemistry/Chromhandler, MIT, commit 5b5b0f4); `Result.xml` in `gc2asm-v181-d` and
  `gc2asm-three-channels-d` (ifpen/GC2ASM, CeCILL-2.1, licence checked in the repository's
  `LICENCE.txt`, commit 161b940). Only the data files of GC2ASM (a ChemStation converter) were
  downloaded.
- **Inferred from the files**: LabSolutions ANDI exports report heights in µV and areas in µV·s for
  a signal stored in volts (the power of ten that maps the vendor heights onto the signal's maxima
  above the vendor's own baselines is 10⁶ in all 20 files; `oracle/quant.py` records it per file).
  With the vendor's own limits and baseline values, LabSolutions areas of narrow peaks are 1–5 %
  larger than a trapezoid over the exported samples while its heights agree to 0.01 %; the
  integration rule behind that is not documented and was not investigated further.

## 2026-09-24 — `--baseline auto` and the vendor-agreement benchmark

- **Question**: neither fixed baseline agrees with every data system. On the development corpus the
  drop-line default reproduced ChemStation and LabSolutions (area median 2–6 %) but gave OpenLab
  CDS areas up to +6,750 % (p95 611 %) on GC-FID peaks riding a solvent tail; `valley` reproduced
  OpenLab (median 0.19 %) and was far off everywhere else (LabSolutions median 33 %, 44 of 129
  peaks lost).
- **Ground truth used** (development corpus only; no held-out file was opened, plotted or
  compared): the vendor peak tables already in `corpus/oracle/quant/` (`mtbls1892-*` LabSolutions
  ANDI, `cheminfo-agilent-hplc-cdf`, `chromhandler-001f010{1..4}-d` `Report.TXT`,
  `gc2asm-v181-d` and `gc2asm-three-channels-d` `Result.xml`, `chromhandler-rau-r505-*-d`
  `RESULTS.CSV`) and `corpus/oracle/openlab/zenodo14316687-polyarc-*` (OpenLab `.rx`, read by
  allotropy; 54 GC-FID injections, 196 peaks). `sciformats-andi-chrom-valid-cdf` holds a
  synthetic 10-point peak table and the allotropy OpenLab LC result sets hold signals cut to
  4 values: neither can test integration and both are left out.
- **Inferred from the files** (plots of every signal with the vendor's limits and baselines, and
  the derivative of the smoothed signal at the vendor's peak limits): (1) every vendor table in
  the corpus uses only `B`/`V` ends (no `T` tangent skims among 496 peaks: 235 `BB`, 89 `BV`,
  85 `VB`, 172 `VV`, plus manual `M` codes), so riders on a larger peak are drop-lined, not
  skimmed; (2) OpenLab ends a peak where the slope of the signal has fallen to about twice the
  noise of the derivative (median |slope| at the vendor's limits 2.0–2.2 × the derivative noise),
  which on a solvent tail is where the peak meets the tail, not where the tail meets the
  baseline; our drop-line walk followed the falling tail to the next valley, and the spurious
  tail maxima formed clusters whose common baseline lay far below the tail; (3) with ends placed
  that way, the peaks on the tail are baseline-to-baseline peaks on the tail (`BB`, what the
  vendor reports), and the drop-line construction is unchanged elsewhere; (4) ChemStation's
  `gc2asm-v181-d` run anchors baselines at chosen valleys of an unresolved hydrocarbon hump while
  `gc2asm-three-channels-d` drops lines to a flat baseline under similar-looking overlaps: no
  rule computed from the signal alone reproduced both (a morphological-opening background with
  valley anchoring was tried and made `three-channels`, LabSolutions and the ChemStation ANDI run
  worse), so the hump is left to the drop-line construction and documented as a known limit.
- **Method** (the general textbook idea of slope-based peak ends — a peak ends where the slope
  of the signal returns to the noise): the flank of a peak also ends at
  the first falling sample, below half the peak's height, where the derivative of the smoothed
  signal is smaller than 2 × its noise (and than 2·10⁻⁴ × height / characteristic width), once
  the flank has been steeper than that, provided the signal does not rise again (by more than
  2 % of the peak's height and the noise hysteresis) within 8 characteristic peak widths — a
  sloped background, not the overlap region before a neighbour. The derivative noise is measured
  like the signal noise (25th percentile of detrended segment RMS). The constants were chosen on
  the development corpus and are stable over ranges (2–3 × noise, 6–20 widths, 2–5 % rise all
  beat `drop` on every vendor data set); the sensitivity runs are in `book/src/guides/quantitation.md`.
- **Benchmark**: `crates/openreadout-corpus-tests/tests/peak_agreement.rs` runs every baseline
  mode over all 92 vendor-integrated chromatograms (496 vendor peaks) and asserts that the
  default does not regress any vendor data set against `drop`.

## 2026-09-24 — bands and regions of spectra (`analyze peaks` on non-time axes), `trace --x-range`

- **Why:** the scenario tier's bitumen sulfoxide-index question had no route: `analyze peaks` refused
  traces without a time axis (exit 6) and `trace` windows were sample indices.
- **Corpus files:** `opus-bitumen-unaged-2`, `opus-bitumen-1h-180c-2`, `opus-bitumen-5h-120c-3`,
  `opus-orange-peach-juice` (OPUS), five `omnic-toffolo-atr-*` spectra and their OMNIC CSV
  exports, `wdf-pywdf-sp` (WiRE), `pesp-orange-single` (PerkinElmer), `jcamp-lancashire-dupinc1`
  (UV-Vis) and `jcamp-isas-specfile` (IR transmittance) (JCAMP-DX), `nmrxiv-s846-50` (Bruker
  `pdata/1` with TopSpin's `intrng` and `integrals.txt`).
- **Prior art consulted:** none for the method: a region area is the trapezoidal integral of the
  signal minus a straight line through its values at the region's ends (or minus zero), the way
  band areas and ageing indices are scripted with NumPy; band detection reuses our documented
  chromatographic detector with x in place of time. The readers used as oracles (brukeropus MIT,
  SpectroChemPy CeCILL-B, renishawWiRE MIT, specio BSD-3, jcamp MIT, nmrglue BSD-3, SciPy BSD-3)
  were run as black boxes.
- **What was inferred from what:** that TopSpin's `integrals.txt` values are plain sums/integrals
  of `1r` between the `intrng` limits with no baseline (bias and slope 0 in `intrng`), normalised
  to region 1: our `--baseline none` region areas scaled the same way agree within 4·10⁻⁴.
  SpectroChemPy rounds OMNIC wavenumbers to 3 decimals and nmrglue's ppm scale sits 8·10⁻⁷ ppm
  from `OFFSET − i·SW_p/(SF·SI)`; the oracle records these as tolerances.
- **Validation:** `oracle/bands.py` → `corpus/oracle/bands/*.json`, `tests/bands.rs`: 711 checks,
  all passing (largest relative difference 1.5·10⁻⁵ against NumPy on the readers' samples — the
  OMNIC axis rounding —, 8.8·10⁻⁶ against OMNIC's CSV exports, 4.1·10⁻⁴ against TopSpin's
  integrals); SciPy's five most prominent bands of each spectrum are found by the detector.
  The scenario's sulfoxide indices (`evals/scenario_facts.py`, brukeropus + NumPy) are reproduced
  to 9 significant digits with and without the baseline.

