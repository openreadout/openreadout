"""Tests for the ``openreadout`` Python package against public corpus files.

Each test skips when its corpus file is absent. Plane hashes are checked against the committed
oracle ground truth (``corpus/oracle/*.json``, produced by czifile/nd2/liffile) when ``xxhash``
is installed.
"""

from __future__ import annotations

import pickle
import threading
from pathlib import Path
from typing import Any, Callable, Dict, Optional

import numpy as np
import openreadout
import pytest
from openreadout import (
    CorruptFileError,
    File,
    InstrumentFileNotFoundError,
    OpenReadoutError,
    UnknownFormatError,
    UsageError,
)

CZI = "aics-s-3-t-1-c-3-z-5.czi"  # 3 scenes, C=3, Z=5, uint16
ND2_RGB = "aics-ND2-dims-rgb.nd2"  # 1 image, RGB uint8
ND2_RGB_MULTI = "aics-ND2-dims-rgb-t3p2c2z3x64y64.nd2"  # 2 positions, T=3, C=2, Z=3, RGB
ND2_MULTIPOS = "aics-ND2-dims-p4z5t3c2y32x32.nd2"  # 4 positions, T=3, C=2, Z=5, uint16
LIF = "aics-s-1-t-4-c-2-z-1.lif"  # 1 series, T=4, C=2, uint16
OME_TIFF = "ome-artificial-multi-channel-z-series.ome.tiff"  # OME-TIFF, C=3, Z=5, int8
IMAGEJ_TIFF = "aics-s_1_t_10_c_3_z_1.tiff"  # ImageJ hyperstack, T=10, C=3, big-endian uint16
MRC = "mrcfile-emd-3197.map"  # EMDB volume, Z=20, float32, 11.4 Å pixels
# Gatan DM4 diffraction pattern, float32 (the thumbnail is an attachment)
DM4 = "zenodo13821437-Figure-2e.dm4"
IMS = "ome-imaris/croppedRetinaLz4.ims"  # Imaris 5 (HDF5), LZ4 chunks, Z=64, C=2, uint8
ZVI = "figshare-zvi/figshare26880337-Figure_1H.zvi"  # AxioVision ZVI, C=3, uint16
OME_ZARR = "zenodo13982701-220605_151046_mip.zarr.zip"  # NGFF 0.4 plate: 2 fields, 2 labels
NWB = "dandi-nwb/dandi000006-sub-anm372907_ses-20170613.nwb"  # NWB 2.0.2, two irregular TimeSeries
DCIMG = "dcimg/zenodo-14281237/Cell07_642_000_000.dcimg"  # Hamamatsu DCIMG v7, T=10, uint16
METAMORPH_ND = (
    "metamorph/ssbd-232-vec-35mm/Dish1.nd"  # MetaMorph .nd series: C=2 x T=31 MetaSeries TIFFs
)

Corpus = Callable[[str], Path]
Oracle = Callable[[str], Optional[Dict[str, Any]]]

# (file, format, image count, shape of image 0 in TCZYX[S], dtype, channel names of image 0)
CASES = [
    (CZI, "czi", 3, (1, 3, 5, 325, 475), "uint16", ["EGFP", "TaRFP", "Bright"]),
    (ND2_RGB, "nd2", 1, (1, 1, 1, 64, 64, 3), "uint8", ["Brightfield"]),
    (ND2_RGB_MULTI, "nd2", 2, (3, 2, 3, 32, 32, 3), "uint8", ["Brightfield", "Brightfield"]),
    (ND2_MULTIPOS, "nd2", 4, (3, 2, 5, 32, 32), "uint16", ["Widefield Green", "Widefield Red"]),
    (LIF, "lif", 1, (4, 2, 1, 614, 614), "uint16", ["Gray", "Green"]),
    (OME_TIFF, "tiff", 1, (1, 3, 5, 167, 439), "int8", [None, None, None]),
    (IMAGEJ_TIFF, "tiff", 1, (10, 3, 1, 325, 475), "uint16", [None, None, None]),
    (MRC, "mrc", 1, (1, 1, 20, 20, 20), "float32", [None]),
    (DM4, "dm", 1, (1, 1, 1, 1024, 1024), "float32", ["003-F3left-hole(8,3)-SA40-diff"]),
    (IMS, "ims", 1, (1, 2, 64, 109, 143), "uint8", ["CollagenIV (TxRed)", "GFAP (FITC)"]),
    (ZVI, "zvi", 1, (1, 3, 1, 1024, 1344), "uint16", ["Alexa 594", "Alexa 488", "DAPI"]),
    (OME_ZARR, "ome-zarr", 4, (1, 1, 1, 680, 535), "uint16", ["R0_DAPI"]),
    (DCIMG, "dcimg", 1, (10, 1, 1, 168, 2048), "uint16", [None]),
    (METAMORPH_ND, "tiff", 1, (31, 2, 1, 512, 512), "uint16", ["FRET", "CFP"]),
]
IDS = [c[0] for c in CASES]


def _xxh3(arr: np.ndarray) -> str:
    xxhash = pytest.importorskip("xxhash")
    le = np.ascontiguousarray(arr, dtype=arr.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(le.tobytes())


# ----- module-level API -------------------------------------------------------------------------


def test_version_and_all() -> None:
    assert openreadout.__version__.count(".") == 2
    for name in openreadout.__all__:
        assert hasattr(openreadout, name), name


def test_formats_lists_the_microscopy_readers() -> None:
    ids = {f["id"] for f in openreadout.formats()}
    assert {"czi", "nd2", "lif", "tiff"} <= ids


@pytest.mark.parametrize("name,fmt,n_images,shape,dtype,channels", CASES, ids=IDS)
def test_info_views(
    corpus: Corpus, name: str, fmt: str, n_images: int, shape: tuple, dtype: str, channels: list
) -> None:
    path = corpus(name)
    meta = openreadout.info(path)
    assert meta["format"]["id"] == fmt
    assert len(meta["images"]) == n_images
    im = meta["images"][0]
    assert (im["size_t"], im["size_c"], im["size_z"], im["size_y"], im["size_x"]) == shape[:5]
    assert im["samples_per_pixel"] == (shape[5] if len(shape) == 6 else 1)
    assert [c.get("name") for c in im["channels"]] == channels
    assert meta["plane_count"] == sum(i["plane_count"] for i in meta["images"])

    det = openreadout.info(str(path), view="format")
    assert det["format"] == fmt and det["confidence"] == "definite"
    full = openreadout.info(path, view="full")
    assert full["file"]["format"]["id"] == fmt and full["vendor"] and full["provenance"]
    assert "vendor" not in openreadout.info(path, view="full", vendor=False)
    structure = openreadout.info(path, view="structure")
    assert structure["format"] == fmt and structure["entries"]
    assert openreadout.info(path, view="explain")["summary"]
    with pytest.raises(UsageError):
        openreadout.info(path, view="dump")


# ----- File ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("name,fmt,n_images,shape,dtype,channels", CASES, ids=IDS)
def test_file_metadata_vendor_check(
    corpus: Corpus, name: str, fmt: str, n_images: int, shape: tuple, dtype: str, channels: list
) -> None:
    with File(corpus(name)) as f:
        assert f.format == fmt
        assert f.dims(0) == ("TCZYXS" if len(shape) == 6 else "TCZYX")
        assert f.shape(0) == shape
        assert f.dtype(0) == np.dtype(dtype)
        assert isinstance(f.vendor, (dict, list)) and f.vendor
        assert f.provenance and all(isinstance(v, str) for v in f.provenance.values())
        assert f.entries()
        report = f.check()
        assert report["ok"] is True, report["findings"]
        assert report["format"] == fmt and report["checks_performed"]
        assert f.ome_xml().lstrip().startswith("<?xml")
        assert fmt in repr(f)


@pytest.mark.parametrize("name,fmt,n_images,shape,dtype,channels", CASES, ids=IDS)
def test_read_plane_owns_memory_and_matches_oracle(
    corpus: Corpus,
    oracle: Oracle,
    name: str,
    fmt: str,
    n_images: int,
    shape: tuple,
    dtype: str,
    channels: list,
) -> None:
    with File(corpus(name)) as f:
        plane = f.read_plane(image=0, c=0, z=0, t=0)
        assert plane.shape == shape[3:]
        assert plane.dtype == np.dtype(dtype) and plane.dtype.isnative
        # The decoded buffer itself (handed over without a copy): writable, contiguous and
        # shared with nothing else.
        assert plane.flags.writeable and plane.flags.c_contiguous
        before = plane.copy()
        plane[0, 0] = 0  # writable, and must not affect a fresh read
        again = f.read_plane(image=0, c=0, z=0, t=0)
        assert again is not plane and not np.shares_memory(again, plane)
        np.testing.assert_array_equal(again, before)

        truth = oracle(name)
        if truth is None:
            return
        checked = 0
        for oim in truth["images"]:
            for p in oim.get("planes", []):
                got = f.read_plane(oim["index"], p["c"], p["z"], p["t"])
                assert _xxh3(got) == p["xxh3"], (oim["index"], p)
                checked += 1
        assert checked > 0


@pytest.mark.parametrize("name,fmt,n_images,shape,dtype,channels", CASES, ids=IDS)
def test_read_image_is_stack_of_planes(
    corpus: Corpus, name: str, fmt: str, n_images: int, shape: tuple, dtype: str, channels: list
) -> None:
    with File(corpus(name)) as f:
        last = n_images - 1
        arr = f.read_image(last)
        # `dtype` is image 0's; the last image may differ (OME-Zarr label images are uint32)
        assert arr.shape == f.shape(last) and arr.dtype == f.dtype(last)
        assert arr.flags.owndata
        t, c, z = (s - 1 for s in arr.shape[:3])
        np.testing.assert_array_equal(arr[t, c, z], f.read_plane(last, c=c, z=z, t=t))
        np.testing.assert_array_equal(arr[0, 0, 0], f.read_plane(last))


def test_ome_zarr_label_images_follow_the_fields(corpus: Corpus) -> None:
    with File(corpus(OME_ZARR)) as f:
        assert [im["name"] for im in f.images] == [
            "C/02/0",
            "C/02/1",
            "C/02/0/labels/org",
            "C/02/1/labels/org",
        ]
        assert [im["extra"].get("label_of") for im in f.images] == [None, None, 0, 1]
        assert f.dtype(0) == np.uint16 and f.dtype(2) == np.uint32
        assert f.read_plane(2).shape == f.read_plane(0).shape


def test_multiposition_nd2_positions_are_distinct_images(corpus: Corpus) -> None:
    with File(corpus(ND2_MULTIPOS)) as f:
        stacks = [f.read_image(i) for i in range(len(f.images))]
        assert all(s.shape == (3, 2, 5, 32, 32) for s in stacks)
        assert not np.array_equal(stacks[0], stacks[1])


def test_rgb_nd2_planes_are_interleaved(corpus: Corpus) -> None:
    with File(corpus(ND2_RGB)) as f:
        plane = f.read_plane()
        assert plane.shape == (64, 64, 3)
        assert f.to_dask().shape == (1, 1, 1, 64, 64, 3)


# ----- lazy access -------------------------------------------------------------------------------


@pytest.mark.parametrize("name", [CZI, ND2_RGB_MULTI, ND2_MULTIPOS, LIF])
def test_to_dask_reads_lazily_and_matches(corpus: Corpus, name: str) -> None:
    pytest.importorskip("dask.array")
    with File(corpus(name)) as f:
        image = len(f.images) - 1
        lazy = f.to_dask(image)
        assert lazy.shape == f.shape(image) and lazy.dtype == f.dtype(image)
        assert lazy.chunksize[:3] == (1, 1, 1)
        full = f.read_image(image)
        np.testing.assert_array_equal(lazy.compute(), full)  # threaded scheduler
        np.testing.assert_array_equal(lazy[-1, 1, 0, 5:9].compute(), full[-1, 1, 0, 5:9])
        # Same file, same image → same dask key (deterministic graph names).
        assert f.to_dask(image).name == lazy.name


def test_to_dask_decodes_only_requested_planes(corpus: Corpus, monkeypatch: Any) -> None:
    pytest.importorskip("dask.array")
    with File(corpus(CZI)) as f:
        calls = []
        real = f._n.read_plane
        monkeypatch.setattr(f, "_n", _Spy(f._n, calls, real))
        f.to_dask(1)[0, 2, 3].compute()
        assert calls == [(1, 2, 3, 0, 0, None)]  # image, c, z, t, level, region (whole plane)


class _Spy:
    def __init__(self, inner: Any, calls: list, real: Any) -> None:
        self._inner, self._calls, self._real = inner, calls, real

    def read_plane(self, *args: int) -> Any:
        self._calls.append(args)
        return self._real(*args)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._inner, name)


def test_get_image_dask_data_reorders_and_selects(corpus: Corpus) -> None:
    pytest.importorskip("dask.array")
    with File(corpus(ND2_MULTIPOS)) as f:
        full = f.read_image(2)
        zyx = f.get_image_data("ZYX", image=2, T=1, C=1)
        np.testing.assert_array_equal(zyx, full[1, 1])
        czyx = f.get_image_data("CZYX", image=2, T=2, C=[1, 0], Z=slice(1, 4))
        np.testing.assert_array_equal(czyx, full[2][[1, 0]][:, 1:4])
        yxz = f.get_image_data("YXZ", image=2)
        np.testing.assert_array_equal(yxz, np.moveaxis(full[0, 0], 0, -1))
        with pytest.raises(UsageError):
            f.get_image_data("ZZYX")
        with pytest.raises(UsageError):
            f.get_image_data("ZYX", Q=1)
        with pytest.raises(ValueError):
            f.get_image_data("ZYX", C=7)


def test_to_xarray_coords_and_attrs(corpus: Corpus) -> None:
    pytest.importorskip("xarray")
    with File(corpus(LIF)) as f:
        xr_lazy = f.to_xarray(0)
        assert xr_lazy.dims == ("T", "C", "Z", "Y", "X")
        assert list(xr_lazy.coords["C"].values) == ["Gray", "Green"]
        ps = f.images[0]["physical_size"]["x"]
        assert xr_lazy.coords["X"].values[1] == pytest.approx(ps)
        assert xr_lazy.coords["T"].values[1] == pytest.approx(f.images[0]["time_increment_s"])
        assert xr_lazy.attrs["openreadout"]["format"] == "lif"
        eager = f.to_xarray(0, delayed=False)
        assert isinstance(eager.data, np.ndarray)
        np.testing.assert_array_equal(xr_lazy.values, eager.values)
    with File(corpus(ND2_RGB)) as f:
        assert f.to_xarray().dims == ("T", "C", "Z", "Y", "X", "S")


def test_file_pickles_and_is_thread_safe(corpus: Corpus) -> None:
    with File(corpus(CZI)) as f:
        clone = pickle.loads(pickle.dumps(f))
        try:
            np.testing.assert_array_equal(clone.read_plane(2, 1, 3), f.read_plane(2, 1, 3))
        finally:
            clone.close()

        expected = {(c, z): f.read_plane(0, c, z) for c in range(3) for z in range(5)}
        errors: list = []

        def worker(c: int) -> None:
            try:
                for z in range(5):
                    np.testing.assert_array_equal(f.read_plane(0, c, z), expected[(c, z)])
            except Exception as e:  # pragma: no cover - reported below
                errors.append(e)

        threads = [threading.Thread(target=worker, args=(c % 3,)) for c in range(6)]
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        assert not errors


def test_context_manager_closes(corpus: Corpus) -> None:
    with openreadout.open(corpus(CZI)) as f:
        assert not f.closed
    assert f.closed
    assert "closed" in repr(f)
    with pytest.raises(ValueError, match="closed"):
        f.read_plane()
    f.close()  # idempotent


# ----- export ------------------------------------------------------------------------------------


@pytest.mark.parametrize("name", [CZI, ND2_RGB, ND2_MULTIPOS, LIF])
def test_export_ome_tiff_round_trip(corpus: Corpus, tmp_path: Path, name: str) -> None:
    out = tmp_path / "out.ome.tiff"
    with File(corpus(name)) as f:
        report = f.export(out, image=0)
        assert report["verified"] is True
        assert report["images_written"] == 1
        assert report["planes_written"] == f.images[0]["plane_count"]
        assert out.is_file() and out.stat().st_size == report["bytes_written"]

        tifffile = pytest.importorskip("tifffile")
        with tifffile.TiffFile(out) as tif:
            assert tif.is_ome
            first = tif.pages[0].asarray()
        np.testing.assert_array_equal(first, f.read_plane(0))

        with pytest.raises(UsageError, match="exists"):
            f.export(out)
        again = f.export(out, image=0, select=["c=0"], compression="lzw", overwrite=True)
        assert again["codec"] == "lzw" and again["verified"]
        with pytest.raises(ValueError, match="compression"):
            f.export(tmp_path / "x.ome.tiff", compression="brotli")
        with pytest.raises(UsageError, match="export format"):
            f.export(tmp_path / "x.png")


# ----- error mapping -----------------------------------------------------------------------------


def test_missing_file_is_ioerror(tmp_path: Path) -> None:
    missing = tmp_path / "nope.czi"
    with pytest.raises(IOError) as exc:
        openreadout.open(missing)
    assert isinstance(exc.value, FileNotFoundError)
    assert isinstance(exc.value, InstrumentFileNotFoundError)
    assert exc.value.code == "io" and exc.value.exit_code == 5
    assert exc.value.hint and exc.value.filename == str(missing)
    with pytest.raises(OSError):
        openreadout.info(missing)


def test_unknown_format_is_valueerror(tmp_path: Path) -> None:
    junk = tmp_path / "junk.bin"
    junk.write_bytes(b"not an instrument file" * 100)
    with pytest.raises(ValueError) as exc:
        openreadout.open(junk)
    assert isinstance(exc.value, UnknownFormatError)
    assert exc.value.code == "unknown_format" and exc.value.exit_code == 3
    with pytest.raises(UnknownFormatError):
        openreadout.info(junk, view="format")


def test_out_of_range_indices_are_valueerror(corpus: Corpus) -> None:
    with File(corpus(CZI)) as f:
        for kwargs in (dict(c=3), dict(z=5), dict(t=1), dict(image=3), dict(c=-1)):
            with pytest.raises(ValueError) as exc:
                f.read_plane(**kwargs)
            assert isinstance(exc.value, UsageError) and exc.value.exit_code == 2
        with pytest.raises(UsageError):
            f.read_image(99)
        with pytest.raises(UsageError):
            f.shape(-1)


def test_truncated_file_is_corrupt(corpus: Corpus, tmp_path: Path) -> None:
    src = corpus(LIF).read_bytes()
    cut = tmp_path / "truncated.lif"
    cut.write_bytes(src[: len(src) // 2])
    with File(cut) as f:
        assert f.check()["ok"] is False
        with pytest.raises(CorruptFileError) as exc:
            f.read_image(0)
        assert isinstance(exc.value, RuntimeError) and isinstance(exc.value, OpenReadoutError)
        assert exc.value.code == "corrupt_file" and exc.value.exit_code == 4
        assert "check" in (exc.value.hint or "")


# ----- mass spectrometry (Thermo .raw) ------------------------------------------------------------

RAW = "mtbls20-caffeine-pos.raw"
# An Orbitrap Exploris 120 run whose export (ProteoWizard 3.0.24054) keeps the peaks the file flags
# as reference ions, as the reader does; mtbls20's 2019 export leaves them out.
RAW_SPECTRA = "mtbls13930-neg-sqc-5.raw"


def test_thermo_raw_spectra_match_oracle(corpus: Corpus, oracle: Any) -> None:
    xxhash = pytest.importorskip("xxhash")
    path = corpus(RAW_SPECTRA)
    truth = oracle(RAW_SPECTRA)
    with File(path) as f:
        assert f.format == "thermo-raw"
        run = f.info["spectra"][0]
        assert run["scan_count"] == 4110 and run["ms_levels"] == [2]
        for s in truth["spectra"]["scans"][:24]:
            sp = f.read_spectrum(scan=s["scan_number"], centroid=s["centroided"])
            assert sp["ms_level"] == s["ms_level"]
            assert sp["precursor_mz"] == pytest.approx(s["precursor_mz"], rel=1e-9)
            assert sp["mz"].dtype == np.float64 and sp["intensity"].dtype == np.float32
            assert xxhash.xxh3_128_hexdigest(sp["mz"].astype("<f8").tobytes()) == s["xxh3_mz"]
            assert (
                xxhash.xxh3_128_hexdigest(sp["intensity"].astype("<f4").tobytes())
                == s["xxh3_intensity"]
            )
        with pytest.raises(UsageError):
            f.read_spectrum()


def test_thermo_raw_experiment(corpus: Corpus) -> None:
    path = corpus(RAW)
    with File(path) as f:
        e = f.experiment
        assert e is not None and e == f.info["experiment"]
        assert e["sample"]["id"] == "884_Caffeine_POS"
        assert e["sample"]["source_field"] == "spectra[0].extra.sample_name"
        assert e["provenance"]["sample.id"]["from"] == "spectra[0].extra.sample_name"
        assert e["instrument"]["model"] == "LTQ Orbitrap Discovery"
        assert e["method"]["parameters"]["polarity"]["value"] == ["positive"]
        assert e["method"]["parameters"]["method_length"] == {
            "value": 2,
            "unit": "min",
            "ucum": "min",
        }
        assert e["measurements"][0]["kind"] == "spectra"
        assert "MS1+MS2" in e["measurements"][0]["what"]


def test_thermo_raw_export_mzml(corpus: Corpus, tmp_path: Path) -> None:
    out = tmp_path / "out.mzML"
    with File(corpus(RAW)) as f:
        report = f.export(out, centroid=True)
    assert report["verified"] is True and report["spectra_written"] == 141
    assert out.read_bytes().startswith(b"<?xml")


def test_truncated_raw_is_corrupt(corpus: Corpus, tmp_path: Path) -> None:
    src = corpus(RAW).read_bytes()
    cut = tmp_path / "truncated.raw"
    cut.write_bytes(src[: len(src) // 2])
    with File(cut) as f:
        report = f.check()
        assert report["ok"] is False
        assert report["findings"][0]["code"] == "truncated"
        with pytest.raises(CorruptFileError) as exc:
            _ = f.info
        assert exc.value.exit_code == 4


# ----- traces (electrophysiology) ---------------------------------------------------------------


@pytest.mark.parametrize(
    "name",
    [
        "pyabf-14o16001-vc-pair-step.abf",
        "pyabf-2020-06-16-0000.abf",
        "pyabf-file-axon-3.abf",
        "pyabf-file-axon-7.abf",
    ],
)
def test_abf_read_trace_matches_oracle(corpus: Corpus, oracle: Oracle, name: str) -> None:
    path = corpus(name)
    truth = oracle(name)
    assert truth is not None
    o = truth["traces"][0]
    with File(path) as f:
        assert f.format == "abf"
        # trace 0 is the recording; an ABF 2 file with an epoch table adds its command (DAC)
        # waveforms as trace 1
        assert f.images == [] and 1 <= len(f.traces) <= 2
        assert all(t["name"] == "command" for t in f.traces[1:])
        t = f.traces[0]
        assert t["sweep_count"] == o["sweep_count"]
        assert len(t["channels"]) == o["channel_count"]
        xxhash = pytest.importorskip("xxhash")
        for sw in o["sweeps"]:
            data = f.read_trace(0, sw["sweep"])
            assert data.shape == (o["channel_count"], sw["sample_count"])
            assert data.dtype == np.float64 and data.flags.writeable
            for c, oc in enumerate(sw["channels"]):
                assert np.allclose(data[c, : len(oc["first"])], oc["first"], rtol=0, atol=1e-9)
                assert xxhash.xxh3_128_hexdigest(data[c].astype("<f8").tobytes()) == oc["xxh3"]
        window = f.read_trace(0, 0, first_sample=5, max_samples=3)
        assert np.array_equal(window, f.read_trace(0, 0)[:, 5:8])


@pytest.mark.parametrize(
    "name, fmt",
    [
        ("nlx-cheetah-v5-7-4-csc1.ncs", "neuralynx"),
        ("brk-pause-correct.ns2", "blackrock"),
        ("sglx-np2-subset-with-sync.imec0.ap.bin", "spikeglx"),
        ("intan-time-split-121054.rhd", "intan"),
        ("plexon-file-plexon-3.plx", "plexon"),
        # recording directories open like files
        ("nlx-cheetah-v5-5-1-session", "neuralynx"),
        ("intan-fpc-rhd-multistim-240514", "intan"),
    ],
)
def test_ephys_read_trace_matches_oracle(corpus: Corpus, name: str, fmt: str) -> None:
    import json

    path = corpus(name)
    oracle_path = (
        Path(__file__).resolve().parents[3]
        / "corpus"
        / "oracle"
        / (name.replace(".", "-") + ".json")
    )
    truth = json.loads(oracle_path.read_text(encoding="utf-8"))
    xxhash = pytest.importorskip("xxhash")
    with File(path) as f:
        assert f.format == fmt
        assert len(f.traces) == len(truth["traces"])
        for o in truth["traces"]:
            for sw in o["sweeps"]:
                data = f.read_trace(o["index"], sw["sweep"])
                assert data.shape == (o["channel_count"], sw["sample_count"])
                for c, oc in enumerate(sw["channels"]):
                    assert xxhash.xxh3_128_hexdigest(data[c].astype("<f8").tobytes()) == oc["xxh3"]


def test_read_trace_on_image_file_is_unsupported(corpus: Corpus) -> None:
    from openreadout import UnsupportedFeatureError

    with File(corpus("fcsparser-guava-muse.fcs")) as f:
        assert f.traces == []
        with pytest.raises(UnsupportedFeatureError):
            f.read_trace()


# ----- spectra (mass spectrometry) --------------------------------------------------------------


@pytest.mark.parametrize(
    "name, oracle_id, fmt",
    [
        ("mzdata-small.mzML", "mzdata-small", "mzml"),
        ("pyteomics-test-mzxml.mzXML", "pyteomics-test-mzxml", "mzxml"),
        ("timsrust-test-dda.d/analysis.tdf", None, "bruker-tdf"),
    ],
)
def test_read_spectrum_matches_oracle(
    corpus: Corpus, name: str, oracle_id: Optional[str], fmt: str
) -> None:
    import json

    path = corpus(name)
    xxhash = pytest.importorskip("xxhash")
    with File(path) as f:
        assert f.format == fmt
        assert f.images == [] and len(f.info["spectra"]) == 1
        n = f.info["spectra"][0]["scan_count"]
        first = f.read_spectrum(0)
        assert first["index"] == 0 and first["mz"].dtype == np.float64
        assert (
            first["intensity"].dtype == np.float32 and first["mz"].shape == first["intensity"].shape
        )
        assert f.read_spectrum(scan=first["scan_number"])["index"] == 0
        if oracle_id is None:
            return
        truth = json.loads(
            (
                Path(__file__).resolve().parents[3] / "corpus" / "oracle" / f"{oracle_id}.json"
            ).read_text()
        )
        assert n == truth["spectra"]["scan_count"]
        for s in truth["spectra"]["scans"]:
            sp = f.read_spectrum(s["index"])
            assert sp["ms_level"] == s["ms_level"]
            assert xxhash.xxh3_128_hexdigest(sp["mz"].astype("<f8").tobytes()) == s["xxh3_mz"]
            assert (
                xxhash.xxh3_128_hexdigest(sp["intensity"].astype("<f4").tobytes())
                == s["xxh3_intensity"]
            )


def test_spectra_match_oracle_headers(corpus: Corpus) -> None:
    import json

    path = corpus("mtbls20-caffeine-pos.raw")
    truth = json.loads(
        (
            Path(__file__).resolve().parents[3] / "corpus" / "oracle" / "mtbls20-caffeine-pos.json"
        ).read_text()
    )["spectra"]["scans"]
    with File(path) as f:
        every = f.spectra()
        assert every["source"] == "headers" and every["matched"] == len(truth)
        for ours, theirs in zip(every["scans"], truth, strict=True):
            assert ours["scan_number"] == theirs["scan_number"]
            assert ours["ms_level"] == theirs["ms_level"]
            assert abs(ours["rt_s"] - theirs["rt_s"]) < 1e-3
            if theirs.get("precursor_mz") is not None:
                assert (
                    abs(ours["precursor_mz"] - theirs["precursor_mz"])
                    < 1e-6 * theirs["precursor_mz"]
                )
        ms2 = f.spectra(ms_level=2, limit=2)
        assert ms2["returned"] == 2 and ms2["truncated"]
        assert ms2["matched"] == sum(1 for s in truth if s["ms_level"] == 2)
        near = f.spectra(precursor=84.08, precursor_tol=0.01, limit=0)
        assert near["returned"] == 0 and near["matched"] >= 1
        early = f.spectra(rt_range=(0.0, 0.05))
        assert all(s["rt_s"] <= 3.0 for s in early["scans"])
        with pytest.raises(UsageError):
            f.spectra(rt_range=(5.0, 1.0))


def test_read_spectrum_on_image_file_is_unsupported(corpus: Corpus) -> None:
    from openreadout import UnsupportedFeatureError

    with File(corpus("fcsparser-guava-muse.fcs")) as f:
        assert f.info.get("spectra", []) == []
        with pytest.raises(UnsupportedFeatureError):
            f.read_spectrum(0)


# ----- tables (FCS) -----------------------------------------------------------------------------


@pytest.mark.parametrize(
    "name", ["fcsparser-guava-muse.fcs", "flowio-g11.fcs", "fcsparser-cytek-xp5.fcs"]
)
def test_fcs_read_table_matches_oracle(corpus: Corpus, oracle: Oracle, name: str) -> None:
    path = corpus(name)
    truth = oracle(name)
    with File(path) as f:
        assert f.format == "fcs"
        assert f.images == []
        assert truth is not None and len(f.tables) == len(truth["tables"])
        for t, o in zip(f.tables, truth["tables"], strict=True):
            assert t["row_count"] == o["event_count"]
            assert [c["name"] for c in t["columns"]] == o["parameter_names"]
            events = f.read_table(t["index"])
            assert events.shape == (o["event_count"], len(o["parameter_names"]))
            assert events.dtype == np.float64 and events.flags.writeable
            if o["xxh3"]:
                xxhash = pytest.importorskip("xxhash")
                col_major = np.asfortranarray(events).astype("<f8").tobytes(order="F")
                assert xxhash.xxh3_128_hexdigest(col_major) == o["xxh3"]
        first = f.read_table(0, first_row=1, max_rows=2)
        assert np.array_equal(first, f.read_table(0)[1:3])


# ----- tables (plate-reader exports) ------------------------------------------------------------


@pytest.mark.parametrize(
    "name",
    ["gen5-multiple-read-modes.txt", "bmg-mars-lum-1536.csv", "magellan-elisa-384.xlsx"],
)
def test_plate_read_table_matches_allotropy(corpus: Corpus, name: str) -> None:
    """Measured values per detection mode as sorted (row, col, value) triples, hashed the way
    oracle/plate.py hashes allotropy's ASM output."""
    import struct

    xxhash = pytest.importorskip("xxhash")
    path = corpus(name)
    truth = _oracle_by_id(Path(name).stem)["plate"]
    with File(path) as f:
        assert f.format == "plate"
        triples: Dict[str, list] = {}
        for t in f.tables:
            assert [c["name"] for c in t["columns"]] == [
                "well",
                "row",
                "col",
                "read",
                "wavelength_nm",
                "time_s",
                "value",
            ]
            rows = f.read_table(t["index"])
            assert rows.shape == (t["row_count"], 7)
            reads = t["extra"]["reads"]
            for _, r, c, rd, _, _, v in rows:
                read = reads[int(rd) - 1]
                if read.get("calculated") or not np.isfinite(v):
                    continue
                triples.setdefault(read["mode"], []).append((int(r) - 1, int(c) - 1, float(v)))
        for group in truth["groups"]:
            ours = triples.get(group["mode"], [])
            packed = sorted(
                (r, c, struct.unpack("<Q", struct.pack("<d", 0.0 if v == 0 else v))[0])
                for r, c, v in ours
            )
            digest = xxhash.xxh3_128_hexdigest(b"".join(struct.pack("<IIQ", *p) for p in packed))
            assert len(ours) == group["values"] and digest == group["value_xxh3"], group["mode"]


# ----- traces (NMR) -----------------------------------------------------------------------------


def _oracle_by_id(ident: str) -> Dict[str, Any]:
    import json

    p = Path(__file__).resolve().parents[3] / "corpus" / "oracle" / f"{ident}.json"
    return json.loads(p.read_text(encoding="utf-8"))


@pytest.mark.parametrize(
    "rel, ident",
    [
        ("nmrxiv-s275/13", "nmrxiv-s275-13"),  # TopSpin 4.3: float64 fid + 1r/1i (NC_proc -2)
        ("nmrxiv-s837/24", "nmrxiv-s837-24"),  # TopSpin 3.7: 2D ser, 128 rows
        ("jcamp-lancashire-pktab1.jdx", "jcamp-lancashire-pktab1"),  # JCAMP-DX MS peak table
        ("nmrpy-test1.fid", "nmrpy-test1-fid"),  # Varian INOVA: 24 int32 traces
        (
            "nmrxiv-s200/Limonene_7020ug200uL_CDCl3_HSQC_400MHz_Jeol.jdf",
            "nmrxiv-s200-hsqc-jdf",
        ),  # JEOL 2D
    ],
)
def test_read_trace_matches_oracle(corpus: Corpus, rel: str, ident: str) -> None:
    path = corpus(rel)
    truth = _oracle_by_id(ident)
    with File(path) as f:
        assert f.format in ("bruker-nmr", "jcamp-dx", "varian-nmr", "jeol-jdf")
        assert f.images == [] and len(f.traces) == len(truth["traces"])
        for t, o in zip(f.traces, truth["traces"], strict=True):
            assert t["sample_count"] == o["sample_count"]
            sweeps = [f.read_trace(t["index"], s) for s in range(t["sweep_count"])]
            assert all(s.shape == (len(t["channels"]), t["sample_count"]) for s in sweeps)
            assert sweeps[0].dtype == np.float64 and sweeps[0].flags.writeable
            for osw in o.get("sweeps", []):
                xxhash = pytest.importorskip("xxhash")
                got = sweeps[osw["sweep"]]
                for c, oc in enumerate(osw["channels"]):
                    col = np.ascontiguousarray(got[c]).astype("<f8")
                    assert xxhash.xxh3_128_hexdigest(col.tobytes()) == oc["xxh3"]


def test_trace_channels_and_axis(corpus: Corpus) -> None:
    with File(corpus("nmrxiv-s275/13")) as f:
        spec = f.traces[1]
        assert spec["extra"]["kind"] == "processed_spectrum"
        real = f.read_trace(1, channel="real")
        assert real.shape == (spec["sample_count"],)
        assert np.array_equal(real, f.read_trace(1)[0])
        ppm = f.trace_axis(1)
        assert ppm is not None and ppm.shape == real.shape
        assert ppm[0] == pytest.approx(spec["extra"]["axis"]["first"])
        part = f.read_trace(1, first_sample=10, max_samples=5)
        assert np.array_equal(part, f.read_trace(1)[:, 10:15])
        with pytest.raises(UsageError):
            f.read_trace(1, channel="nope")


@pytest.mark.parametrize(
    "rel, ident, fmt",
    [
        ("entab-chemstation_mwd.d", "entab-chemstation-mwd-d", "chemstation"),  # five v30 signals
        ("entab-test_179_fid.ch", "entab-test-179-fid-ch", "chemstation"),  # v179 f64 FID
        (
            "cheminfo-agilent-hplc.cdf",
            "cheminfo-agilent-hplc-cdf",
            "andi-chrom",
        ),  # ANDI chromatogram
    ],
)
def test_chromatography_read_trace_matches_oracle(
    corpus: Corpus, rel: str, ident: str, fmt: str
) -> None:
    path = corpus(rel)
    truth = _oracle_by_id(ident)
    with File(path) as f:
        assert f.format == fmt
        assert len(f.traces) == len(truth["traces"])
        for t, o in zip(f.traces, truth["traces"], strict=True):
            y = f.read_trace(t["index"])
            assert y.shape == (o["channel_count"], o["sweeps"][0]["sample_count"])
            minutes = f.trace_axis(t["index"])
            assert minutes is not None and minutes[0] == pytest.approx(o["x_first_min"])
            xxhash = pytest.importorskip("xxhash")
            for c, oc in enumerate(o["sweeps"][0]["channels"]):
                col = np.ascontiguousarray(y[c]).astype("<f8")
                assert xxhash.xxh3_128_hexdigest(col.tobytes()) == oc["xxh3"]


def test_nwb_timeseries_traces_match_oracle(corpus: Corpus, oracle: Oracle) -> None:
    xxhash = pytest.importorskip("xxhash")
    path = corpus(NWB)
    truth = oracle(NWB)
    assert truth is not None
    with File(path) as f:
        assert f.format == "nwb"
        assert len(f.traces) == len(truth["traces"]) == 2
        for o in truth["traces"]:
            sw = o["sweeps"][0]
            data = f.read_trace(o["index"], 0)
            assert data.shape == (o["channel_count"], sw["sample_count"])
            for c, oc in enumerate(sw["channels"]):
                assert xxhash.xxh3_128_hexdigest(data[c].astype("<f8").tobytes()) == oc["xxh3"]


FCS = "flowio-100715.fcs"
PLATE = "bmg-mars-abs-384-qc.csv"
ABF = "pyabf-171116sh-0011.abf"
MZML = "mtbls20-caffeine-pos.mzML"


def test_to_arrow_table_matches_read_table(corpus: Corpus) -> None:
    pa = pytest.importorskip("pyarrow")
    with File(corpus(FCS)) as f:
        t = f.to_arrow()
        assert isinstance(t, pa.Table)
        assert t.num_rows == f.tables[0]["row_count"]
        assert t.column_names == [c["name"] for c in f.tables[0]["columns"]]
        md = t.schema.metadata
        assert md[b"openreadout.kind"] == b"table"
        assert md[b"openreadout.source_format"] == b"fcs"
        rows = f.read_table(0, 0, 1000)
        for i, name in enumerate(t.column_names):
            got = t.column(name).slice(0, 1000).to_numpy(zero_copy_only=False).astype(np.float64)
            assert np.array_equal(got, rows[:, i], equal_nan=True), name


def test_to_arrow_plate_wells_are_dictionaries(corpus: Corpus) -> None:
    pa = pytest.importorskip("pyarrow")
    with File(corpus(PLATE)) as f:
        t = f.to_arrow()
        assert pa.types.is_dictionary(t.schema.field("well").type)
        assert t.column("well")[0].as_py() == "A1"
        assert t.schema.field("value").metadata[b"unit"] == b"OD"


def test_to_arrow_trace_every_sweep_and_one(corpus: Corpus) -> None:
    pytest.importorskip("pyarrow")
    with File(corpus(ABF)) as f:
        tr = f.traces[0]
        whole = f.to_arrow()
        assert whole.num_rows == tr["sample_count"] * tr["sweep_count"]
        assert whole.column_names[:2] == ["sweep", "time_s"]
        one = f.to_arrow(sweep=1, rows=(10, 19))
        assert one.num_rows == 10
        assert set(one.column("sweep").to_pylist()) == {1}
        want = f.read_trace(0, 1, 10, 10)
        got = np.array(one.column(2).to_pylist())
        assert np.array_equal(got, want[0])
        assert one.column("time_s")[0].as_py() == pytest.approx(10 / tr["sample_rate_hz"])


def test_to_arrow_spectra_points_and_scans(corpus: Corpus) -> None:
    pytest.importorskip("pyarrow")
    with File(corpus(MZML)) as f:
        points = f.to_arrow(spectra=True)
        scans = f.to_arrow(spectra=True, per_scan=True)
        assert scans.num_rows == f.info["spectra"][0]["scan_count"]
        assert sum(scans.column("point_count").to_pylist()) == points.num_rows
        first = scans.column("point_count")[0].as_py()
        sp = f.read_spectrum(0)
        assert np.array_equal(np.array(points.column("mz").slice(0, first).to_pylist()), sp["mz"])
        with pytest.raises(UsageError):
            f.to_arrow(table=0, spectra=True)


def test_qpcr_records_and_rdml_export(corpus, tmp_path):
    """qPCR: named records with the vendor's Ct (read independently in corpus/oracle/qpcr), and an
    RDML export that reads back."""
    import json as _json

    path = corpus("eds-7500-abhd17c-ddct.eds")
    oracle = _json.loads(
        (
            Path(__file__).resolve().parents[3] / "corpus/oracle/qpcr/eds-7500-abhd17c-ddct.json"
        ).read_text()
    )
    rep = openreadout.analyze(path, "qpcr", well="A2", target="18s")
    (rec,) = rep["records"]
    want = next(r for r in oracle["records"] if r["well"] == "A2" and r["target"] == "18s")
    assert rec["cq"] == pytest.approx(want["cq"], abs=1e-9)
    rq = openreadout.analyze(path, "qpcr", ddcq=True)
    oe = next(q for q in rq["relative_quantities"] if q["sample"] == "ABHD17C OE")
    assert oe["rq"] == pytest.approx(oe["vendor_rq"], rel=1e-4)
    out = openreadout.export(path, tmp_path / "x.rdml")
    assert out["verified"] and out["cq_values"] > 0
    assert openreadout.info(tmp_path / "x.rdml")["format"]["id"] == "rdml"


# ----- high-content screening plates ------------------------------------------------------------


def test_screening_plate_layout_planes_and_well_stats(corpus: Corpus) -> None:
    """A CellVoyager plate folder: plate layout and plane pixels against oracle/hcs.py (tifffile),
    and per-well statistics that leave never-acquired planes out."""
    truth = _oracle_by_id("hcs-cellvoyager-jump-1053601756-folder")["hcs"]
    with File(corpus("hcs/cellvoyager-jump-1053601756")) as f:
        assert f.format == "cellvoyager"
        plate = f.plate
        assert plate is not None and not plate["complete"]
        assert len(plate["wells"]) == truth["plate"]["wells_imaged"]
        a01 = next(w for w in plate["wells"] if w["well"] == "A01")
        img = next(im for im in truth["images"] if im["well"] == "A01" and im["field"] == 1)
        pl = next(p for p in img["planes"] if p["state"] == "present")
        plane = f.read_plane(image=a01["images"][0], c=pl["c"], z=pl["z"], t=pl["t"])
        assert _xxh3(plane) == pl["xxh3"]
        rows = f.stats(per="well", select=["c=0"], wells=["A01"])["rows"]
        assert (
            rows[0]["well"] == "A01" and rows[0]["planes"] == 2 and rows[0]["planes_missing"] == 4
        )
