"""Cross-check openreadout's NWB traces and tables with pynwb (BSD-3), run as a black box.

Usage:
    uv run python nwb_depth_validate.py FILE.nwb [...] [--openreadout BIN]

For every file: pynwb reads it (which validates the layout against the NWB schema as it builds
the objects); then every ElectricalSeries/TimeSeries/SpatialSeries under acquisition/ and
processing/ is matched to an openreadout trace by name (sample count, channel count, rate,
electrode rows, first samples of the first and last channel scaled as data * conversion *
channel_conversion + offset), the electrodes table and every other DynamicTable to a table by
name (row count, numeric columns compared value by value, text columns decoded through
`extra.categories`), a units table's spike counts per unit and its spike_times, and every
SpikeEventSeries (event count, timestamps, first values). Prints one JSON line per file;
exit 1 when anything differs.
"""

import argparse
import json
import subprocess
import sys
import warnings

import numpy as np

warnings.filterwarnings("ignore")
from pynwb import NWBHDF5IO  # noqa: E402
from pynwb.base import TimeSeries  # noqa: E402
from pynwb.ecephys import ElectricalSeries, SpikeEventSeries  # noqa: E402
from hdmf.common import DynamicTable  # noqa: E402


def run(bin_, *args):
    out = subprocess.run([bin_, *args, "--json"], capture_output=True, text=True, check=False)
    env = json.loads(out.stdout)
    if not env.get("ok"):
        raise SystemExit(f"openreadout {' '.join(args)} failed: {env.get('error')}")
    return env["data"]


def table_rows(bin_, path, index, rows):
    """All rows of table `index` via `openreadout export --to csv` (category codes come back as
    their text), as a list of columns (numbers where the text parses as one)."""
    import csv
    import os
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "t.csv")
        run(bin_, "export", path, "--to", "csv", "--table", str(index), "--output", out)
        with open(out, newline="") as f:
            r = list(csv.reader(f))
    body = r[1:]
    cols = [list(c) for c in zip(*body)] if body else [[] for _ in r[0]]

    def conv(v):
        try:
            return float(v)
        except ValueError:
            return v
    return [[conv(v) for v in c] for c in cols]


def same(a, b):
    a = np.asarray(a, dtype=np.float64)
    b = np.asarray(b, dtype=np.float64)
    return a.shape == b.shape and bool(np.all((a == b) | (np.isnan(a) & np.isnan(b)) | (np.abs(a - b) <= 1e-12 * np.maximum(np.abs(a), np.abs(b)))))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--openreadout", default="openreadout")
    a = ap.parse_args()
    failed = False
    for path in a.files:
        problems, checked = [], 0
        info = run(a.openreadout, "info", path)
        info = info.get("file", info)
        traces = {t["name"]: t for t in info.get("traces", [])}
        tables_by_name = {t["name"]: t for t in info.get("tables", [])}

        class _Tables(dict):
            """Our table names are full HDF5 paths; pynwb names only the object and its
            containers, so a pynwb path matches ours at its end."""
            def get(self, key, default=None):
                for n, t in tables_by_name.items():
                    if n == key or n.endswith("/" + key):
                        return t
                return default
        tables = _Tables()
        with NWBHDF5IO(path, "r", load_namespaces=True) as io:
            nwb = io.read()
            series = []
            for obj in nwb.objects.values():
                path_parts = []
                o = obj
                while o is not None and o is not nwb:
                    path_parts.append(o.name)
                    o = o.parent
                where = "/".join(reversed(path_parts))
                if isinstance(obj, SpikeEventSeries):
                    series.append(("events", obj, where))
                elif isinstance(obj, TimeSeries) and type(obj).__name__ in ("TimeSeries", "ElectricalSeries", "SpatialSeries"):
                    top = where.split("/")[0]
                    in_data = obj.parent is not None and (obj.parent in nwb.acquisition.values() or obj in nwb.acquisition.values() or any(obj.parent is m or obj.parent.parent is m for m in nwb.processing.values()))
                    if in_data:
                        series.append(("trace", obj, where))
                elif isinstance(obj, DynamicTable):
                    series.append(("table", obj, where))
            for kind, obj, where in series:
                if kind == "trace":
                    t = traces.get(obj.name)
                    if t is None:
                        problems.append(f"{where}: no trace")
                        continue
                    data = np.asarray(obj.data[:], dtype=np.float64)
                    cols = data.reshape(data.shape[0], -1)
                    conv = float(obj.conversion)
                    off = float(getattr(obj, "offset", 0.0) or 0.0)
                    cc = getattr(obj, "channel_conversion", None)
                    chans = [c for c in t["channels"] if c["name"] != "time"]
                    if t["sample_count"] != cols.shape[0] or len(chans) != cols.shape[1]:
                        problems.append(f"{where}: shape {t['sample_count']}x{len(chans)} != pynwb {cols.shape}")
                        continue
                    if obj.rate is not None and abs(t["sample_rate_hz"] - obj.rate) > 1e-9 * obj.rate:
                        problems.append(f"{where}: rate {t['sample_rate_hz']} != {obj.rate}")
                    if isinstance(obj, ElectricalSeries):
                        rows = list(obj.electrodes.data[:])
                        ours = [c.get("extra", {}).get("electrode_row") for c in chans]
                        if ours != rows:
                            problems.append(f"{where}: electrode rows differ")
                    tr = run(a.openreadout, "trace", path, "--trace", str(t["index"]), "--max-samples", "16")
                    for ci in (0, cols.shape[1] - 1):
                        want = cols[:16, ci] * conv
                        want = want * float(cc[ci]) + off if cc is not None else want + off
                        got = next(c for c in tr["channels"] if c["name"] == chans[ci]["name"])["samples"]
                        if not same(got, want):
                            problems.append(f"{where}: channel {ci} first samples differ")
                    checked += 1
                elif kind == "events":
                    t = tables.get(where)
                    if t is None:
                        problems.append(f"{where}: no table")
                        continue
                    n = len(obj.timestamps[:])
                    cols = table_rows(a.openreadout, path, t["index"], t["row_count"])
                    data = np.asarray(obj.data[:], dtype=np.float64).reshape(n, -1) * float(obj.conversion) + float(getattr(obj, "offset", 0.0) or 0.0)
                    if t["row_count"] != n or not same(cols[0], obj.timestamps[:]) or not same(np.asarray(cols[1:]).T, data):
                        problems.append(f"{where}: events differ")
                    checked += 1
                else:
                    name = where
                    t = tables.get(name)
                    if t is None:
                        problems.append(f"{where}: no table")
                        continue
                    if t["row_count"] != len(obj.id):
                        problems.append(f"{where}: rows {t['row_count']} != {len(obj.id)}")
                        continue
                    cols = table_rows(a.openreadout, path, t["index"], t["row_count"])
                    names = [c["name"] for c in t["columns"]]
                    for ci, c in enumerate(t["columns"]):
                        n = c["name"]
                        if n == "id":
                            ref = list(obj.id.data[:])
                        elif n.endswith("_count") and n[:-6] in obj.colnames:
                            ref = [len(x) for x in obj[n[:-6]][:]]
                        elif "categories" in c.get("extra", {}):
                            got = [str(v) if not isinstance(v, float) else (str(int(v)) if v.is_integer() else repr(v)) for v in cols[ci]]
                            vals = [x.decode() if isinstance(x, bytes) else str(x) for x in obj[n].data[:]]
                            if [g.strip() for g in got] != [v.strip() for v in vals]:
                                problems.append(f"{where}.{n}: text values differ")
                            continue
                        elif "[" in n:
                            base, k = n[:-1].split("[")
                            ref = np.asarray(obj[base].data[:], dtype=np.float64)[:, int(k)]
                        else:
                            ref = np.asarray(obj[n].data[:]).astype(np.float64)
                        if not same(cols[ci], ref):
                            problems.append(f"{where}.{n}: values differ")
                    if "spike_times" in obj.colnames:
                        st = tables.get(f"{name}/spike_times")
                        ref = np.concatenate([np.asarray(x, dtype=np.float64) for x in obj["spike_times"][:]]) if len(obj.id) else np.zeros(0)
                        scols = table_rows(a.openreadout, path, st["index"], st["row_count"])
                        if st["row_count"] != len(ref) or not same(scols[2], ref):
                            problems.append(f"{where}: spike times differ")
                    checked += 1
        print(json.dumps({"file": path.rsplit("/", 1)[-1], "objects_checked": checked, "ok": not problems, "problems": problems[:10]}))
        failed |= bool(problems)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
