#!/usr/bin/env python3
"""MCP smoke test: drive `openreadout mcp` as a client and check the whole surface.

Usage:
    python oracle/mcp_smoke.py BINARY FILE [FILE ...]           # stdio transport
    python oracle/mcp_smoke.py --http BINARY FILE [FILE ...]    # Streamable HTTP (build with --features mcp-http)
    options: --synthetic   also write and test a small JCAMP-DX spectrum, mzML run, FCS table
                           and RDML qPCR run, and check gate (a Gating-ML rectangle), table
                           filter/count and qpcr against values computed here; and an LC-MS run,
                           a 1H JCAMP-DX spectrum, a current-clamp ATF, a Neuralynx channel, a
                           plate-reader matrix and an OME-Zarr HCS plate, checking chromatogram,
                           the chromatogram, peaks, nmr-peaks, ephys-features, spikes and assay
                           analyses and per-well stats the same way
             --skip TOOL   do not fail when TOOL was never called (e.g. openreadout_scans when
                           no input has a spectrum run)

Standard library only (CI runs it with the runner's python3). For each server it checks the
initialize result (tools, resources and prompts capabilities), that every tool carries a title and
all four annotation hints (readOnlyHint, destructiveHint, idempotentHint, openWorldHint) with the
expected values, that every tool except openreadout_export declares an outputSchema, lists and
reads resources and resource templates, renders every prompt, and calls every tool at least once
over the given files (exports, attachments and report bundles go to a temporary directory; image
exports are sent a progressToken and must report progress). Over HTTP it also checks that a
browser Origin, a foreign Host and a missing bearer token are refused. A second session declares
MCP Apps (the io.modelcontextprotocol/ui extension) and checks the viewer: the ui:// page, the
tools that point at it, and openreadout_view over every file, within its size caps. Exits non-zero
on the first failure.
"""
import base64
import json
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

PROTOCOL = "2025-11-25"
WRITERS = {"openreadout_export", "openreadout_extract", "openreadout_batch", "openreadout_summarize"}
# Write only new files of their own (the index directory; a diagnostic bundle only when
# `output` is given): not read-only, not destructive.
INDEX_WRITERS = {"openreadout_index", "openreadout_report"}
# One tool per analysis; the plate-reader assays are one tool per `analysis`.
ASSAY_TOOLS = {
    "wells": "openreadout_assay_wells", "curve": "openreadout_assay_curve",
    "dose-response": "openreadout_dose_response", "kinetics": "openreadout_kinetics",
    "growth": "openreadout_growth", "qc": "openreadout_assay_qc",
}
# Assay arguments that the tools take under `plate_options`.
PLATE_OPTIONS = {"table", "read", "wavelength_nm", "embedded_layout", "layout_text", "empty_wells", "roles",
                 "blank_subtraction", "outliers", "outlier_threshold", "exclude_outliers"}
EXPECTED_TOOLS = {
    "openreadout_info", "openreadout_check", "openreadout_compare", "openreadout_report",
    "openreadout_preview", "openreadout_stats", "openreadout_trace", "openreadout_table",
    "openreadout_scans", "openreadout_spectrum", "openreadout_peaks", "openreadout_chromatogram", "openreadout_nmr_peaks",
    "openreadout_ephys_features", "openreadout_spikes", "openreadout_qpcr", "openreadout_gate",
    *ASSAY_TOOLS.values(),
    "openreadout_export", "openreadout_batch", "openreadout_link", "openreadout_index",
    "openreadout_search", "openreadout_health", "openreadout_watch", "openreadout_extract",
    "openreadout_summarize",
}


def analysis_call(kind, file, options):
    """The tool and arguments of analysis `kind` (`peaks`, `nmr-peaks`, `assay` with `analysis`)."""
    args = dict(options)
    if kind == "assay":
        name = ASSAY_TOOLS[args.pop("analysis", "wells")]
        plate = {k: args.pop(k) for k in list(args) if k in PLATE_OPTIONS}
        if plate:
            args["plate_options"] = plate
    else:
        name = "openreadout_" + kind.replace("-", "_")
    return name, {"file": file, **args}
# Returns new events on every call: not idempotent.
NOT_IDEMPOTENT = {"openreadout_watch"}
PREVIEW_BUDGET = 750_000
UI_EXTENSION = "io.modelcontextprotocol/ui"
UI_MIME = "text/html;profile=mcp-app"
VIEWER_URI = "ui://openreadout/viewer.html"
VIEW_TOOL = "openreadout_view"
# openreadout_view caps (crates/openreadout-mcp/src/app/view.rs)
VIEW_MAX_IMAGE = 1600
VIEW_MAX_POINTS = 4000


def fail(msg):
    raise SystemExit(f"FAIL: {msg}")


def check(cond, msg):
    if not cond:
        fail(msg)


# ---------- synthetic inputs (--synthetic): a JCAMP-DX spectrum, an mzML run, an FCS table ----------

def write_jcamp(path):
    n = 64
    ys = [round(1000 * math.exp(-((i - 30) / 4.0) ** 2)) + 10 for i in range(n)]
    lines = ["##TITLE=openreadout MCP smoke", "##JCAMP-DX=4.24", "##DATA TYPE=INFRARED SPECTRUM",
             "##ORIGIN=synthetic", "##OWNER=public domain", "##XUNITS=1/CM", "##YUNITS=ABSORBANCE",
             "##XFACTOR=1", "##YFACTOR=0.001", f"##FIRSTX=400", f"##LASTX={400 + (n - 1) * 10}",
             f"##DELTAX=10", f"##NPOINTS={n}", f"##FIRSTY={ys[0] * 0.001}", "##XYDATA=(X++(Y..Y))"]
    for i in range(0, n, 8):
        lines.append(" ".join([str(400 + i * 10)] + [str(y) for y in ys[i:i + 8]]))
    lines.append("##END=")
    open(path, "w").write("\n".join(lines) + "\n")


def write_mzml(path):
    def arr(values, fmt):
        return base64.b64encode(struct.pack("<" + fmt * len(values), *values)).decode()
    spectra = []
    for k in range(3):
        mz = [100.0 + 7.5 * i + k for i in range(20)]
        it = [float((i * 37 + k * 11) % 50 + 1) for i in range(20)]
        m, a = arr(mz, "d"), arr(it, "f")
        spectra.append(f'''      <spectrum index="{k}" id="scan={k + 1}" defaultArrayLength="{len(mz)}">
        <cvParam cvRef="MS" accession="MS:1000511" name="ms level" value="1"/>
        <cvParam cvRef="MS" accession="MS:1000579" name="MS1 spectrum" value=""/>
        <cvParam cvRef="MS" accession="MS:1000127" name="centroid spectrum" value=""/>
        <scanList count="1">
          <cvParam cvRef="MS" accession="MS:1000795" name="no combination" value=""/>
          <scan>
            <cvParam cvRef="MS" accession="MS:1000016" name="scan start time" value="{0.1 * (k + 1)}" unitCvRef="UO" unitAccession="UO:0000031" unitName="minute"/>
          </scan>
        </scanList>
        <binaryDataArrayList count="2">
          <binaryDataArray encodedLength="{len(m)}">
            <cvParam cvRef="MS" accession="MS:1000523" name="64-bit float" value=""/>
            <cvParam cvRef="MS" accession="MS:1000576" name="no compression" value=""/>
            <cvParam cvRef="MS" accession="MS:1000514" name="m/z array" value="" unitCvRef="MS" unitAccession="MS:1000040" unitName="m/z"/>
            <binary>{m}</binary>
          </binaryDataArray>
          <binaryDataArray encodedLength="{len(a)}">
            <cvParam cvRef="MS" accession="MS:1000521" name="32-bit float" value=""/>
            <cvParam cvRef="MS" accession="MS:1000576" name="no compression" value=""/>
            <cvParam cvRef="MS" accession="MS:1000515" name="intensity array" value="" unitCvRef="MS" unitAccession="MS:1000131" unitName="number of detector counts"/>
            <binary>{a}</binary>
          </binaryDataArray>
        </binaryDataArrayList>
      </spectrum>''')
    body = "\n".join(spectra)
    open(path, "w").write(f'''<?xml version="1.0" encoding="utf-8"?>
<mzML xmlns="http://psi.hupo.org/ms/mzml" id="smoke" version="1.1.0">
  <cvList count="2">
    <cv id="MS" fullName="Proteomics Standards Initiative Mass Spectrometry Ontology" URI="https://raw.githubusercontent.com/HUPO-PSI/psi-ms-CV/master/psi-ms.obo"/>
    <cv id="UO" fullName="Unit Ontology" URI="https://raw.githubusercontent.com/bio-ontology-research-group/unit-ontology/master/unit.obo"/>
  </cvList>
  <fileDescription>
    <fileContent>
      <cvParam cvRef="MS" accession="MS:1000579" name="MS1 spectrum" value=""/>
    </fileContent>
  </fileDescription>
  <softwareList count="1">
    <software id="smoke" version="0"/>
  </softwareList>
  <instrumentConfigurationList count="1">
    <instrumentConfiguration id="IC1"/>
  </instrumentConfigurationList>
  <dataProcessingList count="1">
    <dataProcessing id="DP1">
      <processingMethod order="0" softwareRef="smoke"/>
    </dataProcessing>
  </dataProcessingList>
  <run id="run1" defaultInstrumentConfigurationRef="IC1">
    <spectrumList count="{len(spectra)}" defaultDataProcessingRef="DP1">
{body}
    </spectrumList>
  </run>
</mzML>
''')


def fcs_events():
    return [(float(i % 97), float((i * 13) % 211)) for i in range(200)]


# Gating-ML 2.0 rectangle on the synthetic FCS events (min inclusive, max exclusive)
BOX = {"FSC-A": (10.0, 50.0), "SSC-A": (20.0, 100.0)}


def write_gatingml(path):
    dims = "".join(
        f'<gating:dimension gating:min="{lo}" gating:max="{hi}" gating:compensation-ref="uncompensated">'
        f'<data-type:fcs-dimension data-type:name="{name}"/></gating:dimension>'
        for name, (lo, hi) in BOX.items())
    open(path, "w").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<gating:Gating-ML xmlns:gating="http://www.isac-net.org/std/Gating-ML/v2.0/gating" '
        'xmlns:data-type="http://www.isac-net.org/std/Gating-ML/v2.0/datatypes">'
        f'<gating:RectangleGate gating:id="Box">{dims}</gating:RectangleGate></gating:Gating-ML>\n')


def box_count():
    return sum(1 for fsc, ssc in fcs_events()
               if BOX["FSC-A"][0] <= fsc < BOX["FSC-A"][1] and BOX["SSC-A"][0] <= ssc < BOX["SSC-A"][1])


# RDML 1.3 with two wells of one target: vendor Cq 21.5 (unknown) and none (NTC)
RDML_CQ = 21.5


def write_rdml(path):
    import zipfile
    adp = "".join(f"<adp><cyc>{i}</cyc><fluor>{1 + 1000 / (1 + math.exp(-(i - 22)))}</fluor></adp>"
                  for i in range(1, 41))
    xml = ('<?xml version="1.0" encoding="UTF-8"?>\n<rdml xmlns="http://www.rdml.org" version="1.3">'
           '<dye id="SYBR"/><sample id="S1"><type>unkn</type></sample><sample id="N"><type>ntc</type></sample>'
           '<target id="GAPDH"><type>ref</type><dyeId id="SYBR"/></target>'
           '<experiment id="E"><run id="R"><pcrFormat><rows>8</rows><columns>12</columns>'
           '<rowLabel>ABC</rowLabel><columnLabel>123</columnLabel></pcrFormat>'
           f'<react id="1"><sample id="S1"/><data><tar id="GAPDH"/><cq>{RDML_CQ}</cq>{adp}</data></react>'
           '<react id="2"><sample id="N"/><data><tar id="GAPDH"/><cq>-1</cq></data></react>'
           '</run></experiment></rdml>')
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("rdml_data.xml", xml)


def synthetic_analyses(c, tmp, called):
    """gate (Gating-ML rectangle), table filter/count and qpcr on the synthetic inputs, each
    checked against the value this script computes from what it wrote."""
    def call(name, args):
        called.add(name)
        return c.request("tools/call", {"name": name, "arguments": args})[0]

    def analyze(kind, file, **options):
        return call(*analysis_call(kind, file, options))

    fcs, gml, rdml = (os.path.join(tmp, n) for n in ("smoke.fcs", "smoke-gates.xml", "smoke.rdml"))
    g = result_json(analyze("gate", fcs, gatingml=gml), "gate")
    box = [p for p in g["populations"] if p["name"] == "Box"]
    check(box and box[0]["count"] == box_count(), f"gate: Box count {box and box[0]['count']} != {box_count()}")
    lo, hi = BOX["FSC-A"]
    t = result_json(call("openreadout_table", {"file": fcs, "filter": [f"FSC-A >= {lo}", f"FSC-A < {hi}"],
                                                 "count": True}), "table filter")
    want = sum(1 for fsc, _ in fcs_events() if lo <= fsc < hi)
    check(t["filter"]["matched_rows"] == want and not t["rows"],
          f"table filter: {t['filter']['matched_rows']} != {want}")
    q = result_json(analyze("qpcr", rdml), "qpcr")
    cqs = sorted((r["well"], r.get("cq")) for r in q["records"])
    check(len(cqs) == 2 and any(cq == RDML_CQ for _, cq in cqs), f"qpcr records {cqs}")
    print(f"gate: Box {box[0]['count']} events; table filter: {want} of {t['total_rows']}; "
          f"qpcr: {len(cqs)} records, Cq {RDML_CQ}")


# ---------- synthetic inputs for the analysis tools, each with its expected result computed here ----------

def _mzml_doc(spectra):
    body = "\n".join(spectra)
    return f'''<?xml version="1.0" encoding="utf-8"?>
<mzML xmlns="http://psi.hupo.org/ms/mzml" id="smoke-lc" version="1.1.0">
  <cvList count="2">
    <cv id="MS" fullName="Proteomics Standards Initiative Mass Spectrometry Ontology" URI="https://raw.githubusercontent.com/HUPO-PSI/psi-ms-CV/master/psi-ms.obo"/>
    <cv id="UO" fullName="Unit Ontology" URI="https://raw.githubusercontent.com/bio-ontology-research-group/unit-ontology/master/unit.obo"/>
  </cvList>
  <fileDescription><fileContent><cvParam cvRef="MS" accession="MS:1000579" name="MS1 spectrum" value=""/></fileContent></fileDescription>
  <softwareList count="1"><software id="smoke" version="0"/></softwareList>
  <instrumentConfigurationList count="1"><instrumentConfiguration id="IC1"/></instrumentConfigurationList>
  <dataProcessingList count="1"><dataProcessing id="DP1"><processingMethod order="0" softwareRef="smoke"/></dataProcessing></dataProcessingList>
  <run id="run1" defaultInstrumentConfigurationRef="IC1">
    <spectrumList count="{len(spectra)}" defaultDataProcessingRef="DP1">
{body}
    </spectrumList>
  </run>
</mzML>
'''


# LC-MS run: 61 MS1 scans every 0.05 min; m/z 300.0 elutes as a Gaussian at 1.5 min
LC_MZ, LC_APEX, LC_SIGMA, LC_HEIGHT = 300.0, 1.5, 0.1, 1.0e5


def lc_points():
    return [(round(0.05 * k, 4), LC_HEIGHT * math.exp(-0.5 * ((0.05 * k - LC_APEX) / LC_SIGMA) ** 2))
            for k in range(61)]


def write_lc_mzml(path):
    def arr(values, fmt):
        return base64.b64encode(struct.pack("<" + fmt * len(values), *values)).decode()
    spectra = []
    for k, (t, y) in enumerate(lc_points()):
        mz, it = [150.0, LC_MZ, 450.0], [10.0, y, 20.0]
        m, a = arr(mz, "d"), arr(it, "d")
        spectra.append(f'''      <spectrum index="{k}" id="scan={k + 1}" defaultArrayLength="3">
        <cvParam cvRef="MS" accession="MS:1000511" name="ms level" value="1"/>
        <cvParam cvRef="MS" accession="MS:1000579" name="MS1 spectrum" value=""/>
        <cvParam cvRef="MS" accession="MS:1000127" name="centroid spectrum" value=""/>
        <scanList count="1"><cvParam cvRef="MS" accession="MS:1000795" name="no combination" value=""/>
          <scan><cvParam cvRef="MS" accession="MS:1000016" name="scan start time" value="{t}" unitCvRef="UO" unitAccession="UO:0000031" unitName="minute"/></scan>
        </scanList>
        <binaryDataArrayList count="2">
          <binaryDataArray encodedLength="{len(m)}"><cvParam cvRef="MS" accession="MS:1000523" name="64-bit float" value=""/><cvParam cvRef="MS" accession="MS:1000576" name="no compression" value=""/><cvParam cvRef="MS" accession="MS:1000514" name="m/z array" value="" unitCvRef="MS" unitAccession="MS:1000040" unitName="m/z"/><binary>{m}</binary></binaryDataArray>
          <binaryDataArray encodedLength="{len(a)}"><cvParam cvRef="MS" accession="MS:1000523" name="64-bit float" value=""/><cvParam cvRef="MS" accession="MS:1000576" name="no compression" value=""/><cvParam cvRef="MS" accession="MS:1000515" name="intensity array" value="" unitCvRef="MS" unitAccession="MS:1000131" unitName="number of detector counts"/><binary>{a}</binary></binaryDataArray>
        </binaryDataArrayList>
      </spectrum>''')
    open(path, "w").write(_mzml_doc(spectra))


def trapezoid_baseline(points, a, b):
    """Area of (y - straight line between the end points) over the points with a <= t <= b, in
    signal x minutes: a manual integration with a straight baseline."""
    pts = [(t, y) for t, y in points if a - 1e-9 <= t <= b + 1e-9]
    (t0, y0), (t1, y1) = pts[0], pts[-1]
    base = lambda t: y0 + (y1 - y0) * (t - t0) / (t1 - t0)  # noqa: E731
    return sum((pts[i + 1][0] - pts[i][0]) * ((pts[i][1] - base(pts[i][0])) + (pts[i + 1][1] - base(pts[i + 1][0]))) / 2
               for i in range(len(pts) - 1))


# 1H NMR spectrum (JCAMP-DX, X in Hz at 400 MHz): Lorentzian lines at 7.26 ppm (1 part) and 1.25 ppm (3 parts)
NMR_MHZ, NMR_LINES = 400.0, [(7.26, 1.0), (1.25, 3.0)]


def nmr_points():
    n, lo, hi = 8192, -1.0, 11.0
    out = []
    for i in range(n):
        ppm = hi - (hi - lo) * i / (n - 1)  # decreasing x, as spectra are written
        y = sum(1000.0 * a / (1 + ((ppm - c) * NMR_MHZ / 0.8) ** 2) for c, a in NMR_LINES)
        out.append((ppm, y))
    return out


def write_nmr_jcamp(path):
    pts = nmr_points()
    hz = [p * NMR_MHZ for p, _ in pts]
    ys = [round(y * 100) for _, y in pts]
    lines = ["##TITLE=openreadout MCP smoke 1H", "##JCAMP-DX=5.01", "##DATA TYPE=NMR SPECTRUM", "##DATA CLASS=XYDATA",
             "##ORIGIN=synthetic", "##OWNER=public domain", f"##.OBSERVE FREQUENCY={NMR_MHZ}", "##.OBSERVE NUCLEUS=^1H",
             "##XUNITS=HZ", "##YUNITS=ARBITRARY UNITS", "##XFACTOR=1", "##YFACTOR=0.01", f"##FIRSTX={hz[0]}",
             f"##LASTX={hz[-1]}", f"##NPOINTS={len(ys)}", f"##FIRSTY={ys[0] * 0.01}", "##XYDATA=(X++(Y..Y))"]
    dx = (hz[-1] - hz[0]) / (len(hz) - 1)
    for i in range(0, len(ys), 10):
        lines.append(" ".join([f"{hz[0] + i * dx:.6f}"] + [str(y) for y in ys[i:i + 10]]))
    lines.append("##END=")
    open(path, "w").write("\n".join(lines) + "\n")


def nmr_integral(lo, hi):
    return sum(y for p, y in nmr_points() if lo <= p <= hi)


# Current-clamp ATF: three sweeps of 0.5 s at 10 kHz with 0, 3 and 6 action potentials
ATF_RATE, ATF_SAMPLES, ATF_SPIKES = 10_000, 5000, [0, 3, 6]


def atf_sweep(n_spikes):
    v = [-70.0] * ATF_SAMPLES
    for s in range(n_spikes):
        start = 1000 + 500 * s
        shape = [-60, -40, -10, 20, 30, 15, -10, -40, -65, -78, -76, -74, -72]
        for i, x in enumerate(shape):
            v[start + i] = float(x)
    return v


def write_atf(path):
    sweeps = [atf_sweep(n) for n in ATF_SPIKES]
    head = ["ATF\t1.0", f"4\t{1 + len(sweeps)}", '"AcquisitionMode=Episodic Stimulation"', '"Comment=openreadout MCP smoke"',
            '"SignalsExported=IN 0"', '"Signals="\t' + "\t".join('"IN 0"' for _ in sweeps),
            '"Time (s)"\t' + "\t".join(f'"Trace #{i + 1} (mV)"' for i in range(len(sweeps)))]
    rows = [f"{i / ATF_RATE:.5f}\t" + "\t".join(f"{s[i]:.3f}" for s in sweeps) for i in range(ATF_SAMPLES)]
    open(path, "w").write("\r\n".join(head + rows) + "\r\n")


# Neuralynx continuous channel (.ncs): 32 kHz, 64 records of 512 samples, 12 negative spikes on
# deterministic Gaussian noise (σ 5 µV)
NCS_RATE, NCS_RECORDS, NCS_SPIKES, NCS_UV_PER_COUNT = 32000, 64, 12, 0.1


def ncs_samples():
    n = NCS_RECORDS * 512
    state = 12345
    out = []
    for _ in range(0, n, 2):  # Box-Muller on a fixed linear congruential generator
        state = (1103515245 * state + 12345) % 2 ** 31
        u1 = (state + 1) / 2 ** 31
        state = (1103515245 * state + 12345) % 2 ** 31
        u2 = state / 2 ** 31
        r = math.sqrt(-2 * math.log(u1)) * 5.0
        out += [r * math.cos(2 * math.pi * u2), r * math.sin(2 * math.pi * u2)]
    shape = [-20, -80, -160, -200, -150, -60, 20, 50, 40, 25, 10]
    for k in range(NCS_SPIKES):
        at = 1500 + k * 2600
        for i, x in enumerate(shape):
            out[at + i] += x
    return [max(-32768, min(32767, round(v / NCS_UV_PER_COUNT))) for v in out[:n]]


def write_ncs(path):
    header = ("######## Neuralynx Data File Header\r\n## File Name smoke.ncs\r\n-FileType CSC\r\n-RecordSize 1044\r\n"
              f"-SamplingFrequency {NCS_RATE}\r\n-ADBitVolts {NCS_UV_PER_COUNT * 1e-6:.10f}\r\n-ADChannel 0\r\n"
              "-InputInverted False\r\n-AcqEntName CSC1\r\n").encode()
    data = bytearray(header.ljust(16384, b"\0"))
    s = ncs_samples()
    for r in range(NCS_RECORDS):
        ts = int(r * 512 * 1e6 / NCS_RATE)
        data += struct.pack("<QIII", ts, 0, NCS_RATE, 512) + struct.pack("<512h", *s[r * 512:(r + 1) * 512])
    open(path, "wb").write(bytes(data))


# Plate-reader matrix: a linear standard curve (y = 0.01·conc + 0.05) in columns 1-2, samples elsewhere
PLATE_STD = {"A": 100.0, "B": 50.0, "C": 25.0, "D": 12.5, "E": 6.25, "F": 3.125, "G": 1.5625, "H": 0.0}


def plate_values():
    v = {}
    for r, conc in PLATE_STD.items():
        for c in range(1, 13):
            v[f"{r}{c}"] = round(0.01 * conc + 0.05, 6) if c <= 2 else round(0.05 + 0.0037 * ((ord(r) - 64) * 13 + c), 6)
    return v


def write_plate(path):
    v = plate_values()
    lines = ["OD450," + ",".join(str(c) for c in range(1, 13))]
    for r in PLATE_STD:
        lines.append(r + "," + ",".join(f"{v[f'{r}{c}']:.6f}" for c in range(1, 13)))
    open(path, "w").write("\n".join(lines) + "\n")


# Screening plate as OME-Zarr 0.4 HCS (Zarr v2, uncompressed): wells A1, A2, B1 × 1 field × 2 channels
ZPLATE_WELLS = {"A/1": 100, "A/2": 400, "B/1": 1600}


def zplate_pixels(base, c):
    return [base * (c + 1) + (i % 7) for i in range(8 * 10)]


def write_zarr_plate(root):
    def js(p, d):
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "w").write(json.dumps(d))
    js(os.path.join(root, ".zgroup"), {"zarr_format": 2})
    js(os.path.join(root, ".zattrs"), {"plate": {
        "version": "0.4", "name": "smoke", "field_count": 1, "rows": [{"name": "A"}, {"name": "B"}],
        "columns": [{"name": "1"}, {"name": "2"}],
        "wells": [{"path": w, "rowIndex": "AB".index(w[0]), "columnIndex": int(w[2]) - 1} for w in ZPLATE_WELLS]}})
    for w, base in ZPLATE_WELLS.items():
        wd = os.path.join(root, *w.split("/"))
        js(os.path.join(wd, ".zgroup"), {"zarr_format": 2})
        js(os.path.join(wd, ".zattrs"), {"well": {"version": "0.4", "images": [{"path": "0"}]}})
        im = os.path.join(wd, "0")
        js(os.path.join(im, ".zgroup"), {"zarr_format": 2})
        js(os.path.join(im, ".zattrs"), {"multiscales": [{"version": "0.4", "axes": [
            {"name": "c", "type": "channel"}, {"name": "y", "type": "space", "unit": "micrometer"},
            {"name": "x", "type": "space", "unit": "micrometer"}],
            "datasets": [{"path": "0", "coordinateTransformations": [{"type": "scale", "scale": [1, 0.5, 0.5]}]}]}],
            "omero": {"channels": [{"label": "DAPI"}, {"label": "GFP"}]}})
        js(os.path.join(im, "0", ".zarray"), {"zarr_format": 2, "shape": [2, 8, 10], "chunks": [1, 8, 10],
                                              "dtype": "<u2", "compressor": None, "fill_value": 0, "order": "C",
                                              "filters": None, "dimension_separator": "/"})
        for c in range(2):
            chunk = os.path.join(im, "0", str(c), "0", "0")
            os.makedirs(os.path.dirname(chunk), exist_ok=True)
            open(chunk, "wb").write(struct.pack("<80H", *zplate_pixels(base, c)))


def analysis_tools(c, tmp, called):
    """assay, chromatogram, peaks, nmr_peaks, ephys_features, spikes and well_stats on synthetic
    inputs, each checked against the value this script computes from what it wrote."""
    def call(name, args):
        called.add(name)
        return result_json(c.request("tools/call", {"name": name, "arguments": args})[0], name)

    def analyze(kind, file, **options):
        name, args = analysis_call(kind, file, options)
        called.add(name)
        r = c.request("tools/call", {"name": name, "arguments": args})[0]
        return result_json(r, name)

    def answers(kind, file, **options):
        """The tool answers: a result, or an error with a code and a hint."""
        name, args = analysis_call(kind, file, options)
        called.add(name)
        r = c.request("tools/call", {"name": name, "arguments": args})[0]
        if "error" in r:
            check(r["error"].get("data", {}).get("hint"), f"{name}: an error without a hint: {r}")
        else:
            result_json(r, name)

    lc = os.path.join(tmp, "smoke-lc.mzML")
    ch = analyze("chromatogram", lc, mz=[LC_MZ])["chromatograms"][0]
    top = max(lc_points(), key=lambda p: p[1])
    check(abs(ch["apex_rt_min"] - top[0]) < 1e-6 and math.isclose(ch["apex_intensity"], top[1], rel_tol=1e-9),
          f"chromatogram apex {ch['apex_rt_min']} min {ch['apex_intensity']} != {top}")
    pk = analyze("peaks", lc, mz=[LC_MZ], integrate=[[1.0, 2.0]], summary_only=True)
    manual = pk["chromatograms"][0]["manual"][0]
    want = trapezoid_baseline(lc_points(), 1.0, 2.0)
    check(math.isclose(manual["area"], want, rel_tol=1e-3), f"peaks manual area {manual['area']} != {want}")
    print(f"chromatogram: XIC apex {ch['apex_rt_min']} min; peaks: manual area {manual['area']:.1f} (expected {want:.1f})")

    nm = analyze("nmr-peaks", os.path.join(tmp, "smoke-1h.jdx"), integrate=[[7.4, 7.1], [1.4, 1.1]])
    step = 12.0 / 8191
    check(abs(nm["main_peak"]["ppm"] - 1.25) <= 1.5 * step, f"nmr_peaks main peak {nm['main_peak']['ppm']} ppm")
    got = nm["integrals"][1]["value"] / nm["integrals"][0]["value"]
    want = nmr_integral(1.1, 1.4) / nmr_integral(7.1, 7.4)
    check(math.isclose(got, want, rel_tol=0.02), f"nmr_peaks integral ratio {got} != {want}")
    print(f"nmr_peaks: main peak {nm['main_peak']['ppm']:.3f} ppm, integral ratio {got:.3f} (expected {want:.3f})")

    ef = analyze("ephys-features", os.path.join(tmp, "smoke-cc.atf"))
    counts = [s["spike_count"] for s in ef["sweeps"]]
    check(counts == ATF_SPIKES and ef["spike_count_total"] == sum(ATF_SPIKES),
          f"ephys_features spike counts {counts} != {ATF_SPIKES}")
    print(f"ephys_features: spikes per sweep {counts}")

    sp = analyze("spikes", os.path.join(tmp, "smoke-csc.ncs"))
    check(sp["spike_count_total"] == NCS_SPIKES, f"spikes: {sp['spike_count_total']} != {NCS_SPIKES}")
    print(f"spikes: {sp['spike_count_total']} spikes (inserted {NCS_SPIKES})")

    plate = os.path.join(tmp, "smoke-plate.csv")
    vals = plate_values()
    aw = analyze("assay", plate, analysis="wells")
    got = {w["well"]: w["raw"] for w in aw["wells"]}
    check(all(math.isclose(got[w], v, rel_tol=1e-9) for w, v in vals.items()), "assay-wells differ from the written values")
    stds = ";".join(f"{r}1,{r}2={conc}" for r, conc in PLATE_STD.items())
    cv = analyze("assay", plate, analysis="curve", standards=stds, model="linear", blank_subtraction="none")
    well = next(w for w in cv["wells"] if w["well"] == "C7")
    want = (vals["C7"] - 0.05) / 0.01
    check(math.isclose(well["back_calculated"], want, rel_tol=1e-6), f"assay-curve C7 {well['back_calculated']} != {want}")
    print(f"assay: {len(got)} wells as written; linear curve back-calculates C7 = {well['back_calculated']:.4f} ({want:.4f})")
    for analysis in ("dose-response", "kinetics", "growth", "qc"):
        answers("assay", plate, analysis=analysis)

    ws = call("openreadout_stats", {"file": os.path.join(tmp, "smoke-plate.ome.zarr"), "per": "well"})
    for w, base in ZPLATE_WELLS.items():
        name = f"{w[0]}{int(w[2]):02d}"
        for ch_ in range(2):
            px = zplate_pixels(base, ch_)
            row = next(r for r in ws["rows"] if r["well"] in (name, w.replace("/", "")) and r["c"] == ch_)
            check(math.isclose(row["mean"], sum(px) / len(px), rel_tol=1e-9), f"well_stats {name} c{ch_}: {row['mean']}")
    print(f"well_stats: {len(ws['rows'])} rows, per-well means as written")


def write_fcs(path):
    events = fcs_events()
    data = b"".join(struct.pack("<ff", *e) for e in events)
    def text(begin, end):
        kv = [("$BEGINANALYSIS", "0"), ("$ENDANALYSIS", "0"), ("$BEGINSTEXT", "0"), ("$ENDSTEXT", "0"),
              ("$BEGINDATA", f"{begin:08d}"), ("$ENDDATA", f"{end:08d}"), ("$BYTEORD", "1,2,3,4"),
              ("$DATATYPE", "F"), ("$MODE", "L"), ("$NEXTDATA", "0"), ("$PAR", "2"), ("$TOT", str(len(events))),
              ("$P1N", "FSC-A"), ("$P1B", "32"), ("$P1E", "0,0"), ("$P1R", "1024"),
              ("$P2N", "SSC-A"), ("$P2B", "32"), ("$P2E", "0,0"), ("$P2R", "1024")]
        return ("/" + "".join(f"{k}/{v}/" for k, v in kv)).encode()
    t0 = 58
    t = text(0, 0)
    d0 = t0 + len(t)
    d1 = d0 + len(data) - 1
    t = text(d0, d1)
    assert len(t) == d0 - t0
    header = b"FCS3.0    " + b"".join(f"{v:>8}".encode() for v in (t0, t0 + len(t) - 1, d0, d1, 0, 0))
    assert len(header) == 58
    open(path, "wb").write(header + t + data)



class Stdio:
    def __init__(self, binary):
        self.p = subprocess.Popen([binary, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, bufsize=1)
        self.seq = 0

    def request(self, method, params=None):
        """Send a request; return (response, notifications received before it)."""
        self.seq += 1
        msg = {"jsonrpc": "2.0", "id": self.seq, "method": method}
        if params is not None:
            msg["params"] = params
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()
        notes = []
        while True:
            line = self.p.stdout.readline()
            if not line:
                fail(f"server closed; stderr: {self.p.stderr.read()[:2000]}")
            m = json.loads(line)
            if m.get("id") == self.seq and "method" not in m:
                return m, notes
            notes.append(m)

    def notify(self, method, params=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=10)
        check(self.p.returncode == 0, f"server exit code {self.p.returncode}")


class Http:
    def __init__(self, binary, token=None):
        env = dict(os.environ)
        if token:
            env["OPENREADOUT_MCP_TOKEN"] = token
        self.token = token
        self.p = subprocess.Popen([binary, "mcp", "--http", "127.0.0.1:0"], stdout=subprocess.DEVNULL,
                                  stderr=subprocess.PIPE, text=True, env=env)
        line = self.p.stderr.readline()
        m = re.search(r"http://([0-9.]+:[0-9]+)/mcp", line)
        if not m:
            self.p.kill()
            fail(f"no listening address in: {line!r} {self.p.stderr.read()[:1000]}")
        self.url = f"http://{m.group(1)}/mcp"
        self.session = None
        self.seq = 0

    def post(self, msg, extra_headers=None):
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        if self.session:
            headers["Mcp-Session-Id"] = self.session
            headers["MCP-Protocol-Version"] = PROTOCOL
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        headers.update(extra_headers or {})
        req = urllib.request.Request(self.url, data=json.dumps(msg).encode(), headers=headers, method="POST")
        return urllib.request.urlopen(req, timeout=120)

    def request(self, method, params=None):
        self.seq += 1
        msg = {"jsonrpc": "2.0", "id": self.seq, "method": method}
        if params is not None:
            msg["params"] = params
        r = self.post(msg)
        if r.headers.get("Mcp-Session-Id"):
            self.session = r.headers["Mcp-Session-Id"]
        notes = []
        if r.headers.get("Content-Type", "").startswith("application/json"):
            return json.loads(r.read()), notes
        for raw in r:  # server-sent events
            line = raw.decode().rstrip("\r\n")
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if not data:
                continue
            m = json.loads(data)
            if m.get("id") == self.seq and "method" not in m:
                r.close()
                return m, notes
            notes.append(m)
        fail(f"no response to {method}")

    def notify(self, method, params=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        self.post(msg).read()

    def close(self):
        self.p.terminate()
        self.p.wait(timeout=10)


def http_security(binary):
    """Browser Origins, foreign Host headers and missing tokens are refused."""
    init = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}}}
    s = Http(binary)
    try:
        for hdrs, what in (({"Origin": "https://evil.example"}, "browser Origin"),
                           ({"Host": "evil.example"}, "foreign Host")):
            try:
                s.post(init, hdrs).read()
                fail(f"{what} was accepted")
            except urllib.error.HTTPError as e:
                check(e.code in (400, 403), f"{what}: HTTP {e.code}")
                print(f"http: {what} refused with {e.code}")
    finally:
        s.close()
    s = Http(binary, token="smoke-token")
    try:
        s.token = None
        try:
            s.post(init).read()
            fail("request without the bearer token was accepted")
        except urllib.error.HTTPError as e:
            check(e.code == 401, f"missing token: HTTP {e.code}")
            print("http: missing bearer token refused with 401")
        s.token = "smoke-token"
        r, _ = s.request("initialize", init["params"])
        check("result" in r, f"initialize with token failed: {r}")
        print("http: bearer token accepted")
    finally:
        s.close()


def result_json(r, tool):
    check("result" in r, f"{tool} failed: {json.dumps(r.get('error'))[:400]}")
    res = r["result"]
    check(not res.get("isError"), f"{tool} isError: {json.dumps(res)[:400]}")
    texts = [c for c in res["content"] if c["type"] == "text"]
    check(texts, f"{tool}: no text content")
    # The JSON block is not always last: `info` follows it with a thumbnail and its caption.
    blocks = []
    for t in texts:
        try:
            blocks.append(json.loads(t["text"]))
        except json.JSONDecodeError:
            pass
    check(blocks, f"{tool}: no JSON text block")
    data = blocks[-1]
    if "structuredContent" in res:
        check(res["structuredContent"] == data, f"{tool}: structuredContent differs from the text")
    return data


def check_image(b64, mime, what):
    raw = base64.b64decode(b64)
    check(len(raw) <= PREVIEW_BUDGET, f"{what}: {len(raw)} bytes exceeds the budget")
    if mime == "image/png":
        check(raw[:8] == b"\x89PNG\r\n\x1a\n", f"{what}: not a PNG")
        w, h = int.from_bytes(raw[16:20], "big"), int.from_bytes(raw[20:24], "big")
    else:
        check(mime == "image/jpeg" and raw[:2] == b"\xff\xd8", f"{what}: not a JPEG ({mime})")
        w = h = None
    return raw, w, h


def viewer(c, files):
    """A client that declares MCP Apps gets the viewer; its data stays within the caps."""
    r, _ = c.request("initialize", {
        "protocolVersion": PROTOCOL,
        "capabilities": {"extensions": {UI_EXTENSION: {"mimeTypes": [UI_MIME]}}},
        "clientInfo": {"name": "smoke-ui", "version": "0"}})
    check(UI_EXTENSION in r["result"]["capabilities"].get("extensions", {}),
          f"server does not declare {UI_EXTENSION}: {r['result']['capabilities']}")
    c.notify("notifications/initialized")
    tools = {t["name"]: t for t in c.request("tools/list")[0]["result"]["tools"]}
    check(set(tools) == EXPECTED_TOOLS | {VIEW_TOOL}, f"tools with the viewer: {sorted(tools)}")
    check(tools[VIEW_TOOL]["_meta"]["ui"] == {"resourceUri": VIEWER_URI, "visibility": ["app"]},
          f"{VIEW_TOOL} _meta: {tools[VIEW_TOOL].get('_meta')}")
    check(tools["openreadout_info"]["_meta"]["ui"]["resourceUri"] == VIEWER_URI, "info has no viewer")
    check("_meta" not in tools["openreadout_export"], "export should have no viewer")
    page = c.request("resources/read", {"uri": VIEWER_URI})[0]["result"]["contents"][0]
    check(page["mimeType"] == UI_MIME and page["text"].lower().startswith("<!doctype html>"), "viewer page")
    check("http://" not in page["text"] and "https://" not in page["text"], "the viewer page names a URL")
    shown = []
    for f in files:
        r, _ = c.request("tools/call", {"name": "openreadout_info", "arguments": {"file": f}})
        if "result" not in r or r["result"].get("isError"):
            continue
        hint = r["result"].get("_meta", {}).get("openreadout/view")
        check(hint and hint.get("file") == f, f"info {f}: no viewer hint in _meta")
        r, _ = c.request("tools/call", {"name": VIEW_TOOL, "arguments": hint})
        check("result" in r and not r["result"].get("isError"), f"{VIEW_TOOL} {f}: {json.dumps(r)[:400]}")
        res = r["result"]
        sc = res["structuredContent"]
        for img in (x for x in res["content"] if x["type"] == "image"):
            _, w, h = check_image(img["data"], img["mimeType"], f"{VIEW_TOOL} {f}")
            check(w is None or max(w, h) <= VIEW_MAX_IMAGE, f"{VIEW_TOOL} {f}: {w}x{h}")
        for series in (sc.get("plot") or {}).get("series", []):
            for k in ("x", "y", "lo", "hi"):
                n = len(series.get(k) or [])
                check(n <= VIEW_MAX_POINTS, f"{VIEW_TOOL} {f}: {n} points in {k}")
        shown.append(f"{os.path.basename(f)}={sc['view']}")
    check(shown, "the viewer showed no file")
    print("viewer ->", ", ".join(shown))


def surface(c):
    """Checks that do not depend on a file."""
    r, _ = c.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                    "clientInfo": {"name": "smoke", "version": "0"}})
    res = r["result"]
    caps = res["capabilities"]
    check(all(k in caps for k in ("tools", "resources", "prompts")), f"capabilities: {caps}")
    # The instructions state the write guarantee (only export writes next to the data) and name no
    # tool that does not exist.
    instr = res.get("instructions", "")
    named = set(re.findall(r"openreadout_[a-z_]+", instr))
    check("openreadout_export" in named, "instructions do not state which tool writes")
    check(named <= EXPECTED_TOOLS, f"instructions name unknown tools: {sorted(named - EXPECTED_TOOLS)}")
    print("initialize ->", res["serverInfo"], "| protocol", res["protocolVersion"], "| capabilities", sorted(caps))
    c.notify("notifications/initialized")

    tools = c.request("tools/list")[0]["result"]["tools"]
    names = {t["name"] for t in tools}
    check(names == EXPECTED_TOOLS, f"tools differ: missing {EXPECTED_TOOLS - names}, extra {names - EXPECTED_TOOLS}")
    for t in tools:
        a = t.get("annotations") or {}
        w = t["name"] in WRITERS
        ro = not w and t["name"] not in INDEX_WRITERS
        want = {"readOnlyHint": ro, "destructiveHint": w,
                "idempotentHint": t["name"] not in NOT_IDEMPOTENT, "openWorldHint": False}
        got = {k: a.get(k) for k in want}
        check(got == want, f"{t['name']} annotations {a}")
        check(a.get("title"), f"{t['name']} has no title")
        if t["name"] != "openreadout_export":
            check(isinstance(t.get("outputSchema"), dict), f"{t['name']} has no outputSchema")
    print(f"tools: {len(tools)}, all annotated; outputSchema on {sum('outputSchema' in t for t in tools)}")

    res = c.request("resources/list")[0]["result"]["resources"]
    check(any(x["uri"] == "openreadout://formats" for x in res), f"resources: {res}")
    tmpl = c.request("resources/templates/list")[0]["result"]["resourceTemplates"]
    uris = {x["uriTemplate"] for x in tmpl}
    check({"openreadout://file/{path}", "openreadout://preview/{path}"} <= uris, f"templates: {uris}")
    r = c.request("resources/read", {"uri": "openreadout://formats"})[0]
    fm = json.loads(r["result"]["contents"][0]["text"])
    check(len(fm["formats"]) > 10, "formats resource is short")
    print(f"resources: {[x['uri'] for x in res]}, templates: {sorted(uris)}, formats: {len(fm['formats'])}")

    prompts = c.request("prompts/list")[0]["result"]["prompts"]
    pnames = {p["name"] for p in prompts}
    check(pnames == {"summarize_file", "check_file", "convert_to_open_format"}, f"prompts: {pnames}")
    for p in prompts:
        r = c.request("prompts/get", {"name": p["name"], "arguments": {"file": "/data/a.czi"}})[0]
        msgs = r["result"]["messages"]
        check(msgs and "/data/a.czi" in msgs[0]["content"]["text"], f"prompt {p['name']}: {r}")
    r = c.request("prompts/get", {"name": "summarize_file", "arguments": {}})[0]
    check("error" in r, "prompt without its file argument did not fail")
    print(f"prompts: {sorted(pnames)} render")

    r = c.request("tools/call", {"name": "openreadout_info", "arguments": {"file": "/nonexistent.czi"}})[0]
    err = r.get("error") or {}
    check(err.get("data", {}).get("exit_code") == 5 and err["data"].get("hint"), f"error case: {r}")
    print("the error case OK")


def per_file(c, path, tmp, called):
    def call(name, args):
        called.add(name)
        return c.request("tools/call", {"name": name, "arguments": args})

    base = os.path.basename(path.rstrip("/"))
    print(f"--- {base}")
    def analyze(kind, **options):
        return call(*analysis_call(kind, path, options))

    det = result_json(call("openreadout_info", {"file": path, "view": "format"})[0], "info format")
    info = result_json(call("openreadout_info", {"file": path})[0], "info")
    ex = result_json(call("openreadout_info", {"file": path, "view": "explain"})[0], "info explain")
    check(ex["summary"] and isinstance(ex["paragraphs"], list), "info explain")
    ok = result_json(call("openreadout_check", {"file": path})[0], "check")["ok"]
    ls = result_json(call("openreadout_info", {"file": path, "view": "structure"})[0], "info structure")
    rep = result_json(call("openreadout_report", {"file": path,
                                                 "output": os.path.join(tmp, base + ".report.json")})[0], "check report")
    check(rep["output"] and rep["report"]["stages"][0]["stage"] == "detect", "report")
    check(base not in json.dumps(rep["report"]), "report bundle contains the file name")
    dump = result_json(call("openreadout_info", {"file": path, "view": "full", "max_frames": 2})[0], "info full")
    check(dump["file"]["format"]["id"] == det["format"], "info full format")
    print(f"detect {det['format']}, check ok={ok}, {len(info['images'])} images, {len(info.get('tables', []))} tables, "
          f"{len(info.get('traces', []))} traces, {len(info.get('spectra', []))} spectrum runs, {len(ls['entries'])} ls entries")

    r = c.request("resources/read", {"uri": "openreadout://file/" + os.path.abspath(path)})[0]
    contents = r["result"]["contents"]
    check(json.loads(contents[0]["text"])["format"]["id"] == det["format"] and contents[1]["text"], "file resource")

    r = call("openreadout_preview", {"file": path, "max_size": 256})[0]
    if "result" in r:
        out = result_json(r, "preview")
        img = [x for x in r["result"]["content"] if x["type"] == "image"]
        check(len(img) == 1, "preview: expected one image block")
        raw, w, h = check_image(img[0]["data"], img[0]["mimeType"], "preview")
        check(max(out["width"], out["height"]) <= 256, f"preview size {out['width']}x{out['height']}")
        if w is not None:
            check((w, h) == (out["width"], out["height"]), "preview PNG size differs from the report")
        print(f"preview {out['kind']} {out['width']}x{out['height']} {out['encoding']} {len(raw)} bytes")
        r = c.request("resources/read", {"uri": "openreadout://preview/" + os.path.abspath(path)})[0]
        blob = r["result"]["contents"][0]
        check_image(blob["blob"], blob["mimeType"], "preview resource")
    else:
        check(r["error"]["data"]["exit_code"] in (2, 6), f"preview error: {r['error']}")
        print("preview not available:", r["error"]["message"][:120])

    if info["images"]:
        for fmt, name in (("ome-tiff", "x.ome.tiff"), ("ome-zarr", "x.ome.zarr")):
            args = {"name": "openreadout_export", "_meta": {"progressToken": f"p-{fmt}"},
                    "arguments": {"file": path, "format": fmt, "image": 0, "output": os.path.join(tmp, name), "overwrite": True}}
            called.add("openreadout_export")
            r, notes = c.request("tools/call", args)
            rep = result_json(r, f"export {fmt}")
            check(rep["verified"], f"export {fmt} not verified")
            prog = [n["params"] for n in notes if n.get("method") == "notifications/progress"]
            check(prog and all(p["progressToken"] == f"p-{fmt}" for p in prog), f"export {fmt}: no progress notifications")
            check(prog[-1]["progress"] == prog[-1]["total"], f"export {fmt}: progress did not reach the total")
            print(f"export {fmt}: {rep['planes_written']} planes verified, {len(prog)} progress notifications")
        st = result_json(call("openreadout_stats", {"file": path, "image": 0, "select": ["z=0", "t=0"], "bins": 8})[0], "stats")
        check(st["channels"] and st["channels"][0]["stats"]["count"] > 0, "stats: no samples counted")
        cmp_ = result_json(call("openreadout_compare", {"file": path, "against": os.path.join(tmp, "x.ome.tiff"),
                                                         "image": 0, "select": ["z=0", "t=0"]})[0], "compare")
        check(cmp_["planes"]["mismatched"] == 0, f"check against our OME-TIFF export: {cmp_['planes']}")
        print(f"stats: {len(st['channels'])} channels; check against OME-TIFF: {cmp_['planes']['identical']} planes identical")
    if info.get("tables"):
        t = result_json(call("openreadout_table", {"file": path, "max_rows": 3})[0], "table")
        print(f"table: {t['columns'][:4]} total {t['total_rows']}")
        csv_out = os.path.join(tmp, base + ".table.csv")
        e = result_json(call("openreadout_export", {"file": path, "format": "csv", "table": 0,
                                                    "output": csv_out})[0], "export csv")
        check(e["verified"] and os.path.exists(csv_out), f"export csv: {e}")
        print(f"export csv: {e['rows_written']} rows, verified")
    if det["format"] == "plate":
        r = analyze("assay", analysis="wells", reduce="last")[0]
        a = result_json(r, "assay")
        check(a["read"]["wells_measured"] > 0, "assay: no wells")
        r = analyze("assay", analysis="wells", no_such_field=1)[0]
        check("error" in r or r.get("result", {}).get("isError"), "assay: an unknown argument must be refused")
        print(f"assay wells: {a['read']['wells_measured']} wells, roles {a['layout']['roles']}")
    if info.get("traces"):
        t = result_json(call("openreadout_trace", {"file": path, "max_samples": 3})[0], "trace")
        print(f"trace: {t['sample_count']} samples, {len(t['channels'])} channels")
        fam = info["format"]["family"]
        if fam == "chromatography":
            ch = result_json(analyze("chromatogram", traces=[0])[0], "chromatogram")
            pk = result_json(analyze("peaks", summary_only=True)[0], "peaks")
            print(f"chromatogram apex {ch['chromatograms'][0].get('apex_rt_min')} min; "
                  f"peaks: {pk['chromatograms'][0]['peak_count']}")
            r = call("openreadout_batch", {"measure": "peaks", "paths": [path],
                                             "options": {"traces": [0], "rows": "chromatogram"}})[0]
            b = result_json(r, "batch peaks")
            check(b["total_rows"] == 1, f"batch peaks: {b['total_rows']} rows")
            r = call("openreadout_batch", {"measure": "peaks", "paths": [path], "options": {"mzz": 1}})[0]
            check("error" in r or r.get("result", {}).get("isError"), "batch: an unknown option must be refused")
        elif fam == "nmr":
            n = result_json(analyze("nmr-peaks")[0], "nmr-peaks")
            print(f"nmr_peaks: {n['peak_count']} peaks, source {n['source']}")
        elif fam == "electrophysiology":
            if det["format"] in ("abf", "atf"):
                e = result_json(analyze("ephys-features")[0], "ephys-features")
                check(len(e["spikes"]) <= 25, "ephys_features: default spike rows")
                print(f"ephys_features: {len(e['sweeps'])} sweeps, {e['spike_count_total']} spikes")
            else:
                sp = result_json(analyze("spikes", max_seconds=1.0)[0], "spikes")
                print(f"spikes: {sp['spike_count_total']} spikes")
    if info.get("spectra"):
        s = result_json(call("openreadout_spectrum", {"file": path, "spectrum": 0, "max_points": 5})[0], "spectrum")
        print(f"spectrum: scan {s['spectrum']['scan_number']}, {s['point_count']} points")
        sc = result_json(call("openreadout_scans", {"file": path, "limit": 3})[0], "scans")
        check(sc["returned"] <= 3 and sc["matched"] == sc["scan_count"], "scans: every scan counted, 3 listed")
        print(f"scans: {sc['matched']} scans ({sc['ms_level_counts']}), source {sc['source']}")
        r, notes = c.request("tools/call", {"name": "openreadout_export", "_meta": {"progressToken": "p-mzml"},
                                            "arguments": {"file": path, "format": "mzml", "output": os.path.join(tmp, "x.mzML"), "overwrite": True}})
        called.add("openreadout_export")
        rep = result_json(r, "export mzml")
        prog = [n for n in notes if n.get("method") == "notifications/progress"]
        print(f"export mzml: verified={rep.get('verified')}, {len(prog)} progress notifications")
    if info.get("plate"):
        ws = result_json(call("openreadout_stats", {"file": path, "per": "well", "select": ["c=0"]})[0], "stats per well")
        print(f"well_stats: {len(ws['rows'])} rows")
    atts = [e for e in ls["entries"] if e["kind"] == "attachment"]
    if atts:
        r = call("openreadout_extract", {"file": path, "attachment": f"#{atts[0]['details']['index']}",
                                           "output": os.path.join(tmp, "att.bin"), "overwrite": True})[0]
        check(result_json(r, "extract")["verified"], "extract: attachment not verified")
        print(f"extract: {atts[0]['name']}")
    else:
        r = call("openreadout_extract", {"file": path, "attachment": "#0",
                                           "output": os.path.join(tmp, "att.bin")})[0]
        check(r.get("error", {}).get("data", {}).get("hint"), f"extract without attachments: {r}")


def index_and_search(c, tmp, called):
    """Index the temporary directory (the synthetic files and every export written so far), with a
    progress token, then search it."""
    idx = tempfile.mkdtemp()
    try:
        r, notes = c.request("tools/call", {"name": "openreadout_index", "_meta": {"progressToken": "p-index"},
                                            "arguments": {"roots": [tmp], "index_dir": idx, "threads": 2}})
        called.add("openreadout_index")
        m = result_json(r, "index")
        check(m.get("complete") is True, f"index incomplete: {m.get('next')}")
        prog = [n for n in notes if n.get("method") == "notifications/progress"]
        check(prog, "index sent no progress notifications")
        r = c.request("tools/call", {"name": "openreadout_search",
                                     "arguments": {"index_dir": idx, "query": "", "limit": 5}})[0]
        called.add("openreadout_search")
        s = result_json(r, "search")
        check(s.get("total") == m.get("datasets"), f"search total {s.get('total')} != {m.get('datasets')} data sets")
        print(f"index: {m['datasets']} data sets, {m['crawl']['items']} items, {len(prog)} progress notifications; "
              f"search: {s['total']} matches")
        r = c.request("tools/call", {"name": "openreadout_health",
                                     "arguments": {"index_dir": idx, "no_hash": True}})[0]
        called.add("openreadout_health")
        h = result_json(r, "health")
        check(isinstance(h, dict) and h, f"health: {h}")
        print("health answered")
        r = c.request("tools/call", {"name": "openreadout_watch",
                                     "arguments": {"dirs": [tmp], "since": "all"}})[0]
        called.add("openreadout_watch")
        w = result_json(r, "watch")
        news = [e for e in w["events"] if e["event"] == "dataset_new"]
        check(news, f"watch reported no data sets: {w}")
        r = c.request("tools/call", {"name": "openreadout_watch",
                                     "arguments": {"dirs": [tmp], "cursor": w["cursor"]}})[0]
        w2 = result_json(r, "watch")
        check(all(e["seq"] > w["cursor"] for e in w2["events"]), "watch cursor returned old events")
        print(f"watch: {len(news)} data sets, {len(w['events'])} events, cursor {w['cursor']}")
        rows = os.path.join(idx, "rows.csv")
        r = c.request("tools/call", {"name": "openreadout_batch",
                                     "arguments": {"measure": "info", "paths": [tmp], "recursive": True,
                                                   "output": rows}})[0]
        called.add("openreadout_batch")
        b = result_json(r, "batch info")
        check(b["total_rows"] >= 1, "batch info: no rows")
        r = c.request("tools/call", {"name": "openreadout_summarize",
                                     "arguments": {"table": rows, "by": ["format"]}})[0]
        called.add("openreadout_summarize")
        result_json(r, "summarize")
        r = c.request("tools/call", {"name": "openreadout_link", "arguments": {"paths": [tmp]}})[0]
        called.add("openreadout_link")
        result_json(r, "link")
        print(f"batch info: {b['total_rows']} rows; summarize and link answered")
    finally:
        shutil.rmtree(idx, ignore_errors=True)


def main(argv):
    http = synthetic = False
    skip = set()
    args = []
    it = iter(argv)
    for a in it:
        if a == "--http":
            http = True
        elif a == "--synthetic":
            synthetic = True
        elif a == "--skip":
            skip.add(next(it))
        else:
            args.append(a)
    if not args or (len(args) < 2 and not synthetic):
        raise SystemExit(__doc__)
    binary, files = args[0], args[1:]
    tmp = tempfile.mkdtemp()
    if synthetic:
        for name, write in (("smoke.jdx", write_jcamp), ("smoke.mzML", write_mzml), ("smoke.fcs", write_fcs),
                            ("smoke.rdml", write_rdml), ("smoke-lc.mzML", write_lc_mzml),
                            ("smoke-1h.jdx", write_nmr_jcamp), ("smoke-cc.atf", write_atf),
                            ("smoke-csc.ncs", write_ncs), ("smoke-plate.csv", write_plate),
                            ("smoke-plate.ome.zarr", write_zarr_plate)):
            write(os.path.join(tmp, name))
            files.append(os.path.join(tmp, name))
        write_gatingml(os.path.join(tmp, "smoke-gates.xml"))
    out = os.path.join(tmp, "out")
    os.mkdir(out)
    if http:
        http_security(binary)
    c = Http(binary) if http else Stdio(binary)
    called = set()
    t0 = time.time()
    try:
        surface(c)
        for f in files:
            per_file(c, f, out, called)
        if synthetic:
            synthetic_analyses(c, tmp, called)
            analysis_tools(c, tmp, called)
        index_and_search(c, tmp, called)
        v = Http(binary) if http else Stdio(binary)
        try:
            viewer(v, files)
        finally:
            v.close()
    finally:
        c.close()
        shutil.rmtree(tmp, ignore_errors=True)
    missing = EXPECTED_TOOLS - called - skip
    check(not missing, f"tools never called (add a file that exercises them, or --skip them): {sorted(missing)}")
    print(f"OK ({'http' if http else 'stdio'}): every tool called, {time.time() - t0:.1f} s")


if __name__ == "__main__":
    main(sys.argv[1:])
