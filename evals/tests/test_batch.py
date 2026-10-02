"""Batch-tier questions: a folder of files plus a generated sample sheet or plate map."""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import batch
import run
import score


def questions():
    return [q for q in score.load_questions().values() if q["category"] == "batch"]


def test_batch_questions_are_well_formed_and_independent():
    qs = questions()
    assert len(qs) >= 6
    facts = json.loads(batch.FACTS.read_text())
    assert "openreadout" not in facts["generator"].lower()
    for q in qs:
        # evals/batch.py's facts, evals/agent_facts.py's for the analysis-over-a-folder question, or
        # evals/routing_facts.py's (folder MIPs, a raw/mzXML pairing)
        sources = (
            "evals/facts/batch.json",
            "evals/facts/agent.json",
            "evals/facts/routing.json",
            "evals/facts/gaps.json",  # evals/gaps_facts.py: a spectra folder with its sheet and README
        )
        assert q["source"].startswith(sources), q["id"]
        assert q["file"]["stage_as"] == batch.DIR
        staged = [r["stage_as"] for r in q["share"]] + [g["stage_as"] for g in q.get("generated", [])]
        assert len(staged) == len(set(staged)), q["id"]
        assert all(s.startswith(batch.DIR + "/") for s in staged), q["id"]
    assert {q["family"] for q in qs} >= {"flow", "plates", "ms"}


def test_batch_answers_score():
    qs = {q["id"]: q for q in questions()}
    q = qs["batch-fcs-donor-files"]
    right = ", ".join(q["answer"]["value"])
    assert score.score_answer(q["answer"], f"ANSWER: {right}")["correct"]
    assert not score.score_answer(q["answer"], f"ANSWER: {q['answer']['value'][0]}")["correct"]
    q = qs["batch-plate-highest-condition"]
    assert score.score_answer(q["answer"], f"ANSWER: {q['answer']['value']}")["correct"]
    wrong = q["answer"]["reject"][0]
    assert not score.score_answer(q["answer"], f"ANSWER: {q['answer']['value']} or {wrong}")["correct"]
    q = qs["batch-gate-cd4-of-cd3"]
    assert score.score_answer(q["answer"], f"ANSWER: {q['answer']['value'] + 0.1:.2f}%")["correct"]


def test_stage_writes_the_generated_sheet(tmp_path):
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    (corpus / "a.fcs").write_bytes(b"x" * 10)
    q = {
        "id": "t",
        "category": "batch",
        "file": {"corpus_id": "share", "path": "", "stage_as": batch.DIR},
        "share": [{"corpus_id": "a", "path": "a.fcs", "stage_as": f"{batch.DIR}/run_001.fcs"}],
        "generated": [{"stage_as": f"{batch.DIR}/samples.csv", "content": "file,condition\nrun_001,ctrl\n"}],
        "question": "Q?",
        "answer_hint": "a number",
    }
    work = tmp_path / "work"
    names = run.stage(q, work, corpus)
    assert (work / batch.DIR / "samples.csv").read_text().startswith("file,condition")
    prompt = run.build_prompt(q, names)
    assert "1 data files" in prompt and "samples.csv" in prompt
