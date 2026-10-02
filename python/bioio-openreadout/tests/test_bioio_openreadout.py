"""bioio-openreadout against bioio and permissively licensed oracle readers.

``img.data`` is compared with bioio-nd2 (ND2), czifile (CZI) and liffile (LIF) where those are
installed; each comparison skips when its reader or corpus file is absent.
"""

from __future__ import annotations

from datetime import timedelta
from importlib.metadata import entry_points, requires
from pathlib import Path
from typing import Any, Callable, List

import numpy as np
import pytest

bioio = pytest.importorskip("bioio")
import bioio_openreadout  # noqa: E402
from bioio import BioImage  # noqa: E402
from bioio_base import exceptions  # noqa: E402
from bioio_openreadout import Reader  # noqa: E402

Corpus = Callable[[str], Path]

CZI = "aics-s-3-t-1-c-3-z-5.czi"
CZI_MOSAIC = "zenodo7015307-S-2-2x2-T-3-CH-1.czi"
ND2_RGB_MULTI = "aics-ND2-dims-rgb-t3p2c2z3x64y64.nd2"
ND2_MULTIPOS = "aics-ND2-dims-p4z5t3c2y32x32.nd2"
LIF = "aics-s-1-t-4-c-2-z-1.lif"

PLUGIN = "bioio-openreadout"


def _to_tczyxs(dims: List[str], arr: np.ndarray) -> np.ndarray:
    """Reorder an oracle array to T, C, Z, Y, X, S (adding size-1 axes, dropping size-1 extras)."""
    for i in reversed(range(len(dims))):
        if dims[i] not in "TCZYXS":
            assert arr.shape[i] == 1, (dims, arr.shape)
            arr = arr.take(0, axis=i)
            dims = dims[:i] + dims[i + 1 :]
    for d in "TCZYXS":
        if d not in dims:
            arr = arr[np.newaxis]
            dims = [d, *dims]
    return arr.transpose([dims.index(d) for d in "TCZYXS"])


def _ours_tczyxs(img: BioImage) -> np.ndarray:
    data = img.data
    return data if data.ndim == 6 else data[..., np.newaxis]


# ----- packaging / discovery ---------------------------------------------------------------------


def test_entry_point_is_registered() -> None:
    eps = {ep.name: ep for ep in entry_points(group="bioio.readers")}
    assert PLUGIN in eps and eps[PLUGIN].value == "bioio_openreadout"
    assert eps[PLUGIN].load().ReaderMetadata.get_reader() is Reader


def test_no_copyleft_dependencies() -> None:
    reqs = " ".join(requires(PLUGIN) or []).lower()
    for gpl in ("bioio-czi", "bioio-lif", "pylibczirw", "readlif", "bioformats"):
        assert gpl not in reqs


def test_plugin_is_offered_for_each_extension() -> None:
    from bioio.plugins import get_plugins

    plugins = get_plugins(use_cache=False)
    for ext in (".czi", ".nd2", ".lif"):
        assert PLUGIN in [p.entrypoint.name for p in plugins[ext]], ext


@pytest.mark.parametrize("name", [CZI, ND2_MULTIPOS, LIF])
def test_bioimage_picks_plugin_by_extension(corpus: Corpus, monkeypatch: Any, name: str) -> None:
    """With bioio-openreadout the only plugin for the extension, BioImage(path) selects it."""
    import bioio.bio_image
    from bioio.plugins import get_plugins

    path = corpus(name)
    ours_only = {
        ext: [p for p in plugins if p.entrypoint.name == PLUGIN]
        for ext, plugins in get_plugins(use_cache=False).items()
    }
    monkeypatch.setattr(bioio.bio_image, "get_plugins", lambda use_cache=False: ours_only)
    img = BioImage(path)
    assert isinstance(img.reader, Reader)
    assert img.reader.name == PLUGIN


@pytest.mark.parametrize("name", [CZI, ND2_MULTIPOS, LIF])
def test_bioimage_plugin_order_in_this_environment(corpus: Corpus, name: str) -> None:
    """Without overrides, bioio picks us unless a more specific plugin for the extension exists."""
    from bioio.plugins import get_plugins

    path = corpus(name)
    ext = "." + name.rsplit(".", 1)[1]
    candidates = [p.entrypoint.name for p in get_plugins(use_cache=False)[ext]]
    chosen = BioImage.determine_plugin(path).entrypoint.name
    assert chosen == candidates[0] or chosen == PLUGIN
    if candidates == [PLUGIN]:
        assert isinstance(BioImage(path).reader, Reader)


def test_explicit_reader(corpus: Corpus) -> None:
    img = BioImage(corpus(CZI), reader=Reader)
    assert isinstance(img.reader, Reader)
    assert img.scenes == ("P2", "P3", "P1")
    assert img.dims.order == "TCZYX" and img.shape == (1, 3, 5, 325, 475)
    assert img.channel_names == ["EGFP", "TaRFP", "Bright"]
    ps = img.physical_pixel_sizes
    assert ps.X == pytest.approx(1.0833333) and ps.Y == pytest.approx(1.0833333) and ps.Z == 1.0
    assert img.dtype == np.uint16


def test_unsupported_file_is_rejected(tmp_path: Path) -> None:
    junk = tmp_path / "junk.czi"
    junk.write_bytes(b"\0" * 4096)
    with pytest.raises(exceptions.UnsupportedFileFormatError):
        Reader(junk)
    with pytest.raises(exceptions.UnsupportedFileFormatError):
        Reader.is_supported_image(junk)
    with pytest.raises(FileNotFoundError):
        Reader(tmp_path / "missing.czi")


# ----- data equality against oracles --------------------------------------------------------------


@pytest.mark.parametrize("name", [ND2_RGB_MULTI, ND2_MULTIPOS])
def test_nd2_matches_bioio_nd2(corpus: Corpus, name: str) -> None:
    bioio_nd2 = pytest.importorskip("bioio_nd2")
    path = corpus(name)
    ours = BioImage(path, reader=Reader)
    ref = BioImage(path, reader=bioio_nd2.Reader)
    assert len(ours.scenes) == len(ref.scenes)
    for s in range(len(ref.scenes)):
        ours.set_scene(s)
        ref.set_scene(s)
        assert ours.dims.order == ref.dims.order
        assert ours.shape == ref.shape
        assert ours.channel_names == [str(c) for c in ref.channel_names]
        assert ours.physical_pixel_sizes.X == pytest.approx(ref.physical_pixel_sizes.X)
        want = ref.data
        if "S" in ref.dims.order and ref.dims.S == 3:
            # bioio-nd2 (nd2) returns colour planes in stored B, G, R order; we return R, G, B
            # (docs/formats/nd2.md § RGB sample order).
            want = want[..., ::-1]
        np.testing.assert_array_equal(ours.data, want)


@pytest.mark.parametrize("name", [CZI, CZI_MOSAIC])
def test_czi_matches_czifile(corpus: Corpus, name: str) -> None:
    czifile = pytest.importorskip("czifile")
    if not hasattr(czifile.CziFile, "asxarray"):  # pre-2025 czifile (e.g. on Python 3.10)
        pytest.skip("czifile with scenes/asxarray (Python >= 3.11) is required")
    path = corpus(name)
    img = BioImage(path, reader=Reader)
    with czifile.CziFile(path, squeeze=False) as czi:
        scene_keys = list(czi.scenes.keys())
        assert len(scene_keys) == len(img.scenes)
        for s, key in enumerate(scene_keys):
            ref = czi.asxarray(scene=key)
            img.set_scene(s)
            expected = _to_tczyxs(list(ref.dims), ref.values)
            np.testing.assert_array_equal(_ours_tczyxs(img), expected)


def test_lif_matches_liffile(corpus: Corpus) -> None:
    liffile = pytest.importorskip("liffile")
    path = corpus(LIF)
    img = BioImage(path, reader=Reader)
    with liffile.LifFile(path) as lif:
        assert len(lif.images) == len(img.scenes)
        for s, series in enumerate(lif.images):
            ref = series.asxarray()
            img.set_scene(s)
            np.testing.assert_array_equal(_ours_tczyxs(img), _to_tczyxs(list(ref.dims), ref.values))


# ----- lazy reads and metadata --------------------------------------------------------------------


def test_lazy_selection_matches_full_read(corpus: Corpus) -> None:
    img = BioImage(corpus(ND2_MULTIPOS), reader=Reader)
    img.set_scene("point name 3")
    full = img.data
    lazy = img.get_image_dask_data("ZYX", T=2, C=1)
    np.testing.assert_array_equal(lazy.compute(), full[2, 1])
    np.testing.assert_array_equal(img.get_image_data("CYX", T=1, Z=4), full[1, :, 4])
    np.testing.assert_array_equal(
        img.reader._read_indexed("TCZYX", [0, [1, 0], slice(0, 2), 3, slice(None)]),
        full[0][[1, 0]][:, 0:2, 3],
    )


def test_scene_names_fallback_and_rgb(corpus: Corpus) -> None:
    img = BioImage(corpus(ND2_RGB_MULTI), reader=Reader)
    assert img.scenes == ("Position 0", "Position 1")
    assert img.dims.order == "TCZYXS" and img.dims.S == 3


def test_metadata(corpus: Corpus) -> None:
    img = BioImage(corpus(LIF), reader=Reader)
    ome = img.ome_metadata
    assert len(ome.images) == len(img.scenes)
    assert ome.images[0].pixels.size_t == 4 and ome.images[0].pixels.size_c == 2
    assert img.metadata is ome
    assert img.xarray_dask_data.attrs["unprocessed"]  # vendor tree
    assert img.time_interval == timedelta(seconds=img.reader.file.images[0]["time_increment_s"])
    std = img.standard_metadata
    assert std.image_size_t == 4 and std.pixel_size_x == pytest.approx(img.physical_pixel_sizes.X)


def test_version() -> None:
    assert isinstance(bioio_openreadout.__version__, str)


# ----- bioio's own plugin checks, resolution levels ----------------------------------------------

KIDNEY = "zenodo10577621-Kidney-RAC-3color.czi"  # 5 pyramid levels, 4 channels, uint16 mosaic


def test_bioio_base_reader_checks(corpus: Corpus) -> None:
    """bioio-base's own plugin test suite: no file handle left open, scene and resolution-level
    switching, dims, dtype, physical sizes, lazy vs in-memory reads, (de)serialization."""
    tu = pytest.importorskip("bioio_base.test_utilities")
    pytest.importorskip("distributed")
    from ome_types import OME

    scenes = Reader(corpus(CZI)).scenes
    tu.run_image_file_checks(
        ImageContainer=Reader,
        image=corpus(CZI),
        set_scene=scenes[1],
        expected_scenes=scenes,
        expected_current_scene=scenes[1],
        expected_shape=(1, 3, 5, 325, 475),
        expected_dtype=np.dtype(np.uint16),
        expected_dims_order="TCZYX",
        expected_channel_names=["EGFP", "TaRFP", "Bright"],
        expected_physical_pixel_sizes=Reader(corpus(CZI)).physical_pixel_sizes,
        expected_metadata_type=OME,
        reader_kwargs={},
    )


def test_bioio_base_checks_at_a_resolution_level(corpus: Corpus) -> None:
    tu = pytest.importorskip("bioio_base.test_utilities")
    pytest.importorskip("distributed")
    from ome_types import OME

    path = corpus(KIDNEY)
    base = Reader(path)
    ps0 = base.physical_pixel_sizes
    levels = base.file.levels(0)
    lv = levels[3]
    tu.run_image_file_checks(
        ImageContainer=Reader,
        image=path,
        set_scene=base.scenes[0],
        expected_scenes=base.scenes,
        expected_current_scene=base.scenes[0],
        expected_shape=(1, 4, 1, lv["size_y"], lv["size_x"]),
        expected_dtype=np.dtype(np.uint16),
        expected_dims_order="TCZYX",
        expected_channel_names=base.channel_names,
        expected_physical_pixel_sizes=type(ps0)(
            ps0.Z, ps0.Y * lv["downsample_y"], ps0.X * lv["downsample_x"]
        ),
        expected_metadata_type=OME,
        set_resolution_level=3,
        expected_current_resolution_level=3,
        expected_resolution_levels=(0, 1, 2, 3, 4),
        reader_kwargs={},
    )


def test_resolution_level_matches_oracle(corpus: Corpus) -> None:
    """Level 4 equals the committed ground truth: czifile's pixels on the level grid of
    ``oracle/czi_levels.py`` (``floor(w / 16) x floor(h / 16)`` from the scene origin, as
    pylibCZIrw's scaled reads). czifile's own ``levels[4]`` spans the union of the subblocks, a
    grid 1-2 px larger, so it is not compared directly."""
    import json

    xxhash = pytest.importorskip("xxhash")
    path = corpus(KIDNEY)
    repo = Path(__file__).resolve().parents[3]
    oracle = repo / "corpus" / "oracle" / "zenodo10577621-Kidney-RAC-3color.json"
    truth = json.loads(oracle.read_text())
    lv = next(lv for lv in truth["images"][0]["levels"] if lv["level"] == 4)
    img = BioImage(path, reader=Reader)
    img.set_resolution_level(4)
    ours = img.get_image_data("CYX")
    assert ours.shape[1:] == (lv["size_y"], lv["size_x"])
    assert len(lv["planes"]) == ours.shape[0]
    for p in lv["planes"]:
        plane = np.ascontiguousarray(ours[p["c"]], dtype=ours.dtype.newbyteorder("<"))
        assert xxhash.xxh3_128_hexdigest(plane.tobytes()) == p["xxh3"], p


def test_keep_open_reader_holds_the_file(corpus: Corpus) -> None:
    psutil = pytest.importorskip("psutil")
    path = str(corpus(CZI))
    r = Reader(path, keep_open=True)
    assert path in [f.path for f in psutil.Process().open_files()]
    r.file.close()
    lazy = Reader(path)  # default: nothing stays open
    lazy.get_image_data("YX", Z=1, C=2)
    assert path not in [f.path for f in psutil.Process().open_files()]
