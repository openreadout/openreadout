# Provenance log — PerkinElmer Spotlight `.fsm` images

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- specio 0.1.0 (paris-saclay-cds), BSD-3-Clause, https://github.com/paris-saclay-cds/specio
  (`specio/plugins/fsm.py`, the copy installed in `oracle/.venv`): the block list after `PEPE` and
  the 40-byte description (u16 block id, i32 size), block 5100 (name, ten float64 values: x, y
  and z steps, z first and last, two 4-D limits, x, y and z origins; three int32 sizes), block
  5104 (instrument and history texts, read by position), block 5105 (one spectrum of float32
  values per block). specio reads the four texts after the sizes as single bytes; the reader
  here reads them as u16-length texts (below).
- The `.sp` reader's own notes (`docs/formats/perkinelmer-sp.md`): the member encoding (`#u`
  text, `,u`/`$u` flags, …) and the member ids of the history (121) and instrument (123)
  records, which block 5104 uses unchanged.
- specio is also run as the oracle (`oracle/spectro.py`, `fsm()`), a black box.

**Corpus files used:** `specio-spectra-fsm` (specio test data, BSD-3-Clause: 93 × 86 pixels,
1641 points, Spotlight/Spectrum One, 2007), `orange-4x4-pixel-pe-image-fsm`
(Orange-Spectroscopy test data, GPL-3.0-or-later: 4 × 4 pixels, 1626 points, Spotlight
400 / Spectrum 3, 2024).

**What was inferred from the files:**
- Block 5100's body: a u16-length name (empty in the older file, `Image` in the newer), the ten
  float64 values, three int32 sizes (x, y, points), then four u16-length texts: the y and x axis
  labels (`Y - Microns`, `X - micrometers`), the spectral unit (`cm-1`) and the value unit (`%T`).
  The spectral axis runs from z first to z last in steps of z step (4000 → 720 in −2 steps: 1641
  points).
- Block 5104's body is a block tree like an `.sp` data set: history record 121 (user, operation,
  date `Mon Oct 28 16:08:45 2024`, …) and instrument record 123 (model 35837, serial 35838,
  firmware 35839, scans 35840, detector 35841, source 35842, beamsplitter 35843, apodization
  35845, spectrum type 35846, beam type 35847, phase correction 35849, accessory 35854).
- The 5105 blocks run along x first: the 93 × 86 image read that way is smooth at a band
  (mean neighbour difference 0.36 standard deviations), read along y first it is not (0.66).
  Pixel `(i, j)` sits at stage position `(x origin + i·x step, y origin + j·y step)` µm.
