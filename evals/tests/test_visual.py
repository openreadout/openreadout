"""The visual tier (evals/visual.py): its scorers (box IoU, point in a mask, named choices), the
"did the agent look?" detection, and that its facts and questions are well formed and regenerate
from the corpus without drift (in the oracle venv)."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import facts
import generate
import run
import score
import visual


def ok(spec: dict, text: str) -> bool:
    return bool(score.score_answer(spec, f"ANSWER: {text}")["correct"])


# ---------------------------------------------------------------- bbox

BOX = generate.bbox([1000, 2000, 3000, 4000], min_iou=0.6)


@pytest.mark.parametrize(
    ("text", "want"),
    [
        ("x=1000, y=2000, width=3000, height=4000", True),
        ("x = 1,050, y = 1,980, width = 2,900, height = 4,100 px", True),
        ("**x: 1100, y: 2100, w: 2800, h: 3900**", True),
        ("1000, 2000, 3000, 4000", True),
        ("(1000, 2000, 3000, 4000)", True),
        ("x0=1000, y0=2000, x1=4000, y1=6000", True),  # corners
        ("1000, 2000, 4000, 6000", True),  # bare corners: the better reading counts
        ("left=1000, top=2000, right=4000, bottom=6000", True),
        ("x=0, y=0, width=10000, height=10000", False),  # the whole slide
        ("x=1000, y=2000, width=1000, height=1000", False),  # a corner of it
        ("x=5000, y=7000, width=3000, height=4000", False),  # somewhere else
        ("the tissue is in the middle", False),
    ],
)
def test_bbox_scoring(text, want):
    assert ok(BOX, text) is want, score.score_answer(BOX, f"ANSWER: {text}")


def test_bbox_reports_iou():
    got = score.score_answer(BOX, "ANSWER: x=1000, y=2000, width=3000, height=3000")
    assert got["iou"] == pytest.approx(0.75)


# ---------------------------------------------------------------- point

# a 3 × 2 mask at 16 full-resolution pixels per mask pixel: rows 10-11, columns 20-22
MASK = {"scale": [16, 16], "runs": [[10, 20, 23], [11, 20, 23]]}
POINT = generate.point([344, 176], mask=MASK)


@pytest.mark.parametrize(
    ("text", "want"),
    [
        ("x=344, y=176", True),
        ("(330, 180)", True),
        ("x = 367, y = 191 (full-resolution pixels)", True),  # last pixel of the mask
        ("x=368, y=176", False),  # just right of it
        ("x=344, y=192", False),  # just below it
        ("x=1,000, y=176", False),
        ("344 176", True),
        ("somewhere near the top", False),
    ],
)
def test_point_scoring(text, want):
    assert ok(POINT, text) is want, score.score_answer(POINT, f"ANSWER: {text}")


def test_point_radius_and_thousands():
    spec = generate.point([12000, 3400], radius=500)
    assert ok(spec, "x=12,300, y=3,500")
    assert not ok(spec, "x=12,600, y=3,500")


# ---------------------------------------------------------------- choice

FILES = visual.BEAD_STAGED
FOCUS = generate.choice(FILES[4:7], FILES, aliases={n: [n, n.rsplit(".", 1)[0]] for n in FILES})
LONG = [f"bead_bot4__560_00000_{i:05d}.dcimg" for i in range(11)]
SHORT_INSIDE_LONG = generate.choice(
    [LONG[5]], LONG, aliases={n: [n, n.rsplit(".", 1)[0].rsplit("_", 1)[1]] for n in LONG}
)
CORNERS = generate.choice(
    ["top-right", "bottom-left", "bottom-right"],
    list(visual.CORNERS),
    aliases=visual.CORNER_ALIASES,
    multiple=True,
)


@pytest.mark.parametrize(
    ("spec", "text", "want"),
    [
        (FOCUS, "sample_06.dcimg", True),
        (FOCUS, "`sample_05.dcimg` (z = -3.38 µm)", True),
        (FOCUS, "sample_07", True),
        (FOCUS, "sample_01.dcimg", False),
        (FOCUS, "sample_06.dcimg or sample_10.dcimg", False),  # hedging over a wrong file
        (FOCUS, "the middle one", False),
        # a short alias (`00000`) inside a longer matched name does not count as a second option
        (SHORT_INSIDE_LONG, "bead_bot4__560_00000_00005.dcimg", True),
        (SHORT_INSIDE_LONG, "the file ending in 00005", True),
        (SHORT_INSIDE_LONG, "bead_bot4__560_00000_00000.dcimg", False),
        (CORNERS, "top-right, bottom-left, bottom-right", True),
        (CORNERS, "Upper right, lower left and lower right corners", True),
        (CORNERS, "top right, bottom right", False),  # one missing
        (CORNERS, "top-left, top-right, bottom-left, bottom-right", False),  # every corner
        # an option set aside in a contrasting clause is named, not chosen
        (
            CORNERS,
            "top-right, bottom-left, and bottom-right corners are empty (unimaged); "
            "only the top-left corner was scanned",
            True,
        ),
        (CORNERS, "top-right, bottom-left and bottom-right, but not top-left", True),
        (CORNERS, "top-right; bottom-left; bottom-right", True),
        (CORNERS, "top-right or top-left, bottom-left, bottom-right", False),  # hedging
        (CORNERS, "the corners that are not imaged: top-right, bottom-left, bottom-right", True),
    ],
)
def test_choice_scoring(spec, text, want):
    assert ok(spec, text) is want, score.score_answer(spec, f"ANSWER: {text}")


def test_choice_rejects_unknown_values():
    with pytest.raises(ValueError):
        generate.choice("middle", ["left", "right"])


# ---------------------------------------------------------------- looking


def test_look_calls_and_images_seen():
    calls = [
        {"name": "mcp__openreadout__openreadout_preview", "input": {"path": "sample.svs"}},
        {"name": "Bash", "input": {"command": "openreadout preview sample.svs --region 0,0,512,512 -o z.png"}},
        {"name": "Bash", "input": {"command": "openreadout info sample.svs && python preview.py"}},
        {"name": "Read", "input": {"file_path": "/tmp/q/z.PNG"}},
        {"name": "Read", "input": {"file_path": "/tmp/q/notes.txt"}},
    ]
    assert run.look_calls(calls) == {"preview_mcp": 1, "preview_cli": 1, "image_read": 1}
    stream = [
        '{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"image","source":{}}]}]}}',
        '{"type":"user","message":{"content":[{"type":"tool_result","content":"text only"}]}}',
    ]
    assert run.parse_stream(stream)["images_seen"] == 1


def test_looked_from_old_and_new_records():
    assert score.looked({"images_seen": 2})
    assert score.looked({"look_calls": {"preview_mcp": 0, "preview_cli": 0, "image_read": 1}})
    assert score.looked({"tool_calls": {"mcp__openreadout__openreadout_preview": 1}})  # before the fields
    assert not score.looked({"tool_calls": {"Bash": 3}, "look_calls": {"preview_mcp": 0}, "images_seen": 0})


def test_summary_splits_accuracy_by_looking():
    qs = {q["id"]: q for q in score.load_questions().values() if q["category"] == "visual"}
    q = qs["vis-plate-blank-column"]
    recs = [
        {"id": q["id"], "final_text": "ANSWER: 11", "images_seen": 1},
        {"id": q["id"], "final_text": "ANSWER: 12"},
    ]
    s = score.summarize(recs, qs)
    assert s["looking"]["looked"] == {"n": 1, "correct": 1, "score": 1.0}
    assert s["looking_visual"]["did_not_look"]["correct"] == 0
    assert "correct when looking 1/1" in score.markdown_report({}, s)


# ---------------------------------------------------------------- facts and questions


def test_visual_facts_are_well_formed():
    data = generate.load_facts("facts-visual")
    manifest = generate.load_manifest()
    names = {(f.corpus_id, f.name) for f in visual.FACTS}
    assert names == {(cid, n) for cid, e in data.items() for n in e["facts"]}
    for cid, entry in data.items():
        assert cid in manifest, cid
        assert entry["extractor"] == "evals/visual.py" and entry["file"], cid
        for name, f in entry["facts"].items():
            assert set(f) == {"value", "reader", "how", "evidence"}, (cid, name)
            assert f["value"] not in (None, "", [], {}), (cid, name)
            assert "openreadout" not in f["reader"].lower() and "openreadout" not in f["how"].lower()
            assert f["reader"].split()[0] in (
                "tifffile",
                "czifile",
                "dcimg",
                "nd2",
                "pyabf",
                "rainbow-api",
                "allotropy",
            ), f["reader"]
            assert f["evidence"], (cid, name)  # every answer records its robustness margin


def test_visual_questions_are_well_formed():
    qs = [q for q in score.load_questions().values() if q["category"] == "visual"]
    assert 12 <= len(qs) <= 40
    assert {q["subcategory"] for q in qs} <= {"locate", "focus", "blank", "count", "artifact"}
    assert len({q["family"] for q in qs}) >= 4
    wsi = {"openslide-aperio-cmu-1", "openslide-hamamatsu-cmu-1", "ome-qptiff-hande-compressed-scan1"}
    assert len(wsi & {q["file"]["corpus_id"] for q in qs}) >= 2  # overview → zoom on pyramids
    for q in qs:
        assert q["id"].startswith("vis-"), q["id"]
        assert q["source"].startswith("evals/facts/visual.json ["), q["id"]
        assert q["split"] in ("dev", "test"), q["id"]
        name = q["source"].split("facts-visual: ", 1)[1].split(" ", 1)[0]
        assert name in generate.load_facts("facts-visual")[q["file"]["corpus_id"]]["facts"], q["id"]
        # the question must not say how to compute the answer
        for word in ("otsu", "laplacian", "threshold", "welch", "find_peaks", "iou"):
            assert word not in q["question"].lower(), (q["id"], word)
        a = q["answer"]
        if a["type"] == "point":
            assert a.get("mask") or a.get("radius"), q["id"]
            assert score.in_mask(a["mask"], *a["value"]), q["id"]
        if a["type"] == "bbox":
            assert 0.5 <= a["min_iou"] < 1, q["id"]


def reference_text(a: dict) -> str:
    if a["type"] == "bbox":
        return "x={}, y={}, width={}, height={}".format(*a["value"])
    if a["type"] == "point":
        return "x={}, y={}".format(*a["value"])
    if a["type"] == "choice":
        return ", ".join(a["value"]) if a["multiple"] else a["value"][0]
    return f"{a['value']} {a.get('unit') or ''}".strip()


def test_visual_reference_answers_score():
    for q in score.load_questions().values():
        if q["category"] == "visual":
            assert ok(q["answer"], reference_text(q["answer"])), q["id"]


def test_visual_facts_match_the_corpus_files():
    """Recompute when the oracle readers and the corpus are present (the oracle venv); skipped otherwise.
    Slow (whole-slide pyramid levels): `oracle/.venv/bin/python -m pytest evals/tests -k visual`."""
    for mod in ("czifile", "nd2", "dcimg", "pyabf", "rainbow", "allotropy", "scipy"):
        if importlib.util.find_spec(mod) is None:
            pytest.skip("oracle readers not installed (run with oracle/.venv/bin/python -m pytest)")
    missing = [
        f.corpus_id for f in visual.FACTS if not (facts.corpus_dir() / visual.manifest_file(f.corpus_id)).exists()
    ]
    if missing:
        pytest.skip(f"corpus files not present: {', '.join(missing[:3])}")
    assert visual.render(visual.compute()) == visual.OUT.read_text(encoding="utf-8")
