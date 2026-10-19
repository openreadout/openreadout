# Provenance log — MGF (Mascot generic format)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — initial reader (Richard Zimring with Claude as assistant)

**Specification consulted** (public): Matrix Science, "Data file format" help page, <https://www.matrixscience.com/help/data_file_help.html> (read 2026-10-19). Used for: the file layout (file parameters, then `BEGIN IONS` / `END IONS` blocks), comment characters (`#`, `;`, `!`, `/`, outside blocks only), `KEY=value` syntax with case-insensitive keys, the block parameters `PEPMASS` (`mz [intensity] [charge]`, repeated for chimeric spectra), `CHARGE` (`2+`, `2+ and 3+`), `TITLE`, `RTINSECONDS` (seconds, `a[-b]`), `SCANS` (`v[-w]`, lists), `RAWFILE`, `ION_MOBILITY`, that file parameters apply to every query (a file-level `CHARGE` is the default charge), peak lines `mz intensity [charge]` with further text ignored, and that Mascot accepts centroided peak lists only.

**Prior art** (permissively licensed, run as the black-box oracle; its source was not needed): pyteomics 5.0.1, Apache-2.0, <https://github.com/levitsky/pyteomics>, `pyteomics.mgf.MGF` with `use_header=True` (`oracle/gen.py` `mgf_`). The sequential reader is used because the indexed one keys spectra by `TITLE` and drops repeated titles (`mgf-pyteomics-pepmass`).

**Corpus files** (all GitHub test data at pinned commits): `mgf-pyteomics-test`, `mgf-pyteomics-example`, `mgf-pyteomics-pepmass` (levitsky/pyteomics, Apache-2.0); `mgf-matchms-pesticides`, `mgf-matchms-testdata` (matchms/matchms, Apache-2.0); `mgf-openms-gnpsexport1`, `mgf-openms-idconv33`, `mgf-openms-mgf-gnps` (OpenMS/OpenMS, BSD-3-Clause); `mgf-ms2pip-massivekb500` (compomics/ms2pip, Apache-2.0); `mgf-casanovo-sample` (Noble-Lab/casanovo, Apache-2.0); `mgf-msdeisotope-small` (mobiusklein/ms_deisotope, Apache-2.0); `mgf-ms2rescore-test` (compomics/ms2rescore, Apache-2.0).

**Inferred from the corpus:**
- `MSLEVEL`, `IONMODE` / `ION_MODE`, `SPECTRUMID`, `FEATURE_ID`, `SEQ` are writer conventions, not in the Matrix Science description: `MSLEVEL` in the GNPS library and OpenMS GNPSExport files, `IONMODE` in `mgf-matchms-*`, `ION_MODE` in `mgf-openms-mgf-gnps`, `FEATURE_ID` in `mgf-openms-gnpsexport1`, `SEQ` in `mgf-casanovo-sample`. They give the MS level, the polarity and the dialect.
- `SCANS` is not always a number (`F1:2478` in `mgf-casanovo-sample`); then the spectrum is numbered by position, as the oracle does.
- Text after the peak values (`# b1` in the ms2pip test files) is ignored, as the description says; pyteomics refuses such lines, so those files are not in the corpus.
- The writer names: `COM=OpenMS_search` and `USERNAME=OpenMS` (`mgf-openms-idconv33`), GNPS library records (`mgf-openms-mgf-gnps`, `mgf-matchms-*`).
- A block that gives a charge both on its `PEPMASS` line and in `CHARGE` (`mgf-pyteomics-pepmass`, third block: `PEPMASS=983.6 1234 2+`, `CHARGE=3+`): the `PEPMASS` charge belongs to that precursor and is taken, as pyteomics does.
