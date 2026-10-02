"""`File.stats()` against NumPy on the planes the same package reads, and against the CLI's JSON."""

from __future__ import annotations

import json
import os
import shutil
import subprocess

import numpy as np
import openreadout
import pytest

FILE = "zenodo7015307-S-2-T-3-Z-5-CH-1.czi"  # 2 scenes x 3 time points x 5 z planes


def _planes(f, image):
    im = f.images[image]
    return {
        (c, z, t): f.read_plane(image, c=c, z=z, t=t).astype(np.float64)
        for c in range(im["size_c"])
        for z in range(im["size_z"])
        for t in range(im["size_t"])
    }


def test_stats_match_numpy_and_carry_names(corpus):
    with openreadout.open(corpus(FILE)) as f:
        s = f.stats()
        assert len(s["images"]) == len(f.images) >= 2
        for img in s["images"]:
            i = img["image"]
            assert img.get("name") == f.images[i].get("name")
            allv = np.concatenate([p.ravel() for p in _planes(f, i).values()])
            assert img["stats"]["count"] == allv.size
            assert img["stats"]["mean"] == pytest.approx(allv.mean(), rel=1e-12)
            assert img["stats"]["max"] == allv.max()
            assert img["stats"]["percentiles"]["p50"] == pytest.approx(np.percentile(allv, 50))
        for ch in s["channels"]:
            assert ch.get("image_name") == f.images[ch["image"]].get("name")


def test_stats_mip_matches_numpy(corpus):
    with openreadout.open(corpus(FILE)) as f:
        s = f.stats(image=0, select=["c=0"], mip="z", per_plane=True)
        assert s["mip"] == "z"
        planes = _planes(f, 0)
        size_z = f.images[0]["size_z"]
        for p in s["planes"]:
            proj = np.max([planes[(0, z, p["t"])] for z in range(size_z)], axis=0)
            assert p["stats"]["mean"] == pytest.approx(proj.mean(), rel=1e-12)
            assert p["stats"]["max"] == proj.max()
        with pytest.raises(openreadout.UsageError):
            f.stats(mip="x")


@pytest.mark.skipif(
    not (os.environ.get("OPENREADOUT_BIN") or shutil.which("openreadout")),
    reason="no openreadout binary (set OPENREADOUT_BIN)",
)
def test_stats_equal_the_cli_json(corpus):
    exe = os.environ.get("OPENREADOUT_BIN") or shutil.which("openreadout")
    path = corpus(FILE)
    out = subprocess.run(
        [exe, "stats", str(path), "--json", "--bins", "0"], capture_output=True, check=True
    ).stdout
    cli = json.loads(out)["data"]
    with openreadout.open(path) as f:
        py = f.stats()
    assert py["images"] == cli["images"]
    assert py["channels"] == cli["channels"]
    assert py["planes"] == cli["planes"]
