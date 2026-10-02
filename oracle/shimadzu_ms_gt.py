"""Ground truth for the LC-MS data of Shimadzu LabSolutions `.lcd` files: a ground-truth slice
(`scan,ms_level,mz,intensity` for the first spectra) that the test set's author converted with
ProteoWizard msconvert, which reads the file with the vendor's own library. Read with the
Python standard library; written to corpus/oracle/shimadzu/<id>.json as `ms_ground_truth`
{reader, spectra: [{scan, ms_level, n_points, points: [[mz, intensity], ...]}]}
(`points`: the non-zero ones; `n_points` counts all). The comparison is
`crates/openreadout-corpus-tests/tests/shimadzu_oracle/mod.rs`.

Usage: shimadzu_ms_gt.py --id ID FILE.lcd GT.csv.gz
"""
import csv
import gzip
import hashlib
import json
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "shimadzu"


def main() -> None:
    args = sys.argv[1:]
    if len(args) != 4 or args[0] != "--id":
        raise SystemExit(__doc__)
    fid, lcd, gt = args[1], Path(args[2]), Path(args[3])
    spectra: dict[int, dict] = {}
    with gzip.open(gt, "rt") as fh:
        for r in csv.DictReader(fh):
            s = spectra.setdefault(int(r["scan"]), {"scan": int(r["scan"]), "ms_level": int(r["ms_level"]), "n_points": 0, "points": []})
            s["n_points"] += 1
            if float(r["intensity"]) != 0:  # zero-intensity points are counted, not listed
                s["points"].append([float(r["mz"]), float(r["intensity"])])
    data = {
        "id": fid,
        "file": lcd.name,
        "size": lcd.stat().st_size,
        "sha256": hashlib.sha256(lcd.read_bytes()).hexdigest(),
        "reader": "ProteoWizard msconvert (vendor library), ground-truth slice by the test set's author; read with the Python standard library",
        "ms_ground_truth": {"source": gt.name, "spectra": [spectra[k] for k in sorted(spectra)]},
    }
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{fid}.json").write_text(json.dumps(data, indent=1) + "\n")
    print("wrote", f"{fid}.json", len(spectra), "spectra")


if __name__ == "__main__":
    main()
