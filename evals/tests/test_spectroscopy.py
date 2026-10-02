"""The FT-IR/Raman questions (evals/spectroscopy.py): well formed, scored in wavenumbers, and the
JCAMP-DX conversion check (score.check_jcamp, the jcamp package)."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import generate
import score


def spectro_questions() -> dict:
    return {
        q["id"]: q
        for q in score.load_questions().values()
        if q["family"] == "spectroscopy"
        and q.get("split") != "heldout"  # held-out: test_evals.py
        and q["category"] != "scenario"  # folder scenarios: tests/test_scenario.py
    }


def test_spectroscopy_questions_exist_and_cite_third_party_facts():
    qs = spectro_questions()
    assert len(qs) >= 8
    facts = generate.load_facts("facts-spectroscopy")
    gaps = generate.load_facts("facts-gaps")  # band areas and windows (evals/gaps_facts.py)
    for q in qs.values():
        if q["source"].startswith("evals/facts/gaps.json ["):
            assert q["file"]["corpus_id"] in gaps or q["format"] == "share", q["id"]
            continue
        assert q["source"].startswith("evals/facts/spectroscopy.json ["), q["id"]
        assert q["file"]["corpus_id"] in facts, q["id"]
        assert "openreadout" not in q["source"].lower(), q["id"]


def test_wavenumber_answers_score():
    qs = spectro_questions()
    for qid, answer, ok in [
        ("ana-spec-opus-strongest-band", "2919.9 cm-1", True),
        ("ana-spec-opus-strongest-band", "2920 cm⁻¹", True),
        ("ana-spec-opus-strongest-band", "about 2918 wavenumbers", True),
        ("ana-spec-opus-strongest-band", "2850 cm-1", False),
        ("ana-spec-omnic-strongest-band", "2915.4 cm-1", True),
        ("ana-spec-wdf-strongest-band", "1086.7 cm-1 (calcite)", True),
        ("ana-spec-wdf-strongest-band", "282 cm-1", False),
        ("spec-wdf-laser-wavelength", "785 nm", True),
        ("spec-wdf-laser-wavelength", "532 nm", False),
        ("spec-wdf-map-spectra", "2205", True),
        ("spec-opus-resolution", "8 cm-1", True),
        ("spec-opus-resolution", "4 cm-1", False),
        ("spec-opus-apodization", "Blackman-Harris 3-term (B3)", True),
        ("spec-opus-apodization", "Happ-Genzel", False),
        ("spec-pesp-instrument", "PerkinElmer Frontier FT-IR/NIR", True),
    ]:
        got = score.score_answer(qs[qid]["answer"], f"ANSWER: {answer}")
        assert got["correct"] is ok, (qid, answer, got)


def test_jcamp_task_check(tmp_path):
    q = spectro_questions()["task-opus-jcamp"]
    want = q["task"]["expect"]
    n = int(want["points"])
    first, last = float(want["first_x"]), float(want["last_x"])
    step = (last - first) / (n - 1)
    ys = [0.1] * n
    ys[n // 3] = float(want["max_y"])
    lines = [
        "##TITLE= test",
        "##JCAMP-DX= 4.24",
        "##DATA TYPE= INFRARED SPECTRUM",
        "##XUNITS= 1/CM",
        "##YUNITS= ABSORBANCE",
        "##XFACTOR= 1",
        "##YFACTOR= 1",
        f"##FIRSTX= {first!r}",
        f"##LASTX= {last!r}",
        f"##NPOINTS= {n}",
        "##XYPOINTS= (XY..XY)",
    ]
    lines += [f"{first + i * step!r}, {y!r}" for i, y in enumerate(ys)]
    lines.append("##END=")
    good = tmp_path / "spectrum.jdx"
    good.write_text("\n".join(lines) + "\n")
    assert score.check_jcamp(good, want)["ok"]
    short = tmp_path / "short.jdx"
    short.write_text("\n".join([*lines[:-20], "##END="]) + "\n")
    assert not score.check_jcamp(short, want)["ok"]
    junk = tmp_path / "junk.jdx"
    junk.write_text("not a jcamp file")
    assert not score.check_jcamp(junk, want)["ok"]
