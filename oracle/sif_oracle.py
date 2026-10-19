"""Andor SIF (.sif): sif_parser (BSD-3-Clause) reads every data block as a black box; sifreader
(MIT, black box, with a NumPy shim for the binary `fromstring` it calls) reads the files
sif_parser cannot. This module only arranges their arrays the way OpenReadout exposes them
(docs/formats/andor-sif.md):

- a block whose frames are single rows is a trace: one sweep per frame, `sample_count` = the
  row's width; channel 0 is the calibrated axis (sif_parser's `extract_calibration`) when the
  file holds a calibration other than 0 1 0 0, then the values;
- any other block is an image: `size_t` = frames, planes = each frame as stored (float32, rows
  in file order), hashed as little-endian bytes; `check_acquired_at` = the acquisition time;
- a block after the signal that sif_parser cannot parse is listed as a skipped image (presence
  only), and the blocks after it are not looked for.

Blocks after the signal (flags `1` after the signal data) are handed to sif_parser as a temporary
file that starts with the signature, the second line and that block, so sif_parser parses each block
itself; the oracle only steps over each block's data and the flag lines.
"""
import datetime as _dt
from pathlib import Path

import numpy as np
import xxhash

NAMES = ["signal", "reference", "background", "live", "source"]


def _hash_f64(a) -> str:
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(a, dtype="<f8")).tobytes())


def _hash_f32(a) -> str:
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(a, dtype="<f4")).tobytes())


def _first(values, n=8):
    out = []
    for v in np.asarray(values, dtype=np.float64)[:n]:
        if not np.isfinite(v):
            break
        out.append(float(v))
    return out


def _uncalibrated(c) -> bool:
    if c is None:
        return True
    c = list(np.asarray(c, dtype=float).ravel())
    return not c or (c[0] == 0 and len(c) > 1 and c[1] == 1 and all(x == 0 for x in c[2:]))


def _blocks_from(path, raw, prefix, start, role):
    import sif_parser
    import tempfile
    while True:
        if start == len(prefix):
            data, info = sif_parser.np_open(str(path))
        else:
            # sif_parser reads with np.fromfile: hand it a real file holding this block
            with tempfile.NamedTemporaryFile(suffix=".sif") as tf:
                tf.write(prefix + raw[start:])
                tf.flush()
                try:
                    data, info = sif_parser.np_open(tf.name)
                except Exception as e:  # noqa: BLE001  (a later block sif_parser cannot parse)
                    yield NAMES[role], None, {"error": f"{type(e).__name__}: {e}"[:300]}
                    return
        data = np.asarray(data)
        yield NAMES[role], data, info
        pos = start + (info["offset"] - len(prefix)) + data.size * 4
        nxt = None
        for k in range(role + 1, len(NAMES)):
            flag = raw[pos:pos + 2]
            if flag not in (b"0\n", b"1\n"):
                return
            pos += 2
            if flag == b"1\n":
                nxt = k
                break
        if nxt is None:
            return
        role, start = nxt, pos


def _sifreader(p: Path):
    """sifreader on a file sif_parser cannot read: (frames, h, w) float32 array of the signal."""
    import sys
    here = Path(__file__).resolve().parent / "third_party" / "sifreader"
    if str(here) not in sys.path:
        sys.path.insert(0, str(here))
    import sifreader
    orig = np.fromstring

    def fromstring(s, dtype=float, count=-1, sep=""):
        if sep == "" and isinstance(s, (bytes, bytearray)):
            return np.frombuffer(s, dtype=dtype, count=count).copy()
        return orig(s, dtype=dtype, count=count, sep=sep)

    np.fromstring = fromstring
    try:
        f = sifreader.SIFFile(str(p))
        a = np.asarray(f.read_all(), dtype=np.float32)
    finally:
        np.fromstring = orig
    if a.ndim == 2:
        a = a[None]
    return a


def sif(p: Path, max_planes: int = 64) -> dict:
    traces, images = [], []
    reader = None
    try:
        import sif_parser
        from sif_parser.utils import extract_calibration
        raw = p.read_bytes()
        second_end = raw.index(b"\n", 36) + 1
        blocks = list(_blocks_from(p, raw, raw[:second_end], second_end, 0))
        reader = f"sif_parser {sif_parser.__version__} (BSD-3-Clause)"
    except Exception as e:  # noqa: BLE001  (sif_parser fails on some versions: try sifreader)
        first_error = f"{type(e).__name__}: {e}"
        a = _sifreader(p)
        blocks = [("signal", a, None)]
        extract_calibration = None
        reader = f"sifreader 0.2.4 (MIT, black box); sif_parser failed: {first_error}"
    for name, data, info in blocks:
        if data is None:
            # sif_parser failed on this block: only its presence is compared
            images.append({"index": len(images), "name": name, "skip": info["error"]})
            continue
        frames, h, w = data.shape
        if h == 1:
            cal = None
            if info is not None and extract_calibration is not None:
                c = info.get("Calibration_data")
                if not _uncalibrated(c) or "Calibration_data_for_frame_1" in info:
                    cal = np.asarray(extract_calibration(info), dtype=np.float64)
            sweeps = []
            for k in range(frames):
                chans = []
                if cal is not None:
                    xs = cal[k] if cal.ndim == 2 else cal
                    chans.append(xs)
                chans.append(data[k, 0, :].astype(np.float64))
                sweeps.append({"sweep": k, "sample_count": int(w),
                               "channels": [{"xxh3": _hash_f64(v), "first": _first(v)} for v in chans]})
            t = {"index": len(traces), "name": name, "sweep_count": int(frames),
                 "channel_count": 1 + (cal is not None), "sweeps": sweeps}
            traces.append(t)
        else:
            planes = [{"c": 0, "z": 0, "t": k, "xxh3": _hash_f32(data[k])} for k in range(min(frames, max_planes))]
            im = {"index": len(images), "name": name, "size_x": int(w), "size_y": int(h), "size_z": 1,
                  "size_c": 1, "size_t": int(frames), "samples_per_pixel": 1, "pixel_type": "float",
                  "physical_size_um": {}, "planes": planes}
            if info is not None and int(info.get("ExperimentTime") or 0) > 0:  # -1: not recorded
                im["check_acquired_at"] = _dt.datetime.fromtimestamp(int(info["ExperimentTime"]), _dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
            images.append(im)
    out = {"reader": reader, "traces": traces, "images": images}
    if blocks and blocks[0][2] is not None:
        out["sif_version"] = int(blocks[0][2]["SifVersion"])
    return out
