"""Oracles for Magritek Spinsolve experiment directories (format id `magritek-spinsolve`).

    python spinsolve.py            # every magritek-spinsolve input of corpus/manifest.toml

Writes, per input:

- `corpus/oracle/<id>.json`: the generic trace oracle (`crates/openreadout-corpus-tests/tests/
  corpus/`). Parameters and the header come from nmrglue (BSD-3) `spinsolve.read` and
  `guess_udic`; values from nmrglue where its layout rule applies (data type 504, one row: the
  first third of the floats is the x axis), else from the documented layout (501: interleaved
  complex pairs; 503: x block then real values; rows back to back; see
  docs/formats/magritek-spinsolve.md). Where the depositor exported the same array as CSV
  (`DiffusionPlot.csv`, `DiffusionSpectrumStacked.csv`) the values are checked against it to
  the CSV's six significant digits.
- `corpus/oracle/nmr-processing/<id>.json`, when the directory holds the Spinsolve software's
  processed spectra (`spectrum.pt1`, `*-Spectra.pt1`): where each plotted spectrum sits in the
  plot file (its ppm axis block, found by its values: n float32 evenly spaced by
  `bandwidth`/`b1Freq`/n, followed by n complex float32 pairs), and the `Phase()` of
  `processing.script`. The plot file's own header is not decoded; the test reads the arrays at
  these offsets.
"""
import json
import os
import re
import sys
import tomllib
import warnings
from pathlib import Path

import numpy as np
import xxhash

ROOT = Path(__file__).resolve().parent.parent
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus/files"))
FID_NAMES = ["data.1d", "fid.1d", "data.2d", "data.3d", "data.4d"]


def h(values):
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(values, dtype="<f8")).tobytes())


def first(values, n=8):
    return [float(v) for v in np.asarray(values, dtype=np.float64)[:n]]


def header(p: Path):
    b = p.read_bytes()[:32]
    if len(b) < 32 or b[:8] != b"SORPATAD":
        return None
    v = np.frombuffer(b[12:32], "<u4")
    return int(v[0]), [int(x) for x in v[1:]]


def linear(x):
    x = np.asarray(x, dtype=np.float64)
    n = len(x)
    if n < 2 or not np.all(np.isfinite(x)):
        return None
    step = (x[-1] - x[0]) / (n - 1)
    if step == 0:
        return None
    tol = max(1e-3 * abs(step) + 1e-6 * max(abs(x[0]), abs(x[-1])), abs(step) * 1e-3)
    if np.all(np.abs(x - (x[0] + step * np.arange(n))) <= tol):
        return float(x[0]), float(step)
    return None


def read_acqu(d: Path) -> dict:
    """acqu.par through nmrglue's own line parser (nmrglue's read() needs a .1d/.dx file)."""
    from nmrglue.fileio import spinsolve as ns
    acqu = {}
    if (d / "acqu.par").exists():
        for line in (d / "acqu.par").read_text(errors="replace").splitlines():
            if "=" in line:
                k, v = ns.parse_spinsolve_par_line(line)
                acqu[k] = v
    return acqu


def spinsolve_oracle(d: Path) -> dict:
    import nmrglue as ng
    from nmrglue.fileio import spinsolve as ns
    warnings.simplefilter("ignore")
    names = sorted(f.name for f in d.iterdir() if f.is_file() and f.suffix.lower() in (".1d", ".2d", ".3d", ".4d"))
    files = [(n, header(d / n)) for n in names]
    files = [(n, hd) for n, hd in files if hd is not None and hd[0] in (501, 503, 504)]
    acqu = read_acqu(d)
    points = acqu.get("nrPnts")
    fid_name = None
    for want in FID_NAMES:
        for n, (dt, dims) in files:
            if n.lower() == want and dt in (501, 504) and (points is None or dims[0] == points):
                fid_name = n
                break
        if fid_name:
            break
    order = ([fid_name] if fid_name else []) + [n for n, _ in files if n != fid_name]
    hdr = dict(files)
    sw = float(acqu["bandwidth"]) * 1000 if "bandwidth" in acqu else None
    obs = float(acqu["b1Freq"]) if "b1Freq" in acqu else None
    dwell_ms = float(acqu["dwellTime"]) / 1000 if "dwellTime" in acqu else None
    traces = []
    cross = []
    for i, n in enumerate(order):
        dt, dims = hdr[n]
        x_n = dims[0]
        rows = int(np.prod([max(v, 1) for v in dims[1:]]))
        raw = np.frombuffer((d / n).read_bytes()[32:], "<f4")
        reader = "numpy (documented layout)"
        if dt == 504 and rows == 1 and len(raw) == 3 * x_n:
            dic, data = ns.read(str(d), specfile=n)
            x = np.asarray(dic["spectrum"]["xaxis"], dtype=np.float64)
            # nmrglue builds re + 1j·im, which turns a stored −0.0 imaginary part into +0.0; the
            # values are asserted equal and the stored floats (signed zeros kept) are hashed
            c = raw[x_n:3 * x_n].astype(np.float64)
            ys = [[c[0::2], c[1::2]]]
            assert np.array_equal(data.real, ys[0][0]) and np.array_equal(data.imag, ys[0][1]), n
            reader = f"nmrglue {ng.__version__} spinsolve.read (values equal; signed zeros as stored)"
        elif dt == 501:
            a = raw[: rows * x_n * 2].reshape(rows, x_n, 2).astype(np.float64)
            x = None
            ys = [[r[:, 0], r[:, 1]] for r in a]
        elif dt == 503 and rows == 1:
            x = raw[:x_n].astype(np.float64)
            ys = [[raw[x_n:2 * x_n].astype(np.float64)]]
        elif dt == 504 and rows == 1:
            x = raw[:x_n].astype(np.float64)
            c = raw[x_n:3 * x_n].astype(np.float64)
            ys = [[c[0::2], c[1::2]]]
        else:
            continue
        # depositor CSV exports of the same array: x, y (real) [, imag]
        csv = d / (Path(n).stem + ".csv")
        if csv.exists() and x is not None:
            t = np.loadtxt(csv, delimiter=",")
            cols = [x] + ys[0]
            for k, col in enumerate(cols):
                assert np.allclose(t[:, k], col, rtol=2e-5, atol=1e-5 * np.abs(col).max()), (csv, k)
            cross.append(csv.name)
        axis = None
        if x is not None and rows == 1:
            lin = linear(x)
            if lin is not None:
                f0, step = lin
                close = lambda a, b: abs(a - b) <= 1e-4 * max(abs(b), 1e-12)
                if dwell_ms and close(step, dwell_ms):
                    axis = "time"
                elif sw and obs and close(abs(step) * x_n, sw / obs):
                    axis = "ppm"
        is_fid = n == fid_name
        chans = ([] if (x is None or axis is not None) else ["x"]) + (["real", "imag"] if dt in (501, 504) else ["y"])
        sweeps = []
        for s, y in enumerate(ys):
            cols = ([] if (x is None or axis is not None) else [x]) + y
            sweeps.append({"sweep": s, "sample_count": int(x_n),
                           "channels": [{"xxh3": h(c), "first": first(c)} for c in cols]})
        e = {"index": i, "name": n, "sample_count": int(x_n), "sweep_count": rows,
             "channel_count": len(chans), "channel_names": chans, "reader": reader, "sweeps": sweeps}
        if is_fid:
            if sw:
                e["sample_rate_hz"] = sw
            udic = ns.guess_udic({"acqu": acqu, "dx": {}}, None)[0]
            ex = {"spectral_width_hz": float(udic["sw"]), "reference_frequency_mhz": float(udic["obs"]),
                  "carrier_offset_hz": float(udic["car"])}
            if "nucleus" in acqu:
                ex["nucleus"] = str(acqu["nucleus"])
            if "nrScans" in acqu:
                ex["scans"] = int(acqu["nrScans"])
            if "nrPnts" in acqu:
                ex["time_domain_size"] = int(acqu["nrPnts"])
            if "experiment" in acqu:
                ex["pulse_program"] = str(acqu["experiment"])
            if "specType" in acqu:
                ex["instrument"] = str(acqu["specType"])
            if "specID" in acqu:
                ex["instrument_serial"] = str(acqu["specID"])
            e["parameters"] = {"extra": ex}
        elif axis == "ppm":
            f0, step = linear(x)
            e["ppm_first"] = f0
            e["ppm_last"] = f0 + step * (x_n - 1)
        traces.append(e)
    out = {"reader": f"nmrglue {ng.__version__} (parameters, header, 504 single-row data); numpy (501/503 layout)",
           "traces": traces}
    if cross:
        out["cross_checked_with"] = cross
    return out


def plot_spectra(d: Path, acqu: dict):
    """Offsets of the software's processed spectra in `*.pt1` plot files: (file, [offset], n)."""
    if "bandwidth" not in acqu or "b1Freq" not in acqu:
        return []
    sw, obs = float(acqu["bandwidth"]) * 1000, float(acqu["b1Freq"])
    low = float(acqu.get("lowestFrequency", 0.0))
    out = []
    for p in sorted(d.glob("*.pt1")):
        if not (p.name == "spectrum.pt1" or p.name.endswith("-Spectra.pt1")):
            continue
        raw = p.read_bytes()
        found = []
        for n in (int(acqu.get("nrPnts", 0)) * int(acqu.get("zf", 1) or 1),):
            if n < 16:
                continue
            step = sw / obs / n
            for al in range(4):
                a = np.frombuffer(raw[al:al + (len(raw) - al) // 4 * 4], "<f4").astype(np.float64)
                with np.errstate(all="ignore"):
                    good = np.abs(np.diff(a) - step) < 1e-3 * step
                i = 0
                while i < len(good):
                    if good[i]:
                        j = i
                        while j < len(good) and good[j]:
                            j += 1
                        if j - i >= n - 1:
                            off = al + 4 * i
                            # the axis must start where the acquisition says (ppm of lowestFrequency)
                            if abs(a[i] - low / obs) < 2 * step and off + 12 * n <= len(raw):
                                found.append(off)
                        i = j
                    i += 1
            if found:
                out.append({"file": p.name, "points": n, "axis_offsets": sorted(found),
                            "axis_first_ppm": low / obs, "axis_step_ppm": step})
    return out


def main():
    m = tomllib.loads((ROOT / "corpus/manifest.toml").read_text())
    n_ok = 0
    for e in m["file"]:
        if e.get("format") != "magritek-spinsolve" or e.get("role") != "input":
            continue
        p = FILES / e["filename"]
        if not p.exists():
            print("missing", e["id"])
            continue
        d = p if p.is_dir() else p.parent
        data = spinsolve_oracle(d)
        (ROOT / "corpus/oracle" / f"{e['id']}.json").write_text(json.dumps(data, indent=1) + "\n")
        n_ok += 1
        acqu = read_acqu(d)
        spectra = plot_spectra(d, acqu)
        if spectra:
            script = (d / "processing.script").read_text(errors="replace") if (d / "processing.script").exists() else ""
            mt = re.search(r"Phase\(([-\d.eE+]+),\s*([-\d.eE+]+)\)", script)
            proc = {}
            if (d / "proc.par").exists():
                for line in (d / "proc.par").read_text(errors="replace").splitlines():
                    if "=" in line:
                        k, v = line.split("=", 1)
                        proc[k.strip()] = v.strip().strip('"')
            rec = {"id": e["id"], "path": e["filename"], "kind": "spinsolve",
                   "vendor_spectra": spectra,
                   "script_phase_deg": [float(mt.group(1)), float(mt.group(2))] if mt else None,
                   "acqu": {k: acqu.get(k) for k in ("bandwidth", "b1Freq", "lowestFrequency", "nrPnts", "zf", "filter", "filterType")},
                   "proc": proc}
            (ROOT / "corpus/oracle/nmr-processing" / f"{e['id']}.json").write_text(json.dumps(rec, indent=1) + "\n")
            print(e["id"], "vendor spectra:", [(s["file"], len(s["axis_offsets"])) for s in spectra])
    print(n_ok, "oracles written")


if __name__ == "__main__":
    main()
