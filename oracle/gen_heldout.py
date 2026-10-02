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
       JWS2TXT_DIR=... python gen_heldout.py jws                         (JASCO .jws: jasco_oracle.py with the paired export and/or jws2txt)
       CHROMCONVERTER_LIB=... python gen_heldout.py lcd                  (Shimadzu .lcd without an export: shimadzu_oracle.py, chromConverter)
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
                  "netzsch-ngb")


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
        if e["format"] == "shimadzu" and e["id"] in exports:
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
        if e["format"] == "microcal-itc":
            subprocess.run([sys.executable, str(Path(__file__).with_name("itc_oracle.py")), "--out",
                            env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        if e["format"] in SERIES_FORMATS:
            args = [sys.executable, str(Path(__file__).with_name("series_oracle.py")), "--out", env["ORACLE_OUT"],
                    "--id", e["id"], "--format", e["format"], str(p)]
            if e["format"] == "bruker-bes3t":
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
            subprocess.run(args, check=True)
            continue
        if e["format"] in FPLC_FORMATS:
            subprocess.run([sys.executable, str(Path(__file__).with_name("fplc_oracle.py")), "--out",
                            env["ORACLE_OUT"], "--id", e["id"], str(p)], check=True)
            continue
        args = []
        if e["format"] in ("sciex-wiff", "agilent-masshunter", "waters-raw") and e["id"] in exports:
            args += ["--export", str(corpus / exports[e["id"]]["filename"])]
        args += ["--id", e["id"], str(p)]
        script = "qpcr.py" if e["format"] in QPCR_FORMATS else "gen.py"
        subprocess.run([sys.executable, str(Path(__file__).with_name(script)), *args], check=True, env=env)


if __name__ == "__main__":
    main()
