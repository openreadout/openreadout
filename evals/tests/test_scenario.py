"""Scenario tier: end-to-end lab requests over a staged folder (evals/scenario_facts.py), the list
scorer's `reject`, and the OME-Zarr output check."""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import generate
import run
import scenario_facts
import score
import stats


def questions() -> dict[str, dict]:
    return {q["id"]: q for q in score.load_questions().values() if q["category"] == "scenario"}


def test_scenarios_are_well_formed_and_independent():
    qs = questions()
    assert 15 <= len(qs) <= 25
    assert {q["family"] for q in qs.values()} >= {
        "chromatography", "ms", "plates", "hcs", "flow", "ephys", "nmr", "qpcr", "microscopy", "share",
    }  # fmt: skip
    facts = json.loads(scenario_facts.FACTS.read_text())
    with (generate.ROOT / "corpus" / "manifest.toml").open("rb") as fh:
        manifest = tomllib.load(fh)["file"]
    heldout = {e["id"] for e in manifest if e.get("tier") == "heldout" or e.get("role") == "heldout"}
    for q in qs.values():
        assert q["id"].startswith("scn-"), q["id"]
        assert q["source"].startswith("evals/facts/scenario.json ["), q["id"]
        key = q["source"].split("[", 1)[1].split("]", 1)[0]
        assert key in facts and "openreadout" not in json.dumps(facts[key]["reader"]).lower(), q["id"]
        assert q["file"]["stage_as"] == scenario_facts.DIR
        staged = [r["stage_as"] for r in q["share"]] + [g["stage_as"] for g in q["generated"]]
        assert len(staged) == len(set(staged)), q["id"]
        assert all(s.startswith(scenario_facts.DIR + "/") for s in staged), q["id"]
        assert not {r["corpus_id"] for r in q["share"]} & heldout, q["id"]
        assert q["split"] == generate.split_of(q) and q["split"] in ("dev", "test"), q["id"]
        assert stats.tier_of(q) == "scenario"
    splits = {q["split"] for q in qs.values()}
    assert splits == {"dev", "test"}


def test_questions_follow_the_committed_facts():
    """The committed question file is what build_all makes from facts/scenario.json."""
    built = {q["id"]: q for q in scenario_facts.build_all()}
    for qid, q in questions().items():
        b = dict(built[qid])
        b["split"] = generate.split_of(b)
        assert b == q, qid


def test_scenario_answers_score():
    qs = questions()
    q = qs["scn-chrom-gcms-reaction-is-ratio"]
    assert score.score_answer(q["answer"], "ANSWER: 8 h")["correct"]
    assert not score.score_answer(q["answer"], "ANSWER: 5 h")["correct"]
    q = qs["scn-plate-screen-hits"]
    hits = ", ".join(q["answer"]["value"])
    assert score.score_answer(q["answer"], f"ANSWER: {hits}")["correct"]
    # listing every compound (or one non-hit too) is not an answer
    assert not score.score_answer(q["answer"], f"ANSWER: {hits}, {q['answer']['reject'][0]}")["correct"]
    assert not score.score_answer(q["answer"], f"ANSWER: {q['answer']['value'][0]}")["correct"]
    q = qs["scn-qpcr-rdml-hac1u-ddcq"]
    facts = json.loads(scenario_facts.FACTS.read_text())["qpcr_rdml"]
    assert score.score_answer(q["answer"], f"ANSWER: {facts['value']:.3f}")["correct"]
    # keeping the reaction LinRegPCR excluded gives a different answer
    assert not score.score_answer(q["answer"], f"ANSWER: {facts['variants']['excluded_kept']:.3f}")["correct"]
    q = qs["scn-plate-least-potent-ic50"]
    assert score.score_answer(q["answer"], f"ANSWER: {q['answer']['value'] / 1000:.4f} µM")["correct"]
    q = qs["scn-mic-nd2-strongest-bleaching"]
    assert score.score_answer(q["answer"], "ANSWER: run_b.nd2")["correct"]
    assert not score.score_answer(q["answer"], "ANSWER: run_b.nd2 or run_c.nd2")["correct"]
    q = qs["scn-share-damaged-files"]
    right = ", ".join(q["answer"]["value"])
    assert score.score_answer(q["answer"], f"ANSWER: {right}")["correct"]
    intact = next(r for r in q["answer"]["reject"] if "/" in r)
    assert not score.score_answer(q["answer"], f"ANSWER: {right}, {intact}")["correct"]


def test_list_reject():
    spec = {"type": "list", "value": ["A02"], "accept": [["A02"]], "reject": ["A01"]}
    assert score.score_answer(spec, "ANSWER: A02")["correct"]
    assert not score.score_answer(spec, "ANSWER: A01, A02")["correct"]
    spec.pop("reject")
    assert score.score_answer(spec, "ANSWER: A01, A02")["correct"]  # without reject, extras pass


def test_prompt_names_the_folder_and_its_sheet(tmp_path):
    q = questions()["scn-chrom-gcms-reaction-is-ratio"]
    prompt = run.build_prompt(q, [q["file"]["stage_as"] + "/"])
    assert "6 data files" in prompt and "samples.csv" in prompt


# ---------------------------------------------------------------- OME-Zarr output check


def _write_zarr(root: Path, version: int, scale: list[float], unit: str = "micrometer", compress: bool = True):
    import imagecodecs
    import numpy as np

    shape = [2, 3, 8, 10]  # c z y x
    data = np.arange(np.prod(shape), dtype="<u2").reshape(shape)
    axes = [{"name": "c", "type": "channel"}] + [{"name": n, "type": "space", "unit": unit} for n in "zyx"]
    ms = {"axes": axes, "datasets": [{"path": "0", "coordinateTransformations": [{"type": "scale", "scale": scale}]}]}
    arr = root / "0"
    arr.mkdir(parents=True)
    raw = data.tobytes()
    if version == 3:
        (root / "zarr.json").write_text(json.dumps(
            {"zarr_format": 3, "node_type": "group", "attributes": {"ome": {"version": "0.5", "multiscales": [ms]}}}
        ))  # fmt: skip
        codecs = [{"name": "bytes", "configuration": {"endian": "little"}}]
        if compress:
            codecs.append({"name": "zstd", "configuration": {"level": 3}})
        (arr / "zarr.json").write_text(json.dumps({
            "zarr_format": 3, "node_type": "array", "shape": shape, "data_type": "uint16",
            "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": shape}}, "codecs": codecs,
        }))  # fmt: skip
        (arr / "c").mkdir()
        (arr / "c" / "0").mkdir()
        (arr / "c" / "0" / "0").mkdir()
        (arr / "c" / "0" / "0" / "0").mkdir()
        (arr / "c" / "0" / "0" / "0" / "0").write_bytes(imagecodecs.zstd_encode(raw) if compress else raw)
    else:
        (root / ".zgroup").write_text('{"zarr_format": 2}')
        (root / ".zattrs").write_text(json.dumps({"multiscales": [dict(ms, version="0.4")]}))
        (arr / ".zarray").write_text(json.dumps({
            "zarr_format": 2, "shape": shape, "chunks": shape, "dtype": "<u2", "order": "C",
            "compressor": {"id": "zlib", "level": 1} if compress else None, "fill_value": 0, "filters": None,
        }))  # fmt: skip
        (arr / "0.0.0.0").write_bytes(imagecodecs.zlib_encode(raw) if compress else raw)


EXPECT = {"SizeX": 10, "SizeY": 8, "SizeZ": 3, "PhysicalSizeX": 0.1, "PhysicalSizeY": 0.1, "PhysicalSizeZ": 2.5}


@pytest.mark.parametrize("version", [2, 3])
def test_ome_zarr_check_reads_both_zarr_versions(tmp_path, version):
    store = tmp_path / "a.ome.zarr"
    _write_zarr(store, version, [1, 2.5, 0.1, 0.1])
    res = score.check_ome_zarr(store, EXPECT)
    assert res["ok"], res
    assert res["chunk"].startswith("decoded")


def test_ome_zarr_check_converts_units_and_fails_bad_calibration(tmp_path):
    good = tmp_path / "nm.ome.zarr"
    _write_zarr(good, 3, [1, 2500, 100, 100], unit="nanometer", compress=False)
    assert score.check_ome_zarr(good, EXPECT)["ok"]
    bad = tmp_path / "bad.ome.zarr"
    _write_zarr(bad, 2, [1, 1.0, 0.1, 0.1])  # z-step lost
    res = score.check_ome_zarr(bad, EXPECT)
    assert not res["ok"] and not res["PhysicalSizeZ"]["ok"]
    (tmp_path / "plain").mkdir()
    assert not score.check_ome_zarr(tmp_path / "plain", EXPECT)["ok"]


def test_check_task_with_several_outputs(tmp_path):
    _write_zarr(tmp_path / "sample_data" / "a.ome.zarr", 3, [1, 2.5, 0.1, 0.1])
    q = {"task": {"kind": "ome-zarr", "outputs": [{"output": "a.ome.zarr", "expect": EXPECT},
                                                  {"output": "b.ome.zarr", "expect": EXPECT}]}}  # fmt: skip
    res = score.check_task(q, tmp_path)
    assert not res["ok"] and res["outputs"]["a.ome.zarr"]["ok"]
    assert "not written" in res["outputs"]["b.ome.zarr"]["reason"]
    _write_zarr(tmp_path / "b.ome.zarr", 2, [1, 2.5, 0.1, 0.1])
    assert score.check_task(q, tmp_path)["ok"]


def test_zarr_scenario_expects_the_oracle_calibration():
    q = questions()["scn-mic-nd2-zstacks-to-ome-zarr"]
    assert q["task"]["kind"] == "ome-zarr"
    outs = {o["output"]: o["expect"] for o in q["task"]["outputs"]}
    assert set(outs) == {"stack_a.ome.zarr", "stack_b.ome.zarr"}
    assert outs["stack_b.ome.zarr"]["PhysicalSizeZ"] == q["answer"]["value"]


def test_scenario_facts_match_the_corpus_files():
    """Recompute facts/scenario.json when the oracle readers and the corpus are present (the oracle
    venv: `oracle/.venv/bin/python -m pytest evals/tests -k scenario`); skipped in the evals venv."""
    for mod in ("flowkit", "pyteomics", "nd2", "brukeropus", "allotropy", "aston", "pyabf"):
        pytest.importorskip(mod)
    import facts

    if not (facts.corpus_dir() / "flowkit-8c-e01.fcs").exists():
        pytest.skip("corpus not fetched")
    new = json.dumps(scenario_facts.compute(), sort_keys=True)
    assert new == json.dumps(json.loads(scenario_facts.FACTS.read_text()), sort_keys=True)
