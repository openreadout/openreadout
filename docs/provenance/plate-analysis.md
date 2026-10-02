# Provenance: plate analysis (`openreadout analyze assay`, crate `openreadout-assay`)

## 2026-09-23 — first version

**Scope:** layouts and roles, blank subtraction, replicate statistics and outlier flags, standard curves (linear, 4PL, 5PL) with back-calculation, dose-response (IC50/EC50), kinetics, growth curves and assay quality, on the long-form plate tables of the `plate` reader ([plate-analysis.md](../../book/src/guides/plate-analysis.md)). Not a parser: the plate values come from `openreadout-plate` (its provenance: [plate-readers.md](plate-readers.md)); the only parser change made for this work is the SkanIt `Layout definitions` sheet (logged there, same date).

**Methods, and where they come from.** All written from textbook or published definitions:

- Least squares by Levenberg–Marquardt with Marquardt's diagonal scaling (Marquardt 1963); parameter standard errors from s² (JᵀWJ)⁻¹ (the Gauss–Newton/Wald convention of R `nls`, SciPy `curve_fit` and GraphPad Prism's "asymptotic standard errors"); confidence limits value ± t(1 − α/2, n − p)·SE; EC50 limits on the log scale. Student-t quantiles from the regularized incomplete beta function (continued fraction, DLMF 8.17.22, modified Lentz) and the Lanczos ln Γ; checked against `scipy.stats.t.ppf` in unit tests.
- 4PL/5PL parameterisation y = d + (a − d)/(1 + (x/c)^b)[^g]: the form Gen5 and SkanIt print in their own exports (`Y = (A-D)/(1+(X/C)^B) + D`, `y = d + (a-d)/(1+(x/c)^b)`, read from the corpus files below).
- Z′ = 1 − 3(σp + σn)/|µp − µn| (Zhang, Chung & Oldenburg, J. Biomol. Screen. 4:67, 1999); SSMD (Zhang 2007); modified z-score |0.6745(x − median)/MAD| > 3.5 (Iglewicz & Hoaglin 1993); Grubbs' test with the t-based critical value (Grubbs 1969).
- Logistic growth N(t) = K/(1 + ((K − N0)/N0)e^(−rt)), t_mid, t_gen = ln 2/r: the model of the growthcurver paper (Sprouffske & Wagner, BMC Bioinformatics 17:172, 2016); µmax as the steepest log-linear window, the "easy linear" idea of Hall et al. (Evolution 68:2211, 2014), with our own window and threshold rules (documented, not theirs).
- Lag time as the intersection of the maximum-slope tangent with the baseline (Zwietering et al. 1990, AEM 56:1875), baselines as documented.
- LLOQ/ULOQ: the ±20 % recovery / CV ≤ 20 % acceptance of the FDA/EMA bioanalytical method validation guidance for ligand-binding assays (we use 20 % for every level).

**Oracles (run):** SciPy 1.18.1 `curve_fit` (BSD-3), R 4.4.3 `stats::nls`, R `drc` 3.0-1 and `growthcurver` 0.3.1 (GPL-2/3; installed user-locally with micromamba from conda-forge plus CRAN, run through `Rscript` by `oracle/assay.py`). Two conventions were found by running drc as a black box: its `weights` act as 1/σ (it minimises Σ(w·r)²; weights 1/|y| reproduce SciPy's `sigma = |y|`, i.e. our 1/y² weighting, to 3·10⁻⁶), and its standard errors are Hessian-based (they differ from the Gauss–Newton ones of SciPy, `nls` and ours by 3–9 % on these fits; the checks allow 10 % and say so). The 5PL has two asymmetric branches (sign of b); drc started without values lands on the other branch with a worse weighted RSS (0.0074 vs 0.0027), so it is started on SciPy's branch.

**Vendor-computed ground truth (in the files):**
- `skanit-elisa-steps` (allotropy fixture, MIT): SkanIt 7.0's Standard Curve step (4PL a, b, c, d; 63 back-calculated concentrations; 35 `< Min`/`> Max` flags) and Dilution Factor step (47 dilution-corrected concentrations). SkanIt fits the unweighted 4PL to the replicate standard wells after subtracting the mean of the `Blank1` wells: our zero-configuration run (`assay curve FILE`, layout from the file) reproduces its parameters to 3·10⁻⁴ and every concentration to 0.07 %.
- `gen5-abs-stdcurve-linear` (new, allotropy fixture `endpoint_stdcurve_singleplate.txt`, MIT): Gen5 3.12's linear StdCurve (A, B to 3 digits) and 96 `[Concentration]` values; Gen5 fitted unrounded ODs, the export rounds them to 0.001, so concentrations agree within 0.0006 OD / slope.
- `gen5-abs-kinetic-meanv-4pl` (new, allotropy fixture `Kinetic_Analysis_Mean_Slope_and_Standard_Curve_tab.txt`, MIT): Gen5's 4PL on its Mean V results (A–D and R² to 3 digits) and 96 `[Concentration]` values with `<0.000`/`>40.000` flags. Its Mean V values come from all 61 reads while the export lists 13, so `--reduce mean-slope` on the exported reads differs from Mean V by ≤ 1 % (reported, not asserted).

**Synthetic plates with known truth** (`oracle/make_assay_fixtures.py`, plain Python, deterministic): a 3-compound dose-response plate with controls, a 5PL ELISA with dilutions, 24 enzyme progress curves, 12 logistic growth curves with blanks (`crates/openreadout-assay/tests/fixtures/`; the first two also in the corpus for evals).

**Validation** (`oracle/assay.py` → `corpus/oracle/assay/*.json`, 10 cases, 995 checks, all passing; `cargo test -p openreadout-assay --test oracle -- --nocapture`), largest relative difference per source:

| case | checks | vs SciPy | vs R nls | vs R drc / growthcurver | vs the vendor |
| --- | --- | --- | --- | --- | --- |
| synthetic dose-response (raw, normalised) | 44 + 39 | 7·10⁻⁵ (EC50, top, bottom, Hill, R², SE, log-scale CI) | 1·10⁻⁶ (EC50, its SE) | EC50 3·10⁻⁵, Hill 8·10⁻⁶; ED50 SE 9 % (Hessian) | — |
| synthetic 5PL ELISA, 1/y² | 152 | 3·10⁻⁶ (parameters), 2·10⁻⁵ (SEs), 1·10⁻⁷ (80 back-calculations, 40 dilution-corrected) | — | 3·10⁻⁶ (parameters, fitted values, weighted RSS) | — |
| synthetic kinetics | 144 | NumPy re-implementation of the definitions: 6·10⁻¹⁶ | — | — | — |
| synthetic growth | 66 | 3·10⁻⁹ (logistic r, K); NumPy µmax 1·10⁻¹⁵ | — | growthcurver r, K, t_gen 5·10⁻⁹ | — |
| SkanIt ELISA (zero configuration) | 165 | 1·10⁻⁷ (parameters), 5·10⁻⁷ (SEs) | 1·10⁻⁶ | drc 2.3·10⁻³ (stops early in a flat valley) | parameters 3·10⁻⁴, 63 concentrations and 47 dilution-corrected 7·10⁻⁴, 35 flags exact |
| Gen5 linear (zero configuration) | 101 | NumPy polyfit 3·10⁻¹³ | — | — | slope/intercept within their 3 printed digits; 96 concentrations within the rounding bound |
| Gen5 4PL on Mean V (`--read "Mean V"`) | 130 | 6·10⁻⁸ (parameters), 1·10⁻⁷ (SEs) | 1·10⁻⁶ | drc 3·10⁻⁶ (parameters) | A–D 1·10⁻³ (3 digits), 73 concentrations ≤ 1.1 % near the asymptote (tolerance from the curve's slope), 23 flags exact |
| Tecan i-control growth (8 wells, 632 reads) | 56 | 2·10⁻⁸ | — | growthcurver (`bg_correct = "min"`) 4·10⁻⁶ | — |
| Gen5 screening plate, POSCON/NEGCON layout (`assay wells --normalize controls`, zero configuration) | 98 | — | — | — | 16 control roles exact; 96 `NormLum` percent-of-control values within 0.008 (Gen5 prints 3 decimals) |

**Inferred, not corroborated:** our µmax window and threshold rules, the lag baselines and the LLOQ rule are our documented choices; they were checked against re-implementations of the same definitions, not against another tool's (none defines them identically). Gen5's own Max V/lag conventions could not be checked: the only Gen5 export with Max V and Lagtime results (`gen5-kinetic-growth-curve`) lists 20 of its 999 reads.

## 2026-09-24 — layout role names, user role maps, percent of controls for `wells`

- **Corpus files:** `gen5-lum-endpoint` (allotropy fixture `endpoint_singleplate.txt`, MIT): a luminescence screening plate whose embedded layout (`Well ID`) names 8 `POSCON`, 8 `NEGCON` and 80 `SPL1`…`SPL80` wells, and whose `NormLum` read is Gen5's own normalisation, vendor-computed and stored under each well.
- **Prior art consulted:** none beyond the file. That `NormLum` = 100 (LUM − mean NEGCON) / (mean POSCON − mean NEGCON) was inferred from the file (recomputed from its LUM read and its layout to < 0.01 for all 96 wells, `evals/scenario_facts.py` and `oracle/assay.py`), which fixes POSCON as the full-signal (positive) and NEGCON as the no-signal (negative) control.
- **What changed:** `POSCON`/`NEGCON` (and `PosCtrl`/`NegCtrl`, `poscon1`…) are read as positive/negative controls instead of samples; the role synonyms gained `no-cell`, `cell-free`, `medium only` (blank), `maximum`, `minimum`; `vehicle`, `DMSO`, `solvent`, `untreated` are now controls of unspecified sign (they were negative): whether a vehicle well is the 0 % or the 100 % control depends on the assay (inhibition or activation), so the user says it with `--role NAME=ROLE` / `roles`. Role texts no role matches, unsigned controls and unmatched `--role` entries are reported under `warnings` (they used to be read as samples silently). `--normalize controls` also applies to `assay wells` (per-well `percent_effect`).
- **Validation:** `corpus/oracle/assay/gen5-lum-normalised.json` (98 checks: the control counts and every well's `percent_effect` against `NormLum`, tolerance 0.012), all passing with the export's own layout and no options; largest difference 0.008 (Gen5 computes from unrounded control means and prints 3 decimals).
- **Not corroborated:** SoftMax Pro text exports carry no plate layout (their `Group:` blocks are kept verbatim), so no SoftMax role names could be checked; SkanIt's `Blank1`/`Std0001`/`Un0001` names were already covered by the SkanIt case.

## 2026-09-24 — plate-map header overflow

**Scope:** a parsing guard; layouts that parsed before parse the same. The `assay_input` fuzz target (90 s) found a plate-map grid whose header row starts with a column number near 2^32: checking that the header counts `1 2 3 …` added the position to it without overflow checks. The check now fails (the row is not a column header) instead. Regression fixture: `crates/openreadout-assay/tests/fixtures/malformed/layout-fuzz-column-number-overflow.csv`, run by `crates/openreadout-assay/tests/fuzz_regressions.rs`.
