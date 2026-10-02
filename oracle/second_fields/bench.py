"""Bench instruments: a second reading of each run's start and instrument.

The primary oracles (oracle/series_oracle.py, oracle/*_oracle.py) compare the sampled columns and
a few facts. These checks set the normalized `experiment.acquisition.started_at` and
`experiment.instrument.model` against another reader or the vendor's own export:

- BioLogic EC-Lab: the `.mpt` text header (`Acquisition started on :`, `Device :`), read with a
  regular expression; for a `.mpr` the EC-Lab export of the same run (its `.mpt`) is that reader.
- Gamry `.DTA`: gamry-parser (MIT) header (`DATE`, `TIME`, `PSTAT`).
- NETZSCH `.ngb-*`: pyNGB (MIT) metadata (`date_performed`, `instrument`).
- Neware `.nda`/`.ndax`: NewareNDA (BSD-3) first record's `Timestamp`.
- Bruker EPR BES3T: DeerLab's `deerload` (MIT, vendored in oracle/third_party) parameters
  (`DATE`, `TIME`).
- PANalytical `.xrdml`: the XML (`scan/header/startTimeStamp`, the `Diffractometer system=`
  comment), read with ElementTree.
- Rigaku `.ras`/`.rasx`: the `*MEAS_SCAN_START_TIME` and `*FILE_SYSTEM_NAME` header lines.
- Cytiva ÄKTA/UNICORN, Biacore: allotropy (MIT) ASM (`measurement time`, `model number`).
"""
from __future__ import annotations

import re
import sys
import zipfile
from datetime import datetime
from importlib.metadata import version
from pathlib import Path

from . import Rec

FAMILY = "bench"
FORMATS = {"biologic-mpt", "biologic-mpr", "gamry-dta", "netzsch-ngb", "neware-nda", "neware-ndax", "bruker-bes3t",
           "panalytical-xrdml", "rigaku-ras", "rigaku-rasx", "cytiva-unicorn-zip", "cytiva-biacore-blr",
           "bruker-brml", "genepix-gpr", "microcal-itc"}

T = "/experiment/acquisition/started_at"
M = "/experiment/instrument/model"


def us_time(date: str, time: str | None = None) -> str | None:
    """`03/02/2021 16:17:59.000` (month/day/year) → ISO local clock."""
    s = f"{date} {time}" if time else date
    for f in ("%m/%d/%Y %H:%M:%S.%f", "%m/%d/%Y %H:%M:%S", "%m/%d/%Y %I:%M:%S %p", "%m/%d/%y %H:%M:%S"):
        try:
            return datetime.strptime(s.strip(), f).isoformat()
        except ValueError:
            continue
    return None


def mpt_header(p: Path) -> dict:
    head = p.read_bytes()[:20000].decode("latin-1")
    out = {}
    m = re.search(r"Acquisition started on\s*:\s*(\S+ \S+)", head)
    if m:
        out["start"] = us_time(m.group(1))
    m = re.search(r"^Device\s*:\s*(.+?)(?:\s*\(SN.*)?$", head, re.M)
    if m:
        out["device"] = m.group(1).strip()
    return out


def biologic(rec: Rec, entry: dict, path: Path) -> None:
    mpt = path if path.suffix.lower() == ".mpt" else path.with_suffix(".mpt")
    if not mpt.exists() or not mpt_header(mpt):
        if path.suffix.lower() == ".mpr":
            galvani_mpr(rec, path)
        return
    h = mpt_header(mpt)
    what = "the file's" if mpt == path else f"the EC-Lab export of the same run ({mpt.name})"
    r = rec.reader(f"{what} text header, read with regular expressions")
    rec.check("Acquisition started on", T, h.get("start"), cmp="time", reader=r)
    rec.check("Device", M, h.get("device"), cmp="loose", reader=r)


def galvani_mpr(rec: Rec, path: Path) -> None:
    """A `.mpr` without an EC-Lab text export: galvani (GPL-3.0, run as a black box) log timestamp."""
    from galvani import BioLogic
    m = BioLogic.MPRfile(str(path))
    ts = getattr(m, "timestamp", None)
    if ts is not None:
        r = rec.reader(f"galvani {version('galvani')} (GPL-3.0, run as a black box): the log module's timestamp")
        rec.check("log module timestamp", T, ts.isoformat(), cmp="time", abs=0.001, reader=r)


def gamry(rec: Rec, entry: dict, path: Path) -> None:
    import warnings
    warnings.filterwarnings("ignore")
    import gamry_parser as gp
    g = gp.GamryParser(str(path))
    g.load()
    h = g.get_header()
    r = rec.reader(f"gamry-parser {version('gamry-parser')} (MIT) header")
    if h.get("DATE") and h.get("TIME"):
        rec.check("DATE TIME", T, us_time(str(h["DATE"]), str(h["TIME"])), cmp="time", reader=r)
    pstat = h.get("PSTAT")
    if isinstance(pstat, str) and pstat.strip():
        rec.check("PSTAT (potentiostat label)", "/experiment/instrument/serial", pstat.strip(), cmp="text", reader=r)


def netzsch(rec: Rec, entry: dict, path: Path) -> None:
    import json
    import pyngb
    t = pyngb.read_ngb(str(path))
    meta = json.loads(t.schema.metadata[b"file_metadata"])
    r = rec.reader(f"pyNGB {version('pyngb')} (MIT) metadata")
    d = meta.get("date_performed")
    if d:
        rec.check("date_performed", T, str(d), cmp="time", abs=1.0, reader=r)
    ins = meta.get("instrument")
    # the instrument name is `<model code><variant>-<serial>-<suffix>` (`STA449F3A-0157-M`, `DIL402SE-...`);
    # the model code is compared inside our model, which is the product name when the file's
    # instrument table carries one (`NETZSCH STA 449 F3 Jupiter`) and the instrument name otherwise
    m = re.match(r"([A-Za-z]+\d+(?:[A-Za-z]\d+)?)[A-Za-z0-9]*-", ins or "")
    if m:
        rec.check("instrument (model code before the serial)", M, m.group(1), cmp="contains", reader=r)


def neware(rec: Rec, entry: dict, path: Path) -> None:
    import NewareNDA
    df = NewareNDA.read(str(path), log_level="ERROR")
    if len(df) == 0 or "Timestamp" not in df.columns:
        return
    r = rec.reader(f"NewareNDA {version('NewareNDA')} (BSD-3): the first record's Timestamp")
    ts = df["Timestamp"].iloc[0]
    rec.check("first record Timestamp", T, str(ts).replace(" ", "T"), cmp="time_or_utc", abs=1.0, reader=r)


def bes3t(rec: Rec, entry: dict, path: Path) -> None:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "third_party"))
    from deerload import deerload  # MIT, vendored unchanged
    import contextlib
    import io
    dsc = path if path.suffix.lower() == ".dsc" else path.with_suffix(".DSC")
    if not dsc.exists():
        dsc = path.with_suffix(".dsc")
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            _, _, params = deerload(str(dsc), False, True)
    except ValueError as e:  # deerload refuses some data formats (IRFMT) before returning the parameters
        rec.note(f"deerload: {e}")
        return
    r = rec.reader("DeerLab deerload (MIT, vendored): the .DSC parameters")
    spl = params.get("SPL") or {}
    date, time = spl.get("DATE"), spl.get("TIME")
    if isinstance(date, str) and isinstance(time, str):
        rec.check("DATE TIME", T, us_time(date.strip("'\" "), time.strip("'\" ")), cmp="time", reader=r)


def xrdml(rec: Rec, entry: dict, path: Path) -> None:
    import xml.etree.ElementTree as ET
    root = ET.parse(path).getroot()
    ns = {"x": root.tag.split("}")[0].strip("{")} if root.tag.startswith("{") else {}
    q = (lambda t: f"x:{t}") if ns else (lambda t: t)
    r = rec.reader("the .xrdml XML, read with ElementTree")
    st = root.find(f".//{q('scan')}/{q('header')}/{q('startTimeStamp')}", ns)
    if st is not None and st.text:
        rec.check("scan/header/startTimeStamp", T, st.text.strip(), cmp="time", zone=True, reader=r)
    for e in root.iter(f"{{{ns['x']}}}entry" if ns else "entry"):
        m = re.match(r"\s*Diffractometer system\s*=\s*(.+)", e.text or "")
        if m:
            rec.check("comment `Diffractometer system=`", M, m.group(1).strip(), cmp="text", reader=r)
            break


def ras_lines(text: str) -> dict:
    out = {}
    for line in text.splitlines():
        m = re.match(r'\*(\w+)\s+"?(.*?)"?\s*$', line)
        if m and m.group(1) not in out:
            out[m.group(1)] = m.group(2)
    return out


def rigaku(rec: Rec, entry: dict, path: Path) -> None:
    if path.suffix.lower() == ".rasx":
        with zipfile.ZipFile(path) as z:
            names = [n for n in z.namelist() if n.lower().endswith(".xml")]
            text = "\n".join(z.read(n).decode("utf-8", "replace") for n in names)
        r = rec.reader("the .rasx MesurementConditions XML (`ScanInformation/StartTime`, `SystemName`), read with "
                       "regular expressions")
        m = re.search(r"<StartTime>([^<]+)</StartTime>", text)
        if m:
            rec.check("ScanInformation StartTime", T, m.group(1).strip(), cmp="time", zone=True, reader=r)
        m = re.search(r"<SystemName>([^<]+)</SystemName>", text)
        if m:
            rec.check("GeneralInformation SystemName", M, m.group(1).strip(), cmp="text", reader=r)
        return
    else:
        kv = ras_lines(path.read_bytes().decode("latin-1"))
        r = rec.reader("the .ras text header (`*KEY \"value\"` lines)")
    t = kv.get("MEAS_SCAN_START_TIME")
    if t:
        iso = us_time(t) or (t.replace(" ", "T") if re.match(r"\d{4}-\d{2}-\d{2}", t) else None)
        rec.check("MEAS_SCAN_START_TIME", T, iso, cmp="time_or_utc", reader=r)
    rec.check("FILE_SYSTEM_NAME", M, kv.get("FILE_SYSTEM_NAME"), cmp="text", reader=r)


def brml(rec: Rec, entry: dict, path: Path) -> None:
    with zipfile.ZipFile(path) as z:
        texts = {n: z.read(n).decode("utf-8", "replace") for n in z.namelist() if n.lower().endswith(".xml")}
    r = rec.reader("the .brml XML members, read with regular expressions")
    for n, t in sorted(texts.items()):
        m = re.search(r"<TimeStampStarted>([^<]+)</TimeStampStarted>", t)
        if m:
            rec.check("RawData TimeStampStarted", T, m.group(1).strip(), cmp="time", zone=True, reader=r)
            break
    for t in texts.values():
        m = re.search(r"<DeviceTypeDesc>([^<]+)</DeviceTypeDesc>", t)
        if m:
            rec.check("InstrumentDescription DeviceTypeDesc", M, m.group(1).strip(), cmp="text", reader=r)
            break


def allotropy_asm(rec: Rec, entry: dict, path: Path, vendor: str) -> None:
    import warnings
    warnings.filterwarnings("ignore")
    from allotropy.parser_factory import Vendor
    from allotropy.to_allotrope import allotrope_from_file
    asm = allotrope_from_file(str(path), getattr(Vendor, vendor))
    r = rec.reader(f"allotropy {version('allotropy')} (MIT) ASM")
    found = {}
    keys = ("measurement time", "model number", "equipment serial number", "injection time",
            "asset management identifier")

    def walk(v):
        if isinstance(v, dict):
            for k, x in v.items():
                lk = k.lower()
                if lk in keys and lk not in found \
                        and isinstance(x, str):
                    found[lk] = x
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)
    walk(asm)
    if found.get("measurement time"):
        rec.check("ASM measurement time", T, found["measurement time"], cmp="time_or_utc", abs=1.0, reader=r)
    if found.get("model number"):
        rec.check("ASM model number", M, found["model number"], cmp="loose", reader=r)
    if vendor == "CYTIVA_UNICORN":
        # allotropy writes the stored MethodStartTime with +00:00, dropping the file's
        # MethodStartTimeUtcOffsetMinutes: the wall clock is compared, not the instant.
        if found.get("injection time"):
            rec.check("ASM injection time (wall clock)", T, found["injection time"], cmp="wallclock", reader=r)
        if found.get("asset management identifier"):
            rec.check("ASM asset management identifier", M, found["asset management identifier"], cmp="text",
                      reader=r)


def gpr(rec: Rec, entry: dict, path: Path) -> None:
    head = path.read_bytes()[:20000].decode("latin-1")
    r = rec.reader("the .gpr ATF header records (`\"DateTime=...\"`, `\"Scanner=...\"`)")
    m = re.search(r'"DateTime=([^"]+)"', head)
    if m:
        v = m.group(1).strip()
        iso = None
        for f in ("%Y/%m/%d %H:%M:%S", "%m/%d/%Y %H:%M:%S", "%Y-%m-%d %H:%M:%S"):
            try:
                iso = datetime.strptime(v, f).isoformat()
                break
            except ValueError:
                continue
        rec.check("DateTime", T, iso, cmp="time", reader=r)
    m = re.search(r'"Scanner=([^"\[]+)', head)
    if m:
        rec.check("Scanner", M, m.group(1).strip(), cmp="text", reader=r)


def itc(rec: Rec, entry: dict, path: Path) -> None:
    lines = path.read_bytes()[:4000].decode("latin-1").splitlines()
    first = next((l for l in lines if l.startswith("%")), None)
    if first:
        r = rec.reader("the .itc file's first `%` line, read as text")
        # `%VPITC06.03.471`, `%ITC200_04.10.329`: the instrument's name, then its firmware version
        m = re.match(r"%\s*([A-Za-z-]*ITC(?:\d+(?=_))?)", first, re.I)
        if m:
            rec.check("first `%` line (instrument before the firmware version)", M, m.group(1), cmp="loose",
                      reader=r)


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    fmt = entry["format"]
    if fmt in ("biologic-mpt", "biologic-mpr"):
        biologic(rec, entry, path)
    elif fmt == "gamry-dta":
        gamry(rec, entry, path)
    elif fmt == "netzsch-ngb":
        netzsch(rec, entry, path)
    elif fmt in ("neware-nda", "neware-ndax"):
        neware(rec, entry, path)
    elif fmt == "bruker-bes3t":
        bes3t(rec, entry, path)
    elif fmt == "panalytical-xrdml":
        xrdml(rec, entry, path)
    elif fmt in ("rigaku-ras", "rigaku-rasx"):
        rigaku(rec, entry, path)
    elif fmt == "bruker-brml":
        brml(rec, entry, path)
    elif fmt == "cytiva-unicorn-zip":
        allotropy_asm(rec, entry, path, "CYTIVA_UNICORN")
    elif fmt == "cytiva-biacore-blr":
        allotropy_asm(rec, entry, path, "CYTIVA_BIACORE_T200_CONTROL")
    elif fmt == "genepix-gpr":
        gpr(rec, entry, path)
    elif fmt == "microcal-itc":
        itc(rec, entry, path)
