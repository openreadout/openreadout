"""Signal analyses through the Python bindings, against the committed third-party ground truth
(``corpus/oracle/ephys-analysis`` from eFEL / SpikeInterface, ``corpus/oracle/nmr-processing``
from TopSpin's stored integrals)."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Callable

import pytest

import openreadout

REPO = Path(__file__).resolve().parents[3]
Corpus = Callable[[str], Path]


def _oracle(name: str) -> dict:
    p = REPO / "corpus" / "oracle" / name
    if not p.is_file():
        pytest.skip(f"no oracle {name}")
    return json.loads(p.read_text())


def test_nmr_peaks_integrals_match_topspin(corpus: Corpus) -> None:
    path = corpus("nmrxiv-s846/50")
    o = _oracle("nmr-processing/nmrxiv-s846-50.json")
    regions = [(r["from_ppm"], r["to_ppm"]) for r in o["topspin_integrals"]]
    ref = o["topspin_integrals"][0]["value"]
    r = openreadout.analyze(
        path, "nmr-peaks", from_="fid", integrate=regions, integral_reference=(0, ref)
    )
    assert r["source"] == "fid"
    for ours, theirs in zip(r["integrals"], o["topspin_integrals"]):
        assert ours["normalized"] == pytest.approx(theirs["value"], rel=0.02)
    assert r["main_peak"]["ppm"] == pytest.approx(1.5731, abs=0.01)


def test_ephys_features_match_efel(corpus: Corpus) -> None:
    path = corpus("pyabf-171116sh-0018.abf")
    o = _oracle("ephys-analysis/pyabf-171116sh-0018.json")
    r = openreadout.analyze(path, "ephys-features")
    assert r["clamp_mode"] == "current_clamp"
    assert r["cell"]["rheobase_pa"] == o["cell"]["rheobase_pa"]
    assert [s["spike_count"] for s in r["sweeps"]] == [int(s["spike_count"][0]) for s in o["sweeps"]]
    assert r["cell"]["input_resistance_mohm"] == pytest.approx(o["cell"]["input_resistance_mohm"], rel=0.01)


def test_spikes_match_spikeinterface(corpus: Corpus) -> None:
    path = corpus("brk-filespec2-3001.ns5")
    o = _oracle("ephys-analysis/brk-filespec2-3001.spikes.json")
    r = openreadout.analyze(path, "spikes", channels=[0], sweeps=[0])
    assert r["channels"][0]["spike_count"] == o["channels"][0]["count"]


def test_bad_options_raise(corpus: Corpus) -> None:
    path = corpus("nmrxiv-s846/50")
    with pytest.raises(ValueError):
        openreadout.analyze(path, "nmr-peaks", phase="sideways")
    with pytest.raises(openreadout.UsageError):
        openreadout.analyze(path, "nmr-peaks", no_such_option=1)
    with pytest.raises(openreadout.UsageError):
        openreadout.analyze(path, "nmr_peaks")
