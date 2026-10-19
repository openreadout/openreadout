#!/usr/bin/env python
"""Ground truth for NanoDrop and Qubit exports (`nanodrop`, `qubit`).

Usage:  uv run --group plate python benchtop_oracle.py [--out DIR] --id ID FILE

- Report exports (NanoDrop 8000, Eight, One; Qubit 4 and Flex): allotropy (MIT, Benchling) converts
  the file to Allotrope Simple Model JSON with its own parser. Recorded per measurement document,
  in file order: the sample and location identifiers, the measurement time, and every numeric leaf
  under the document by its key (`mass concentration`, `absorbance`, `detector wavelength setting`,
  `fluorescence`, ...), with unit; per calculated-data document, its name, value and the
  measurement it was computed from.
- Spectrum exports (the NanoDrop One UV-Vis export, which allotropy does not read) and the spectra
  columns of NanoDrop Eight exports: pandas (BSD-3) reads the numbers. Recorded per spectrum: its
  first and last wavelength, point count, and the absorbance at up to 32 evenly spaced points.

Writes `<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/corpus/benchtop_oracle/mod.rs`
compares.
"""
from __future__ import annotations

import io
import json
import math
import sys
from importlib.metadata import version
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "benchtop"


def leaves(o, out, key=None):
    """Numeric leaves `{value, unit}` under `o` by their key (the innermost name)."""
    if isinstance(o, dict):
        if "value" in o and isinstance(o.get("value"), (int, float)) and key:
            if not math.isfinite(o["value"]):
                return
            out.append([key, float(o["value"]), o.get("unit")])
            return
        for k, v in o.items():
            if k in ("calculated data aggregate document",):
                continue
            leaves(v, out, k)
    elif isinstance(o, list):
        for x in o:
            leaves(x, out, key)


def from_asm(asm: dict) -> dict:
    agg_key = next(k for k in asm if k.endswith("aggregate document"))
    agg = asm[agg_key]
    docs_key = next(k for k in agg if k.endswith(" document") and isinstance(agg[k], list))
    measurements = []
    for d in agg[docs_key]:
        mad = d.get("measurement aggregate document", {})
        for m in mad.get("measurement document", []):
            s = m.get("sample document", {})
            nums = []
            leaves(m, nums)
            measurements.append({
                "id": m.get("measurement identifier"),
                "sample": s.get("sample identifier"),
                "location": s.get("location identifier"),
                "time": m.get("measurement time") or mad.get("measurement time"),
                "values": nums,
            })
    calculated = []
    for c in agg.get("calculated data aggregate document", {}).get("calculated data document", []):
        src = [x.get("data source identifier") for x in
               c.get("data source aggregate document", {}).get("data source document", [])]
        r = c.get("calculated result", {})
        if isinstance(r.get("value"), (int, float)) and math.isfinite(r["value"]):
            calculated.append({"name": c.get("calculated data name"), "value": float(r["value"]), "sources": src})
    device = agg.get("device system document", {})
    return {"measurements": measurements, "calculated": calculated,
            "model": device.get("model number"), "serial": device.get("equipment serial number")}


def spectra_one(path: Path) -> list:
    """The NanoDrop One UV-Vis export: blocks of name, date, titles, `//` lines, pairs."""
    import pandas as pd

    text = path.read_text(encoding="utf-8", errors="replace")
    blocks = [b for b in text.split("\n\n") if b.strip()]
    out = []
    for b in blocks:
        lines = b.strip("\n").split("\n")
        data = "\n".join(l for l in lines[3:] if not l.startswith("//"))
        df = pd.read_csv(io.StringIO(data), sep="\t", header=None, usecols=[0, 1])
        out.append(sample_points(lines[0].strip(), df[0].to_numpy(), df[1].to_numpy()))
    return out


def spectra_eight(path: Path) -> list:
    import pandas as pd

    lines = path.read_text(encoding="utf-8").splitlines()
    start = next(i for i, l in enumerate(lines) if l.count("\t") >= 4 and not l.split("\t")[0].endswith(":"))
    df = pd.read_csv(io.StringIO("\n".join(lines[start:])), sep="\t", dtype=str)
    wl = []
    for c in df.columns:
        try:
            v = float(str(c).replace(",", "."))
        except ValueError:
            continue
        if 150 <= v <= 1100:
            wl.append((c, v))
    if len(wl) < 2:
        return []
    out = []
    name_col = "Sample Name" if "Sample Name" in df.columns else df.columns[1]
    for _, row in df.iterrows():
        x = [v for _, v in wl]
        y = [float(str(row[c]).replace(",", ".")) for c, _ in wl]
        name = "" if pd.isna(row[name_col]) else str(row[name_col])
        out.append(sample_points(name, x, y))
    return out


def sample_points(name, x, y) -> dict:
    n = len(x)
    idx = sorted({round(j * (n - 1) / 31) for j in range(32)}) if n else []
    return {"name": name, "first": float(x[0]), "last": float(x[-1]), "points": n,
            "samples": [[i, float(x[i]), float(y[i])] for i in idx]}


def main(argv: list[str]) -> int:
    out_dir, ident, files = OUT, None, []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        else:
            files.append(Path(a))
    if len(files) != 1 or not ident:
        print(__doc__, file=sys.stderr)
        return 2
    path = files[0]
    raw = path.read_bytes()[:4096]
    # NanoDrop One exports come as UTF-8 (with or without a BOM) or UTF-16 LE.
    enc = "utf-16" if raw[:2] in (b"\xff\xfe", b"\xfe\xff") else "utf-8-sig"
    head = raw.decode(enc, errors="replace")
    first = head.splitlines()[0] if head else ""
    out = {"id": ident, "readers": {}}
    lines = head.splitlines()
    if len(lines) >= 3 and lines[2].startswith("Wavelength (nm)"):
        out["format"] = "nanodrop"
        out["spectra"] = spectra_one(path)
        out["readers"]["spectra"] = f"pandas {version('pandas')} (BSD-3)"
    else:
        from allotropy.constants import CHARDET_ENCODING
        from allotropy.parser_factory import Vendor
        from allotropy.to_allotrope import allotrope_from_file

        if first.startswith("Run ID") or first.startswith("sep="):
            out["format"] = "qubit"
            vendor = Vendor.THERMO_FISHER_QUBIT_FLEX if "Sample Name" in head.splitlines()[1 if first.startswith("sep=") else 0] else Vendor.THERMO_FISHER_QUBIT4
        else:
            out["format"] = "nanodrop"
            if first.startswith("Application:"):
                vendor = Vendor.THERMO_FISHER_NANODROP_EIGHT
            elif first.startswith(("Date,", "Date\t")):
                vendor = Vendor.THERMO_FISHER_NANODROP_ONE
            else:
                vendor = Vendor.THERMO_FISHER_NANODROP_8000
        src = path
        if vendor == Vendor.THERMO_FISHER_NANODROP_ONE and (enc == "utf-16" or first.startswith("Date\t")):
            # allotropy takes the One export only as comma-separated UTF-8 named .csv; the software
            # also writes it tab-separated, in UTF-16. The csv module (standard library) rewrites
            # the same fields comma-separated for allotropy; no value is touched.
            import csv
            import tempfile

            text = path.read_bytes().decode(enc)
            rows = list(csv.reader(text.splitlines(), delimiter="\t"))
            src = Path(tempfile.mkdtemp()) / (path.stem + ".csv")
            with src.open("w", encoding="utf-8", newline="") as fh:
                csv.writer(fh).writerows(rows)
            out["readers"]["transcode"] = "tab-separated fields rewritten comma-separated (Python csv) for allotropy"
        asm = allotrope_from_file(str(src), vendor, encoding=CHARDET_ENCODING)
        out.update(from_asm(asm))
        out["readers"]["asm"] = f"allotropy {version('allotropy')} (MIT), {vendor.name}"
        if vendor.name == "THERMO_FISHER_NANODROP_EIGHT":
            s = spectra_eight(path)
            if s:
                out["spectra"] = s
                out["readers"]["spectra"] = f"pandas {version('pandas')} (BSD-3)"
    out_dir.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(ident, len(out.get("measurements", [])), "measurements", len(out.get("spectra", [])), "spectra")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
