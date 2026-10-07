#!/usr/bin/env python
"""Ground truth for the generic HDF5 reader (`hdf5`): the object tree as h5py sees it.

Source, never our reader: h5py (BSD-3-Clause, https://www.h5py.org) on the HDF5 library. The
generic reader lists a file's groups and datasets (`openreadout ls --json`, `info`), so the
oracle records what a listing can be checked against:

- `superblock_version` (h5py's file creation property list), which `info` reports as
  `format_version` ("superblock version N");
- `groups` and `datasets`: the counts `info` puts in its notes (the root group included);
- `top_level`: the names directly under `/`, sorted;
- `objects`: every group and dataset reached from `/` through hard links, breadth-first with the
  children of each group sorted by name, each visited once: `path`, `kind` (`group` or
  `dataset`), for datasets `shape` and `dtype` (NumPy's name; `string` for HDF5 strings,
  `compound`, `enum`, `reference`, `opaque` or `vlen` otherwise), and `attributes`: each attribute
  name with its value when it is a number, a short number array (up to 16 values) or a text up to
  256 characters, else `null` (present, value not compared). Soft and external links are listed
  under `links` and not followed.

Usage (any venv with h5py; the shared oracle venv has it):
    python oracle/hdf5_structure_oracle.py --id ID corpus/files/hdf5/FILE.h5 [--out DIR]

Writes `<DIR>/<ID>.json` (default `corpus/oracle/hdf5/`). No corpus test reads it yet; see the
provenance log for the comparison it is meant for.
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import deque
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import oracle_json  # noqa: E402

OUT = HERE.parent / "corpus" / "oracle" / "hdf5"


def dtype_name(dt, h5type=None) -> str:
    """NumPy's name for the dataset's type, except for HDF5 classes NumPy hides: h5py reads an
    HDF5 enum of FALSE/TRUE as NumPy bool, but the stored type is an enum."""
    import h5py
    import numpy as np

    if h5type is not None and h5type.get_class() == h5py.h5t.ENUM:
        return "enum"
    if h5py.check_string_dtype(dt) is not None or dt.kind in ("S", "U"):
        return "string"
    if h5py.check_vlen_dtype(dt) is not None:
        return "vlen"
    if h5py.check_enum_dtype(dt) is not None:
        return "enum"
    if h5py.check_ref_dtype(dt) is not None:
        return "reference"
    if dt.names:
        return "compound"
    if dt.kind == "V":
        return "opaque"
    return np.dtype(dt).name


def attr_value(v):
    import numpy as np

    if isinstance(v, bytes):
        v = v.decode("utf-8", "replace")
    if isinstance(v, str):
        return v if len(v) <= 256 else None
    if isinstance(v, (int, float, np.integer, np.floating, np.bool_)):
        return v.item() if hasattr(v, "item") else v
    if isinstance(v, np.ndarray):
        if v.dtype.kind in "iufb" and v.size <= 16:
            return [x.item() for x in v.ravel()] if v.ndim else v.item()
        if v.dtype.kind in "SUO" and v.size == 1:
            return attr_value(v.ravel()[0])
    return None


def walk(f) -> tuple[list[dict], list[dict]]:
    import h5py

    objects, links = [], []
    seen = set()
    queue = deque([("/", f["/"])])
    while queue:
        path, obj = queue.popleft()
        addr = h5py.h5o.get_info(obj.id).addr
        if addr in seen:
            continue
        seen.add(addr)
        entry = {"path": path, "kind": "group" if isinstance(obj, h5py.Group) else "dataset"}
        if entry["kind"] == "dataset":
            entry["shape"] = list(obj.shape) if obj.shape is not None else None
            entry["dtype"] = dtype_name(obj.dtype, obj.id.get_type())
        attrs = {}
        for name in sorted(obj.attrs.keys()):
            try:
                attrs[name] = attr_value(obj.attrs[name])
            except Exception:  # unreadable by h5py: present, value not compared
                attrs[name] = None
        entry["attributes"] = attrs
        objects.append(entry)
        if isinstance(obj, h5py.Group):
            for name in sorted(obj.keys()):
                child = path.rstrip("/") + "/" + name
                link = obj.get(name, getlink=True)
                if isinstance(link, (h5py.SoftLink, h5py.ExternalLink)):
                    links.append({"path": child, "kind": type(link).__name__})
                    continue
                queue.append((child, obj[name]))
    return objects, links


def main() -> None:
    import h5py

    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", type=Path, default=OUT)
    ap.add_argument("file", type=Path)
    a = ap.parse_args()
    try:
        with h5py.File(a.file, "r") as f:
            objects, links = walk(f)
            sb = f.id.get_create_plist().get_version()[0]
            o = {"id": a.id, "format": "hdf5", "independent": True,
                 "reader": f"h5py {h5py.__version__} (HDF5 {h5py.version.hdf5_version}, BSD-3-Clause)",
                 "superblock_version": sb,
                 "groups": sum(1 for x in objects if x["kind"] == "group"),
                 "datasets": sum(1 for x in objects if x["kind"] == "dataset"),
                 "top_level": sorted(f["/"].keys()),
                 "objects": objects, "links": links}
    except Exception as e:  # the independent reader could not read it: an oracle error, recorded
        o = {"id": a.id, "format": "hdf5", "error": f"{type(e).__name__}: {e}"}
    a.out.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(a.out / f"{a.id}.json", json.dumps(o, indent=1) + "\n")
    print(f"{a.id}: {len(o.get('objects', []))} objects" if "objects" in o else f"{a.id}: oracle error {o['error']}")


if __name__ == "__main__":
    main()
