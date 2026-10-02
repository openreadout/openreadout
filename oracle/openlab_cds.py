#!/usr/bin/env python
"""Ground truth for Agilent OpenLab CDS injections (`.dx` containers, `.rx` result packages).

Two independent routes, each a third-party program run as a black box
(docs/legal/clean-room-policy.md rule 3; docs/provenance/openlab-cds.md):

* **Signals** (`gen.py` calls `dx()`; run with `uv run --group chrom`): the `.CH` parts are
  extracted with `zipfile` and read with rainbow-api (LGPL-3.0) `chemstation.parse_file`; the
  whole `.dx` is also read with chromConverter (GPL-3.0, R) `read_agilent_dx(what = c("chroms",
  "instrument"))` through `openlab_chromconverter.R` (set `CHROMCONVERTER_LIB`). The two agree
  sample for sample on the `.CH` parts (checked here); the `.IT` instrument curves come from
  chromConverter only (rainbow does not read them). The trace order is ours: detector signals in
  manifest order, then instrument curves in manifest order (the manifest is read here with
  `xml.etree` only to order and name the traces).

* **Vendor peaks** (`uv run --group plate python openlab_cds.py vendor`): the vendor's own
  integration results, never OpenReadout's. For the Polyarc GC-FID injections (Zenodo
  14316687): allotropy (MIT) `extract_rx_file` reads each `.rx`, and the vendor's peak-table
  export `<name>_1.csv` (read with the `csv` module) names the integrated signal and must agree
  with the `.rx` values to its printed precision (checked here: the run stops otherwise). For the
  allotropy result sets: allotropy `decode_data` on the zipped result set (which maps the `.rx`
  signals through the sequence `.acaml`). Output: `corpus/oracle/openlab/<id>.json`, one per
  injection; `dx()` turns them into `tables` entries (row count and sorted-column hashes).
"""

import csv
import io
import json
import os
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

import numpy as np
import xxhash

ROOT = Path(__file__).resolve().parent.parent
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
VENDOR_OUT = ROOT / "corpus" / "oracle" / "openlab"
R_SCRIPT = Path(__file__).resolve().parent / "openlab_chromconverter.R"
ACMD = "{urn:schemas-agilent-com:acmd20}"
POLYARC = "zenodo14316687-polyarc"
LUXO = (
    "allotropy-openlab-luxo-rslt",
    "allotropy-openlab-luxo-rslt.zip",
    "Luxo HPLC-2023-09-01 07-52-44-04-00.rslt",
    "allotropy-openlab-luxo-",
)
SIRIUS = (
    "allotropy-openlab-sirius-rslt",
    "allotropy-openlab-sirius-rslt.zip",
    "Sirius-2023-09-01 07-52-44-04-00.rslt",
    "allotropy-openlab-sirius-",
)


# ---------------------------------------------------------------- signals (gen.py)


def _manifest(z: zipfile.ZipFile):
    root = ET.fromstring(z.read("injection.acmd").decode("utf-8-sig"))
    info = root.find(ACMD + "InjectionInfo")
    sigs = []
    for s in info.iter(ACMD + "Signal"):
        d = {c.tag.replace(ACMD, ""): (c.text or "").strip() for c in s}
        sigs.append(d)
    return info, sigs


def _chromconverter(p: Path):
    """chromConverter's traces of a .dx: {signal name: (rt_min array, values array, meta)}."""
    with tempfile.TemporaryDirectory() as d:
        r = subprocess.run(["Rscript", str(R_SCRIPT), str(p), d], capture_output=True, text=True, errors="replace")
        if r.returncode != 0:
            raise RuntimeError(f"chromConverter failed: {r.stderr[-400:]}")
        version = r.stdout.strip().splitlines()[-1]
        out = {}
        raw = (Path(d) / "index.csv").read_bytes()
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            text = raw.decode("latin-1")  # R wrote `°C` in its native single-byte encoding
        for row in csv.DictReader(io.StringIO(text, newline="")):
            data = np.loadtxt(Path(d) / f"{row['kind']}_{row['i']}.csv", delimiter=",", skiprows=1, ndmin=2)
            key = row["signal"].split(",")[0].strip()
            out[key] = (data[:, 0], data[:, 1], row)
        return out, version


def dx(p: Path) -> dict:
    """Traces of one .dx (rainbow on the .CH parts, chromConverter on the whole container)."""
    import gen  # the trace-record helpers (_chrom_trace)
    from rainbow.agilent import chemstation as cs

    gen._rainbow()  # silences rainbow's warnings
    z = zipfile.ZipFile(p)
    info, sigs = _manifest(z)
    names = {Path(n).stem.lower(): n for n in z.namelist()}
    cc, cc_version = _chromconverter(p)
    ch = [s for s in sigs if s["Encoding"].rsplit("/", 1)[-1].startswith("Signal") and s["TraceId"].lower() in names]
    it = [s for s in sigs if s["Encoding"].rsplit("/", 1)[-1].startswith("InstrumentTrace") and s["TraceId"].lower() in names]
    sp = [s for s in sigs if s["Encoding"].rsplit("/", 1)[-1].startswith("Spectra") and s["TraceId"].lower() in names]
    traces, notes = [], []
    worst = 0.0
    with tempfile.TemporaryDirectory() as tmp:
        for s in ch:
            part = names[s["TraceId"].lower()]
            q = Path(tmp) / part
            q.write_bytes(z.read(part))
            df = cs.parse_file(str(q))
            x = np.asarray(df.xlabels, dtype=np.float64)
            y = np.asarray(df.data, dtype=np.float64)[:, 0]
            unit = (df.metadata or {}).get("unit", "")
            c = cc.get(s["ChannelName"])
            if c is None:
                raise RuntimeError(f"chromConverter has no trace {s['ChannelName']}")
            if len(c[1]) != len(y) or not np.allclose(c[1], y, rtol=0, atol=1e-9) or not np.allclose(c[0], x, rtol=1e-9, atol=1e-12):
                raise RuntimeError(f"{s['ChannelName']}: rainbow and chromConverter disagree")
            worst = max(worst, float(np.max(np.abs(c[1] - y))) if len(y) else 0.0)
            t = gen._chrom_trace(len(traces), [s["ChannelName"]], [unit], x, [y])
            t["name"] = (df.metadata or {}).get("signal") or s["ChannelName"]
            meta = c[2]
            t["parameters"] = {"extra": {"sample_name": meta["sample_name"], "operator": meta["operator"],
                                         "method_path": meta["method"]}}
            traces.append(t)
        for s in it:
            c = cc.get(s["ChannelName"])
            if c is None:
                raise RuntimeError(f"chromConverter has no instrument curve {s['ChannelName']}")
            x, y, meta = c
            t = gen._chrom_trace(len(traces), [s["ChannelName"]], [meta["units"]], x, [y])
            t["name"] = meta["signal"]
            traces.append(t)
        for s in sp:
            # DAD spectra parts (ChemStation-style .uv files): rainbow parse_uv. For records of
            # f64 values (tag 70) rainbow returns stored value x scale; the unit (mAU) is one more
            # scale factor away (docs/formats/openlab-cds.md: established against the DAD's own
            # channels, slope 0.997, r 0.999997), applied here as the reader does, in the same order.
            # the spectra part itself, not its spectra directory (.UVD, same stem)
            part = next(n for n in z.namelist() if n.lower() == s["TraceId"].lower() + ".uv")
            q = Path(tmp) / (Path(part).stem + ".uv")
            q.write_bytes(z.read(part))
            df = cs.parse_uv(str(q))
            x = np.asarray(df.xlabels, dtype=np.float64)
            data = np.asarray(df.data, dtype=np.float64)
            scale = float(s.get("ScaleFactor") or 1.0)
            b = q.read_bytes()
            first_tag = int.from_bytes(b[4096:4098], "little") if len(b) > 4098 else 0
            if first_tag == 70:
                data = data * scale
            unit = (df.metadata or {}).get("unit", "")
            names_w = [f"{w:g} nm" for w in np.asarray(df.ylabels, dtype=np.float64)]
            t = gen._chrom_trace(len(traces), names_w, [unit] * len(names_w), x, [data[:, k] for k in range(data.shape[1])])
            t["name"] = s.get("Description") or s["ChannelName"]
            traces.append(t)
            notes.append(f"spectra part {part}: rainbow-api parse_uv ({data.shape[0]} spectra x {data.shape[1]} wavelengths){' x the scale factor once more (tag-70 f64 records)' if first_tag == 70 else ''}")
    notes.append(f"detector signals: rainbow-api parse_file on the extracted .CH parts, equal to chromConverter {cc_version} (max |difference| {worst:g}); instrument curves: chromConverter {cc_version} read_agilent_dx(what = 'instrument')")
    out = {"reader": f"rainbow-api (black box) + chromConverter {cc_version} (black box)", "oracle_note": "; ".join(notes), "traces": traces}
    vendor = VENDOR_OUT / f"{_id_for(p)}.json"
    if vendor.exists():
        v = json.loads(vendor.read_text())
        out["tables"] = [_table_oracle(v)]
    return out


def _id_for(p: Path) -> str:
    """The oracle id of a .dx (manifest id pattern: polyarc and allotropy sets)."""
    s = str(p)
    if POLYARC in s:
        return f"{POLYARC}-{p.stem.replace('FKB-FA-035-', '').lower()}"
    if LUXO[2] in s:
        return LUXO[3] + p.stem.rsplit("-", 1)[-1]
    if SIRIUS[2] in s:
        return SIRIUS[3] + p.stem.rsplit("-", 1)[-1]
    return p.stem.lower()


def _sorted_hash(values) -> str:
    v = np.sort(np.asarray(values, dtype="<f8"))
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(v).tobytes())


def _table_oracle(v: dict) -> dict:
    """Row count and sorted-column hashes of the vendor peak table (values exactly as stored)."""
    peaks = [p for s in v["signals"] for p in s["peaks"]]
    t = {"index": 0, "event_count": len(peaks), "parameter_names": []}
    cols = ["rt_min", "start_min", "end_min", "area", "height", "area_percent"]
    if v.get("exact", True) and peaks:
        t["sorted_column_hashes"] = [
            {"column": c, "xxh3": _sorted_hash([p[c] for p in peaks]), "count": len(peaks)}
            for c in cols
            if all(p.get(c) is not None for p in peaks)
        ]
    return t


# ---------------------------------------------------------------- vendor peaks


def _f(x):
    """xmltodict `{'@val': '1.5', '@unit': 'min'}` (or a plain string) → float, None if absent."""
    if x is None:
        return None
    if isinstance(x, dict):
        x = x.get("@val")
    try:
        return float(x)
    except (TypeError, ValueError):
        return None


def _minutes(x):
    v = _f(x)
    if v is None:
        return None
    unit = x.get("@unit", "min") if isinstance(x, dict) else "min"
    return {"min": v, "s": v / 60.0, "ms": v / 60000.0}.get(unit)


def _peak(p: dict) -> dict:
    text = lambda k: (p.get(k) or "").strip() if isinstance(p.get(k), str) else None  # noqa: E731
    return {
        "rt_min": _minutes(p.get("RetentionTime")),
        "start_min": _minutes(p.get("BeginTime")),
        "end_min": _minutes(p.get("EndTime")),
        "area": _f(p.get("Area")),
        "area_unit": (p.get("Area") or {}).get("@unit"),
        "height": _f(p.get("Height")),
        "area_percent": _f(p.get("AreaPercent")),
        "height_percent": _f(p.get("HeightPercent")),
        "width_base_min": _minutes(p.get("WidthBase")),
        "symmetry": _f(p.get("Symmetry")),
        "baseline_start": _f(p.get("BaselineStart")),
        "baseline_end": _f(p.get("BaselineEnd")),
        "code": text("BaselineCode"),
        "type": text("Type"),
    }


def _csv(path: Path) -> dict:
    """The vendor's peak-table export (UTF-8 CSV: header key/value pairs, then the table)."""
    lines = path.read_text(encoding="utf-8-sig").splitlines()
    head, rows, cols = {}, [], None
    for row in csv.reader(lines):
        if not row:
            continue
        if cols is None and row[0].startswith("RT [min]"):
            cols = row
            continue
        if cols is None:
            for k, v in zip(row[0::2], row[1::2]):
                head[k.rstrip(":").strip()] = v.strip()
        elif row[0].strip():
            d = dict(zip(cols, row))
            rows.append({"rt_min": float(d["RT [min]"]), "type": d["Type"].strip(), "width_min": float(d["Width [min]"]),
                         "area": float(d["Area"]), "height": float(d["Height"]), "area_percent": float(d["Area%"]),
                         "name": d.get("Name", "").strip() or None})
    return {"header": head, "peaks": rows}


def _decimals(s: float, d: int) -> float:
    return 0.5 * 10 ** (-d) + 1e-9


def _check_csv(name: str, rx_peaks: list, c: dict):
    """The .rx values must round to the CSV's (3 decimals RT, 2 elsewhere)."""
    if len(rx_peaks) != len(c["peaks"]):
        raise RuntimeError(f"{name}: {len(rx_peaks)} .rx peaks, {len(c['peaks'])} CSV rows")
    for a, b in zip(sorted(rx_peaks, key=lambda p: p["rt_min"]), c["peaks"]):
        for k, d, ck in [("rt_min", 3, "rt_min"), ("width_base_min", 2, "width_min"), ("area", 2, "area"),
                         ("height", 2, "height"), ("area_percent", 2, "area_percent")]:
            if abs(a[k] - b[ck]) > _decimals(b[ck], d):
                raise RuntimeError(f"{name}: {k} {a[k]} does not round to the CSV's {b[ck]}")
        if a["code"] != b["type"]:
            raise RuntimeError(f"{name}: baseline code {a['code']!r} != CSV type {b['type']!r}")


def polyarc():
    from allotropy.parsers.agilent_openlab_cds.agilent_openlab_cds_decoder import extract_rx_file

    meas = CORPUS / POLYARC / "Measurements"
    n = 0
    for dxp in sorted(meas.glob("*.dx")):
        name = dxp.stem
        with open(dxp.with_suffix(".rx"), "rb") as f:
            details = extract_rx_file(f)
        with_peaks = [d for d in details if d.get("Peak")]
        if len(with_peaks) != 1:
            raise RuntimeError(f"{name}: {len(with_peaks)} signal results with peaks (expected 1)")
        raw = with_peaks[0]["Peak"]
        raw = raw if isinstance(raw, list) else [raw]
        peaks = sorted((_peak(p) for p in raw), key=lambda p: p["rt_min"])
        c = _csv(meas / f"{name}_1.csv")
        _check_csv(name, peaks, c)
        h = c["header"]
        data = {
            "id": _id_for(dxp),
            "input": str(dxp.relative_to(CORPUS)),
            "source": "allotropy (MIT) extract_rx_file on the .rx; signal name and metadata from the vendor's _1.csv export; .rx values checked against the CSV to its printed precision",
            "exact": True,
            "signals": [{"signal": h["Signal"], "signal_source": "vendor CSV export", "peaks": peaks}],
            "csv": {
                "sample_name": h.get("Sample name"), "operator": h.get("Operator"), "vial": h.get("Location"),
                "injection_volume_ul": float(h["Inj. volume"]) if h.get("Inj. volume") else None,
                "method": h.get("Acq. method"), "processing_method": h.get("Processing method"),
                "injection_date": h.get("Injection date"), "signal": h["Signal"], "sequence": h.get("Sequence Name"),
                "peaks": c["peaks"],
            },
        }
        VENDOR_OUT.mkdir(parents=True, exist_ok=True)
        (VENDOR_OUT / f"{data['id']}.json").write_text(json.dumps(data, indent=1) + "\n")
        n += 1
    print(f"polyarc: {n} injections")


def result_set(which):
    from allotropy.parsers.agilent_openlab_cds.agilent_openlab_cds_decoder import decode_data

    bundle, zip_name, rslt, prefix = which
    raw = (CORPUS / zip_name).read_bytes()
    d = decode_data(io.BytesIO(raw))
    by_file = {}
    for chrom in d["Result Data"]:
        dx_name = Path(chrom["file_name"]).name
        signal = chrom["Metadata"]["signal"].split(",")[0].strip()
        peaks = chrom.get("Peak") or []
        entry = by_file.setdefault(dx_name, [])
        entry.append({"signal": signal, "signal_source": "allotropy (sequence .acaml)",
                      "peaks": sorted((_peak(p) for p in peaks), key=lambda p: p["rt_min"])})
    n = 0
    for dx_name, signals in sorted(by_file.items()):
        dxp = CORPUS / bundle / rslt / dx_name
        data = {"id": _id_for(dxp), "input": str(dxp.relative_to(CORPUS)),
                "source": "allotropy (MIT) decode_data on the zipped result set (peaks per signal, mapped through the sequence .acaml)",
                "exact": True, "signals": [s for s in signals if s["peaks"]]}
        VENDOR_OUT.mkdir(parents=True, exist_ok=True)
        (VENDOR_OUT / f"{data['id']}.json").write_text(json.dumps(data, indent=1) + "\n")
        n += 1
    print(f"{prefix}*: {n} injections")


if __name__ == "__main__":
    if sys.argv[1:2] == ["vendor"]:
        polyarc()
        result_set(LUXO)
        result_set(SIRIUS)
    else:
        sys.exit(__doc__)
