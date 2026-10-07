"""Mass spectrometry: a second reading of every run.

- Vendor files (Thermo .raw, Agilent .d, Waters .raw, Sciex .wiff) with a conversion made by the
  vendor's own library (the depositor's or ProteoWizard's mzML/mzXML): the conversion is read
  with pyopenms (OpenMS, BSD-3; not pyteomics, which the primary oracle uses) for the spectrum
  list (count, MS levels, retention times, polarity, precursor m/z and charge), and its header
  with lxml for the run's start time stamp, instrument model and serial number, and the
  acquisition software.
- Open formats (mzML, mzXML, mzMLb) are read the same way (mzMLb: the mzML document stored in
  the HDF5 container, read with h5py; its spectra with pyopenms from that document).
- Bruker timsTOF .d (analysis.tdf/.tsf are SQLite): Python's sqlite3 on the frame and precursor
  tables and GlobalMetadata (timsrust is the primary oracle).
"""
from __future__ import annotations

import re
import sqlite3
from importlib.metadata import version
from pathlib import Path

from . import FILES, Rec, spread

FAMILY = "mass-spectrometry"
FORMATS = {"thermo-raw", "agilent-masshunter", "waters-raw", "sciex-wiff", "mzml", "mzxml", "mzmlb", "bruker-tdf"}
SCANS = ("scans", "--limit", "100000000")


def find_export(entry: dict, path: Path, ctx) -> Path | None:
    for rid in entry.get("mz_references", []):
        e = ctx.entry(rid)
        if e and "centroid" not in rid:
            q = ctx.path(e)
            if q.exists() and q.suffix.lower() in (".mzml", ".mzxml"):
                return q
    o = ctx.oracle(entry["id"]) or {}
    name = o.get("paired_export")
    if not name:
        return None
    for e in ctx.manifest:
        if e.get("role") == "oracle-export" and Path(e["filename"]).name == name:
            q = ctx.path(e)
            if q.exists():
                return q
    for d in (path.parent, path.parent.parent):
        q = d / name
        if q.exists():
            return q
    return None


def _cv(el, ns):
    return {c.get("accession"): (c.get("name"), c.get("value")) for c in el.findall(f"{ns}cvParam")}


def mzml_header(q: Path) -> dict:
    """Run-level metadata from an mzML header (lxml iterparse, stops at the first spectrum)."""
    from lxml import etree
    out: dict = {"software": []}
    groups: dict = {}
    for ev, el in etree.iterparse(str(q), events=("end",), huge_tree=True):
        tag = etree.QName(el).localname
        ns = "{%s}" % etree.QName(el).namespace if etree.QName(el).namespace else ""
        if tag == "referenceableParamGroup":
            groups[el.get("id")] = _cv(el, ns)
        elif tag == "software":
            names = [n for a, (n, v) in _cv(el, ns).items()]
            out["software"].append((el.get("id"), el.get("version"), names))
        elif tag == "instrumentConfiguration" and "instrument" not in out:
            cv = _cv(el, ns)
            for ref in el.findall(f"{ns}referenceableParamGroupRef"):
                cv.update(groups.get(ref.get("ref"), {}))
            out["instrument"] = cv
        elif tag == "run":
            break
        elif tag in ("spectrum", "chromatogram"):
            break
    # the <run> start tag is read by a plain scan (iterparse ends it only after every spectrum)
    head = q.open("rb").read(4 << 20).decode("utf-8", "replace")
    m = re.search(r"<run\b[^>]*\bstartTimeStamp=\"([^\"]+)\"", head)
    if m:
        out["start"] = m.group(1)
    return out


def mzxml_header(q: Path) -> dict:
    head = q.open("rb").read(1 << 20).decode("utf-8", "replace")
    out: dict = {}
    m = re.search(r"<msRun\b[^>]*\bstartTime=\"([^\"]+)\"", head)
    if m:
        out["start_time_rel"] = m.group(1)
    for k in ("msManufacturer", "msModel"):
        m = re.search(rf"<{k}\b[^>]*\bvalue=\"([^\"]*)\"", head)
        if m:
            out[k] = m.group(1)
    m = re.search(r"<software\b[^>]*type=\"acquisition\"[^>]*>", head)
    if m:
        a = dict(re.findall(r'(\w+)="([^"]*)"', m.group(0)))
        out["software"] = (a.get("name"), a.get("version"))
    return out


MODEL_PARENTS = None


def instrument_model(cv: dict) -> str | None:
    """The instrument model term of an mzML instrument configuration: pyopenms' ControlledVocabulary
    (the PSI-MS ontology shipped with OpenMS) says which terms are models (children of MS:1000031)."""
    global MODEL_PARENTS
    import pyopenms as oms
    if MODEL_PARENTS is None:
        voc = oms.ControlledVocabulary()
        try:
            voc.loadFromOBO("PSI-MS", oms.File.find("CV/psi-ms.obo", []))
        except Exception:
            voc = None
        MODEL_PARENTS = voc
    for acc, (name, value) in cv.items():
        if acc == "MS:1000031":  # "instrument model" with the model as value
            return value or name
        if MODEL_PARENTS is not None:
            try:
                if MODEL_PARENTS.isChildOf(acc, "MS:1000031"):
                    return name
            except Exception:
                pass
    return None


def load_meta(q: Path):
    """pyopenms spectra without peak data."""
    import pyopenms as oms
    exp = oms.MSExperiment()
    opts = oms.PeakFileOptions()
    opts.setFillData(False)
    if q.suffix.lower() == ".mzxml":
        f = oms.MzXMLFile()
    else:
        f = oms.MzMLFile()
    f.setOptions(opts)
    f.load(str(q), exp)
    return exp


def emr_ids(q: Path) -> set[str]:
    """Native ids of the conversion's non-MS spectra (`electromagnetic radiation spectrum`,
    MS:1000804: DAD/PDA absorbance spectra), which pyopenms loads like MS1 spectra."""
    import mmap
    with q.open("rb") as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as m:
        if m.find(b"MS:1000804") < 0:
            return set()
    from lxml import etree
    out = set()
    for _, el in etree.iterparse(str(q), events=("end",), tag="{*}spectrum", huge_tree=True):
        if any(c.get("accession") == "MS:1000804" for c in el.iterfind("{*}cvParam")):
            out.add(el.get("id"))
        el.clear()
    return out


def is_ms(spec, emr: set[str]) -> bool:
    nid = spec.getNativeID()
    if re.search(r"controllerType=[1-9]", nid):  # Thermo LC-detector spectra
        return False
    if nid in emr:
        return False
    return spec.getMSLevel() >= 1


POL = {0: None, 1: "positive", 2: "negative"}


def spectra_checks(rec: Rec, exp, reader: int, key: str, keyfn, emr: set[str], entry: dict, rt_abs=1e-3,
                   count=True):
    specs = [s for s in exp.getSpectra() if is_ms(s, emr)]
    if not specs:
        return
    levels: dict = {}
    for s in specs:
        levels[str(s.getMSLevel())] = levels.get(str(s.getMSLevel()), 0) + 1
    # our scan list also holds LC-detector spectra (MS level 0) of some files: MS scans only
    MS = "/scans/{ms_level!=0}"
    if count:
        rec.check("spectrum count", f"{MS}#len", len(specs), cmd=SCANS, scope="spectra", reader=reader)
        for lvl, n in sorted(levels.items()):
            rec.check(f"MS{lvl} spectrum count", f"/ms_level_counts/{lvl}", n, cmd=SCANS, scope="spectra", reader=reader)
        rts = [s.getRT() for s in specs if s.getRT() >= 0]  # pyopenms: -1 when a spectrum has none
        if rts:
            rec.check("first retention time", f"{MS}/rt_s#min", min(rts), cmd=SCANS, cmp="num", abs=rt_abs,
                      scope="spectra", reader=reader)
            rec.check("last retention time", f"{MS}/rt_s#max", max(rts), cmd=SCANS, cmp="num", abs=rt_abs,
                      scope="spectra", reader=reader)
    pols = sorted({POL.get(s.getInstrumentSettings().getPolarity()) for s in specs} - {None})
    rec.check("polarities", "/experiment/method/parameters/polarity/value", pols, cmp="set", reader=reader)
    idx = spread(len(specs))
    lv, rt, pol, pmz, chg = {}, {}, {}, {}, {}
    for i in idx:
        s = specs[i]
        k = str(i) if keyfn is None else keyfn(s)
        if k is None:
            continue
        lv[k] = s.getMSLevel()
        # pyopenms: -1 when the spectrum has no scan start time; ours must then have none (null)
        rt[k] = s.getRT() if s.getRT() >= 0 else None
        p = POL.get(s.getInstrumentSettings().getPolarity())
        if p:
            pol[k] = p
        pre = s.getPrecursors()
        # MS2 only: for MSn (SPS, MS3) "the" precursor is a convention (first or last stage)
        if pre and s.getMSLevel() == 2 and not entry.get("precursor_not_compared"):
            if pre[0].getMZ() > 0:
                pmz[k] = pre[0].getMZ()
            if pre[0].getCharge() > 0:
                chg[k] = pre[0].getCharge()
    kw = dict(cmd=SCANS, scope="spectra", reader=reader, by=key)
    rec.check("scan MS level", MS, lv, each="/ms_level", **kw)
    rec.check("scan retention time", MS, rt, each="/rt_s", cmp="num", abs=rt_abs, **kw)
    rec.check("scan polarity", MS, pol, each="/polarity", cmp="text", **kw)
    rec.check("scan precursor m/z", MS, pmz, each="/precursor_mz", cmp="num",
              rel=entry.get("precursor_tolerance", 1e-6), **kw)
    rec.check("scan precursor charge", MS, chg, each="/precursor_charge", **kw)


GENERIC = re.compile(r"instrument model$|^unknown$", re.I)


def header_checks(rec: Rec, q: Path, reader: int, fmt: str):
    if q.suffix.lower() == ".mzxml":
        h = mzxml_header(q)
        rec.check("instrument model", "/experiment/instrument/model", h.get("msModel"), cmp="text", reader=reader)
        sw = h.get("software")
        if sw and sw[1] and not GENERIC.search(sw[1]):
            rec.check("acquisition software version", "/experiment/instrument/software_version", sw[1],
                      cmp="prefix", reader=reader)
        return
    h = mzml_header(q)
    cv = h.get("instrument", {})
    model = instrument_model(cv)
    if model and not GENERIC.search(model):  # "Waters instrument model": the converter did not know it
        rec.check("instrument model", "/experiment/instrument/model", model, cmp="text", reader=reader)
    serial = cv.get("MS:1000529", (None, None))[1]
    if serial not in (None, "", "0"):
        rec.check("instrument serial number", "/experiment/instrument/serial", serial, cmp="text", reader=reader)
    vendor_sw = [s for s in h["software"] if not any(
        w in (s[0] or "").lower() + " ".join(s[2]).lower() for w in ("pwiz", "proteowizard", "msconvert", "openms", "conversion", "psims"))]
    # (ProteoWizard writes its Agilent library's version, "8.0", for every MassHunter version)
    if fmt != "agilent-masshunter" and len(vendor_sw) == 1 and vendor_sw[0][1] and not GENERIC.search(vendor_sw[0][1]):
        rec.check("acquisition software version", "/experiment/instrument/software_version", vendor_sw[0][1],
                  cmp="prefix", reader=reader)
    if h.get("start") and re.match(r"\d{4}-\d{2}-\d{2}", h["start"]):  # (not "-infinity")
        # ProteoWizard writes the converting computer's local clock with a "Z" (compare
        # docs/benchmark/second-opinions.md): the same reading up to a zone offset
        rec.check("acquisition start (up to a zone offset)", "/experiment/acquisition/started_at", h["start"],
                  cmp="time_mod_zone", abs=2.0, reader=reader)


def native_key(s):
    return s.getNativeID()


def thermo_key(s):
    m = re.search(r"\bscan=(\d+)", s.getNativeID())
    return m.group(1) if m else None


MS_DEVICE = re.compile(r"quad|tof|trap|mass|ms", re.I)


def agilent_acqdata(rec: Rec, path: Path) -> None:
    """The .d's own XML (AcqData/Contents.xml, Devices.xml, MSTS.xml), read with ElementTree."""
    import xml.etree.ElementTree as ET
    acq = path / "AcqData"
    if not acq.is_dir():
        return
    r = rec.reader("the .d's AcqData XML (Contents.xml, Devices.xml, MSTS.xml), read with ElementTree")

    def load(name):
        p = acq / name
        if not p.exists():
            return None
        text = p.read_bytes().decode("utf-8-sig", "replace")
        text = re.sub(r"\sxmlns(:\w+)?=\"[^\"]*\"", "", text, count=0)
        try:
            return ET.fromstring(text)
        except ET.ParseError:
            return None
    c = load("Contents.xml")
    if c is not None and c.findtext("AcquiredTime"):
        rec.check("acquisition start (Contents.xml)", "/experiment/acquisition/started_at",
                  c.findtext("AcquiredTime").strip(), cmp="time", reader=r)
    d = load("Devices.xml")
    if d is not None:
        ms = [x for x in d.findall("Device") if MS_DEVICE.search(x.findtext("Name") or "")]
        if ms:
            rec.check("instrument model (Devices.xml)", "/experiment/instrument/model",
                      (ms[0].findtext("ModelNumber") or "").strip(), cmp="text", reader=r)
            sn = (ms[0].findtext("SerialNumber") or "").strip()
            if sn and not re.fullmatch(r"[A-Z]*x+", sn):
                rec.check("instrument serial (Devices.xml)", "/experiment/instrument/serial", sn, cmp="text", reader=r)
    t = load("MSTS.xml")
    # (ion-mobility .d: MSTS.xml counts frames, each holding many drift scans)
    if t is not None and not (acq / "IMSFrame.bin").exists():
        n = [int(x.text) for x in t.iter("NumOfScans") if (x.text or "").strip().isdigit()]
        if n:
            rec.check("scans in the time segments (MSTS.xml)", "/scans/{ms_level!=0}#len", sum(n), cmd=SCANS,
                      scope="spectra", reader=r)


def waters_header(rec: Rec, path: Path) -> None:
    """The .raw's _HEADER.TXT ('$$ Key: value' lines, written by MassLynx), read as text."""
    from datetime import datetime
    p = path / "_HEADER.TXT"
    if not p.exists():
        return
    kv = {}
    for line in p.read_bytes().decode("latin-1").splitlines():
        m = re.match(r"\$\$ ([^:]+):\s?(.*)$", line)
        if m:
            kv[m.group(1).strip()] = m.group(2).strip()
    r = rec.reader("the .raw's _HEADER.TXT (MassLynx text header), read as text")
    date, time = kv.get("Acquired Date"), kv.get("Acquired Time")
    if date and time:
        for f in ("%d-%b-%Y %H:%M:%S", "%d-%b-%y %H:%M:%S", "%d-%b-%Y %H:%M"):
            try:
                ts = datetime.strptime(f"{date} {time}", f)
                rec.check("acquisition start (_HEADER.TXT)", "/experiment/acquisition/started_at", ts.isoformat(),
                          cmp="time", reader=r)
                break
            except ValueError:
                continue
    inst = kv.get("Instrument")
    if inst:
        model = inst.split("#")[0].strip()
        rec.check("instrument (_HEADER.TXT)", "/experiment/instrument/model", model, cmp="text", reader=r)
    rec.check("operator (_HEADER.TXT User Name)", "/experiment/acquisition/operator", kv.get("User Name"), cmp="text", reader=r)
    # the description is the experiment's sample name (on the spectra run, or on the SRM table of MRM-only runs)
    rec.check("sample description (_HEADER.TXT)", "/experiment/sample/name", kv.get("Sample Description"), cmp="text", reader=r)
    rec.check("acquired name (_HEADER.TXT)", "/experiment/sample/id", kv.get("Acquired Name"), cmp="text", reader=r)


def sciex_container_time(rec: Rec, path: Path) -> None:
    """The .wiff is a compound file: its storages' creation times are FILETIMEs in UTC (MS-CFB);
    the earliest is when acquisition created the file. Read with olefile (BSD-2)."""
    import olefile
    o = olefile.OleFileIO(str(path))
    times = []
    for e in o.listdir(streams=False, storages=True):
        t = o.getctime("/".join(e))
        if t and t.year > 1980:
            times.append(t)
    o.close()
    if times:
        r = rec.reader(f"olefile {version('olefile')} (BSD-2): the .wiff storages' creation times (UTC)")
        first = min(times).replace(microsecond=0)
        rec.check("acquisition start (compound-file creation time, UTC)", "/experiment/acquisition/started_at",
                  first.isoformat() + "Z", cmp="time", abs=5.0, reader=r)


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    fmt = entry["format"]
    if fmt == "bruker-tdf":
        return tdf(rec, entry, path)
    if fmt == "agilent-masshunter":
        agilent_acqdata(rec, path)
    if fmt == "waters-raw":
        waters_header(rec, path)
    if fmt == "sciex-wiff":
        sciex_container_time(rec, path)
    if fmt in ("mzml", "mzxml", "mzmlb"):
        q = path
        if fmt == "mzmlb":
            q = mzmlb_document(path)
        elif path.suffix.lower() == ".gz":
            q = gunzip(path)
    else:
        q = find_export(entry, path, ctx)
        if q is None:
            return
    what = "the file" if fmt in ("mzml", "mzxml", "mzmlb") else f"the vendor-library conversion ({q.name})"
    r = rec.reader(f"pyopenms {version('pyopenms')} (BSD-3) on {what}")
    h = rec.reader(f"{what} header, read with lxml/regular expressions")
    exp = load_meta(q)
    match = entry.get("mz_match") or ("scan-number" if fmt == "thermo-raw" else "native-id")
    if entry.get("native_id_not_compared") or match == "index":
        key, keyfn = "/index", None
    elif match == "scan-number":
        key, keyfn = "/scan_number", thermo_key
    else:
        key, keyfn = "/native_id", native_key
    primary = ctx.oracle(entry["id"]) or {}
    if fmt in ("mzml", "mzxml", "mzmlb") or "spectra" in primary:
        # (a conversion the primary oracle does not compare spectrum by spectrum is not a full
        # conversion of the run: an excerpt, or MRM traces stored as one-point scans)
        # (mzXML conversions write retention times to 0.01 s)
        spectra_checks(rec, exp, r, key, keyfn, emr_ids(q), entry,
                       rt_abs=0.0051 if q.suffix.lower() == ".mzxml" and fmt != "mzxml" else 1e-3)
    header_checks(rec, q, h, fmt)


def gunzip(p: Path) -> Path:
    import gzip, hashlib, tempfile
    out = Path(tempfile.gettempdir()) / f"second-{hashlib.md5(str(p).encode()).hexdigest()}-{p.stem}"
    if not out.exists():
        out.write_bytes(gzip.decompress(p.read_bytes()))
    return out


def mzmlb_document(p: Path) -> Path:
    """The mzML XML an mzMLb stores in its `mzML` dataset, as a file (binary arrays are elsewhere
    in the container; pyopenms reads the spectrum list without peak data)."""
    import h5py, hashlib, tempfile
    import hdf5plugin  # noqa: F401  (Blosc and other filters)
    out = Path(tempfile.gettempdir()) / f"second-{hashlib.md5(str(p).encode()).hexdigest()}.mzML"
    if not out.exists():
        with h5py.File(p, "r") as f:
            out.write_bytes(bytes(f["mzML"][()]))
    return out


def tdf(rec: Rec, entry: dict, path: Path) -> None:
    db = path if path.suffix in (".tdf", ".tsf") else next(path.glob("analysis.t?f"))
    con = sqlite3.connect(f"file:{db}?mode=ro&immutable=1", uri=True)
    r = rec.reader(f"Python sqlite3 (SQLite {sqlite3.sqlite_version}) on {db.name}")
    tables = {t for (t,) in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    if "GlobalMetadata" not in tables or "Frames" not in tables:
        return
    meta = dict(con.execute("SELECT Key, Value FROM GlobalMetadata").fetchall())
    ex = "/spectra/0/extra"
    rec.check("instrument model", "/experiment/instrument/model", meta.get("InstrumentName"), cmp="text", reader=r)
    rec.check("instrument serial number", "/experiment/instrument/serial", meta.get("InstrumentSerialNumber"), cmp="text", reader=r)
    rec.check("acquisition software version", "/experiment/instrument/software_version",
              meta.get("AcquisitionSoftwareVersion"), cmp="text", reader=r)
    rec.check("acquisition start", "/experiment/acquisition/started_at", meta.get("AcquisitionDateTime"),
              cmp="time", zone=True, reader=r)
    rec.check("operator", "/experiment/acquisition/operator", meta.get("OperatorName"), cmp="text", reader=r)
    rec.check("sample name", "/experiment/sample/id", meta.get("SampleName"), cmp="text", reader=r)
    lo, hi = meta.get("MzAcqRangeLower"), meta.get("MzAcqRangeUpper")
    if lo and hi:
        rec.check("m/z acquisition range", f"{ex}/mz_acquisition_range", [float(lo), float(hi)], cmp="num", rel=1e-9, reader=r)
    cols = {c[1] for c in con.execute("PRAGMA table_info(Frames)")}
    frames = con.execute("SELECT COUNT(*) FROM Frames").fetchone()[0]
    rec.check("frame count", f"{ex}/frame_count", frames, reader=r)
    if "MsMsType" in cols:
        n = dict(con.execute("SELECT MsMsType, COUNT(*) FROM Frames GROUP BY MsMsType").fetchall())
        rec.check("MS1 frames", f"{ex}/ms1_frames", n.get(0, 0), reader=r)
        if db.suffix == ".tdf":
            rec.check("PASEF frames", f"{ex}/pasef_frames", n.get(8, 0), reader=r)
            rec.check("diaPASEF frames", f"{ex}/dia_frames", n.get(9, 0), reader=r)
    if "Polarity" in cols:
        pol = sorted({{"+": "positive", "-": "negative"}.get(p, p) for (p,) in con.execute("SELECT DISTINCT Polarity FROM Frames")})
        rec.check("polarities", "/experiment/method/parameters/polarity/value", pol, cmp="set", reader=r)
    if "NumScans" in cols and db.suffix == ".tdf":
        rec.check("mobility scans per frame", f"{ex}/scans_per_frame",
                  con.execute("SELECT MAX(NumScans) FROM Frames").fetchone()[0], reader=r)
    tables = {t for (t,) in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    if "Precursors" in tables:
        rec.check("precursor count", f"{ex}/precursor_count", con.execute("SELECT COUNT(*) FROM Precursors").fetchone()[0], reader=r)
    lo_k, hi_k = meta.get("OneOverK0AcqRangeLower"), meta.get("OneOverK0AcqRangeUpper")
    if lo_k and hi_k:
        rec.check("1/K0 acquisition range", f"{ex}/inverse_reduced_mobility_range", [float(lo_k), float(hi_k)],
                  cmp="num", rel=1e-9, reader=r)
    # DDA-PASEF scans are per precursor: each one's m/z, charge and frame time
    if "Precursors" in tables and "PasefFrameMsMsInfo" in tables:
        rows = con.execute("SELECT p.Id, p.MonoisotopicMz, p.LargestPeakMz, p.Charge, f.Time FROM Precursors p "
                           "JOIN Frames f ON f.Id = p.Parent ORDER BY p.Id").fetchall()
        pick = [rows[i] for i in spread(len(rows))]
        mz = {f"precursor={i}": (mono if mono else big) for i, mono, big, _, _ in pick if (mono or big)}
        ch = {f"precursor={i}": c for i, _, _, c, _ in pick if c}
        kw = dict(cmd=SCANS, scope="spectra", reader=r, by="/native_id")
        rec.check("scan precursor m/z", "/scans", mz, each="/precursor_mz", cmp="num", rel=1e-9, **kw)
        rec.check("scan precursor charge", "/scans", ch, each="/precursor_charge", **kw)
    con.close()
