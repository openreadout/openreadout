#!/usr/bin/env python
"""Validate an mzML written by `openreadout export --to mzml`.

Usage: uv run python mzml_validate.py OURS.mzML [REFERENCE.mzML|.mzXML] [--xsd]

1. Reads OURS with pyteomics through its index (PreIndexedMzML), which exercises the
   <indexList> offsets, and decodes every binary array.
2. Checks the SHA-1 <fileChecksum>.
3. With --xsd, validates against the PSI indexed-mzML 1.1 schema (downloaded once to
   oracle/.cache/; needs network the first time).
4. With a REFERENCE (the depositor's own conversion), compares every spectrum: count, scan
   number, MS level, polarity, retention time (1e-3 s), filter string, precursor m/z (1e-6
   relative) and the m/z (1e-6 relative) and intensity (1e-4 relative) arrays.
Exit status 0 when everything agrees.
"""
import hashlib, re, sys, urllib.request
from pathlib import Path
import numpy as np
from pyteomics import mzml, mzxml

HERE = Path(__file__).resolve().parent
XSD_URLS = {
    "mzML1.1.2_idx.xsd": "https://raw.githubusercontent.com/HUPO-PSI/mzML/master/schema/schema_1.1/mzML1.1.2_idx.xsd",
    "mzML1.1.0.xsd": "https://raw.githubusercontent.com/HUPO-PSI/mzML/master/schema/schema_1.1/mzML1.1.0.xsd",
}


def checksum_ok(path: Path) -> bool:
    raw = path.read_bytes()
    marker = b"<fileChecksum>"
    at = raw.rfind(marker)
    stored = raw[at + len(marker): raw.find(b"</fileChecksum>", at)].decode()
    return hashlib.sha1(raw[: at + len(marker)]).hexdigest() == stored


def xsd_ok(path: Path) -> str:
    from lxml import etree
    cache = HERE / ".cache"
    cache.mkdir(exist_ok=True)
    for name, url in XSD_URLS.items():
        p = cache / name
        if not p.exists():
            p.write_bytes(urllib.request.urlopen(url, timeout=60).read())
    # The indexed schema imports the plain one by its psidev URL; point it at the local copy.
    idx = (cache / "mzML1.1.2_idx.xsd").read_text()
    idx = re.sub(r'schemaLocation="[^"]*mzML1\.1\.0\.xsd"', 'schemaLocation="mzML1.1.0.xsd"', idx)
    (cache / "idx_local.xsd").write_text(idx)
    schema = etree.XMLSchema(etree.parse(str(cache / "idx_local.xsd")))
    doc = etree.parse(str(path))
    if schema.validate(doc):
        return "valid"
    return "; ".join(str(e) for e in list(schema.error_log)[:5])


def records(path: Path, indexed: bool = False):
    if path.suffix.lower() == ".mzxml":
        with mzxml.MzXML(str(path), read_schema=True) as f:
            for sp in f:
                prec = (sp.get("precursorMz") or [{}])[-1]
                yield {
                    "scan": int(sp["num"]), "ms_level": int(sp["msLevel"]),
                    "rt_s": float(sp["retentionTime"]) * 60.0,
                    "polarity": {"+": "positive", "-": "negative"}.get(sp.get("polarity")),
                    "centroided": bool(int(sp.get("centroided", 0))), "filter": sp.get("filterLine"),
                    "precursor": prec.get("precursorMz"),
                    "mz": np.asarray(sp["m/z array"], dtype=np.float64), "it": np.asarray(sp["intensity array"], dtype=np.float64),
                }
        return
    reader = mzml.PreIndexedMzML(str(path)) if indexed else mzml.MzML(str(path))
    with reader as f:
        for sp in f:
            scan = sp.get("scanList", {}).get("scan", [{}])[0]
            m = re.search(r"scan=(\d+)", sp["id"])
            pl = sp.get("precursorList", {}).get("precursor", [])
            prec = pl[-1]["selectedIonList"]["selectedIon"][0].get("selected ion m/z") if pl else None
            yield {
                "scan": int(m.group(1)) if m else sp["index"] + 1, "ms_level": int(sp["ms level"]),
                "rt_s": float(scan["scan start time"]) * 60.0,
                "polarity": "positive" if "positive scan" in sp else "negative" if "negative scan" in sp else None,
                "centroided": "centroid spectrum" in sp, "filter": scan.get("filter string"),
                "precursor": prec,
                "mz": np.asarray(sp["m/z array"], dtype=np.float64), "it": np.asarray(sp["intensity array"], dtype=np.float64),
            }


def close(a, b, tol):
    return a.shape == b.shape and bool(np.all(np.abs(a - b) <= tol * np.maximum(np.abs(a), np.abs(b)) + 1e-12))


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    ours = Path(args[0])
    ref = Path(args[1]) if len(args) > 1 else None
    problems = []
    print("checksum:", "ok" if checksum_ok(ours) else "MISMATCH")
    if not checksum_ok(ours):
        problems.append("checksum")
    if "--xsd" in sys.argv:
        r = xsd_ok(ours)
        print("schema:", r)
        if r != "valid":
            problems.append("schema")
    import warnings
    with warnings.catch_warnings():
        warnings.simplefilter("error")  # a fallback from the embedded index is a failure for our files
        a = list(records(ours, indexed=True))
    print("spectra read through the index:", len(a))
    if ref is not None:
        b = list(records(ref))
        if len(a) != len(b):
            problems.append(f"count {len(a)} != {len(b)}")
        exact = close_n = differ = meta = 0
        for x, y in zip(a, b):
            bad = []
            for k in ("scan", "ms_level", "polarity", "centroided"):
                if x[k] != y[k]:
                    bad.append(f"{k} {x[k]} != {y[k]}")
            if abs(x["rt_s"] - y["rt_s"]) > 1e-3:
                bad.append(f"rt {x['rt_s']} != {y['rt_s']}")
            if y["filter"] is not None and x["filter"] != y["filter"]:
                bad.append(f"filter {x['filter']!r} != {y['filter']!r}")
            if y["precursor"] is not None and (x["precursor"] is None or abs(x["precursor"] - y["precursor"]) > 1e-6 * y["precursor"]):
                bad.append(f"precursor {x['precursor']} != {y['precursor']}")
            if bad:
                meta += 1
                if meta <= 5:
                    print("  scan", y["scan"], "; ".join(bad))
            ym = y["mz"]
            xm = x["mz"].astype(np.float32).astype(np.float64) if ym.dtype == np.float64 and np.array_equal(ym, ym.astype(np.float32).astype(np.float64)) and not np.array_equal(x["mz"], ym) else x["mz"]
            if np.array_equal(xm, ym) and np.array_equal(x["it"].astype(np.float32), y["it"].astype(np.float32)):
                exact += 1
            elif close(x["mz"], ym, 1e-6) and close(x["it"], y["it"], 1e-4):
                close_n += 1
            else:
                differ += 1
                if differ <= 5:
                    print("  scan", y["scan"], "arrays differ:", len(x["mz"]), "vs", len(ym), "points")
        print(f"vs {ref.name}: {len(b)} spectra; arrays {exact} exact, {close_n} within tolerance, {differ} differ; metadata mismatches {meta}")
        if differ or meta:
            problems.append("comparison")
    print("OK" if not problems else "PROBLEMS: " + ", ".join(problems))
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
