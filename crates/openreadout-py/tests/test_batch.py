"""Batch tables, summaries and links from Python (``openreadout.batch``)."""

from __future__ import annotations

import json
import math
import shutil
from pathlib import Path

import pytest

import openreadout as ic

REPO = Path(__file__).resolve().parents[3]

FCS = ["fcsparser-fortessa-a01.fcs", "fcsparser-hts-lsr-ii-d06.fcs", "fcsparser-facs-diva.fcs"]


def test_table_batch_with_sample_sheet_and_summary(corpus, tmp_path):
    files = [corpus(n) for n in FCS]
    for f in files:
        shutil.copy(f, tmp_path / f.name)
    (tmp_path / "samples.csv").write_text(
        "file,condition\n"
        "fcsparser-fortessa-a01.fcs,control\n"
        "fcsparser-hts-lsr-ii-d06,treated\n"
        "fcsparser-facs-diva.fcs,treated\n"
    )
    res = ic.batch(
        "table",
        [tmp_path],
        sample_sheets=[tmp_path / "samples.csv"],
        where=["parameter=FITC-A"],
        by=["condition"],
        values=["median"],
    )
    assert res.inputs["datasets"] == 3
    assert res.inputs["failed"] == 0
    assert res.joins[0]["key"][0]["field"] == "stem"
    df = res.table
    assert len(df) == 3
    assert set(df["condition"]) == {"control", "treated"}
    assert str(df["median"].dtype) == "float64"
    # FlowIO ground truth (corpus/oracle/batch/fcs-summaries.json)
    truth = json.loads((REPO / "corpus/oracle/batch/fcs-summaries.json").read_text())
    med = {
        f["id"]: p["median"] for f in truth["files"] for p in f["parameters"] if p["parameter"] == "FITC-A"
    }
    treated = [med["fcsparser-hts-lsr-ii-d06"], med["fcsparser-facs-diva"]]
    s = res.summary
    row = s[s["condition"] == "treated"].iloc[0]
    assert row["n"] == 2
    assert math.isclose(row["mean"], sum(treated) / 2, rel_tol=1e-9)
    t = res.to_arrow()
    assert t.num_rows == 3


def test_summarize_and_link(tmp_path):
    src = REPO / "corpus/oracle/batch/summarize-input.csv"
    df = ic.batch("summarize", src, by=["condition"], values=["mean"], test="welch", control="ctrl")
    assert list(df.columns[:3]) == ["condition", "channel_name", "value"]
    assert len(df) == 6
    assert df.attrs["openreadout"]["rows_excluded"] == 1
    with pytest.raises(ValueError):
        ic.batch("summarize", src, by=["nope"])
    out = ic.link(tmp_path)
    assert out["groups"] == []
