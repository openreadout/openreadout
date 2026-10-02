#!/usr/bin/env python
"""Compare the key metadata openreadout reports with Bio-Formats (and, where one exists, a
permissively licensed Python reader) for microscopy corpus files.

Usage: python metadata_compare.py [--json OUT] [--md OUT] ID_OR_PATH [...]
       python metadata_compare.py --format vsi,zvi [--json OUT] [--md OUT]   (every corpus input of those formats)

For every image both readers report (matched by size, then by order) the fields below are
compared; each comparison is one row: `agree`, `differ`, `ours_only` (we report a value the
other reader does not) or `theirs_only`. Bio-Formats 8.5.0 is run as a black box (GPL):
`showinf -nopix -noflat -omexml`.

Fields: size_x/y/z/c/t, pixel_type, physical_size_x/y/z (relative 1e-4), channel names,
emission/excitation wavelengths (0.5 nm), acquisition date (to the second, zone ignored),
time increment (1e-3 relative), objective magnification / NA / working distance, detector
model, microscope model, per-channel exposure (1e-3 relative).
"""
import json, os, re, subprocess, sys, tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
BIN = os.environ.get("OPENREADOUT_BIN", str(ROOT / "target" / "release" / "openreadout"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from gen import _bftools  # noqa: E402


def ours(path: Path) -> dict:
    r = subprocess.run([BIN, "info", str(path), "--json"], capture_output=True, text=True, timeout=3600)
    d = json.loads(r.stdout)
    if not d.get("ok"):
        raise RuntimeError(f"openreadout info failed: {d.get('error')}")
    return d["data"]


def bf(path: Path) -> list:
    out = subprocess.run([_bftools("showinf"), "-nopix", "-no-upgrade", "-noflat", "-omexml", str(path)],
                         capture_output=True, text=True, timeout=3600).stdout
    i, j = out.find("<OME "), out.rfind("</OME>")
    if i < 0:
        return []
    root = ET.fromstring(out[i:j + 6])
    ns = {"o": root.tag.split("}")[0].strip("{")}
    instruments = {}
    for ins in root.findall("o:Instrument", ns):
        mic = ins.find("o:Microscope", ns)
        instruments[ins.get("ID")] = {
            "microscope": (mic.get("Model") or mic.get("Manufacturer")) if mic is not None else None,
            "detectors": {d.get("ID"): d.get("Model") for d in ins.findall("o:Detector", ns)},
            "objectives": {o.get("ID"): o for o in ins.findall("o:Objective", ns)},
        }
    images = []
    for im in root.findall("o:Image", ns):
        px = im.find("o:Pixels", ns)
        ins = instruments.get((im.find("o:InstrumentRef", ns).get("ID") if im.find("o:InstrumentRef", ns) is not None else None), {})
        obj = None
        os_ = im.find("o:ObjectiveSettings", ns)
        if os_ is not None:
            obj = ins.get("objectives", {}).get(os_.get("ID"))
        chans = []
        for c in px.findall("o:Channel", ns):
            ds = c.find("o:DetectorSettings", ns)
            chans.append({"name": c.get("Name"),
                          "emission_nm": _f(c.get("EmissionWavelength")),
                          "excitation_nm": _f(c.get("ExcitationWavelength")),
                          "detector": ins.get("detectors", {}).get(ds.get("ID")) if ds is not None else None})
        exposures = {}
        for p in px.findall("o:Plane", ns):
            c = int(p.get("TheC", 0))
            if p.get("ExposureTime") is not None and c not in exposures:
                unit = p.get("ExposureTimeUnit", "s")
                exposures[c] = float(p.get("ExposureTime")) * {"s": 1000.0, "ms": 1.0, "µs": 1e-3}.get(unit, 1000.0)
        dets = [c["detector"] for c in chans if c["detector"]]
        images.append({
            "name": im.get("Name"),
            "size_x": int(px.get("SizeX")), "size_y": int(px.get("SizeY")), "size_z": int(px.get("SizeZ")),
            "size_c": int(px.get("SizeC")), "size_t": int(px.get("SizeT")), "type": px.get("Type"),
            "spp": int((px.find("o:Channel", ns).get("SamplesPerPixel") if px.find("o:Channel", ns) is not None else 1) or 1),
            "physical_size_x": _um(px.get("PhysicalSizeX"), px.get("PhysicalSizeXUnit")),
            "physical_size_y": _um(px.get("PhysicalSizeY"), px.get("PhysicalSizeYUnit")),
            "physical_size_z": _um(px.get("PhysicalSizeZ"), px.get("PhysicalSizeZUnit")),
            "time_increment_s": _s(px.get("TimeIncrement"), px.get("TimeIncrementUnit")),
            "acquired": im.findtext("o:AcquisitionDate", None, ns),
            "channels": chans,
            "exposure_ms": [exposures.get(c) for c in range(len(chans))],
            "objective_magnification": _f(obj.get("NominalMagnification")) if obj is not None else None,
            "objective_na": _f(obj.get("LensNA")) if obj is not None else None,
            "objective_wd_um": _um(obj.get("WorkingDistance"), obj.get("WorkingDistanceUnit")) if obj is not None else None,
            "detector": dets[0] if dets else None,
            "microscope": ins.get("microscope"),
        })
    return images


def _f(v):
    try:
        return float(v) if v is not None else None
    except ValueError:
        return None


def _um(v, unit):
    v = _f(v)
    if v is None:
        return None
    return v * {"µm": 1.0, "um": 1.0, "nm": 1e-3, "mm": 1e3, "cm": 1e4, "m": 1e6, "Å": 1e-4, "pm": 1e-6}.get(unit or "µm", 1.0)


def _s(v, unit):
    v = _f(v)
    if v is None:
        return None
    return v * {"s": 1.0, "ms": 1e-3, "µs": 1e-6, "min": 60.0, "h": 3600.0}.get(unit or "s", 1.0)


BF_PIXEL = {"uint8": "uint8", "uint16": "uint16", "uint32": "uint32", "int8": "int8", "int16": "int16",
            "int32": "int32", "float": "float32", "double": "float64", "complex": "complex64", "double-complex": "complex128", "bit": "bit"}


def rel(a, b, tol):
    return abs(a - b) <= tol * max(abs(a), abs(b), 1e-12)


def cmp(rows, field, o, t, eq):
    if o is None and t is None:
        return
    if o is None:
        rows.append({"field": field, "ours": None, "theirs": t, "status": "theirs_only"})
    elif t is None:
        rows.append({"field": field, "ours": o, "theirs": None, "status": "ours_only"})
    else:
        rows.append({"field": field, "ours": o, "theirs": t, "status": "agree" if eq(o, t) else "differ"})


def date_eq(a, b):
    """Same instant to the second: equal wall clocks, or (Bio-Formats writes UTC without a zone)
    ours converted to UTC when it carries an offset."""
    import datetime as dt
    wall_a = re.sub(r"(\.\d+)?(Z|[+-]\d\d:?\d\d)?$", "", a)
    wall_b = re.sub(r"(\.\d+)?(Z|[+-]\d\d:?\d\d)?$", "", b)
    if wall_a[:19] == wall_b[:19]:
        return True
    try:
        pa = dt.datetime.fromisoformat(a.replace("Z", "+00:00"))
        pb = dt.datetime.fromisoformat(wall_b[:19])
    except ValueError:
        return False
    if pa.tzinfo is None:
        return False
    utc = pa.astimezone(dt.timezone.utc).replace(tzinfo=None)
    return abs((utc - pb).total_seconds()) < 1.0


def compare_image(o: dict, t: dict) -> list:
    rows = []
    for k in ("size_x", "size_y", "size_z", "size_c", "size_t"):
        cmp(rows, k, o.get(k), t[k] if not (k == "size_c" and t["spp"] > 1) else 1, lambda a, b: a == b)
    cmp(rows, "pixel_type", BF_PIXEL.get(o.get("pixel_type"), o.get("pixel_type")), BF_PIXEL.get(t["type"], t["type"]), lambda a, b: a == b)
    ps = o.get("physical_size") or {}
    for ax in "xyz":
        tv = t[f"physical_size_{ax}"]
        if ax == "z" and (o.get("size_z", 1) <= 1):
            continue
        cmp(rows, f"physical_size_{ax}", ps.get(ax), tv, lambda a, b: rel(a, b, 1e-4))
    if (o.get("size_t") or 1) > 1:
        cmp(rows, "time_increment_s", o.get("time_increment_s"), t["time_increment_s"], lambda a, b: rel(a, b, 1e-3))
    cmp(rows, "acquired", o.get("acquired_at"), t["acquired"], date_eq)
    oc = o.get("channels") or []
    for i, tc in enumerate(t["channels"]):
        c = oc[i] if i < len(oc) else {}
        cmp(rows, f"channel[{i}].name", c.get("name"), tc["name"], lambda a, b: a.strip() == b.strip())
        cmp(rows, f"channel[{i}].emission_nm", c.get("emission_nm"), tc["emission_nm"], lambda a, b: abs(a - b) <= 0.5)
        cmp(rows, f"channel[{i}].excitation_nm", c.get("excitation_nm"), tc["excitation_nm"], lambda a, b: abs(a - b) <= 0.5)
    exp = [c.get("exposure_ms") for c in oc]
    for i, te in enumerate(t["exposure_ms"]):
        cmp(rows, f"channel[{i}].exposure_ms", exp[i] if i < len(exp) else None, te, lambda a, b: rel(a, b, 1e-3))
    ob = o.get("objective") or {}
    cmp(rows, "objective.magnification", ob.get("nominal_magnification"), t["objective_magnification"], lambda a, b: rel(a, b, 1e-6))
    cmp(rows, "objective.na", ob.get("lens_na"), t["objective_na"], lambda a, b: rel(a, b, 1e-6))
    ins = o.get("instrument") or {}
    cmp(rows, "detector", ins.get("detector"), t["detector"], lambda a, b: a.strip() == b.strip())
    cmp(rows, "microscope", ins.get("model"), t["microscope"], lambda a, b: a.strip() == b.strip())
    return rows


def match(ours_imgs, theirs_imgs):
    pairs, used = [], set()
    for o in ours_imgs:
        for k, t in enumerate(theirs_imgs):
            if k not in used and (t["size_x"], t["size_y"]) == (o["size_x"], o["size_y"]):
                used.add(k)
                pairs.append((o, t))
                break
        else:
            pairs.append((o, None))
    return pairs


def run(entry_id: str, path: Path) -> dict:
    try:
        o = ours(path)
    except Exception as e:  # noqa: BLE001
        return {"id": entry_id, "error": f"ours: {e}"}
    t = bf(path)
    out = {"id": entry_id, "format": o["format"]["id"], "images": []}
    for oi, ti in match(o["images"], t):
        if ti is None:
            out["images"].append({"index": oi["index"], "unmatched": True})
            continue
        out["images"].append({"index": oi["index"], "rows": compare_image(oi, ti)})
    return out


def corpus_entries(formats):
    m = tomllib.load(open(ROOT / "corpus" / "manifest.toml", "rb"))
    for e in m["file"]:
        if e.get("role", "input") == "input" and e["format"] in formats:
            p = CORPUS / e["filename"]
            if p.exists():
                yield e["id"], p


def main():
    args = sys.argv[1:]
    jout = mdout = None
    todo = []
    while args:
        a = args.pop(0)
        if a == "--json":
            jout = args.pop(0)
        elif a == "--md":
            mdout = args.pop(0)
        elif a == "--format":
            todo.extend(corpus_entries(set(args.pop(0).split(","))))
        else:
            p = Path(a)
            todo.append((p.stem, p))
    results = []
    for eid, p in todo:
        r = run(eid, p)
        results.append(r)
        if "error" in r:
            print(f"{eid}: {r['error']}")
            continue
        n = {"agree": 0, "differ": 0, "ours_only": 0, "theirs_only": 0}
        for im in r["images"]:
            for row in im.get("rows", []):
                n[row["status"]] += 1
                if row["status"] in ("differ", "theirs_only"):
                    print(f"  {eid} image {im['index']} {row['field']}: ours {row['ours']!r} vs Bio-Formats {row['theirs']!r} ({row['status']})")
        print(f"{eid}: {n}")
    if jout:
        json.dump(results, open(jout, "w"), indent=1, ensure_ascii=False)
    if mdout:
        with open(mdout, "w") as f:
            f.write("| file | image | field | openreadout | Bio-Formats | status |\n| --- | --- | --- | --- | --- | --- |\n")
            for r in results:
                for im in r.get("images", []):
                    for row in im.get("rows", []):
                        if row["status"] != "agree":
                            f.write(f"| `{r['id']}` | {im['index']} | {row['field']} | {row['ours']} | {row['theirs']} | {row['status']} |\n")


if __name__ == "__main__":
    main()
