"""Ground truth for Shimadzu LabSolutions `.lcd` files from the vendor's own ASCII export of the
same run (LabSolutions "ASCII conversion"), read with the Python standard library.

    python shimadzu_export.py --id ID FILE.lcd EXPORT.txt     -> ../corpus/oracle/shimadzu/<id>.json

Every `[LC Chromatogram(<detector>)]` section: `# of Points`, `Interval(msec)`, `Start Time(min)`,
`End Time(min)`, `Intensity Units`, `Intensity Multiplier`, the first values and every value of
the `Intensity` column (the column times the multiplier is the chromatogram in its units), and
every `[Peak Table(...)]` (R.Time, Area, Height, Name per peak). `oracle/gen_heldout.py` uses the
same function for held-out `.lcd` inputs; the comparison is
`crates/openreadout-corpus-tests/tests/shimadzu_oracle/mod.rs`.
"""
import hashlib
import json
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "shimadzu"


def labsolutions_export(txt: Path) -> dict:
    """The LabSolutions ASCII export of a run, parsed with the standard library: every
    [LC Chromatogram(...)] block (points, interval, start/end time, first values, all values) and
    every [Peak Table(...)] (R.Time, Area, Height per peak), as the vendor software wrote them."""
    sections: dict[str, list[list[str]]] = {}
    name = None
    for line in txt.read_text(encoding="latin-1").splitlines():
        if line.startswith("[") and line.rstrip().endswith("]"):
            name = line.strip()[1:-1]
            sections[name] = []
        elif name is not None and line.strip():
            sections[name].append(line.split("\t"))
    chroms, tables, status = [], {}, []
    for sec, rows in sections.items():
        if sec.startswith("LC Status Trace("):
            kv = {r[0]: r[1] for r in rows if len(r) > 1 and not r[0][:1].isdigit()}
            data = [r for r in rows if r and r[0][:1].isdigit() and len(r) >= 2]
            status.append({
                "name": sec[len("LC Status Trace("):-1],
                "points": len(data),
                "intensity_units": kv.get("Intensity Units"),
                "intensity_multiplier": float(kv.get("Intensity Multiplier", "1")),
                # the export prints each value with this many decimals at most
                "decimals": max((len(r[1].split(".")[1]) if "." in r[1] else 0) for r in data) if data else 0,
                "values": [float(r[1]) for r in data],
            })
        elif sec.startswith("LC Chromatogram("):
            kv = {r[0]: r[1] for r in rows if len(r) > 1 and not r[0][:1].isdigit()}
            data = [r for r in rows if r and r[0][:1].isdigit() and len(r) >= 2]
            chroms.append({
                "name": sec[len("LC Chromatogram("):-1],
                "points": int(kv["# of Points"]),
                "interval_ms": float(kv["Interval(msec)"]),
                "start_min": float(kv["Start Time(min)"]),
                "end_min": float(kv["End Time(min)"]),
                "intensity_units": kv.get("Intensity Units"),
                "intensity_multiplier": float(kv.get("Intensity Multiplier", "1")),
                "wavelength_nm": _first_number(kv.get("Wavelength(nm)")),
                "first": [float(r[1]) for r in data[:8]],
                "values": [float(r[1]) for r in data],
            })
        elif sec.startswith("Peak Table("):
            hdr = next((r for r in rows if r[0] == "Peak#"), None)
            peaks = []
            if hdr is not None:
                for r in rows[rows.index(hdr) + 1:]:
                    d = dict(zip(hdr, r))
                    peaks.append({"rt_min": float(d["R.Time"]), "area": float(d["Area"]), "height": float(d["Height"]),
                                  "name": d.get("Name", "").strip()})
            tables[sec[len("Peak Table("):-1]] = peaks
    return {"reader": "the depositor's LabSolutions ASCII export of the same run (Python standard library)",
            "shimadzu_export": {"chromatograms": chroms, "peak_tables": tables, "status_traces": status}}


def main() -> None:
    args = sys.argv[1:]
    if len(args) != 4 or args[0] != "--id":
        raise SystemExit(__doc__)
    fid, lcd, txt = args[1], Path(args[2]), Path(args[3])
    data = {"id": fid, "file": lcd.name, "size": lcd.stat().st_size,
            "sha256": hashlib.sha256(lcd.read_bytes()).hexdigest(), **labsolutions_export(txt)}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{fid}.json").write_text(json.dumps(data, indent=1) + "\n")
    ex = data["shimadzu_export"]
    print("wrote", f"{fid}.json", f"{len(ex['chromatograms'])} chromatograms, {len(ex['status_traces'])} status traces, {sum(len(v) for v in ex['peak_tables'].values())} peaks (vendor export)")




def _first_number(text):
    """The first number in a LabSolutions field (`280`, `280nm`), or None when it holds none
    (some exports write only the unit, `nm`, for detectors without a wavelength)."""
    import re

    m = re.search(r"[-+]?\d+(?:\.\d+)?", text or "")
    return float(m.group()) if m else None


if __name__ == "__main__":
    main()
