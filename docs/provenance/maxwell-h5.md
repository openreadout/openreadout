# Provenance log — MaxWell MaxOne / MaxTwo recordings (`.raw.h5`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted:** Neo 0.14.5 `neo/rawio/maxwellrawio.py` (BSD-3-Clause,
https://github.com/NeuralEnsemble/python-neo): `/version`, layout 20160704 (`/sig`, `/mapping`,
`/settings/lsb` or `gain`, 20 kHz), later layouts (`/wells`, `groups/routed/raw`,
`settings/mapping`, `settings/sampling`), lsb × 10⁶ as µV, and filter 401 needing MaxWell's
plugin. Neo is also run as a black box in the oracle (`oracle/gen.py`, `maxwell()`).

**Corpus files used:** `zenodo-6578989-drug-2950-50um` (layout 20160704); the six
`gin-maxwell-*` files (layout 20190530, compressed with filter 401, refused).

**Inferred from the files:** in layout 20160704 the mapping's channel numbers are rows of the
1028-row `/sig` matrix; in later layouts `groups/routed/channels` gives each row's channel. The
start times (`/time`, `start_time`) are read as local time and Unix ms.
