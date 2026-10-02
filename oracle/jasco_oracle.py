#!/usr/bin/env python
"""Ground truth for JASCO Spectra Manager `.jws` files (`jasco-jws`).

Usage:  python jasco_oracle.py [--out DIR] --id ID FILE [--export EXPORT] [--other-acquisition]
        [--jws2txt DIR]

- `--export`: Spectra Manager's text/CSV export of the same file (JASCO's `TITLE … XYDATA` layout,
  tab- or comma-separated, decimal commas in some locales, then an optional footer of settings):
  the header (DATA TYPE, XUNITS, YUNITS, FIRSTX, LASTX, NPOINTS, DELTAX), the footer's key/value
  pairs, the row count and x/y at up to 256 evenly spaced rows. Standard library only.
- `--other-acquisition`: the export is of another measurement with the same name (its first value
  and measurement time differ from the file's): only its instrument facts are compared.
- `--jws2txt DIR`: a second opinion for compound files from jws2txt (MIT; `DIR` holds its
  `helpers.py`) run as a black box: channel names, point count and values at 64 points per channel.

Writes `<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/jasco_oracle/mod.rs` compares.
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB in corpus/oracle/heldout)

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "jasco"
HEADER_KEYS = ("DATA TYPE", "XUNITS", "YUNITS", "FIRSTX", "LASTX", "NPOINTS", "DELTAX", "DATE", "TIME")


def _num(s: str, comma: bool):
    s = s.strip()
    if comma:
        s = s.replace(",", ".")
    try:
        v = float(s)
    except ValueError:
        return None
    return v if math.isfinite(v) else None


def read_export(p: Path) -> dict:
    raw = p.read_bytes()
    for enc in ("utf-8", "shift_jis", "cp1250", "latin-1"):
        try:
            text = raw.decode(enc)
            break
        except UnicodeDecodeError:
            continue
    lines = text.splitlines()
    delim = "\t" if any("\t" in l for l in lines[:5]) else ","
    header, footer, rows = {}, {}, []
    i = 0
    while i < len(lines) and lines[i].strip() != "XYDATA":
        k, _, v = lines[i].partition(delim)
        header[k.strip()] = v.strip()
        i += 1
    # decimal commas: tab-separated with a comma in the numbers (Polish locale)
    comma = delim == "\t" and "," in header.get("DELTAX", "") + header.get("FIRSTX", "")
    i += 1
    npoints = int(_num(header.get("NPOINTS", "0"), comma) or 0)
    while i < len(lines):
        l = lines[i]
        if not l.strip():
            break
        a, _, b = l.partition(delim)
        x, y = _num(a, comma), _num(b, comma)
        if x is None or y is None:
            break
        rows.append((x, y))
        i += 1
        if npoints and len(rows) == npoints:
            break
    for l in lines[i:]:
        k, sep, v = l.partition("\t" if "\t" in l else delim)
        if sep and k.strip() and not k.startswith("["):
            footer.setdefault(k.strip(), v.strip())
    n = len(rows)
    idx = sorted({round(j * (n - 1) / 255) for j in range(256)}) if n else []
    hdr = {k: header.get(k) for k in HEADER_KEYS}
    for k in ("FIRSTX", "LASTX", "DELTAX", "NPOINTS"):
        if hdr.get(k) is not None:
            hdr[k] = _num(hdr[k], comma)
    return {"file": p.name, "decimal_comma": comma, "header": hdr, "footer": footer, "n": n,
            "samples": [[j, rows[j][0], rows[j][1]] for j in idx]}


def second_opinion(path: Path, lib: Path) -> dict:
    sys.path.insert(0, str(lib))
    from helpers import JWSFile  # jws2txt (MIT), run as a black box

    f = JWSFile(str(path))
    data = f.unpacked_data  # [x, channel 1, channel 2, …]
    chans = []
    for k, ch in enumerate(data[1:]):
        n = len(ch)
        idx = sorted({round(j * (n - 1) / 63) for j in range(64)}) if n else []
        chans.append({"name": f.header_names[k + 1] if k + 1 < len(f.header_names) else None, "n": n,
                      "samples": [[j, ch[j]] for j in idx]})
    xs = list(data[0])
    return {"reader": "jws2txt 1.0 (MIT)", "channels": chans, "x_count": len(xs), "x_first": xs[0] if xs else None,
            "sample_name": getattr(f, "sample_name", None), "comment": getattr(f, "comment", None)}


def main(argv: list[str]) -> int:
    out_dir, ident, export, other, lib, files = OUT, None, None, False, None, []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        elif a == "--export":
            export = Path(next(it))
        elif a == "--other-acquisition":
            other = True
        elif a == "--jws2txt":
            lib = Path(next(it))
        else:
            files.append(Path(a))
    if len(files) != 1 or not ident:
        print(__doc__, file=sys.stderr)
        return 2
    path = files[0]
    out = {"id": ident, "format": "jasco-jws"}
    if export:
        out["export"] = read_export(export)
        out["export"]["other_acquisition"] = other
    if lib and path.read_bytes()[:4] == bytes.fromhex("D0CF11E0"):
        try:
            out["jws2txt"] = second_opinion(path, lib)
        except Exception as e:  # noqa: BLE001 - recorded; the test then skips it
            out["jws2txt_error"] = f"{type(e).__name__}: {e}"
    out_dir.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(out, indent=1, ensure_ascii=False, allow_nan=False) + "\n")
    print(ident, "export" if export else "", "jws2txt" if "jws2txt" in out else out.get("jws2txt_error", ""))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
