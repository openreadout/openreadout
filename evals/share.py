"""Search questions: questions about a whole lab share that only a search across many files answers.

Each question stages a *share*: a directory `sample_share/` holding many corpus files under
neutral names (`sample_share/scope_a/run_004.nd2`) in a few sub-folders, and asks something like
"how many of these ND2 files were acquired with a 60x objective?". The answers are computed here,
with plain Python, from the committed third-party ground truth (corpus/oracle/<id>.json:
nd2, czifile, liffile, flowio, pyabf, pyteomics on the depositor's mzML) and from
corpus/manifest.toml (a file's `role = "corrupt"`); OpenReadout is never consulted. The
selection of files is a pure function of the manifest (tiers `smoke` and `standard`, whose files
`cargo xtask corpus fetch --tier standard` downloads), so the questions regenerate without the
corpus.

    uv run python share.py            # print every share question with its answer and evidence
"""

from __future__ import annotations

import re
import sys
import tomllib
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
sys.path.append(str(ROOT / "oracle"))
import oracle_json  # noqa: E402  (oracle/oracle_json.py: <id>.json, or <id>.json.gz over 1 MiB)

ORACLE_DIR = ROOT / "corpus" / "oracle"
MANIFEST = ROOT / "corpus" / "manifest.toml"

SHARE_DIR = "sample_share"
TIERS = {"smoke", "standard"}
# Sub-folders the staged files are spread over, round-robin, so the share has some depth.
FOLDERS = ["scope_a", "scope_b/2019", "scope_b/2021", "archive/old_runs"]
# Harness truncation, as for the single-file integrity questions.
TRUNCATE_FRACTION = 0.6


def load_manifest() -> list[dict]:
    with MANIFEST.open("rb") as fh:
        return tomllib.load(fh)["file"]


def oracle(corpus_id: str) -> dict | None:
    p = ORACLE_DIR / f"{corpus_id}.json"
    if not oracle_json.exists(p):
        return None
    return oracle_json.load(p)


def suffix(filename: str) -> str:
    base = filename.rstrip("/").rsplit("/", 1)[-1]
    for compound in (".ome.tiff", ".ome.tif"):
        if base.lower().endswith(compound):
            return base[-len(compound) :]
    return base[base.rindex(".") :] if "." in base else ""


def inputs(
    manifest: list[dict], fmt: str, *, roles=("input",), need: Callable[[dict], bool] | None = None
) -> list[dict]:
    """Manifest entries of one format in the fetched tiers, with an oracle that has what `need` wants.
    Byte-identical copies (same sha256) are kept once, so "which file…" answers are unique."""
    out = []
    for e in sorted(manifest, key=lambda e: e["id"]):
        if e.get("format") != fmt or e.get("tier") not in TIERS or e.get("role", "input") not in roles:
            continue
        if e["filename"].endswith("/") or "/" in e["filename"]:
            continue  # single top-level files only (directory data sets and folder members stay out)
        if e.get("sha256") and any(k.get("sha256") == e["sha256"] for k in out):
            continue
        o = oracle(e["id"])
        if need is not None and (o is None or not need(o)):
            continue
        out.append(e)
    return out


def images(o: dict) -> list[dict]:
    return o.get("images") or []


def objective_mag(o: dict) -> int | None:
    optics = (o.get("text_info") or {}).get("optics")
    if not optics:
        return None
    m = re.search(r"(\d+)\s*[xX×]", optics)
    return int(m.group(1)) if m else None


def fcs_labels(o: dict) -> list[str]:
    return [lab for t in o.get("tables") or [] for lab in (t.get("parameter_labels") or []) if lab]


def has_marker(o: dict, marker: str) -> bool:
    """A `$PnS` label naming the marker as a word (`CD4 FITC`; not `CD45`)."""
    return any(marker in re.split(r"[\s/_-]+", lab.upper()) for lab in fcs_labels(o))


def abf_rate(o: dict) -> float:
    return float(o["traces"][0]["sample_rate_hz"])


def abf_created_year(o: dict) -> int | None:
    c = o["traces"][0].get("created")
    m = re.match(r"(\d{4})-\d{2}-\d{2}", c or "")
    return int(m.group(1)) if m else None


def largest_plane(o: dict) -> int:
    return max((i.get("size_x") or 0) * (i.get("size_y") or 0) for i in images(o))


@dataclass
class ShareSpec:
    qid: str
    family: str
    question: str
    select: Callable[[list[dict]], list[dict]]
    answer: Callable[[list[tuple[dict, dict | None, str]]], dict]
    source: str
    truncate: list[str] = field(default_factory=list)  # corpus ids staged as truncated copies
    answer_hint: str | None = None


def integer(value: int) -> dict:
    return {"type": "number", "value": int(value), "unit": None, "tolerance": {"abs": 0}}


def staged_name(staged: str) -> dict:
    base = staged.rsplit("/", 1)[-1]
    rel = staged.split("/", 1)[1] if "/" in staged else staged
    return {
        "type": "string",
        "value": rel,
        "accept": sorted({staged, rel, base}),
    }


def staged_names(staged: list[str]) -> dict:
    """A list answer: every file must be named (by its path in the share, with or without the
    share directory)."""
    if not staged:
        raise SystemExit("a list question with no matching file")
    return {
        "type": "list",
        "value": [s.split("/", 1)[1] for s in staged],
        "accept": [sorted({s, s.split("/", 1)[1]}) for s in staged],
        "ordered": False,
    }


def unique_max(items: list, key: Callable) -> Any:
    """The item with the largest key; fails when two items share it (the answer would be ambiguous)."""
    ranked = sorted(items, key=key, reverse=True)
    if len(ranked) > 1 and key(ranked[0]) == key(ranked[1]):
        raise SystemExit(f"ambiguous maximum: {ranked[0][0]['id']} and {ranked[1][0]['id']}")
    return ranked[0]


def count(pred: Callable[[dict, dict | None], bool]) -> Callable[[list[tuple[dict, dict | None, str]]], dict]:
    return lambda files: integer(sum(1 for e, o, _ in files if pred(e, o)))


def mixed_damaged(manifest: list[dict]) -> list[dict]:
    """Intact files of several families, the corpus files recorded as corrupt, and two copies
    the harness truncates."""
    pick = []
    by_id = {e["id"]: e for e in manifest}
    for cid in [
        "fcsparser-corrupted",
        "fcsparser-cytek-nl-2000-header",
        "synthetic-mzml-truncated",
        "zenodo7015307-Z-5-CH-2",
        "aics-ND2-jonas-header-test2",
        "flowio-g11",
        "flowio-data1",
        "pyabf-2018-11-16-sh-0006",
        "pyabf-171116sh-0011",
        "zenodo7015307-T-2-CH-1",
        "aics-ND2-dims-t3c2y32x32",
        "pyteomics-tiny-pwiz",
        "mzdata-three-test-scans",
    ]:
        pick.append(by_id[cid])
    return sorted(pick, key=lambda e: e["id"])


SHARE_SPECS: list[ShareSpec] = [
    ShareSpec(
        "share-nd2-objective-60x",
        "microscopy",
        "How many of the ND2 files in this share were acquired with a 60x objective?",
        # Only files whose oracle names the objective, so the count is known for every file.
        lambda m: inputs(m, "nd2", need=lambda o: bool(images(o)) and objective_mag(o) is not None),
        count(lambda e, o: o is not None and objective_mag(o) == 60),
        "oracle: /text_info/optics of every ND2 file (magnification parsed from the objective name); "
        "only files whose optics text names an objective",
    ),
    ShareSpec(
        "share-nd2-zstacks",
        "microscopy",
        "How many of the ND2 files in this share contain a z-stack (more than one focal plane)?",
        lambda m: inputs(m, "nd2", need=lambda o: bool(images(o))),
        count(lambda e, o: o is not None and any((i.get("size_z") or 1) > 1 for i in images(o))),
        "oracle: /images/*/size_z of every ND2 file",
    ),
    ShareSpec(
        "share-czi-multiscene",
        "microscopy",
        "How many of the CZI files in this share hold more than one scene?",
        lambda m: inputs(m, "czi", need=lambda o: bool(images(o))),
        count(lambda e, o: o is not None and len(images(o)) > 1),
        "oracle: number of /images (scenes) of every CZI file",
    ),
    ShareSpec(
        "share-lif-multi-image",
        "microscopy",
        "How many of the Leica LIF files in this share contain more than one image (series)?",
        lambda m: [
            e
            for e in inputs(m, "lif", need=lambda o: bool(images(o)))
            if "lifext" not in e["id"] and "AMR1" not in e["id"]
        ],
        count(lambda e, o: o is not None and len(images(o)) > 1),
        "oracle: number of /images of every LIF file",
    ),
    ShareSpec(
        "share-largest-image",
        "microscopy",
        "Which file in this share has the largest image, by pixel count (width × height) of its biggest "
        "image or scene? Give its path within the share.",
        lambda m: (
            inputs(m, "nd2", need=lambda o: bool(images(o)))[:12]
            # aics-tiled is the same mosaic as aics-merged-tiles: only one of them, so the answer is unique
            + [e for e in inputs(m, "lif", need=lambda o: bool(images(o))) if e["id"] != "aics-tiled"][:6]
            + [
                e
                for e in inputs(m, "czi", need=lambda o: bool(images(o)))
                if e["id"].startswith(("zenodo7015307", "aics-s-"))
            ]
        ),
        lambda files: staged_name(unique_max(files, lambda f: largest_plane(f[1]))[2]),
        "oracle: max /images/*/size_x × size_y per file (czifile, nd2, liffile)",
        answer_hint="the path of the file within the share",
    ),
    ShareSpec(
        "share-fcs-large",
        "flow",
        "How many FCS files in this share contain more than 100,000 events?",
        lambda m: inputs(m, "fcs", need=lambda o: bool(o.get("tables"))),
        count(lambda e, o: o is not None and any((t.get("event_count") or 0) > 100_000 for t in o["tables"])),
        "oracle: /tables/*/event_count of every FCS file (flowio)",
    ),
    ShareSpec(
        "share-fcs-cd4",
        "flow",
        "Which FCS files in this share have a CD4 channel (a stained CD4 marker)? Give their paths within the share.",
        lambda m: inputs(m, "fcs", need=lambda o: bool(o.get("tables"))),
        lambda files: staged_names([s for e, o, s in files if o and has_marker(o, "CD4")]),
        "oracle: /tables/*/parameter_labels ($PnS) of every FCS file naming CD4 as a word (CD45 is another marker)",
        answer_hint="the paths of the files within the share, comma-separated",
    ),
    ShareSpec(
        "share-abf-fast",
        "ephys",
        "How many of the ABF recordings in this share were sampled at 20 kHz or faster?",
        lambda m: inputs(m, "abf", need=lambda o: bool(o.get("traces"))),
        count(lambda e, o: o is not None and abf_rate(o) >= 20_000),
        "oracle: /traces/0/sample_rate_hz of every ABF file (pyabf)",
    ),
    ShareSpec(
        "share-abf-before-2015",
        "ephys",
        "How many of the ABF recordings in this share were made before 2015?",
        # Two files record no usable date (an invalid date; a year of 1806): left out of the share.
        lambda m: [
            e
            for e in inputs(m, "abf", need=lambda o: bool(o.get("traces")))
            if (abf_created_year(oracle(e["id"])) or 0) >= 1990
        ],
        count(lambda e, o: o is not None and (abf_created_year(o) or 9999) < 2015),
        "oracle: /traces/0/created of every ABF file (pyabf)",
    ),
    ShareSpec(
        "share-thermo-total-scans",
        "ms",
        "How many mass spectra (scans) do the Thermo .raw files in this share hold in total?",
        lambda m: [
            e
            for e in inputs(m, "thermo-raw", need=lambda o: isinstance(o.get("spectra"), dict))
            if (e.get("size") or 0) < 50_000_000
        ],
        lambda files: integer(sum(o["spectra"]["scan_count"] for _, o, _ in files if o)),
        "oracle: /spectra/scan_count of every Thermo file (pyteomics on the depositor's mzML export)",
    ),
    ShareSpec(
        "share-damaged",
        "share",
        "Some files in this share may have been damaged when they were copied off the instrument PCs. "
        "How many of the files are truncated or corrupt?",
        mixed_damaged,
        lambda files: integer(sum(1 for e, _, _ in files if e.get("role") == "corrupt") + len(DAMAGED_TRUNCATE)),
        'manifest: files with role = "corrupt", plus the copies the harness truncates to '
        f"{int(TRUNCATE_FRACTION * 100)} %",
        truncate=[],
    ),
]

DAMAGED_TRUNCATE = ["zenodo7015307-Z-5-CH-2", "aics-ND2-jonas-header-test2"]
SHARE_SPECS[-1].truncate = DAMAGED_TRUNCATE


def stage_plan(files: list[dict]) -> list[tuple[dict, str]]:
    """Neutral names: `sample_share/<folder>/run_<n><suffix>`, spread over FOLDERS in id order."""
    plan = []
    for i, e in enumerate(files):
        folder = FOLDERS[i % len(FOLDERS)]
        plan.append((e, f"{SHARE_DIR}/{folder}/run_{i + 1:03d}{suffix(e['filename'])}"))
    return plan


def build(spec: ShareSpec, manifest: list[dict]) -> dict[str, Any]:
    files = spec.select(manifest)
    plan = stage_plan(files)
    triples = [(e, oracle(e["id"]), staged) for e, staged in plan]
    answer = spec.answer(triples)
    staged_of = {e["id"]: s for e, s in plan}
    q: dict[str, Any] = {
        "id": spec.qid,
        "family": spec.family,
        "format": "share",
        "category": "search",
        "file": {"corpus_id": "share", "path": "", "stage_as": SHARE_DIR},
        "share": [{"corpus_id": e["id"], "path": e["filename"], "stage_as": s} for e, s in plan],
        "question": spec.question,
        "answer_hint": spec.answer_hint or ("a number" if answer["type"] == "number" else "a short answer"),
        "answer": answer,
        "source": (
            f"evals/share.py over corpus/oracle/*.json and corpus/manifest.toml ({len(files)} files) — {spec.source}"
        ),
    }
    if spec.truncate:
        q["prepare"] = {"truncate": {staged_of[c]: TRUNCATE_FRACTION for c in spec.truncate}}
    return q


def build_all() -> list[dict]:
    manifest = load_manifest()
    return [build(s, manifest) for s in SHARE_SPECS]


def main() -> int:
    for q in build_all():
        print(f"{q['id']}: {len(q['share'])} files -> {q['answer']['value']!r}")
        print(f"  {q['question']}")
        print(f"  {q['source']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
