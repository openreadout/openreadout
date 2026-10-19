#!/usr/bin/env python
"""Ground truth for National Instruments TDMS files from npTDMS (LGPL-3.0, run as a black box:
its source is not read): per channel the group and channel names, the value count, the unit and
sample interval properties, and the values as npTDMS returns them by default (scaled with the
file's NI_Scale properties): the first 16, the last, 64 sampled values with their index, and the
sum, minimum and maximum of the finite values. Timestamps are seconds since 1970-01-01 UTC.
String channels: the value count and the first 8 strings.

Usage:  python tdms_oracle.py [--out DIR] --id ID PATH

Writes `<DIR>/<ID>.json` (default corpus/oracle/tdms); compared by
`crates/openreadout-corpus-tests/tests/corpus/tdms_oracle/mod.rs`.
"""
from __future__ import annotations

import json
import math
import sys
from importlib.metadata import version
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "tdms"


def _num(x):
    x = float(x)
    if math.isnan(x):
        return "nan"
    if math.isinf(x):
        return "inf" if x > 0 else "-inf"
    return x


def channel(c) -> dict:
    import numpy as np

    props = c.properties
    out = {
        "group": c.group_name,
        "channel": c.name,
        "dtype": c.data_type.__name__ if c.data_type else None,
        "count": len(c),
        "unit": props.get("unit_string"),
        "wf_increment": float(props["wf_increment"]) if "wf_increment" in props else None,
    }
    data = c[:]
    if data.dtype.kind in "OUS":
        out["string"] = True
        out["first_strings"] = [str(v) for v in data[:8]]
        return out
    if data.dtype.kind == "M":
        data = data.astype("datetime64[ns]").astype(np.int64) / 1e9
    data = np.asarray(data, dtype=np.float64)
    n = len(data)
    idx = sorted({round(j * (n - 1) / 63) for j in range(64)}) if n > 1 else list(range(n))
    finite = data[np.isfinite(data)]
    out.update({
        "first": [_num(v) for v in data[:16]],
        "last": _num(data[-1]) if n else None,
        "sampled": [[i, _num(data[i])] for i in idx],
        "sum": float(finite.sum()) if finite.size else None,
        "sum_abs": float(np.abs(finite).sum()) if finite.size else None,
        "min": float(finite.min()) if finite.size else None,
        "max": float(finite.max()) if finite.size else None,
    })
    return out


def tdms(path: Path) -> dict:
    from nptdms import TdmsFile

    f = TdmsFile.read(str(path))
    chans = [channel(c) for g in f.groups() for c in g.channels()]
    return {
        "reader": f"npTDMS {version('npTDMS')} (LGPL-3.0, black box)",
        "independent": True,
        "channels": [c for c in chans if c["count"] > 0],
    }


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
    try:
        body = tdms(files[0])
    except Exception as e:  # recorded so the comparison reports it instead of skipping
        body = {"error": f"{type(e).__name__}: {e}"}
    out = {"id": ident, "format": "ni-tdms", **body}
    out_dir.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(out, indent=1) + "\n")
    print(ident, "error" if "error" in out else f"{len(out.get('channels', []))} channels")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
