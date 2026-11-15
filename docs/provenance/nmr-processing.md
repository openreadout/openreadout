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

## 2026-10-11 — peak picking on water-suppressed metabolomics spectra

**Corpus files used:** `zenodo20308061-andrographis-mac50-3`, `zenodo20308061-andrographis-ult100-2` (Zenodo 20308061, CC-BY-4.0; all 27 experiments of the record were compared), `zenodo22712212-aiw-61` (Zenodo 22712212, CC-BY-4.0; 61 experiments compared), and the 1H experiments already in the corpus. **Prior art consulted:** none new.

**What was measured:** the highest point of TopSpin's `1r` in five windows against `analyze nmr-peaks --range-ppm` (`main_peak`), on the stored spectrum and on the FID processed here. The stored spectrum agrees everywhere except where two lines differ by less than 0.5 % in height (parabolic interpolation reorders them) and where the window cuts a line: a line just outside the window was reported because the index window was rounded outwards. Peaks are now kept only when their interpolated shift is inside the window. The FID processed here matches `1r` at correlation 0.99999 but sits on a baseline about 0.3 % of the tallest line lower than TopSpin's in these DIGMOD 3 data sets (the difference is in the first time-domain point, and no single first-point factor explains all files). This reorders weak lines of nearly equal height in 4 of 27 windows; it is listed as a known gap.

## 2026-10-11 — FIDs without stored referencing

**Corpus files used:** every Bruker experiment on disk with `pdata/1/procs` (nmrXiv and the two Zenodo bundles, about 290), comparing `procs` SF with `acqus` BF1. **Prior art consulted:** none.
**What was measured:** the referencing correction (SF − BF1)/SF is within 0.2 ppm of 0 in most experiments and 0.44 to 1.41 ppm (either sign) in 20. An FID processed without `procs` can only take BF1 as 0 ppm, so its shifts are off by that much. The FID processing now says so in a note when no `procs` gives SF.

## 2026-11-14 — multidimensional processing (indirect dimensions, pseudo-2D series)

**Corpus files used:** the Bruker experiments with TopSpin's own `pdata/<n>/2rr` (and `2ri`, `2ir`, `2ii`) outside the held-out tier: `bmrb-bmse000008-cosy-topspin21-500`, `bmrb-bmse000797-jres-topspin21-700`, `bmrb-bmse001104-hsqc-topspin32-500`, `bmrb-bmse001250-hmbc-topspin35-500` (BMRB, CC-BY-4.0 / public domain as recorded in the manifest), the nmrXiv bundles s38, s49, s73, s279, s314, s501, s519 (DOSY), s572, s736, s740, s753, s853, s1247, s1407, s1517, s1608, s1617, s2331 (CC-BY-4.0/CC0), `nmrglue-bruker-3d`, `nmrglue-agilent-2d`, `nmrglue-agilent-2d-tppi` (nmrglue test data, BSD-3-Clause), the JEOL 2D `.jdf` files of nmrXiv s200 and figshare, and the Spinsolve `data.2d` series of `spinsolveproc` and `spinsolve-zenodo20597567`. TopSpin's `auditp.txt` in each `pdata/<n>` (the command history with the parameters each command used) was read alongside `procs`/`proc2s`.

**Prior art consulted (read as documentation, run as a comparison):**
- nmrglue 0.12 (BSD-3-Clause, https://github.com/jjhelmus/nmrglue): `process/proc_base.py` (window shapes `sp`, `em`, `gm`, `tm`), `process/proc_lp.py` (forward linear prediction with root reflection, run to compare), `fileio/bruker.py` (`bruker_dsp_table`), the NMRPipe conversion scripts shipped with the nmrglue test data (`conversion_scripts/*.com`: acquisition modes and parameters of the test sets). nmrglue was run as a black-box pipeline on the Varian and JEOL sets.
- J. P. Burg's maximum-entropy method as described in textbooks (e.g. Press et al., *Numerical Recipes*, §13.6) for the prediction coefficients.
- D. J. States, R. A. Haberkorn, D. J. Ruben, J. Magn. Reson. 48 (1982) 286 (hypercomplex States data); D. Marion, M. Ikura, R. Tschudin, A. Bax, J. Magn. Reson. 85 (1989) 393 (States-TPPI); J. Keeler, D. Neuhaus, J. Magn. Reson. 63 (1985) 454 and A. L. Davis et al., J. Magn. Reson. 98 (1992) 207 (echo/antiecho selection); J. C. Traficante, D. Ziessow, J. Magn. Reson. 66 (1986) 182 (Traficante window).

**What was inferred from what (numpy reproductions of `2rr` from `ser` with the 1D conventions for F2 and the stored `procs`/`proc2s`, before writing the Rust code):**
- F2: the 1D pipeline unchanged (group delay, window over the points after the group delay, `FCOR`, `TDeff`, `PHC0`/`PHC1`); sine windows `sin((π − φ)·i/n + φ)`, φ = π/`SSB` (`SSB` < 2: φ = 0), `n` the window's points, squared for QSINE (WDW 4). Correlation with `2rr` 0.9999 on the experiments without linear prediction.
- F1, by `MC2` (= FnMODE − 1), searched over sign, order and conjugation on s736 and s572 and then confirmed on every other set: QF (0): each column of the F2 spectrum, conjugated, is the complex F1 interferogram; TPPI (2): the real parts are a real interferogram at twice the rate, transformed as `2·SI1` real points, of which the first `SI1` bins conjugated are the spectrum; States (3) and States-TPPI (4): `Re(row 2k) + i·Re(row 2k+1)` (and the same of the imaginary parts), States-TPPI with (−1)^k; echo/antiecho (5): with E, A the rows of a pair, `C = i₂·(E + A)` and `S = E − A` (i₂ the F2 imaginary unit) and the interferogram `Re C − i·Re S`. Then the same window, first-point factor, transform, order and phase convention as F2. `PH_mod` 2 in a dimension is the magnitude over that dimension's imaginary parts (`mc2 F2` after `xfb`: √(rr² + ir²), s1247/14).
- DSPFVS 12 (table group delay 72.125, s572 2D and its 1D `zg`): the stored phases give the spectrum inverted; adding 180° to PHC0 reproduces `1r` (0.9994) and `2rr` (0.9999). Applied to DSPFVS 11 and 12 with a fractional table delay.
- Linear prediction (`ME_mod` 2, `NCOEF`, `LPBIN`): forward prediction of each F1 interferogram to `LPBIN` rows (`LPBIN`/2 complex points for hypercomplex data) before the window. Burg coefficients reproduce `2rr` as well as nmrglue's least-squares prediction where both are stable (0.9999 on s1617 TOCSY) and stay stable where `NCOEF` approaches the number of points (s314: least squares diverges). `ME_mod` 6 (mirror image): the interferogram extended to negative time by complex conjugation about point 0 before prediction (s38/16, 0.996).
- `auditp.txt` shows later commands that `procs` do not record: `sym` (s38/12, s49/5: replacing each point and its mirror about the diagonal by the smaller one reproduces `2rr` from 0.85 to 0.998), `tilt` and `symj` (JRES), `abs1`/`abs2` baseline corrections, and `apkm` phase corrections (whose result is the stored PHC0/PHC1, so phases stored with `PH_mod` 0 are applied).
- Pseudo-2D (DOSY `ledbpgp2s` with `difflist`, s519): F1 is not transformed; `2rr` rows are the F2 spectra of the `ser` rows.
- In Rust, on every corpus set (`tests/nmr_processing_2d.rs`): the numpy conventions held. Three further findings. JEOL `real_complex` rows (nmrXiv s200 COSY) need no conjugation in F1: the diagonal then runs from high to high shift (44 % of the intensity on it, 3 % conjugated). VnmrJ F1 runs the other way from Bruker's: matching nmrglue run as a black box after the NMRPipe scripts of the nmrglue test data (no reversal for States, `REV` after the TPPI transform) needs the F1 interferogram conjugated (States 0.83 against 0.57; TPPI 0.86 against 0.05, magnitudes binned). TopSpin's `tilt` moves each F1 row along F2 by +F1 Hz in our point order (J-resolved `bmse000797`: 0.94 against 0.58 the other way).
