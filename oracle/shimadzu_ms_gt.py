"""Ground truth for the LC-MS data of Shimadzu LabSolutions `.lcd` files: a ground-truth slice
(`scan,ms_level,mz,intensity` for the first spectra) that the test set's author converted with
ProteoWizard msconvert, which reads the file with the vendor's own library. Read with the
Python standard library; written to corpus/oracle/shimadzu/<id>.json as `ms_ground_truth`
{reader, spectra: [{scan, ms_level, n_points, points: [[mz, intensity], ...]}]}
(`points`: the non-zero ones; `n_points` counts all). The comparison is
`crates/openreadout-corpus-tests/tests/shimadzu_oracle/mod.rs`.

With `--export`, the ground truth is a whole open-format export of the run instead (the vendor's
own LabSolutions mzXML, or a ProteoWizard mzML), read with pyteomics: every spectrum with its
retention time (s), polarity and the precursor of each point in the export's order
(`precursors`). An ANDI-MS export (`.cdf`, GCMSsolution) gives every scan without polarity or
precursors; its spectra are matched to ours by time (`match_by_rt`), as it leaves out a GC-MS
run's selected-ion and MRM records. To keep the committed JSON under 1 MiB it holds every spectrum whose points have
more than one precursor and every 50th of the others (`ORACLE_SHIMADZU_EVERY` changes the 50).

Usage: shimadzu_ms_gt.py --id ID FILE.lcd GT.csv.gz
       shimadzu_ms_gt.py --id ID FILE.lcd --export EXPORT.mzXML|EXPORT.mzML
"""
import csv
import gzip
import hashlib
import json
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "shimadzu"


def from_export(path: Path) -> tuple[str, list]:
    """Every spectrum of an mzXML or mzML export of the run."""
    from importlib.metadata import version as pkg_version
    import os
    every = int(os.environ.get("ORACLE_SHIMADZU_EVERY", "50"))
    out = []
    if path.suffix.lower() == ".cdf":
        # an ANDI-MS export (GCMSsolution): every scan; no polarity, no precursor
        from scipy.io import netcdf_file
        with netcdf_file(str(path), "r", mmap=False) as f:
            v = f.variables
            t = v["scan_acquisition_time"][:].copy()
            first = v["scan_index"][:].copy()
            n = v["point_count"][:].copy()
            mz_all = v["mass_values"][:].copy()
            it_all = v["intensity_values"][:].copy()
        for k in range(len(t)):
            mz = mz_all[first[k]: first[k] + n[k]]
            it = it_all[first[k]: first[k] + n[k]]
            # every N-th scan, and every scan with a value of 2^22 or more (the ones stored in four
            # bytes are among them)
            if k % every and (len(it) == 0 or it.max() < (1 << 22)):
                continue
            out.append({
                "scan": k, "ms_level": 1, "n_points": int(n[k]), "rt_s": float(t[k]), "polarity": None,
                "precursors": [],
                "points": [[float(m), float(x)] for m, x in zip(mz, it) if x != 0],
            })
        from importlib.metadata import version as pkg_version
        return f"SciPy {pkg_version('scipy')} netcdf_file on {path.name}", out
    if path.suffix.lower() == ".mzxml":
        from pyteomics import mzxml
        reader, name = mzxml.MzXML(str(path)), "MzXML"
    else:
        from pyteomics import mzml
        reader, name = mzml.MzML(str(path)), "MzML"
    with reader as f:
        for k, sp in enumerate(f):
            mz, it = sp["m/z array"], sp["intensity array"]
            if name == "MzXML":
                level = int(sp["msLevel"])
                rt = float(sp["retentionTime"]) * 60.0
                pol = {"+": "positive", "-": "negative"}.get(sp.get("polarity"))
                pre = [float(p["precursorMz"]) for p in sp.get("precursorMz", [])]
            else:
                level = int(sp["ms level"])
                rt = float(sp["scanList"]["scan"][0]["scan start time"]) * 60.0
                pol = "positive" if "positive scan" in sp else ("negative" if "negative scan" in sp else None)
                pre = [float(si["selected ion m/z"]) for p in sp.get("precursorList", {}).get("precursor", [])
                       for si in p.get("selectedIonList", {}).get("selectedIon", [])]
            if len(set(pre)) < 2 and k % every:
                continue
            out.append({
                "scan": k, "ms_level": level, "n_points": len(mz), "rt_s": rt, "polarity": pol,
                "precursors": pre,
                "points": [[float(m), float(v)] for m, v in zip(mz, it) if v != 0],
            })
    return f"pyteomics {pkg_version('pyteomics')} {name} on {path.name}", out


def main() -> None:
    args = sys.argv[1:]
    if len(args) == 5 and args[0] == "--id" and args[3] == "--export":
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import oracle_json
        fid, lcd, export = args[1], Path(args[2]), Path(args[4])
        reader, spectra = from_export(export)
        gt = {"source": export.name, "spectra": spectra}
        if export.suffix.lower() == ".cdf":
            # the ANDI export holds a run's scans only: its spectra are found by their time
            gt["match_by_rt"] = True
        data = {
            "id": fid,
            "file": lcd.name,
            "size": lcd.stat().st_size,
            "sha256": hashlib.sha256(lcd.read_bytes()).hexdigest(),
            "reader": reader,
            "ms_ground_truth": gt,
        }
        OUT.mkdir(parents=True, exist_ok=True)
        written = oracle_json.write_text(OUT / f"{fid}.json", json.dumps(data) + "\n")
        print("wrote", written.name, len(spectra), "spectra")
        return
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
