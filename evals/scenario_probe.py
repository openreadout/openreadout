"""Can a straightforward OpenReadout command sequence answer each scenario? (No model is called.)

For every scenario question (`questions/scenario.jsonl`) this stages the folder the agent would get,
runs the few `openreadout` commands a scientist would reach for (listed per scenario below, the
last step being at most a little arithmetic on the JSON), and scores the result against the answer
from `facts/scenario.json` with the question's own scorer. A scenario that fails, or that needs
more than the listed commands (the `route` says what is missing), is a capability gap: it is a
finding for the tool, never an input to the answers, which come from third-party readers only.

    uv run python scenario_probe.py --binary ../target/release/openreadout [--ids a,b] [--keep DIR]
"""

from __future__ import annotations

import argparse
import csv
import json
import statistics
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run
import scenario_facts as sf
import score

D = sf.DIR


class Cli:
    def __init__(self, binary: Path, cwd: Path) -> None:
        self.binary, self.cwd = binary, cwd.resolve()

    def json(self, *args: str) -> dict:
        p = subprocess.run([str(self.binary), *args, "--json"], cwd=self.cwd, capture_output=True, text=True)
        env = json.loads(p.stdout)
        if not env.get("ok"):
            raise RuntimeError(f"{' '.join(args)}: {env.get('error')}")
        return env["data"]

    def jsonl(self, *args: str) -> list[dict]:
        p = subprocess.run([str(self.binary), *args, "--jsonl"], cwd=self.cwd, capture_output=True, text=True)
        return [json.loads(line) for line in p.stdout.splitlines() if line.strip()]

    def rows(self, *args: str) -> list[dict]:
        out = self.cwd / "_probe_rows.csv"
        subprocess.run([str(self.binary), *args, "-o", str(out), "--overwrite"], cwd=self.cwd, capture_output=True)
        with out.open() as fh:
            return list(csv.DictReader(fh))


def sheet(cwd: Path, name: str) -> list[dict]:
    with (cwd / D / name).open() as fh:
        return list(csv.DictReader(fh))


def gcms(c: Cli) -> tuple[object, str]:
    (c.cwd / "targets.csv").write_text("name,rt,window\nIS,6.30,0.1\nproduct,10.29,0.1\n")
    rows = c.rows("analyze", "peaks", D, "-r", "--targets", "targets.csv")  # the sheet in the folder is skipped
    area = {(r["path"].rsplit("/", 1)[-1], r["compound"]): float(r["area"]) for r in rows}
    times = {r["data_file"]: float(r["time_h"]) for r in sheet(c.cwd, "samples.csv")}
    ratio = {t: area[(f, "product")] / area[(f, "IS")] for f, t in times.items()}
    return (
        f"{min(t for t, v in ratio.items() if v > 1.0):g} h",
        "analyze peaks DIR -r --targets (IS, product) → ratio per run",
    )


def hplc(c: Cli) -> tuple[object, str]:
    (c.cwd / "targets.csv").write_text("name,rt,window\nanalyte,27.35,0.2\nIS,56.97,0.03\n")
    rows = c.rows("analyze", "peaks", f"{D}/*_ch2.cdf", "--targets", "targets.csv", "--pick", "nearest")
    area = {(Path(r["path"]).name.split("_")[0], r["compound"]): float(r["area"]) for r in rows}
    geno = {r["sample"]: r["genotype"] for r in sheet(c.cwd, "samples.csv")}
    ratio = {s: area[(s, "analyte")] / area[(s, "IS")] for s in geno}
    m = {g: statistics.mean(v for s, v in ratio.items() if geno[s] == g) for g in set(geno.values())}
    return m["MicroTom"] / m["della"], "analyze peaks --targets on channel-2 files (own integration) → genotype means"


def lcms(c: Cli) -> tuple[object, str]:
    seq = sheet(c.cwd, "sequence.csv")
    apex = {}
    for r in seq:
        d = c.json(
            "analyze",
            "chromatogram",
            f"{D}/{r['data_file']}",
            "--mz",
            "178.0510",
            "--ppm",
            "10",
            "--rt-range",
            "5.5-6.1",
        )
        apex[r["data_file"]] = d["chromatograms"][0]["apex_intensity"]
    qc = next(r["data_file"] for r in seq if r["sample_type"] == "pooled QC")
    worst = max(100 * apex[r["data_file"]] / apex[qc] for r in seq if r["sample_type"] == "blank")
    return worst, "analyze chromatogram --mz --rt-range per run → apex_intensity ratio"


def ics(c: Cli) -> tuple[object, str]:
    pct = {}
    for r in sheet(c.cwd, "tubes.csv"):
        d = c.json("analyze", "gate", f"{D}/{r['fcs_file']}", "--workspace", f"{D}/analysis.wsp")
        pop = {p["path"]: p for p in d["populations"]}
        pct[r["stimulation"]] = pop[sf.ICS_CD8 + "/IFNg+"]["percent_of_parent"]
    return max(
        v - pct["unstimulated"] for k, v in pct.items() if k != "unstimulated"
    ), "analyze gate --workspace per tube"


def rheobase(c: Cli) -> tuple[object, str]:
    vals = [
        c.json("analyze", "ephys-features", f"{D}/{r['file']}")["cell"]["rheobase_pa"]
        for r in sheet(c.cwd, "cells.csv")
        if r["cell_type"] == "pyramidal"
    ]
    return f"{statistics.mean(vals)} pA", "analyze ephys-features per cell → cell.rheobase_pa"


def ic50(c: Cli) -> tuple[object, str]:
    d = c.json("analyze", "assay", "dose-response", f"{D}/viability_plate.xlsx", "--layout", f"{D}/plate_map.csv")
    worst = max(d["compounds"], key=lambda x: x["ec50"])
    return f"{worst['ec50'] * 1000} nM", "analyze dose-response --layout → compounds[].ec50 (µM)"


def screen(c: Cli) -> tuple[object, str]:
    d = c.json("analyze", "assay", "wells", f"{D}/screen_plate_07.txt", "--normalize", "controls")
    comp = {r["sample_id"]: r["compound"] for r in sheet(c.cwd, "compounds.csv")}
    hits = [comp[w["sample"]] for w in d["wells"] if w["sample"] in comp and w["percent_effect"] < 10]
    return ", ".join(sorted(hits)), "analyze assay-wells --normalize controls (POSCON/NEGCON roles) → percent_effect"


def hcs_dapi(c: Cli) -> tuple[object, str]:
    d = c.json("stats", f"{D}/plate_2017_03", "--per", "well")
    m = {r["well"]: r["mean"] for r in d["rows"] if r["channel"] == "DAPI" and r.get("mean") is not None}
    return 100 * (m["C07"] - m["A01"]) / m["A01"], "stats --per well → DAPI means"


def hcs_missing(c: Cli) -> tuple[object, str]:
    d = c.json("stats", f"{D}/BSF018292-1A", "--per", "well")
    n_ch = len({r["channel"] for r in d["rows"]})
    per: dict[str, int] = {}
    for r in d["rows"]:
        per[r["well"]] = per.get(r["well"], 0) + (1 if r.get("planes", 0) > 0 else 0)
    return ", ".join(w for w, k in per.items() if 0 < k < n_ch), "stats --per well (rows per well × channel)"


def bleach(c: Cli) -> tuple[object, str]:
    drop = {}
    for name in ("run_a.nd2", "run_b.nd2", "run_c.nd2"):
        d = c.json("stats", f"{D}/{name}")
        planes = sorted(d["planes"], key=lambda p: p["t"])
        drop[name] = (planes[0]["stats"]["mean"] - planes[-1]["stats"]["mean"]) / planes[0]["stats"]["mean"]
    return max(drop, key=drop.get), "stats per file → planes[].stats.mean first vs last t"


def zarr(c: Cli) -> tuple[object, str]:
    for s in ("stack_a", "stack_b"):
        c.json("export", f"{D}/{s}.nd2", "--format", "ome-zarr", "-o", f"{D}/{s}.ome.zarr", "--overwrite")
    z = c.json("info", f"{D}/stack_b.nd2")["images"][0]["physical_size"]["z"]
    return f"{z} µm", "export --format ome-zarr ×2; info → images[0].physical_size.z"


def rdml(c: Cli) -> tuple[object, str]:
    d = c.json(
        "analyze", "qpcr", f"{D}/upr_qpcr.rdml", "--ddcq", "--control-sample", "28_1",
        "--reference-target", "ACT1", "--reference-target", "TFC1", "--target", sf.RDML_TARGET, "--max-records", "0",
    )  # fmt: skip
    dcq = {r["sample"]: r["delta_cq"] for r in d["relative_quantities"] if r["target"] == sf.RDML_TARGET}
    grp = {r["sample"]: r["group"] for r in sheet(c.cwd, "groups.csv")}
    m = {g: statistics.mean(v for s, v in dcq.items() if grp.get(s) == g) for g in ("control", "treated")}
    return 2 ** -(m["treated"] - m["control"]), "analyze qpcr --ddcq (per-sample ΔCq) → group means"


def eds(c: Cli) -> tuple[object, str]:
    d = c.json(
        "analyze", "qpcr", f"{D}/abhd17c_run.eds", "--ddcq", "--control-sample", sf.EDS_CONTROL,
        "--reference-target", sf.EDS_REF, "--target", sf.EDS_TARGET, "--max-records", "0",
    )  # fmt: skip
    rq = next(r["rq"] for r in d["relative_quantities"] if r["sample"] == sf.EDS_TREATED)
    return rq, "analyze qpcr --ddcq --control-sample → relative_quantities[].rq"


def bitumen(c: Cli) -> tuple[object, str]:
    index = {}
    for f in sorted((c.cwd / D).iterdir()):
        d = c.json("analyze", "peaks", f"{D}/{f.name}", "--x-range", "980:1060", "--x-range", "1350:1525")
        r = d["spectra"][0]["regions"]
        index[f.name.rsplit(".", 1)[0]] = r[0]["area"] / r[1]["area"]
    return max(index, key=index.get), "analyze peaks --x-range 980:1060 --x-range 1350:1525 → regions[].area ratio"


def damaged(c: Cli) -> tuple[object, str]:
    bad = [e["path"] for e in c.jsonl("check", D, "-r") if not (e.get("data") or {}).get("ok", False)]
    return ", ".join(bad), "check -r → files not ok"


def reconcile(c: Cli) -> tuple[object, str]:
    names = {
        (e.get("data") or {}).get("experiment", {}).get("sample", {}).get("id")
        for e in c.jsonl("info", D, "-r", "--skip-unknown")
    }
    missing = [r["sample_name"] for r in sheet(c.cwd, "expected_samples.csv") if r["sample_name"] not in names]
    return ", ".join(missing), "info -r → experiment.sample.id vs the sheet"


def gcfid(c: Cli) -> tuple[object, str]:
    (c.cwd / "targets.csv").write_text("name,rt,window\nanalyte,2.82,0.1\n")
    rows = c.rows("analyze", "peaks", D, "-r", "--skip-unknown", "--targets", "targets.csv", "--area-seconds")
    area = {r["path"].split("/")[1]: float(r["area"]) for r in rows}
    seq = sheet(c.cwd, "sequence.csv")
    std = next(r for r in seq if r["type"] == "standard")
    k = float(std["analyte_percent"]) / area[std["data_file"]]
    return statistics.mean(k * area[r["data_file"]] for r in seq if r["type"] == "sample"), "analyze peaks --targets"


def nmr(c: Cli) -> tuple[object, str]:
    regions = sheet(c.cwd, "regions.csv")
    args = [a for r in regions for a in ("--integrate", f"{r['from_ppm']}:{r['to_ppm']}")]
    d = c.json("analyze", "nmr-peaks", f"{D}/NF20190708/50", "--from", "fid", *args)
    vals = [i["value"] for i in d["integrals"]]
    return sum(2 * v / vals[0] for v in vals[1:]), "analyze nmr-peaks --from fid --integrate ×5"


PROBES: dict[str, Callable[[Cli], tuple[object, str]]] = {
    "scn-chrom-gcms-reaction-is-ratio": gcms,
    "scn-chrom-hplc-is-normalised-genotype": hplc,
    "scn-ms-lcms-hippurate-carryover": lcms,
    "scn-flow-ics-ifng-background-subtracted": ics,
    "scn-ephys-pyramidal-rheobase": rheobase,
    "scn-plate-least-potent-ic50": ic50,
    "scn-plate-screen-hits": screen,
    "scn-hcs-dapi-change-vs-dmso": hcs_dapi,
    "scn-hcs-wells-missing-channel": hcs_missing,
    "scn-mic-nd2-strongest-bleaching": bleach,
    "scn-mic-nd2-zstacks-to-ome-zarr": zarr,
    "scn-qpcr-rdml-hac1u-ddcq": rdml,
    "scn-qpcr-eds-lenvatinib-oe": eds,
    "scn-spec-bitumen-sulfoxide-index": bitumen,
    "scn-share-damaged-files": damaged,
    "scn-flow-sheet-missing-samples": reconcile,
    "scn-chrom-gcfid-external-standard": gcfid,
    "scn-nmr-aromatic-proton-count": nmr,
}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--binary")
    ap.add_argument("--ids")
    ap.add_argument("--keep", help="stage into this directory and keep it")
    a = ap.parse_args()
    binary = run.find_binary(a.binary)
    if binary is None:
        raise SystemExit("needs the openreadout binary (--binary or OPENREADOUT_BIN)")
    qs = [q for q in score.load_questions().values() if q["category"] == "scenario"]
    if a.ids:
        qs = [q for q in qs if q["id"] in a.ids.split(",")]
    corpus = run.corpus_dir()
    base = Path(a.keep) if a.keep else Path(tempfile.mkdtemp(prefix="openreadout-scn-"))
    ok = 0
    for q in qs:
        wd = base / q["id"]
        if not wd.exists():
            wd.mkdir(parents=True)
            run.stage(q, wd, corpus)
        cli = Cli(binary, wd)
        try:
            got, route = PROBES[q["id"]](cli)
            verdict = score.score_answer(q["answer"], f"ANSWER: {got}")
            if "task" in q:
                art = score.check_task(q, wd)
                verdict["correct"] = verdict["correct"] and art["ok"]
                route += f"; output check {'ok' if art['ok'] else 'FAILED'}"
            status = "ok" if verdict["correct"] else "WRONG"
        except NotImplementedError as exc:
            got, route, status = None, f"no command: {exc}", "GAP"
        except Exception as exc:  # a probe that breaks is a finding too
            got, route, status = None, f"{type(exc).__name__}: {exc}"[:300], "ERROR"
        ok += status == "ok"
        print(f"{status:5} {q['id']:42} got={str(got)[:60]!s:62} want={str(q['answer']['value'])[:40]}  [{route}]")
    print(f"{ok}/{len(qs)} scenarios answered by OpenReadout command sequences")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
