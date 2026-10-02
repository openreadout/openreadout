"""Fixtures: locate corpus files (``OPENREADOUT_CORPUS_DIR`` or ``<repo>/corpus/files``)."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Callable

import pytest

REPO = Path(__file__).resolve().parents[3]


@pytest.fixture(scope="session")
def corpus() -> Callable[[str], Path]:
    """``corpus("name.czi")`` → path, or skip the test if the file is absent."""
    root = Path(os.environ.get("OPENREADOUT_CORPUS_DIR") or REPO / "corpus" / "files")

    def get(name: str) -> Path:
        p = root / name
        if not p.is_file():
            pytest.skip(f"corpus file {name} not present (cargo xtask corpus fetch --tier smoke)")
        return p

    return get
