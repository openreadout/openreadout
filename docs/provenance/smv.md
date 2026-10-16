# Provenance log — SMV diffraction images (`smv`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-15 — reader (Richard Zimring with Claude as assistant)

**Prior art read (permissive):** dxtbx (BSD-3, https://github.com/cctbx/dxtbx, `FormatSMV.py`, `FormatSMVADSC.py`, `FormatSMVADSCCetaD.py`, `FormatSMVTimePix_SU.py`) and FabIO 2026.6.0 (MIT, `adscimage.py`) as documentation of the SMV layout that ADSC introduced: a text header `{ KEY=value; … }` padded to `HEADER_BYTES` (512 or more), then `SIZE1` × `SIZE2` samples of `TYPE` (`unsigned_short`, ...) in `BYTE_ORDER`. From dxtbx: on ADSC-style files `BEAM_CENTER_X`/`BEAM_CENTER_Y` are millimetres, with X along the slow axis and Y along the fast axis (dxtbx passes `(beam_y, beam_x)` as the fast/slow beam centre); on files from the TimePix detector at Stockholm University (`BEAMLINE=TimePix_SU`, written by Instamatic) they are pixels along the fast and slow axis.

**Corpus files used:** Zenodo 1321880 "Continuous rotation electron diffraction data for zeolite Mordenite" (CC-BY-4.0, zip members `mordenite_cRED_2/SMV/data/*.img`: 516 × 516 uint16, TimePix_SU, 0.0251 Å); Zenodo 4737864 "Biotin Electron Diffraction (MicroED) Datasets — Glacios TEM with a CETA-D" (CC-BY-4.0, zip member `801406_1/g20feb26f_801406_1_gl71_007.img`: 2048 × 2048 uint16, `BEAMLINE=CETAD_TUI`, `IMAGE_PEDESTAL`).

**Observed:** the header keys and values above, the header length in `HEADER_BYTES`, the file length = header + SIZE1 × SIZE2 × 2. On the TimePix file `DENZO_X_BEAM` = `BEAM_CENTER_Y` × `PIXEL_SIZE` and `DENZO_Y_BEAM` = `BEAM_CENTER_X` × `PIXEL_SIZE`: the Denzo pair is in millimetres and swaps the axes, as the ADSC pair does.

**Inferred:** a header value is reported as stored (`extra.smv`); only the keys above are mapped to the shared geometry. The beam-centre convention is chosen by `BEAMLINE` as dxtbx does; other SMV dialects (Rigaku `SPATIAL_DISTORTION_INFO`) are listed raw without a beam centre.
