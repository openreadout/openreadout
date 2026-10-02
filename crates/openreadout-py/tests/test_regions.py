"""Regions, pyramid levels, tile-chunked dask arrays, zero-copy buffers, parallel reads and
pyramidal exports, checked against third-party ground truth: the region oracles
(``corpus/oracle/regions/*.json``: czifile, tifffile + zarr, h5py, zarr-python, nd2), tifffile and
zarr-python reading our exports, and Bio-Formats (``showinf``, run as a black box) when present."""

from __future__ import annotations

import json
import os
import subprocess
import threading
from pathlib import Path
from typing import Any, Callable, Dict, List

import openreadout
import numpy as np
import pytest
from openreadout import File, UsageError

Corpus = Callable[[str], Path]
REPO = Path(__file__).resolve().parents[3]
REGIONS = REPO / "corpus" / "oracle" / "regions"

SVS = "openslide-aperio-CMU-1-Small-Region.svs"  # RGB, JPEG tiles 240 x 240, one level
SVS_BIG = "openslide-aperio-CMU-1.svs"  # 46000 x 32914 RGB (4.5 GB decoded), 3 levels
KIDNEY = "zenodo10577621-Kidney-RAC-3color.czi"  # 10128 x 5718 x 4 ch uint16 mosaic, 5 levels
CZI = "aics-s-3-t-1-c-3-z-5.czi"
IMS = "ome-imaris/retina_large.ims"
PYR_TIFF = "ome-subresolutions-retina_large.ome.tiff"


def _xxh3(a: np.ndarray) -> str:
    xxhash = pytest.importorskip("xxhash")
    le = np.ascontiguousarray(a).astype(a.dtype.newbyteorder("<"), copy=False)
    return str(xxhash.xxh3_128_hexdigest(le.tobytes()))


def _oracles() -> List[Dict[str, Any]]:
    out = []
    for p in sorted(REGIONS.glob("*.json")):
        d = json.loads(p.read_text())
        if not d["lossy"]:
            out.append(d)
    return out


@pytest.mark.parametrize("oracle", _oracles(), ids=lambda d: d["id"])
def test_regions_match_third_party_readers(corpus: Corpus, oracle: Dict[str, Any]) -> None:
    """Every lossless region of the oracle files, read through Python, is bit-exact."""
    path = corpus(oracle["file"])
    checked = 0
    with File(path) as f:
        for r in oracle["regions"]:
            if r["level_size"][0] * r["level_size"][1] > 50_000_000 and checked >= 6:
                continue  # keep the test quick on whole slides
            got = f.read_plane(
                r["image"],
                c=r["c"],
                z=r["z"],
                t=r["t"],
                level=r["level"],
                region=tuple(r["region"]),
            )
            assert got.shape[:2] == (r["region"][3], r["region"][2])
            assert _xxh3(got) == r["xxh3"], (oracle["id"], r["level"], r["region"])
            checked += 1
    assert checked > 0


def test_region_equals_crop_of_the_plane(corpus: Corpus) -> None:
    with File(corpus(CZI)) as f:
        whole = f.read_plane(2, c=1, z=3)
        part = f.read_plane(2, c=1, z=3, region=(17, 29, 101, 57))
        np.testing.assert_array_equal(part, whole[29:86, 17:118])
        with pytest.raises(UsageError):
            f.read_plane(2, region=(400, 0, 100, 10))  # past the 475-pixel width
        with pytest.raises(UsageError):
            f.read_plane(2, region=(0, 0, 0, 10))
        with pytest.raises(UsageError):
            f.read_plane(2, level=1)  # no pyramid: level 1 does not exist


def test_levels_shapes_and_level_reads(corpus: Corpus) -> None:
    with File(corpus(KIDNEY)) as f:
        levels = f.levels(0)
        assert [lv["level"] for lv in levels] == [0, 1, 2, 3, 4]
        assert (levels[0]["size_x"], levels[0]["size_y"]) == (10128, 5718)
        assert levels[2]["downsample_x"] == pytest.approx(4.0)
        assert f.shape(0, level=4) == (1, 4, 1, levels[4]["size_y"], levels[4]["size_x"])
        small = f.read_image(0, level=4)
        assert small.shape == f.shape(0, level=4)
        # the committed czifile oracle of level 4
        truth = json.loads(
            (REPO / "corpus/oracle/zenodo10577621-Kidney-RAC-3color.json").read_text()
        )
        lv = next(x for x in truth["images"][0]["levels"] if x["level"] == 4)
        for p in lv["planes"]:
            assert _xxh3(small[p["t"], p["c"], p["z"]]) == p["xxh3"]


def test_whole_slide_level0_by_region_and_tiled_dask(corpus: Corpus) -> None:
    """A 4.5 GB level 0 cannot be read whole; windows and tile-aligned dask chunks can."""
    pytest.importorskip("dask.array")
    with File(corpus(SVS_BIG)) as f:
        with pytest.raises(openreadout.UnsupportedFeatureError):
            f.read_plane(0)
        lv0 = f.levels(0)[0]
        assert (lv0["tile_width"], lv0["tile_height"]) == (256, 256)
        lazy = f.to_dask(0)
        assert lazy.shape == (1, 1, 1, 32914, 46000, 3)
        cy, cx = lazy.chunksize[3:5]
        assert cy % 256 == 0 and cx % 256 == 0 and cy * cx * 3 <= 16 << 20
        x, y = (46000 - 512) // 2, (32914 - 512) // 2
        win = lazy[0, 0, 0, y : y + 512, x : x + 512].compute()
        np.testing.assert_array_equal(win, f.read_plane(0, region=(x, y, 512, 512)))
        oracle = json.loads((REGIONS / "openslide-aperio-cmu-1.json").read_text())
        r = next(
            r for r in oracle["regions"] if r["level"] == 0 and r["region"] == [x, y, 512, 512]
        )
        assert abs(float(win.mean()) - r["mean"]) < 0.05  # JPEG: any decoder within a fraction


def test_xarray_level_coordinates(corpus: Corpus) -> None:
    pytest.importorskip("xarray")
    with File(corpus(KIDNEY)) as f:
        px = f.images[0]["physical_size"]["x"]
        x0 = f.to_xarray(0)
        x2 = f.to_xarray(0, level=2)
        assert x2.sizes["X"] == f.levels(0)[2]["size_x"]
        f2 = f.levels(0)[2]["downsample_x"]
        # pixel centres of level 2 in the level-0 frame
        assert float(x2.X[0]) == pytest.approx((0.5 * f2 - 0.5) * px)
        assert float(x2.X[1] - x2.X[0]) == pytest.approx(f2 * px)
        assert float(x0.X[1] - x0.X[0]) == pytest.approx(px)
        assert x2.attrs["openreadout"]["level"] == 2


def test_imaris_levels_have_their_own_z(corpus: Corpus) -> None:
    with File(corpus(IMS)) as f:
        levels = f.levels(0)
        assert levels[-1].get("size_z") == 32
        assert f.shape(0, level=len(levels) - 1)[2] == 32


def test_buffers_are_handed_over_without_copies(corpus: Corpus) -> None:
    with File(corpus(CZI)) as f:
        a = f.read_plane(0)
        b = f.read_plane(0)
        assert a.flags.writeable and a.flags.c_contiguous and not np.shares_memory(a, b)
        # The view's base is the decoded buffer itself (a uint8 array owning Rust memory).
        assert a.base is not None and a.base.dtype == np.uint8 and a.base.nbytes == a.nbytes


def test_parallel_reads_of_one_file_use_several_handles(corpus: Corpus) -> None:
    with File(corpus(KIDNEY)) as f:
        want = [f.read_plane(0, c=c, level=2) for c in range(4)]
        got: Dict[int, np.ndarray] = {}
        errors: List[BaseException] = []

        def work(c: int) -> None:
            try:
                for _ in range(3):
                    got[c] = f.read_plane(0, c=c, level=2)
            except BaseException as e:  # pragma: no cover - reported below
                errors.append(e)

        threads = [threading.Thread(target=work, args=(c,)) for c in range(4)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        assert not errors
        for c in range(4):
            np.testing.assert_array_equal(got[c], want[c])
        assert 1 <= f._n.handle_count <= 4
    assert f.closed


def test_to_pandas(corpus: Corpus) -> None:
    pytest.importorskip("pyarrow")
    pytest.importorskip("pandas")
    path = corpus("flowio-m0-wm278-s1-zero-gain.fcs")
    with File(path) as f:
        df = f.to_pandas()
        assert len(df) == f.tables[0]["row_count"]
        assert list(df.columns) == [c["name"] for c in f.tables[0]["columns"]]


def test_pyramidal_ome_tiff_export_reads_in_tifffile(corpus: Corpus, tmp_path: Path) -> None:
    tifffile = pytest.importorskip("tifffile")
    out = tmp_path / "kidney.ome.tiff"
    with File(corpus(KIDNEY)) as f:
        report = f.export(out, select=["c=0,3"], pyramid="source")
        assert report["verified"]
        sizes = report["resolutions"][0]["sizes"]
        assert len(sizes) == 5
        lv3 = f.read_plane(0, c=3, level=3)
        lv0_win = f.read_plane(0, c=0, region=(4000, 2000, 700, 300))
    with tifffile.TiffFile(out) as tf:
        s = tf.series[0]
        assert tf.is_ome and len(s.levels) == 5
        assert [lvl.shape[-1] for lvl in s.levels] == [w for w, _ in sizes]
        np.testing.assert_array_equal(s.levels[3].asarray()[1], lv3)
        np.testing.assert_array_equal(s.levels[0].asarray()[0, 2000:2300, 4000:4700], lv0_win)


def test_ome_zarr_export_of_a_region_reads_in_zarr_python(corpus: Corpus, tmp_path: Path) -> None:
    zarr = pytest.importorskip("zarr")
    out = tmp_path / "win.ome.zarr"
    with File(corpus(SVS_BIG)) as f:
        region = (20000, 15000, 3000, 2000)
        report = f.export(out, to="ome-zarr", region=region, pyramid="mean", levels=3)
        assert report["verified"]
        want = f.read_plane(0, region=region)
    g = zarr.open_group(str(out), mode="r")
    ms = dict(g.attrs)["ome"]["multiscales"][0]
    assert [d["path"] for d in ms["datasets"]] == ["0", "1", "2"]
    a0 = np.asarray(g["0"][0, :, 0])  # (c = R, G, B, y, x)
    np.testing.assert_array_equal(np.moveaxis(a0, 0, -1), want)
    assert g["2"].shape[-2:] == (500, 750)


def _showinf() -> str | None:
    env = os.environ.get("BFTOOLS_DIR")
    cands = [Path(env)] if env else []
    cands.append(REPO / "oracle" / "bftools" / "bftools")
    for parent in REPO.parents:
        if parent.name == "worktrees" and parent.parent.name == ".claude":
            cands.append(parent.parent.parent / "oracle" / "bftools" / "bftools")
    for c in cands:
        if (c / "showinf").exists():
            return str(c / "showinf")
    return None


def test_bioformats_sees_the_subifd_pyramid(corpus: Corpus, tmp_path: Path) -> None:
    showinf = _showinf()
    if showinf is None:
        pytest.skip("Bio-Formats bftools not installed")
    out = tmp_path / "p.ome.tiff"
    with File(corpus(PYR_TIFF)) as f:
        f.export(out, image=0, select=["c=0", "z=0-1"], pyramid="source")
    r = subprocess.run(
        [showinf, "-nopix", "-noflat", "-no-upgrade", str(out)],
        capture_output=True,
        text=True,
        timeout=600,
    )
    assert "Resolutions = 3" in r.stdout, r.stdout[-2000:]
