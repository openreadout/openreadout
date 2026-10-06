#!/usr/bin/env python
"""Write the held-out ground truth: gen.py for every held-out input, into corpus/oracle/heldout/.

Usage: uv run python gen_heldout.py [ID-SUBSTRING ...]          (every held-out input by default)
       uv run --group chrom python gen_heldout.py yucca 101f0101 dad1a   (ChemStation inputs: rainbow-api >= 1.5.2)
       uv run --group plate python gen_heldout.py bnext minikel barrick  (plate exports: allotropy)
       uv run --group qpcr python gen_heldout.py eds rex rdml           (qPCR inputs: qpcr.py, rdmlpython)
       uv run --group plate --group fplc python gen_heldout.py unicorn  (ÄKTA/UNICORN: fplc_oracle.py)
       python gen_heldout.py itc                                         (MicroCal ITC: itc_oracle.py, structure only)
       python gen_heldout.py gpr                                         (GenePix Results: gpr_oracle.py, pandas)
       ARBIN_PYTHON=... python gen_heldout.py arbin                      (Arbin .res: arbin_oracle.py, access_parser in its own venv)
       python gen_heldout.py seahorse                                    (Seahorse .asyr: seahorse_oracle.py, self-consistency)
       python gen_heldout.py epr xrd                                     (EPR, diffraction: series_oracle.py)
       python gen_heldout.py ta trios nda                                (TA files with a paired export, old .nda via neware_reader: series_oracle.py)
       JWS2TXT_DIR=... python gen_heldout.py jws                         (JASCO .jws: jasco_oracle.py with the paired export and/or jws2txt)
       CHROMCONVERTER_LIB=... python gen_heldout.py lcd                  (Shimadzu .lcd without an export: shimadzu_oracle.py, chromConverter)
       WITEC_PYTHON=... python gen_heldout.py wip                        (WITec Project: witec_oracle.py, witio in its own venv)
       python gen_heldout.py spinsolve                                   (Magritek Spinsolve directories: spinsolve.py)
Env:   OPENREADOUT_CORPUS_DIR (default ../corpus/files)

Held-out inputs are the manifest entries with `role = "heldout"` (docs/benchmark/heldout.md). They
keep their original file names, so each is addressed by id (`gen.py --id`); a plate export's dialect
comes from its `plate_vendor` field (PLATE_VENDOR for oracle/plate.py), a Sciex/Agilent/Waters
input gets its paired depositor export (`--export`), a qPCR input goes to oracle/qpcr.py, and a
Shimadzu `.lcd` input with a paired LabSolutions ASCII export gets its ground truth from that export
(`labsolutions_export`: chromatogram sizes and the vendor's peak tables, standard library only).
"""
import os, subprocess, sys, tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
QPCR_FORMATS = ("rdml", "applied-biosystems-eds", "rotor-gene-rex")
FPLC_FORMATS = ("cytiva-unicorn-res", "cytiva-unicorn-zip")
# series_oracle.py: DeerLab's deerload on BES3T, geddes on diffraction files, paired .xy exports
SERIES_FORMATS = ("bruker-bes3t", "panalytical-xrdml", "bruker-raw", "bruker-brml", "rigaku-ras", "rigaku-rasx",
                  "biologic-mpr", "biologic-mpt", "gamry-dta", "neware-nda", "neware-ndax",
                  "netzsch-ngb", "ta-universal-analysis", "ta-trios")
# Held-out inputs the default series reader refuses, and what reads them instead:
# - a RAS file whose MEAS_DATA_COUNT is a decimal (geddes refuses it): a standard-library reading of
#   its own data block, recorded as a second implementation (not independent);
# - a version 8 Neware .nda (NewareNDA refuses it): neware_reader (BSD-2-Clause, black box).
SELF_PARSE_RAS = {"ho-zenodo14997537-xrd-ras-tmfeo3"}
NEWARE_READER = {"ho-gh-fthuld-neware-nda-old"}


from shimadzu_export import labsolutions_export  # noqa: E402  (shared with the development oracles)
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB)


def write_export_oracle(e: dict, p: Path, export: Path) -> None:
    import hashlib
    import json

    data = {"id": e["id"], "file": p.name, "size": p.stat().st_size,
            "sha256": hashlib.sha256(p.read_bytes()).hexdigest(), **labsolutions_export(export)}
    out = ROOT / "corpus" / "oracle" / "heldout" / f"{e['id']}.json"
    out = oracle_json.write_text(out, json.dumps(data, indent=1) + "\n")
    print("wrote", out.name, f"{len(data['shimadzu_export']['chromatograms'])} chromatograms (vendor export)")


# An mzXML whose <scanOrigin num="…"/> elements pyteomics mistakes for scans (Shimadzu's own
# converter writes them): pyteomics reads a copy without them and without the index.
MZXML_SCANORIGIN = {"ho-zenodo17020981-mzxml-qc"}


def tdf_oracle(e: dict, p: Path) -> None:
    """A timsTOF .d: oracle/timsrust-oracle (timsrust, black box) on a copy of the directory, as for
    the development files. TIMSRUST_ORACLE names the built binary
    (`cargo build --release --manifest-path oracle/timsrust-oracle/Cargo.toml`)."""
    import json
    import shutil
    import tempfile

    exe = os.environ.get("TIMSRUST_ORACLE")
    if not exe:
        print(e["id"], ": skipped (set TIMSRUST_ORACLE to the built oracle/timsrust-oracle binary)")
        return
    d = p if p.is_dir() else p.parent
    tmp = Path(tempfile.mkdtemp(prefix="openreadout-heldout-tdf-"))
    shutil.copytree(d, tmp / d.name)
    out = ROOT / "corpus" / "oracle" / "heldout" / f"{e['id']}.json"
    subprocess.run([exe, str(tmp / d.name), e["id"], str(tmp / "out.json")], check=True)
    out = oracle_json.write_text(out, json.dumps(json.loads((tmp / "out.json").read_text()), indent=1) + "\n")
    shutil.rmtree(tmp)
    print("wrote", out.name, "(timsrust-oracle)")


def headerless_arw_oracle(e: dict, p: Path) -> None:
    """An Empower .arw export without its two quoted header rows (time and value only): no
    third-party Empower reader takes it, so the trace is the text itself read with NumPy, in the
    shape of oracle/empower_arw_oracle.py's records."""
    import hashlib
    import json

    import numpy as np

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import gen

    text = p.read_bytes().decode("latin-1").replace("\r\n", "\n").replace("\r", "\n")
    a = np.loadtxt(text.splitlines(), delimiter="\t", ndmin=2)
    trace = gen._chrom_trace(0, ["value"], [""], a[:, 0], [a[:, 1]])
    data = {"id": e["id"], "file": p.name, "size": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
            "reader": f"numpy {np.__version__} loadtxt on the export text",
            "oracle_note": "headerless export (no SampleName/Channel rows): rows of time (min) and value only",
            "traces": [trace]}
    out = oracle_json.write_text(ROOT / "corpus" / "oracle" / "heldout" / f"{e['id']}.json", json.dumps(data, indent=1) + "\n")
    print("wrote", out.name, f"{len(a)} points (numpy)")


def mzxml_without_scanorigin(e: dict, p: Path, env: dict) -> None:
    import json
    import re
    import shutil
    import tempfile

    tmp = Path(tempfile.mkdtemp(prefix="openreadout-heldout-mzxml-"))
    t = re.sub(rb"\s*<scanOrigin [^>]*/>", b"", p.read_bytes())
    t = re.sub(rb"<index .*?</index>\s*<indexOffset>.*?</indexOffset>", b"", t, flags=re.S)
    (tmp / p.name).write_bytes(t)
    subprocess.run([sys.executable, str(Path(__file__).with_name("gen.py")), "--id", e["id"], str(tmp / p.name)],
                   check=True, env=env)
    out = ROOT / "corpus" / "oracle" / "heldout" / f"{e['id']}.json"
    data = oracle_json.load(out)
    import hashlib

    data["size"], data["sha256"] = p.stat().st_size, hashlib.sha256(p.read_bytes()).hexdigest()
    data["oracle_note"] = ("pyteomics read a copy without the <scanOrigin> elements and the scan index "
                           "(it takes their num attributes for scans); the peaks are untouched")
    oracle_json.write_text(out, json.dumps(data, indent=1, default=str))
    shutil.rmtree(tmp)


def main():
    only = sys.argv[1:]
    corpus = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
    manifest = tomllib.loads((ROOT / "corpus" / "manifest.toml").read_text())["file"]
    exports = {e["id"]: e for e in manifest if e.get("tier") == "heldout" and e.get("role") == "oracle-export"}
    for e in manifest:
        if e.get("role") != "heldout" or (only and not any(o in e["id"] for o in only)):
            continue
        p = corpus / e["filename"]
        if not p.exists():
            print("missing", e["id"])
            continue
        env = {**os.environ, "ORACLE_OUT": str(ROOT / "corpus" / "oracle" / "heldout")}
        env.pop("PLATE_VENDOR", None)
        if e.get("plate_vendor"):
            env["PLATE_VENDOR"] = e["plate_vendor"]
        if e["format"] == "bruker-tdf":
            tdf_oracle(e, p)
            continue
        if e["format"] == "chromeleon":
            # the development oracle needs the vendor's ASCII exports or PDF report of the same injections
            print(e["id"], ": no oracle (no Chromeleon export or report of this archive)")
            continue
        if e["format"] == "empower-arw" and not p.read_bytes()[:1] == b'"':
            headerless_arw_oracle(e, p)
            continue
        if e["id"] in MZXML_SCANORIGIN:
            mzxml_without_scanorigin(e, p, env)
            continue
        if e["format"] == "shimadzu" and e["id"] in exports and exports[e["id"]]["filename"].lower().endswith(".txt"):
            write_export_oracle(e, p, corpus / exports[e["id"]]["filename"])
            continue
        if e["format"] == "shimadzu":
            # no vendor export: chromConverter (GPL, black box; CHROMCONVERTER_LIB) as for development files
            subprocess.run([sys.executable, str(Path(__file__).with_name("shimadzu_oracle.py")), "--out",
                            env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True, env=env)
            continue
        if e["format"] == "agilent-seahorse-asyr":
            subprocess.run([sys.executable, str(Path(__file__).with_name("seahorse_oracle.py")), "--out",
                            env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        if e["format"] == "jasco-jws":
            args = [sys.executable, str(Path(__file__).with_name("jasco_oracle.py")), "--out", env["ORACLE_OUT"], "--id", e["id"]]
            if e["id"] in exports:
                args += ["--export", str(corpus / exports[e["id"]]["filename"])]
            if os.environ.get("JWS2TXT_DIR"):  # jws2txt (MIT) helpers.py: the second opinion for compound files
                args += ["--jws2txt", os.environ["JWS2TXT_DIR"]]
            subprocess.run(args + [str(p)], check=True)
            continue
        if e["format"] in ("cytiva-biacore-blr", "cytiva-biacore-bme"):
            subprocess.run([sys.executable, str(Path(__file__).with_name("biacore_oracle.py")), "--out",
                            env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        if e["format"] == "arbin-res":
            # access_parser (Apache-2.0) lives in its own venv: ARBIN_PYTHON names its interpreter
            subprocess.run([os.environ.get("ARBIN_PYTHON", sys.executable), str(Path(__file__).with_name("arbin_oracle.py")),
                            "--out", env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        if e["format"] == "genepix-gpr":
            subprocess.run([sys.executable, str(Path(__file__).with_name("gpr_oracle.py")), "--out",
                            env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        if e["format"] == "witec-project":
            # witio (MIT-0) lives in its own venv, as for the development files: WITEC_PYTHON names its interpreter
            subprocess.run([os.environ.get("WITEC_PYTHON", sys.executable), str(Path(__file__).with_name("witec_oracle.py")),
                            "--out", env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        if e["format"] == "magritek-spinsolve":
            # the development oracle (spinsolve.py: nmrglue parameters and header, documented layouts)
            import json
            from spinsolve import spinsolve_oracle

            d = p if p.is_dir() else p.parent
            data = {"id": e["id"], "file": d.name, "size": sum(f.stat().st_size for f in d.rglob("*") if f.is_file()),
                    **spinsolve_oracle(d)}
            out = oracle_json.write_text(ROOT / "corpus" / "oracle" / "heldout" / f"{e['id']}.json",
                                         json.dumps(data, indent=1) + "\n")
            print("wrote", out.name, f"{len(data['traces'])} traces (spinsolve.py)")
            continue
        if e["format"] == "microcal-itc":
            args = [sys.executable, str(Path(__file__).with_name("itc_oracle.py")), "--out", env["ORACLE_OUT"],
                    "--id", e["id"], str(p)]
            # concentrations the depositor wrote into the file name (`…_120uM_cell_…_478uM_syr…`)
            import re

            m = re.search(r"_(\d+(?:\.\d+)?)uM_cell_.*?_(\d+(?:\.\d+)?)(?:uM)?_sy", p.name)
            if m:
                args += ["--fact", f"cell_concentration_mM={float(m.group(1)) / 1000}",
                         "--fact", f"syringe_concentration_mM={float(m.group(2)) / 1000}",
                         "--fact-source", f"the depositor's file name ({p.name})"]
            subprocess.run(args, check=True)
            continue
        if e["format"] == "sartorius-octet-frd":
            # pykingenie (MIT, black box; needs `pip install pykingenie`), as for the development files
            r = subprocess.run([sys.executable, str(Path(__file__).with_name("octet_oracle.py")), "--out",
                                env["ORACLE_OUT"], "--id", e["id"], str(p)])
            if r.returncode:
                print(e["id"], ": no oracle (pykingenie cannot read this file)")
            continue
        if e["format"] == "ta-trios" and e["id"] not in exports:
            print(e["id"], ": no oracle (no TRIOS export of this run)")
            continue
        if e["format"] in SERIES_FORMATS:
            args = [sys.executable, str(Path(__file__).with_name("series_oracle.py")), "--out", env["ORACLE_OUT"],
                    "--id", e["id"], "--format", e["format"], str(p)]
            own = exports.get(e["id"])  # a depositor export with the input's own id
            if e["id"] in SELF_PARSE_RAS:
                args += ["--export", str(p), "--skip-until", "*RAS_INT_START", "--xcol", "0", "--ycol", "1",
                         "--x-tol", "1e-9", "--second-implementation"]
            elif e["id"] in NEWARE_READER:
                args += ["--neware-reader"]
            elif e["format"] == "ta-universal-analysis":
                args += ["--ta-export", str(corpus / own["filename"])]
            elif e["format"] == "ta-trios":
                args += ["--trios-export", str(corpus / own["filename"])]
            elif e["format"] == "bruker-bes3t":
                args += ["--deerload"]
            elif e["format"] == "biologic-mpr":
                args += ["--galvani", "--start"]
            elif e["format"] == "netzsch-ngb":
                args += ["--pyngb"]
            elif e["format"] in ("neware-nda", "neware-ndax"):
                args += ["--newarenda", "--start"]
            elif e["format"] == "gamry-dta":
                args += ["--gamry"]
            elif e["format"] == "biologic-mpt":
                args += ["--mpt", str(p), "--second-implementation"]
            else:
                args += ["--geddes"]
            for x in exports.values():
                if x["id"].startswith(e["id"] + "-") and x["filename"].lower().endswith(".xy"):
                    args += ["--export", str(corpus / x["filename"]), "--xcol", "0", "--ycol", "1", "--x-tol", "1e-6"]
            if own and own["filename"].lower().endswith(".uxd"):  # a Bruker UXD conversion of a RAW file
                args += ["--export", str(corpus / own["filename"]), "--skip-until", "_2THETACOUNTS", "--xcol", "0",
                         "--ycol", "1", "--x-tol", "1e-6"]
            subprocess.run(args, check=True)
            continue
        if e["format"] in FPLC_FORMATS:
            subprocess.run([sys.executable, str(Path(__file__).with_name("fplc_oracle.py")), "--out",
                            env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        if e["format"] == "spikeglx" and p.is_file() and "\nfileName=" not in "\n" + p.with_suffix(".meta").read_text(errors="replace"):
            # Neo's SpikeGLXRawIO needs the .meta's `fileName` key, which some SpikeGLX writers leave out:
            # run gen.py on copies whose .meta gets `fileName=<the .bin's name>` appended (the .bin is unchanged)
            import json
            import shutil
            import tempfile

            tmp = Path(tempfile.mkdtemp(prefix="openreadout-heldout-sglx-"))
            shutil.copy(p, tmp)
            meta = p.with_suffix(".meta").read_text(errors="replace")
            (tmp / p.with_suffix(".meta").name).write_text(meta.rstrip("\n") + f"\nfileName={p.name}\n")
            subprocess.run([sys.executable, str(Path(__file__).with_name("gen.py")), "--id", e["id"], str(tmp / p.name)],
                           check=True, env=env)
            out = ROOT / "corpus" / "oracle" / "heldout" / f"{e['id']}.json"
            data = oracle_json.load(out)
            data["oracle_note"] = "the .meta has no fileName key; Neo read a copy with fileName appended"
            oracle_json.write_text(out, json.dumps(data, indent=1, default=str))
            shutil.rmtree(tmp)
            continue
        if e["format"] == "heka-patchmaster":
            # gen.py (load-heka-python); when it stops partway through a bundle (an assertion on
            # channel kinds it has not been tested on), fall back to gen.py's pyHEKA oracle (AGPL, black box)
            import json

            subprocess.run([sys.executable, str(Path(__file__).with_name("gen.py")), "--id", e["id"], str(p)],
                           check=True, env=env)
            out = ROOT / "corpus" / "oracle" / "heldout" / f"{e['id']}.json"
            data = oracle_json.load(out)
            if "error" in data:
                os.environ["ORACLE_OUT"] = env["ORACLE_OUT"]
                import gen

                fallback = gen._heka_pyheka(p, f"load-heka-python failed: {data['error']}")
                data = {k: data[k] for k in ("id", "file", "size", "sha256", "max_planes_hashed")} | fallback
                oracle_json.write_text(out, json.dumps(data, indent=1, default=str))
                print("wrote", out.name, f"{len(data['traces'])} traces (pyHEKA fallback)")
            continue
        args = []
        if e["format"] in ("sciex-wiff", "agilent-masshunter", "waters-raw") and e["id"] in exports:
            args += ["--export", str(corpus / exports[e["id"]]["filename"])]
        args += ["--id", e["id"], str(p)]
        script = "qpcr.py" if e["format"] in QPCR_FORMATS else "gen.py"
        subprocess.run([sys.executable, str(Path(__file__).with_name(script)), *args], check=True, env=env)


if __name__ == "__main__":
    main()
