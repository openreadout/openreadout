#!/usr/bin/env python
"""Write the Bruker 3D and 4D processed-data fixtures used by crates/openreadout-nmr/tests.

Usage:  cd oracle && uv run python make_nmr_fixtures.py [--out DIR]

No public Bruker experiment with 4D processed files (4rrrr ...) was found, and the 3D fixture
predates the public 3D sets now in the corpus, so nmrglue (BSD-3) is run as a black box to
*write* them: `nmrglue.bruker.write_pdata` stores a known array in XDIM submatrices,
little-endian int32, NC_proc 0. The Rust tests read the files back and expect exactly the array,
so the submatrix order is checked against nmrglue's writer rather than against our own reading
of its `reorder_submatrix`.

- `bruker_3d_pdata`: m[k][j][i] = 10000*k + 100*j + i (k: proc3s, j: proc2s, i: direct), in
  `3rrr`; `3rri` holds -m.
- `bruker_4d_pdata`: m[l][k][j][i] = 1000000*l + 10000*k + 100*j + i (l: proc4s), in `4rrrr`;
  `4iiii` holds -m.
"""
from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parent / "crates" / "openreadout-nmr" / "tests" / "fixtures"

# (SI, XDIM, components): SI and XDIM slowest dimension first, the direct dimension last
FIXTURES_SPEC = {
    "bruker_3d_pdata": ((4, 4, 8), (2, 2, 4), ("3rrr", "3rri")),
    "bruker_4d_pdata": ((2, 4, 2, 8), (1, 2, 2, 4), ("4rrrr", "4iiii")),
}
AXES = [(12.0, 7200.0, 600.13, "<1H>"), (130.0, 2400.0, 60.8, "<15N>"), (70.0, 3000.0, 150.9, "<13C>"),
        (40.0, 1500.0, 242.9, "<31P>")]
ACQ_NUC = ["<15N>", "<13C>", "<31P>"]


def jcamp(pairs: dict, title: str) -> str:
    lines = [f"##TITLE= {title}", "##JCAMPDX= 5.0", "##DATATYPE= Parameter Values"]
    lines += [f"##${k}= {v}" for k, v in pairs.items()]
    lines.append("##END=")
    return "\n".join(lines) + "\n"


def write(out: Path, si: tuple, xdim: tuple, comps: tuple) -> None:
    import nmrglue as ng

    ndim = len(si)
    exp = out / "1"
    pdata = exp / "pdata" / "1"
    pdata.mkdir(parents=True, exist_ok=True)
    title = "Parameter file, TopSpin 4.1.0"
    (exp / "acqus").write_text(jcamp({"TD": 16, "AQ_mod": 3, "SW_h": 1000, "NUC1": "<1H>", "SFO1": 600.13,
                                      "BYTORDA": 0, "DTYPA": 0, "PARMODE": ndim - 1}, title))
    for k in range(2, ndim + 1):
        (exp / f"acqu{k}s").write_text(jcamp({"TD": 2, "NUC1": ACQ_NUC[k - 2], "FnMODE": 6 if k == 2 else 5}, title))
    (exp / "ser").write_bytes(bytes(1024 * 4 * 2 ** (ndim - 1)))
    # procs is the direct dimension (last axis of the array), proc2s the next, ...
    names = ["procs"] + [f"proc{k}s" for k in range(2, ndim + 1)]
    dic = {}
    for d, name in enumerate(names):
        axis = ndim - 1 - d
        off, sw, sf, nuc = AXES[d]
        pars = {"SI": si[axis], "XDIM": xdim[axis], "NC_proc": 0, "BYTORDP": 0, "DTYPP": 0,
                "OFFSET": off, "SW_p": sw, "SF": sf, "AXNUC": nuc, "MC2": 5 if d == 1 else 4 if d > 1 else 0}
        dic[name] = pars
        (pdata / name).write_text(jcamp(pars, title))
    grids = np.meshgrid(*(np.arange(n) for n in si), indexing="ij")
    weights = [100 ** (ndim - 1 - a) for a in range(ndim)]
    m = sum(w * g for w, g in zip(weights, grids)).astype(np.float64)
    for fname, data in ((comps[0], m), (comps[1], -m)):
        path = pdata / fname
        if path.exists():
            path.unlink()
        ng.bruker.write_pdata(str(pdata), dic, data, scale_data=True, shape=si, submatrix_shape=xdim,
                              bin_file=fname, overwrite=True, big=False, isfloat=False)
    # read back with nmrglue to be sure the writer and reader agree
    _, back = ng.bruker.read_pdata(str(pdata), bin_files=list(comps), read_acqus=False, scale_data=True,
                                   all_components=True, big=False, isfloat=False)
    assert np.array_equal(back[0], m) and np.array_equal(back[1], -m), "nmrglue read-back differs"
    print("wrote", out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=FIXTURES, help="directory that receives the fixture folders")
    out = ap.parse_args().out
    for name, (si, xdim, comps) in FIXTURES_SPEC.items():
        write(out / name, si, xdim, comps)


if __name__ == "__main__":
    main()
