#!/usr/bin/env python
"""The committed Bruker flex fixture (`crates/openreadout-bruker-tims/tests/fixtures/flex-synthetic/`):
a run of three spots written from scratch with values known by construction.

- `0_A1/1/1SLin/`: linear, little-endian, TD 8, DW 2, DELAY 1000, ML1 1e6, ML2 = ML3 = 0, so
  m/z(i) = ((1000 + 2 i) / 1000)^2; intensities 0, 10, ..., 70; negative ions.
- `0_B2/1/1SRef/`: reflector, big-endian (BYTORDA 1), TD 4, ML3 = 1e-3 (the quadratic branch);
  intensities -2, -1, 0, 2^20.
- `0_C3/1/1500.5000.LIFT/1SRef/`: LIFT, TD 4, `Parent` 1500.5, positive ions.

Usage:  python make_flex_fixtures.py
"""
from __future__ import annotations

import struct
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "crates" / "openreadout-bruker-tims" / "tests" / "fixtures" / "flex-synthetic"


def acqus(**kv) -> str:
    lines = ["##TITLE= XMASS Parameter file", "##JCAMP-DX= 5.0", "##DATATYPE= Parameter Values",
             "##ORIGIN= XMASS, Bruker-Daltonics", "##OWNER= nobody"]
    for k, v in kv.items():
        k = k.replace("_SP_", " ")
        lines.append(f"##.{k}= {v}" if k == "IONIZATION MODE" else f"##${k}= {v}")
    lines.append("##END=")
    return "\n".join(lines) + "\n"


def spot(rel: str, values: list[int], big: bool, **kv) -> None:
    d = OUT / rel
    d.mkdir(parents=True, exist_ok=True)
    (d / "acqus").write_text(acqus(TD=len(values), BYTORDA=1 if big else 0, **kv))
    (d / "fid").write_bytes(struct.pack((">" if big else "<") + "i" * len(values), *values))


def main() -> None:
    base = dict(DW=2, DELAY=1000, ML1=1000000, ML2=0, NoSHOTS=100, INSTRUM="<FIXTURE-PC>", InstTyp=9,
                FCVer="<flexControl 3.4.0.0>", AQ_DATE="<2024-01-02T03:04:05.000-05:00>")
    spot("0_A1/1/1SLin", [10 * i for i in range(8)], False, ML3=0, AQ_mod=1, SPType=0,
         IONIZATION_SP_MODE="LD-", SPOTNO="<A1>", **base)
    spot("0_B2/1/1SRef", [-2, -1, 0, 1 << 20], True, ML3=0.001, AQ_mod=2, SPType=0,
         IONIZATION_SP_MODE="LD+", SPOTNO="<B2>", **base)
    spot("0_C3/1/1500.5000.LIFT/1SRef", [1, 2, 3, 4], False, ML3=0, AQ_mod=6, SPType=2, Parent=1500.5,
         IONIZATION_SP_MODE="LD+", SPOTNO="<C3>", **base)


if __name__ == "__main__":
    main()
