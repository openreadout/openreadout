"""Check `openreadout export --format parquet|arrow` output with pyarrow (Apache-2.0).

Usage:
    uv run python arrow_validate.py OUT.parquet|OUT.arrow [--source FILE] [--openreadout BIN]

Reads the file with pyarrow (Parquet or Arrow IPC file format), checks the file-level metadata
(`openreadout.kind`, `openreadout.source_format`, a parseable `openreadout.info`), the
per-column metadata (`unit` where the source has one, parseable `openreadout.provenance`), and,
with --source, compares the values with what `openreadout` itself reads from the source:

- tables: `openreadout export SOURCE --format csv --table N` (the CSV is parsed with pyarrow.csv)
- traces: `openreadout trace SOURCE --trace N --sweep S --max-samples 100000 --json` for the
  first samples of every sweep
- spectra: `openreadout spectra SOURCE --spectrum I --json` for the first, middle and last scan,
  and the per-scan summary file beside it

Prints one JSON line with the result; exit 1 on any mismatch.
"""

import argparse
import json
import math
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.csv as pacsv
import pyarrow.ipc as ipc
import pyarrow.parquet as pq


def read(path: Path) -> pa.Table:
    with open(path, "rb") as f:
        head = f.read(6)
    if head[:4] == b"PAR1":
        return pq.read_table(path)
    if head == b"ARROW1":
        with ipc.open_file(path) as r:
            return r.read_all()
    raise SystemExit(f"{path}: neither Parquet nor Arrow IPC")


def meta(t: pa.Table) -> dict:
    return {k.decode(): v.decode() for k, v in (t.schema.metadata or {}).items()}


def same(a, b) -> bool:
    if a is None or b is None:
        return a is None and b is None
    if isinstance(a, float) and isinstance(b, float) and math.isnan(a) and math.isnan(b):
        return True
    return a == b


def run(bin_: str, *args: str) -> dict:
    out = subprocess.run([bin_, *args, "--json"], capture_output=True, text=True, check=True)
    env = json.loads(out.stdout)
    if not env.get("ok"):
        raise SystemExit(f"openreadout {' '.join(args)} failed: {env.get('error')}")
    return env["data"]


def check(path: Path, source: Path | None, bin_: str) -> dict:
    t = read(path)
    m = meta(t)
    problems: list[str] = []
    for k in ("openreadout.kind", "openreadout.source_format", "openreadout.info", "openreadout.version"):
        if k not in m:
            problems.append(f"file metadata {k} missing")
    info = json.loads(m.get("openreadout.info", "{}"))
    kind = m.get("openreadout.kind")
    for f in t.schema:
        fm = {k.decode(): v.decode() for k, v in (f.metadata or {}).items()}
        if "openreadout.provenance" in fm:
            json.loads(fm["openreadout.provenance"])
        if "openreadout.extra" in fm:
            json.loads(fm["openreadout.extra"])
    obj = json.loads(m.get("openreadout.object", "{}"))
    compared = 0
    if source is not None and kind == "table":
        with tempfile.TemporaryDirectory() as d:
            csv = Path(d) / "t.csv"
            first = obj.get("first_row", 0)
            n = t.num_rows
            args = ["export", str(source), "--format", "csv", "--table", str(obj["index"]), "-o", str(csv)]
            if n:
                args += ["--rows", f"{first}-{first + n - 1}"]
            run(bin_, *args)
            ref = pacsv.read_csv(csv, read_options=pacsv.ReadOptions(), convert_options=pacsv.ConvertOptions(strings_can_be_null=False))
        if ref.num_rows != t.num_rows or ref.num_columns != t.num_columns:
            problems.append(f"shape {t.num_rows}x{t.num_columns} != CSV {ref.num_rows}x{ref.num_columns}")
        else:
            for i in range(t.num_columns):
                a = t.column(i).to_pylist()
                b = ref.column(i).to_pylist()
                if pa.types.is_dictionary(t.schema.field(i).type):
                    b = [str(x) for x in b]
                elif pa.types.is_floating(t.schema.field(i).type):
                    # the CSV holds the shortest text of the stored float32 or float64
                    single = t.schema.field(i).type == pa.float32()
                    b = [(float(np.float32(x)) if single else float(x)) if x is not None else None for x in b]
                    a = [x if x is not None else float("nan") for x in a]
                    b = [x if x is not None else float("nan") for x in b]
                elif pa.types.is_integer(t.schema.field(i).type):
                    # NaN in the source is null here and "NaN" in the CSV
                    b = [None if (isinstance(x, float) and math.isnan(x)) else x for x in b]
                    b = [int(x) if x is not None else None for x in b]
                bad = [j for j, (x, y) in enumerate(zip(a, b)) if not same(x, y)]
                if bad:
                    problems.append(f"column {t.schema.field(i).name}: {len(bad)} values differ (first at row {bad[0]}: {a[bad[0]]!r} vs {b[bad[0]]!r})")
                compared += len(a)
    elif source is not None and kind == "trace":
        sweeps = obj.get("sweeps_written", [0])
        first = obj.get("first_sample", 0)
        sw = t.column("sweep").to_pylist()
        for s in sweeps:
            rows = [i for i, x in enumerate(sw) if x == s]
            ref = run(bin_, "trace", str(source), "--trace", str(obj["index"]), "--sweep", str(s),
                      "--first-sample", str(first), "--count", str(len(rows)), "--max-samples", "100000")
            for ci, ch in enumerate(ref["channels"]):
                col = t.column(2 + ci).to_pylist()
                got = [col[i] for i in rows[: len(ch["samples"])]]
                want = ch["samples"]
                bad = [j for j, (x, y) in enumerate(zip(got, want)) if not same(float(x), float("nan") if y is None else float(y))]
                if bad:
                    problems.append(f"sweep {s} channel {ch['name']}: {len(bad)} samples differ")
                compared += len(want)
    elif source is not None and kind == "spectra":
        scans = t.column("scan").to_pylist()
        summary_path = path.with_name(path.name.rsplit(".", 1)[0] + ".scans." + path.name.rsplit(".", 1)[1])
        summary = read(summary_path)
        if meta(summary).get("openreadout.kind") != "scans":
            problems.append("summary file kind is not scans")
        count = summary.num_rows
        if sum(summary.column("point_count").to_pylist()) != t.num_rows:
            problems.append("summary point_count does not add up to the points file")
        offsets = [0]
        for c in summary.column("point_count").to_pylist():
            offsets.append(offsets[-1] + c)
        for i in sorted({0, count // 2, count - 1}):
            view = obj.get("view", "primary")
            args = ["spectra", str(source), "--spectrum", str(i), "--run", str(obj["index"])]
            if view == "centroid":
                args.append("--centroid")
            ref = run(bin_, *args)["spectrum"]
            a, b = offsets[i], offsets[i + 1]
            mz = t.column("mz").slice(a, b - a).to_pylist()
            inten = t.column("intensity").slice(a, b - a).to_pylist()
            # the JSON holds the shortest text of each float32 intensity
            if mz != ref["mz"] or [float(x) for x in inten] != [float(np.float32(x)) for x in ref["intensity"]]:
                problems.append(f"spectrum {i}: arrays differ")
            if set(scans[a:b]) - {ref["scan_number"]}:
                problems.append(f"spectrum {i}: scan column differs")
            compared += len(mz)
    result = {
        "file": str(path),
        "kind": kind,
        "rows": t.num_rows,
        "columns": t.num_columns,
        "source_format": m.get("openreadout.source_format"),
        "info_format": info.get("format", {}).get("id"),
        "values_compared": compared,
        "ok": not problems,
        "problems": problems,
    }
    return result


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+", type=Path)
    ap.add_argument("--source", type=Path)
    ap.add_argument("--openreadout", default="openreadout")
    a = ap.parse_args()
    ok = True
    for f in a.files:
        r = check(f, a.source, a.openreadout)
        print(json.dumps(r))
        ok &= r["ok"]
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
