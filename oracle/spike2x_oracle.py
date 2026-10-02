"""Oracles for 64-bit Spike2 `.smrx` files from the Spike2 software's own exports of the same
recordings (never from the `.smrx` itself): writes `corpus/oracle/spike2x/<id>.json`, checked by
`crates/openreadout-corpus-tests/tests/spike2x_oracle/`.

    python spike2x_oracle.py --mat  <id> <export.mat>   # Spike2 "Export As MATLAB" (HDF5, v7.3)
    python spike2x_oracle.py --txt  <id> <export.txt>   # Spike2 "Export As text" (tab separated)

MATLAB exports carry each waveform channel's `values` (int16 × scale / 6553.6 + offset in
double), `scale`, `offset`, `start`, `interval`, `title`, `units`, `comment`, and each event
channel's `times`. The oracle recovers the stored int16 codes, round((value − offset) / scale),
and hashes them (xxh3-128 of little-endian int64), so the comparison is exact and independent of
the order of floating-point operations. Channels whose exported values are not a whole number of
codes (Spike2 channel processes applied at export: `walk`, `Pupil` in the corpus) keep only their
length, start and interval.

Text exports carry every channel at one sample interval with 5 decimals, event channels as 1 in
the sample bin of each event; the oracle keeps each waveform channel's length and 500 evenly
spaced values (compared within 1.5e-5), and the bins of every event (hashed).
"""
import json
import sys
from pathlib import Path

import numpy as np
import xxhash

OUT = Path(__file__).resolve().parent.parent / "corpus/oracle/spike2x"


def h64(a):
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(a, dtype="<i8")).tobytes())


def mat_text(g, k):
    if k not in g:
        return ""
    v = g[k][()]
    if v.dtype == np.uint16:
        return "".join(chr(c) for c in v.ravel()).strip()
    return ""


def from_mat(path):
    import h5py
    f = h5py.File(path, "r")
    traces, events = [], []
    for key in sorted(f.keys()):
        g = f[key]
        if "title" not in g:
            continue
        title = mat_text(g, "title")
        if "values" in g:
            v = g["values"][()].ravel().astype(np.float64)
            scale = float(g["scale"][()].ravel()[0])
            offset = float(g["offset"][()].ravel()[0])
            codes = (v - offset) / scale
            r = np.round(codes)
            exact = bool(np.abs(codes - r).max() < 1e-6) and bool(np.all(np.abs(r) <= 32768))
            t = {"title": title, "sample_count": int(len(v)), "start_s": float(g["start"][()].ravel()[0]),
                 "interval_s": float(g["interval"][()].ravel()[0]), "unit": mat_text(g, "units"),
                 "comment": mat_text(g, "comment"), "gain": scale, "offset": offset}
            if exact:
                t["codes_xxh3"] = h64(r)
                t["codes_first"] = [int(x) for x in r[:8]]
            else:
                t["processed_at_export"] = True
            traces.append(t)
        elif "times" in g:
            n = int(g["length"][()].ravel()[0])
            times = g["times"][()].ravel()[:n].astype(np.float64) if n else np.array([])
            res = float(g["resolution"][()].ravel()[0])
            ticks = np.round(times / res)
            assert np.abs(times / res - ticks).max(initial=0) < 1e-6
            e = {"title": title, "count": n, "ticks_xxh3": h64(ticks), "ticks_first": [int(x) for x in ticks[:8]],
                 "tick_s": res}
            if "codes" in g and n:
                e["codes"] = [int(x) for x in g["codes"][()].ravel()[:n]]
            events.append(e)
    return {"export": f"Spike2 MATLAB export {Path(path).name}", "traces": traces, "events": events}


def from_txt(path):
    with open(path) as fh:
        head = [h.strip('"') for h in fh.readline().rstrip("\n").split("\t")]
    d = np.loadtxt(path, skiprows=1, delimiter="\t")
    t = d[:, 0]
    dt = float(np.round(t[1] - t[0], 12))
    traces, events = [], []
    for k, name in enumerate(head[1:], 1):
        num, title = name.split(" ", 1)
        col = d[:, k]
        if "spikes" in title and set(np.unique(col)) <= {0.0, 1.0}:
            bins = np.nonzero(col)[0]
            events.append({"channel_number": int(num), "title": title, "count": int(len(bins)),
                           "bins_xxh3": h64(bins), "bin_s": dt, "bins_first": [int(x) for x in bins[:8]]})
        else:
            idx = np.linspace(0, len(col) - 1, 500).round().astype(int)
            traces.append({"channel_number": int(num), "title": title, "sample_count": int(len(col)),
                           "start_s": float(t[0]), "interval_s": dt,
                           "samples": {"index": [int(i) for i in idx], "value": [float(col[i]) for i in idx],
                                       "tolerance": 1.5e-5}})
    return {"export": f"Spike2 text export {Path(path).name}", "traces": traces, "events": events}


def main():
    mode, cid, path = sys.argv[1], sys.argv[2], sys.argv[3]
    out = from_mat(path) if mode == "--mat" else from_txt(path)
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{cid}.json").write_text(json.dumps(out, indent=1) + "\n")
    print(cid, len(out["traces"]), "traces", len(out["events"]), "event channels")


if __name__ == "__main__":
    main()
