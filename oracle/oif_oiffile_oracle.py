#!/usr/bin/env python
"""Olympus OIF/OIB ground truth from oiffile alone, for files Bio-Formats misreads.

`gen.py`'s `oif_()` takes geometry, sample type and planes from Bio-Formats and cross-checks the
planes with oiffile (`oiffile_agrees`). On OIF files whose settings say `ValidBitCounts=8` while
the plane TIFFs store 16-bit samples (figshare 19409903: `FileVersion` 1.2.3.0, `FileType`
"Color32Bit", axes calibrated in pixels), Bio-Formats 8.5.0 reports uint8 planes whose values are
neither the stored samples nor their low or high bytes, and a 1 µm pixel size the file does not
state. There `oiffile_agrees` is false. This script writes the oracle from oiffile (BSD-3,
https://github.com/cgohlke/oiffile) and tifffile (BSD-3) only:

- planes: `gen._oif_oiffile_planes` (every plane TIFF read with `OifFile.asarray`, placed by the
  axis indices in its file name), hashed as `gen.py` hashes them;
- geometry and sample type: the shape and dtype of those planes and the number of C, Z and T
  indices;
- physical sizes: from the main settings file's `Axis N Parameters Common` (X, Y, Z) only when
  `UnitName` is a length unit (nm, um, mm), as `(EndPosition - StartPosition) / (MaxSize - 1)`
  for X and Y and the same for Z; none here, where the unit is "Pixel".

Usage (the shared oracle venv):
    oracle/.venv/bin/python oracle/oif_oiffile_oracle.py --id ID path/to/file.oif

Writes `corpus/oracle/<ID>.json` in the shape `gen.py` writes for OIF/OIB.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

UNIT_UM = {"nm": 1e-3, "um": 1.0, "µm": 1.0, "mm": 1e3}


def physical_sizes(main: dict) -> dict:
    out = {}
    for sec in main.values():
        if not isinstance(sec, dict) or "AxisCode" not in sec:
            continue
        axis = str(sec.get("AxisCode", "")).strip('"').lower()
        unit = str(sec.get("UnitName", "")).strip('"')
        if axis not in ("x", "y", "z") or unit not in UNIT_UM:
            continue
        n = int(sec.get("MaxSize", 0) or 0)
        if n > 1:
            step = abs(float(sec["EndPosition"]) - float(sec["StartPosition"])) / (n - 1)
            out[axis] = step * UNIT_UM[unit]
    return out


def main() -> None:
    import numpy as np
    import oiffile
    import gen

    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--id", required=True)
    ap.add_argument("file", type=Path)
    a = ap.parse_args()
    p = a.file
    ref = gen._oif_oiffile_planes(p)
    images = []
    with oiffile.OifFile(p) as f:
        main_settings = dict(f.mainfile)
        tifs = sorted(f.glob("*.tif"))
        first = np.asarray(f.asarray(tifs[0]))
    phys = physical_sizes(main_settings)
    for k, r in enumerate(ref):
        keys = list(r["planes"])
        C = len({c for c, _, _ in keys})
        Z = len({z for _, z, _ in keys})
        T = len({t for _, _, t in keys})
        planes = [{"c": c, "z": z, "t": t, "xxh3": r["planes"][(c, z, t)]} for (c, z, t) in sorted(keys, key=lambda q: (q[2], q[1], q[0]))]
        images.append({"index": k, "size_x": int(first.shape[-1]), "size_y": int(first.shape[-2]),
                       "size_z": Z, "size_c": C, "size_t": T, "samples_per_pixel": 1,
                       "pixel_type": gen.dtype_name(first.dtype),
                       "physical_size_um": {ax: v for ax, v in phys.items() if not (ax == "z" and Z <= 1)},
                       "planes": planes[:gen.MAX_PLANES]})
    data = {"id": a.id, "file": p.name, "size": p.stat().st_size, "sha256": gen.sha256(p),
            "max_planes_hashed": gen.MAX_PLANES,
            "reader": f"oiffile {oiffile.__version__} (BSD-3) planes and geometry; Bio-Formats 8.5.0 misreads this file (see oracle/oif_oiffile_oracle.py)",
            "images": images}
    gen.OUT.mkdir(parents=True, exist_ok=True)
    import oracle_json
    oracle_json.write_text(gen.OUT / f"{a.id}.json", json.dumps(data, indent=1) + "\n")
    print(f"wrote {a.id}.json {len(images)} images")


if __name__ == "__main__":
    main()
