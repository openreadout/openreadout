# Provenance log — Bruker ParaVision (`bruker-paravision`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-15 — initial derivation (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed, read as documentation; no code copied):
- brukerapi 0.4.6 (MIT, Pilsen MR group, https://github.com/isi-nmr/brukerapi-python): `brukerapi/jcampdx.py` (parameter-file syntax: `##$` records, size brackets followed by values, `<strings>` with backslash escapes, `(...)` structures, `@n*(value)` runs, `$$` comment lines, the empty-string form `<\` newline `>`), `brukerapi/config/properties_2dseq_core.json` and `brukerapi/schemas.py` (`VisuCoreWordType` and `VisuCoreByteOrder` to sample types; frame shape `VisuCoreSize`; frame groups from `VisuFGOrderDesc` with the first group varying fastest (Fortran order); scaling `stored × VisuCoreDataSlope + VisuCoreDataOffs` per frame; `VisuCoreTransposition` exchanges two core dimensions of a frame). brukerapi is also the oracle.
- bruker2nifti (MIT) was not needed.

The ParaVision manuals were not consulted.

**Corpus files used:** the BrukerAPI test data sets on Zenodo (CC-BY-4.0; records 4522220, 4048253, 4048286): ParaVision 7.0.0, 6.0.1 and 5.1 phantom studies. Expnos 6 (B1 map, FG_ECHO × FG_SLICE, 32-bit), 10 (DTI EPI, FG_DIFFUSION), 20 (flow map, FG_FLOW) and 35 (single-pulse spectroscopy) of the PV 7.0.0 study were dumped while writing the reader.

**Decisions (ours):** FG_SLICE is Z; FG_CYCLE and FG_MOVIE are T; every other group is a channel named by `VisuFGElemComment`. Values are float32 when any frame has a slope other than 1 or an offset other than 0. Spectroscopy reconstructions are traces with a ppm axis centred on `PVM_FrqWorkPpm` (inferred from the parameter names and values, not validated by an oracle). Subject names, IDs, birth dates and referring users are withheld.
