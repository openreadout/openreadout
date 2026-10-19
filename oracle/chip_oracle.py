#!/usr/bin/env python
"""Ground truth for Agilent Bioanalyzer XML exports (`agilent-bioanalyzer-xml`) and TapeStation
exports (`agilent-tapestation`): an independent pass over the files with Python's standard library
(xml.etree, base64, csv), and the sizes the instrument's software stored for every peak.

Usage:  python chip_oracle.py [--out DIR] --id ID FILE.xml[.gz] [CSV[.gz]]

Recorded per well: the name, the number of signal values, their sum and values at up to 64 evenly
spaced positions with each position's own coordinate (migration time in s, or run distance in %);
RIN, RINe and DIN; the TapeStation concentration; and every peak that is not a marker: its
position and the instrument's size. A TapeStation XML without its electropherogram CSV, and a
Bioanalyzer XML without signals, give no signal: such a well is compared with the reader's samples
and peaks tables instead of its trace.

Writes `<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/corpus/chip_oracle/mod.rs`
compares.
"""
from __future__ import annotations

import base64
import csv
import gzip
import io
import json
import math
import struct
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "chip"


def read(p: Path) -> bytes:
    b = p.read_bytes()
    return gzip.decompress(b) if b[:2] == b"\x1f\x8b" else b


def num(s):
    try:
        return float(str(s).replace(",", ""))
    except (TypeError, ValueError):
        return None


def sampled(x, y):
    n = len(y)
    idx = sorted({round(j * (n - 1) / 63) for j in range(64)}) if n > 1 else list(range(n))
    return {"count": n, "sum": float(sum(v for v in y if math.isfinite(v))), "at": [[x[i], y[i]] for i in idx]}


def bioanalyzer(root):
    wells = []
    for s in root.find("Chips/Chip/Files/File/Samples"):
        if s.findtext("HasData") != "true":
            continue
        w = {"well": s.findtext("WellNumber"), "name": (s.findtext("Name") or "").strip()}
        rin = num(s.findtext("DAResultStructures/DARRIN/Channel/RIN"))
        if rin and rin > 0:
            w["rin"] = rin
        for ch in s.findall("DASignals/DetectorChannels/Channel/SignalData"):
            if ch.findtext("ChannelID") != "RedFluorescence":
                continue
            n = int(ch.findtext("NumberOfSamples"))
            raw = base64.b64decode("".join(ch.findtext("ProcessedSignal").split()))
            y = list(struct.unpack("<%df" % n, raw[: 4 * n]))
            x0, dx = float(ch.findtext("XStart")), float(ch.findtext("XStep"))
            w["signal"] = sampled([x0 + dx * i for i in range(n)], y)
            break
        peaks = []
        for c in s.findall("DAResultStructures/DARIntegrator/Channel"):
            ps = c.findall("PeaksMolecular/PeakMolecular")
            if ps:
                for p in ps:
                    obs = (p.findtext("Observations") or "").strip()
                    if "Marker" in obs:
                        continue
                    peaks.append([num(p.findtext("MigrationTime")), num(p.findtext("FragmentSize"))])
                break
        w["peaks"] = peaks
        wells.append(w)
    return {"position": "time", "wells": wells}


def tapestation(root, csv_bytes):
    """Without the electropherogram CSV (`csv_bytes` None) only the results are recorded."""
    head, data = [], []
    if csv_bytes is not None:
        text = csv_bytes.decode("latin-1").lstrip("﻿")
        rows = list(csv.reader(io.StringIO(text)))
        head = [h.split(":")[0].strip() for h in rows[0]]
        data = [r for r in rows[1:] if r]
    n = len(data)
    wells = []
    for s in root.find("Samples"):
        well = s.findtext("WellNumber")
        w = {"well": well, "name": (s.findtext("Comment") or "").strip()}
        conc = num(s.findtext("Concentration"))
        if conc is not None:
            w["concentration"] = conc
        for k, tag in (("rine", "RNA/RINe"), ("din", "DIN")):
            v = num(s.findtext(tag))
            if v is not None:
                w[k] = v
        if well in head:
            k = head.index(well)
            y = [num(r[k]) if num(r[k]) is not None else math.nan for r in data]
            w["signal"] = sampled([(n - i) / n * 100 for i in range(n)], y)
        w["peaks"] = [
            [num(p.findtext("RunDistance")), num(p.findtext("Size"))]
            for p in s.findall("Peaks/Peak")
            if "Marker" not in (p.findtext("Observations") or "") and num(p.findtext("Size")) is not None
        ]
        wells.append(w)
    return {"position": "run_distance", "wells": wells}


def main(argv):
    out_dir, ident, files = OUT, None, []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        else:
            files.append(Path(a))
    if not ident or not files:
        print(__doc__, file=sys.stderr)
        return 2
    root = ET.fromstring(read(files[0]))
    if root.tag == "Chipset":
        body, fmt = bioanalyzer(root), "agilent-bioanalyzer-xml"
    else:
        csv_bytes = read(files[1]) if len(files) > 1 else None
        body, fmt = tapestation(root, csv_bytes), "agilent-tapestation"
    out = {"id": ident, "format": fmt, "reader": "Python xml.etree, base64, csv (standard library)", **body}
    out_dir.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(ident, len(body["wells"]), "wells", sum(len(w["peaks"]) for w in body["wells"]), "peaks")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
