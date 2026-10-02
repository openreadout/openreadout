"""Opening from bytes and binary file-like objects (no corpus needed: committed fixtures)."""

from __future__ import annotations

import io
import pickle
import threading
from pathlib import Path
from typing import Any, Dict

import numpy as np
import openreadout
import pytest
from openreadout import File

REPO = Path(__file__).resolve().parents[3]
FIXTURES = [
    REPO / "crates/openreadout-cli/tests/fixtures/mini.czi",
    REPO / "crates/openreadout-cli/tests/fixtures/mini.nd2",
    REPO / "crates/openreadout-cli/tests/fixtures/mini.lif",
    REPO / "crates/openreadout-hdf5/tests/fixtures/synthetic-imaris.ims",
]


def _without_path(info: Dict[str, Any]) -> Dict[str, Any]:
    out = dict(info)
    out.pop("path", None)
    return out


def _planes(f: File) -> list:
    return [f.read_plane(i) for i in range(len(f.images))]


@pytest.mark.parametrize("path", FIXTURES, ids=lambda p: p.name)
def test_bytes_and_file_objects_match_the_path(path: Path) -> None:
    data = path.read_bytes()
    with File(path) as ref:
        want_info = _without_path(ref.info)
        want_planes = _planes(ref)
        want_check = ref.check()["ok"]
    sources = [
        ("bytes", data),
        ("bytearray", bytearray(data)),
        ("memoryview", memoryview(data)),
        ("BytesIO", io.BytesIO(data)),
    ]
    for how, src in sources:
        with openreadout.open(src, name=path.name) as f:
            assert f.path == path.name, how
            assert _without_path(f.info) == want_info, how
            for got, want in zip(_planes(f), want_planes, strict=True):
                np.testing.assert_array_equal(got, want, err_msg=how)
            assert f.check()["ok"] == want_check, how


def test_an_open_file_names_itself() -> None:
    path = FIXTURES[0]
    with path.open("rb") as fh, openreadout.open(fh) as f:
        assert f.path == path.name
        assert f.format == "czi"
        assert f.read_plane().shape[:2] == (f.images[0]["size_y"], f.images[0]["size_x"])


def test_detection_is_by_content_without_a_name() -> None:
    data = FIXTURES[1].read_bytes()
    assert openreadout.info(data, view="format")["format"] == "nd2"
    assert openreadout.info(io.BytesIO(data), view="format")["format"] == "nd2"
    assert openreadout.info(data)["path"] == "memory"
    assert openreadout.info(data, view="format", name="x.nd2")["path"] == "x.nd2"


def test_bytes_backed_files_pickle_but_file_objects_do_not() -> None:
    data = FIXTURES[0].read_bytes()
    with File(data, name="mini.czi") as f:
        clone = pickle.loads(pickle.dumps(f))
        try:
            np.testing.assert_array_equal(clone.read_plane(), f.read_plane())
            assert clone.path == "mini.czi"
        finally:
            clone.close()
    with File(io.BytesIO(data)) as f, pytest.raises(TypeError, match="pickled"):
        pickle.dumps(f)


def test_file_objects_serve_several_threads() -> None:
    data = FIXTURES[2].read_bytes()
    with File(io.BytesIO(data), name="mini.lif") as f, File(data, name="mini.lif") as ref:
        want = [ref.read_plane(i) for i in range(len(ref.images))]
        errors: list = []

        def worker() -> None:
            try:
                for _ in range(5):
                    for i, w in enumerate(want):
                        np.testing.assert_array_equal(f.read_plane(i), w)
            except Exception as e:  # pragma: no cover - reported below
                errors.append(e)

        threads = [threading.Thread(target=worker) for _ in range(4)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        assert not errors


def test_bad_sources_are_type_errors(tmp_path: Path) -> None:
    text = tmp_path / "notes.txt"
    text.write_text("hello")
    with text.open("r") as fh, pytest.raises(TypeError, match="binary mode"):
        File(fh)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="not int"):
        File(42)  # type: ignore[arg-type]


def test_unrecognised_bytes_are_unknown_format() -> None:
    with pytest.raises(openreadout.UnknownFormatError):
        File(b"\x00\x01 not an instrument file", name="blob.bin")
    with pytest.raises(openreadout.CorruptFileError):
        File(b"", name="empty.czi")


def test_lazy_arrays_of_different_buffers_do_not_collide() -> None:
    pytest.importorskip("dask.array")
    a, b = FIXTURES[0].read_bytes(), FIXTURES[2].read_bytes()
    with File(a) as fa, File(b) as fb, File(a) as fa2:
        assert fa.path == fb.path == "memory"
        assert fa.to_dask(0).name != fb.to_dask(0).name
        assert fa.to_dask(0).name == fa2.to_dask(0).name
        np.testing.assert_array_equal(fa.to_dask(0).compute(), fa.read_image(0))
