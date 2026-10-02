"""Ground truth for Thermo Scientific Chromeleon 7 archives (`.cmbx`) from the vendor's own
outputs of the same injections, read without any Chromeleon reader:

- Chromeleon ASCII chromatogram exports (tab-separated text: an injection block, a
  "Chromatogram Data Information" block, then `Time (min)`, `Step (s)`, `Value (<unit>)` rows),
  read with the standard library;
- a Chromeleon PDF report ("Chromatogram and Results" pages), read with pypdf (BSD-3-Clause):
  the page text (injection name, run time, inject time, the integration results table) and the
  page's vector drawing (the plotted chromatogram path, the red peak baselines and the blue
  peak delimiters), in PDF points.

    uv run --no-project --with pypdf==6.19.0 python chromeleon_oracle.py --id ID --exports EXPORT.txt...
    uv run --no-project --with pypdf==6.19.0 python chromeleon_oracle.py --id ID --report REPORT.pdf
                                                      -> ../corpus/oracle/chromeleon/<id>.json

The comparison is `crates/openreadout-corpus-tests/tests/chromeleon_oracle/mod.rs`.
"""
import argparse
import datetime
import json
import re
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "chromeleon"


def ascii_export(path: Path) -> dict:
    """One Chromeleon ASCII export: its header fields and every (time, value) row."""
    text = path.read_text(encoding="utf-8-sig")
    meta, times, values = {}, [], []
    for line in text.splitlines():
        cells = line.split("\t")
        if re.match(r"^-?\d+(\.\d+)?$", cells[0]) and len(cells) >= 3:
            times.append(float(cells[0]))
            values.append(float(cells[2]))
        elif len(cells) >= 2 and cells[0] and cells[0] not in meta:
            meta[cells[0]] = cells[1].strip()
    # "5/07/2024 7:38:21 PM +10:00" -> UTC
    inject = datetime.datetime.strptime(meta["Inject Time"], "%d/%m/%Y %I:%M:%S %p %z")
    # the printed times are a regular grid: keep its first time and step, and how far any
    # printed time is from it (instead of every time)
    step_min = float(meta["Average Step (s)"]) / 60.0
    grid_dev = max((abs(t - (times[0] + i * step_min)) for i, t in enumerate(times)), default=0.0)
    return {
        "file": path.name,
        "sequence": meta.get("Sequence"),
        "injection": meta.get("Name"),
        "channel": meta.get("Channel"),
        "inject_time_utc": inject.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "unit": meta.get("Signal Unit"),
        "points": int(meta["Data Points"]),
        "time_min": float(meta["Time Min. (min)"]),
        "time_max": float(meta["Time Max. (min)"]),
        "signal_min": float(meta["Signal Min."]),
        "signal_max": float(meta["Signal Max."]),
        "step_s": float(meta["Average Step (s)"]),
        "generating_system": meta.get("Generating Data System"),
        "number": meta.get("Number"),
        "position": meta.get("Position"),
        "injection_type": meta.get("Type"),
        "status": meta.get("Status"),
        "processing_method": meta.get("Processing Method"),
        "instrument_method": meta.get("Instrument Method"),
        "volume_ul": float(meta["Volume (µl)"]) if meta.get("Volume (µl)") else None,
        "dilution_factor": float(meta["Dilution Factor"]) if meta.get("Dilution Factor") else None,
        "weight": float(meta["Weight"]) if meta.get("Weight") else None,
        "first_time": times[0] if times else None,
        "time_grid_max_deviation_min": grid_dev,
        "values": values,
    }


def _num(s: str) -> float:
    return float(s.replace(",", "."))


def _paths(stream: str):
    """(stroke colour, [(x, y), ...]) for every path drawn with m/l operators."""
    toks = re.findall(r"-?\d+\.?\d*|[A-Za-z*'\"]+|<[0-9A-Fa-f]*>|\[|\]", stream)
    stack, out, cur, colour = [], [], None, None
    for t in toks:
        if re.fullmatch(r"-?\d+\.?\d*", t):
            stack.append(float(t))
            continue
        if t == "RG" and len(stack) >= 3:
            colour = tuple(stack[-3:])
        elif t == "G" and stack:
            colour = (stack[-1],) * 3
        elif t == "m" and len(stack) >= 2:
            cur = [(stack[-2], stack[-1])]
            out.append((colour, cur))
        elif t == "l" and cur is not None and len(stack) >= 2:
            cur.append((stack[-2], stack[-1]))
        if not t.startswith("<"):
            stack = []
    return out


def report(path: Path) -> list:
    """Every 'Chromatogram and Results' page of a Chromeleon PDF report."""
    import pypdf  # BSD-3-Clause

    pages = []
    for i, page in enumerate(pypdf.PdfReader(str(path)).pages):
        text = page.extract_text()
        if "Integration Results" not in text:
            continue
        name = re.search(r"Injection Name: (.*?) Run Time \(min\): ([\d,]+)", text)
        when = re.search(r"Injection Date/Time: (.*?) Sample Weight", text)

        def field(label, until):
            m = re.search(re.escape(label) + r"\s*(.*?)\s+" + re.escape(until), text, re.S)
            return m.group(1).strip() if m else None

        details = {
            "vial": field("Vial Number:", "Injection Volume:"),
            "volume_ul": _num(field("Injection Volume:", "Injection Type:") or "nan"),
            "injection_type": field("Injection Type:", "Channel:"),
            "channel": field("Channel:", "Calibration Level:"),
            "instrument_method": field("Instrument Method:", "Bandwidth:"),
            "processing_method": field("Processing Method:", "Dilution Factor:"),
            "dilution_factor": _num(field("Dilution Factor:", "Injection Date/Time:") or "nan"),
            "weight": _num((re.search(r"Sample Weight: ([\d,.]+)", text) or [None, "nan"])[1]),
        }
        peaks = []
        for line in text.split("\n"):
            m = re.match(
                r"^(\d+) (.*?) ?([\d,]+) ([\d,]+) ([\d,]+) ([\d,]+) ([\d,]+) (n\.a\.|[\d,]+)\s*$", line
            )
            if m:
                peaks.append({
                    "number": int(m.group(1)),
                    "name": m.group(2).strip(),
                    "rt_min": _num(m.group(3)),
                    "area": _num(m.group(4)),
                    "height": _num(m.group(5)),
                    "rel_area": _num(m.group(6)),
                    "rel_height": _num(m.group(7)),
                })
        units = re.search(r"\n\s*min (\S+) (\S+) % %", text)
        paths = _paths(page.get_contents().get_data().decode("latin-1"))
        trace = max(paths, key=lambda p: len(p[1]))[1]
        red = [p for c, p in paths if c == (1.0, 0.0, 0.0) and len(p) == 2]
        blue = [p for c, p in paths if c == (0.0, 0.0, 1.0) and len(p) == 2]
        pages.append({
            "page": i + 1,
            "injection": name.group(1),
            "run_time_min": _num(name.group(2)),
            "injection_time": when.group(1) if when else None,
            "details": details,
            "area_unit": units.group(1) if units else None,
            "height_unit": units.group(2) if units else None,
            "peaks": peaks,
            "plot": {
                "x0": min(x for x, _ in trace),
                "x1": max(x for x, _ in trace),
                "vertices": [[x, y] for x, y in trace],
                "baselines": [[a[0], a[1], b[0], b[1]] for a, b in red],
                "delimiters": [[a[0], a[1], b[1]] for a, b in blue if a[0] == b[0]],
            },
        })
    return pages


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", required=True)
    ap.add_argument("--exports", nargs="*", default=[])
    ap.add_argument("--report")
    a = ap.parse_args()
    out = {"id": a.id, "reader": "Chromeleon's own exports (stdlib; pypdf for the PDF report)"}
    if a.exports:
        out["ascii_exports"] = [ascii_export(Path(p)) for p in a.exports]
    if a.report:
        out["pdf_report"] = {"file": Path(a.report).name, "pages": report(Path(a.report))}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{a.id}.json").write_text(json.dumps(out, separators=(",", ":")) + "\n")
    print(a.id, {k: (len(v) if isinstance(v, list) else len(v.get("pages", []))) for k, v in out.items() if k in ("ascii_exports", "pdf_report")})


if __name__ == "__main__":
    main()
