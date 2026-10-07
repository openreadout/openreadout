"""Header-less Gen5 exports for the development corpus (finding N-M2 of held-out draw B, 2026-09-24).

Gen5's export options can leave the file header out (`Software Version` ... `Reading Type`, and
with it the procedure): the text then starts at the `Layout` section. No public, licensed export of
that shape with an embedded layout and standards was found (docs/provenance/plate-readers.md,
2026-09-24), so the development files are made from two real Gen5 3.12 exports already in the
corpus (Benchling-Open-Source/allotropy test data, MIT) by dropping every line before `Layout`.
Nothing else changes: the layout, results, Gen5-computed blank-corrected values, concentrations
and curve-fit tables are the vendor's, so the ground truth of the originals applies unchanged
(`oracle/assay.py` cases `gen5-linear-headerless`, `gen5-meanv-4pl-headerless`).

    python make_gen5_headerless.py [--corpus DIR]     (default: $OPENREADOUT_CORPUS_DIR or ../corpus/files)
"""
import os
import sys
from pathlib import Path

PAIRS = {
    "gen5-abs-stdcurve-linear.txt": "synthetic-gen5-headerless-stdcurve-linear.txt",
    "gen5-abs-kinetic-meanv-4pl.txt": "synthetic-gen5-headerless-kinetic-meanv-4pl.txt",
}


def strip_header(raw: bytes) -> bytes:
    """Every line from the first `Layout` line on, byte for byte (line endings kept)."""
    lines = raw.splitlines(keepends=True)
    for i, line in enumerate(lines):
        if line.strip() == b"Layout":
            return b"".join(lines[i:])
    raise ValueError("no Layout section")


def main() -> None:
    args = sys.argv[1:]
    corpus = Path(args[args.index("--corpus") + 1]) if "--corpus" in args else Path(
        os.environ.get("OPENREADOUT_CORPUS_DIR", Path(__file__).resolve().parent.parent / "corpus" / "files"))
    for src, dst in PAIRS.items():
        out = strip_header((corpus / src).read_bytes())
        (corpus / dst).write_bytes(out)
        print("wrote", dst, len(out), "bytes")


if __name__ == "__main__":
    main()
