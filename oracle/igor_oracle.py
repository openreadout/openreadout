"""Igor Pro binary waves (.ibw, .bwav) and packed experiments (.pxp, .pxt): igor2 (LGPL-3.0,
run as a black box) reads every wave; this module only arranges its arrays the way OpenReadout
exposes them (docs/formats/igor.md):

- numeric waves of one dimension, and of two dimensions whose columns carry labels, are traces
  (one channel per column; complex values as a real and an imaginary channel). The trace name is
  the wave name (`.ibw`) or `root:folder:...:name` (packed experiment); the channel name is the
  wave name or the column label (`column N` when a column has none). `sample_rate_hz` is
  1 / step when the row units are a time unit;
- other numeric waves of two to four dimensions are images: rows are X, columns Y, layers are
  channels when they carry labels and Z planes otherwise, chunks are time points. A plane is the
  wave's [:, :, layer, chunk] slice transposed (rows of Y), hashed as little-endian bytes;
- text waves and waves without points are not compared.

Traces hash every value as little-endian float64 (gen.py `_trace_hash`).
"""
import sys
from pathlib import Path

import numpy as np
import xxhash

SUFFIXES = (".ibw", ".bwav", ".pxp", ".pxt")
TIME_UNITS = {"s": 1.0, "sec": 1.0, "ms": 1e-3, "us": 1e-6, "µs": 1e-6, "min": 60.0, "h": 3600.0}
LENGTH_UM = {"m": 1e6, "mm": 1e3, "um": 1.0, "µm": 1.0, "nm": 1e-3, "pm": 1e-6}


def _text(b) -> str:
    if isinstance(b, str):
        return b
    b = bytes(b).split(b"\0")[0]
    try:
        return b.decode("utf-8").strip()
    except UnicodeDecodeError:
        return b.decode("latin-1").strip()


def _hash_f64(a) -> str:
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(a, dtype="<f8")).tobytes())


def _hash_le(a) -> str:
    a = np.ascontiguousarray(a)
    if a.dtype.byteorder == ">" or (a.dtype.byteorder == "=" and sys.byteorder == "big"):
        a = a.astype(a.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(a.tobytes())


def _first(values, n=8):
    out = []
    for v in np.asarray(values, dtype=np.float64)[:n]:
        if not np.isfinite(v):
            break
        out.append(float(v))
    return out


def _dtype_name(dt) -> str:
    return {"uint8": "uint8", "uint16": "uint16", "uint32": "uint32", "uint64": "uint64", "int8": "int8",
            "int16": "int16", "int32": "int32", "int64": "int64", "float32": "float", "float64": "double",
            "complex64": "complex", "complex128": "double-complex"}[np.dtype(dt).name]


def _units(h, version):
    """Data units and units per dimension, extended units included (igor2's wave dict)."""
    data = _text(h.get("dataUnits", b"")) if not isinstance(h.get("dataUnits"), np.ndarray) else _text(b"".join(h["dataUnits"]))
    dims = []
    if version == 5:
        for row in h["dimUnits"]:
            dims.append(_text(b"".join(row)))
    else:
        x = h.get("xUnits", b"")
        dims.append(_text(b"".join(x) if isinstance(x, np.ndarray) else x))
    while len(dims) < 4:
        dims.append("")
    return data, dims


def _waves(path: Path):
    """(full name, version, igor2 wave dict) for every wave in file order."""
    if path.suffix.lower() in (".ibw", ".bwav"):
        from igor2 import binarywave
        d = binarywave.load(str(path))
        yield _text(d["wave"]["wave_header"]["bname"]), d["version"], d["wave"]
        return
    from igor2 import packed
    records, _fs = packed.load(str(path))
    folders = []
    for r in records:
        n = type(r).__name__
        if n == "FolderStartRecord":
            folders.append(_text(r.null_terminated_text))
        elif n == "FolderEndRecord":
            if folders:
                folders.pop()
        elif n == "WaveRecord":
            w = r.wave
            name = _text(w["wave"]["wave_header"]["bname"])
            yield ":".join(["root", *folders, name]), w["version"], w["wave"]


def igor(p: Path, max_planes: int = 64) -> dict:
    import igor2
    traces, images = [], []
    for full, version, w in _waves(p):
        h = w["wave_header"]
        a = np.asarray(w["wData"])
        if a.dtype.kind in "SUO" or h.get("type", 1) == 0 or a.size == 0:
            continue  # text waves and empty waves are vendor metadata
        dims = [int(x) for x in (h["nDim"] if version == 5 else [h["npnts"], 0, 0, 0])]
        rank = sum(1 for x in dims if x > 0)
        labels = w.get("labels") or [[], [], [], []]
        col_labels = [_text(x) for x in (labels[1][1:] if len(labels) > 1 and labels[1] else [])]
        layer_labels = [_text(x) for x in (labels[2][1:] if len(labels) > 2 and labels[2] else [])]
        data_units, dim_units = _units(h, version)
        if w.get("data_units"):
            data_units = _text(w["data_units"])
        for d, u in enumerate(w.get("dimension_units") or []):
            if d < 4 and u:
                dim_units[d] = _text(u)
        steps = list(h["sfA"]) if version == 5 else [h["hsA"]]
        complex_ = np.iscomplexobj(a)
        if rank <= 1 or (rank == 2 and any(col_labels)):
            rows = dims[0] if dims[0] > 0 else a.shape[0]
            cols = dims[1] if rank == 2 else 1
            m = a.reshape((rows, cols), order="F")
            names = ([full.split(":")[-1]] if rank <= 1 else
                     [col_labels[c] if c < len(col_labels) and col_labels[c] else f"column {c}" for c in range(cols)])
            chans, ch_names = [], []
            for c in range(cols):
                v = m[:, c]
                if complex_:
                    chans += [v.real, v.imag]
                    ch_names += [names[c] + " (real)", names[c] + " (imaginary)"]
                else:
                    chans.append(v)
                    ch_names.append(names[c])
            t = {"index": len(traces), "name": full, "sweep_count": 1, "channel_count": len(chans),
                 "channel_names": ch_names, "channel_units": [data_units] * len(chans),
                 "sweeps": [{"sweep": 0, "sample_count": int(rows),
                             "channels": [{"xxh3": _hash_f64(v), "first": _first(v)} for v in chans]}]}
            f = TIME_UNITS.get(dim_units[0])
            if f and steps[0] > 0:
                t["sample_rate_hz"] = 1.0 / (float(steps[0]) * f)
            traces.append(t)
            continue
        nx, ny = dims[0], dims[1]
        nl = dims[2] if rank >= 3 else 1
        nk = dims[3] if rank >= 4 else 1
        m = a.reshape((nx, ny, nl, nk), order="F")
        as_channels = any(layer_labels)
        planes = []
        for k in range(nk):
            for layer in range(nl):
                if len(planes) >= max_planes:
                    break
                c, z = (layer, 0) if as_channels else (0, layer)
                planes.append({"c": c, "z": z, "t": k, "xxh3": _hash_le(m[:, :, layer, k].T)})
        phys = {}
        for axis, d in (("x", 0), ("y", 1)) + ((("z", 2),) if rank >= 3 and not as_channels else ()):
            f = LENGTH_UM.get(dim_units[d])
            phys[axis] = abs(float(steps[d]) * f) if f and steps[d] else None
        images.append({"index": len(images), "name": full, "size_x": nx, "size_y": ny,
                       "size_z": 1 if as_channels else nl, "size_c": nl if as_channels else 1, "size_t": nk,
                       "samples_per_pixel": 1, "pixel_type": _dtype_name(a.dtype.newbyteorder("=")),
                       "physical_size_um": phys, "planes": planes})
    return {"reader": f"igor2 {igor2.__version__} (LGPL-3.0, black box)", "images": images, "traces": traces}
