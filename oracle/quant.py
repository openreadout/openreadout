#!/usr/bin/env python
"""Ground truth for `openreadout analyze chromatogram` and `openreadout analyze peaks` (book/src/guides/quantitation.md).

Usage (from oracle/, OPENREADOUT_CORPUS_DIR pointing at the corpus files):
  uv run python quant.py vendor [ID ...]   vendor peak tables (+ pyOpenMS and SciPy on ANDI signals)
  uv run python quant.py xic [ID ...]      TIC/BPC/XIC from the depositors' mzML (pyteomics)
                                           + pyOpenMS peak picking on each XIC
Without ids, every known file is written. Output: ../corpus/oracle/quant/<id>.json.

Nothing here reads a file with OpenReadout. The sources are:

- **Vendor integration results** stored with the raw data, parsed as text/XML/netCDF:
  - ANDI chromatography (`.cdf`) peak tables (`peak_retention_time`, `peak_area`, ...) written by
    the vendor data system that exported the file (Agilent ChemStation, Shimadzu LabSolutions),
    read with scipy.io.netcdf_file (BSD-3). The signal itself is read the same way for pyOpenMS.
  - ChemStation `Report.TXT` area-percent reports (Chromhandler, MIT) and `Result.xml` exports
    (GC2ASM, CeCILL-2.1): parsed here.
  - MSD ChemStation `RESULTS.CSV` integrator results of GC-MS total-ion chromatograms.
- **pyOpenMS** (BSD-3) `PeakPickerChromatogram` (automatic peak picking) and `PeakIntegrator`
  (trapezoidal area, base-to-base baseline, and peak-shape metrics) on the ANDI signals and on
  the XICs below; **SciPy** `find_peaks`/`peak_widths` as a second opinion on apexes and widths.
- **pyteomics** (Apache-2.0) reads the depositors' own mzML conversions of vendor raw files; the
  TIC and BPC are the spectra's `total ion current`/`base peak intensity` values and each XIC the
  sum of the intensities within ±10 ppm of the target in every MS1 spectrum (the same
  definition `openreadout analyze chromatogram` documents).
"""
import json, os, re, sys, tomllib
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files")).resolve()
OUT = ROOT / "corpus" / "oracle" / "quant"
PPM = 10.0


def manifest():
    with open(ROOT / "corpus" / "manifest.toml", "rb") as f:
        return {e["id"]: e for e in tomllib.load(f)["file"]}


def r(v, digits=10):
    """Round for JSON (keeps files small, far below any tolerance)."""
    if v is None:
        return None
    v = float(v)
    if not np.isfinite(v):
        return None
    return float(f"{v:.{digits}g}")


def write(id_, data):
    OUT.mkdir(parents=True, exist_ok=True)
    p = OUT / f"{id_}.json"
    p.write_text(json.dumps(data, indent=1) + "\n")
    print("wrote", p.relative_to(ROOT))


# ---------------------------------------------------------------- pyOpenMS / SciPy on a signal

def oms():
    import pyopenms  # noqa: F401  (heavy import, only when needed)
    return pyopenms


def chrom(t_s, y):
    ms = oms()
    c = ms.MSChromatogram()
    c.set_peaks((np.asarray(t_s, dtype=float), np.asarray(y, dtype=float)))
    return c


def integrate(t_s, y, left_s, right_s):
    """pyOpenMS PeakIntegrator: trapezoid, base-to-base; plus its peak-shape metrics."""
    ms = oms()
    pi = ms.PeakIntegrator()
    p = pi.getDefaults()
    p.setValue("integration_type", "trapezoid")
    p.setValue("baseline_type", "base_to_base")
    pi.setParameters(p)
    c = chrom(t_s, y)
    pa = pi.integratePeak(c, left_s, right_s)
    bg = pi.estimateBackground(c, left_s, right_s, pa.apex_pos)
    # peak-shape metrics on the baseline-corrected signal (the same base-to-base line), so that
    # widths are measured at fractions of the height above the baseline
    t_a, y_a = np.asarray(t_s, dtype=float), np.asarray(y, dtype=float)
    il, ir = int(np.searchsorted(t_a, left_s - 1e-9)), int(np.searchsorted(t_a, right_s + 1e-9)) - 1
    base = y_a[il] + (y_a[ir] - y_a[il]) * (t_a - t_a[il]) / (t_a[ir] - t_a[il] if ir > il else 1.0)
    sm = pi.calculatePeakShapeMetrics(chrom(t_a, y_a - base), left_s, right_s, pa.height - bg.height, pa.apex_pos)
    return {
        "left_s": r(left_s), "right_s": r(right_s),
        "area_raw": r(pa.area), "background_area": r(bg.area), "area": r(pa.area - bg.area),
        "height_raw": r(pa.height), "background_height": r(bg.height),
        "apex_s": r(pa.apex_pos),
        "width_at_5_s": r(sm.width_at_5), "width_at_10_s": r(sm.width_at_10),
        "width_at_50_s": r(sm.width_at_50), "tailing_factor": r(sm.tailing_factor),
        "asymmetry_factor": r(sm.asymmetry_factor),
    }


def pick(t_s, y, sn=1.0):
    """pyOpenMS PeakPickerChromatogram (formerly PeakPickerMRM), SG smoothing, then each picked
    peak re-integrated with PeakIntegrator between the picker's boundaries."""
    ms = oms()
    pp = ms.PeakPickerChromatogram()
    p = pp.getDefaults()
    p.setValue("use_gauss", "false")
    p.setValue("signal_to_noise", float(sn))
    p.setValue("method", "corrected")
    pp.setParameters(p)
    picked = ms.MSChromatogram()
    c = chrom(t_s, y)
    pp.pickChromatogram(c, picked)
    fda = {a.getName(): list(a) for a in picked.getFloatDataArrays()}
    out = []
    rts, ints = picked.get_peaks()
    for k, (rt, it) in enumerate(zip(rts, ints)):
        lw, rw = fda["leftWidth"][k], fda["rightWidth"][k]
        e = {"apex_s": r(rt), "picked_intensity": r(it), "left_s": r(lw), "right_s": r(rw),
             "integrated_intensity": r(fda.get("IntegratedIntensity", [None] * (k + 1))[k])}
        try:
            e["trapezoid"] = integrate(t_s, y, lw, rw)
        except Exception as ex:  # noqa: BLE001
            e["trapezoid_error"] = str(ex)
        out.append(e)
    return out


def scipy_peaks(t_s, y, prominence):
    from scipy.signal import find_peaks, peak_widths
    y = np.asarray(y, dtype=float)
    idx, props = find_peaks(y, prominence=prominence)
    w = peak_widths(y, idx, rel_height=0.5, prominence_data=(props["prominences"], props["left_bases"], props["right_bases"]))
    step = float(np.median(np.diff(t_s))) if len(t_s) > 1 else 0.0
    return [{"apex_s": r(t_s[i]), "height": r(y[i]), "prominence": r(pr), "fwhm_s": r(wd * step)}
            for i, pr, wd in zip(idx, props["prominences"], w[0])]


# ---------------------------------------------------------------- vendor tables

def andi(id_, e):
    from scipy.io import netcdf_file
    path = CORPUS / e["filename"]
    with netcdf_file(path, "r", mmap=False) as n:
        v = n.variables
        a = n._attributes
        dec = lambda b: b.decode("latin-1").strip() if isinstance(b, bytes) else str(b)  # noqa: E731
        y = np.array(v["ordinate_values"][:], dtype=float)
        dt = float(v["actual_sampling_interval"].getValue())
        delay = float(v["actual_delay_time"].getValue()) if "actual_delay_time" in v else 0.0
        t_s = delay + dt * np.arange(len(y))
        unit_t = dec(a.get(b"retention_unit", a.get("retention_unit", b"seconds"))).lower()
        k = 60.0 if unit_t.startswith("min") else 1.0
        def col(name):
            return [float(x) for x in v[name][:]] if name in v else None
        def txt(name):
            if name not in v:
                return None
            return [bytes(x).rstrip(b"\x00").decode("latin-1").strip() for x in v[name][:]]
        rt, st, en = col("peak_retention_time"), col("peak_start_time"), col("peak_end_time")
        area, height = col("peak_area"), col("peak_height")
        bs, be = col("baseline_start_value"), col("baseline_stop_value")
        bst, bet = col("baseline_start_time"), col("baseline_stop_time")
        codes_s, codes_e, names = txt("peak_start_detection_code"), txt("peak_stop_detection_code"), txt("peak_name")
        software = "Shimadzu LabSolutions" if b"LabSolutions" in (a.get("raw_data_table_name", b"") + a.get("detection_method_name", b"")) else "Agilent ChemStation"
        det_unit = dec(a.get("detector_unit", b""))
    peaks = []
    for i in range(len(rt)):
        peaks.append({
            "rt_min": r(rt[i] * k / 60), "start_min": r(st[i] * k / 60), "end_min": r(en[i] * k / 60),
            "area": r(area[i]), "height": r(height[i]),
            "baseline_start_min": r(bst[i] * k / 60) if bst else None,
            "baseline_end_min": r(bet[i] * k / 60) if bet else None,
            "baseline_start": r(bs[i]) if bs else None, "baseline_end": r(be[i]) if be else None,
            "code": ((codes_s[i] if codes_s else "") .strip() + (codes_e[i] if codes_e else "").strip()) or None,
            "name": (names[i] or None) if names else None,
        })
    # The data system reports heights and areas in its own scale of the detector unit (LabSolutions:
    # µV and µV·s for a signal stored in volts). The factor is the power of ten that maps the
    # vendor heights onto the signal's peak maxima above the vendor's own baselines.
    ratios = []
    for p in peaks:
        m = (t_s / 60 >= p["start_min"]) & (t_s / 60 <= p["end_min"])
        if not m.any() or p["baseline_start"] is None:
            continue
        i = int(np.argmax(np.where(m, y, -np.inf)))
        f = (t_s[i] / 60 - p["start_min"]) / max(p["end_min"] - p["start_min"], 1e-12)
        b = p["baseline_start"] + (p["baseline_end"] - p["baseline_start"]) * f
        if y[i] - b > 0 and p["height"] > 0:
            ratios.append(p["height"] / (y[i] - b))
    scale = 10.0 ** round(np.log10(np.median(ratios))) if ratios else 1.0
    for p in peaks:
        for key in ("baseline_start", "baseline_end"):
            if p[key] is not None and scale != 1.0 and abs(p[key]) < 1e3:
                pass  # baselines are in the signal's own unit in both corpus exports
    out = {
        "id": id_, "input": e["filename"], "signal": {"trace": 0},
        "vendor": {
            "software": software, "source": "ANDI peak table (peak_* variables) read with scipy.io.netcdf_file",
            "time_unit": "min", "signal_unit": det_unit,
            "height_scale": r(scale),
            "area_to_signal_s": r(1.0 / scale),
            "note": "vendor area / height_scale = signal·s; vendor height / height_scale = signal",
        },
        "peaks": peaks,
    }
    # pyOpenMS with the vendor's boundaries (sample times nearest the vendor's start/end)
    forced = []
    for p in peaks:
        il = int(np.argmin(np.abs(t_s / 60 - p["start_min"])))
        ir = int(np.argmin(np.abs(t_s / 60 - p["end_min"])))
        forced.append(integrate(t_s, y, t_s[il], t_s[ir]) if ir > il else None)
    out["pyopenms_forced"] = forced
    # PeakPickerChromatogram's noise estimator bins intensities: give it the vendor's scale
    # (µV rather than V), then scale areas and heights back to the signal unit.
    picked = pick(t_s, y * scale, sn=1.0)
    for q in picked:
        q["picked_intensity"] = r(q["picked_intensity"] / scale) if q["picked_intensity"] is not None else None
        if "trapezoid" in q:
            for key in ("area_raw", "background_area", "area", "height_raw", "background_height"):
                if q["trapezoid"][key] is not None:
                    q["trapezoid"][key] = r(q["trapezoid"][key] / scale)
    out["pyopenms_picked"] = picked
    noise = float(np.median(np.abs(np.diff(y)))) + 1e-12
    out["scipy"] = scipy_peaks(t_s, y, prominence=max(float(np.ptp(y)) * 1e-3, 10 * noise))
    return out


def report_txt(id_, e, manifest_entries):
    """ChemStation area-percent Report.TXT (UTF-16): one table per signal."""
    d = CORPUS / e["filename"]
    b = (d / "Report.TXT").read_bytes()
    t = b.decode("utf-16") if b[:2] in (b"\xff\xfe", b"\xfe\xff") else b.decode("latin-1")
    sections = re.split(r"\n\s*Signal \d+: ", t)
    signals = []
    for s in sections[1:]:
        head = s.split("\n", 1)[0].strip().rstrip(",").strip()  # "FID1 A"
        units = re.search(r"\[min\]\s+\[([^\]]+)\]\s+\[([^\]]+)\]", s)
        rows = []
        for line in s.split("\n"):
            m = re.match(r"\s*(\d+)\s+([\d.]+)\s+([A-Z]{2,4}(?: [A-Z])?)\s+([\d.]+)\s+([\d.eE+-]+)\s+([\d.eE+-]+)\s+([\d.eE+-]+)", line)
            if m:
                rows.append({"rt_min": r(m.group(2)), "code": m.group(3).strip(), "width_min": r(m.group(4)),
                             "area": r(m.group(5)), "height": r(m.group(6)), "area_percent": r(m.group(7))})
            if line.startswith("Totals"):
                break
        det = head.replace(" ", "")  # FID1A
        if not rows or any(x["trace_name"] == det for x in signals):
            continue  # the summed-peaks sections repeat the signal names without rows
        signals.append({"signal": head, "trace_name": det, "area_unit": units.group(1) if units else None,
                        "height_unit": units.group(2) if units else None, "peaks": rows})
    return {
        "id": id_, "input": e["filename"],
        "vendor": {"software": "Agilent ChemStation", "source": "Report.TXT (area-percent report)",
                   "time_unit": "min", "area_to_signal_s": 1.0,
                   "note": "areas in signal·s ([pA*s], [25 uV*s]); widths are ChemStation's peak widths"},
        "signals": signals,
    }


def result_xml(id_, e):
    """ChemStation Result.xml export (UTF-16 XML): IntegrationResults per Signal, compound names."""
    import xml.etree.ElementTree as ET
    d = CORPUS / e["filename"]
    root = ET.fromstring((d / "Result.xml").read_bytes())
    names = {}
    for p in root.iter("Peak"):
        desc = (p.findtext("SignalDesc") or "").strip()
        rt = p.findtext("MeasRetTime")
        if rt:
            names[(desc, round(float(rt), 4))] = (p.findtext("Name") or "").strip()
    signals = []
    for s in root.iter("Signal"):
        desc = (s.findtext("Description") or "").strip()
        det = (s.findtext("Detector") or "") + (s.findtext("SignalId") or "")
        rows = []
        for ir in s.findall("IntegrationResults"):
            g = lambda k: ir.findtext(k)  # noqa: E731
            rt = float(g("RetTime"))
            rows.append({"rt_min": r(rt), "start_min": r(g("TimeStart")), "end_min": r(g("TimeEnd")),
                         "area": r(g("Area")), "height": r(g("Height")), "width_min": r(g("Width")),
                         "area_percent": r(g("AreaPercent")), "symmetry": r(g("Symmetry")),
                         "baseline_start": r(g("BaselineStart")), "baseline_end": r(g("BaselineEnd")),
                         "name": names.get((desc, round(rt, 4))) or None})
        if rows:
            signals.append({"signal": desc, "trace_name": det, "x_units": s.findtext("XUnits"),
                            "y_units": s.findtext("YUnits"), "peaks": rows})
    return {
        "id": id_, "input": e["filename"],
        "vendor": {"software": "Agilent ChemStation", "source": "Result.xml (ChemStation XML export)",
                   "time_unit": "min", "area_to_signal_s": 1.0, "note": "areas in signal·s"},
        "signals": signals,
    }


def results_csv(id_, e):
    """MSD ChemStation RESULTS.CSV: integrator results of the total-ion chromatogram."""
    p = CORPUS / e["filename"]
    rows = []
    for line in p.read_text(errors="replace").splitlines():
        m = re.match(r"\d+=,\s*(\d+),\s*([\d.]+),\s*(\d+),\s*(\d+),\s*(\d+),\"([^\"]*)\",\s*(\d+),\s*(\d+),\s*([\d.]+),\s*([\d.]+)", line)
        if m:
            rows.append({"rt_min": r(m.group(2)), "first_scan": int(m.group(3)), "max_scan": int(m.group(4)),
                         "last_scan": int(m.group(5)), "code": m.group(6).strip() or None,
                         "height": r(m.group(7)), "area": r(m.group(8)), "percent_of_max": r(m.group(9)),
                         "area_percent": r(m.group(10))})
    return {
        "id": id_, "input": e["filename"].replace("RESULTS.CSV", "data.ms"), "signal": {"tic": True},
        "vendor": {"software": "MSD ChemStation (MassHunter GC/MS)", "source": "RESULTS.CSV",
                   "time_unit": "min", "area_to_signal_s": None,
                   "note": "areas in the integrator's own units: compare area % (area_percent)"},
        "peaks": rows,
    }


VENDOR = {
    "andi": lambda m: [i for i, e in m.items() if e["format"] == "andi-chrom" and (i.startswith("mtbls1892-") or i == "cheminfo-agilent-hplc-cdf")],
    "report": lambda m: [f"chromhandler-001f010{k}-d" for k in range(1, 5)],
    "xml": lambda m: ["gc2asm-v181-d", "gc2asm-three-channels-d"],
    "csv": lambda m: [i for i in m if i.startswith("chromhandler-rau-r505-") and i.endswith("-results-csv")],
}


def vendor(ids):
    m = manifest()
    todo = []
    for kind, f in VENDOR.items():
        for i in f(m):
            if not ids or i in ids:
                todo.append((kind, i))
    for kind, i in todo:
        e = m[i]
        if kind == "andi":
            write(i, andi(i, e))
        elif kind == "report":
            write(i, report_txt(i, e, m))
        elif kind == "xml":
            write(i, result_xml(i, e))
        else:
            write(i.replace("-results-csv", ""), results_csv(i, e))


# ---------------------------------------------------------------- XIC from depositor mzML

# (oracle id, the vendor raw input to extract from, the depositor's mzML)
XIC = [
    ("mtbls20-caffeine-pos", "mtbls20-caffeine-pos.raw", "mtbls20-caffeine-pos.mzML"),
    ("mtbls404-QC1_001", "mtbls404-QC1_001.raw", "mtbls404-QC1_001.mzML"),
    ("mtbls1820-lumos-uplc-31", "mtbls1820-lumos-uplc-31.raw", "mtbls1820-lumos-uplc-31.mzML"),
    ("mtbls755-hilic-dpoly", "mtbls755-hilic-dpoly.raw", "mtbls755-hilic-dpoly.mzML"),
    ("mtbls7386-DS017_KO2_3_C18MSpos_IO29_20250129",
     "mtbls7386-DS017_KO2_3_C18MSpos_IO29_20250129-d/DS017_KO2_3_C18MSpos_IO29_20250129.d",
     "mtbls7386-DS017_KO2_3_C18MSpos_IO29_20250129.mzML"),
    ("mtbls7290-scfa240-001-neg-blank-raw",
     "mtbls7290-scfa240-001-neg-blank-raw-zip/DIME__SCFA240_20220530_SSCFA_001_neg_Blank.raw",
     "mtbls7290-scfa240-001-neg-blank-raw.mzML"),
    # not mtbls11360 (Sciex TripleTOF): its export holds vendor-centroided spectra, which the
    # reader does not reproduce (it returns the calibrated TDC profiles), so XICs differ by design
]


def xic_one(id_, raw, mzml_name):
    from pyteomics import mzml
    rows = []
    with mzml.MzML(str(CORPUS / mzml_name), decode_binary=True, use_index=True) as f:
        for s in f:
            if s.get("ms level") != 1:
                continue
            scan = s["scanList"]["scan"][0]
            rt = float(scan["scan start time"])
            unit = getattr(scan["scan start time"], "unit_info", "minute")
            rt_min = rt if str(unit).startswith("min") else rt / 60.0
            pol = "positive" if "positive scan" in s else ("negative" if "negative scan" in s else "unknown")
            mz = np.asarray(s["m/z array"], dtype=float)
            it = np.asarray(s["intensity array"], dtype=float)
            rows.append((rt_min, pol, s.get("total ion current"), s.get("base peak intensity"), mz, it,
                         "centroid spectrum" in s))
    rows.sort(key=lambda x: x[0])
    # targets: the five most intense distinct m/z of the spectrum with the largest TIC
    k = int(np.argmax([x[2] if x[2] is not None else x[5].sum() for x in rows]))
    mz, it = rows[k][4], rows[k][5]
    targets = []
    for i in np.argsort(-it):
        m = float(mz[i])
        if all(abs(m - t) > 0.05 for t in targets):
            targets.append(m)
        if len(targets) == 3:
            break
    targets = [round(t, 4) for t in targets]
    rt = [r(x[0]) for x in rows]
    xics = []
    for tmz in targets:
        d = tmz * PPM * 1e-6
        vals = []
        for x in rows:
            lo, hi = np.searchsorted(x[4], tmz - d, "left"), np.searchsorted(x[4], tmz + d, "right")
            vals.append(r(float(x[5][lo:hi].sum())))
        t_s = np.array(rt, dtype=float) * 60
        xics.append({"mz": tmz, "ppm": PPM, "intensity": vals,
                     "pyopenms_picked": pick(t_s, np.array(vals, dtype=float), sn=1.0)})
    return {
        "id": id_, "input": raw, "export": mzml_name,
        "source": "pyteomics reading the depositor's mzML; XIC = sum of intensities within ±10 ppm per MS1 spectrum",
        "centroided": bool(all(x[6] for x in rows)),
        "polarities": sorted({x[1] for x in rows}),
        "ms1_scans": len(rows),
        "rt_min": rt,
        "tic": [r(x[2]) for x in rows],
        "bpc": [r(x[3]) for x in rows],
        "xics": xics,
    }


def xic(ids):
    for id_, raw, mz in XIC:
        if ids and id_ not in ids:
            continue
        if not (CORPUS / mz).exists() or not (CORPUS / raw).exists():
            print("skip (missing)", id_)
            continue
        write(f"{id_}.xic", xic_one(id_, raw, mz))


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in ("vendor", "xic"):
        sys.exit(__doc__)
    {"vendor": vendor, "xic": xic}[sys.argv[1]](set(sys.argv[2:]))
