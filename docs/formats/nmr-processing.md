# NMR processing

OpenReadout can turn NMR FIDs into spectra, pick peaks and integrate regions. This works on top of the Bruker, Varian/Agilent, JEOL, Magritek Spinsolve and JCAMP-DX readers. Use it with `openreadout analyze nmr-peaks`, with `--process` on `trace`, `export` and `preview`, or through the MCP tool `openreadout_analyze` (kind `nmr-peaks`). This page describes the processing steps, their defaults and how they were validated. It is not a file format; its vocabulary table covers the analysis code. Code: `crates/openreadout-signal/src/nmr/`. Provenance: [`docs/provenance/nmr-processing.md`](https://github.com/openreadout/openreadout/blob/main/docs/provenance/nmr-processing.md).

## What it does

`analyze nmr-peaks FILE` takes a spectrum and returns a peak list and integrals:

- `--from auto` (default): the vendor's processed spectrum when the data set has one (Bruker
  `pdata/<n>/1r`, a JEOL processed `.jdf`, a JCAMP-DX NMR spectrum), else the FID processed here;
  `--from fid` always processes the FID; `--from processed` only uses a stored spectrum.
- `trace/export/preview --process` present every complex FID trace as its processed spectrum
  (same trace index; channels `real`, `imag`, or `magnitude`; `extra.kind = processed_spectrum`,
  `extra.axis` in ppm), so `export --process --to jcamp|csv|parquet` writes the spectrum.

Only 1-D processing is done: a `ser` or arrayed FID is processed row by row (`--sweep`), the
indirect dimension of 2-D data is not transformed.

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
  `normalized` rescales so region I has value V (`--integral-reference I=V`, default `0=1`).

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

## Known gaps

- 1-D only; no second-dimension transform, NUS reconstruction or linear prediction.
- Linear prediction (Bruker `ME_mod`, VnmrJ `proc='lp'`) is not done: spectra the vendor processed
  with it differ at the start of the FID (a note says so). Varian `lp` is not reproduced.
- Real-only (`AQ_mod` 0/2) FIDs are refused (exit 6).
- Automatic phasing can miss on noise-dominated spectra (S/N < ~30) and on first-order phases
  beyond ±1440°.

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
| `NmrRequest`, `trace`, `sweep`, `process`, `baseline_on_stored`, `peaks`, `integrals`, `integral_reference` | `analyze nmr-peaks` request |
| `NmrReport`, `path`, `format`, `trace_name`, `processing`, `noise_sd`, `peak_options`, `peak_count`, `main_peak` | `analyze nmr-peaks` output |
| `ChosenSpectrum`, `spectrum`, `load`, `analyze` | the spectrum used and the analysis entry points |
| `ProcessedNmrDataset`, `record` | dataset wrapper behind `--process` |
