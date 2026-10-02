"""napari-openreadout without a GUI: the npe2 manifest validates and routes files to the
reader, and the reader returns lazy (multiscale) layer data whose pixels equal the oracles'."""

from __future__ import annotations

import itertools
import json
from importlib.metadata import entry_points
from pathlib import Path
from typing import Callable

import napari_openreadout
import numpy as np
import pytest
from napari_openreadout import get_reader, read

Corpus = Callable[[str], Path]
REPO = Path(__file__).resolve().parents[3]

CZI = "aics-s-3-t-1-c-3-z-5.czi"  # 3 scenes, C=3, Z=5
SVS = "openslide-aperio-CMU-1-Small-Region.svs"  # RGB, one level
PYRAMID_CZI = "zenodo10577621-Kidney-RAC-3color.czi"  # 5 levels, C=4
ND2_RGB = "aics-ND2-dims-rgb.nd2"


def test_manifest_validates_and_is_registered() -> None:
    npe2 = pytest.importorskip("npe2")
    eps = {ep.name: ep for ep in entry_points(group="napari.manifest")}
    assert "napari-openreadout" in eps
    mf = npe2.PluginManifest.from_distribution("napari-openreadout")
    assert mf.name == "napari-openreadout"
    patterns = mf.contributions.readers[0].filename_patterns
    for ext in ("*.czi", "*.nd2", "*.lif", "*.svs", "*.vsi", "*.ims", "*.zarr"):
        assert ext in patterns


def test_npe2_routes_files_to_the_reader(corpus: Corpus) -> None:
    npe2 = pytest.importorskip("npe2")
    path = str(corpus(CZI))
    pm = npe2.PluginManager.instance()
    pm.discover()
    readers = [r.plugin_name for r in pm.iter_compatible_readers([path])]
    assert "napari-openreadout" in readers
    layers = npe2.read([path], plugin_name="napari-openreadout", stack=False)
    assert len(layers) == 3  # one per scene


def test_unknown_files_are_declined(tmp_path: Path) -> None:
    junk = tmp_path / "notes.czi"
    junk.write_bytes(b"not an image at all" * 10)
    assert get_reader(str(junk)) is None
    assert get_reader([str(junk), str(junk)]) is None


def test_channels_become_named_layers_with_scale(corpus: Corpus) -> None:
    path = corpus(CZI)
    reader = get_reader(str(path))
    assert reader is read
    layers = read(str(path))
    assert len(layers) == 3
    data, kw, kind = layers[1]
    assert kind == "image" and kw["channel_axis"] == 1 and not kw["multiscale"]
    assert len(kw["name"]) == 3 and kw["name"][0].endswith("EGFP")
    assert len(kw["scale"]) == 4 and kw["scale"][2] == pytest.approx(kw["scale"][3])
    assert data.shape == (1, 3, 5, 325, 475)
    # Lazy: nothing decoded until computed; values equal the File's own read.
    import openreadout

    with openreadout.File(path) as f:
        np.testing.assert_array_equal(np.asarray(data[0, 2, 4]), f.read_plane(1, c=2, z=4))
    assert kw["metadata"]["openreadout"]["format"] == "czi"


def test_rgb_single_channel(corpus: Corpus) -> None:
    data, kw, _ = read(str(corpus(SVS)))[0]
    assert kw["rgb"] and "channel_axis" not in kw
    assert data.shape[-1] == 3 and len(kw["scale"]) == 5


def test_pyramid_is_multiscale_and_matches_levels(corpus: Corpus) -> None:
    data, kw, _ = read(str(corpus(PYRAMID_CZI)))[0]
    assert kw["multiscale"] and isinstance(data, list) and len(data) == 5
    shapes = [d.shape[-2:] for d in data]
    assert shapes[0] == (5718, 10128)
    assert all(a[0] > b[0] for a, b in itertools.pairwise(shapes))
    # A coarse level equals the committed czifile oracle for that level.
    oracle = json.loads(
        (REPO / "corpus" / "oracle" / "zenodo10577621-Kidney-RAC-3color.json").read_text()
    )
    lv = next(x for x in oracle["images"][0]["levels"] if x["level"] == 4 and x["planes"])
    xxhash = pytest.importorskip("xxhash")
    for p in lv["planes"][:2]:
        plane = np.ascontiguousarray(np.asarray(data[4][0, p["c"], p["z"]]))
        assert xxhash.xxh3_128_hexdigest(plane.tobytes()) == p["xxh3"]


def test_version() -> None:
    assert napari_openreadout.__version__
