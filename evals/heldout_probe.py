"""Run OpenReadout over every held-out file and compare it with the held-out oracles. No model calls.

The generalization measurement of docs/benchmark/heldout.md: for each held-out input
(`role = "heldout"` in corpus/manifest.toml) run `info --view format`, `info`, `check` and
`info --view full` (all `--json`; recorded as `detect`, `info`, `check`, `dump`), record exit codes,
error codes and hints, wall time, whether `info --view format` names the manifest's format,
and compare `info` with `corpus/oracle/heldout/<id>.json` field by field (image count and geometry,
pixel type, physical sizes, channel names; trace count, sweeps, samples, sample rate; table count
and rows; spectra scan count). The corpus test `heldout_agreement_is_recorded` adds the deep
comparison (plane hashes, trace values, every scan); pass its `HELDOUT_REPORT` JSON lines with
`--corpus-test` to merge them.

    uv run python heldout_probe.py --binary ../target/release/openreadout \\
        [--corpus-test /tmp/heldout-corpus.jsonl] [-o results/heldout-probe.json]

It never reads a held-out file for development: it only runs the finished binary and records
what happens (the point is to measure; fixes are developed on other files).

An exposed input (manifest `exposed`: its source record was developed on before a draw reserved it)
is probed like any other; its row carries `exposed` and the closing lines list it apart, so reports
built on this JSON can leave it out of the generalization numbers (heldout_summary.py does).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import time
import tomllib
from pathlib import Path

EVALS = Path(__file__).resolve().parent
ROOT = EVALS.parent
sys.path.append(str(ROOT / "oracle"))
import oracle_json  # noqa: E402  (oracle/oracle_json.py: <id>.json, or <id>.json.gz over 1 MiB)

MANIFEST = ROOT / "corpus" / "manifest.toml"
ORACLE_DIR = ROOT / "corpus" / "oracle" / "heldout"
TIMEOUT_S = 600


def corpus_dir() -> Path:
    env = os.environ.get("OPENREADOUT_CORPUS_DIR")
    if env:
        return Path(env)
    for parent in [ROOT, *ROOT.parents]:
        cand = parent / "corpus" / "files"
        if cand.exists():
            return cand
    raise SystemExit("corpus files not found: set OPENREADOUT_CORPUS_DIR")


# What each probe runs: `openreadout <cmd[0]> FILE <cmd[1:]> --json`.
PROBES = {
    "detect": ["info", "--view", "format"],
    "info": ["info"],
    "check": ["check"],
    "dump": ["info", "--view", "full", "--vendor"],
}


def run(binary: Path, cmd: list[str], path: Path) -> dict:
    t0 = time.monotonic()
    try:
        p = subprocess.run(
            [str(binary), cmd[0], str(path), *cmd[1:], "--json"], capture_output=True, text=True, timeout=TIMEOUT_S
        )
    except subprocess.TimeoutExpired:
        return {"exit": None, "wall_s": TIMEOUT_S, "error": f"timeout after {TIMEOUT_S} s"}
    out: dict = {"exit": p.returncode, "wall_s": round(time.monotonic() - t0, 3)}
    if "panicked" in p.stderr or p.returncode == 101:
        out["panic"] = p.stderr.strip().splitlines()[-1][:300] if p.stderr.strip() else "panic"
    try:
        env = json.loads(p.stdout)
    except json.JSONDecodeError:
        out["error"] = (p.stderr.strip() or p.stdout.strip())[:300]
        return out
    if not env.get("ok", True) or "error" in env:
        err = env.get("error") or {}
        out["error"] = f"{err.get('code', '')}: {err.get('message', '')}"[:300]
        if err.get("hint"):
            out["hint"] = err["hint"][:300]
    out["data"] = env.get("data")
    return out


def close(a, b, rel: float = 1e-3) -> bool:
    if a is None or b is None:
        return a is None and b is None
    return math.isclose(float(a), float(b), rel_tol=rel, abs_tol=1e-9)


def text_or_none(v):
    """Empty or whitespace-only text is 'not recorded' (book/src/guides/metadata.md)."""
    if isinstance(v, str) and not v.strip():
        return None
    return v


def compare(info: dict, oracle: dict) -> tuple[list[str], list[str]]:
    """(fields that agree, disagreements) between `info` and the oracle."""
    ok: list[str] = []
    bad: list[str] = []

    def cmp(name: str, ours, theirs, eq=None) -> None:
        if theirs is None:
            return
        same = eq(ours, theirs) if eq else ours == theirs
        (ok if same else bad).append(name if same else f"{name}: ours {ours!r} != oracle {theirs!r}")

    o_imgs = [i for i in oracle.get("images", []) if not i.get("skip")]
    if o_imgs:
        imgs = info.get("images", [])
        cmp("image count", len(imgs), len(oracle.get("images", [])))
        for oi in o_imgs:
            k = oi.get("index", 0)
            if k >= len(imgs):
                continue
            im = imgs[k]
            for f in ("size_x", "size_y", "size_z", "size_c", "size_t"):
                cmp(f"image {k} {f}", im.get(f), oi.get(f))
            cmp(f"image {k} pixel_type", im.get("pixel_type"), oi.get("pixel_type"))
            ps, ops = im.get("physical_size") or {}, oi.get("physical_size_um") or {}
            for ax in ("x", "y", "z"):
                theirs = ops.get(ax)
                if theirs in (None, 0, 0.0) or (ax == "z" and (oi.get("size_z") or 1) <= 1):
                    continue
                cmp(f"image {k} physical_size.{ax}", ps.get(ax), theirs, close)
            if oi.get("channel_names"):
                # book/src/guides/metadata.md: text the file stores empty is omitted, so an
                # oracle's "" and our missing name are the same value.
                cmp(
                    f"image {k} channel names",
                    [text_or_none(c.get("name")) for c in im.get("channels", [])],
                    [text_or_none(n) for n in oi["channel_names"]],
                )
    series = bool(oracle.get("traces")) and all("trace" in ot and "samples" in ot for ot in oracle["traces"])
    if series:
        # oracle/series_oracle.py writes one entry per channel (or per reader) of a trace, with
        # its index in `trace`; the corpus test compares their samples
        cmp("trace count", len(info.get("traces", [])), len({ot["trace"] for ot in oracle["traces"]}))
    elif oracle.get("traces"):
        tr = info.get("traces", [])
        cmp("trace count", len(tr), len(oracle["traces"]))
        for k, ot in enumerate(oracle["traces"]):
            if k >= len(tr) or ot.get("error"):
                continue
            t = tr[k]
            cmp(f"trace {k} sweeps", t.get("sweep_count"), ot.get("sweep_count"))
            cmp(f"trace {k} samples", t.get("sample_count"), ot.get("sample_count"))
            tol = ot.get("sample_rate_tolerance_hz")
            cmp(
                f"trace {k} sample rate",
                t.get("sample_rate_hz"),
                ot.get("sample_rate_hz"),
                (lambda a, b, tol=tol: a is not None and abs(a - b) <= tol) if tol else close,
            )
    if oracle.get("tables"):
        tb = info.get("tables", [])
        cmp("table count", len(tb), len(oracle["tables"]))
        for k, ot in enumerate(oracle["tables"]):
            if k < len(tb):
                cmp(f"table {k} rows", tb[k].get("rows"), ot.get("rows"))
    if oracle.get("spectra"):
        sp = info.get("spectra") or []
        cmp("scan count", sum(r.get("scan_count", 0) for r in sp), oracle["spectra"].get("scan_count"))
    if oracle.get("shimadzu_export"):
        # a LabSolutions ASCII export of the same run: its chromatograms' sizes and clocks
        chroms = oracle["shimadzu_export"]["chromatograms"]
        tr = [t for t in info.get("traces", []) if (t.get("sample_rate_hz") or 0) > 0]
        cmp("detector chromatograms", len(tr), len(chroms))
        cmp("chromatogram points", sorted(t.get("sample_count") for t in tr), sorted(c["points"] for c in chroms))
        cmp(
            "chromatogram sampling (Hz)",
            sorted(round(t.get("sample_rate_hz") or 0, 6) for t in tr),
            sorted(round(1000.0 / c["interval_ms"], 6) for c in chroms),
        )
    if oracle.get("chromatograms") is not None and oracle.get("chromatograms"):
        cmp("chromatogram traces", len(info.get("traces", [])), len(oracle["chromatograms"]))
    return ok, bad


QPCR_FORMATS = ("rdml", "applied-biosystems-eds", "rotor-gene-rex")


def run_args(binary: Path, args: list[str]) -> dict | None:
    """`openreadout ARGS --json` → the envelope's data (None on failure)."""
    try:
        p = subprocess.run([str(binary), *args, "--json"], capture_output=True, text=True, timeout=TIMEOUT_S)
        env = json.loads(p.stdout)
    except (subprocess.TimeoutExpired, json.JSONDecodeError):
        return None
    return env.get("data") if env.get("ok", True) else None


def qpcr_table(binary: Path, path: Path, info: dict, name: str) -> list[dict]:
    """Rows of the qPCR table `name` (`results`, `amplification`) as dicts, category codes decoded."""
    tables = info.get("tables") or []
    k = next((i for i, t in enumerate(tables) if t.get("name") == name), None)
    if k is None:
        return []
    cats = {c["name"]: (c.get("extra") or {}).get("categories") for c in tables[k]["columns"]}
    data = run_args(binary, ["table", str(path), "--table", str(k), "--max-rows", "1000000"]) or {}
    out = []
    for row in data.get("rows", []):
        d = dict(zip(data["columns"], row, strict=False))
        for c, cs in cats.items():
            if cs and d.get(c) is not None:
                d[c] = cs[int(d[c])]
        out.append(d)
    return out


def compare_qpcr(binary: Path, path: Path, info: dict, oracle: dict) -> tuple[list[str], list[str]]:
    """Vendor Cq per (well, target) (RDML/.eds) or the raw cycling readings (.rex) against the oracle."""
    ok: list[str] = []
    bad: list[str] = []
    if "records" in oracle:
        rows = qpcr_table(binary, path, info, "results")
        ours = {(r.get("well"), r.get("target")): r.get("cq") for r in rows}
        cycles = oracle.get("cycles")
        agree = differ = missing = 0
        examples = []
        for rec in oracle["records"]:
            want = rec.get("cq")
            if want is None or rec.get("cq_undetermined") or (cycles and want >= cycles):
                want = None
            well = rec.get("well") or f"{chr(64 + rec['row'])}{rec['col']}"
            key = (well, rec.get("target"))
            if key not in ours:
                missing += 1
                continue
            got = ours[key]
            got = None if got is None or (isinstance(got, float) and math.isnan(got)) else got
            if (want is None and got is None) or (want is not None and got is not None and abs(got - want) <= 1e-3):
                agree += 1
            else:
                differ += 1
                if len(examples) < 3:
                    examples.append(f"{key}: ours {got} != vendor {want}")
        line = f"vendor Cq: {agree} of {len(oracle['records'])} (well, target) agree"
        if differ or missing:
            bad.append(f"{line}; {differ} differ, {missing} absent ({'; '.join(examples)})")
        else:
            ok.append(line)
    elif oracle.get("channels"):
        rows = qpcr_table(binary, path, info, "amplification")
        want_n = sum(r["n"] for r in oracle["channels"][0]["readings"])
        want_sum = sum(r["sum"] for r in oracle["channels"][0]["readings"])
        vals = [r["fluorescence"] for r in rows if r.get("fluorescence") is not None]
        same = len(vals) == want_n and close(sum(vals), want_sum, 1e-6)
        name = oracle["channels"][0]["name"]
        msg = f"{name}: {len(vals)} readings sum {sum(vals):.6g} vs oracle {want_n} sum {want_sum:.6g}"
        (ok if same else bad).append(msg)
    return ok, bad


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--binary", default=os.environ.get("OPENREADOUT_BIN", str(ROOT / "target/release/openreadout")))
    ap.add_argument("--corpus-test", help="HELDOUT_REPORT JSON lines from heldout_agreement_is_recorded")
    ap.add_argument("-o", "--out", default=str(EVALS / "results" / "heldout-probe.json"))
    args = ap.parse_args()
    binary = Path(args.binary)
    deep = {}
    if args.corpus_test:
        for line in Path(args.corpus_test).read_text().splitlines():
            if line.strip():
                r = json.loads(line)
                deep[r["id"]] = {"status": r["status"], "detail": r["detail"]}
    with MANIFEST.open("rb") as fh:
        entries = [e for e in tomllib.load(fh)["file"] if e.get("role") == "heldout"]
    corpus = corpus_dir()
    rows = []
    for e in entries:
        path = corpus / e["filename"]
        row: dict = {"id": e["id"], "format": e["format"], "family": e.get("family", ""), "filename": e["filename"]}
        if e.get("exposed"):
            row["exposed"] = e["exposed"]
        if not path.exists():
            row["status"] = "absent"
            rows.append(row)
            print(f"absent   {e['id']}")
            continue
        res = {c: run(binary, cmd, path) for c, cmd in PROBES.items()}
        det = (res["detect"].get("data") or {}).get("format")
        row["detected"] = det
        row["detect_ok"] = det == e["format"]
        for c, r in res.items():
            row[c] = {k: v for k, v in r.items() if k != "data"}
        info = res["info"].get("data") or {}
        # the per-file assurance signal (docs/assurance.md): what `--strict` would refuse
        a = info.get("assurance") or (res["check"].get("data") or {}).get("assurance")
        if a:
            row["assurance"] = {
                "level": a.get("level"),
                "strict_refuses": a.get("strict_refuses") or [],
                "reader_confidence": a.get("reader_confidence"),
                "fingerprint": a.get("fingerprint"),
                "reasons": (a.get("reasons") or [])[:4],
            }
        if (res["check"].get("data") or {}).get("findings"):
            row["check_findings"] = res["check"]["data"]["findings"][:5]
        oracle_path = ORACLE_DIR / f"{e['id']}.json"
        if oracle_json.exists(oracle_path):
            oracle = oracle_json.load(oracle_path)
            if oracle.get("error"):
                row["oracle"] = f"oracle error: {oracle['error'][:200]}"
            elif info and e["format"] in QPCR_FORMATS:
                agree, disagree = compare_qpcr(binary, path, info, oracle)
                row["agree"] = agree
                row["disagree"] = disagree
            elif info:
                agree, disagree = compare(info, oracle)
                row["agree"] = agree
                row["disagree"] = disagree
        else:
            row["oracle"] = "no oracle"
        if e["id"] in deep:
            row["corpus_test"] = deep[e["id"]]
        panics = [c for c, r in res.items() if "panic" in r]
        if panics:
            row["status"] = "panic"
        elif res["info"]["exit"] != 0 or not row["detect_ok"]:
            row["status"] = "fails"
        elif row.get("disagree") or (row.get("corpus_test", {}).get("status") in ("FAIL", "PANIC")):
            row["status"] = "disagrees"
        elif "partial copy" in e.get("notes", "") and res["check"]["exit"] == 4 and res["dump"]["exit"] == 0:
            row["status"] = "clean (partial copy: check rightly exits 4)"
        elif res["check"]["exit"] != 0 or res["dump"]["exit"] != 0:
            row["status"] = "reads, check/dump fail"
        else:
            row["status"] = "clean"
        rows.append(row)
        print(
            f"{row['status']:<24} {e['id']:<46} detect={det} info={res['info']['exit']} "
            f"check={res['check']['exit']} dump={res['dump']['exit']} agree={len(row.get('agree', []))} "
            f"disagree={len(row.get('disagree', []))} {row.get('corpus_test', {}).get('status', '')} "
            f"assurance={row.get('assurance', {}).get('level')}"
        )
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    version = subprocess.run([str(binary), "--version"], capture_output=True, text=True).stdout.strip()
    out.write_text(json.dumps({"openreadout": version, "files": rows}, indent=1, ensure_ascii=False) + "\n")
    exposed = [r for r in rows if r.get("exposed")]
    if exposed:
        print(f"exposed (not generalization evidence; heldout_summary.py reports them apart): {len(exposed)}")
        for r in exposed:
            print(f"  {r['status']:<24} {r['id']}")
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
