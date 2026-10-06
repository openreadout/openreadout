"""Held-out draw D (2026-10-06): mass spectrometry and chromatography.

Part of evals/heldout_draw_d.py; the questions were written from the held-out oracles, the
depositors' exports and records, and facts computed with third-party readers, before OpenReadout
was run on any of these files.

Vendor files (Thermo, Agilent, Waters, Sciex) are answered from the depositor's open export of the
same run; timsTOF files from their SQLite tables (Python sqlite3 on a copy); ChemStation files from
the vendor's own integration report; ANDI files with SciPy. Values transcribed from a vendor PDF
report say so in their source.
"""

from __future__ import annotations

from typing import Any

C: dict[str, str] = {
    # mass spectrometry: vendor files
    "d_ms_altis": "ho-mtbls12457-raw-altis",
    "d_ms_elite": "ho-mtbls4415-raw-elite",
    "d_ms_isq": "ho-mtbls1104-raw-isq",
    "d_ms_qeplus": "ho-mtbls11030-raw-qeplus",
    "d_ms_6495": "ho-mtbls15503-d-6495",
    "d_ms_6545": "ho-mtbls13906-d-6545",
    "d_ms_reims": "ho-mtbls11776-raw-reims",
    "d_ms_xevotq": "ho-mtbls11394-raw-xevotq",
    "d_ms_zenotof": "ho-mtbls14098-wiff-zenotof",
    "d_ms_tt6600": "ho-mtbls14234-wiff-tt6600",
    "d_ms_tdf_blank": "ho-mtbls14519-tdf-blank",
    "d_ms_tdf_prm": "ho-mtbls14813-tdf-prm",
    # their frame blobs (companions), staged next to analysis.tdf in a `.d` folder
    "d_ms_tdf_blank_bin": "ho-mtbls14519-tdf-blank-bin",
    "d_ms_tdf_prm_bin": "ho-mtbls14813-tdf-prm-bin",
    # mass spectrometry: open formats
    "d_ms_mzml_sim": "ho-mtbls15607-mzml-isq7000",
    "d_ms_mzml_avg": "ho-figshare8038448-mzml-avg",
    "d_ms_mzxml_qc": "ho-zenodo17020981-mzxml-qc",
    # chromatography
    "d_ms_tcd": "ho-gh-jarweile-ch-tcd",
    "d_ms_perfume": "ho-zenodo3249821-gcms-212sexy",
    "d_ms_cdf_faah": "ho-zenodo19606563-cdf-faah",
    "d_ms_cdf_gcms": "ho-figshare8038448-cdf-gcms",
    "d_ms_cdf_tq8040": "ho-mtbls2217-cdf-tq8040",
    "d_ms_lcd": "ho-figshare31095109-lcd-lumefantrine",
    "d_ms_openlab": "ho-gh-cloudbywu-dx-hplc",
    "d_ms_arw": "ho-figshare30962618-arw-standard",
}


# ---------------------------------------------------------------- facts (oracle venv only)


def _h():
    import heldout

    return heldout


def _export_spectra(cid: str, role: str = "oracle-export") -> list[dict]:
    return _h().ms_spectra(cid, role)


def tic_apex(cid: str, role: str = "oracle-export", from_cvparam: bool = False):
    """Retention time (min) of the MS1 spectrum with the largest summed intensity (pyteomics)."""
    return _h().ms_tic_apex(cid, role, from_cvparam)


def mzxml_attr_tic_apex(cid: str, role: str = "oracle-export"):
    """The same from each scan's `totIonCurrent` attribute (pyteomics mzxml, MS1 only)."""
    from pyteomics import mzxml

    with mzxml.MzXML(str(_h().hpath(cid, role))) as r:
        best = max((s for s in r if int(s["msLevel"]) == 1), key=lambda s: float(s["totIonCurrent"]))
    return round(float(best["retentionTime"]), 4), _h().rd("pyteomics") + " (`totIonCurrent` attributes)"


def reims_base_peak(cid: str, attr: bool = False):
    """m/z of the most intense point of the scan with the largest summed intensity (REIMS export)."""
    from pyteomics import mzxml

    with mzxml.MzXML(str(_h().hpath(cid, "oracle-export"))) as r:
        scans = list(r)
    best = max(scans, key=lambda s: float(s["intensity array"].sum()))
    if attr:
        return round(float(best["basePeakMz"]), 2), _h().rd("pyteomics") + " (`basePeakMz` attribute of that scan)"
    return round(float(best["m/z array"][best["intensity array"].argmax()]), 2), _h().rd("pyteomics")


def strongest_transition(cid: str, role: str = "oracle-export"):
    """`precursor > product` of the SRM chromatogram with the largest summed intensity (pyteomics)."""
    from pyteomics import mzml

    sums = {}
    with mzml.MzML(str(_h().hpath(cid, role))) as r:
        for c in r.iterfind("chromatogram"):
            if "selected reaction monitoring chromatogram" not in c:
                continue
            pre, pro = c["precursor"], c["product"]
            pre = pre[0] if isinstance(pre, list) else pre
            pro = pro[0] if isinstance(pro, list) else pro
            q1 = pre["isolationWindow"]["isolation window target m/z"]
            q3 = pro["isolationWindow"]["isolation window target m/z"]
            sums[f"{q1:g} > {q3:g}"] = float(c["intensity array"].sum())
    s = sorted(sums, key=sums.get)
    assert sums[s[-1]] > 1.2 * sums[s[-2]], (s[-1], s[-2])
    return s[-1], _h().rd("pyteomics")


def sim_apex_min(cid: str, chrom_id: str):
    """Time (min) of the largest value of one SIM chromatogram (pyteomics)."""
    from pyteomics import mzml

    with mzml.MzML(str(_h().hpath(cid))) as r:
        c = r.get_by_id(chrom_id)
    t = c["time array"]
    unit = getattr(t, "unit_info", "minute")
    v = float(t[int(c["intensity array"].argmax())])
    return round(v / 60.0 if unit == "second" else v, 3), _h().rd("pyteomics")


def distinct_precursors(cid: str):
    sp = _export_spectra(cid)
    return len({round(s["precursor"], 2) for s in sp if s["level"] == 2 and s["precursor"] is not None}), _h().rd(
        "pyteomics"
    ) + " on the depositor's mzML"


def zenotof_precursors(cid: str):
    """Distinct precursor m/z (to 0.01) of the MS2 spectra of the depositor's mzML (pyteomics)."""
    from pyteomics import mzml

    seen = set()
    with mzml.MzML(str(_h().hpath(cid, "oracle-export"))) as r:
        for s in r:
            if s.get("ms level") == 2:
                ion = s["precursorList"]["precursor"][0]["selectedIonList"]["selectedIon"][0]
                seen.add(round(float(ion["selected ion m/z"]), 2))
    return len(seen), _h().rd("pyteomics") + " on the depositor's mzML"


def tdf_query(cid: str, sql: str):
    """One value from the analysis.tdf SQLite tables (Python sqlite3 on a copy)."""
    import shutil
    import sqlite3
    import tempfile

    tmp = tempfile.mkdtemp()
    shutil.copy(_h().hpath(cid), tmp)
    with sqlite3.connect(f"{tmp}/analysis.tdf") as c:
        (v,) = c.execute(sql).fetchone()
    shutil.rmtree(tmp)
    import sqlite3 as s3

    return v, f"Python sqlite3 (SQLite {s3.sqlite_version}) on a copy of analysis.tdf"


def tdf_ms1_tic_apex_min(cid: str):
    v, how = tdf_query(cid, "select Time from Frames where MsMsType = 0 order by SummedIntensities desc limit 1")
    return round(float(v) / 60.0, 3), how + " (Frames.SummedIntensities of MS1 frames)"


def report_largest_area_rt(cid: str):
    """Retention time of the largest-area peak of a ChemStation area-percent report (Report.TXT)."""
    import re

    text = _h().hpath(cid, "oracle-export").read_bytes().decode("utf-16")
    rows = [
        m.groups()
        for m in re.finditer(r"^\s*(\d+)\s+(\d+\.\d+)\s+\S+\s+(\d+\.\d+)\s+(\d+\.\d+)\s+(\d+\.\d+)", text, re.M)
    ]
    rows = sorted(rows, key=lambda r: -float(r[3]))
    assert float(rows[0][3]) > 1.2 * float(rows[1][3]), rows[:2]
    return float(rows[0][1]), "the ChemStation area-percent report (Report.TXT), Python re"


def results_csv_largest_area_rt(cid: str):
    """Retention time of the largest-area TIC peak in MSD ChemStation's RESULTS.CSV (INT TIC block)."""
    import csv

    lines = _h().hpath(cid, "oracle-export").read_text(errors="replace").splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith("[INT TIC"))
    peaks = []
    for line in lines[start + 3 :]:
        if line.startswith("["):
            break
        row = next(csv.reader([line.split("=", 1)[1]]))
        peaks.append((float(row[2]), float(row[8])))  # (empty), peak, R.T., first, max, last, type, height, area
    peaks.sort(key=lambda p: -p[1])
    assert peaks[0][1] > 1.2 * peaks[1][1], peaks[:2]
    return peaks[0][0], "MSD ChemStation integration (RESULTS.CSV, INT TIC block), Python csv"


def andi_tic_apex(cid: str, summed: bool = False):
    import heldout_draw_c as dc

    return dc.andi_tic_apex_min(cid, summed)


def andi_date(cid: str):
    """YYYY-MM-DD of an ANDI file's experiment_date_time_stamp written as `YYYY,MM,DD,hh:mm:ss+zzzz`."""
    import re

    from scipy.io import netcdf_file

    with netcdf_file(_h().hpath(cid), "r", mmap=False) as f:
        stamp = f.experiment_date_time_stamp.decode()
    y, mo, d = re.match(r"(\d{4}),(\d{2}),(\d{2}),", stamp).groups()
    return f"{y}-{mo}-{d}", _h().rd("scipy") + f" netcdf_file ({stamp!r})"


def arw_apex_min(cid: str, csvmod: bool = False):
    """Time (min) of the largest absorbance of a headerless Empower export."""
    import numpy as np

    text = _h().hpath(cid).read_bytes().decode("latin-1").replace("\r\n", "\n").replace("\r", "\n")
    if csvmod:
        import csv

        rows = [(float(a), float(b)) for a, b in csv.reader(text.splitlines(), delimiter="\t")]
        t, v = zip(*rows, strict=True)
        i = max(range(len(v)), key=v.__getitem__)
        return round(t[i], 3), "Python csv on the export text"
    a = np.loadtxt(text.splitlines(), delimiter="\t")
    return round(float(a[int(a[:, 1].argmax()), 0]), 3), _h().rd("numpy") + " loadtxt on the export text"


def transcribed(value, what: str):
    """A value read off a vendor PDF report (pypdf 6.19 text extraction) and transcribed here."""
    return lambda: (value, f"transcribed from {what} (text extracted with pypdf 6.19.0)")


def facts(Fact, H) -> list:
    return [
        Fact(
            C["d_ms_altis"],
            "strongest_transition",
            "SRM transition (precursor > product m/z) with the largest summed intensity (depositor's mzML)",
            lambda: strongest_transition(C["d_ms_altis"]),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_elite"],
            "ms1_tic_apex_min",
            "retention time (min) of the MS1 scan with the largest summed intensity (depositor's mzXML)",
            lambda: tic_apex(C["d_ms_elite"]),
            lambda: mzxml_attr_tic_apex(C["d_ms_elite"]),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_isq"],
            "tic_apex_min",
            "retention time (min) of the scan with the largest summed intensity (depositor's mzXML)",
            lambda: tic_apex(C["d_ms_isq"]),
            lambda: mzxml_attr_tic_apex(C["d_ms_isq"]),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_6545"],
            "tic_apex_min",
            "retention time (min) of the spectrum with the largest summed intensity (depositor's mzML)",
            lambda: tic_apex(C["d_ms_6545"]),
            lambda: tic_apex(C["d_ms_6545"], from_cvparam=True),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_reims"],
            "base_peak_mz",
            "m/z of the most intense point of the scan with the largest summed intensity (depositor's mzXML)",
            lambda: reims_base_peak(C["d_ms_reims"]),
            lambda: reims_base_peak(C["d_ms_reims"], attr=True),
            role="oracle-export",
            rel=1e-4,
        ),
        Fact(
            C["d_ms_zenotof"],
            "distinct_precursors",
            "distinct precursor m/z values (to 0.01) among the MS2 spectra (depositor's mzML)",
            lambda: zenotof_precursors(C["d_ms_zenotof"]),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_tdf_blank"],
            "precursors",
            "rows of the Precursors table (precursors selected for PASEF MS/MS)",
            lambda: tdf_query(C["d_ms_tdf_blank"], "select count(*) from Precursors"),
        ),
        Fact(
            C["d_ms_tdf_blank"],
            "ms1_frames",
            "frames with MsMsType 0 (MS1)",
            lambda: tdf_query(C["d_ms_tdf_blank"], "select count(*) from Frames where MsMsType = 0"),
        ),
        Fact(
            C["d_ms_tdf_prm"],
            "prm_targets",
            "rows of the PrmTargets table",
            lambda: tdf_query(C["d_ms_tdf_prm"], "select count(*) from PrmTargets"),
        ),
        Fact(
            C["d_ms_tdf_prm"],
            "instrument",
            "GlobalMetadata InstrumentName",
            lambda: tdf_query(C["d_ms_tdf_prm"], "select Value from GlobalMetadata where Key = 'InstrumentName'"),
        ),
        Fact(
            C["d_ms_mzml_sim"],
            "sim74_apex_min",
            "time (min) of the largest value of the m/z 74 SIM chromatogram",
            lambda: sim_apex_min(C["d_ms_mzml_sim"], "SIM SIC 74"),
        ),
        Fact(
            C["d_ms_mzml_avg"],
            "base_peak_mz",
            "m/z of the most intense point of the file's one (averaged) spectrum",
            lambda: H.ms_base_peak_mz(C["d_ms_mzml_avg"]),
        ),
        Fact(
            C["d_ms_tcd"],
            "largest_area_rt",
            "retention time (min) of the largest peak by area in the ChemStation report of the run",
            lambda: report_largest_area_rt(C["d_ms_tcd"]),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_perfume"],
            "largest_area_rt",
            "retention time (min) of the largest TIC peak by area in the MSD ChemStation integration",
            lambda: results_csv_largest_area_rt(C["d_ms_perfume"]),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_cdf_faah"],
            "acquired_date",
            "acquisition date in the file's experiment_date_time_stamp attribute",
            lambda: andi_date(C["d_ms_cdf_faah"]),
        ),
        Fact(
            C["d_ms_cdf_gcms"],
            "tic_apex_min",
            "scan time (min) of the largest total ion current",
            lambda: andi_tic_apex(C["d_ms_cdf_gcms"]),
            lambda: andi_tic_apex(C["d_ms_cdf_gcms"], summed=True),
        ),
        Fact(
            C["d_ms_cdf_tq8040"],
            "tic_apex_min",
            "scan time (min) of the largest total ion current",
            lambda: andi_tic_apex(C["d_ms_cdf_tq8040"]),
            lambda: andi_tic_apex(C["d_ms_cdf_tq8040"], summed=True),
        ),
        Fact(
            C["d_ms_openlab"],
            "largest_area_rt_220",
            "retention time (min) of the largest peak by area in the 220 nm signal, from the vendor's report",
            transcribed(3.218, "the OpenLab CDS report -S-001_1.pdf (DAD1A, Sig=220,4 peak table)"),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_openlab"],
            "dad_wavelengths",
            "signal wavelengths (nm) of the diode-array signals, from the vendor's report",
            transcribed([220, 254], "the OpenLab CDS report -S-001_1.pdf (DAD1A Sig=220,4; DAD1B Sig=254,4)"),
            role="oracle-export",
        ),
        Fact(
            C["d_ms_arw"],
            "apex_min",
            "time (min) of the largest absorbance in the export",
            lambda: arw_apex_min(C["d_ms_arw"]),
            lambda: arw_apex_min(C["d_ms_arw"], csvmod=True),
        ),
    ]


def specs(spec, fact, g, H) -> list[Any]:
    unit_hint = "a number with its unit"

    def ms_level(o: dict, level: int) -> int:
        return sum(1 for s in o["spectra"]["scans"] if s["ms_level"] == level)

    def srm(o: dict) -> list[dict]:
        return [c for c in o["chromatograms"] if c.get("kind") == "srm"]

    return [
        # ------------------------------------------------ Thermo
        spec(
            "ho-d-ms-raw-altis-transitions",
            "d_ms_altis",
            "counts",
            "How many SRM transitions (selected-reaction chromatograms) does this triple-quadrupole run record?",
            lambda o, m: g.integer(len(srm(o))),
            "oracle: /chromatograms of kind srm (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-d-ana-ms-raw-altis-strongest",
            "d_ms_altis",
            "analysis",
            "Which SRM transition (precursor m/z > product m/z) has the largest total signal summed over the run?",
            fact("strongest_transition", lambda v: g.string(v, accept=[v.replace(" > ", "->"), v.replace(" > ", "/")])),
            "facts-heldout: strongest_transition (pyteomics on the depositor's mzML)",
            answer_hint="precursor m/z > product m/z",
        ),
        spec(
            "ho-d-ms-raw-elite-ms2",
            "d_ms_elite",
            "counts",
            "How many MS/MS (MS2) scans does this run contain?",
            lambda o, m: g.integer(ms_level(o, 2)),
            "oracle: /spectra/scans with ms_level 2 (pyteomics on the depositor's mzXML)",
        ),
        spec(
            "ho-d-ana-ms-raw-elite-tic",
            "d_ms_elite",
            "analysis",
            "At what retention time (minutes) does the MS1 total ion chromatogram of this HILIC run reach its maximum?",
            fact("ms1_tic_apex_min", lambda v: g.number(v, "min", abs_=0.05)),
            "facts-heldout: ms1_tic_apex_min (pyteomics on the depositor's mzXML; the totIonCurrent attributes agree)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ms-raw-isq-scans",
            "d_ms_isq",
            "counts",
            "How many mass spectra (scans) does this GC-MS run contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (pyteomics on the depositor's mzXML)",
        ),
        spec(
            "ho-d-ana-ms-raw-isq-tic",
            "d_ms_isq",
            "analysis",
            "At what retention time (minutes) is the total ion current of this GC-MS run of an algal extract largest?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: tic_apex_min (pyteomics on the depositor's mzXML; the totIonCurrent attributes agree)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ms-raw-qeplus-ms2",
            "d_ms_qeplus",
            "counts",
            "This is a direct-infusion lipidomics run. How many MS2 spectra does it contain?",
            lambda o, m: g.integer(ms_level(o, 2)),
            "oracle: /spectra/scans with ms_level 2 (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-d-ms-raw-qeplus-first-precursor",
            "d_ms_qeplus",
            "values",
            "What is the precursor m/z of the first MS2 spectrum in this run?",
            lambda o, m: g.number(
                next(s for s in o["spectra"]["scans"] if s["ms_level"] == 2)["precursor_mz"], None, abs_=0.01
            ),
            "oracle: /spectra/scans, first ms_level 2 precursor_mz (pyteomics on the depositor's mzML)",
            answer_hint="the m/z",
        ),
        # ------------------------------------------------ Agilent MassHunter
        spec(
            "ho-d-ms-d-6495-transitions",
            "d_ms_6495",
            "counts",
            "How many MRM transitions (SRM chromatograms) does this triple-quadrupole lipid run record?",
            lambda o, m: g.integer(len(srm(o))),
            "oracle: /chromatograms of kind srm (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-d-ms-d-6545-scans",
            "d_ms_6545",
            "counts",
            "How many mass spectra does this Q-TOF run contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-d-ana-ms-d-6545-tic",
            "d_ms_6545",
            "analysis",
            "At what retention time (minutes) does the total ion chromatogram of this run reach its maximum?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.05)),
            "facts-heldout: tic_apex_min (pyteomics on the depositor's mzML; its TIC cvParams agree)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ Waters
        spec(
            "ho-d-chrom-waters-reims-scans",
            "d_ms_reims",
            "counts",
            "This REIMS acquisition sampled a bacterial colony. How many mass spectra (scans) does it contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (pyteomics on the depositor's mzXML)",
        ),
        spec(
            "ho-d-ana-chrom-waters-reims-base-peak",
            "d_ms_reims",
            "analysis",
            "In the scan with the highest total ion current, what is the m/z of the most intense peak?",
            fact("base_peak_mz", lambda v: g.number(v, None, abs_=0.02)),
            "facts-heldout: base_peak_mz (pyteomics on the depositor's mzXML; the scan's basePeakMz attribute agrees)",
            answer_hint="the m/z",
        ),
        spec(
            "ho-d-chrom-waters-xevotq-transition",
            "d_ms_xevotq",
            "values",
            "This MRM run has two functions. What precursor and product m/z does the second function monitor?",
            lambda o, m: g.items([f"{srm(o)[1]['precursor_mz']:.2f}", f"{srm(o)[1]['product_mz']:.2f}"], ordered=True),
            "oracle: /chromatograms, the SRM chromatogram of function 2 (pyteomics on the depositor's mzML)",
            answer_hint="precursor m/z, product m/z",
        ),
        # ------------------------------------------------ Sciex
        spec(
            "ho-d-ms-wiff-zenotof-ms2",
            "d_ms_zenotof",
            "counts",
            "How many MS2 spectra does this ZenoTOF run contain?",
            lambda o, m: g.integer(ms_level(o, 2)),
            "oracle: /spectra/scans with ms_level 2 (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-d-ana-ms-wiff-zenotof-precursors",
            "d_ms_zenotof",
            "analysis",
            "How many different precursor m/z values (to 0.01) were selected for fragmentation in this run?",
            fact("distinct_precursors", g.integer),
            "facts-heldout: distinct_precursors (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-d-ms-wiff-tt6600-scans",
            "d_ms_tt6600",
            "counts",
            "How many spectra does this TripleTOF run contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (pyteomics on the depositor's mzML)",
        ),
        # ------------------------------------------------ Bruker timsTOF
        spec(
            "ho-d-ms-tdf-blank-precursors",
            "d_ms_tdf_blank",
            "counts",
            "How many precursors were selected for PASEF MS/MS in this timsTOF run?",
            fact("precursors", g.integer),
            "facts-heldout: precursors (Python sqlite3 on analysis.tdf, Precursors table)",
            stage_as="sample.d/analysis.tdf",
            extra=[(C["d_ms_tdf_blank_bin"], "sample.d/analysis.tdf_bin")],
        ),
        spec(
            "ho-d-ms-tdf-blank-ms1-frames",
            "d_ms_tdf_blank",
            "counts",
            "How many MS1 frames does this timsTOF run contain?",
            fact("ms1_frames", g.integer),
            "facts-heldout: ms1_frames (Python sqlite3 on analysis.tdf, Frames with MsMsType 0)",
            stage_as="sample.d/analysis.tdf",
            extra=[(C["d_ms_tdf_blank_bin"], "sample.d/analysis.tdf_bin")],
        ),
        spec(
            "ho-d-ms-tdf-prm-targets",
            "d_ms_tdf_prm",
            "counts",
            "How many PRM targets (precursors in the prm-PASEF target list) does this run define?",
            fact("prm_targets", g.integer),
            "facts-heldout: prm_targets (Python sqlite3 on analysis.tdf, PrmTargets table)",
            stage_as="sample.d/analysis.tdf",
            extra=[(C["d_ms_tdf_prm_bin"], "sample.d/analysis.tdf_bin")],
        ),
        spec(
            "ho-d-ms-tdf-prm-instrument",
            "d_ms_tdf_prm",
            "instrument",
            "Which instrument model acquired this run?",
            fact(
                "instrument", lambda v: g.string("timsTOF Pro 2", accept=[v, "timsTOF pro 2", "Bruker timsTOF Pro 2"])
            ),
            "facts-heldout: instrument (Python sqlite3 on analysis.tdf, GlobalMetadata InstrumentName)",
            stage_as="sample.d/analysis.tdf",
            extra=[(C["d_ms_tdf_prm_bin"], "sample.d/analysis.tdf_bin")],
        ),
        # ------------------------------------------------ open formats
        spec(
            "ho-d-ms-mzml-sim-channels",
            "d_ms_mzml_sim",
            "channels",
            "This GC-MS run was recorded in selected-ion monitoring. Which m/z values were monitored?",
            lambda o, m: g.items([t["id"].rsplit(" ", 1)[1] for t in o["traces"] if t["id"].startswith("SIM")]),
            "oracle: /traces with SIM ids (pyteomics)",
            answer_hint="the m/z values",
        ),
        spec(
            "ho-d-ana-ms-mzml-sim-apex",
            "d_ms_mzml_sim",
            "analysis",
            "At what time (minutes) does the m/z 74 SIM trace of this run reach its maximum?",
            fact("sim74_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: sim74_apex_min (pyteomics)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-ms-mzml-avg-base-peak",
            "d_ms_mzml_avg",
            "analysis",
            "This file holds one averaged negative-mode spectrum. What is the m/z of its most intense peak?",
            fact("base_peak_mz", lambda v: g.number(v, None, abs_=0.01)),
            "facts-heldout: base_peak_mz (pyteomics)",
            answer_hint="the m/z",
        ),
        spec(
            "ho-d-ms-mzxml-qc-ms2",
            "d_ms_mzxml_qc",
            "counts",
            "How many MS2 scans does this LCMS-IT-TOF run contain?",
            lambda o, m: g.integer(ms_level(o, 2)),
            "oracle: /spectra/scans with ms_level 2 (pyteomics on a copy without <scanOrigin> elements)",
        ),
        # ------------------------------------------------ chromatography
        spec(
            "ho-d-ana-chrom-ch-tcd-largest",
            "d_ms_tcd",
            "analysis",
            "In this GC-TCD run, at what retention time (minutes) is the largest peak by area?",
            fact("largest_area_rt", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: largest_area_rt (the ChemStation area-percent report of the same run)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-chrom-ch-tcd-points",
            "d_ms_tcd",
            "counts",
            "How many data points does the TCD signal of this run have?",
            lambda o, m: g.integer(o["traces"][0]["sweeps"][0]["sample_count"]),
            "oracle: /traces/0/sweeps/0/sample_count (rainbow-api, black box)",
        ),
        spec(
            "ho-d-chrom-gcms-perfume-scans",
            "d_ms_perfume",
            "counts",
            "How many mass spectra (scans) does this GC-MS run of a perfume contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (rainbow-api, black box)",
        ),
        spec(
            "ho-d-ana-chrom-gcms-perfume-largest",
            "d_ms_perfume",
            "analysis",
            "Integrate the total ion chromatogram of this perfume GC-MS run. At what retention time (minutes) "
            "is the largest peak by area?",
            fact("largest_area_rt", lambda v: g.number(v, "min", abs_=0.03)),
            "facts-heldout: largest_area_rt (MSD ChemStation's own integration, RESULTS.CSV)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-chrom-cdf-faah-scans",
            "d_ms_cdf_faah",
            "counts",
            "How many mass spectra (scans) does this ANDI LC/MS file contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (scipy netcdf)",
        ),
        spec(
            "ho-d-chrom-cdf-faah-date",
            "d_ms_cdf_faah",
            "acquisition-time",
            "On what date was this LC/MS run acquired?",
            fact("acquired_date", lambda v: g.date(v, tolerance_days=1)),
            "facts-heldout: acquired_date (scipy netcdf_file, the experiment_date_time_stamp attribute)",
        ),
        spec(
            "ho-d-ana-chrom-cdf-gcms-tic",
            "d_ms_cdf_gcms",
            "analysis",
            "At what time (minutes) is the total ion current of this GC-MS run of a yeast extract largest?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: tic_apex_min (scipy netcdf; the summed scans agree)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-chrom-cdf-tq8040-scans",
            "d_ms_cdf_tq8040",
            "counts",
            "How many mass spectra (scans) does this ANDI GC-MS file contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (scipy netcdf)",
        ),
        spec(
            "ho-d-ana-chrom-cdf-tq8040-tic",
            "d_ms_cdf_tq8040",
            "analysis",
            "At what time (minutes) is the total ion current of this GC-MS run largest?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: tic_apex_min (scipy netcdf; the summed scans agree)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-chrom-lcd-peaks",
            "d_ms_lcd",
            "counts",
            "How many peaks does the peak table stored with this HPLC run list?",
            lambda o, m: g.integer(o["peak_count"]),
            "oracle: /peak_count (chromConverter, black box, reads the vendor's stored peak table)",
        ),
        spec(
            "ho-d-ana-chrom-lcd-largest",
            "d_ms_lcd",
            "analysis",
            "According to the peak table stored with this HPLC run, at what retention time (minutes) "
            "is the largest peak by area?",
            lambda o, m: g.number(round(max(o["peak_table"], key=lambda p: p["area"])["rt_min"], 3), "min", abs_=0.02),
            "oracle: /peak_table, largest area (chromConverter, black box, reads the vendor's stored peak table)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-chrom-openlab-wavelengths",
            "d_ms_openlab",
            "channels",
            "At which wavelengths were the diode-array signals of this HPLC injection recorded?",
            fact("dad_wavelengths", lambda v: g.items([str(x) for x in v])),
            "facts-heldout: dad_wavelengths (the vendor's report of the same injection, transcribed)",
            answer_hint="wavelengths in nm",
        ),
        spec(
            "ho-d-ana-chrom-openlab-largest",
            "d_ms_openlab",
            "analysis",
            "In the 220 nm signal of this HPLC injection, at what retention time (minutes) "
            "is the largest peak by area?",
            fact("largest_area_rt_220", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: largest_area_rt_220 (the vendor's report of the same injection, transcribed)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-chrom-arw-points",
            "d_ms_arw",
            "counts",
            "How many data points does this exported chromatogram have?",
            lambda o, m: g.integer(o["traces"][0]["sweeps"][0]["sample_count"]),
            "oracle: /traces/0/sweeps/0/sample_count (numpy on the export text)",
        ),
        spec(
            "ho-d-ana-chrom-arw-apex",
            "d_ms_arw",
            "analysis",
            "At what time (minutes) does the absorbance in this exported chromatogram of a reference standard "
            "reach its maximum?",
            fact("apex_min", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: apex_min (numpy on the export text; Python csv agrees)",
            answer_hint=unit_hint,
        ),
    ]
