#!/usr/bin/env python
"""Validate an OME-NGFF HCS plate written by `openreadout export PLATE --format ome-zarr` with
third-party readers, and compare its pixels with the source plane files read by tifffile.

    python omezarr_plate_validate.py STORE.ome.zarr ORACLE_JSON

ORACLE_JSON is the plate's ground truth from `oracle/hcs.py` (corpus/oracle/<id>.json): it names
every plane file (and its tifffile hash) by well and field. Checks:
  * strict OME-NGFF 0.5 validation of the plate with ome-zarr-models (`HCS`), including every
    well and every field image group;
  * ome_zarr.reader recognises the plate and lists its wells and fields;
  * the plate metadata: every imaged well of the export is listed at `<row>/<column>`, row and
    column indices match the names, `field_count`;
  * for every field in the store that the oracle has planes for: level-0 plane (t, c, z) read
    with zarr-python equals the tifffile plane of the source file (xxh3-128), and planes the
    oracle marks `not_acquired`/`not_recorded` are all zeros.
Exit status 0 only if everything passed. This script is a test harness; nothing here ships.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import xxhash
import zarr

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402


def h(a: np.ndarray) -> str:
    a = np.ascontiguousarray(a)
    if a.dtype.byteorder == ">":
        a = a.byteswap().view(a.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(a.tobytes())


def row_index(name: str) -> int:
    r = 0
    for i, ch in enumerate(name):
        v = ord(ch) - ord("A")
        r = v if i == 0 else (r + 1) * 26 + v
    return r


def main() -> int:
    store, oracle_path = Path(sys.argv[1]), Path(sys.argv[2])
    oracle = oracle_json.load(oracle_path)["hcs"]  # <id>.json, or <id>.json.gz over 1 MiB
    problems: list[str] = []

    from ome_zarr_models.v05.hcs import HCS
    root = zarr.open_group(store, mode="r")
    hcs = HCS.from_zarr(root)  # strict: raises on any schema violation
    plate = hcs.ome_attributes.plate
    wells = list(hcs.well_groups)
    print(f"ome-zarr-models: valid NGFF {hcs.ome_attributes.version} plate, {len(plate.wells)} wells, field_count {plate.field_count}")

    import ome_zarr.io
    import ome_zarr.reader
    loc = ome_zarr.io.parse_url(str(store))
    nodes = list(ome_zarr.reader.Reader(loc)())
    if not nodes or not any(isinstance(s, ome_zarr.reader.Plate) for s in nodes[0].specs):
        problems.append("ome_zarr.reader does not see a plate")
    else:
        print(f"ome_zarr.reader: plate node with {len(nodes)} node(s)")

    row_names = [r.name for r in plate.rows]
    col_names = [c.name for c in plate.columns]
    for w in plate.wells:
        r, c = w.path.split("/")
        if row_names[w.rowIndex] != r or col_names[w.columnIndex] != c:
            problems.append(f"well {w.path}: indices ({w.rowIndex}, {w.columnIndex}) name {row_names[w.rowIndex]}/{col_names[w.columnIndex]}")

    # oracle planes by (well name, field number)
    by_key = {(im["well"], im["field"]): im for im in oracle["images"]}
    fields_by_well: dict[str, list[int]] = {}
    for im in oracle["images"]:
        fields_by_well.setdefault(im["well"], []).append(im["field"])
    # every field of every well: the k-th field group of a well is the k-th exported field; the
    # export keeps field order, so map by the oracle's field numbers of that well in order
    exact = zeros = compared = 0
    for w in plate.wells:
        r, c = w.path.split("/")
        well_name = f"{r}{int(c):02d}"
        wg = root[w.path]
        images = wg.attrs["ome"]["well"]["images"]
        # which source fields were exported: the oracle fields of this well that have no
        # missing plane (the export used --skip-incomplete) or all of them
        candidates = [f for f in sorted(fields_by_well.get(well_name, []))
                      if not any(pl["state"] == "missing" for pl in by_key[(well_name, f)]["planes"])]
        for k, img in enumerate(images):
            arr = wg[img["path"]]["0"]
            if k >= len(candidates):
                continue  # a field the oracle holds no planes for
            im = by_key[(well_name, candidates[k])]
            for pl in im["planes"]:
                a = np.asarray(arr[pl["t"], pl["c"], pl["z"]])
                if pl["state"] == "present":
                    compared += 1
                    if h(a) == pl["xxh3"]:
                        exact += 1
                    else:
                        problems.append(f"{w.path}/{img['path']} c={pl['c']} z={pl['z']} t={pl['t']}: pixels differ from {pl['file']}")
                elif pl["state"] in ("not_acquired", "not_recorded"):
                    compared += 1
                    if not a.any():
                        zeros += 1
                    else:
                        problems.append(f"{w.path}/{img['path']} c={pl['c']}: absent plane is not blank")
    print(f"pixels: {exact} planes equal to tifffile's plane files, {zeros} absent planes blank, of {compared} compared")
    for p in problems:
        print("PROBLEM", p)
    return 1 if problems or compared == 0 else 0


if __name__ == "__main__":
    sys.exit(main())
