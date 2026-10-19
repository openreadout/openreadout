# Provenance log — 3Brain BrainWave raw data (`.brw`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted:** Neo 0.14.5 `neo/rawio/biocamrawio.py` (BSD-3-Clause,
https://github.com/NeuralEnsemble/python-neo, the copy installed in `oracle/.venv`): the BRW 3.x
recording variables, the `3BData` versions 100 (frames × channels) and 101/102 (interleaved),
signal inversion, the gain (max − min) / 2^bit depth and offset min; the BRW 4.x experiment
settings JSON (value and time converters), wells with `StoredChIdxs`, `Raw` and the event-based
sparse layout. Neo is also run as the oracle (`oracle/gen.py`, `brw()`), as a black box.

**Corpus files used:** `gin-biocam-hw3-0-fw1-6`, `gin-biocam-hw3-0-fw1-7-0-12-raw`,
`gin-biocam-version-100`, `gin-biocam-version-101`, `gin-biocam-brainwave5-hw3-0-fw1-7` (sparse,
refused).

**Inferred from the files:** a `3BData` group without a version attribute (`gin-biocam-hw3-0-fw1-6`)
holds an interleaved vector. BRW 4.x channels are named by their stored index, not placed on the
chip.
