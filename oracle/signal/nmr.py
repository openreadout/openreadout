"""NMR processing ground truth for `analyze nmr-peaks` / `--process` (corpus/oracle/nmr-processing/*.json).

Per Bruker experiment with a TopSpin-processed pdata/1/1r (the vendor's own processing of the
same FID -- the strongest oracle):

- `procs`: the processing parameters TopSpin stored (SI, LB, PHC0, PHC1, OFFSET, SF, SW_p);
- `topspin_peaks`: peaks of TopSpin's 1r picked by SciPy (`scipy.signal.find_peaks`, height >=
  `PICK_SNR` x the noise SD, prominence >= `PICK_PROMINENCE` x the noise SD: nmrglue's picker has
  no prominence criterion and returns the truncation ripples on the flanks of a water line),
  positions in ppm from nmrglue's own unit conversion of the pdata axis, the tallest `TOP` kept;
- `topspin_peaklist`: TopSpin's stored peak list (pdata/1/peaklist.xml) when present;
- `topspin_integrals`: TopSpin's stored integrals (pdata/1/integrals.txt) when present;
- `topspin_snr`: max(1r) / noise SD, so tests can skip noise-only spectra;
- `nmrglue_fid_peaks`: the FID processed by an nmrglue pipeline (remove_digital_filter,
  zf_size(SI), em(LB/SW_h), fft, ps(stored phases), di, rev), peaks picked the same way.

Per JEOL FID with a spectrum processed by other software (MestReNova JCAMP-DX export of the
same acquisition), `reference_peaks` are SciPy picks on that spectrum (ppm from its X axis).

Usage: cd oracle/signal && uv run python nmr.py [--out ../../corpus/oracle/nmr-processing]
"""
import argparse
import json
import os
import re
import sys
import warnings

import numpy as np
import nmrglue as ng
from scipy.signal import find_peaks

warnings.filterwarnings("ignore")

CORPUS = os.environ.get("OPENREADOUT_CORPUS_DIR", os.path.join(os.path.dirname(__file__), "..", "..", "corpus", "files"))
PICK_SNR = 20.0
PICK_PROMINENCE = 5.0
TOP = 25

BRUKER = [
    ("nmrxiv-s275-1", "nmrxiv-s275/1"),
    ("nmrxiv-s275-11", "nmrxiv-s275/11"),
    ("nmrxiv-s275-13", "nmrxiv-s275/13"),
    ("nmrxiv-s275-21", "nmrxiv-s275/21"),
    ("nmrxiv-s596-1", "nmrxiv-s596/1"),
    ("nmrxiv-s846-50", "nmrxiv-s846/50"),
    ("nmrxiv-s501-7", "nmrxiv-s501/7"),
    # processing parity (2026-09-26): nmrXiv samples from 16 more projects
    ("nmrxiv-s886-1", "nmrxiv-s886/1"),
    ("nmrxiv-s882-1", "nmrxiv-s882/1"),
    ("nmrxiv-s904-1", "nmrxiv-s904/1"),
    ("nmrxiv-s897-1", "nmrxiv-s897/1"),
    ("nmrxiv-s1082-80", "nmrxiv-s1082/80"),
    ("nmrxiv-s1082-81", "nmrxiv-s1082/81"),
    ("nmrxiv-s1132-1", "nmrxiv-s1132/1"),
    ("nmrxiv-s1132-2", "nmrxiv-s1132/2"),
    ("nmrxiv-s1148-1", "nmrxiv-s1148/1"),
    ("nmrxiv-s1247-10", "nmrxiv-s1247/10"),
    ("nmrxiv-s1247-15", "nmrxiv-s1247/15"),
    ("nmrxiv-s1250-1", "nmrxiv-s1250/1"),
    ("nmrxiv-s1250-2", "nmrxiv-s1250/2"),
    ("nmrxiv-s1250-3", "nmrxiv-s1250/3"),
    ("nmrxiv-s1250-4", "nmrxiv-s1250/4"),
    ("nmrxiv-s1250-5", "nmrxiv-s1250/5"),
    ("nmrxiv-s1250-6", "nmrxiv-s1250/6"),
    ("nmrxiv-s1439-1h-nmr", "nmrxiv-s1439/1H NMR"),
    ("nmrxiv-s1462-13c-nmr", "nmrxiv-s1462/13C NMR"),
    ("nmrxiv-s1462-1h-nmr", "nmrxiv-s1462/1H NMR"),
    ("nmrxiv-s1462-31p-nmr", "nmrxiv-s1462/31P NMR"),
    ("nmrxiv-s279-20", "nmrxiv-s279/20"),
    ("nmrxiv-s279-21", "nmrxiv-s279/21"),
    ("nmrxiv-s597-1", "nmrxiv-s597/1"),
    ("nmrxiv-s699-1", "nmrxiv-s699/1"),
    ("nmrxiv-s699-2", "nmrxiv-s699/2"),
    ("nmrxiv-s702-1", "nmrxiv-s702/1"),
    ("nmrxiv-s702-2", "nmrxiv-s702/2"),
    ("nmrxiv-s736-1", "nmrxiv-s736/1"),
    ("nmrxiv-s736-2", "nmrxiv-s736/2"),
    ("nmrxiv-s740-1", "nmrxiv-s740/1"),
    ("nmrxiv-s740-4", "nmrxiv-s740/4"),
    ("nmrxiv-s753-20", "nmrxiv-s753/20"),
    ("nmrxiv-s753-proton-25", "nmrxiv-s753/proton_25"),
    ("nmrxiv-s753-proton-m25", "nmrxiv-s753/proton_M25"),
    ("nmrxiv-s853-1", "nmrxiv-s853/1"),
    ("nmrxiv-s853-2", "nmrxiv-s853/2"),
    ("nmrxiv-s853-3", "nmrxiv-s853/3"),
    ("nmrxiv-s853-4", "nmrxiv-s853/4"),
]
JEOL = [
    ("nmrxiv-s200-qhnmr-jdf", "nmrxiv-s200/Limonene_7020ug200uL_CDCl3_qHNMR_400MHz_Jeol.jdf",
     "nmrxiv-s200/Limonene_7020ug200uL_CDCl3_qHNMR_400MHz_JDX.jdx"),
]


def noise_sd(y):
    """Robust noise SD: 25th percentile over 64 blocks of 1.4826 x MAD of the residual of a line
    fit (the same definition the tool documents, so S/N thresholds mean the same thing)."""
    n = len(y)
    bs = max(8, n // 64)
    sds = []
    for i in range(0, n - bs + 1, bs):
        c = y[i:i + bs]
        x = np.arange(len(c))
        p = np.polyfit(x, c, 1)
        r = c - np.polyval(p, x)
        sds.append(1.4826 * np.median(np.abs(r - np.median(r))))
    sds.sort()
    return float(sds[(len(sds) - 1) // 4])


def pick(y, ppm_of_index, sd):
    idx, props = find_peaks(y, height=PICK_SNR * sd, prominence=PICK_PROMINENCE * sd)
    out = [{"index": int(i), "ppm": float(ppm_of_index(float(i))), "height": float(y[i])} for i in idx]
    out.sort(key=lambda p: -p["height"])
    return out[:TOP]


def bruker(eid, rel):
    d = os.path.join(CORPUS, rel)
    dic, fid = ng.bruker.read(d)
    pdic, r = ng.bruker.read_pdata(os.path.join(d, "pdata", "1"), scale_data=True)
    p = pdic["procs"]
    a = dic["acqus"]
    udic = ng.bruker.guess_udic(pdic, r)
    uc = ng.fileiobase.uc_from_udic(udic)
    sd = noise_sd(r)
    out = {
        "id": eid,
        "path": rel,
        "procs": {k: p.get(k) for k in ["SI", "LB", "WDW", "PHC0", "PHC1", "OFFSET", "SF", "SW_p", "NC_proc", "ME_mod", "TDoff", "BC_mod", "FCOR", "TDeff"]},
        "acqus": {k: a.get(k) for k in ["DSPFVS", "DIGMOD", "AQ_mod", "GRPDLY", "DECIM"]},
        "nucleus": a.get("NUC1"),
        "topspin_snr": float(np.max(r) / sd) if sd > 0 else None,
        "topspin_noise_sd": sd,
        "topspin_peaks": pick(r, uc.ppm, sd),
    }
    # TopSpin's stored peak list and integrals
    pl = os.path.join(d, "pdata", "1", "peaklist.xml")
    if os.path.exists(pl):
        txt = open(pl, encoding="utf-8", errors="replace").read()
        out["topspin_peaklist"] = [{"ppm": float(m.group(1)), "intensity": float(m.group(2))}
                                   for m in re.finditer(r'<Peak1D F1="([^"]+)" intensity="([^"]+)"', txt)]
    it = os.path.join(d, "pdata", "1", "integrals.txt")
    if os.path.exists(it):
        rows = []
        for line in open(it, encoding="utf-8", errors="replace"):
            m = re.match(r"\s*(\d+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.eE+]+)\s*$", line)
            if m:
                rows.append({"from_ppm": float(m.group(2)), "to_ppm": float(m.group(3)), "value": float(m.group(4))})
        out["topspin_integrals"] = rows
    # nmrglue pipeline from the FID with the stored parameters
    if a.get("AQ_mod") in (1, 3):
        data = ng.bruker.remove_digital_filter(dic, fid)
        data = ng.proc_base.zf_size(data, p["SI"])
        data = ng.proc_base.em(data, lb=p["LB"] / a["SW_h"])
        data = ng.proc_base.fft(data)
        best = None
        for s0 in (1, -1):
            for s1 in (1, -1):
                y = ng.proc_base.rev(ng.proc_base.di(ng.proc_base.ps(data, p0=s0 * p["PHC0"], p1=s1 * p["PHC1"])))
                c = float(np.corrcoef(y, r)[0, 1])
                if best is None or c > best[0]:
                    best = (c, s0, s1, y)
        c, s0, s1, y = best
        ysd = noise_sd(y)
        out["nmrglue_fid"] = {
            "pipeline": f"remove_digital_filter, zf_size({p['SI']}), em({p['LB']}/SW_h), fft, ps({s0:+d}*PHC0, {s1:+d}*PHC1), di, rev",
            "correlation_with_topspin": c,
            "peaks": pick(y, uc.ppm, ysd),
        }
    return out


def jeol(eid, rel, ref_rel):
    dic, data = ng.jcampdx.read(os.path.join(CORPUS, ref_rel))
    # MestReNova export: LINK block with one NMR SPECTRUM; XYDATA in Hz with .OBSERVE FREQUENCY
    b = dic  # nmrglue merges the NMR SPECTRUM block's labels into the top level
    y = np.asarray(data if not isinstance(data, list) else data[0], dtype=float)
    if y.ndim > 1:
        y = y[0]
    first = float(b["FIRSTX"][0]); last = float(b["LASTX"][0])
    obs = float(b[".OBSERVEFREQUENCY"][0])
    n = len(y)
    hz = lambda i: first + (last - first) * i / (n - 1)
    sd = noise_sd(y)
    return {
        "id": eid,
        "path": rel,
        "reference": ref_rel,
        "reference_software": b.get("ORIGIN", [""])[0] + " / MestReNova JCAMP-DX export",
        "reference_snr": float(np.max(y) / sd),
        "reference_peaks": pick(y, lambda i: hz(i) / obs, sd),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(os.path.dirname(__file__), "..", "..", "corpus", "oracle", "nmr-processing"))
    ap.add_argument("ids", nargs="*")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    for eid, rel in BRUKER:
        if args.ids and eid not in args.ids:
            continue
        o = bruker(eid, rel)
        json.dump(o, open(os.path.join(args.out, eid + ".json"), "w"), indent=1)
        print(eid, "topspin S/N %.0f" % (o["topspin_snr"] or 0), "peaks", len(o["topspin_peaks"]),
              "nmrglue corr %.5f" % o.get("nmrglue_fid", {}).get("correlation_with_topspin", float("nan")))
    for eid, rel, ref in JEOL:
        if args.ids and eid not in args.ids:
            continue
        o = jeol(eid, rel, ref)
        json.dump(o, open(os.path.join(args.out, eid + ".json"), "w"), indent=1)
        print(eid, "reference peaks", len(o["reference_peaks"]))


if __name__ == "__main__":
    sys.exit(main())
