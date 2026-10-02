"""Committed ground truth on disk: `<name>.json`, or `<name>.json.gz` when the JSON exceeds 1 MiB.

Standard library only (the evals and the Python tests import it too). Every path argument names
the logical `<name>.json`; the file on disk may be that path or `<name>.json.gz` (never both).
Gzip output is deterministic (mtime 0, no file name in the header), so regenerating identical
content gives identical bytes. `cargo xtask corpus compress` converts files written before this.
"""

from __future__ import annotations

import gzip
import io
import json
from pathlib import Path
from typing import Any, Iterator

# JSON larger than this is committed gzip-compressed (the same limit as `cargo xtask corpus compress`).
GZ_THRESHOLD = 1 << 20

_ORACLE = Path(__file__).resolve().parent.parent / "corpus" / "oracle"
# The directories whose readers accept `.json.gz` (the same list as `xtask/src/compress.rs`); other
# ground truth (per-family and per-analysis subdirectories, test-fixture oracles) stays plain JSON.
GZ_DIRS = (_ORACLE, _ORACLE / "heldout", _ORACLE / "flow")


def _gz(path: Path) -> Path:
    return path.with_name(path.name + ".gz")


def resolve(path: Path | str) -> Path | None:
    """The file that holds `<name>.json`: itself, `<name>.json.gz`, or None. Both present is an error."""
    path = Path(path)
    gz = _gz(path)
    plain, packed = path.is_file(), gz.is_file()
    if plain and packed:
        raise FileExistsError(f"both {path} and {gz} exist; keep one (cargo xtask corpus compress)")
    return path if plain else gz if packed else None


def exists(path: Path | str) -> bool:
    return resolve(path) is not None


def read_text(path: Path | str) -> str:
    p = resolve(path)
    if p is None:
        raise FileNotFoundError(f"{path} (nor {_gz(Path(path)).name})")
    if p.name.endswith(".gz"):
        return gzip.decompress(p.read_bytes()).decode("utf-8")
    return p.read_text(encoding="utf-8")


def load(path: Path | str) -> Any:
    return json.loads(read_text(path))


def gzip_bytes(data: bytes) -> bytes:
    """Deterministic gzip: no file name, mtime 0, fixed level."""
    buf = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=buf, compresslevel=9, mtime=0) as fh:
        fh.write(data)
    return buf.getvalue()


def write_text(path: Path | str, text: str) -> Path:
    """Write `<name>.json`, or `<name>.json.gz` when the text exceeds GZ_THRESHOLD bytes and the
    directory is one of GZ_DIRS; remove the stale sibling. Returns the path written."""
    path = Path(path)
    data = text.encode("utf-8")
    gz = _gz(path)
    if len(data) > GZ_THRESHOLD and path.parent.resolve() in GZ_DIRS:
        gz.write_bytes(gzip_bytes(data))
        path.unlink(missing_ok=True)
        return gz
    path.write_bytes(data)
    gz.unlink(missing_ok=True)
    return path


def glob(directory: Path | str, pattern: str = "*.json") -> Iterator[Path]:
    """Logical `<name>.json` paths matching `pattern` (which must end in `.json`), sorted, whether
    stored plain or gzip-compressed. Raises when a name is stored both ways."""
    directory = Path(directory)
    seen: dict[str, Path] = {}
    for p in list(directory.glob(pattern)) + list(directory.glob(pattern + ".gz")):
        logical = p.with_name(p.name[: -len(".gz")]) if p.name.endswith(".gz") else p
        if str(logical) in seen:
            raise FileExistsError(f"both {logical} and {logical}.gz exist; keep one (cargo xtask corpus compress)")
        seen[str(logical)] = logical
    return iter(sorted(seen.values()))
