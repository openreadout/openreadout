#!/usr/bin/env python
"""Write the Bruker 3D processed-data fixture used by crates/openreadout-nmr/tests/varian_jeol.rs.

Usage:  cd oracle && uv run python make_nmr_fixtures.py [--out DIR]

No public Bruker experiment with 3D processed files (3rrr ...) was found, so nmrglue (BSD-3) is
run as a black box to *write* one: `nmrglue.bruker.write_pdata` stores a known array
m[k][j][i] = 10000*k + 100*j + i (k: F1 of proc3s, j: F2 of proc2s, i: direct dimension) in
XDIM submatrices, little-endian int32, NC_proc 0. The Rust test reads the files back and expects
exactly m, so the submatrix order is checked against nmrglue's writer rather than against our own
reading of its `reorder_submatrix`. The imaginary component `3rri` holds -m.
"""
from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
DEFAULT_OUT = HERE.parent / "crates" / "openreadout-nmr" / "tests" / "fixtures" / "bruker_3d_pdata"

SI = (4, 4, 8)      # (F1 from proc3s, F2 from proc2s, direct from procs)
XDIM = (2, 2, 4)


def jcamp(pairs: dict, title: str) -> str:
    lines = [f"##TITLE= {title}", "##JCAMPDX= 5.0", "##DATATYPE= Parameter Values"]
    lines += [f"##${k}= {v}" for k, v in pairs.items()]
    lines.append("##END=")
    return "\n".join(lines) + "\n"


def main() -> None:
    import nmrglue as ng

    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    out = ap.parse_args().out
    exp = out / "1"
    pdata = exp / "pdata" / "1"
    pdata.mkdir(parents=True, exist_ok=True)
    title = "Parameter file, TopSpin 4.1.0"
    (exp / "acqus").write_text(jcamp({"TD": 16, "AQ_mod": 3, "SW_h": 1000, "NUC1": "<1H>", "SFO1": 600.13,
                                      "BYTORDA": 0, "DTYPA": 0}, title))
    (exp / "acqu2s").write_text(jcamp({"TD": 2, "NUC1": "<15N>"}, title))
    (exp / "acqu3s").write_text(jcamp({"TD": 2, "NUC1": "<13C>"}, title))
    (exp / "ser").write_bytes(bytes(1024 * 4))
    dims = {"procs": 2, "proc2s": 1, "proc3s": 0}
    offsets = {"procs": (12.0, 7200.0, 600.13), "proc2s": (130.0, 2400.0, 60.8), "proc3s": (70.0, 3000.0, 150.9)}
    dic = {}
    for name, axis in dims.items():
        off, sw, sf = offsets[name]
        pars = {"SI": SI[axis], "XDIM": XDIM[axis], "NC_proc": 0, "BYTORDP": 0, "DTYPP": 0,
                "OFFSET": off, "SW_p": sw, "SF": sf, "AXNUC": "<1H>"}
        dic[name] = pars
        (pdata / name).write_text(jcamp(pars, title))
    k, j, i = np.meshgrid(*(np.arange(n) for n in SI), indexing="ij")
    m = (10000 * k + 100 * j + i).astype(np.float64)
    for fname, data in (("3rrr", m), ("3rri", -m)):
        path = pdata / fname
        if path.exists():
            path.unlink()
        ng.bruker.write_pdata(str(pdata), dic, data, scale_data=True, shape=SI, submatrix_shape=XDIM,
                              bin_file=fname, overwrite=True, big=False, isfloat=False)
    # read back with nmrglue to be sure the writer and reader agree
    _, back = ng.bruker.read_pdata(str(pdata), read_acqus=False, scale_data=True, all_components=True,
                                   big=False, isfloat=False)
    assert np.array_equal(back[0], m) and np.array_equal(back[1], -m), "nmrglue read-back differs"
    print("wrote", out)


if __name__ == "__main__":
    main()
