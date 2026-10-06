"""Plate-reader oracle: allotropy (MIT, Benchling) converts each export to Allotrope ASM JSON; we
summarize the ASM per detection mode. Imported by gen.py (branch `plate`) and asm_compare.py.

Summary per mode (`absorbance`, `fluorescence`, `luminescence`) over every *measured* value
(calculated-data documents are ignored; measurements that carry an error document are counted as
errors, not values):
  wells       distinct well locations with at least one value
  values      numeric values (points of data cubes count one each)
  errors      measurements with an error document
  wavelengths sorted distinct detector wavelength settings (nm), when stated
  value_xxh3  xxh3-128 over the sorted (row, col, value) triples, each packed little-endian as
              u32 row, u32 col (zero-based; `A1` = 0,0; lower-case rows follow `Z`), f64 bits
              (-0.0 as +0.0). `openreadout` computes the same from `read_table` (corpus harness).
Header: instrument model, serial number and software version where allotropy reports one.

Tecan i-control exports have no allotropy parser; for them a small independent reader here
(`icontrol_summary`) is the oracle.
"""
import json, math, os, re, struct
from pathlib import Path

import xxhash

# manifest id prefix -> allotropy Vendor name
VENDORS = {
    # exports allotropy rejects: this module's independent text readers (TEXT_DIALECTS)
    "bmg-table-": "BMG_CSV",
    "envision-text-": "ENVISION_CSV",
    "gen5-": "AGILENT_GEN5",
    "softmax-": "MOLDEV_SOFTMAX_PRO",
    "bmg-mars-": "BMG_MARS",
    "bmg-smart-control-": "BMG_LABTECH_SMART_CONTROL",
    "envision-": "PERKIN_ELMER_ENVISION",
    "kaleido-": "REVVITY_KALEIDO",
    "magellan-": "TECAN_MAGELLAN",
    "skanit-": "THERMO_SKANIT",
}
SUFFIXES = {".txt", ".csv", ".xlsx", ".xls"}


def vendor_for(name: str):
    for k, v in VENDORS.items():
        if name.startswith(k):
            return v
    return None


def parse_well(s: str):
    m = re.fullmatch(r"([A-Za-z]{1,2})0*(\d{1,3})", s.strip())
    if not m:
        return None
    letters, col = m.group(1), int(m.group(2))
    if len(letters) == 1:
        c = letters
        row = ord(c) - ord("A") if c.isupper() else 26 + ord(c) - ord("a")
    else:
        row = (ord(letters[0].upper()) - ord("A") + 1) * 26 + ord(letters[1].upper()) - ord("A")
    return row, col - 1


def digest(triples) -> str:
    buf = bytearray()
    for r, c, v in sorted((r, c, struct.unpack("<Q", struct.pack("<d", 0.0 if v == 0 else v))[0]) for r, c, v in triples):
        buf += struct.pack("<IIQ", r, c, v)
    return xxhash.xxh3_128_hexdigest(bytes(buf))


def mode_of(field: str):
    f = field.lower()
    if f.startswith("absor"):
        return "absorbance"
    if f.startswith("fluorescence"):
        return "fluorescence"
    if f.startswith("luminescence"):
        return "luminescence"
    return None


def allotropy_asm(path: Path, vendor: str) -> dict:
    from allotropy.parser_factory import Vendor
    from allotropy.to_allotrope import allotrope_from_file
    return allotrope_from_file(str(path), Vendor[vendor])


def summarize_asm(asm: dict) -> dict:
    agg = asm["plate reader aggregate document"]
    groups = {}
    def g(mode):
        return groups.setdefault(mode, {"wells": set(), "values": 0, "errors": 0, "wavelengths": set(), "triples": []})
    for doc in agg.get("plate reader document", []):
        for m in doc["measurement aggregate document"]["measurement document"]:
            well = parse_well(m.get("sample document", {}).get("location identifier", ""))
            if well is None:
                continue
            has_error = "error aggregate document" in m
            wl = None
            for dc in m.get("device control aggregate document", {}).get("device control document", []):
                if isinstance(dc.get("detector wavelength setting"), dict):
                    wl = dc["detector wavelength setting"].get("value")
            for k, v in m.items():
                mode = mode_of(k)
                if mode is None:
                    continue
                grp = g(mode)
                if isinstance(v, dict) and "cube-structure" in v:
                    vals = [x for x in (v.get("data", {}).get("measures") or [[]])[0] if x is not None]
                    if has_error:
                        # allotropy writes -0.0 for the cube points its error documents describe
                        grp["errors"] += 1
                        vals = [x for x in vals if not (x == 0 and math.copysign(1.0, x) < 0)]
                    for x in vals:
                        grp["values"] += 1
                        grp["triples"].append((*well, float(x)))
                        grp["wells"].add(well)
                elif isinstance(v, dict) and "value" in v:
                    if has_error:
                        grp["errors"] += 1
                        continue
                    grp["values"] += 1
                    grp["triples"].append((*well, float(v["value"])))
                    grp["wells"].add(well)
                else:
                    continue
                if wl is not None:
                    grp["wavelengths"].add(float(wl))
    out = []
    for mode in sorted(groups):
        grp = groups[mode]
        out.append({"mode": mode, "wells": len(grp["wells"]), "values": grp["values"], "errors": grp["errors"],
                    "wavelengths": sorted(grp["wavelengths"]), "value_xxh3": digest(grp["triples"])})
    dev = agg.get("device system document", {})
    ds = agg.get("data system document", {})
    header = {}
    for key, val in (("model", dev.get("model number")), ("serial_number", dev.get("equipment serial number")),
                     ("software_version", ds.get("software version"))):
        if val not in (None, "", "N/A", "n/a", "Unknown"):
            header[key] = str(val)
    return {"groups": out, "header": header, "documents": len(agg.get("plate reader document", []))}


def icontrol_summary(path: Path) -> dict:
    """Independent reader for Tecan i-control exports (text or XLSX): `Label:` sections with a
    `Mode` line and either a `<>` matrix or a `Cycle Nr.` kinetic list."""
    rows = []
    if path.suffix.lower() in (".xlsx", ".xls"):
        from python_calamine import CalamineWorkbook
        wb = CalamineWorkbook.from_path(str(path))
        for name in wb.sheet_names:
            for r in wb.get_sheet_by_name(name).to_python(skip_empty_area=False):
                rows.append(["" if c is None else (str(c) if not isinstance(c, float) else c) for c in r])
    else:
        text = path.read_bytes().decode("utf-8-sig", errors="replace")
        rows = [line.split("\t") for line in text.splitlines()]
    cell = lambda r, i: (r[i] if i < len(r) else "")
    groups, mode, header = {}, None, {}
    i = 0
    while i < len(rows):
        r = rows[i]
        first = str(cell(r, 0)).strip()
        # German exports write `Programm:`, `Gerät:` and `Modus` (`Absorption`)
        if first.startswith(("Device:", "Gerät:")) and "model" not in header:
            header["model"] = first.split(":", 1)[1].strip()
        if first.startswith(("Application:", "Programm:")) and "software_version" not in header:
            ver = [str(c) for c in r if " , " in str(c)]
            if ver:
                header["software_version"] = ver[0].split(" , ")[1].strip()
        if first in ("Mode", "Modus"):
            mode_text = next((str(c) for c in r[1:] if str(c).strip()), "")
            mode = "absorbance" if ("Absorb" in mode_text or "Absorp" in mode_text) else "fluorescence" if "Fluor" in mode_text else "luminescence" if "Lumin" in mode_text else None
        if first == "Measurement Wavelength" and mode is None:
            mode = "absorbance"
        if first.startswith("Label:"):
            mode = None
        if first == "<>":
            cols = [int(float(c)) for c in r[1:] if str(c).strip()]
            j = i + 1
            while j < len(rows) and re.fullmatch(r"[A-Z]", str(cell(rows[j], 0)).strip()):
                row = ord(str(cell(rows[j], 0)).strip()) - ord("A")
                for k, col in enumerate(cols):
                    v = cell(rows[j], k + 1)
                    try:
                        x = float(v)
                    except (TypeError, ValueError):
                        continue
                    groups.setdefault(mode or "absorbance", []).append((row, col - 1, x))
                j += 1
            i = j
            continue
        if first in ("Cycles / Well", "Zyklen / Well"):
            # several reads per well, kinetic: `<well>` + cycle numbers, then Time, Temp., Mean
            # (Mittelwert), StDev and one line per read position; the well's value is the Mean
            w = parse_well(str(cell(rows[i + 1], 0))) if i + 1 < len(rows) else None
            j = i + 2
            block = {}
            while j < len(rows) and str(cell(rows[j], 0)).strip() not in ("", "Cycles / Well", "Zyklen / Well"):
                block[str(cell(rows[j], 0)).strip()] = rows[j][1:]
                j += 1
            mean = block.get("Mean", block.get("Mittelwert"))
            if w and mean is not None:
                cycles = rows[i + 1][1:]
                for k, v in enumerate(mean):
                    if k >= len(cycles) or str(cycles[k]).strip() == "":
                        continue
                    try:
                        x = float(v)
                    except (TypeError, ValueError):
                        continue
                    groups.setdefault(mode or "absorbance", []).append((*w, x))
            i = j
            continue
        if first == "Well" and str(cell(r, 1)).strip() in ("Mean", "Mittelwert"):
            # several reads per well, endpoint: a list of wells with their Mean in column 2
            j = i + 1
            while j < len(rows) and parse_well(str(cell(rows[j], 0))):
                try:
                    groups.setdefault(mode or "absorbance", []).append((*parse_well(str(cell(rows[j], 0))), float(cell(rows[j], 1))))
                except (TypeError, ValueError):
                    pass
                j += 1
            i = j
            continue
        if first.startswith("Cycle Nr"):
            j = i + 1
            while j < len(rows):
                w = parse_well(str(cell(rows[j], 0)))
                lab = str(cell(rows[j], 0)).strip()
                if w:
                    for v in rows[j][1:]:
                        try:
                            x = float(v)
                        except (TypeError, ValueError):
                            continue
                        groups.setdefault(mode or "absorbance", []).append((*w, x))
                elif lab and not lab.startswith(("Time", "Temp")):
                    break
                j += 1
            i = j
            continue
        i += 1
    out = [{"mode": m, "wells": len({(a, b) for a, b, _ in t}), "values": len(t), "errors": 0, "wavelengths": [],
            "value_xxh3": digest(t)} for m, t in sorted(groups.items())]
    return {"groups": out, "header": header}


def tecan_csv_summary(path: Path) -> dict:
    """Independent pandas reader for the comma-delimited Tecan exports without `Label:` lines
    (i-control 1.11 CSV, SparkControl CSV), written from the text of the files
    (docs/provenance/plate-readers.md, 2026-09-24) and not from openreadout's parser.

    Read settings are `Mode` lines (`Mode,,,,Absorbance` or `Mode,Absorbance`; `Kinetic` is not a
    read) with an optional `Name` line; data tables start at a `Cycle Nr.` or `<>` line, with an
    optional title line just above (`OD600:600`, `OD600`). A table takes the read whose `Name` is
    its title (before `:`), else the next read in order. Each table is parsed by pandas in the
    orientation it has: cycles as rows (header `Cycle Nr.,Time [s],Temp. [°C],A1,…`), wells as
    rows (`Cycle Nr.,1,2,…` / `Time [s]` / `Temp.` / `A1,…`), or a well list (`<>,Value,…`).
    Numeric cells only; text cells (`OVER`) are not values, as in icontrol_summary."""
    import csv
    import io

    import pandas as pd

    raw = path.read_bytes()
    try:
        text = raw.decode("utf-8-sig")
    except UnicodeDecodeError:
        text = raw.removeprefix(b"\xef\xbb\xbf").decode("cp1252")
    lines = text.splitlines()
    rows = list(csv.reader(lines))
    first = lambda r: next((c.strip() for c in r if c.strip()), "")  # noqa: E731
    nonblank = lambda r: [c.strip() for c in r if c.strip()]  # noqa: E731
    header = {}
    for r in rows[:6]:
        cells = nonblank(r)
        if cells and cells[0].startswith("Device:"):
            header["model"] = cells[0].split(":", 1)[1].strip()
        if cells and cells[0].startswith("Application:"):
            ver = [c for c in cells if " , " in c]
            if ver:
                header["software_version"] = ver[0].split(" , ")[1].strip()
            elif "SparkControl" in cells[0] and len(cells) > 1:
                header["software_version"] = cells[1].lstrip("V")
    reads = []  # [mode, name, used]
    in_read = False
    for r in rows:
        cells = nonblank(r)
        head = r[0].strip() if r else ""
        if head.startswith("Cycle Nr") or head == "<>" or not cells:
            in_read = False
            continue
        if head == "Mode":
            if cells[1:2] == ["Kinetic"]:
                in_read = False
                continue
            text_mode = cells[1] if len(cells) > 1 else ""
            mode = ("absorbance" if "Absorb" in text_mode else "fluorescence" if "Fluor" in text_mode
                    else "luminescence" if "Lumin" in text_mode else None)
            reads.append([mode, None, False])
            in_read = True
        elif in_read and head == "Name" and len(cells) > 1:
            reads[-1][1] = cells[1]
    groups = {}
    starts = [i for i, r in enumerate(rows) if r and (r[0].strip().startswith("Cycle Nr") or r[0].strip() == "<>")]
    for k, i in enumerate(starts):
        title = None
        if i > 0 and len(nonblank(rows[i - 1])) == 1 and rows[i - 1][0].strip():
            title = rows[i - 1][0].strip().split(":", 1)[0].strip()
        pick = next((x for x in reads if not x[2] and title is not None and x[1] == title), None)
        pick = pick or next((x for x in reads if not x[2]), None)
        if pick is not None:
            pick[2] = True
        mode = pick[0] if pick else "absorbance"
        end = starts[k + 1] if k + 1 < len(starts) else len(rows)
        block = lines[i:end]
        head = rows[i]
        triples = []
        if head[0].strip().startswith("Cycle Nr") and len(head) > 1 and head[1].strip().startswith("Time"):
            df = pd.read_csv(io.StringIO("\n".join(block)), dtype=str, keep_default_na=False)
            df = df[pd.to_numeric(df.iloc[:, 0], errors="coerce").notna()]
            for col in df.columns:
                w = parse_well(str(col))
                if not w:
                    continue
                for v in pd.to_numeric(df[col], errors="coerce").dropna():
                    triples.append((*w, float(v)))
        else:
            df = pd.read_csv(io.StringIO("\n".join(block)), header=None, dtype=str, keep_default_na=False)
            value_cols = [1] if head[0].strip() == "<>" and head[1].strip() == "Value" else list(range(1, df.shape[1]))
            for _, rr in df.iloc[1:].iterrows():
                w = parse_well(str(rr.iloc[0]))
                if not w:
                    if str(rr.iloc[0]).strip() and not str(rr.iloc[0]).startswith(("Time", "Temp")):
                        break
                    continue
                for c in value_cols:
                    x = pd.to_numeric(rr.iloc[c], errors="coerce")
                    if pd.notna(x):
                        triples.append((*w, float(x)))
        groups.setdefault(mode, []).extend(triples)
    out = [{"mode": m, "wells": len({(a, b) for a, b, _ in t}), "values": len(t), "errors": 0, "wavelengths": [],
            "value_xxh3": digest(t)} for m, t in sorted(groups.items())]
    return {"groups": out, "header": header}


BINARY_VENDORS = {".pda": "MOLDEV_SOFTMAX_PRO", ".sda": "MOLDEV_SOFTMAX_PRO", ".xpt": "AGILENT_GEN5"}


def plate_export(binary: Path, export: Path) -> dict:
    """A vendor binary document (SoftMax Pro .pda/.sda, Gen5 .xpt) is checked against the text or
    XLSX export the depositor made of the same document: allotropy reads the export. Exports with
    bare-CR line ends (SoftMax Pro on the Mac) are given to allotropy with LF line ends (a copy);
    nothing else is changed."""
    import tempfile
    vendor = os.environ.get("PLATE_VENDOR") or BINARY_VENDORS.get(binary.suffix.lower())
    if vendor is None:
        raise RuntimeError(f"no plate oracle for {binary.name}")
    src = export
    raw = export.read_bytes()
    note = ""
    if export.suffix.lower() == ".txt" and b"\r" in raw and b"\n" not in raw:
        tmp = Path(tempfile.mkdtemp()) / export.name
        tmp.write_bytes(raw.replace(b"\r", b"\n"))
        src = tmp
        note = "; bare-CR line ends given to allotropy as LF"
    if export.suffix.lower() == ".xlsx" and vendor == "MOLDEV_SOFTMAX_PRO":
        # a workbook whose first sheet holds SoftMax Pro's text export: its cells written out
        # as tab-separated text (numbers as Python prints them, booleans as SoftMax writes them)
        import openpyxl
        ws = openpyxl.load_workbook(export, read_only=True, data_only=True).worksheets[0]

        def cell(v):
            if v is None:
                return ""
            if isinstance(v, bool):
                return "TRUE" if v else "FALSE"
            if isinstance(v, float) and v.is_integer():
                return str(int(v))
            return str(v)

        lines = ["\t".join(cell(v) for v in row).rstrip("\t") for row in ws.iter_rows(values_only=True)]
        tmp = Path(tempfile.mkdtemp()) / (export.stem + ".txt")
        tmp.write_text("\n".join(lines) + "\n", encoding="utf-8")
        note = "; its first sheet written out as tab-separated text"
        src = tmp
    import allotropy
    version = getattr(allotropy, "__version__", None)
    if version is None:
        from importlib.metadata import version as v
        version = v("allotropy")
    try:
        summary = summarize_asm(allotropy_asm(src, vendor))
        failed = None if all(g["errors"] == 0 for g in summary["groups"]) else "error documents for measured cells"
    except Exception as e:  # noqa: BLE001 - the reason is recorded
        summary, failed = None, f"{type(e).__name__}: {e}"[:200]
    if failed is None:
        return {"reader": f"allotropy {version} ({vendor}) on the depositor's export {export.name}{note}",
                "plate": summary}
    if vendor != "MOLDEV_SOFTMAX_PRO":
        raise RuntimeError(f"allotropy could not read {export.name}: {failed}")
    return {"reader": f"oracle/plate.py softmax_text_summary on the depositor's export {export.name}{note} "
                      f"(independent reader of the text; allotropy {version} failed: {failed})",
            "plate": softmax_text_summary(src)}


def softmax_text_summary(path: Path) -> dict:
    """An independent reading of a SoftMax Pro text export (written from the text, not from our
    Rust code): `Plate:` blocks in PlateFormat (rows of values under a column-number header) or
    TimeFormat (well-name header, one line per read); every non-empty numeric cell of the raw
    data is a (row, col, value) triple. Blank cells (reads never made) are skipped."""
    raw = path.read_bytes()
    if raw[:2] in (b"\xff\xfe", b"\xfe\xff"):
        text = raw.decode("utf-16")
    else:
        text = raw.decode("latin-1")
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    groups = {}
    i = 0
    while i < len(lines):
        f = lines[i].split("\t")
        if f[0] != "Plate:":
            i += 1
            continue
        mode = mode_of(f[5]) or "unknown"
        fl = mode == "fluorescence"
        o = 1 if fl else 0
        wls = [float(x) for x in f[15 + o].split()] if len(f) > 15 + o else []
        g = groups.setdefault(mode, {"t": [], "wl": set()})
        if mode != "luminescence":
            g["wl"].update(wls)
        fmt = f[3]
        j = i + 1
        header = lines[j].split("\t")
        j += 1
        if fmt == "TimeFormat":
            cols = [(k, parse_well(h)) for k, h in enumerate(header) if parse_well(h)]
            while j < len(lines) and lines[j].strip() and lines[j] != "~End":
                cells = lines[j].split("\t")
                for k, w in cols:
                    if k < len(cells) and cells[k].strip():
                        g["t"].append((w[0], w[1], float(cells[k])))
                j += 1
        else:
            ncol = [k for k, h in enumerate(header) if h.strip().isdigit()]
            r = 0
            while j < len(lines) and lines[j].strip() and lines[j] != "~End":
                cells = lines[j].split("\t")
                for k in ncol:
                    if k < len(cells) and cells[k].strip():
                        g["t"].append((r, int(header[k]) - 1, float(cells[k])))
                r += 1
                j += 1
        i = j
    out = [{"mode": m, "wells": len({(a, b) for a, b, _ in g["t"]}), "values": len(g["t"]), "errors": 0,
            "wavelengths": sorted(g["wl"]), "value_xxh3": digest(g["t"])} for m, g in sorted(groups.items())]
    return {"groups": out, "header": {}}


def _groups(groups: dict, wavelengths: dict | None = None) -> list:
    wavelengths = wavelengths or {}
    return [{"mode": m, "wells": len({(a, b) for a, b, _ in t}), "values": len(t), "errors": 0,
             "wavelengths": sorted(wavelengths.get(m, [])), "value_xxh3": digest(t)} for m, t in sorted(groups.items())]


def gen5_text_summary(path: Path) -> dict:
    """Independent reader for BioTek Gen5 tab-separated text exports whose read results are plain
    8 x 12 (or 16 x 24) grids: a header row of column numbers, then one row per plate row
    (`<tab>A<tab>v1…<tab><wavelength>`), under a `Results` line. The read mode comes from the
    `Read` procedure line (`Absorbance Endpoint`, `Fluorescence …`, `Luminescence …`), the
    wavelength from `Wavelengths:`. Written for held-out exports that allotropy 0.1.x rejects
    ("invalid or missing measurement data"); numeric cells only."""
    text = path.read_bytes().decode("utf-8-sig", errors="replace")
    rows = [line.split("\t") for line in text.splitlines()]
    header, mode, wls = {}, None, set()
    groups: dict = {}
    for r in rows:
        k = r[0].strip() if r else ""
        if k == "Software Version" and len(r) > 1:
            header["software_version"] = r[1].strip()
        if k == "Reader Type:" and len(r) > 1:
            header["model"] = r[1].strip()
        if k == "Reader Serial Number:" and len(r) > 1:
            header["serial_number"] = r[1].strip()
        if k == "Read" and len(r) > 1:
            mode = mode_of(r[1].strip())
        cells = [c.strip() for c in r]
        for c in cells:
            if c.startswith("Wavelengths:"):
                wls.update(float(x) for x in re.findall(r"\d+(?:\.\d+)?", c.split(":", 1)[1]))
    i = 0
    while i < len(rows):
        cells = [c.strip() for c in rows[i]]
        nums = [c for c in cells[1:] if c]
        if cells and cells[0] == "" and nums and all(re.fullmatch(r"\d{1,2}", c) for c in nums) and nums[0] == "1":
            cols = [int(c) for c in cells[2:] if c.isdigit()]
            j = i + 1
            while j < len(rows):
                rc = [c.strip() for c in rows[j]]
                if len(rc) < 2 or not re.fullmatch(r"[A-P]", rc[1]):
                    break
                row = ord(rc[1]) - ord("A")
                for n, col in enumerate(cols):
                    try:
                        x = float(rc[2 + n])
                    except (IndexError, ValueError):
                        continue
                    groups.setdefault(mode or "absorbance", []).append((row, col - 1, x))
                j += 1
            i = j
            continue
        i += 1
    return {"groups": _groups(groups, {mode or "absorbance": wls}), "header": header}


def _text_rows(path: Path) -> list:
    """Lines of a delimited text export split on its delimiter (the one of `;`, tab or `,` that
    occurs on most lines)."""
    raw = path.read_bytes()
    try:
        text = raw.decode("utf-8-sig")
    except UnicodeDecodeError:
        text = raw.decode("cp1252")
    lines = text.splitlines()
    counts = {d: sum(d in line for line in lines[:400]) for d in (";", "\t", ",")}
    delim = max(counts, key=lambda d: counts[d])
    return [line.split(delim) for line in lines]


def bmg_csv_summary(path: Path) -> dict:
    """Independent reader for BMG LABTECH MARS table-view exports (CLARIOstar, Omega), comma or
    semicolon separated: `Test name:`/`Date:` header lines, a mode line (`Fluorescence (FI)
    spectrum`, `Absorbance spectrum`, `Absorbance`, `Luminescence …`), then a table
    `Well;Content;<column title>;…` with an optional `;Wavelength [nm];320;321;…` row, and one
    row per well (`A01;Sample X1;v1;v2;…`). Columns titled `Raw Data (…)` hold measured values
    (`groups`); columns with any other title (`Blank corrected based on …`, `Average over
    replicates based on …`) hold values MARS calculated (`calculated_groups`), as allotropy
    separates calculated data. A spectrum's wavelengths are recorded. Written for exports that
    allotropy 0.1.x rejects."""
    rows = _text_rows(path)
    header, mode, wls = {}, None, []
    groups: dict = {}
    calc: dict = {}
    titles: list = []
    for r in rows:
        first = r[0].strip() if r else ""
        if ":" not in first:
            m = mode_of(first)
            if m and mode is None:
                mode = m
        if first == "Well" and len(r) > 2:
            titles = [c.strip() for c in r[2:]]
            continue
        if first == "" and len(r) > 1 and r[1].strip().startswith("Wavelength"):
            wls = [float(c) for c in r[2:] if c.strip()]
            continue
        w = parse_well(first) if first else None
        if w and len(r) > 2:
            for i, c in enumerate(r[2:]):
                try:
                    v = float(c)
                except ValueError:
                    continue
                title = titles[i] if i < len(titles) else "Raw Data"
                target = groups if title.startswith("Raw Data") else calc
                target.setdefault(mode or "fluorescence", []).append((*w, v))
    out = {"groups": _groups(groups, {mode or "fluorescence": wls}), "header": header}
    if calc:
        out["calculated_groups"] = _groups(calc, {mode or "fluorescence": wls})
    return out


def _envision_mode(label: str) -> str:
    return ("luminescence" if re.search(r"\bLUM\b|Lumin", label, re.I) else
            "absorbance" if re.search(r"\bAbs|\bA\d{3}\b", label, re.I) else "fluorescence")


def envision_csv_summary(path: Path) -> dict:
    """Independent reader for PerkinElmer EnVision CSV exports (comma, semicolon or tab
    separated): `Results for <label> - channel N` blocks, each a `,01,02,…` header row and one
    row per plate row (`A,v1,v2,…`); and untitled matrices, which some export formats write
    without row letters or column numbers: consecutive lines of equal field count whose
    non-empty fields are all numbers, as many lines as the plate type has rows (the protocol's
    `Number of rows`, else the standard plate of that many wells); their read is the `Label` of
    the plate information around them. `Calculated results:` blocks (crosstalk correction,
    ratios) are derived values and are not read, as with allotropy's calculated-data documents.
    The mode is taken from the label (`LUM` luminescence, `Abs`/`A450` absorbance, else
    fluorescence). Written for exports that allotropy 0.1.x rejects."""
    rows = _text_rows(path)
    cells = [[c.strip() for c in r] for r in rows]
    groups: dict = {}
    sections = ("Basic assay information", "Protocol information", "Auto export parameters:",
                "Labels:", "Platemap:", "Calculations:", "Instrument:", "Filters:", "Operations:")
    # the read named by the plate information (the `Label` column)
    plate_labels = []
    for i, r in enumerate(cells):
        if r and r[0] == "Plate information" and i + 2 < len(cells) and "Label" in cells[i + 1]:
            k = cells[i + 1].index("Label")
            if k < len(cells[i + 2]) and cells[i + 2][k]:
                plate_labels.append((i, cells[i + 2][k]))
    i = 0
    in_trailer = False
    while i < len(cells):
        r = cells[i]
        first = r[0] if r else ""
        if first in sections:
            in_trailer = True
        if first in ("Plate information", "Background information") or first.startswith(("Results for", "Calculated results")):
            in_trailer = False
        if first.startswith("Results for "):
            label = first[len("Results for "):]
            mode = _envision_mode(label)
            cols = [int(c) for c in cells[i + 1][1:] if c.isdigit()]
            j = i + 2
            while j < len(cells) and cells[j] and re.fullmatch(r"[A-Z]{1,2}", cells[j][0]):
                row = parse_well(cells[j][0] + "1")[0]
                for n, col in enumerate(cols):
                    try:
                        groups.setdefault(mode, []).append((row, col - 1, float(cells[j][n + 1])))
                    except (IndexError, ValueError):
                        continue
                j += 1
            i = j
            continue
        if first.startswith("Calculated results"):
            # skip the calculated matrix: its title, header and lettered rows
            j = i + 2
            while j < len(cells) and cells[j] and re.fullmatch(r"[A-Z]{1,2}", cells[j][0]):
                j += 1
            i = j
            continue

        def numeric_line(k, width):
            if k >= len(cells) or len(cells[k]) != width:
                return False
            return all(c == "" or re.fullmatch(r"[-+]?\d+(\.\d+)?([eE][-+]?\d+)?", c) for c in cells[k])

        width = len(r)
        if not in_trailer and width >= 3 and numeric_line(i, width):
            j = i
            while numeric_line(j, width):
                j += 1
            n_rows = j - i
            lead = 1 if all(cells[k][0] == "" for k in range(i, j)) else 0
            n_cols = width - lead
            standard = {(8, 12), (16, 24), (32, 48), (2, 3), (3, 4), (4, 6), (6, 8), (48, 72)}
            if (n_rows, n_cols) in standard and any(c for k in range(i, j) for c in cells[k]):
                before = [lab for (pi, lab) in plate_labels if pi < i]
                after = [lab for (pi, lab) in plate_labels if pi > i]
                label = after[0] if after else (before[-1] if before else "")
                mode = _envision_mode(label)
                for k in range(i, j):
                    for c in range(n_cols):
                        v = cells[k][lead + c]
                        if v != "":
                            groups.setdefault(mode, []).append((k - i, c, float(v)))
                i = j
                continue
        i += 1
    return {"groups": _groups(groups), "header": {}}


TEXT_DIALECTS = {"GEN5_TEXT": gen5_text_summary, "BMG_CSV": bmg_csv_summary, "ENVISION_CSV": envision_csv_summary}


def plate(p: Path) -> dict:
    # PLATE_VENDOR names the dialect when the file name does not (held-out files keep their
    # original names): an allotropy Vendor name, or TECAN_ICONTROL for the independent reader.
    vendor = os.environ.get("PLATE_VENDOR") or vendor_for(p.name)
    if vendor in TEXT_DIALECTS:
        summary = TEXT_DIALECTS[vendor](p)
        if not summary.get("groups") and not summary.get("calculated_groups"):
            raise RuntimeError(f"{vendor}: no plate values found")
        return {"reader": f"oracle/plate.py {TEXT_DIALECTS[vendor].__name__} (independent reader of the export text; "
                          "allotropy cannot parse this export)", "plate": summary}
    if vendor == "TECAN_ICONTROL":
        vendor = None
    if vendor is None:
        if p.name.startswith("tecan-sparkcontrol") or (p.name.startswith("tecan-icontrol-csv")):
            summary = tecan_csv_summary(p)
            if not summary.get("groups"):
                raise RuntimeError("tecan_csv_summary found no data tables")
            return {"reader": "oracle/plate.py tecan_csv_summary (independent pandas reader of the CSV text; "
                              "allotropy has no i-control or SparkControl parser)",
                    "plate": summary}
        if p.name.startswith("tecan-icontrol") or os.environ.get("PLATE_VENDOR") == "TECAN_ICONTROL":
            summary = icontrol_summary(p)
            if not summary.get("groups"):
                raise RuntimeError("no independent oracle: allotropy has no Tecan i-control parser, and "
                                   "icontrol_summary found no plate blocks (it reads the tab-delimited text "
                                   "and XLSX exports, not the comma-delimited CSV)")
            return {"reader": "oracle/plate.py icontrol_summary (independent reader; allotropy has no i-control parser)",
                    "plate": summary}
        raise RuntimeError(f"no plate oracle for {p.name}")
    import allotropy
    version = getattr(allotropy, "__version__", None)
    if version is None:
        from importlib.metadata import version as v
        version = v("allotropy")
    asm = allotropy_asm(p, vendor)
    return {"reader": f"allotropy {version} ({vendor})", "plate": summarize_asm(asm)}
