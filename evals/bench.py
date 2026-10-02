"""Bench-instrument questions (ÄKTA/UNICORN, MicroCal ITC, Biacore SPR, Image Lab gels, JASCO):
vendor-computed values and run facts an agent should read without re-deriving them.

Every answer comes from the vendor's own numbers or a third-party reader, never from OpenReadout:
UNICORN's peak table in the result export (XML, standard library), Origin's integrated-heats
export of an ITC run, the Biacore control software's `Environment`/`Chip` text (olefile, BSD-2),
Image Lab's `ImageHeader` XML, and Spectra Manager's text export of a JASCO FT-IR spectrum. Where
a second source exists it must agree or the script stops.

The values go to `evals/facts/bench.json` (committed); `generate.py` reads them like the other facts
(`facts-bench:` sources), so the questions regenerate without the corpus.

    oracle/.venv/bin/python evals/bench.py          # recompute evals/facts/bench.json
    oracle/.venv/bin/python evals/bench.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "oracle"))
import analysis as a
import facts
import generate as g

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(__file__).resolve().parent / "facts" / "bench.json"


# ---------------------------------------------------------------- readers


def unicorn_peak_table(cid: str) -> list[dict]:
    """UNICORN's peak table(s) in the result export: Chrom.1.Xml `PeakTables` (vendor values)."""
    from fplc_oracle import _fixzip, _logical

    z = _fixzip(a.path(cid).read_bytes())
    names = {_logical(n): n for n in z.namelist()}
    root = ET.fromstring(z.read(names["chrom.1.xml"]).decode("utf-8-sig"))
    out = []
    pts = root.find("PeakTables")
    for pt in list(pts) if pts is not None else []:
        pks = pt.find("Peaks")
        for pk in list(pks) if pks is not None else []:
            out.append(
                {
                    "table": pt.findtext("Name"),
                    "area": float(pk.findtext("Area")),
                    "height": float(pk.findtext("Height")),
                    "retention": float(pk.findtext("MaxPeakRetention")),
                }
            )
    return out


def unicorn_largest_area(cid: str):
    peaks = unicorn_peak_table(cid)
    return max(p["area"] for p in peaks), "UNICORN peak table (result export XML)"


def unicorn_peak_count(cid: str):
    return len(unicorn_peak_table(cid)), "UNICORN peak table (result export XML)"


def itc_heats(cid: str) -> dict:
    """Origin's integrated-heats export (the `-heats` oracle-export entry of the run)."""
    import tomllib

    from itc_oracle import heats_export

    entries = tomllib.loads(facts.MANIFEST.read_text())["file"]
    name = next(
        e["filename"]
        for e in entries
        if e["id"] == cid and e.get("role") == "oracle-export" and "-heats." in e["filename"]
    )
    return heats_export(facts.corpus_dir() / name)


def biacore_text(cid: str, stream: str) -> dict:
    import olefile

    o = olefile.OleFileIO(str(a.path(cid)))
    text = o.openstream(stream).read().decode("latin-1")
    return dict(line.split("=", 1) for line in text.splitlines() if "=" in line)


def biacore_cycles(cid: str):
    import olefile

    o = olefile.OleFileIO(str(a.path(cid)))
    return len({e[0] for e in o.listdir(streams=True, storages=False) if e[0].startswith("_Cycle ")}), a.rd("olefile")


def scn_attribute(cid: str, name: str):
    """A `scan_attributes` value from Image Lab's ImageHeader XML part (text in the file)."""
    b = a.path(cid).read_bytes()
    i = b.find(b"<scan_attributes")
    j = b.find(b"</scan_attributes>", i)
    root = ET.fromstring(b[i : j + len(b"</scan_attributes>")].decode("utf-8", "replace"))
    for att in root.iter():
        if att.get("name") == name:
            return att.get("value"), "Image Lab ImageHeader XML"
        if att.findtext("name") == name:
            return att.findtext("value"), "Image Lab ImageHeader XML"
    raise SystemExit(f"{cid}: no scan attribute {name!r}")


def jasco_footer(cid: str, key: str):
    """A setting from the Spectra Manager text export's footer (vendor text)."""
    raw = a.path(cid, "oracle-export").read_bytes().decode("cp1250")
    for line in raw.splitlines():
        k, _, v = line.partition("\t")
        if k.strip() == key:
            return v.strip(), "JASCO Spectra Manager text export"
    raise SystemExit(f"{cid}: no {key!r} in the export")


def seahorse_root(cid: str):
    import gzip

    return ET.fromstring(gzip.decompress(a.path(cid).read_bytes()))


def seahorse_port_a(cid: str):
    """The reagent loaded in port A (the first group with injections), from the assay XML."""
    for inj in seahorse_root(cid).iter():
        if inj.tag.split("}")[-1] == "Injections":
            for p in inj:
                vals = {x.tag.split("}")[-1]: (x.text or "").strip() for x in p.iter()}
                if vals.get("PortLocation") == "A" and vals.get("ReagentName"):
                    return vals["ReagentName"], "Seahorse assay XML (gzip + ElementTree)"
    raise SystemExit(f"{cid}: no port A reagent")


def seahorse_measurements(cid: str):
    root = seahorse_root(cid)
    spans = [e for e in root.iter() if e.tag.split("}")[-1] == "RateSpan"]
    measures = [e for e in root.iter() if e.tag.split("}")[-1] == "CommandName" and e.text == "Measure"]
    if len(spans) != len(measures):
        raise SystemExit(f"{cid}: {len(spans)} rate spans, {len(measures)} Measure commands")
    return len(spans), "Seahorse assay XML: rate spans (= Measure commands)"


def gpr_table(cid: str):
    import io

    import pandas as pd

    lines = a.path(cid).read_bytes().decode("latin-1").splitlines()
    n = int(lines[1].split()[0])
    return pd.read_csv(io.StringIO("\n".join(lines[2 + n :])), sep="\t", dtype=str, keep_default_na=False)


def gpr_brightest(cid: str, column: str):
    """Name of the feature with the highest value in `column` (pandas)."""
    import pandas as pd

    df = gpr_table(cid)
    v = pd.to_numeric(df[column], errors="coerce")
    i = int(v.idxmax())
    if (v == v.iloc[i]).sum() != 1:
        raise SystemExit(f"{cid}: several features share the maximum of {column}")
    return df["Name"].iloc[i].strip(), a.rd("pandas")


def xrd_export_max(cid: str, above_deg: float):
    """2θ of the most intense point above `above_deg` in the diffractometer software's CSV export
    ([Scan points]). The low-angle end of a scan is background rising towards the direct beam
    (the whole scan's maximum is its first points), so the strongest reflection is sought above
    it; a tie for the maximum stops the script."""
    lines = a.path(cid, "oracle-export").read_text(encoding="utf-8", errors="replace").splitlines()
    k = next(i for i, line in enumerate(lines) if line.strip() == "[Scan points]")
    head = [h.strip() for h in lines[k + 1].split(",")]
    rows = [[float(v) for v in line.split(",")] for line in lines[k + 2 :] if line.strip()]
    ia, ii = head.index("Angle"), head.index("Intensity")
    rows = [r for r in rows if r[ia] > above_deg]
    best = max(rows, key=lambda r: r[ii])
    if sum(1 for r in rows if r[ii] == best[ii]) != 1:
        raise SystemExit(f"{cid}: several points above {above_deg}° share the largest intensity")
    return best[ia], "Data Collector CSV export"


def mpt_column_extreme(cid: str, column: str, kind: str):
    """Largest (or smallest) value of a column in EC-Lab's own `.mpt` export of the run."""
    lines = a.path(cid, "input").read_text(encoding="latin-1").splitlines()
    nh = int(lines[1].split(":")[1])
    head = [h.strip() for h in lines[nh - 1].split("\t")]
    col = head.index(column)
    vals = [float(line.split("\t")[col].replace(",", ".")) for line in lines[nh:] if line.strip()]
    return (max(vals) if kind == "max" else min(vals)), "EC-Lab .mpt export"


def ua_export_header(cid: str, key: str):
    """A header line of a Universal Analysis text export (UTF-16)."""
    raw = a.path(cid, "oracle-export").read_bytes()
    for line in raw.decode("utf-16").splitlines():
        f = line.split("\t")
        if f[0] == key:
            return f[1].strip(), "Universal Analysis export"
    raise SystemExit(f"{cid}: no {key} line in the export")


def neware_max_voltage(cid: str):
    import NewareNDA  # type: ignore

    df = NewareNDA.read(str(a.path(cid)), software_cycle_number=False, log_level="ERROR")
    return float(df["Voltage"].max()), a.rd("NewareNDA")


def ngb_sample_mass(cid: str):
    import json

    import pyngb  # type: ignore

    t = pyngb.read_ngb(str(a.path(cid)))
    return json.loads(t.schema.metadata[b"file_metadata"])["sample_mass"], a.rd("pyngb")


def leading_number(s: str) -> float:
    m = re.search(r"[-+]?\d+(?:[.,]\d+)?", s.split("(", 1)[-1])
    return float(m.group(0).replace(",", "."))


# ---------------------------------------------------------------- facts


FACTS: list[a.Fact] = [
    a.Fact(
        "unicorn-zip-fs-sample",
        "largest_peak_area",
        "UNICORN peak table: largest Area",
        lambda: unicorn_largest_area("unicorn-zip-fs-sample"),
    ),
    a.Fact(
        "unicorn-zip-fs-sample",
        "peaks",
        "UNICORN peak table: peak count",
        lambda: unicorn_peak_count("unicorn-zip-fs-sample"),
    ),
    a.Fact(
        "itc-bitc-caii-050611a",
        "injections",
        "Origin integrated heats: rows",
        lambda: (len(itc_heats("itc-bitc-caii-050611a")["injection_volumes_ul"]), "Origin heats export"),
    ),
    a.Fact(
        "itc-bitc-caii-050611a",
        "cell_concentration_uM",
        "Origin integrated heats: Mt (mM) × 1000",
        lambda: (itc_heats("itc-bitc-caii-050611a")["cell_concentration_mM"] * 1000, "Origin heats export"),
    ),
    a.Fact(
        "itc-bitc-caii-050611a",
        "injection_volume_ul",
        "Origin integrated heats: INJV",
        lambda: (itc_heats("itc-bitc-caii-050611a")["injection_volumes_ul"][0], "Origin heats export"),
    ),
    a.Fact(
        "biacore-zenodo5011513-strep1",
        "chip",
        "Chip stream Name",
        lambda: (biacore_text("biacore-zenodo5011513-strep1", "Chip")["Name"], a.rd("olefile")),
    ),
    a.Fact(
        "biacore-zenodo5011513-strep1",
        "cycles",
        "_Cycle storages",
        lambda: biacore_cycles("biacore-zenodo5011513-strep1"),
    ),
    a.Fact(
        "biacore-allotropy-ed-fig1a-b2",
        "software_version",
        "Environment Version",
        lambda: (biacore_text("biacore-allotropy-ed-fig1a-b2", "Environment")["Version"], a.rd("olefile")),
    ),
    a.Fact(
        "scn-zenodo7977915-ncbp2",
        "exposure",
        "scan attribute Exposure Time (sec)",
        lambda: scn_attribute("scn-zenodo7977915-ncbp2", "Exposure Time (sec)"),
    ),
    a.Fact(
        "scn-zenodo7977915-ncbp2",
        "imager",
        "scan attribute Imager",
        lambda: scn_attribute("scn-zenodo7977915-ncbp2", "Imager"),
    ),
    a.Fact(
        "seahorse-zenodo10435506-mst-1",
        "port_a",
        "reagent of injection port A",
        lambda: seahorse_port_a("seahorse-zenodo10435506-mst-1"),
    ),
    a.Fact(
        "seahorse-zenodo10435506-mst-1",
        "measurements",
        "measurement cycles",
        lambda: seahorse_measurements("seahorse-zenodo10435506-mst-1"),
    ),
    a.Fact(
        "gpr-zenodo22128078-s1",
        "brightest_f532",
        "Name of the feature with the highest F532 Median",
        lambda: gpr_brightest("gpr-zenodo22128078-s1", "F532 Median"),
    ),
    a.Fact(
        "gpr-zenodo22128078-s1",
        "features",
        "rows of the feature table",
        lambda: (len(gpr_table("gpr-zenodo22128078-s1")), a.rd("pandas")),
    ),
    a.Fact(
        "jws-zenodo13347737-da-ba",
        "accumulations",
        "export footer Accumulation",
        lambda: jasco_footer("jws-zenodo13347737-da-ba", "Accumulation"),
    ),
    a.Fact(
        "jws-zenodo13347737-da-ba",
        "resolution",
        "export footer Resolution",
        lambda: jasco_footer("jws-zenodo13347737-da-ba", "Resolution"),
    ),
    a.Fact(
        "jws-zenodo13347737-da-ba",
        "accessory",
        "export footer Accessory",
        lambda: jasco_footer("jws-zenodo13347737-da-ba", "Accessory"),
    ),
    a.Fact(
        "xrd-zenodo15557974-car24014",
        "two_theta_strongest_above_10",
        "export [Scan points]: Angle of the largest Intensity above 10° 2θ",
        lambda: xrd_export_max("xrd-zenodo15557974-car24014-export", 10.0),
    ),
    a.Fact(
        "echem-yadg-cv",
        "max_current_ma",
        "EC-Lab export: largest <I>/mA",
        lambda: mpt_column_extreme("echem-yadg-cv-mpt", "<I>/mA", "max"),
    ),
    a.Fact(
        "ta-dscq20-data-079",
        "sample_size",
        "Universal Analysis export: Size",
        lambda: ua_export_header("ta-dscq20-data-079-export", "Size"),
    ),
    a.Fact(
        "echem-zenodo21631502-sintef-nda",
        "max_voltage",
        "NewareNDA: largest Voltage",
        lambda: neware_max_voltage("echem-zenodo21631502-sintef-nda"),
    ),
    a.Fact(
        "ngb-zenodo18902844-pla-fw",
        "sample_mass",
        "pyNGB metadata sample_mass",
        lambda: ngb_sample_mass("ngb-zenodo18902844-pla-fw"),
    ),
]


def compute() -> dict:
    out: dict = {}
    for f in FACTS:
        value, reader = f.compute()
        value = a.tidy(value)
        entry = out.setdefault(
            f.corpus_id,
            {"file": facts.manifest_file(f.corpus_id, f.role), "extractor": "evals/bench.py", "facts": {}},
        )
        entry["facts"][f.name] = {"value": value, "reader": reader, "how": f.how}
        print(f"{f.corpus_id} {f.name} = {value!r}", file=sys.stderr)
    return out


# ---------------------------------------------------------------- questions

BENCH_SPECS: list[g.Spec] = [
    g.Spec(
        "ana-bench-akta-peak-area",
        "unicorn-zip-fs-sample",
        "analysis",
        "This is an ÄKTA/UNICORN result export. UNICORN integrated the UV curve and stored a peak table. "
        "What is the area of the largest peak in that table?",
        lambda f, m: g.number(f["largest_peak_area"], None, rel=0.001),
        "facts-bench: largest_peak_area (UNICORN's own peak table in the export)",
        answer_hint="the area as UNICORN reports it (mAU·ml)",
    ),
    g.Spec(
        "bench-akta-peak-count",
        "unicorn-zip-fs-sample",
        "counts",
        "How many peaks are in the peak table UNICORN stored with this run?",
        lambda f, m: g.integer(f["peaks"]),
        "facts-bench: peaks (UNICORN's own peak table)",
    ),
    g.Spec(
        "bench-itc-injections",
        "itc-bitc-caii-050611a",
        "counts",
        "How many injections were made in this isothermal titration calorimetry run?",
        lambda f, m: g.integer(f["injections"]),
        "facts-bench: injections (Origin's integrated-heats export of the same run)",
    ),
    g.Spec(
        "bench-itc-cell-concentration",
        "itc-bitc-caii-050611a",
        "method",
        "What was the concentration of the macromolecule in the ITC cell, in micromolar?",
        lambda f, m: g.number(f["cell_concentration_uM"], "µM", rel=0.01),
        "facts-bench: cell_concentration_uM (Origin's heats export, Mt)",
        answer_hint="a concentration in µM",
    ),
    g.Spec(
        "bench-itc-injection-volume-ul",
        "itc-bitc-caii-050611a",
        "method",
        "What volume was injected per injection in this ITC run?",
        lambda f, m: g.number(f["injection_volume_ul"], None, rel=0.01),
        "facts-bench: injection_volume_ul (Origin's heats export, INJV)",
        answer_hint="a volume in µL",
    ),
    g.Spec(
        "bench-spr-chip",
        "biacore-zenodo5011513-strep1",
        "instrument",
        "Which type of sensor chip was docked for this Biacore run?",
        lambda f, m: g.string(f["chip"], [f"Series S Sensor Chip {f['chip']}", f"{f['chip']} chip"]),
        "facts-bench: chip (the file's Chip stream)",
        answer_hint="the chip type",
    ),
    g.Spec(
        "bench-spr-cycles",
        "biacore-zenodo5011513-strep1",
        "counts",
        "How many cycles does this Biacore run contain?",
        lambda f, m: g.integer(f["cycles"]),
        "facts-bench: cycles (_Cycle storages of the file)",
    ),
    g.Spec(
        "bench-spr-software",
        "biacore-allotropy-ed-fig1a-b2",
        "instrument",
        "Which version of the Biacore control software recorded this run?",
        lambda f, m: g.string(f["software_version"]),
        "facts-bench: software_version (the file's Environment stream)",
        answer_hint="a version number",
    ),
    g.Spec(
        "bench-gel-exposure",
        "scn-zenodo7977915-ncbp2",
        "method",
        "What exposure time was used to capture this blot image?",
        lambda f, m: g.number(float(f["exposure"]), "s", rel=0.01),
        "facts-bench: exposure (Image Lab's scan attributes)",
        answer_hint="a time with its unit",
    ),
    g.Spec(
        "bench-gel-imager",
        "scn-zenodo7977915-ncbp2",
        "instrument",
        "Which imager captured this blot?",
        lambda f, m: g.string(
            f["imager"], [f["imager"].replace("™", ""), "ChemiDoc MP"] if "ChemiDoc" in f["imager"] else None
        ),
        "facts-bench: imager (Image Lab's scan attributes)",
        answer_hint="the imager model",
    ),
    g.Spec(
        "bench-seahorse-port-a",
        "seahorse-zenodo10435506-mst-1",
        "method",
        "Which compound was loaded into injection port A for this Seahorse XF assay?",
        lambda f, m: g.string(f["port_a"], [f["port_a"].lower()]),
        "facts-bench: port_a (the assay XML's injection conditions)",
        answer_hint="the compound name",
    ),
    g.Spec(
        "bench-seahorse-measurements",
        "seahorse-zenodo10435506-mst-1",
        "counts",
        "How many measurement cycles did this Seahorse XF run have?",
        lambda f, m: g.integer(f["measurements"]),
        "facts-bench: measurements (rate spans, equal to the protocol's Measure commands)",
    ),
    g.Spec(
        "ana-bench-gpr-brightest-spot",
        "gpr-zenodo22128078-s1",
        "analysis",
        "In this protein microarray scan, which feature (spot name) has the highest median foreground "
        "intensity at 532 nm?",
        lambda f, m: g.string(f["brightest_f532"]),
        "facts-bench: brightest_f532 (pandas on the feature table)",
        answer_hint="the spot name as written in the file",
    ),
    g.Spec(
        "bench-gpr-features",
        "gpr-zenodo22128078-s1",
        "counts",
        "How many features (spots) does this GenePix results file list?",
        lambda f, m: g.integer(f["features"]),
        "facts-bench: features (pandas)",
    ),
    g.Spec(
        "bench-jws-accumulations",
        "jws-zenodo13347737-da-ba",
        "method",
        "How many scans were accumulated for this FT-IR spectrum?",
        lambda f, m: g.integer(int(leading_number(f["accumulations"]))),
        "facts-bench: accumulations (Spectra Manager's text export of the same measurement)",
    ),
    g.Spec(
        "bench-jws-resolution",
        "jws-zenodo13347737-da-ba",
        "method",
        "At what spectral resolution was this FT-IR spectrum measured?",
        lambda f, m: g.number(leading_number(f["resolution"]), "cm-1", abs_=0.01),
        "facts-bench: resolution (Spectra Manager's text export)",
        answer_hint="the resolution in cm-1",
    ),
    g.Spec(
        "ana-bench-xrd-strongest-reflection",
        "xrd-zenodo15557974-car24014",
        "analysis",
        "At which 2θ angle (degrees) is the strongest reflection (the highest-intensity peak) above 10° 2θ "
        "in this X-ray diffractogram?",
        lambda f, m: g.number(f["two_theta_strongest_above_10"], "°", abs_=0.02),
        "facts-bench: two_theta_strongest_above_10 (the diffractometer software's CSV export of the same scan)",
        answer_hint="2θ in degrees",
    ),
    g.Spec(
        "ana-bench-eclab-max-current",
        "echem-yadg-cv",
        "analysis",
        "What is the largest current, in mA, recorded in this cyclic voltammetry file?",
        lambda f, m: g.number(f["max_current_ma"], "mA", rel=0.001),
        "facts-bench: max_current_ma (EC-Lab's own .mpt export of the same run)",
        answer_hint="the current in mA",
    ),
    g.Spec(
        "bench-ta-sample-size",
        "ta-dscq20-data-079",
        "method",
        "What sample mass, in mg, was entered for this DSC run?",
        lambda f, m: g.number(float(f["sample_size"].split()[0]), "mg", rel=0.001),
        "facts-bench: sample_size (Universal Analysis export header)",
        answer_hint="the mass in mg",
    ),
    g.Spec(
        "ana-bench-neware-max-voltage",
        "echem-zenodo21631502-sintef-nda",
        "analysis",
        "What is the highest cell voltage, in volts, recorded in this battery-cycler file?",
        lambda f, m: g.number(f["max_voltage"], "V", abs_=0.0005),
        "facts-bench: max_voltage (NewareNDA, BSD-3, run as a third-party reader)",
        answer_hint="the voltage in V",
    ),
    g.Spec(
        "bench-ngb-sample-mass",
        "ngb-zenodo18902844-pla-fw",
        "method",
        "What was the sample mass, in mg, in this NETZSCH DSC measurement?",
        lambda f, m: g.number(f["sample_mass"], "mg", rel=0.001),
        "facts-bench: sample_mass (pyNGB, MIT, run as a third-party reader)",
        answer_hint="the mass in mg",
    ),
]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/bench.json is out of date")
    args = ap.parse_args()
    text = a.render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/bench.json is out of date; run evals/bench.py", file=sys.stderr)
            return 1
        print("evals/facts/bench.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
