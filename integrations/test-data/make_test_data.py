#!/usr/bin/env python3
"""Write the small FCS test files used by the workflow integrations (Nextflow, Galaxy, Snakemake).

Standard library only; the files follow the FCS 3.0 specification (HEADER, TEXT with `/`
delimiters, float32 little-endian list-mode DATA), and their values are known, so the tests can
check what `openreadout` reads back:

    python3 integrations/test-data/make_test_data.py        # rewrites tube_a.fcs, tube_b.fcs

tube_a.fcs: 3 parameters (FSC-A, SSC-A, FITC-A with $P3S CD4) x 10 events, FITC-A = 100..109.
tube_b.fcs: same parameters, FITC-A = 200..209.
"""

import struct
from pathlib import Path

HERE = Path(__file__).resolve().parent


def write_fcs(path: Path, names, labels, rows) -> None:
    npar = len(names)
    data = b"".join(struct.pack("<" + "f" * npar, *r) for r in rows)
    kw = {
        "$BEGINANALYSIS": "0",
        "$ENDANALYSIS": "0",
        "$BEGINSTEXT": "0",
        "$ENDSTEXT": "0",
        "$BEGINDATA": "@" * 10,
        "$ENDDATA": "#" * 10,
        "$BYTEORD": "1,2,3,4",
        "$DATATYPE": "F",
        "$MODE": "L",
        "$NEXTDATA": "0",
        "$PAR": str(npar),
        "$TOT": str(len(rows)),
        "$CYT": "integration test writer",
    }
    for j, name in enumerate(names, start=1):
        kw[f"$P{j}B"] = "32"
        kw[f"$P{j}E"] = "0,0"
        kw[f"$P{j}N"] = name
        kw[f"$P{j}R"] = "262144"
        if labels[j - 1]:
            kw[f"$P{j}S"] = labels[j - 1]
    text = "/" + "".join(f"{k}/{v}/" for k, v in kw.items())
    text_start = 58
    text_end = text_start + len(text.encode()) - 1
    data_start = text_end + 1
    data_end = data_start + len(data) - 1
    text = text.replace("@" * 10, f"{data_start:010d}").replace("#" * 10, f"{data_end:010d}")
    header = "FCS3.0    " + "".join(
        f"{v:>8}" for v in (text_start, text_end, data_start, data_end, 0, 0)
    )
    path.write_bytes(header.encode() + text.encode() + data)


def main() -> None:
    names = ["FSC-A", "SSC-A", "FITC-A"]
    labels = ["", "", "CD4"]
    for name, base in (("tube_a.fcs", 100.0), ("tube_b.fcs", 200.0)):
        rows = [(1000.0 + 10 * i, 500.0 + 5 * i, base + i) for i in range(10)]
        write_fcs(HERE / name, names, labels, rows)


if __name__ == "__main__":
    main()
