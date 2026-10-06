"""Ground truth for plate-reader binary documents whose depositor export is a spreadsheet that
allotropy does not read (Gen5 Excel exports) or a depositor's own sheet of exported values.

An independent reading with openpyxl (not our Rust code) of every value cell of the export,
written to corpus/oracle/plate-binary/<id>.json as
  {"export": file, "reader": ..., "kind": "gen5-xlsx" | "kinetic-sheet", "header": {...},
   "values": [[well, read label, time_s or null, value], ...]}
`tests/plate_binary.rs` checks that the binary decode holds every one of these values (within
the export's rounding) under the same well, read and time.

Usage: plate_exports.py ID KIND EXPORT [SHEET | DOCUMENT]
  KIND gen5-xlsx: Gen5's Excel export (header lines, `Results`, matrices with row letters in
                  column B, column numbers from C, the read label after the last column).
  KIND kinetic-sheet: a sheet whose header row starts with `Time` followed by well names, one
                  row per read time (h:mm:ss); the read label is given as the 4th argument
                  (`sheet:label`).
  KIND softmax-xlsx: SoftMax Pro's text export in the first sheet of a workbook (PlateFormat
                  endpoint blocks, one column group per wavelength); the 4th argument names the
                  .sda/.pda document it was exported from.
"""
import datetime as dt
import json
import re
import sys
from pathlib import Path

import openpyxl

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "plate-binary"
WELL = re.compile(r"^([A-Z]{1,2})(\d{1,3})$")


def cell_text(v):
    if isinstance(v, float) and v.is_integer():
        return str(int(v))
    return "" if v is None else str(v).strip()


def gen5_xlsx(path: Path) -> dict:
    ws = openpyxl.load_workbook(path, data_only=True).worksheets[0]
    rows = [list(r) for r in ws.iter_rows(values_only=True)]
    header = {}
    for r in rows:
        if r and isinstance(r[0], str) and r[0].strip() and len(r) > 1 and r[1] is not None:
            key = r[0].strip().rstrip(":")
            if key in ("Software Version", "Reader Type", "Reader Serial Number", "Plate Number"):
                header[key] = cell_text(r[1])
            if key == "Date" and isinstance(r[1], dt.datetime):
                header["Date"] = r[1].date().isoformat()
            if key == "Time" and isinstance(r[1], dt.time):
                header["Time"] = r[1].isoformat()
            if key == "Actual Temperature":
                header.setdefault("Actual Temperature", r[1])
    values = []
    i = 0
    while i < len(rows):
        r = rows[i]
        # a matrix header: column numbers 1, 2, ... starting in some column
        c0 = next((c for c, v in enumerate(r) if v == 1), None)
        if c0 is None or c0 == 0 or any(v not in (None, "") for v in r[:c0]):
            i += 1
            continue
        n = 0
        while c0 + n < len(r) and r[c0 + n] == n + 1:
            n += 1
        if n < 3:
            i += 1
            continue
        letter = None
        j = i + 1
        while j < len(rows) and any(v is not None for v in rows[j]):
            row = rows[j]
            if row[c0 - 1] not in (None, ""):
                letter = str(row[c0 - 1]).strip()
            label = cell_text(row[c0 + n]) if c0 + n < len(row) else ""
            for c in range(n):
                v = row[c0 + c]
                if isinstance(v, (int, float)) and letter:
                    values.append([f"{letter}{c + 1}", label, None, float(v)])
            j += 1
        i = j
    return {"kind": "gen5-xlsx", "header": header, "values": values}


def kinetic_sheet(path: Path, sheet: str, label: str) -> dict:
    ws = openpyxl.load_workbook(path, data_only=True)[sheet]
    rows = [list(r) for r in ws.iter_rows(values_only=True)]
    h = next(i for i, r in enumerate(rows) if r and r[0] == "Time")
    wells = [(c, v) for c, v in enumerate(rows[h]) if isinstance(v, str) and WELL.match(v)]
    values = []
    for r in rows[h + 1:]:
        t = r[0]
        if not isinstance(t, dt.time):
            continue
        secs = t.hour * 3600 + t.minute * 60 + t.second
        for c, w in wells:
            v = r[c]
            if isinstance(v, (int, float)):
                values.append([w, label, float(secs), float(v)])
    return {"kind": "kinetic-sheet", "header": {}, "values": values}


def softmax_xlsx(path: Path) -> dict:
    """SoftMax Pro's text export held in the first sheet of a workbook: each `Plate:` line (name in
    the second cell, PlateFormat endpoint data, the wavelength list in the 16th cell), then a
    header row of column numbers (one group per wavelength, groups separated by an empty cell)
    and one row per plate row with the temperature in the second cell. Values are
    [plate/well, wavelength, null, value]."""
    ws = openpyxl.load_workbook(path, read_only=True, data_only=True).worksheets[0]
    rows = [list(r) for r in ws.iter_rows(values_only=True)]
    values = []
    plates = []
    i = 0
    while i < len(rows):
        r = rows[i]
        if not r or r[0] != "Plate:":
            i += 1
            continue
        name, fmt, read_type = cell_text(r[1]), cell_text(r[3]), cell_text(r[4])
        if fmt != "PlateFormat" or read_type != "Endpoint":
            sys.exit(f"{path.name}: {name} is {fmt} {read_type}, not a PlateFormat endpoint block")
        wls = cell_text(r[15]).split()
        plates.append({"plate": name, "wavelengths": wls})
        head = rows[i + 1]
        # column-number groups: runs of 1, 2, 3, ... after the temperature column
        groups, cur = [], []
        for c, v in enumerate(head):
            if isinstance(v, (int, float)) and c >= 2:
                cur.append((c, int(v)))
            elif cur:
                groups.append(cur)
                cur = []
        if cur:
            groups.append(cur)
        if len(groups) != len(wls):
            sys.exit(f"{path.name}: {name}: {len(groups)} column groups for {len(wls)} wavelengths")
        j = i + 2
        row = 0
        while j < len(rows) and rows[j] and any(v not in (None, "") for v in rows[j]) and rows[j][0] != "~End":
            for wl, group in zip(wls, groups):
                for c, col in group:
                    v = rows[j][c] if c < len(rows[j]) else None
                    if isinstance(v, (int, float)):
                        values.append([f"{name}/{chr(65 + row)}{col}", wl, None, float(v)])
            row += 1
            j += 1
        i = j
    return {"kind": "softmax-xlsx", "header": {"plates": plates}, "values": values}


def main():
    fid, kind, export = sys.argv[1], sys.argv[2], Path(sys.argv[3])
    if kind == "gen5-xlsx":
        data = gen5_xlsx(export)
    elif kind == "softmax-xlsx":
        data = softmax_xlsx(export)
        data["document"] = sys.argv[4]
    elif kind == "kinetic-sheet":
        sheet, label = sys.argv[4].split(":", 1)
        data = kinetic_sheet(export, sheet, label)
    else:
        sys.exit(f"unknown kind {kind}")
    data = {"id": fid, "export": export.name,
            "reader": f"oracle/plate_exports.py {kind} (openpyxl {openpyxl.__version__})", **data}
    OUT.mkdir(parents=True, exist_ok=True)
    values = data.pop("values")
    body = ",\n".join(json.dumps(v, separators=(",", ":")) for v in values)
    head = json.dumps(data, indent=1)
    (OUT / f"{fid}.json").write_text(head[:-2] + ",\n \"values\": [\n" + body + "\n ]\n}\n")
    print("wrote", fid, len(values), "values")


if __name__ == "__main__":
    main()
