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

## Known gaps

- 1-D only; no second-dimension transform, NUS reconstruction or linear prediction.
- Linear prediction (Bruker `ME_mod`, VnmrJ `proc='lp'`) is not done: spectra the vendor processed
  with it differ at the start of the FID (a note says so). Varian `lp` is not reproduced.
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

## 2D and nD peak picking and volume integration (`peaks2d`, `source2d`, `analyze_2d`)

`analyze_2d` reads a stored 2D or 3D spectrum and returns its cross peaks, their volumes and
the integrals of boxes the caller gives. The picker itself (`pick_cross_peaks`) takes a
row-major matrix, its shape and one `PpmAxis` per dimension (slowest first: F1, F2), so it
works on any matrix, including spectra transformed here later.

### Reading the matrix (`load_matrix`)

| reader | trace | axes and nuclei |
| --- | --- | --- |
| Bruker | `pdata/<n>` with `extra.shape` (`2rr`, `3rrr`) | `extra.indirect_axes`, `extra.axis`; `AXNUC` of `procs`, `proc2s`, `proc3s` |
| JEOL | processed spectrum with `extra.indirect_dimensions[]` | `indirect_dimensions[0].axis`; with twice as many sweeps as F1 points the even (real) rows |
| JCAMP-DX | `nD NMR SPECTRUM` NTUPLES, pages keyed by F1 | F1 page values in Hz, converted with the observe frequency × Ξ(F1)/Ξ(F2) (IUPAC 2001 frequency ratios, `FREQUENCY_RATIOS`; a note says so); nuclei from `##.NUCLEUS=` |

Rows are read one at a time with a column window, and only the rows and columns inside
`ranges_ppm` (plus 8 points of margin) are read. The matrix is one `Vec<f64>`, the only full
copy: picking keeps the maxima and the footprint points (a hash map of the points above the
footprint floor that are connected to a peak). A crop larger than `max_points` (default 2²⁸,
2 GiB) is refused with exit 6 and a hint to narrow the ranges. The carrier of each indirect
dimension is at point n/2 of its full axis.

### Method (`pick_cross_peaks`, `annotate`, `integrate_regions`)

- **Noise SD** (`matrix_noise_sd`): runs of 64 points along the direct dimension (at most
  16384, spread evenly), a least-squares line removed from each, 1.4826 · MAD of the residual,
  25th percentile over runs (as in 1D) × 1.12 (`NOISE_P25_SCALE`, which makes it unbiased on
  Gaussian noise); exactly constant runs are skipped. Data with no negative values are treated
  as magnitude mode: the Rayleigh noise floor spreads 0.66 times as much as the noise on a line,
  so the SD is multiplied by 1.52 (`MAGNITUDE_NOISE_SCALE`, a note says so). **Noise from the
  maxima**: the 90th percentile of the heights of all local maxima of |value| (nearly all of
  them noise), divided by its value for Gaussian noise (`MAXIMA_P90_RATIO` 2.5 in 2D, +0.4
  per extra dimension, `MAXIMA_P90_MAGNITUDE` 2.95), gives `maxima_noise_sd`. Spectra with
  non-uniform sampling, t1 noise or truncation have noise with heavier tails than Gaussian
  (up to 3.9 times the run estimate in the corpus); `noise_sd`, which the thresholds and `snr`
  use, is the larger of `run_noise_sd` and `maxima_noise_sd`. **Line noise**:
  1.4826 · MAD of every line along each dimension (each column and row of a 2D spectrum). The
  local noise of a point is the largest of the noise SD and the noise of its lines.
- **Maxima**: at least as high as all 3ᴺ − 1 neighbours, off the edge, height ≥ `min_snr` ×
  noise SD and ≥ `min_height_fraction` × the largest |value| in range, prominence along every
  dimension ≥ `min_prominence_snr` × noise SD (the 1D rule along each line). Negative maxima the
  same way on the negated data (`include_negative`). Maxima below `min_local_snr` × their local
  noise (prominence scaled the same way) are on a noise ridge (t1 noise, a solvent column):
  counted in `ridge_suppressed` and left out, or kept and flagged `ridge` with
  `keep_ridge_peaks`. Kept maxima on a noisy line (≥ `ridge_factor` × the noise SD) that stand
  less than 2 × `min_snr` above its noise are flagged `ridge`.
- **Position and width**: per dimension, a parabola through the maximum and its two
  neighbours; full width at half height per dimension by walking along that dimension (`None`
  when an overlapping line keeps the signal from reaching half height).
- **Cross peaks**: maxima of either sign within `group_gap_hz` (or `group_gap_points`) of each
  other in every dimension are one cross peak (single linkage). `ppm` is the |height|-weighted
  mean of the members, `apex_ppm` the tallest member's position, `maxima` the member count,
  `sign` positive, negative or mixed (antiphase). `list_maxima` also returns every member
  (`maxima[]`, each with the index of its cross peak).
- **Footprint and volume**: flooding from the members, highest point first, through face
  neighbours of the same sign at ≥ `footprint_snr` × local noise; where two floods meet the
  first to arrive keeps the point. `volume` = Σ values × Π |step_ppm| (intensity·ppmᴺ),
  `absolute_volume` the same with |values| (antiphase cross peaks have volume near 0),
  `footprint_points`, `footprint_ppm` (extent per dimension). Cross peaks with fewer than
  `min_footprint_points` footprint points are isolated spikes (reconstruction artefacts of
  non-uniformly sampled data): left out and counted in `small_footprint_suppressed`.
- **Annotations**: when two dimensions observe the same nucleus over overlapping ranges
  (`homonuclear_pair`), cross peaks within `diagonal_tolerance_hz` (at least 1.5 points) of the
  diagonal are `diagonal`; each other cross peak is paired with the nearest cross peak at its
  mirror image within `symmetry_tolerance_hz` (at least 2 points): `symmetric_partner` (index
  in `peaks`) and `partner_height_ratio`, else `partner_missing`. `axial`: within one point of
  an indirect dimension's carrier. `ridge` also marks a cross peak on the indirect-dimension
  line (same direct position within the stronger peak's half width) of a cross peak
  `ridge_ratio` times taller: t1 noise and truncation wiggles; on homonuclear spectra a cross
  peak with a symmetric partner is not marked. Flags never remove a peak.
- **Integrals** (`integrate_regions`): Σ values in a ppm box per dimension × the cell size;
  `normalized` rescales to the reference box (default box 0 = 1), as the 1D integrals.

Defaults (`CrossPeakOptions`):

| option | default | meaning |
| --- | --- | --- |
| `min_snr` | 10 | height in noise SDs |
| `min_local_snr` | 6 | height in local-noise units (row and column noise) |
| `min_height_fraction` | 0 | fraction of the largest value |
| `min_prominence_snr` | 5 | prominence along each dimension in noise SDs |
| `include_negative` | true | negative peaks too |
| `ranges_ppm` | whole spectrum | one optional ppm range per dimension |
| `min_footprint_points` | 3 | smallest footprint of a cross peak |
| `max_peaks` | 1000 | tallest cross peaks kept |
| `group_gap_hz`, `group_gap_points` | 15 Hz, 1.5 points | maxima closer than this in every dimension form one cross peak |
| `footprint_snr` | 3 | footprint floor in local-noise units |
| `ridge_factor` | 3 | a line with noise ≥ 3 × the noise SD is a ridge |
| `ridge_ratio` | 20 | a cross peak on the F1 line of one 20 × taller is flagged `ridge` |
| `keep_ridge_peaks` | false | list maxima on ridges (flagged) |
| `diagonal_tolerance_hz`, `symmetry_tolerance_hz` | 15 Hz, 25 Hz | homonuclear annotations |
| `list_maxima` | false | return the member maxima too |

### Validation (2026-11-14, `crates/openreadout-corpus-tests/tests/nmr_2d_peaks.rs`)

Every processed Bruker 2D spectrum of the development corpus (74 `pdata/<n>/2rr`;
the 12 DOSY spectra are read and picked but not scored), split in two halves
by a hash of the id: the defaults were chosen on the `tune` half. None of these data sets
stores a TopSpin 2D peak list, so the picks are compared with nmrglue and with chemistry:

- **nmrglue** (`analysis.peakpick.pick`, run as a black box at the same absolute threshold,
  `tests/corpus/nmr2d_oracle/`): *maxima precision*, our maxima that are nmrglue maxima
  (within 1.5 points); *maxima recall*, nmrglue's 300 tallest maxima that we report (we leave
  out maxima without prominence and maxima on t1 ridges on purpose, so this is below 1 where
  ridges are strong); *regions*, nmrglue's `connected` regions that hold one of our maxima or
  footprints. The largest distance between a matched pair is 0.5 point (our interpolated
  position against nmrglue's grid point) on most spectra and 1.44 points at most.
- **Symmetry** (COSY, TOCSY, NOESY, ROESY not symmetrized in processing): off-diagonal cross
  peaks with a symmetric partner.
- **HSQC/HMQC against 1D lists** (5 data sets whose 1H and 13C spectra carry TopSpin peak
  lists): of the 10 tallest cross peaks, those on a 1H line and a 13C line after removing the
  referencing offset between the experiments (the median distance of the 12 tallest to their
  nearest 13C line).

| experiment type | set | spectra | cross peaks | nmrglue maxima recall | maxima precision | nmrglue regions | symmetric partners | tallest HSQC peaks on 1D lines |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| COSY | test | 6 | 1169 | 898/1800 = 0.50 | 829/829 = 1.00 | 520/1677 = 0.31 | 391/486 = 0.80 | – |
| COSY | tune | 7 | 3010 | 1470/1910 = 0.77 | 1347/1347 = 1.00 | 1031/1746 = 0.59 | 250/328 = 0.76 | – |
| J-resolved | test | 1 | 25 | 122/300 = 0.41 | 111/111 = 1.00 | 237/300 = 0.79 | – | – |
| HMBC | test | 5 | 406 | 708/740 = 0.96 | 642/642 = 1.00 | 704/709 = 0.99 | – | – |
| HMBC | tune | 10 | 1756 | 1214/1455 = 0.83 | 1089/1089 = 1.00 | 1056/1138 = 0.93 | – | – |
| NOESY | test | 3 | 1295 | 640/900 = 0.71 | 592/592 = 1.00 | 569/900 = 0.63 | 624/656 = 0.95 | – |
| NOESY | tune | 4 | 905 | 565/1200 = 0.47 | 532/532 = 1.00 | 448/836 = 0.54 | 353/371 = 0.95 | – |
| HSQC, HMQC | test | 11 | 803 | 793/1350 = 0.59 | 727/727 = 1.00 | 817/906 = 0.90 | – | 20/20 = 1.00 |
| HSQC, HMQC | tune | 5 | 365 | 338/574 = 0.59 | 315/315 = 1.00 | 293/494 = 0.59 | – | 26/30 = 0.87 |
| other (solid state, no pulse program) | test | 2 | 41 | 53/94 = 0.56 | 49/49 = 1.00 | 43/45 = 0.96 | – | – |
| other (solid state, no pulse program) | tune | 2 | 62 | 86/282 = 0.30 | 85/85 = 1.00 | 49/127 = 0.39 | – | – |
| ROESY | tune | 4 | 1037 | 650/1200 = 0.54 | 603/603 = 1.00 | 499/988 = 0.51 | 440/461 = 0.95 | – |
| TOCSY | test | 1 | 161 | 71/300 = 0.24 | 63/63 = 1.00 | 113/300 = 0.38 | 70/76 = 0.92 | – |
| TOCSY | tune | 1 | 161 | 71/300 = 0.24 | 63/63 = 1.00 | 113/300 = 0.38 | 70/76 = 0.92 | – |

Further checks: the JCAMP-DX `nD NMR SPECTRUM` export of the `nmrxiv-s200` limonene HSQC
gives a cross peak for each of limonene's 8 protonated carbons, and 10 of its 11 cross peaks
with S/N ≥ 100 lie on a line of the same sample's 1H and 13C spectra (both JCAMP-DX, picked
with the 1D picker). On non-uniformly sampled HSQCs the noise level from the maxima is 2 to
3.9 times the run estimate; without it `nmrxiv-s853-5` gave 2972 cross peaks scattered over
empty regions, with it 46. Synthetic spectra (unit tests) cover Lorentzian and Gaussian lines,
negative and antiphase peaks, overlapping multiplets, t1 ridges, single-point spikes,
magnitude-mode noise, diagonal symmetry, axial peaks, volumes and 3D.

**Performance** (corpus profile, Apple M-series): 0.02–2.1 s per spectrum (1024 × 1024: about
0.1–0.4 s; 2048 × 2048: 0.7–2.1 s; 1024 × 32768 DOSY: about 5 s), the matrix plus footprints in
memory.

**Known gaps (2D):** J-resolved and DOSY spectra are picked as any other 2D spectrum (F1 is
not a chemical shift: J-resolved F1 is in Hz around 0, the DOSY F1 axis is a diffusion axis
the reader labels in ppm). Spectra processed with symmetrization (TopSpin `syma`) are trivially
symmetric. The streak and ridge flags mark likely artefacts but can mark a real weak cross peak
on the line of a strong one. Cross peaks closer than `group_gap_hz` in every dimension are
merged, so distinct correlations of nearly degenerate protons can come out as one cross peak
with several maxima.

### Vocabulary of the 2D code (`nmr/peaks2d.rs`, `nmr/source2d.rs`, `nmr/mod.rs`)

| identifier | meaning |
| --- | --- |
| `CrossPeakOptions`, `min_snr`, `min_local_snr`, `ridge_ratio`, `min_footprint_points`, `min_height_fraction`, `min_prominence_snr`, `include_negative`, `ranges_ppm`, `max_peaks`, `group_gap_hz`, `group_gap_points`, `footprint_snr`, `ridge_factor`, `keep_ridge_peaks`, `diagonal_tolerance_hz`, `symmetry_tolerance_hz`, `list_maxima` | 2D picking options |
| `CrossPeak`, `ppm`, `hz`, `apex_ppm`, `index`, `height`, `sign`, `snr`, `local_snr`, `width_hz`, `width_ppm`, `maxima`, `volume`, `absolute_volume`, `footprint_points`, `footprint_ppm`, `flags`, `symmetric_partner`, `partner_height_ratio`, `has` | one cross peak |
| `PeakSign` { `Positive`, `Negative`, `Mixed` } | sign of a cross peak's members |
| `PeakFlag` { `Diagonal`, `PartnerMissing`, `Ridge`, `Axial` } | annotations |
| `Maximum`, `peak` | one member maximum |
| `CrossPeakPick`, `maxima_count`, `ridge_suppressed`, `small_footprint_suppressed`, `run_noise_sd`, `maxima_noise_sd`, `MAXIMA_P90_RATIO`, `MAXIMA_P90_PER_DIMENSION`, `MAXIMA_P90_MAGNITUDE`, `pick_cross_peaks`, `matrix_noise_sd`, `NOISE_P25_SCALE`, `MAGNITUDE_NOISE_SCALE`, `MAX_FOOTPRINT_POINTS`, `MAX_DIMENSIONS` | picking result, entry point and limits |
| `SpectrumContext`, `nuclei`, `carrier_points`, `homonuclear_pair`, `annotate` | annotations |
| `VolumeIntegral`, `integrate_regions` | box integrals (`ranges_ppm`, `value`, `normalized`, `points`) |
| `MatrixLayout`, `sweep_stride`, `matrix_layout`, `choose_matrix_trace`, `MatrixRequest`, `margin_points`, `max_points`, `DEFAULT_MAX_POINTS`, `StoredMatrix`, `data`, `full_shape`, `offsets`, `load_matrix` | reading a stored 2D/3D spectrum |
| `FREQUENCY_RATIOS`, `frequency_ratio`, `normalize_nucleus` | nuclei and their frequency ratios Ξ |
| `Nmr2dRequest`, `Nmr2dReport`, `homonuclear_dimensions`, `analyze_2d` | the 2D analysis entry point and its report (`path`, `format`, `source`, `trace`, `trace_name`, `nuclei`, `shape`, `axes`, `noise_sd`, `peak_options`, `peak_count`, `main_peak`, `peaks`, `integrals`, `notes`) |
