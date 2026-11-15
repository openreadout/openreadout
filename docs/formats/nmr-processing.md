# NMR processing

OpenReadout can turn NMR FIDs into spectra, pick peaks and integrate regions. This works on top of the Bruker, Varian/Agilent, JEOL, Magritek Spinsolve and JCAMP-DX readers. Use it with `openreadout analyze nmr-peaks`, with `--process` on `trace`, `export` and `preview`, or through the MCP tool `openreadout_nmr_peaks`. This page describes the processing steps, their defaults and how they were validated. It is not a file format; its vocabulary table covers the analysis code. Code: `crates/openreadout-signal/src/nmr/`. Provenance: [`docs/provenance/nmr-processing.md`](https://github.com/openreadout/openreadout/blob/main/docs/provenance/nmr-processing.md).

## What it does

`analyze nmr-peaks FILE` takes a spectrum and returns a peak list and integrals:

- `--from auto` (default): the vendor's processed spectrum when the data set has one (Bruker
  `pdata/<n>/1r`, a JEOL processed `.jdf`, a JCAMP-DX NMR spectrum), else the FID processed here;
  `--from fid` always processes the FID; `--from processed` only uses a stored spectrum.
- `trace/export/preview --process` present every complex FID trace as its processed spectrum
  (same trace index; channels `real`, `imag`, or `magnitude`; `extra.kind = processed_spectrum`,
  `extra.axis` in ppm), so `export --process --format jcamp|csv|parquet` writes the spectrum.

The commands above process one row at a time: a `ser` or arrayed FID row by row (`--sweep`).
The library also transforms the indirect dimensions of 2D and 3D data and stacks pseudo-2D
series (`process_nd`, `load_processed_nd`, below); the commands do not use it yet.

## FID processing (`process_fid`)

1. the FID points the stored processing used (Bruker `TDeff`/2, when fewer than acquired), zero
   filled to `size` complex points (a power of two; default: the stored `SI`/`fn`/2, else the
   next power of two ≥ 2 × the FID);
2. digital-filter group delay `G`: the integer part ⌊G⌋ is removed by rotating the buffer left
   (the filter's pre-response ends up at negative time, the end of the buffer), the fractional
   part by a linear phase about the carrier after the FT, followed by a zero-order term
   π·(1 − frac): together the spectrum a shift by ⌈G⌉ points with the remaining fraction's ramp
   pivoted at the spectrum edge gives, which is the spectrum Bruker's stored phases refer to
   (measured on 46 TopSpin data sets, below);
3. apodization with the true time zero: exponential (`--lb`, default: stored `LB`/`lb`, else
   0.3 Hz for 1H/3H/19F and 1 Hz otherwise) or Gaussian (`--gb`);
4. first point × the stored factor (Bruker `FCOR`; default 0.5); forward FFT; index 0 =
   high-frequency edge, carrier at `size/2`;
5. phase `S_k·e^{−i(φ0 + φ1·k/N)}` (degrees, `k` from the left edge): `--phase default` uses the
   stored phases unless more than 20 % of the (baseline-corrected) signal comes out negative and
   automatic phasing does at least twice as well; `stored`, `auto`, `magnitude`, `none`, `P0,P1`;
6. baseline (`--baseline`): `default` = polynomial of order 1 through automatically recognised
   baseline blocks (block-flatness rule, 3σ-clipped refits); `poly:N`; `none`;
7. ppm axis: `first = ((carrier − reference)·10⁶ + SW/2)/reference`, `step = −SW/(N·reference)`.

Automatic phasing: ACME entropy minimisation (Chen et al. 2002) restricted to signal regions
(|S| > 5 noise SD, ±16 points) with a weak prior `0.05·(φ1/360°)²`, grid φ0 15° × φ1 45° over
±1440°, Nelder–Mead refinement.

### Parameters per vendor

| our name | Bruker | Varian/Agilent | JEOL | Magritek Spinsolve |
| --- | --- | --- | --- | --- |
| `spectral_width_hz` | `SW_h` | `sw` | `X_SWEEP` | `bandwidth` × 1000 |
| `carrier_frequency_mhz` | `SFO1` | `sfrq` | `X_FREQ` | `b1Freq` + (`lowestFrequency` + width/2)·10⁻⁶ |
| `reference_frequency_mhz` (0 ppm) | `SF` of the first `pdata/<n>/procs`, else `BF1` | `reffrq`, else from `rfl`, `rfp` | carrier / (1 + `X_OFFSET`·10⁻⁶) | `b1Freq` |
| `group_delay_points` | `GRPDLY`, else the DSPFVS/DECIM table | 0 | from `orders`/`factors` (below) | 0 |
| `conjugate` | no | yes | yes | yes |
| stored `phase0_deg`, `phase1_deg` | `PHC0`, `PHC1` | −`rp`, −`lp` | – | `p0Phase`, `p1Phase` (`proc.par`), else −p0, −p1 of `Phase(p0, p1)` in `processing.script` |
| stored `line_broadening_hz`, `size` | `LB` (0 when `WDW` is 0), `SI` | `lb`, `fn`/2 | – | 0 when `filter` is `no` (`exp:X` → X Hz, unverified); `nrPnts` × `zf` |
| stored `first_point_factor`, `time_domain_points` | `FCOR`, `TDeff`/2 | – | – | – |

JEOL group delay (inferred, see provenance): `orders` = `"s o1 … os"` (number of stages, filter
orders), `factors` = `"f1 … fs"` (decimation factors); delay = Σᵢ (oᵢ − 1)/2 / Πⱼ≥ᵢ fⱼ output
points (`"2 54 73"`, `"8  2"` → 19.656 points, which removed the pre-response in the corpus FIDs).

## Peak picking and integration (`pick_peaks`, `integrate`)

- noise SD: 64 blocks, per block 1.4826·MAD of the residual of a least-squares line, 25th
  percentile over blocks;
- a peak is a local maximum with height ≥ `--min-snr` (10) × noise SD, ≥ `--min-height-fraction`
  × the tallest point, and prominence (height above the higher of the two minima reached walking
  outwards to a higher point) ≥ `--min-prominence` (5) × noise SD;
- position and height by parabolic interpolation; width at half height by linear interpolation
  (`None` when an overlapping line keeps the signal from reaching half height);
- `--negative` also reports negative peaks;
- `--integrate A:B` (repeatable): sum of the points in the region × the point spacing in ppm;
  `normalized` rescales so region I has value V (`--integral-reference I=V`, default `0=1`);
- `--auto-integrate` (MCP `auto_integrate`), without `--integrate`: one region per multiplet,
  around each group of positive peaks closer than 15 Hz, reaching 10 Hz (or the outer peak's
  half width) beyond its outer peaks; regions that then overlap are merged.

## Validation (automated in `crates/openreadout-corpus-tests/tests/nmr_processing.rs`)

Oracle: `oracle/signal/nmr.py` → `corpus/oracle/nmr-processing/*.json`: TopSpin's own processed
spectrum of the same FID (`pdata/1/1r`, `1i`), its stored peak list and integrals, and an
nmrglue pipeline for comparison.

### Processing parity with TopSpin (2026-09-26: 46 experiments, 22 nmrXiv projects)

The FID processed here with TopSpin's stored parameters (`SI`, `LB`, `PHC0`, `PHC1`, `FCOR`,
`TDeff`, `SF`, `GRPDLY`) against TopSpin's `1r`: TopSpin 3.5–4.3, 300–600 MHz, 1H, 13C (incl.
DEPT and UDEFT), 31P, DSPFVS 10/20/21, DIGMOD 1 and 3. Columns: the correlation over signal
points (|1r| > 10 × its noise SD); peaks our picker finds against the 25 tallest SciPy finds on
`1r` (within 3 points) and the largest position offset; the same picker on both spectra after
the same 5th-order baseline correction (matched, largest relative height difference); integrals
over TopSpin's own regions, both spectra corrected and integrated identically, as the largest
absolute difference in units of the largest region; automatic phasing's signal correlation with
`1r`. The ppm axis equals TopSpin's on every experiment (`OFFSET`, 1e-4 ppm).

| experiment | nucleus, sequence | S/N | signal correlation | peaks (≤ 3 points) / max offset | heights, same picker | integrals Δ | automatic phasing |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `nmrxiv-s1082-80` | 1H zg30 | 257002 | 1.000000 | 25/25 / 0.45 pt | 25/25, 0.00 % | – | 0.99956 |
| `nmrxiv-s1082-81` | 13C zgpg30 | 612 | 1.000000 | 15/15 / 0.48 pt | 15/15, 0.00 % | – | 0.99974 |
| `nmrxiv-s1132-1` | 1H zg30 | 63639 | 0.999882 | 25/25 / 0.59 pt | 25/25, 1.60 % | 0.0020 | 0.99987 |
| `nmrxiv-s1132-2` | 13C zgpg30 · DSPFVS 10 | 3519 | 0.999939 | 25/25 / 0.41 pt | 25/25, 0.00 % | – | 0.87247 |
| `nmrxiv-s1148-1` | 1H zg30 | 198506 | 1.000000 | 25/25 / 0.50 pt | 25/25, 0.00 % | – | 0.99824 |
| `nmrxiv-s1247-10` | 1H zg30 | 66858 | 1.000000 | 25/25 / 0.44 pt | 25/25, 0.01 % | 0.0003 | 0.99987 |
| `nmrxiv-s1247-15` | 13C zgpg30 | 6205 | 1.000000 | 25/25 / 0.46 pt | 25/25, 0.00 % | – | 0.99947 |
| `nmrxiv-s1250-1` | 1H zg30 | 4205 | 1.000000 | 25/25 / 0.43 pt | 25/25, 0.00 % | 0.0014 | 0.99787 |
| `nmrxiv-s1250-2` | 1H zg30 | 4438 | 0.999997 | 25/25 / 0.44 pt | 25/25, 0.00 % | – | 0.99767 |
| `nmrxiv-s1250-3` | 1H zg30 | 2576 | 0.999998 | 23/23 / 0.49 pt | 23/23, 0.00 % | 0.0015 | 0.99867 |
| `nmrxiv-s1250-4` | 1H zg30 | 4216 | 0.999998 | 25/25 / 0.45 pt | 25/25, 0.00 % | 0.0002 | 0.99449 |
| `nmrxiv-s1250-5` | 1H zg30 | 3898 | 0.999994 | 25/25 / 0.45 pt | 25/25, 0.01 % | 0.0010 | 0.99028 |
| `nmrxiv-s1250-6` | 1H zg30 | 1642 | 0.999999 | 25/25 / 0.47 pt | 25/25, 0.01 % | 0.0006 | 0.99703 |
| `nmrxiv-s1439-1h-nmr` | 1H zg30 | 15916 | 1.000000 | 25/25 / 0.50 pt | 25/25, 0.00 % | – | 0.99871 |
| `nmrxiv-s1462-13c-nmr` | 13C zgpg30 | 506 | 1.000000 | 25/25 / 0.45 pt | 25/25, 0.01 % | – | 1.00000 |
| `nmrxiv-s1462-1h-nmr` | 1H zg30 | 25544 | 1.000000 | 25/25 / 0.47 pt | 25/25, 0.00 % | – | 0.99917 |
| `nmrxiv-s1462-31p-nmr` | 31P zgig | 1465 | 1.000000 | 5/5 / 0.49 pt | 5/5, 0.00 % | – | 0.99980 |
| `nmrxiv-s275-1` | (earlier set) | 2643613 | 1.000000 | 25/25 / 0.47 pt | 25/25, 0.00 % | – | 1.00000 |
| `nmrxiv-s275-11` | (earlier set) | 5769 | 1.000000 | 25/25 / 0.50 pt | 25/25, 0.00 % | – | 0.99958 |
| `nmrxiv-s275-13` | (earlier set) | 5 | NaN | 0/0 / 0.00 pt | 0/0, 0.00 % | – | NaN |
| `nmrxiv-s275-21` | (earlier set) | 25 | 0.996739 | 12/12 / 0.49 pt | 12/12, 1.97 % | – | 0.67171 |
| `nmrxiv-s279-20` | 1H zg30 | 5287 | 0.999995 | 22/24 / 0.48 pt | 22/22, 0.02 % | 0.0014 | 0.99566 |
| `nmrxiv-s279-21` | 13C udeft · LP (ME_mod 2) | 476 | 0.999812 | 17/17 / 2.69 pt | 16/17, 1.05 % | – | 0.99964 |
| `nmrxiv-s501-7` | (earlier set) | 93133 | 0.999994 | 25/25 / 0.48 pt | 25/25, 0.03 % | – | 0.99867 |
| `nmrxiv-s596-1` | (earlier set) | 5825 | 1.000000 | 2/2 / 0.43 pt | 2/2, 0.00 % | – | 0.99996 |
| `nmrxiv-s597-1` | 13C zgpg30 · DSPFVS 10 | 1841 | 1.000000 | 6/6 / 0.37 pt | 6/6, 0.01 % | – | 0.99783 |
| `nmrxiv-s699-1` | 1H zg30 | 32138 | 1.000000 | 25/25 / 0.45 pt | 25/25, 0.00 % | 0.0000 | 0.99948 |
| `nmrxiv-s699-2` | 13C zgpg30 | 1191 | 0.999999 | 18/18 / 0.36 pt | 18/18, 0.00 % | – | 0.99959 |
| `nmrxiv-s702-1` | 1H zg30 | 16318 | 1.000000 | 25/25 / 0.50 pt | 25/25, 0.00 % | 0.0000 | 0.99866 |
| `nmrxiv-s702-2` | 13C zgpg30 | 609 | 1.000000 | 4/4 / 0.44 pt | 4/4, 0.00 % | – | 0.99991 |
| `nmrxiv-s736-1` | 1H zg30 | 49740 | 1.000000 | 25/25 / 0.49 pt | 25/25, 0.00 % | 0.0001 | 0.99900 |
| `nmrxiv-s736-2` | 13C zg0pg · LP (ME_mod 4) | 1061 | 0.902327 | 15/20 / 2.62 pt | 15/20, 7.68 % | – | 0.92156 |
| `nmrxiv-s740-1` | 1H zg30 | 41073 | 0.999730 | 25/25 / 0.50 pt | 25/25, 0.00 % | – | 0.99971 |
| `nmrxiv-s740-4` | 13C zgpg | 515 | 0.999983 | 22/22 / 0.43 pt | 22/22, 0.00 % | – | 0.99361 |
| `nmrxiv-s753-20` | 1H zg30 | 103482 | 0.999994 | 25/25 / 0.49 pt | 25/25, 0.00 % | – | 0.99936 |
| `nmrxiv-s753-proton-25` | 1H zg30 | 122137 | 0.999992 | 25/25 / 0.50 pt | 25/25, 0.00 % | – | 0.99871 |
| `nmrxiv-s753-proton-m25` | 1H noesyigld1d | 209770 | 0.999998 | 25/25 / 0.48 pt | 24/25, 0.00 % | – | 0.99960 |
| `nmrxiv-s846-50` | (earlier set) | 4886 | 1.000000 | 25/25 / 0.50 pt | 25/25, 0.00 % | 0.0000 | 0.99542 |
| `nmrxiv-s853-1` | 1H zg30 | 11419 | 1.000000 | 25/25 / 0.48 pt | 25/25, 0.00 % | – | 0.99539 |
| `nmrxiv-s853-2` | 13C udeft · LP (ME_mod 2) | 1024 | 0.999846 | 22/22 / 0.67 pt | 25/25, 0.39 % | – | 0.64338 |
| `nmrxiv-s853-3` | 13C deptsp90 | 32 | 1.000000 | 4/4 / 0.15 pt | 4/4, 0.00 % | – | 0.69809 |
| `nmrxiv-s853-4` | 13C deptsp135 | 40 | 1.000000 | 1/1 / 0.14 pt | 1/1, 0.00 % | – | 0.71651 |
| `nmrxiv-s882-1` | 1H zg30 | 9899 | 1.000000 | 25/25 / 0.47 pt | 25/25, 0.01 % | 0.0001 | 0.99983 |
| `nmrxiv-s886-1` | 1H zg30 | 36603 | 1.000000 | 25/25 / 0.47 pt | 25/25, 0.01 % | 0.0003 | 0.99943 |
| `nmrxiv-s897-1` | 1H zg30 | 54364 | 0.999870 | 25/25 / 0.47 pt | 25/25, 0.07 % | 0.0050 | 0.98761 |
| `nmrxiv-s904-1` | 1H zg30 | 11406 | 1.000000 | 25/25 / 0.50 pt | 25/25, 0.00 % | 0.0001 | 0.99937 |


Summary: every experiment TopSpin processed with steps done here (43 of 46) reproduces `1r` at
signal correlation ≥ 0.99988 (41 at ≥ 0.99999), every peak within 0.6 point, heights within 1.6 %,
integrals within 0.012 of the largest region. The group-delay zero-order term took the DIGMOD 1
experiments from 0.9985–0.9993 to 1.000000 and turned the three "inverted" DSPFVS 10 data sets
(−0.96) into 0.99994–1.000000; `FCOR` and the per-experiment `TDeff` removed the constant offset
that made FID-based integrals of `FCOR = 1` data up to 48 % off. Not reproduced (reported, not
asserted): linear prediction (`ME_mod` 2/4: `s279-21`, `s853-2`, `s736-2`; the last also has
`TDoff` −16 and analog-filter data, correlation 0.90) and noise-dominated spectra (S/N < 100).
TopSpin's stored `integrals.txt` values are themselves reproduced within 3.4 % by integrating
`1r` here (TopSpin corrects each region's bias and slope); TopSpin's `peaklist.xml` is found
within 0.002 ppm for every current list (`s740-1`'s list predates its referencing and is
recognised as stale).

Automatic phasing reaches ≥ 0.99 on every phased experiment except `s1132-2` (13C, an old
DSPFVS 10 console: 0.872, a second minimum of the objective) and the LP-processed 13C spectra.

Further checks:

| data set | against | result |
| --- | --- | --- |
| `nmrxiv-s846-50` (1H, TopSpin 3.5) | TopSpin `peaklist.xml`, `integrals.txt` | 32/32 stored peaks found within 0.002 ppm; integrals over TopSpin's 5 regions from the FID within 0.28 % (on TopSpin's own `1r` 0.20 %) |
| `nmrxiv-s200-qhnmr-jdf` (JEOL Delta 1H FID) | MestReNova-processed JCAMP-DX of the same acquisition | 20/25 peaks within 0.003 ppm (max 0.0028 ppm) |
| nmrglue pipeline on the same Bruker FIDs | – | reported: nmrglue's digital-filter removal reproduces TopSpin less closely (correlation −0.08–0.998) |

### Magritek Spinsolve (2026-09-26: 135 FID rows, 9 experiments, 2 depositors)

The Spinsolve software keeps its own processed spectra in plot files (`spectrum.pt1`,
`*-Spectra.pt1`: one per FID row); `oracle/spinsolve.py` locates each plotted array by its ppm
axis and the test compares the FID row processed here (the software's phase from
`processing.script`, no baseline) point for point:

| experiment | rows | software phase | ppm axis max \|Δ\| | correlation (worst row) | largest difference / max (worst row) | automatic phasing, \|signal corr\| (single-sign rows) |
| --- | --- | --- | --- | --- | --- | --- |
| `spinsolveproc-proton` (80 MHz, 1D, `proc.par`) | 1 | −0.35° | 3.8e-6 ppm | 1.0000000 | 2.0e-7 | 0.976 |
| `spinsolveproc-t1` (80 MHz, T1 series) | 10 | 156.16° | 3.8e-6 ppm | 1.0000000 | 1.2e-2 | 0.953 |
| `spinsolve-zenodo20597567-*-t1` (5 × 60 MHz T1 series) | 90 | 289–308° | ≤ 9.1e-6 ppm | 1.0000000 | ≤ 2.0e-3 | ≥ 0.986 (16 rows near the inversion null not asserted) |
| `spinsolve-zenodo20597567-*-slic` (3 × 60 MHz SLIC series) | 50 | 0 (unphased) | ≤ 8.6e-6 ppm | 1.0000000 | ≤ 2.0e-3 | – (no phasing reference) |

So the conventions (conjugated FID, ppm = (`lowestFrequency` + k·width/n)/`b1Freq`, first point ×
0.5, no apodization with `filter = "no"`, phase = −p0 of `Phase(p0, p1)`) reproduce the software
exactly; the remaining differences on series rows are at the 10⁻³ level of the tallest line
(float32 values in the plot file). The first-order phase sign and `exp:X` apodization are
unverified (every public directory has p1 = 0 and `filter = "no"`).

Varian: no vendor-processed spectra are public in the corpus; TMS (0.000 ppm) and CHCl3 (7.263 ppm)
land where expected in `nmrxiv-s325` 1H, and `rp` with the sign inverted phases the s325 and s501
spectra (≤ 7 % negative signal). Stored `lp` was not reproduced on the two files with a non-zero
value, and VnmrJ data processed with linear prediction and a long receiver delay (`nmrxiv-s501`
CARBON_01: `proc='lp'`, `ddrtc` 364 µs) need a first-order phase beyond the automatic search range.

**Performance** (release build, Apple M-series, 2026-09-24): `analyze nmr-peaks --from fid` on
`nmrxiv-s846-50` (65,536 complex points, 131,072-point transform) 0.03 s with stored phases,
0.54 s with automatic phasing, 23 MB peak memory. The largest transform is 2²² points
(`MAX_TRANSFORM_SIZE`).

## Multidimensional processing (`process_nd`, `load_processed_nd`)

Code: `processnd.rs` (pipeline), `indirect.rs` (parameters per format, loader), `window.rs`,
`lp.rs`. `load_processed_nd(ds, info, trace, &NdOptions)` reads every row of an FID trace and
the parameters of every dimension and returns a `SpectrumNd`: `shape` and `axes` slowest
dimension first (2D: `[F1, F2]`), `real` row-major, optional hypercomplex `components`, the
`series` list of a pseudo-2D data set, and a `record` of what was done per dimension.

1. Direct dimension, each row: the 1D steps above (stored `TDeff`, group delay, window from the
   true time zero, `FCOR`, transform, fractional group-delay term), optional linear prediction,
   then the phase.
2. Each indirect dimension, fastest first: rows combined into complex interferograms by the
   encoding, optional linear prediction, window, first point × `FCOR` (default 0.5), zero filling,
   transform in the same order as F2 (index 0 = high frequency), phase.
3. Magnitude or power over the dimensions that ask for it, `REVERSE`, the direct strip
   (`STSR`/`STSI`), optional `tilt` (J-resolved, with symmetrisation about F1 = 0) and
   `symmetrize` (TopSpin `sym`), baseline per dimension (default none), ppm axes.

Encodings (rows `2k` = u, `2k+1` = v; i₂ = the F2 imaginary unit):

| `Encoding` | Bruker `MC2` (FnMODE − 1) | Varian | JEOL | interferogram point k |
| --- | --- | --- | --- | --- |
| `Complex` | 0 QF | `phase` = 1 | `real_complex` | conj(row k) |
| `Sequential` | 1 QSEQ | – | – | as `Tppi` (not verified) |
| `Tppi` | 2 TPPI | `phase` = 3 | `tppi` | real points at 2·SW: 2N real points transformed, first N bins conjugated |
| `States` | 3 States | `phase` = 1,2 | `complex` | Re₂(u) + i·Re₂(v) |
| `StatesTppi` | 4 States-TPPI | – | – | as `States`, × (−1)^k |
| `EchoAntiecho` | 5 echo/antiecho | `f1coef` with 8 values | – | Re₂(C) − i·Re₂(S), C = i₂(u + v), S = u − v |
| `Series` | pseudo-2D (below) | arrays other than `phase` | – | not transformed |

FnMODE 0 (undefined) uses the stored `MC2`, else a guess from the pulse program (`qf` →
`Complex`, `et`/`ea` → `EchoAntiecho`, else `StatesTppi`, with a note).

Defaults per dimension (`DimensionOptions`, `None`/`Default` fields): the stored transform size,
window, phases, phase mode (Bruker `PH_mod` 2 = magnitude, 3 = power; phases stored with
`PH_mod` 0 are applied, since TopSpin's later `apk`/`apkm` store them there), first-point factor,
points used (`TDeff`), linear prediction (`ME_mod`, `NCOEF`, `LPBIN`) and `REVERSE`. Without
stored values: cosine-squared window (sine power 2, offset 0.5) in every dimension of 2D data and
the 1D exponential for the F2 of a series; a size of the next power of two ≥ 2 × the points;
magnitude for `Complex` dimensions and automatic phasing for the others (F2 on the first
increment, F1 on the column through the strongest point). `NdOptions` adds `ignore_group_delay`,
`symmetrize`, `tilt` and `keep_components`. Windows: `Window` { `None`, `Exponential`,
`Gaussian`, `LorentzToGauss`, `Sine` (offset, power), `Trapezoid`, `Sinc`, `Traficante` }, mapped
from Bruker `WDW` 0–10 (`window.rs` module docs; sine and exponential windows were checked against
TopSpin, the others follow their published definitions).

Linear prediction (`LinearPrediction` { `direction`, `coefficients`, `points` }): Burg
coefficients, `Forward` to `points` rows (Bruker `LPBIN`), `Backward` (replaces the first points),
`MirrorForward` (`ME_mod` 5/6: the series mirrored to negative time by conjugation before the
fit). Burg's coefficients stay stable when `NCOEF` approaches the number of points; they are
biased towards weaker damping, which shows on short, fast-decaying interferograms.

Pseudo-2D series: a Bruker experiment whose directory holds `difflist`, `vdlist`, `vclist`,
`vplist`, `vtlist` or `valist` and whose F1 is not hypercomplex, or whose pulse program is a
diffusion or relaxation sequence (`ledbp`, `stebp`, `dstebp`, `dosy`, `diff`; `t1ir`, `t1rho`,
`cpmg`, `t2` at the start), every Spinsolve `data.2d` (delays from `minDelay`, `maxDelay`,
`delaySpacing`, `nrSteps`, in ms) and Varian arrays: F2 transformed row by row, the rows kept, the
varied values in `SpectrumNd::series` (`name`, `values`, `unit`, `source`).

Non-uniform sampling: `place_on_grid(rows, schedule, dims, full)` puts the measured rows of a
`nuslist` on the full grid with zeros elsewhere; `load_processed_nd` does that for Bruker NUS
data (with a note) and processes the zero-filled grid. A reconstruction step fills the same grid
and calls `process_nd`.

Projections and slices: `SpectrumNd::projection(axis, Projection::{Skyline, NegativeSkyline,
Sum})`, `trace(axis, &indices)`, `trace_at_ppm(axis, &ppm)` (2D: `trace_at_ppm(1, &[f1])` is the
row at F1 shift f1, `trace_at_ppm(0, &[f2])` the column at F2 shift f2), `point_of`, `get`.

Bruker DSPFVS 11/12 firmware (table group delay with a fraction): 180° is added to the stored F2
phase; without it TopSpin's `1r` and `2rr` come out inverted (s572, 1D and 2D).

### Parity with TopSpin's `2rr` (2026-11-14; `tests/nmr_processing_2d.rs`)

The `ser` processed with the stored parameters against TopSpin's `pdata/1/2rr`: signal
correlation over |2rr| > 10 × noise SD (all-point correlation in brackets when it differs), the
tallest point within one point in both dimensions, both ppm axes equal to TopSpin's `OFFSET`.

| experiment | pulse program, encoding | F1 linear prediction | signal (all) correlation | notes |
| --- | --- | --- | --- | --- |
| `bmrb-bmse000008-cosy` | cosyqf45, QF magnitude | – | 0.999995 | |
| `bmrb-bmse001104-hsqc` | hsqcdietgpsisp.2, EA | LPfc 32 | 0.999805 | |
| `bmrb-bmse001250-hmbc` | hmbcetgpl3nd, EA | LPfc 32 | 0.999857 | |
| `bmrb27037 …Chsqc…` | hsqcetgpsp, EA | LPfc 32 | 0.998967 | stored F1 width edited (SW_p ≠ SW_h): F1 axis not compared |
| `nmrxiv-s1247/12` | cosygpppqf, QF | – | 0.999928 | |
| `nmrxiv-s1247/13` | hsqcetgpsi, EA | – | 0.999982 (0.994338) | |
| `nmrxiv-s1247/14` | impact-hmbc, EA, F2 magnitude | – | 0.999983 | `TDeff` 104 of 256 rows |
| `nmrxiv-s1407/COSY` | cosygpppqf, QF | LPfc 32 | 0.998039 | |
| `nmrxiv-s1517/16` | noesygpphpp, States-TPPI | – | 0.999992 | |
| `nmrxiv-s1608/RPDLF` | NEA_RPDLF (solid state), FnMODE 0 → MC2 QF | – | 0.999918 | |
| `nmrxiv-s1617/COSY` | cosygpppqf, QF | LPfc 32 | 0.999948 | |
| `nmrxiv-s1617/HMBC` | hmbcetgpl3nd, EA, F2 magnitude | LPfc 60 of 64 points | 0.990955 (0.954147) | prediction order near the number of points |
| `nmrxiv-s1617/HSQC` | hsqcedetgpsisp2.3, EA | LPfc 60 of 64 points | 0.987163 | same |
| `nmrxiv-s1617/TOCSY` | mlevphpp, States-TPPI | LPfc 32 | 0.999981 | |
| `nmrxiv-s2331/11` | cosygpppqf, QF | LPfc 32 | 0.994912 | |
| `nmrxiv-s2331/12` | hsqcedetgp, EA | LPfc 32 | 0.999765 | |
| `nmrxiv-s2331/13` | hmbcetgpl3nd, EA | LPfc 32 | 0.999550 | |
| `nmrxiv-s2331/15` | noesygpphzs, States-TPPI | LPfc 32 | 0.999933 | |
| `nmrxiv-s279/22` | cosygpmfqf, QF | – | 0.999995 | |
| `nmrxiv-s314/202` | noesygpph, TPPI | – | 0.999999 | |
| `nmrxiv-s38/12` | cosygpqf, QF + `sym` | LPfc 16 | 0.999775 | |
| `nmrxiv-s38/15` | hsqcetgp, EA | LPfc 16 | 0.998650 | |
| `nmrxiv-s38/16` | hmbcgplpndqf, QF | LPmifc 90 | 0.995837 | mirror-image prediction |
| `nmrxiv-s38/17` | noesyetgp, EA | LPfc 16 | 0.996670 | |
| `nmrxiv-s49/5` | cosygpppqf, QF + `sym` | LPfc 32 | 0.997659 | |
| `nmrxiv-s49/6` | hsqcedetgpsisp2.3, EA | LPfc 32 | 0.999884 | |
| `nmrxiv-s49/7` | hmbcetgpl3nd, EA | LPfc 32 | 0.999764 | |
| `nmrxiv-s501/6` | roesyphpr.2, States-TPPI | LPfc 32 | 0.999977 | |
| `nmrxiv-s572/3` | cosygpmfph, States-TPPI (DSPFVS 12) | – | 0.999103 | |
| `nmrxiv-s572/4` | noesygpph, States-TPPI (DSPFVS 12) | – | 0.997785 | F2 EM 5 Hz; the residual is not explained |
| `nmrxiv-s572/5` | hsqcedetgpsp, EA (DSPFVS 12) | – | 0.999995 | |
| `nmrxiv-s572/6` | hmbcgpl2ndqf, QF | – | 0.999191 | |
| `nmrxiv-s73/4` | cosygpppqf, QF | LPfc 32 | 0.999311 | |
| `nmrxiv-s73/5` | hsqcedetgpsisp2.3, EA | LPfc 32 | 0.999980 | |
| `nmrxiv-s73/6` | hmbcgplpndqf, QF | LPfc 32 | 0.979348 | prediction of a short magnitude HMBC |
| `nmrxiv-s736/3` | cosygpppqf, QF | – | 0.999989 | |
| `nmrxiv-s736/4` | hsqcedetgpsisp.2, EA | – | 0.999995 | |
| `nmrxiv-s736/5` | hmbcgplpndqf, QF | – | 0.999996 | |
| `nmrxiv-s736/6` | noesygpphpp, TPPI | – | 0.999985 | |
| `nmrxiv-s736/7` | hsqcetgpsi (15N), EA | – | 0.999917 (0.986998) | |
| `nmrxiv-s736/8` | hmbcgpndqf (15N), QF | – | 0.999915 (0.978266) | |
| `nmrxiv-s740/2` | hsqcetgpsi, EA | – | 0.999996 | |
| `nmrxiv-s740/3` | hmbcgplpndqf, QF | – | 0.999998 | |
| `nmrxiv-s740/5` | roesyphpp, TPPI | – | 0.999988 | |
| `nmrxiv-s853/8` | noesygpphpp, States-TPPI | – | 0.999980 | |

Summary: 30 of the 31 experiments TopSpin made without linear prediction reach a signal
correlation ≥ 0.9991 (24 at ≥ 0.9999); the exception is s572/4 (0.9978). With Burg linear
prediction in F1, 14 of 21 reach ≥ 0.999 and all but three ≥ 0.994; the three below are short
interferograms (64 complex points) predicted with 32–60 coefficients. Differences in the
all-point correlation of the 15N and some HSQC spectra are in the noise: TopSpin's `abs1`/`abs2`
baseline corrections after the transform are not reproduced.

Reported, not asserted: J-resolved `bmse000797` (with `tilt`: 0.938; TopSpin's `abs1` in F1
after `tilt` and `symj` is not reproduced),
non-uniformly sampled sets (s279/23, /24, s753/21, s853/5–7, s1407 HSQC/HMBC, s1517/13–15: the
zero-filled grid correlates 0.30–0.97 with TopSpin's reconstruction), solvent-filtered sets
(`BC_mod` qfil: s314/20, /200, `nmrtools/15n-trosy-ts3`) and solid-state MQMAS
(`desperate/RbNO3_MQMAS`, which TopSpin shears). DOSY (`nmrxiv-s519/310–321`): TopSpin's `2rr`
is the diffusion-transformed spectrum, so the first F2 row processed here is compared with its sum
over F1.

Other vendors (no vendor-processed 2D data are public in the corpus):

| data set | encoding | against | result |
| --- | --- | --- | --- |
| `nmrglue-agilent-2d` | States (`phase` = 1,2) | nmrglue pipeline after the NMRPipe script of the test data (black box), magnitudes binned to 64 × 256 | correlation 0.83 with both axes the same way round (0.57 with F1 reversed); the windows differ |
| `nmrglue-agilent-2d-tppi` | TPPI (`phase` = 3) | the same, with the script's `REV` after the F1 transform | 0.86 (0.05 reversed) |
| `nmrxiv-s200` COSY (JEOL, `real_complex`) | `Complex`, magnitude | the diagonal | 44 % of the intensity within 2 % of the width of the diagonal (3 % with F1 conjugated) |
| JEOL `nmrxiv-s200` HSQC/HMBC, `figshare29098781-hmbc`, `zenodo19692590-discomycin-hsqc` | States / `Complex` | – | finite, axes 170…0 ppm (13C) and 225…−25 ppm; the s200 HSQC/HMBC are NUS (processed as measured, with a note) |
| 13 Spinsolve `data.2d` (T1, T2, SLIC, PGSTE) | `Series` | – | every row kept; delays from `minDelay`/`maxDelay`/`nrSteps` where present (9 of 13) |
| `nmrglue-bruker-3d` (HNCA, no `acqu3s`) | States × States, dimensions from the test data's conversion script | – | 128 × 128 × 2048 spectrum, finite, 64 s (corpus build) |

## Known gaps

- The commands (`analyze nmr-peaks`, `--process`) use the 1D path; the multidimensional API is
  library-only until it is wired in.
- No NUS reconstruction (zero-filled grid only), no solvent filter (`BC_mod` 5/6), no shearing
  (MQMAS), no TopSpin `abs1`/`abs2` baseline; `tilt`/`symmetrize` run only when asked.
- Linear prediction in F1 is Burg's: it falls short of TopSpin when `NCOEF` is close to the
  number of points. Varian `lp` (1D) is not reproduced; VnmrJ `proc1='lp'` is not read.
- Varian windows other than `lb1`, VnmrJ 3D row order, JEOL NUS and Spinsolve delay spacing are
  taken as documented in `indirect.rs` and not verified on vendor spectra.
- Real-only (`AQ_mod` 0/2) FIDs are refused (exit 6).
- Bruker DIGMOD 3 FIDs processed here sit on a baseline about 0.3 % of the tallest line away
  from TopSpin's `1r` (water-suppressed metabolomics spectra, 2026-10-11). Weak lines of nearly
  equal height can then rank differently; when the data set has TopSpin's spectrum, the default
  (`--from auto`) uses it.
- Automatic phasing can miss on noise-dominated spectra (S/N < ~30), on first-order phases
  beyond ±1440°, and on crowded water-suppressed spectra in D2O (signal correlation 0.97 on two
  plant-extract spectra, where it settles on a first-order phase near 90°). The default uses the
  stored phases when the data set has them.

## Vocabulary (public identifiers of `crates/openreadout-signal/src/nmr/*.rs`, `lib.rs`, `fft.rs`)

| identifier | meaning |
| --- | --- |
| `Complex`, `re`, `im`, `new`, `from_phase`, `conj`, `abs`, `scale`, `fft_in_place`, `next_pow2` | complex numbers and the radix-2 FFT |
| `FidParameters`, `spectral_width_hz`, `carrier_frequency_mhz`, `reference_frequency_mhz`, `group_delay_points`, `conjugate`, `nucleus` | acquisition parameters processing needs |
| `StoredProcessing`, `source`, `size`, `line_broadening_hz`, `phase0_deg`, `phase1_deg`, `spectrum_stored`, `first_point_factor`, `time_domain_points`, `usable_phases` | processing values the vendor software stored (`first_point_factor`: Bruker `FCOR`; `time_domain_points`: `TDeff`/2) |
| `Apodization` { `None`, `Exponential`, `Gaussian` } | window |
| `PhaseMode` { `Default`, `Stored`, `Auto`, `Manual`, `Magnitude`, `None` } | phasing choice |
| `ProcessOptions`, `apodization`, `phase`, `baseline`, `ignore_group_delay` | processing options |
| `PpmAxis`, `first_ppm`, `step_ppm`, `ppm`, `index_of`, `last_ppm`, `hz_per_point`, `ppm_axis` | chemical-shift axis |
| `Spectrum`, `real`, `imag`, `axis` | a processed spectrum |
| `ProcessingRecord`, `steps`, `fid_points`, `apodization_source`, `phase_mode`, `parameters`, `stored`, `notes` | what processing did |
| `process_fid`, `MAX_TRANSFORM_SIZE`, `STORED_PHASE_MAX_NEGATIVE` | processing entry point and limits |
| `apply_phase`, `autophase`, `acme_objective`, `negative_fraction`, `signal_points`, `wrap_degrees`, `NEGATIVE_PENALTY`, `FIRST_ORDER_PRIOR`, `MAX_FIRST_ORDER` | phasing |
| `BaselineMode` { `None`, `Polynomial`, `Default` }, `order`, `polynomial_baseline`, `correct` | baseline correction |
| `noise_sd`, `PeakOptions`, `min_snr`, `min_height_fraction`, `min_prominence_snr`, `include_negative`, `range_ppm`, `max_peaks`, `pick_peaks` | peak picking |
| `Peak`, `hz`, `index`, `height`, `width_hz`, `width_ppm`, `snr` | one peak |
| `Integral`, `from_ppm`, `to_ppm`, `value`, `normalized`, `points`, `integrate` | one integral |
| `SpectrumSource` { `Auto`, `Fid`, `Processed` }, `is_fid`, `spectrum_axis`, `choose_trace`, `load_spectrum`, `fid_parameters`, `read_fid`, `jeol_group_delay` | choosing and reading a spectrum or FID from any NMR reader |
| `NmrRequest`, `trace`, `sweep`, `process`, `baseline_on_stored`, `peaks`, `integrals`, `auto_integrals`, `integral_reference` | `analyze nmr-peaks` request |
| `multiplet_regions`, `MULTIPLET_GAP_HZ`, `MULTIPLET_MARGIN_HZ` | automatic integration regions |
| `NmrReport`, `path`, `format`, `trace_name`, `processing`, `noise_sd`, `peak_options`, `peak_count`, `main_peak` | `analyze nmr-peaks` output |
| `ChosenSpectrum`, `spectrum`, `load`, `analyze` | the spectrum used and the analysis entry points |
| `ProcessedNmrDataset`, `record` | dataset wrapper behind `--process` |
| `process_nd`, `MAX_ND_POINTS`, `SpectrumNd`, `shape`, `components`, `axes`, `series`, `ndim`, `get`, `point_of`, `trace_at_ppm`, `projection` | multidimensional processing entry point, limit and result |
| `Component`, `mask`, `values` | one hypercomplex component of a result |
| `Projection` { `Skyline`, `NegativeSkyline`, `Sum` } | how a projection combines points |
| `Encoding` { `Complex`, `Sequential`, `Tppi`, `States`, `StatesTppi`, `EchoAntiecho`, `Series` }, `rows_per_point`, `is_fourier` | sampling of an indirect dimension |
| `Series`, `name`, `unit` | the varied parameter of a pseudo-2D series |
| `DimensionParameters`, `rows`, `encoding` | acquisition parameters of an indirect dimension |
| `StoredDimension`, `window`, `phase_mode`, `linear_prediction`, `reverse`, `strip` | stored processing of one dimension |
| `StoredPhaseMode` { `Phased`, `Magnitude`, `Power` } | stored phase mode (Bruker `PH_mod`) |
| `DimensionOptions`, `power`, `first_point_factor` | per-dimension options |
| `LpMode` { `Default`, `None`, `Manual` }, `prediction` | linear prediction choice |
| `NdOptions`, `direct`, `indirect`, `symmetrize`, `tilt`, `keep_components` | multidimensional options |
| `DimensionRecord`, `label`, `encoding`, `points`, `window_source`, `reversed`, `NdRecord`, `dimensions` | what was done per dimension |
| `Window` { `None`, `Exponential`, `Gaussian`, `LorentzToGauss`, `Sine`, `Trapezoid`, `Sinc`, `Traficante` }, `gaussian_position`, `offset`, `power`, `rise`, `fall`, `center`, `width`, `weights` | window functions |
| `LinearPrediction`, `direction`, `coefficients`, `points`, `LpDirection` { `Forward`, `Backward`, `MirrorForward` }, `burg_coefficients`, `predict` | linear prediction |
| `NdParameters`, `direct_stored`, `schedule`, `nd_parameters`, `read_rows`, `place_on_grid`, `load_processed_nd`, `bruker_encoding`, `bruker_window`, `parse_list` | parameters of every dimension per format and the loader |
