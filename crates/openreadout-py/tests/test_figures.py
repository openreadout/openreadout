"""Pictures from Python: previews with annotations, figures from plot specs, and the
measurements on one plane (line, polygon, objects)."""

from __future__ import annotations

import json

import numpy as np
import openreadout
import pytest

CZI = "zenodo7015307-S-2-T-3-Z-5-CH-1.czi"
MZML = "mtbls20-caffeine-pos.mzML"
ABF = "gh-swharden-pyabf-17o05028-ic-steps.abf"
PLATE = "softmax5-kinetic-phage-120308b.pda"

PNG = b"\x89PNG\r\n\x1a\n"


def test_preview_with_annotations_reports_where_they_landed(corpus):
    with openreadout.open(corpus(CZI)) as f:
        p = f.preview(
            max_size=512,
            annotations=[
                {"type": "point", "x": 20, "y": 30, "label": "here"},
                {"type": "box", "x": 5, "y": 5, "width": 40, "height": 20},
            ],
        )
    assert p.data.startswith(PNG)
    assert p.format == "png" and max(p.width, p.height) <= 512
    assert [a["type"] for a in p.annotations] == ["point", "box"]
    assert p._repr_png_() == p.data and p._repr_svg_() is None
    assert "Preview image" in repr(p)


def test_preview_svg_and_dark(corpus):
    p = openreadout.preview(corpus(MZML), format="svg", dark=True, max_size=600)
    assert p.format == "svg"
    svg = p._repr_svg_()
    assert svg is not None and svg.lstrip().startswith("<svg")
    assert p._repr_png_() is None


def test_preview_marks_spikes_on_a_current_clamp_sweep(corpus):
    with openreadout.open(corpus(ABF)) as f:
        p = f.preview(sweep=10, marks="spikes")
    assert p.meta["kind"] == "trace"
    assert p.meta.get("marks"), p.meta.get("notes")


def test_preview_options_are_checked(corpus):
    with openreadout.open(corpus(CZI)) as f:
        with pytest.raises(openreadout.UsageError, match="colour"):
            f.preview(colour="red")
        with pytest.raises(openreadout.UsageError):
            f.preview(format="gif")


def test_preview_save_is_verified(corpus, tmp_path):
    p = openreadout.preview(corpus(CZI), max_size=256)
    out = p.save(tmp_path / "plane.png")
    assert open(out, "rb").read() == p.data
    with pytest.raises(openreadout.UsageError, match="exists"):
        p.save(out)
    p.save(out, overwrite=True)


def test_plot_tic_renders_every_format(corpus, tmp_path):
    fig = openreadout.plot(
        data={"file": corpus(MZML), "chromatogram": "tic"}, title="Total ion current"
    )
    assert isinstance(fig, openreadout.Figure)
    w, h = fig.size
    assert w == 1200 and h > 200
    assert fig.summary["title"] == "Total ion current"
    assert fig.summary["panels"][0]["series"]
    assert fig.png().startswith(PNG)
    big, meta = fig._repr_png_()
    assert big.startswith(PNG) and meta == {"width": w, "height": h}
    assert fig.svg().lstrip().startswith("<svg")
    assert fig.pdf().startswith(b"%PDF")
    for ext in ("png", "svg", "pdf"):
        out = fig.save(tmp_path / f"tic.{ext}")
        assert (tmp_path / f"tic.{ext}").stat().st_size > 0, out
    with pytest.raises(openreadout.UsageError):
        fig.save(tmp_path / "tic.gif")


def test_plot_spec_forms_agree(corpus, tmp_path):
    spec = {"data": {"file": str(corpus(MZML)), "chromatogram": "tic"}, "width": 600}
    a = openreadout.plot(spec)
    b = openreadout.plot(json.dumps(spec))
    path = tmp_path / "spec.json"
    path.write_text(json.dumps(spec))
    c = openreadout.plot(path)
    d = openreadout.plot(**spec)
    assert a.png() == b.png() == c.png() == d.png()


def test_plot_plate_growth_in_plate_positions(corpus):
    fig = openreadout.plot(
        data={"file": corpus(PLATE), "wells": "A1:H12"}, facet="plate", theme="dark"
    )
    assert fig.summary["theme"] == "dark"
    assert fig.summary["panels"], fig.summary


def test_plot_errors_name_the_choices():
    with pytest.raises(openreadout.UsageError, match="known"):
        openreadout.plot(data={"values": [1, 2, 3]}, colour="red")
    with pytest.raises(openreadout.UsageError, match="nothing to plot"):
        openreadout.plot({})


def test_stats_line_polygon_objects(corpus):
    with openreadout.open(corpus(CZI)) as f:
        plane = f.read_plane(0, c=0, z=2, t=0).astype(np.float64)
        h, w = plane.shape[:2]
        prof = f.stats(select=["z=2"], line=(0, h // 2, w - 1, h // 2))["profile"]
        ch = prof["channels"][0]
        # samples between pixel centres are interpolated, so they stay within the two rows
        rows = plane[h // 2 - 1 : h // 2 + 1, :]
        assert rows.min() <= ch["min"] <= ch["mean"] <= ch["max"] <= rows.max()
        assert prof["samples"] >= w - 1 and len(ch["values"]) > 0
        roi = f.stats(select=["z=2"], polygon=[(0, 0), (w, 0), (w, h), (0, h)])["roi"]
        assert roi["area_px"] == pytest.approx(w * h, rel=0.01)
        assert roi["channels"][0]["mean"] == pytest.approx(plane.mean(), rel=0.01)
        obj = f.stats(select=["z=2"], objects={"min_area_px": 4})["objects"]
        assert obj["count"] >= 0 and "threshold" in obj
        assert (
            f.stats(select=["z=2"], objects=True)["objects"]["count"]
            == f.stats(select=["z=2"], objects={})["objects"]["count"]
        )
        with pytest.raises(openreadout.UsageError):
            f.stats(line=(0, 0, 1))
        with pytest.raises(openreadout.UsageError):
            f.stats(polygon=[(0, 0), (1, 1)])
        with pytest.raises(openreadout.UsageError):
            f.stats(line=(0, 0, 5, 5), per="well")


def test_growth_patterns_and_summary(corpus):
    r = openreadout.analyze(corpus(PLATE), "assay", analysis="growth")
    rows = r["growth"]
    assert rows and all(
        row["pattern"] in ("grew", "no_growth", "crashed", "regrew") for row in rows
    )
    summary = r["growth_summary"]["wells"]
    assert sum(len(v) for v in summary.values()) == len(rows)
    for row in rows:
        if row["pattern"] in ("crashed", "regrew"):
            assert row["crash_s"] < row["trough_s"]
        if row["pattern"] == "regrew":
            assert row["regrowth_s"] >= row["trough_s"]
