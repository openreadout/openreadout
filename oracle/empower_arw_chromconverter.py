"""Ground truth for Waters Empower ASCII exports (`.arw`) from chromConverter (Ethan Bass,
GPL-3.0, CRAN), run as a black box (docs/legal/clean-room-policy.md rule 3):
`read_chroms(path, format_in = "waters_arw")`.

    CHROMCONVERTER_LIB=... python empower_arw_chromconverter.py --id ID EXPORT.arw
                                                   -> ../corpus/oracle/<id>.json

chromConverter 0.9.0 reads every line after the first as a (time, value) row. On exports whose
header holds one `"name"<TAB>value` field per line (the layout of the GPCreader and HPLC-RS
test files) the header lines come back as rows whose time is not a number; those rows are left
out here, so the oracle holds the rows chromConverter returned as two numbers. The header is read
here only to name the trace (`SampleName` and `Channel`, when present).

The comparison is the generic trace comparison of the corpus test (every value's xxh3, first
and last retention time), as for `empower_arw_oracle.py`.
"""
import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

import gen  # the trace-record helper
import oracle_json

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle"

R_CODE = r"""
lib <- Sys.getenv("CHROMCONVERTER_LIB")
if (nzchar(lib)) .libPaths(c(lib, .libPaths()))
suppressMessages(library(chromConverter))
args <- commandArgs(trailingOnly = TRUE)
x <- read_chroms(args[1], format_in = "waters_arw", progress_bar = FALSE,
                 data_format = "long", format_out = "data.frame")[[1]]
d <- as.data.frame(x)
write.table(data.frame(rt = as.character(d[[1]]), intensity = as.character(d[[2]])), args[2],
            sep = "\t", row.names = FALSE, quote = FALSE)
cat(as.character(packageVersion("chromConverter")), file = args[3])
"""


def _num(s: str):
    try:
        return float(s)
    except ValueError:
        return None


def header_fields(p: Path) -> dict:
    """The export's quoted header fields, in either layout (names row + values row, or one
    `"name"<TAB>value` pair per line). Used only to name the trace."""
    text = p.read_bytes().decode("utf-8", errors="replace").replace("\r\n", "\n").replace("\r", "\n")
    lines = [l for l in text.split("\n") if l.strip()]
    cells = [[c.strip().strip('"') for c in l.split("\t")] for l in lines[:120]]
    head = [c for c in cells if not all(_num(x) is not None for x in c)]
    if len(head) >= 2 and len(head[0]) > 2 and len(head[0]) == len(head[1]):
        return dict(zip(head[0], head[1]))
    return {c[0]: c[1] for c in head if len(c) >= 2}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", required=True)
    ap.add_argument("export")
    a = ap.parse_args()
    p = Path(a.export)
    with tempfile.TemporaryDirectory(prefix="openreadout-arw-") as d:
        rows_path, ver_path = Path(d) / "rows.tsv", Path(d) / "version.txt"
        r = subprocess.run(["Rscript", "-e", R_CODE, str(p), str(rows_path), str(ver_path)],
                           capture_output=True, text=True, errors="replace", env=os.environ)
        if r.returncode:
            raise SystemExit(f"chromConverter failed: {r.stderr[-2000:]}")
        version = ver_path.read_text().strip()
        rows = [l.split("\t") for l in rows_path.read_text().splitlines()[1:]]
    pts = [(_num(t), _num(v)) for t, v in rows]
    kept = [(t, v) for t, v in pts if t is not None and v is not None]
    dropped = len(pts) - len(kept)
    meta = header_fields(p)
    channel = meta.get("Channel", "value")
    x = [t for t, _ in kept]
    y = [v for _, v in kept]
    trace = gen._chrom_trace(0, [channel], [""], x, [y])
    # the documented trace name `<SampleName> / <Channel>`; left out (not compared) otherwise
    if meta.get("SampleName") and meta.get("Channel"):
        trace["name"] = f"{meta['SampleName']} / {meta['Channel']}"
    out = {
        "id": a.id,
        "file": p.name,
        "size": p.stat().st_size,
        "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
        "reader": f"chromConverter {version} read_chroms(format_in = 'waters_arw') (GPL-3.0, black box)",
        "oracle_note": (f"rows chromConverter returned as two numbers ({len(kept)}); {dropped} header "
                        "rows it returned as data rows were left out" if dropped else
                        f"every row chromConverter returned ({len(kept)})"),
        "traces": [trace],
    }
    oracle_json.write_text(OUT / f"{a.id}.json", json.dumps(out, indent=1) + "\n")
    print(a.id, len(y), "points,", dropped, "non-numeric rows left out")


if __name__ == "__main__":
    main()
