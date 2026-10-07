"""Tests for the eval harness: answer parsing, unit conversion, task checks, staging. No model calls."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import numpy as np
import pytest
import tifffile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import facts
import generate
import run
import score
import share


def num(value, unit=None, rel=None, abs_=None):
    return generate.number(value, unit, rel=rel, abs_=abs_)


# ---------------------------------------------------------------- answer line


@pytest.mark.parametrize(
    ("text", "want"),
    [
        ("blah\nANSWER: 5", "5"),
        ("**ANSWER:** 0.5 µm", "0.5 µm"),
        ("Answer: first\nmore\nANSWER: second", "second"),
        ("`ANSWER: yes`", "yes"),
        ("no answer line here", None),
        (None, None),
    ],
)
def test_extract_answer(text, want):
    assert score.extract_answer(text) == want


# ---------------------------------------------------------------- numbers and units


@pytest.mark.parametrize(
    ("spec", "answer", "ok"),
    [
        (num(1.0833, "µm", rel=0.01), "1.083 µm", True),
        (num(1.0833, "µm", rel=0.01), "1083 nm", True),
        (num(1.0833, "µm", rel=0.01), "1.08 um/pixel", True),
        (num(1.0833, "µm", rel=0.01), "1.2 µm", False),
        (num(1.0833, "µm", rel=0.01), "1.083", True),  # bare number read in the expected unit
        (num(11.4, "Å", rel=0.01), "1.14 nm", True),
        (num(11.4, "Å", rel=0.01), "11.4 angstroms", True),
        (num(11.4, "Å", rel=0.01), "11.4 A", True),
        (num(3.0044e-5, "µm", rel=0.01), "0.030 nm", True),
        (num(3.0044e-5, "µm", rel=0.01), "3.0 × 10^-11 m", True),
        (num(3.0044e-5, "µm", rel=0.01), "3.0 x 10⁻¹¹ m", True),
        (num(100000, "Hz", rel=0.001), "100 kHz", True),
        (num(100000, "Hz", rel=0.001), "100,000 Hz", True),
        (num(100000, "Hz", rel=0.001), "10 kHz", False),
        (num(1017.375, "Hz", rel=0.001), "1017.4 samples/s", True),
        (num(400.152, "MHz", rel=0.002), "400.15 MHz", True),
        (num(400.152, "MHz", rel=0.002), "400 MHz", True),
        (num(29.234, "s", rel=0.005), "0.4872 min", True),
        (num(29.234, "s", rel=0.005), "29234 ms", True),
        (num(10.0, "min", abs_=0.05), "600 s", True),
        (num(298, "K", abs_=0.5), "24.85 °C", True),
        (num(298, "K", abs_=0.5), "25 C", True),
        (num(298, "K", abs_=0.5), "298 K", True),
        (num(-119.14, "pA", abs_=0.05), "-0.11914 nA", True),
        (num(-119.14, "pA", abs_=0.05), "−119.14 pA", True),
        (generate.integer(94), "94 MS2 scans", True),
        (generate.integer(94), "MS2 scans: 94", True),  # the 2 of MS2 is not a number
        (generate.integer(5785), "5,785 events", True),
        (generate.integer(20), "21", False),
        (num(84.0803, None, abs_=0.01), "m/z 84.08", True),
        (num(254, "nm", abs_=0.5), "254 nm", True),
        (num(254, "nm", abs_=0.5), "0.254 µm", True),
        (num(254, "nm", abs_=0.5), "254 s", False),
    ],
)
def test_numbers(spec, answer, ok):
    assert score.score_answer(spec, f"ANSWER: {answer}")["correct"] is ok, answer


def test_bare_number_is_flagged():
    got = score.score_answer(num(1.0, "µm", rel=0.01), "ANSWER: 1")
    assert got["correct"] and got["unit_assumed"] == "µm"


# ---------------------------------------------------------------- strings, booleans, lists, dates


def test_string_variants_and_reject():
    spec = generate.string("negative", ["neg"], reject=["positive"])
    assert score.score_answer(spec, "ANSWER: Negative ion mode")["correct"]
    assert not score.score_answer(spec, "ANSWER: positive or negative")["correct"]
    assert not score.score_answer(spec, "ANSWER: negligible")["correct"]


def test_string_word_boundaries():
    spec = generate.string("100x")
    assert score.score_answer(spec, "ANSWER: Plan Apo 100× Oil")["correct"]
    assert score.score_answer(spec, "ANSWER: 100 x")["correct"]
    assert not score.score_answer(spec, "ANSWER: 10x")["correct"]
    assert not score.score_answer(spec, "ANSWER: 1100x")["correct"]


def test_string_trademark_sign():
    spec = generate.items(["Alexa Fluor 405"])
    assert score.score_answer(spec, "ANSWER: Alexa Fluor™ 405")["correct"]


@pytest.mark.parametrize(
    ("answer", "want"),
    [
        ("No", False),
        ("no, it is truncated", False),
        ("The file is truncated", False),
        ("Not intact", False),
        ("incomplete", False),
        ("Yes", True),
        ("yes, complete and intact", True),
        ("Intact", True),
        ("maybe", None),
    ],
)
def test_boolean(answer, want):
    assert score.parse_boolean(answer) is want


def test_list_needs_every_item():
    spec = generate.items(["FSC-H", "SSC-H", "FL1-H", "FL2-H"])
    assert score.score_answer(spec, "ANSWER: FSC-H, SSC-H, FL1-H, FL2-H")["correct"]
    got = score.score_answer(spec, "ANSWER: FSC-H, SSC-H, FL1-H")
    assert not got["correct"] and got["missing"] == ["FL2-H"]


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("2009-03-06", True),
        ("6 March 2009", True),
        ("March 6, 2009", True),
        ("06/03/2009", True),  # d/m/y
        ("03/06/2009", True),  # m/d/y
        ("2009-03-07", False),
        ("sometime in 2009", False),
    ],
)
def test_dates(answer, ok):
    assert score.score_answer(generate.date("2009-03-06"), f"ANSWER: {answer}")["correct"] is ok


def test_date_tolerance():
    spec = generate.date("2019-07-08", tolerance_days=1)
    assert score.score_answer(spec, "ANSWER: 2019-07-09")["correct"]
    assert not score.score_answer(spec, "ANSWER: 2019-07-10")["correct"]


def test_unknown_and_missing():
    spec = generate.integer(5)
    assert score.score_answer(spec, "ANSWER: unknown")["reason"] == "answered unknown"
    assert score.score_answer(spec, "The answer is 5.")["reason"] == "no ANSWER: line"


# ---------------------------------------------------------------- conversion tasks


def _ome(path: Path, z_step: float, unit: str = "µm") -> None:
    data = np.zeros((2, 2, 5, 16, 16), dtype=np.uint16)
    tifffile.imwrite(
        path,
        data,
        ome=True,
        metadata={
            "axes": "TCZYX",
            "PhysicalSizeX": 0.5,
            "PhysicalSizeY": 0.5,
            "PhysicalSizeZ": z_step,
            "PhysicalSizeZUnit": unit,
        },
    )


EXPECT = {"SizeX": 16, "SizeY": 16, "SizeZ": 5, "SizeC": 2, "SizeT": 2, "PhysicalSizeZ": 1.0}


def test_ome_tiff_check(tmp_path):
    _ome(tmp_path / "out.ome.tiff", 1.0)
    assert score.check_ome_tiff(tmp_path / "out.ome.tiff", EXPECT)["ok"]


def test_ome_tiff_check_converts_units(tmp_path):
    _ome(tmp_path / "out.ome.tiff", 1000.0, "nm")
    assert score.check_ome_tiff(tmp_path / "out.ome.tiff", EXPECT)["ok"]


def test_ome_tiff_check_catches_wrong_geometry_and_plain_tiff(tmp_path):
    _ome(tmp_path / "out.ome.tiff", 2.0)
    got = score.check_ome_tiff(tmp_path / "out.ome.tiff", EXPECT)
    assert not got["ok"] and not got["PhysicalSizeZ"]["ok"]
    tifffile.imwrite(tmp_path / "plain.tiff", np.zeros((4, 4), np.uint8))
    assert not score.check_ome_tiff(tmp_path / "plain.tiff", EXPECT)["ok"]


def test_csv_check(tmp_path):
    p = tmp_path / "events.csv"
    p.write_text("A,B,C\n" + "".join(f"{i},{i * 2},{i * 3}\n" for i in range(10)))
    assert score.check_csv(p, {"rows": 10, "columns": 3})["ok"]
    assert not score.check_csv(p, {"rows": 11, "columns": 3})["ok"]
    # pandas' default index column is tolerated
    q = tmp_path / "idx.csv"
    q.write_text(",A,B,C\n" + "".join(f"{i},{i},{i},{i}\n" for i in range(10)))
    assert score.check_csv(q, {"rows": 10, "columns": 3})["ok"]
    # two header rows ($PnN and $PnS)
    r = tmp_path / "labels.csv"
    r.write_text("time_s,IN 0\ns,pA\n0,-119.14\n5e-05,-118.9\n")
    assert score.check_csv(r, {"rows": 2, "first_value": -119.14, "first_value_tolerance": 0.05})["ok"]


def test_task_verdict_needs_the_file(tmp_path):
    q = {
        "answer": generate.integer(2),
        "task": {"output": "out.ome.tiff", "kind": "ome-tiff", "expect": EXPECT},
    }
    rec = {"final_text": "ANSWER: 2", "artifact": score.check_task(q, tmp_path)}
    assert not score.score_record(q, rec)["correct"]
    _ome(tmp_path / "out.ome.tiff", 1.0)
    rec["artifact"] = score.check_task(q, tmp_path)
    assert score.score_record(q, rec)["correct"]


# ---------------------------------------------------------------- questions, selection, staging


def test_questions_are_up_to_date():
    rendered = generate.render(generate.build_all())
    for name, text in rendered.items():
        assert (generate.OUT_DIR / name).read_text() == text, f"run evals/generate.py ({name})"


def test_question_schema():
    qs = {k: q for k, q in score.load_questions().items() if q.get("split") != "heldout"}  # heldout: its own test
    assert 45 <= len(qs) <= 400
    for q in qs.values():
        for key in ("id", "family", "category", "file", "question", "answer", "source"):
            assert key in q, (q["id"], key)
        assert q["category"] in generate.CATEGORIES
        assert q["answer"]["type"] in score.SCORERS
        assert any(s in q["source"] for s in ("corpus/", "harness", "evals/facts/")), q["id"]
        # The file name must not give the answer away: every staged name is neutral.
        assert q["file"]["stage_as"].startswith(("sample", generate.UNKNOWN_NAME)), q["id"]
    fams = {q["family"] for q in qs.values()}
    assert fams == set(generate.FAMILIES)


def test_question_ids_carry_their_tier_prefix():
    """README.md: analysis `ana-*`, quantitation `qnt-*`, batch `batch-*`, scenario `scn-*`, visual
    `vis-*`, search `share-*`, conversion `task-*`, held-out `ho-*` (whatever the category)."""
    qs = score.load_questions().values()
    wrong = [(q["id"], generate.id_prefix(q)) for q in qs if not q["id"].startswith(generate.id_prefix(q) or "")]
    assert not wrong, wrong
    assert generate.id_prefix({"category": "analysis", "split": "heldout"}) == "ho-"
    assert generate.id_prefix({"category": "counts", "split": "dev"}) is None


def test_family_taxonomy_agrees():
    """Every family the questions use is declared in generate.FAMILIES and has a README name
    (readme_counts.py), and every question's format maps to its family."""
    import readme_counts

    declared = set(generate.FAMILIES)
    used = {q["family"] for q in score.load_questions().values()}
    assert used <= declared, used - declared
    assert set(readme_counts.FAMILY_NAMES) == declared - {"share"}
    formats = [f for fmts in generate.FAMILIES.values() for f in fmts]
    assert len(formats) == len(set(formats)), "a format belongs to one family"
    for q in score.load_questions().values():
        if q.get("format") in formats:
            assert generate.family_of(q["format"]) == q["family"], q["id"]


def test_stratified_covers_every_group():
    qs = list(score.load_questions().values())
    picked = run.stratified(qs, 20)
    assert len(picked) == 20
    assert {run.group_of(q) for q in picked} == {run.group_of(q) for q in qs}


def test_stage_truncates_a_copy(tmp_path):
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    (corpus / "a.bin").write_bytes(b"x" * 1000)
    work = tmp_path / "work"
    work.mkdir()
    q = {
        "id": "t",
        "file": {"corpus_id": "a", "path": "a.bin", "stage_as": "sample.bin"},
        "prepare": {"truncate_fraction": 0.6},
    }
    assert run.stage(q, work, corpus) == ["sample.bin"]
    assert (work / "sample.bin").stat().st_size == 600
    assert (corpus / "a.bin").stat().st_size == 1000


def test_fixtures_score():
    fixtures = score.load_records(run.FIXTURES)
    qs = score.load_questions()
    by_cond = {c: [r for r in fixtures if r["condition"] == c] for c in ("with", "without")}
    with_ok = sum(score.score_record(qs[r["id"]], r)["correct"] for r in by_cond["with"])
    without_ok = sum(score.score_record(qs[r["id"]], r)["correct"] for r in by_cond["without"])
    assert with_ok == len(by_cond["with"])  # every recorded `with` fixture answer is right
    assert without_ok == 11


def test_command_isolation():
    import argparse

    args = argparse.Namespace(claude="claude", model="m", max_turns=3, max_budget_usd=0.1, with_mode="all", timeout=10)
    h = run.Harness(None, "all", keep=False)
    try:
        cmd, env = run.claude_command(args, h, "without", "PROMPT")
        assert "--strict-mcp-config" in cmd and "--mcp-config" not in cmd and "--plugin-dir" not in cmd
        assert "Skill" not in cmd[cmd.index("--tools") + 1]
        assert set(env) <= set(run.ENV_ALLOW) | {"PATH", "DISABLE_AUTOUPDATER"}
        assert all("openreadout" not in json.dumps(v) or k == "PATH" for k, v in env.items())
    finally:
        h.close()


# ---------------------------------------------------------------- experiment facts


def test_facts_are_well_formed():
    data = generate.load_facts()
    manifest = generate.load_manifest()
    assert set(data) == set(facts.SOURCES)
    for cid, entry in data.items():
        assert cid in manifest, cid
        assert entry["extractor"].startswith("evals/facts.py: ")
        assert entry["facts"], cid
        for name, f in entry["facts"].items():
            assert set(f) == {"value", "where"}, (cid, name)
            assert f["value"] not in (None, ""), (cid, name)


def test_experiment_questions_cite_independent_sources():
    qs = score.load_questions()
    exp = [q for q in qs.values() if q["id"].startswith("exp-")]
    assert len(exp) >= 15
    assert {"sample", "method", "operator"} <= {q["category"] for q in exp}
    for q in exp:
        assert any(
            s in q["source"] for s in ("evals/facts/experiment.json", "corpus/oracle/", "corpus/manifest.toml")
        ), q["id"]


def test_analysis_facts_are_well_formed():
    data = generate.load_facts("facts-analysis")
    manifest = generate.load_manifest()
    for cid, entry in data.items():
        assert cid in manifest, cid
        assert entry["extractor"] == "evals/analysis.py" and entry["file"], cid
        for name, f in entry["facts"].items():
            assert {"value", "reader", "how"} <= set(f) <= {"value", "reader", "how", "files", "cross_check"}, name
            assert f["value"] not in (None, "", {}), (cid, name)
            assert "openreadout" not in json.dumps(f).lower(), (cid, name)


def test_analysis_questions_are_well_formed():
    qs = [q for q in score.load_questions().values() if q["category"] == "analysis" and q.get("split") != "heldout"]
    assert 25 <= len(qs) <= 250  # a sanity bound on the generator, not a target
    assert {q["family"] for q in qs} == set(generate.FAMILIES) - {"share"}
    for q in qs:
        assert q["id"].startswith("ana-"), q["id"]
        # analysis.py's facts, spectroscopy.py's for the FT-IR/Raman questions, regions.py's for the
        # whole-slide region/level questions (ana-wsi-*), or agent_facts.py's (evals/facts/agent.json)
        if q["source"].startswith("evals/facts/spectroscopy.json ["):
            kind = "facts-spectroscopy"
        elif q["source"].startswith("evals/facts/agent.json ["):
            kind = "facts-agent"
        elif q["source"].startswith("evals/facts/routing.json ["):  # evals/routing_facts.py
            kind = "facts-routing"
        elif q["source"].startswith("evals/facts/gaps.json ["):  # evals/gaps_facts.py
            kind = "facts-gaps"
        elif q["source"].startswith("evals/facts/bench.json ["):  # evals/bench.py
            kind = "facts-bench"
        elif q["id"].startswith("ana-wsi-"):
            kind = "facts-regions"
        else:
            kind = "facts-analysis"
        assert q["source"].startswith(f"evals/facts/{generate.facts_path(kind).name} ["), q["id"]
        data = generate.load_facts(kind)
        name = q["source"].split(f"{kind}: ", 1)[1].split(" ", 1)[0]
        assert name in data[q["file"]["corpus_id"]]["facts"], q["id"]
        assert q["answer_hint"] and q["answer"]["type"] in ("number", "string"), q["id"]
        if q["answer"]["type"] == "number":
            tol = q["answer"]["tolerance"]
            assert tol.get("rel", 0) <= 0.01, q["id"]  # tight enough that only the right computation passes
            if isinstance(q["answer"]["value"], int):
                assert tol == {"abs": 0}, q["id"]
        for ref in [q["file"], *q.get("extra_files", [])]:
            assert ref["stage_as"].startswith("sample"), q["id"]


def test_region_facts_come_from_third_party_readers():
    data = generate.load_facts("facts-regions")
    assert {"openslide-aperio-cmu-1", "openslide-hamamatsu-cmu-1", "zenodo10577621-Young-mouse"} <= set(data)
    for entry in data.values():
        assert entry["extractor"] == "evals/regions.py"
        for f in entry["facts"].values():
            assert "openreadout" not in f["reader"].lower()
            assert f["reader"].split()[0] in ("tifffile", "czifile", "pylibCZIrw"), f["reader"]


def test_quant_facts_and_questions_are_well_formed():
    data = generate.load_facts("facts-quant")
    manifest = generate.load_manifest()
    for cid, entry in data.items():
        assert cid in manifest, cid
        assert entry["extractor"] == "evals/quant.py" and entry["file"], cid
        for name, f in entry["facts"].items():
            assert set(f) == {"value", "reader", "how"}, name
            assert "openreadout" not in json.dumps(f).lower(), (cid, name)
    qs = [
        q
        for q in score.load_questions().values()
        if q["category"] == "quantitation" and not q["source"].startswith("evals/facts/openlab.json [")
    ]
    assert len(qs) >= 5
    for q in qs:
        assert q["id"].startswith("qnt-"), q["id"]
        assert q["source"].startswith("evals/facts/quant.json ["), q["id"]
        name = q["source"].split("facts-quant: ", 1)[1].split(" ", 1)[0]
        assert name in data[q["file"]["corpus_id"]]["facts"], q["id"]
        assert q["answer"]["type"] == "number", q["id"]
        # wide enough for any correct integrator, narrow enough that a wrong peak fails
        tol = q["answer"]["tolerance"]
        assert tol.get("rel", 0) <= 0.15 and tol.get("abs", 0) <= 1.0, q["id"]
        assert q["file"]["stage_as"].startswith("sample"), q["id"]
        # the vendor report the answer comes from is not staged with the signal file
        assert not q["file"]["path"].lower().endswith(("report.txt", "results.csv")), q["id"]


def test_openlab_facts_and_questions_are_well_formed():
    """OpenLab CDS answers are the vendor's own CSV export, which is never staged."""
    data = generate.load_facts("facts-openlab")
    manifest = generate.load_manifest()
    for cid, entry in data.items():
        assert cid in manifest, cid
        assert entry["extractor"] == "evals/openlab_facts.py" and entry["file"], cid
        for name, f in entry["facts"].items():
            assert set(f) == {"value", "reader", "how"}, name
            assert "openreadout" not in json.dumps(f).lower(), (cid, name)
    qs = [q for q in score.load_questions().values() if q["source"].startswith("evals/facts/openlab.json [")]
    assert len(qs) >= 4
    for q in qs:
        assert q["format"] == "openlab-cds", q["id"]
        assert q["file"]["path"].endswith(".dx") and q["file"]["stage_as"] == "sample.dx", q["id"]
        extra = q.get("extra_files", [])
        # the result package beside the injection, or nothing (the tail-peak questions make the
        # agent integrate the signal itself)
        assert [e["stage_as"] for e in extra] in (["sample.rx"], []), q["id"]
        assert bool(extra) != q["id"].startswith("qnt-openlab-tail-"), q["id"]
        assert not any(e["path"].lower().endswith(".csv") for e in extra), q["id"]
        name = q["source"].split("facts-openlab: ", 1)[1].split(" ", 1)[0]
        assert name in data[q["file"]["corpus_id"]]["facts"], q["id"]


def test_analysis_answers_score():
    qs = score.load_questions()
    for qid, answer, ok in [
        ("ana-mic-czi-compare-mean", "sample_b.czi", True),
        ("ana-mic-czi-compare-mean", "sample_a.czi", False),
        ("ana-mic-czi-compare-mean", "sample_b.czi (higher than sample_a.czi)", False),
        ("ana-mic-lif-brightest-series", "Series007", True),
        ("ana-mic-lif-brightest-series", "Series012", False),
        ("ana-ephys-ncs-std", "54.4 uV", True),
        ("ana-ephys-ncs-std", "0.0544 mV", True),
        ("ana-ms-raw-tic-apex", "36.77 s", True),
        ("ana-plate-gen5-top-well", "B2", True),
        ("ana-plate-gen5-top-well", "B8", False),
    ]:
        assert score.score_answer(qs[qid]["answer"], f"ANSWER: {answer}")["correct"] is ok, (qid, answer)


def test_generate_needs_no_oracle_readers():
    """generate.py runs in the evals venv: building the analysis questions must not import a reader."""
    readers = {"czifile", "nd2", "liffile", "mrcfile", "pyabf", "flowio", "pyteomics", "nmrglue", "rainbow"}
    generate.build_all()
    assert not readers & set(sys.modules)


def test_analysis_facts_match_the_corpus_files():
    """Recompute when the oracle readers and the corpus are present (the oracle venv); skipped otherwise."""
    import importlib.util

    if importlib.util.find_spec("czifile") is None:
        pytest.skip("oracle readers not installed (run with oracle/.venv/bin/python -m pytest)")
    import analysis

    missing = [
        f.corpus_id for f in analysis.FACTS if not (facts.corpus_dir() / facts.manifest_file(f.corpus_id)).exists()
    ]
    if missing:
        pytest.skip(f"corpus files not present: {', '.join(missing[:3])}")
    assert analysis.render(analysis.compute()) == analysis.OUT.read_text(encoding="utf-8")


def test_facts_match_the_corpus_files():
    """Re-extract when the corpus is present: the committed facts must not drift from the files."""
    missing = [
        cid
        for cid, (role, _, _) in facts.SOURCES.items()
        if not (facts.corpus_dir() / facts.manifest_file(cid, role)).exists()
    ]
    if missing:
        pytest.skip(f"corpus files not present: {', '.join(missing[:3])}")
    assert facts.render(facts.extract()) == facts.OUT.read_text(encoding="utf-8")


def test_fcs_text_parser_handles_escaped_delimiters(tmp_path):
    text = "/$WELLID/B03/TUBE NAME/a//b/$OP/me/"
    start = 58
    header = f"FCS3.1    {start:>8}{start + len(text) - 1:>8}{0:>8}{0:>8}{0:>8}{0:>8}".ljust(start)
    p = tmp_path / "x.fcs"
    p.write_bytes((header + text).encode("latin-1"))
    got = {k: v["value"] for k, v in facts.fcs_text(p).items()}
    assert got == {"well_id": "B03", "tube_name": "a/b", "operator": "me"}


# ---------------------------------------------------------------- search questions over a share


def test_share_questions_are_independent_and_well_formed():
    qs = [q for q in score.load_questions().values() if q["category"] == "search"]
    assert len(qs) >= 10
    for q in qs:
        assert q["source"].startswith("evals/share.py over corpus/oracle"), q["id"]
        assert q["file"]["stage_as"] == share.SHARE_DIR
        staged = [r["stage_as"] for r in q["share"]]
        assert len(staged) == len(set(staged)) >= 9, q["id"]
        for s in staged:
            # Neutral names: the folder and run number say nothing about the answer.
            assert s.startswith(share.SHARE_DIR + "/") and "/run_" in s, s
        rel = {s.split("/", 1)[1] for s in staged}
        if q["answer"]["type"] == "string":
            assert q["answer"]["value"] in rel
        if q["answer"]["type"] == "list":
            assert set(q["answer"]["value"]) <= rel
    assert any(q["family"] == "share" for q in qs)


def test_share_answers_score():
    qs = score.load_questions()
    q = qs["share-largest-image"]
    right = q["answer"]["value"]
    other = next(r["stage_as"].split("/", 1)[1] for r in q["share"] if not r["stage_as"].endswith(right))
    assert score.score_answer(q["answer"], f"ANSWER: {share.SHARE_DIR}/{right}")["correct"]
    assert score.score_answer(q["answer"], f"ANSWER: {right}")["correct"]
    assert not score.score_answer(q["answer"], f"ANSWER: {other}")["correct"]
    cd4 = qs["share-fcs-cd4"]["answer"]
    # The answer is generated from the corpus, so it grows as FCS files are added.
    assert len(cd4["value"]) >= 2
    assert score.score_answer(cd4, "ANSWER: " + ", ".join(cd4["value"]))["correct"]
    assert not score.score_answer(cd4, f"ANSWER: {cd4['value'][0]}")["correct"], "every file must be named"
    n = qs["share-fcs-large"]
    assert score.score_answer(n["answer"], f"ANSWER: {n['answer']['value']} files")["correct"]


def test_stage_share_clones_and_truncates(tmp_path):
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    (corpus / "a.fcs").write_bytes(b"a" * 1000)
    (corpus / "b.nd2").write_bytes(b"b" * 500)
    work = tmp_path / "work"
    work.mkdir()
    q = {
        "id": "t",
        "file": {"corpus_id": "share", "path": "", "stage_as": "sample_share"},
        "share": [
            {"corpus_id": "a", "path": "a.fcs", "stage_as": "sample_share/scope_a/run_001.fcs"},
            {"corpus_id": "b", "path": "b.nd2", "stage_as": "sample_share/scope_b/2019/run_002.nd2"},
        ],
        "prepare": {"truncate": {"sample_share/scope_a/run_001.fcs": 0.6}},
    }
    assert run.stage(q, work, corpus) == ["sample_share/"]
    assert (work / "sample_share/scope_a/run_001.fcs").stat().st_size == 600
    assert (work / "sample_share/scope_b/2019/run_002.nd2").read_bytes() == b"b" * 500
    assert (corpus / "a.fcs").stat().st_size == 1000, "the corpus is never touched"
    prompt = run.build_prompt({**q, "question": "Q?", "answer_hint": "a number"}, ["sample_share/"])
    assert "a lab file share with 2 files" in prompt


# ---------------------------------------------------------------- benchmark: baseline, splits, stats, triage


def test_baseline_command(monkeypatch):
    import argparse

    monkeypatch.setattr(run.baseline, "ready", lambda *a, **k: None)
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    args = argparse.Namespace(claude="claude", model="m", max_turns=3, max_budget_usd=0.1, with_mode="all", timeout=10)
    h = run.Harness(None, "all", keep=False)
    try:
        cmd, env = run.claude_command(args, h, "baseline", "PROMPT")
        assert cmd[cmd.index("--plugin-dir") + 1] == str(h.baseline_plugin)
        assert "--mcp-config" not in cmd and "--disable-slash-commands" not in cmd
        assert "Skill" in cmd[cmd.index("--tools") + 1]
        assert env["PATH"].split(os.pathsep)[:2] == [str(d) for d in run.baseline.bin_dirs()]
        assert "ANTHROPIC_API_KEY" not in env, "subscription auth unless --allow-api-key"
        assert (h.baseline_plugin / "skills" / "lab-file-readers" / "SKILL.md").exists()
        skill = (h.baseline_plugin / "skills" / "lab-file-readers" / "SKILL.md").read_text()
        assert "openreadout" not in skill.lower()
    finally:
        h.close()


def test_run_refuses_api_key(monkeypatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    monkeypatch.setattr(run, "corpus_dir", lambda: Path("."))
    with pytest.raises(SystemExit, match="subscription"):
        run.main(["--ids", "mic-czi-pixel-size", "--conditions", "without", "--run-name", "never-written"])


@pytest.mark.parametrize(
    ("res", "stderr", "want"),
    [
        ({"is_error": True, "result": "Claude AI usage limit reached|1760000000"}, "", True),
        ({"is_error": True, "result": "You've hit your limit · resets 3pm"}, "", True),
        ({}, "API Error: 429 rate_limit_error", True),
        ({"is_error": False, "result": "ANSWER: rate limit is 5 Hz"}, "", False),
        ({"is_error": True, "result": "some other failure"}, "", False),
    ],
)
def test_hit_limit(res, stderr, want):
    assert run.hit_limit(res, stderr) is want


def test_splits_are_by_file_and_stable():
    qs = [q for q in score.load_questions().values() if q["split"] != "heldout"]
    assert {q["split"] for q in qs} == {"dev", "test"}
    by_file: dict[str, set] = {}
    for q in qs:
        if "share" not in q:
            by_file.setdefault(q["file"]["corpus_id"], set()).add(q["split"])
    assert all(len(s) == 1 for s in by_file.values()), "every question about one file shares its split"
    # Pinned: changing the salt or the rule would move questions between splits.
    assert generate.split_of({"id": "x", "file": {"corpus_id": "zenodo7015307-S-2-2x2-CH-1"}}) == generate.split_of(
        {"id": "y", "file": {"corpus_id": "zenodo7015307-S-2-2x2-CH-1"}}
    )
    assert generate.SPLIT_SALT == "icli-split-1" and generate.DEV_PERCENT == 60


def test_sign_test():
    import stats

    assert stats.sign_test(0, 0) == 1.0
    assert stats.sign_test(4, 0) == pytest.approx(0.125)
    assert stats.sign_test(10, 0) == pytest.approx(2 / 1024)
    assert stats.sign_test(3, 3) == 1.0


def test_stats_on_synthetic_records(tmp_path, monkeypatch):
    import stats

    monkeypatch.setattr(stats, "RESULTS", tmp_path)
    qs = score.load_questions()
    ids = [q["id"] for q in qs.values() if q["answer"]["type"] == "number" and q["split"] != "heldout"][:6]

    def rec(qid, ok, rep):
        v = qs[qid]["answer"]
        text = f"ANSWER: {v['value']} {v.get('unit') or ''}" if ok else "ANSWER: unknown"
        return {"id": qid, "rep": rep, "final_text": text, "cost_usd": 0.01, "num_turns": 3, "wall_s": 5}

    for cond, right in (("with", ids), ("baseline", ids[:3])):
        lines = [json.dumps(rec(q, q in right, r)) for q in ids for r in range(2)]
        (tmp_path / f"t-{cond}.records.jsonl").write_text("\n".join(lines) + "\n")
    r = stats.analyse("t", ["with", "baseline", "without"])
    assert r["conditions"] == ["with", "baseline"]
    assert r["overall"]["with"]["accuracy"] == 1.0 and r["overall"]["baseline"]["accuracy"] == 0.5
    assert r["overall"]["with"]["runs"] == 12 and r["overall"]["with"]["pass_all_k"] == 1.0
    p = r["paired"]["with vs baseline"]
    assert (p["wins"], p["losses"], p["ties"]) == (3, 0, 3)
    assert p["diff"] == pytest.approx(0.5) and p["diff_ci95"][0] > 0
    assert "**100 %**" in stats.markdown(r)


def test_triage_causes():
    import triage

    q = {"answer": generate.number(10.0, "Hz", rel=0.01), "category": "sample-rate"}
    for text, cause in (
        ("ANSWER: 600 Hz", "unit_slip"),
        ("ANSWER: 10 kHz", "unit_slip"),
        ("ANSWER: 37 Hz", "wrong_answer"),
    ):
        rec = {"condition": "without", "final_text": text, "subtype": "success"}
        assert triage.cause_of(q, rec, score.score_answer(q["answer"], text)) == cause, text
    qi = {"answer": generate.integer(12), "category": "counts"}
    r2 = {"condition": "with", "final_text": "ANSWER: 13", "subtype": "success", "used_openreadout": True}
    assert triage.cause_of(qi, r2, score.score_answer(qi["answer"], r2["final_text"])) == "off_by_one"
    r2["used_openreadout"] = False
    assert triage.cause_of(qi, r2, score.score_answer(qi["answer"], r2["final_text"])) == "tool_not_used"
    r3 = {"condition": "with", "final_text": None, "subtype": "error_max_turns"}
    assert triage.cause_of(qi, r3, score.score_answer(qi["answer"], None)) == "turn_cap"


def test_triage_finds_answers_in_tool_output():
    import triage

    doc = {"data": {"images": [{"physical_size_um": {"x": 0.1083}}], "objective": "Plan-Apochromat 63x/1.4 Oil"}}
    assert (
        triage.find_answer(generate.number(108.3, "nm", rel=0.01), doc) == "/data/images/0/physical_size_um/x (× 1000)"
    )
    assert triage.find_answer(generate.string("63x"), doc) == "/data/objective"
    assert triage.find_answer(generate.string("40x"), doc) is None


# ---------------------------------------------------------------- held-out split (evals/heldout.py)


def _heldout_manifest() -> dict[str, list[dict]]:
    return {cid: [e for e in es if e.get("tier") == "heldout"] for cid, es in generate.load_manifest().items()}


def test_heldout_questions_are_sealed_and_independent():
    """Held-out questions: their own file and split, only held-out files, answers from held-out ground truth."""
    import heldout

    qs = [q for q in score.load_questions().values() if q.get("split") == "heldout"]
    assert 40 <= len(qs) <= 600  # a sanity bound on the generator (draws add questions), not a target
    assert {Path(p).name for p in (generate.OUT_DIR).glob("*.jsonl") if "heldout" in p.name} == {"heldout.jsonl"}
    on_disk = [json.loads(line) for line in (generate.OUT_DIR / "heldout.jsonl").read_text().splitlines()]
    assert {q["id"] for q in on_disk} == {q["id"] for q in qs}
    manifest = _heldout_manifest()
    data = heldout.load_facts()
    for q in qs:
        assert q["id"].startswith("ho-"), q["id"]
        for ref in [q["file"], *q.get("extra_files", [])]:
            assert manifest.get(ref["corpus_id"]), (q["id"], ref["corpus_id"], "not a held-out file")
            # (a VSI's frame files sit in `_<name>_/`, so `_sample_/...` for `sample.vsi`)
            assert ref["stage_as"].startswith(("sample", "_sample_/", generate.UNKNOWN_NAME)), q["id"]
        src = q["source"]
        assert src.startswith(("corpus/oracle/heldout/", "evals/facts/heldout.json [", "corpus/manifest.toml [")), q[
            "id"
        ]
        if src.startswith("evals/facts/heldout.json"):
            name = src.split("facts-heldout: ", 1)[1].split(" ", 1)[0]
            assert name in data[q["file"]["corpus_id"]]["facts"], q["id"]
        if q["category"] == "analysis" and q["answer"]["type"] == "number":
            assert q["answer"]["tolerance"].get("rel", 0) <= 0.01, q["id"]
    # every family is covered, and the analysis tier is a real share of the set
    # Families added after the held-out set was drawn have no held-out file yet; add one, then drop it here.
    no_heldout_yet: set[str] = set()
    assert {q["family"] for q in qs} == set(generate.FAMILIES) - {"share"} - no_heldout_yet
    assert sum(q["category"] == "analysis" for q in qs) >= 15


def test_heldout_questions_never_touch_development_files():
    """No dev/test question asks about a held-out file, and no held-out question about a development file."""
    manifest = _heldout_manifest()
    for q in score.load_questions().values():
        refs = [q["file"], *q.get("extra_files", [])] if "share" not in q else q["share"]
        held = [bool(manifest.get(r.get("corpus_id", ""))) for r in refs]
        if q.get("split") == "heldout":
            assert all(held), q["id"]
        else:
            assert not any(held), q["id"]


def test_heldout_split_runs_only_when_asked():
    import argparse

    base = {"ids": None, "family": None, "category": None, "sample": None, "limit": None}
    everything = run.select(argparse.Namespace(split="all", **base))
    assert everything and all(q["split"] in ("dev", "test") for q in everything)
    held = run.select(argparse.Namespace(split="heldout", **base))
    assert held and all(q["split"] == "heldout" for q in held)
    assert len(held) == sum(q.get("split") == "heldout" for q in score.load_questions().values())


def test_heldout_answers_score():
    for q in score.load_questions().values():
        if q.get("split") == "heldout":
            text = "ANSWER: " + score.expected_text(q["answer"])
            assert score.score_answer(q["answer"], text)["correct"], (q["id"], text)


def test_stats_reports_heldout_on_its_own(tmp_path, monkeypatch):
    import stats

    monkeypatch.setattr(stats, "RESULTS", tmp_path)
    qs = score.load_questions()
    held = [
        q["id"]
        for q in qs.values()
        if q["split"] == "heldout" and q["answer"]["type"] == "number" and not q.get("exposed")
    ][:4]
    test = [q["id"] for q in qs.values() if q["split"] == "test" and q["answer"]["type"] == "number"][:4]

    def rec(qid, ok):
        v = qs[qid]["answer"]
        text = f"ANSWER: {v['value']} {v.get('unit') or ''}" if ok else "ANSWER: unknown"
        return json.dumps({"id": qid, "rep": 0, "final_text": text, "cost_usd": 0.01, "num_turns": 2, "wall_s": 3})

    lines = [rec(q, True) for q in test] + [rec(q, i < 2) for i, q in enumerate(held)]
    (tmp_path / "t-with.records.jsonl").write_text("\n".join(lines) + "\n")
    assert stats.analyse("t", ["with"], "all")["overall"]["with"]["questions"] == len(test)  # `all` = dev + test
    h = stats.analyse("t", ["with"], "heldout")
    assert h["overall"]["with"]["questions"] == len(held) and h["overall"]["with"]["accuracy"] == 0.5
    written = stats.write("t", ["with"], "heldout")
    md = written[1].read_text()
    assert "split: heldout" in md and "Generalization (held-out vs test split)" in md and "-50.0 pts" in md
    # a run without held-out records gets no held-out report
    (tmp_path / "u-with.records.jsonl").write_text("\n".join(rec(q, True) for q in test) + "\n")
    assert stats.write("u", ["with"], "heldout") == []


def test_exposed_heldout_questions_are_marked():
    """A held-out question carries `exposed` exactly when one of its files is an exposed held-out
    input in the manifest (docs/benchmark/heldout.md, rule 7)."""
    exposed_ids = {cid for cid, es in _heldout_manifest().items() if any(e.get("exposed") for e in es)}
    assert exposed_ids  # draw C holds eight
    held = [q for q in score.load_questions().values() if q["split"] == "heldout"]
    assert any(q.get("exposed") for q in held)
    for q in held:
        on_exposed = any(r["corpus_id"] in exposed_ids for r in [q["file"], *q.get("extra_files", [])])
        assert bool(q.get("exposed")) == on_exposed, q["id"]
    assert not any(q.get("exposed") for q in score.load_questions().values() if q["split"] != "heldout")


def test_stats_leaves_exposed_heldout_questions_out(tmp_path, monkeypatch):
    """Exposed held-out questions are scored but never count toward held-out accuracy."""
    import stats

    monkeypatch.setattr(stats, "RESULTS", tmp_path)
    qs = score.load_questions()
    held = [q for q in qs.values() if q["split"] == "heldout"]
    exposed = [q["id"] for q in held if q.get("exposed")][:3]
    counted = [q["id"] for q in held if not q.get("exposed")][:3]
    assert exposed and counted

    def rec(qid, ok):
        text = "ANSWER: " + score.expected_text(qs[qid]["answer"]) if ok else "ANSWER: unknown"
        return json.dumps({"id": qid, "rep": 0, "final_text": text, "cost_usd": 0.01, "num_turns": 2, "wall_s": 3})

    # every exposed question right, every counted one wrong: accuracy must be 0, not above it
    lines = [rec(q, True) for q in exposed] + [rec(q, False) for q in counted]
    (tmp_path / "t-with.records.jsonl").write_text("\n".join(lines) + "\n")
    h = stats.analyse("t", ["with"], "heldout")
    assert h["overall"]["with"]["questions"] == len(counted)
    assert h["overall"]["with"]["accuracy"] == 0.0
    assert h["exposed_questions_excluded"] == sum(bool(q.get("exposed")) for q in held)
    written = stats.write("t", ["with"], "heldout")
    assert "exposed held-out files" in written[1].read_text()


def test_heldout_facts_match_the_files():
    """Recompute when the oracle readers and the held-out files are present; skipped otherwise."""
    import importlib.util

    if importlib.util.find_spec("czifile") is None or importlib.util.find_spec("allotropy") is None:
        pytest.skip("oracle readers not installed (run with oracle/.venv/bin/python -m pytest)")
    import heldout

    missing = [
        f.corpus_id
        for f in heldout.FACTS
        if not (facts.corpus_dir() / facts.manifest_file(f.corpus_id, f.role)).exists()
    ]
    if missing:
        pytest.skip(f"held-out files not present: {', '.join(missing[:3])}")
    assert heldout.render(heldout.compute()) == heldout.OUT.read_text(encoding="utf-8")


def test_stats_turn_budgets_and_caps(tmp_path, monkeypatch):
    import stats

    monkeypatch.setattr(stats, "RESULTS", tmp_path)
    qs = score.load_questions()
    ids = [q for q in qs if qs[q]["answer"]["type"] == "number" and qs[q].get("split") != "heldout"][:4]
    recs = []
    for i, qid in enumerate(ids):
        v = qs[qid]["answer"]
        right = f"ANSWER: {v['value']} {v.get('unit') or ''}"
        turns, subtype, text = (
            [(5, "success", right), (15, "success", right), (35, "success", right)][i]
            if i < 3
            else (
                60,
                "error_max_turns",
                None,
            )
        )
        recs.append({"id": qid, "rep": 0, "final_text": text, "num_turns": turns, "subtype": subtype})
    (tmp_path / "t-with.records.jsonl").write_text("\n".join(json.dumps(r) for r in recs) + "\n")
    o = stats.analyse("t", ["with"])["overall"]["with"]
    assert o["accuracy"] == pytest.approx(0.75)
    assert o["capped_rate"] == pytest.approx(0.25)
    assert o["accuracy_at_turns"] == pytest.approx({"10": 0.25, "20": 0.5, "40": 0.75})


def test_preflight_tells_reader_gaps_from_staging_problems():
    """`check` exit 6 (unsupported variant) on a recognised file is a reader gap the run measures,
    not a staging problem; unrecognised files and integrity disagreements still need a look."""
    lookup = {"category": "counts", "answer": {"type": "number", "value": 7}, "file": {}}
    assert run.preflight_status(lookup, "emd", 0) == "ok"
    assert run.preflight_status(lookup, "emd", 6) == "GAP"
    assert run.preflight_status(lookup, "emd", 4) == "CHECK"
    assert run.preflight_status(lookup, None, 0) == "CHECK"
    intact = {"category": "integrity", "answer": {"type": "boolean", "value": True}, "file": {}}
    truncated = {**intact, "answer": {"type": "boolean", "value": False}}
    assert run.preflight_status(intact, "czi", 0) == "ok"
    assert run.preflight_status(intact, "czi", 6) == "CHECK"  # the answer depends on `check`
    assert run.preflight_status(truncated, "czi", 4) == "ok"
    assert run.preflight_status(truncated, "czi", 0) == "CHECK"
    partial = {**lookup, "file": {"partial": True}}
    assert run.preflight_status(partial, "opera-harmony", 4) == "ok"
