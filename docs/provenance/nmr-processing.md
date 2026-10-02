# Provenance log — NMR processing, peak picking and integration

## 2026-09-23 — FID processing, phasing, baseline, peaks, integrals

**Corpus files used:** `nmrxiv-s275-1`, `-11`, `-13` … `-21` (TopSpin 4.3.0, CC0), `nmrxiv-s596-1`
(TOPSPIN 1.3, CC-BY-4.0), `nmrxiv-s846-50` (TopSpin 3.5, CC-BY-4.0), `nmrxiv-s501-7` and the
s501 VnmrJ directories (CC-BY-SA-4.0), `nmrxiv-s325` VnmrJ 4.0 (1H, 13C, DEPT), `nmrxiv-s837`
(TopSpin 3.7, FID-only), `nmrxiv-s200` JEOL Delta FIDs and their MestReNova JCAMP-DX exports
(CC0), `nmrxiv-s1243` JEOL FIDs, nmrglue test data `agilent_1d`, `nmrpy-test2`.

**Prior art consulted (read as documentation):**
- nmrglue (BSD-3-Clause, https://github.com/jjhelmus/nmrglue): `process/proc_base.py` (`em`, `gm`,
  `ps`, `fft` ordering, `fsh2`), `fileio/bruker.py` (`rm_dig_filter`, `bruker_dsp_table`),
  `fileio/jeol.py` (`X_OFFSET` as the carrier position in ppm), `fileio/varian.py`. Used to
  understand conventions; our implementation is our own.
- Chen, Weng, Goh, Garland, *An efficient algorithm for automatic phase correction of NMR spectra
  based on entropy minimization*, J. Magn. Reson. 158 (2002) 164–168 (ACME objective).
- Dietrich, Rüdel, Neumann, *Fast and precise automatic baseline correction of one- and
  two-dimensional NMR spectra*, J. Magn. Reson. 91 (1991) 1–11 (baseline recognition idea).

**What was inferred from what:**
- Bruker orientation and phase sign: processing the s275-1 FID and correlating with TopSpin's
  `pdata/1/1r` for the 16 combinations of spectrum reversal, phase sign, first-point handling and
  group-delay handling. Result: no conjugation, spectrum index k = FFT bin (N/2 − k) mod N (carrier
  at N/2; an earlier off-by-one ordering gave 0.4 correlation on 13C and was found by
  cross-correlating magnitudes: lag −1), correction e^{−i(PHC0 + PHC1·k/SI)}, first point × 0.5,
  group delay removed by rotation with the pre-response kept at negative time. Correlation
  1.000000 on s275-1/-11 and s846-50, scale 1.0006.
- `OFFSET = ((SFO1 − SF)·10⁶ + SW_h/2)/SF` reproduces the stored `OFFSET` (15.114770 vs 15.11477).
- Old consoles (s596-1: `AQ_mod` 1, DSPFVS 10, table group delay 59.083): the stored phases give
  an inverted spectrum and a residual first-order phase of 44°; not resolved (one file). Default
  phasing detects the inversion (negative fraction) and uses automatic phases instead.
- Varian: conjugating the FID puts TMS at 0.000 ppm and CHCl3 at 7.263 ppm in s325 1H with
  `reffrq` as the 0-ppm frequency; automatic phasing lands on −`rp` (−84.7 vs `rp` 85.0), and
  −`rp` gives ≤ 7 % negative signal on s325 1H/13C and s501 PROTON; `lp` could not be matched on
  the two files with non-zero `lp` (agilent_1d, nmrpy-test2).
- JEOL: the FID's first ~20 points are a digital-filter pre-response. The parameters `orders`
  (`"2 54 73"`) and `factors` (`"8  2"`) were read as stage count, filter orders and decimation
  factors; Σ (order−1)/2 / Π later factors = 19.656 points matches the observed onset, and the
  processed s200 qHNMR FID agrees with MestReNova's processed spectrum to 0.003 ppm (20/25 peaks).
  This reading of the two parameters is our inference from one instrument's files.
- DC offset: our unbaselined spectrum carries a constant the first point contributes that TopSpin's
  `1r` does not; the default first-order baseline removes it (peak comparisons use it).

## 2026-09-24 — rerun after the shared build directory was retired

All validation numbers were regenerated in a private target directory (the shared
`CARGO_BUILD_BUILD_DIR` could link another worktree's crates).

## 2026-09-26 — processing parity across 46 TopSpin experiments (Richard Zimring with Claude as assistant)

**Corpus files used:** 39 more Bruker experiments with TopSpin's own `pdata/1/1r` from 16 more
nmrXiv projects (P55, P72, P74, P80, P87, P88, P90, P98, P100, P101, P105, P107, P111, P122,
P146; CC-BY-4.0/CC0; `nmrxiv-parity/`, ids `nmrxiv-s<sample>-<experiment>`), with the 7 of
2026-09-23: TopSpin 3.5–4.3, 300–600 MHz, 1H, 13C (incl. DEPT and UDEFT), 31P, DSPFVS 10/20/21,
DIGMOD 1 and 3, FCOR 0/0.5/1, forward and backward linear prediction.

**Prior art consulted:** none new. nmrglue (BSD-3) is still only run as a comparison pipeline.

**What was measured and inferred (before changing the code):**
- With the stored phases our spectrum matched `1r` at correlation 1.000000 on every experiment
  acquired in DIGMOD 3 (integer `GRPDLY`), but only 0.9985–0.9993 on DIGMOD 1 experiments
  (`GRPDLY` 67.984–67.988). Fitting a residual phase against `1r` + i·`1i` gave, at the carrier,
  a zero-order offset of 1.1–2.8° with no first-order part, equal (within 0.4°) to
  180° × (⌈GRPDLY⌉ − GRPDLY) (2.13° for 67.9882, 2.48° for 67.9862, 2.84° for 67.9842). The same
  rule predicts the three "inverted" old-console experiments (DSPFVS 10, table delay 59.083 and
  similar): 180° × 0.917 = 165°, cos 165° = −0.966, the measured correlation (−0.963 … −0.966).
  Inference: the stored phases refer to a spectrum whose fractional group-delay correction
  carries a zero-order term π·(⌈G⌉ − G) relative to ours (ramp pivoted at the spectrum edge
  after a shift by ⌈G⌉ points instead of at the carrier after a shift by ⌊G⌋).
- FCOR (procs): the factor applied to the first FID point; FCOR 1 files kept the constant
  offset the halving removes, which showed as integral errors up to 48 % on the FID but not on
  `1r`. TDeff (procs): the number of FID points used (real + imaginary).

## 2026-09-26 — Magritek Spinsolve FIDs (Richard Zimring with Claude as assistant)

**Corpus files used:** `spinsolveproc-proton`, `spinsolveproc-t1`, the ten `spinsolve-zenodo20597567-*` series (the Spinsolve software's own processed spectra in `spectrum.pt1` / `*-Spectra.pt1`, one per FID row).

**Prior art consulted:** nmrglue 0.12 `fileio/spinsolve.py` (BSD-3-Clause): spectral width, observe frequency and carrier from `acqu.par`. Nothing else; no Magritek software or documentation.

**What was inferred from what:** fitting each FID row's transform to the plotted spectrum (a least-squares complex scale over the conjugated/non-conjugated, reversed/shifted variants): the plotted spectrum at ascending ppm point k equals the FFT of the conjugated FID (first point × 0.5, no apodization when `filter = "no"`) at frequency index k − n/2 times e^{+i·p0}, with p0 the first argument of `Phase(p0, p1)` in `processing.script` (= −`p0Phase` of `proc.par`), relative residual 2·10⁻⁷ on the 1D proton spectrum; `b1Freq` is the 0 ppm frequency. Hence `conjugate` = yes, stored φ0 = −p0, reference = `b1Freq`, no group delay. Unverified (no public example): a non-zero first-order phase, `filter = "yes"` apodization, `zf` > 1.
