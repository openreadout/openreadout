"""``openreadout.analyze(..., "assay")`` (plate analysis) on the committed synthetic plates, checked against the
SciPy ground truth in ``corpus/oracle/assay`` (written by ``oracle/assay.py``, no OpenReadout)."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

import openreadout
import pytest
from openreadout import UsageError

REPO = Path(__file__).resolve().parents[3]
FIXTURES = REPO / "crates" / "openreadout-assay" / "tests" / "fixtures"


def _expected(case: str, path: str, source_prefix: str = "scipy") -> float:
    doc: Dict[str, Any] = json.loads((REPO / "corpus" / "oracle" / "assay" / f"{case}.json").read_text(encoding="utf-8"))
    for c in doc["checks"]:
        if c["path"] == path and c["source"].startswith(source_prefix):
            return float(c["expect"])
    raise KeyError(path)


def test_dose_response_ic50_matches_scipy() -> None:
    r = openreadout.analyze(
        FIXTURES / "assay-synth-dose-response.csv",
        "assay",
        analysis="dose-response",
        layout=FIXTURES / "assay-synth-dose-response-layout.csv",
        plot=True,
    )
    by = {c["compound"]: c for c in r["compounds"]}
    for name in ("CPD-A", "CPD-B", "CPD-C"):
        want = _expected("synth-dose-response", f"compounds[compound={name}].ec50")
        assert by[name]["kind"] == "IC50"
        assert by[name]["ec50"] == pytest.approx(want, rel=1e-5)
    assert r["quality"]["z_prime"] == pytest.approx(_expected("synth-dose-response", "quality.z_prime", "numpy"), rel=1e-9)
    assert isinstance(r["plot_png"], bytes) and r["plot_png"].startswith(b"\x89PNG")


def test_standard_curve_with_well_flags() -> None:
    r = openreadout.analyze(
        FIXTURES / "assay-synth-elisa-5pl.csv",
        "assay",
        analysis="curve",
        layout=FIXTURES / "assay-synth-elisa-5pl-layout.csv",
        model="5pl",
        weighting="1/y2",
    )
    c = {p["name"]: p["value"] for p in r["curve"]["fit"]["parameters"]}
    assert c["c"] == pytest.approx(_expected("synth-elisa-5pl", "curve.fit.parameters[name=c].value"), rel=1e-4)
    a1 = next(w for w in r["wells"] if w["well"] == "A3")
    assert a1["back_calculated"] == pytest.approx(_expected("synth-elisa-5pl", "wells[well=A3].back_calculated"), rel=2e-4)


def test_errors_are_usage_errors() -> None:
    with pytest.raises(UsageError):
        openreadout.analyze(FIXTURES / "assay-synth-dose-response.csv", "assay", analysis="curve")
    with pytest.raises(ValueError):
        openreadout.analyze(
            FIXTURES / "assay-synth-dose-response.csv", "assay", analysis="curve", no_such_option=1
        )
