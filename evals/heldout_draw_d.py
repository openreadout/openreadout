"""Held-out draw D (2026-10-06): questions about the fourth generalization draw.

The fourth draw (docs/benchmark/heldout-2026-10-06d.md) adds held-out inputs from source records
no development or earlier held-out file uses. Its questions were written from the held-out oracles
(`corpus/oracle/heldout/<id>.json`), the depositors' exports and records, and facts computed with
third-party readers, and committed before OpenReadout was run on any of these files.

The draw is split by area, one module each:
  - heldout_draw_d_ms.py: mass spectrometry and chromatography;
  - heldout_draw_d_imaging.py: light microscopy, electron microscopy, screening plates, gel imaging;
  - heldout_draw_d_signals.py: electrophysiology, NMR, optical spectroscopy, flow cytometry;
  - heldout_draw_d_assays.py: plate readers, qPCR and the bench instruments.

`evals/heldout.py` imports this module and extends its `C`, `FACTS` and `SPECS`. Each area module
has the same three members; the readers are imported inside the functions that use them, so
generate.py runs without them.
"""

from __future__ import annotations

from typing import Any

import heldout_draw_d_assays
import heldout_draw_d_imaging
import heldout_draw_d_ms
import heldout_draw_d_signals

AREAS = (heldout_draw_d_ms, heldout_draw_d_imaging, heldout_draw_d_signals, heldout_draw_d_assays)

C: dict[str, str] = {}
for _area in AREAS:
    if set(C) & set(_area.C):
        raise SystemExit(f"{_area.__name__}: key used twice: {sorted(set(C) & set(_area.C))}")
    C.update(_area.C)


def facts(Fact, H) -> list:
    return [f for area in AREAS for f in area.facts(Fact, H)]


def specs(spec, fact, g, H) -> list[Any]:
    return [s for area in AREAS for s in area.specs(spec, fact, g, H)]
