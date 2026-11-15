"""nmrglue's 2D peak picker run as a black box on the processed Bruker 2D spectra (2rr).

For each JSON that `nmr_2d_peaks.rs` wrote with OPENREADOUT_NMR2D_OUT (our report, with the
noise SD and thresholds it used), read the same `pdata/<n>/2rr` with nmrglue
(`bruker.read_pdata`, BSD-3-Clause) and pick at the same absolute threshold (min_snr x our
noise SD, both signs):

- `peaks`: `peakpick.pick(algorithm="thres-fast", msep=(1, 1))`, every point that is the
  largest (smallest) in its 3 x 3 neighbourhood: the maxima view;
- `connected`: `peakpick.pick(algorithm="connected")`, one peak per connected region above the
  threshold: a grouped view.

Edge points are dropped (our picker does not report them), and only the TOP tallest of each
list are kept, so the files stay small. Writes `<id>.json` next to this script: {id, threshold,
shape, scale_ratio, peaks_total, connected_total, peaks: [[i, j, height], ...],
connected: [...]}, tallest first. DOSY spectra (pulse programs ledbp*, stebp*) are skipped.
Run from the repository root:

    oracle/.venv/bin/python -I crates/openreadout-corpus-tests/tests/corpus/nmr2d_oracle/nmrglue_peaks.py DIR
"""

import json
import pathlib
import sys

import nmrglue as ng
import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
TOP = 300


def pick(data, thres, algorithm):
    kw = {"msep": (1, 1)} if algorithm == "thres-fast" else {}
    locs = ng.peakpick.pick(
        data, pthres=thres, nthres=-thres, algorithm=algorithm, est_params=False,
        cluster=False, table=False, **kw,
    )
    n0, n1 = data.shape
    out = []
    for loc in locs:
        i, j = int(loc[0]), int(loc[1])
        if 0 < i < n0 - 1 and 0 < j < n1 - 1:
            out.append([i, j, float("%.6g" % data[i, j])])
    out.sort(key=lambda p: -abs(p[2]))
    return out


def main(src):
    for f in sorted(pathlib.Path(src).glob("*.json")):
        ours = json.loads(f.read_text())
        rep = ours["report"]
        if len(rep["shape"]) != 2 or ours["kind"] == "Dosy":
            continue
        thres = rep["peak_options"]["min_snr"] * rep["noise_sd"]
        if not thres > 0:
            continue
        _, data = ng.bruker.read_pdata(ours["pdata"], scale_data=True, all_components=False)
        data = np.asarray(data, dtype=float)
        if list(data.shape) != rep["shape"]:
            print(f"{ours['id']}: nmrglue shape {data.shape} != {rep['shape']}", file=sys.stderr)
            continue
        ratio = None
        mp = rep.get("main_peak")
        if mp:
            i, j = mp["index"]
            ratio = float(data[i, j]) / mp["height"] if mp["height"] else None
        peaks = pick(data, thres, "thres-fast")
        connected = pick(data, thres, "connected")
        res = {
            "id": ours["id"],
            "threshold": thres,
            "shape": list(data.shape),
            "scale_ratio": ratio,
            "peaks_total": len(peaks),
            "connected_total": len(connected),
            "peaks": peaks[:TOP],
            "connected": connected[:TOP],
        }
        (HERE / f"{ours['id']}.json").write_text(json.dumps(res, separators=(",", ":")) + "\n")
        print(ours["id"], len(peaks), len(connected), ratio)


if __name__ == "__main__":
    main(sys.argv[1])
