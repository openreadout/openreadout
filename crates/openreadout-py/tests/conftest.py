"""Shared fixtures: locate corpus files and skip cleanly when they are not downloaded.

Corpus files come from ``cargo xtask corpus fetch --tier smoke`` (see ``corpus/manifest.toml``).
Set ``OPENREADOUT_CORPUS_DIR`` to use a directory other than ``<repo>/corpus/files``.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Any, Callable, Dict, Optional

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.append(str(REPO / "oracle"))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB)


def _corpus_dir() -> Path:
    env = os.environ.get("OPENREADOUT_CORPUS_DIR")
    return Path(env) if env else REPO / "corpus" / "files"


@pytest.fixture(scope="session")
def corpus() -> Callable[[str], Path]:
    """``corpus("name.czi")`` → path, or skip the test if the file is absent."""

    def get(name: str) -> Path:
        p = _corpus_dir() / name
        if not p.exists():
            pytest.skip(f"corpus file {name} not present (cargo xtask corpus fetch --tier smoke)")
        return p

    return get


@pytest.fixture(scope="session")
def oracle() -> Callable[[str], Optional[Dict[str, Any]]]:
    """``oracle("name.czi")`` → the committed ground truth JSON (``corpus/oracle/``), or None."""

    def get(name: str) -> Optional[Dict[str, Any]]:
        p = REPO / "corpus" / "oracle" / (name.rsplit(".", 1)[0] + ".json")
        if oracle_json.exists(p):
            return oracle_json.load(p)
        # Oracles are named by manifest id; TIFF-family ids differ from the file stem.
        leaf = Path(name).name
        for q in oracle_json.glob(REPO / "corpus" / "oracle"):
            data = oracle_json.load(q)
            if data.get("file") == leaf:
                return data
        return None

    return get
