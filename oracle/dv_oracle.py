"""DeltaVision (.dv, .r3d): mrc (BSD-3-Clause, Talley Lambert) `DVFile` reads the planes as a
black box; Bio-Formats 8.5.0 (GPL, black box) `showinf` gives a second opinion on the geometry
and pixel type (`bioformats_geometry_agrees`).

One image: size_x/size_y = the section size, size_c = wavelengths, size_z, size_t from mrc's
`sizes`; planes = mrc's array indexed by (C, Z, T) whatever the file's section order, hashed as
little-endian bytes (at most `max_planes`, spread over the stack); physical size = mrc's voxel
size in µm (Z only when there is more than one Z plane).
"""
from pathlib import Path

import numpy as np
import xxhash


def _hash(a) -> str:
    a = np.ascontiguousarray(a)
    if a.dtype.byteorder == ">":
        a = a.astype(a.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(a.tobytes())


def _dtype_name(dt) -> str:
    return {"uint8": "uint8", "int16": "int16", "uint16": "uint16", "int32": "int32", "float32": "float",
            "complex64": "complex"}[np.dtype(dt).newbyteorder("=").name]


def _spread(n: int, k: int):
    if n <= k:
        return list(range(n))
    return sorted({round(i * (n - 1) / (k - 1)) for i in range(k)})


def dv(p: Path, max_planes: int = 64, bioformats=None) -> dict:
    import mrc
    f = mrc.DVFile(str(p))
    try:
        sizes = dict(f.sizes)
        data = f.data
        axes = list(sizes)
        nc, nz, nt = sizes.get("C", 1), sizes.get("Z", 1), sizes.get("T", 1)
        idx = [(c, z, t) for t in range(nt) for c in range(nc) for z in range(nz)]
        planes = []
        for k in _spread(len(idx), max_planes):
            c, z, t = idx[k]
            sel = tuple({"C": c, "Z": z, "T": t}[a] for a in axes[:-2])
            planes.append({"c": c, "z": z, "t": t, "xxh3": _hash(np.asarray(data[sel]))})
        vs = f.voxel_size
        phys = {"x": float(vs.x) or None, "y": float(vs.y) or None}
        if nz > 1:
            phys["z"] = float(vs.z) or None
        img = {"index": 0, "size_x": sizes["X"], "size_y": sizes["Y"], "size_z": nz, "size_c": nc, "size_t": nt,
               "samples_per_pixel": 1, "pixel_type": _dtype_name(f.dtype), "physical_size_um": phys,
               "planes": planes}
        dtype = f.dtype
    finally:
        f.close()
    out = {"reader": f"mrc {mrc.__version__} (BSD-3-Clause) DVFile", "images": [img]}
    if bioformats is not None:
        try:
            s = bioformats(p)[0]
            agrees = (s["width"], s["height"], s.get("size_z", 1), s.get("size_c", 1), s.get("size_t", 1)) == (
                img["size_x"], img["size_y"], nz, nc, nt)
            out["bioformats_geometry"] = {k: s.get(k) for k in ("width", "height", "size_z", "size_c", "size_t", "pixel_type")}
            out["bioformats_geometry_agrees"] = bool(agrees)
        except Exception as e:  # noqa: BLE001  (a second opinion only)
            out["bioformats_error"] = f"{type(e).__name__}: {e}"[:300]
    del dtype
    return out
