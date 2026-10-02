#!/usr/bin/env python
"""Instrument-method facts of a Thermo `.raw` file, read independently of openreadout: the
embedded method is a Compound File (found by its signature) opened with olefile (BSD-2); each
device's `Text` stream is the human-readable method the acquisition software printed (UTF-16).

Usage:  python thermo_method_oracle.py --id ID FILE.raw [...]
Writes `corpus/oracle/thermo-method/<ID>.json`: the device texts' `key: value [unit]` lines that
describe the LC detectors (UV channel wavelengths, data collection rates, acquired channels).
These are values the vendor software wrote, not values computed by openreadout.
"""
from __future__ import annotations

import io
import json
import re
import sys
from pathlib import Path

import olefile

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "thermo-method"
CFB = bytes.fromhex("D0CF11E0A1B11AE1")


def method_texts(raw: bytes) -> dict[str, str]:
    at = raw.find(CFB, 0, 64 << 20)
    if at < 0:
        return {}
    ole = olefile.OleFileIO(io.BytesIO(raw[at : at + (32 << 20)]))
    out = {}
    for entry in ole.listdir():
        if entry[-1] == "Text":
            out["/".join(entry[:-1])] = ole.openstream(entry).read().decode("utf-16-le", "replace")
    return out


def main() -> None:
    args = sys.argv[1:]
    OUT.mkdir(parents=True, exist_ok=True)
    while args:
        assert args.pop(0) == "--id"
        fid, path = args.pop(0), Path(args.pop(0))
        texts = method_texts(path.read_bytes())
        facts: dict = {"uv_channel_wavelength_nm": {}, "acquired": []}
        for text in texts.values():
            for line in text.splitlines():
                line = line.strip()
                if m := re.match(r"UV\.(UV_VIS_\d+)\.Wavelength:\s*([\d.]+)\s*\[nm\]", line):
                    facts["uv_channel_wavelength_nm"][m.group(1)] = float(m.group(2))
                elif m := re.match(r"(UV|CAD)\.Data_Collection_Rate:\s*([\d.]+)\s*\[Hz\]", line):
                    facts[f"{m.group(1).lower()}_data_collection_rate_hz"] = float(m.group(2))
                elif m := re.match(r"UV\.3DFIELD\.(MinWavelength|MaxWavelength|BunchWidth):\s*([\d.]+)", line):
                    facts[f"pda_{m.group(1).lower()}_nm"] = float(m.group(2))
                elif m := re.match(r"(?:\w+\.)*(\w+)\.AcqOn\s*$", line):
                    if m.group(1) not in facts["acquired"]:
                        facts["acquired"].append(m.group(1))
        data = {"id": fid, "file": path.name, "reader": f"olefile {olefile.__version__} on the embedded instrument-method compound file", "facts": facts}
        (OUT / f"{fid}.json").write_text(json.dumps(data, indent=1) + "\n")
        print("wrote", fid, facts)


if __name__ == "__main__":
    main()
