# Bruker EPR (BES3T `.DSC`/`.DTA`, ESP/WinEPR `.par`/`.spc`) provenance

## 2026-09-26 — before the readers were written (Richard Zimring with Claude as assistant)

**Permissive prior art consulted (read as documentation):**

- EasySpin (Stefan Stoll and contributors, MIT licence: `LICENSE.md` of
  github.com/StollLab/EasySpin, checked 2026-09-26 with the GitHub licence API), commit
  d46e5ef96d092d78c99e9a561d84308e1ca9331d: `easyspin/eprload.m` (dispatch by extension),
  `easyspin/private/eprload_BrukerBES3T.m`, `easyspin/private/eprload_BrukerESP.m`,
  `easyspin/private/getmatrix.m`. What we took from them:
  - BES3T: the `.DSC` descriptor is text of `KEY value` lines in layers `#DESC`, `#SPL`, `#DSL`
    (device blocks `.DVC name, version`) and `#MHL` (reading stops there); a line ending in `\`
    continues on the next and `\n` inside such a value is a newline; values may be quoted with
    `'`. `IKKF` lists one `REAL`/`CPLX` per data value of a point (comma-separated), `IRFMT`
    (`C` int8, `S` int16, `I` int32, `F` float32, `D` float64; `A` ASCII and `0`/`N` "no data"
    are refused) must list as many and `IIFMT` must equal it; `BSEQ` `BIG`/`LIT` byte order;
    `XPTS`/`YPTS`/`ZPTS` points per dimension; per axis `xTYP` `IDX` (linear:
    `xMIN + linspace(0, xWID, xPTS)`), `IGD` (the values are in a companion `.XGF`/`.YGF`/`.ZGF`
    file in the format `xFMT`, same byte order), `NTUP` (refused). The `.DTA` holds
    `values-per-point × XPTS × YPTS × ZPTS` numbers, x fastest; a complex value is stored as
    real then imaginary.
  - ESP/WinEPR: the `.par` file is `KEY value` lines; a `DOS` key (`DOS  Format`) marks WinEPR
    files (little-endian float32 `.spc`); otherwise ESP (big-endian int32). `JSS` flag bits: bit
    5 (value 16) complex, bit 13 (value 4096) two-dimensional. Points: `SSX`/`SSY` (2D), `ANZ`
    (total), `RES`/`REY`, `XPLS` (later keys override earlier ones in that order). The field
    axis: `GST` + `GSI` × linspace(0, 1) (ENDOR, or when all of `HCF`/`HSW`/`GST`/`GSI` are
    present), `HCF` ± `HSW`/2 (centre and sweep width), or `XXLB`/`XXWI` (`XYLB`/`XYWI` for the
    second axis of 2D EMX data); a `JEX` of `Time-Sweep` has time `i × RCT / 1000` s. Parameter
    meanings used for the experiment model: `MF` microwave frequency (GHz), `MP` power (mW), `RMA`
    modulation amplitude (G), `RRG` receiver gain, `RCT` conversion time (ms), `RTC` time constant
    (ms), `JSD` scans done, `TE` temperature (K), `JDA`/`JTM` date and time, `JCO` comment,
    `JUN` field unit, `JEX`/`JEY` experiment type.
- DeerLab (Jeschke lab, MIT licence, checked with the GitHub licence API), commit
  54798458c325071882cb8bf9c0a2dd017ae8bf70, `deerlab/deerload.py`: the same BES3T rules; it is run
  as an independent reader (black box and documentation) in the oracle.

**Corpus files used** (all in `corpus/manifest.toml` with their licences): BES3T —
`epr-easyspin-e580-cwx` (EasySpin test file, MIT), `epr-cwepr-bdpa-2dfieldpower` (cwepr, BSD-2,
with a `.YGF`), `epr-pyepri-fusillo` (pyepri, MIT, with a text export), `epr-killian-tot-accu` and
`epr-killian-tot-kinetics` (CC-BY-4.0, with Xepr's ASCII export; the second a 2D kinetics series
with a `.YGF`), `epr-zenodo21084153-tempol` and `epr-zenodo21287247-powersat` (pySpecData
documentation data, CC0, ten detection harmonics per point), `epr-zenodo15590546-deer` (Q-band DEER,
complex), `epr-zenodo10463869-asympol`, `epr-zenodo14034164-ref` and `-img` (L-band and a
reconstructed image written by other software, `DSRC MAN`), `epr-zenodo18969515-eseem` (pulse ESEEM,
`DSRC MAN`), `epr-deernet-deer-252cl` and `epr-deernet-white-1b` (DEERNet examples, MIT),
`epr-mars-ky3-96k` (MaRs, Apache-2.0), `epr-watersplitting-js463` (MIT); ESP/WinEPR —
`epr-zenodo45520-sige` (with WinEPR's `.asc` export), `epr-zenodo5925657-angle` (2D angular sweep
with the depositor's CSV), `epr-easyspin-emx-2dpowersweep` (EasySpin test file). One further
depositor is held out (`docs/benchmark/heldout.md`).

### Inferred from the files (scratch scripts, not committed)

- Every BES3T file follows the rules above; the `.DTA` size equals the described size in all 19.
- `DATE` is month/day/year in Xepr files (`11/18/21` with `TIME 16:22:52`); files written by other
  programs (`DSRC MAN`: EasySpin's export writes day/month/year, `07/04/22` for 7 April) are not
  used for the date.
- Two data sets store ten values per point (`IKKF REAL,…`): their signal channel enables the 1st
  to 5th harmonics in phase and at 90° (`Enable1stHarm`, `Enable1stHarm90`, …); we name the values
  in that order. This is an inference from the device parameters (no independent reader
  separates them; DeerLab's `deerload` refuses multi-value points).
- DeerLab's `deerload` (black box) reads negative `XMIN` values as positive (its key/value pattern
  consumes the minus sign); on those files its abscissa is not compared (its values are).
- WinEPR `.par` files end lines with CR only; `JDA` is `MM/DD/YYYY` or `13-Jun-2018`.
- WinEPR's `.asc` export prints the field axis from float32 values (differences up to 1e-4 G from
  `GST` + i·`GSI`/(n − 1) in float64); the intensities agree to its six decimals.
