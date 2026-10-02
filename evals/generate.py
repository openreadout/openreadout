"""Generate the eval questions in evals/questions/*.jsonl.

Every answer is read from the committed ground truth in corpus/oracle/<id>.json (written by
third-party readers: czifile, nd2, liffile, tifffile, mrcfile, flowio, pyabf, neo, nmrglue,
pyteomics, rainbow/Aston, allotropy, ...), from a fact recorded in corpus/manifest.toml, or (for the
experiment-level questions) from evals/facts/experiment.json, which evals/facts.py extracts with
plain Python from the depositor's mzML exports and the vendor's own text files (Waters
_HEADER.TXT, ChemStation result.ini, TopSpin acqus and title, the FCS TEXT segment, plate-reader
text exports). Our own reader's output is never consulted, so a question cannot be "right" just
because OpenReadout says so.

The specs below choose which corpus file and which oracle field each question asks about; the
values themselves are looked up at generation time, so regenerating after an oracle update keeps
the answers in step. Run:

    uv run --project evals python evals/generate.py          # rewrite evals/questions/*.jsonl
    uv run --project evals python evals/generate.py --check  # fail if the committed files differ
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import re
import sys
import tomllib
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import share

ROOT = Path(__file__).resolve().parent.parent
ORACLE_DIR = ROOT / "corpus" / "oracle"
MANIFEST = ROOT / "corpus" / "manifest.toml"
FACTS = Path(__file__).resolve().parent / "facts" / "experiment.json"
# Analysis-tier values (pixels, sweeps, events, spectra) computed by evals/analysis.py with third-party readers.
ANALYSIS_FACTS = Path(__file__).resolve().parent / "facts" / "analysis.json"
# Quantitation-tier values (peak areas, heights, XIC/SRM peaks) computed by evals/quant.py
# from vendor reports and pyOpenMS.
QUANT_FACTS = Path(__file__).resolve().parent / "facts" / "quant.json"
# Vibrational-spectroscopy values computed by evals/spectroscopy.py with third-party readers.
SPECTRO_FACTS = Path(__file__).resolve().parent / "facts" / "spectroscopy.json"
# Agent-surface values (RGB components, projections, folder analyses) computed by evals/agent_facts.py.
AGENT_FACTS = Path(__file__).resolve().parent / "facts" / "agent.json"
# Whole-slide region/level values computed by evals/regions.py with third-party readers.
REGION_FACTS = Path(__file__).resolve().parent / "facts" / "regions.json"
FACT_FILES = {
    "facts": FACTS,
    "facts-analysis": ANALYSIS_FACTS,
    "facts-quant": QUANT_FACTS,
    "facts-spectroscopy": SPECTRO_FACTS,
    "facts-agent": AGENT_FACTS,
    "facts-regions": REGION_FACTS,
    # Agilent OpenLab CDS vendor results (evals/openlab_facts.py, from the vendor CSV exports)
    "facts-openlab": Path(__file__).resolve().parent / "facts" / "openlab.json",
    # one-call routes: Thermo PDA, compensated counts, folder MIPs, raw/mzXML pairing (evals/routing_facts.py)
    "facts-routing": Path(__file__).resolve().parent / "facts" / "routing.json",
    # spectral band areas, Gen5 control roles, a spectra folder with its sheet (evals/gaps_facts.py)
    "facts-gaps": Path(__file__).resolve().parent / "facts" / "gaps.json",
    # visual tier: tissue boxes, fold/hole points, best focus, blank wells (evals/visual.py)
    "facts-visual": Path(__file__).resolve().parent / "facts" / "visual.json",
    # bench instruments: UNICORN peak tables, ITC heats, Biacore/Image Lab/JASCO run facts (evals/bench.py)
    "facts-bench": Path(__file__).resolve().parent / "facts" / "bench.json",
}
OUT_DIR = Path(__file__).resolve().parent / "questions"

FAMILIES = {
    # (formats after the `|` are only asked about in the held-out set so far: evals/heldout.py)
    "microscopy": {"czi", "nd2", "lif", "tiff", "oir", "vsi", "zvi", "ims", "ome-zarr"} | {"oib", "oif", "dcimg"},
    "em": {"mrc", "dm", "ser", "emd"},
    "flow": {"fcs"},
    "ephys": {"abf", "neuralynx", "blackrock", "spikeglx", "intan", "nwb"} | {"atf", "plexon"},
    "nmr": {"bruker-nmr", "jcamp-dx"} | {"varian-nmr", "jeol-jdf"},
    "ms": {"thermo-raw", "mzml", "mzxml", "imzml", "bruker-tdf", "mzmlb"} | {"agilent-masshunter", "sciex-wiff"},
    "chromatography": {
        "chemstation",
        "andi-chrom",
        "waters-raw",
        "shimadzu",
        "openlab-cds",
        "chromeleon",
        "empower-arw",
    },
    "plates": {"plate"},
    "qpcr": {"rdml", "applied-biosystems-eds", "bio-rad-pcrd", "rotor-gene-rex", "roche-lightcycler-ixo"},
    "hcs": {"opera-harmony", "imagexpress", "cellvoyager"},
    "spectroscopy": {"bruker-opus", "thermo-omnic", "renishaw-wdf", "perkinelmer-sp"},
    # bench instruments (evals/bench.py): protein purification, biophysics, gel imaging, JASCO spectra
    "bench": {
        "cytiva-unicorn-res",
        "cytiva-unicorn-zip",
        "microcal-itc",
        "cytiva-biacore-blr",
        "biorad-scn",
        "jasco-jws",
        "agilent-seahorse-asyr",
        "genepix-gpr",
        "panalytical-xrdml",
        "biologic-mpr",
        "neware-nda",
        "netzsch-ngb",
        "ta-universal-analysis",
    },
    # search questions over a whole staged share (share.py); single-family shares use their family
    "share": set(),
}

CATEGORIES = [
    "identify-format",
    "dimensions",
    "pixel-size",
    "channels",
    "acquisition-time",
    "instrument",
    "integrity",
    "counts",
    "sample-rate",
    "values",
    "conversion",
    "sample",
    "method",
    "operator",
    "search",
    "analysis",
    "quantitation",
    "batch",
    "scenario",
    "visual",
]

# Id prefix of every question in a tier (README.md): the held-out split is `ho-*` whatever its
# category; the lookup categories keep `<family>-<format>-<what>` (or `exp-*`, `int-*`, ...).
TIER_ID_PREFIX = {
    "analysis": "ana-",
    "quantitation": "qnt-",
    "batch": "batch-",
    "scenario": "scn-",
    "visual": "vis-",
    "search": "share-",
    "conversion": "task-",
}
HELDOUT_ID_PREFIX = "ho-"


def id_prefix(q: dict) -> str | None:
    """The prefix the question's id must carry, or None for a lookup (free-form `<family>-...`)."""
    if q.get("split") == "heldout":
        return HELDOUT_ID_PREFIX
    return TIER_ID_PREFIX.get(q["category"])


# Accepted names for "what format is this?" answers, keyed by the manifest's `format`.
FORMAT_NAMES = {
    "nd2": ["ND2", "Nikon NIS-Elements", "NIS-Elements", "Nikon"],
    "czi": ["CZI", "Carl Zeiss Image", "ZEN"],
    "lif": ["LIF", "Leica Image File", "Leica LAS X", "Leica"],
    "mrc": ["MRC", "CCP4", "MRC2014"],
    "fcs": ["FCS", "Flow Cytometry Standard"],
    "abf": ["ABF", "Axon Binary", "pCLAMP", "Axon"],
    "thermo-raw": ["Thermo RAW", "Thermo", "Xcalibur", "Thermo Fisher"],
}

# Truncated copies: the harness keeps this fraction of the bytes (see run.py `stage`).
TRUNCATE_FRACTION = 0.6

UNKNOWN_NAME = "unknown_file"


# ---------------------------------------------------------------- answer builders


def number(value: float, unit: str | None = None, rel: float | None = None, abs_: float | None = None) -> dict:
    tol: dict[str, float] = {}
    if rel is not None:
        tol["rel"] = rel
    if abs_ is not None:
        tol["abs"] = abs_
    if not tol:
        tol["abs"] = 0
    return {"type": "number", "value": value, "unit": unit, "tolerance": tol}


def integer(value: int, unit: str | None = None) -> dict:
    return number(int(value), unit, abs_=0)


def string(value: str, accept: list[str] | None = None, reject: list[str] | None = None) -> dict:
    out: dict[str, Any] = {
        "type": "string",
        "value": value,
        "accept": sorted(set([value, *(accept or [])])),
    }
    if reject:
        out["reject"] = reject
    return out


def boolean(value: bool) -> dict:
    return {"type": "boolean", "value": value}


def items(values: list[str], variants: dict[str, list[str]] | None = None, ordered: bool = False) -> dict:
    variants = variants or {}
    return {
        "type": "list",
        "value": values,
        "accept": [sorted(set([v, *variants.get(v, [])])) for v in values],
        "ordered": ordered,
    }


def date(value: str, tolerance_days: int = 0) -> dict:
    return {"type": "date", "value": value, "tolerance_days": tolerance_days}


def bbox(value: list[float], min_iou: float = 0.5) -> dict:
    """A box [x, y, width, height] (pixels, origin top-left), scored by intersection over union."""
    return {"type": "bbox", "value": [int(v) for v in value], "min_iou": min_iou}


def point(value: list[float], radius: float | None = None, mask: dict | None = None) -> dict:
    """A point [x, y] (pixels, origin top-left): correct inside `mask` (row runs [row, x0, x1) at
    `scale` full-resolution pixels per mask pixel along x and y) or within `radius` of `value`."""
    out: dict[str, Any] = {"type": "point", "value": [int(v) for v in value]}
    if radius is not None:
        out["radius"] = radius
    if mask is not None:
        out["mask"] = {"scale": [float(s) for s in mask["scale"]], "runs": mask["runs"]}
    return out


def choice(
    value: str | list[str],
    options: list[str],
    aliases: dict[str, list[str]] | None = None,
    multiple: bool = False,
) -> dict:
    """Named options. Single choice: the answer must name at least one option and only options in
    `value` (the accepted ones). `multiple`: the named options must be exactly the set `value`."""
    accepted = [value] if isinstance(value, str) else list(value)
    unknown = [v for v in accepted if v not in options]
    if unknown:
        raise ValueError(f"choice: {unknown} not among the options")
    aliases = aliases or {}
    return {
        "type": "choice",
        "value": accepted,
        "multiple": multiple,
        "options": {o: sorted({o, *aliases.get(o, [])}) for o in options},
    }


# ---------------------------------------------------------------- oracle access


def load_manifest() -> dict[str, list[dict]]:
    with MANIFEST.open("rb") as fh:
        data = tomllib.load(fh)
    by_id: dict[str, list[dict]] = {}
    for entry in data["file"]:
        by_id.setdefault(entry["id"], []).append(entry)
    return by_id


def manifest_entry(manifest: dict[str, list[dict]], corpus_id: str, role: str | None = None) -> dict:
    entries = manifest.get(corpus_id)
    if not entries:
        raise KeyError(f"{corpus_id}: not in corpus/manifest.toml")
    if role is None:
        # Thermo inputs share their id with the depositor's mzML (role = "oracle-export").
        entries = [e for e in entries if e.get("role") != "oracle-export"] or entries
        return entries[0]
    for e in entries:
        if e.get("role") == role:
            return e
    raise KeyError(f"{corpus_id}: no entry with role {role}")


def load_facts(kind: str = "facts") -> dict:
    """Experiment facts extracted by facts.py: {corpus id: {file, extractor, facts: {name: {value, where}}}};
    with kind "facts-analysis", the analysis-tier values analysis.py computed (same shape, each fact
    with its reader and computation instead of `where`)."""
    with FACT_FILES.get(kind, FACTS).open() as fh:
        return json.load(fh)


def facts_path(kind: str) -> Path:
    """The facts file of a question source kind (`facts`, `facts-analysis`, `facts-regions`, ...);
    unknown kinds (`facts-heldout`) read facts/experiment.json."""
    return FACT_FILES.get(kind, FACTS)


def load_oracle(corpus_id: str) -> dict:
    path = ORACLE_DIR / f"{corpus_id}.json"
    # format-specific oracles kept apart from the harness' (corpus/oracle/<kind>/)
    if not share.oracle_json.exists(path):
        path = next(ORACLE_DIR.glob(f"*/{corpus_id}.json"), path)
    return share.oracle_json.load(path)  # <id>.json, or <id>.json.gz over 1 MiB


def pointer(doc: Any, ptr: str) -> Any:
    """Resolve a JSON pointer (RFC 6901) such as /images/0/size_z."""
    cur = doc
    for raw in ptr.split("/")[1:]:
        key = raw.replace("~1", "/").replace("~0", "~")
        cur = cur[int(key)] if isinstance(cur, list) else cur[key]
    return cur


COMPOUND_SUFFIXES = (".ome.tiff", ".ome.tif", ".ome.btf")


def suffix_of(filename: str) -> str:
    """The extension that readers rely on: `.czi`, `.ome.tiff`, `.D` (never a dotted stem such as `15.4nm`)."""
    base = filename.rsplit("/", 1)[-1]
    for compound in COMPOUND_SUFFIXES:
        if base.lower().endswith(compound):
            return base[-len(compound) :]
    return base[base.rindex(".") :] if "." in base else ""


# ---------------------------------------------------------------- spec


@dataclass
class Spec:
    qid: str
    corpus_id: str
    category: str
    question: str
    answer: Callable[[dict, dict], dict]
    source: str
    stage_as: str | None = "auto"  # "auto": sample<suffix>; None: keep the corpus name
    extra: list[tuple[str, str | None]] = field(default_factory=list)  # (corpus id, stage_as)
    prepare: dict | None = None
    task: dict | None = None
    answer_hint: str | None = None
    subcategory: str | None = None  # the visual tier's kind of question (locate, focus, count, ...)


def objective_mag(optics: str) -> str:
    match = re.search(r"(\d+)\s*[xX×]", optics)
    if not match:
        raise ValueError(f"no magnification in {optics!r}")
    return f"{match.group(1)}x"


def epoch_date(seconds: int) -> str:
    return dt.datetime.fromtimestamp(seconds, tz=dt.UTC).date().isoformat()


def ms2_count(o: dict) -> int:
    return sum(1 for s in o["spectra"]["scans"] if s["ms_level"] == 2)


def first_ms2_precursor(o: dict) -> float:
    return next(s["precursor_mz"] for s in o["spectra"]["scans"] if s["ms_level"] == 2)


def wavelength_from_signal(name: str) -> float:
    match = re.search(r"Sig=(\d+(?:\.\d+)?)", name)
    if not match:
        raise ValueError(f"no signal wavelength in {name!r}")
    return float(match.group(1))


def dye_labels(o: dict) -> list[str]:
    """$PnS labels of fluorescence parameters, without the -A/-H/-W suffix and trademark signs."""
    seen: list[str] = []
    for name, label in zip(o["tables"][0]["parameter_names"], o["tables"][0]["parameter_labels"], strict=True):
        if not label or label == name or label.startswith(("FSC", "SSC", "Time")):
            continue
        base = re.sub(r"-[AHW]$", "", label).replace("™", "").strip()
        if base not in seen:
            seen.append(base)
    return seen


DETECTOR_NAMES = {"FID": ["flame ionization"], "TCD": ["thermal conductivity"]}


def detector_items(o: dict) -> dict:
    """Detector types from ChemStation signal names (`FID1A` -> FID); the signal name also counts."""
    signals = [t["channel_names"][0] for t in o["traces"]]
    kinds = [re.sub(r"\d+[A-Z]?$", "", s) for s in signals]
    variants = {k: [*DETECTOR_NAMES.get(k, []), s] for k, s in zip(kinds, signals, strict=True)}
    return items(kinds, variants)


def pixel_um(o: dict, axis: str = "x", image: int = 0) -> float:
    return pointer(o, f"/images/{image}/physical_size_um/{axis}")


SPECS: list[Spec] = [
    # ------------------------------------------------------------ microscopy
    Spec(
        "mic-nd2-identify",
        "aics-ND2-dims-t3c2y32x32",
        "identify-format",
        "A colleague sent me this microscope file but the extension got lost. What file format is it, "
        "i.e. which microscope software wrote it?",
        lambda o, m: string(m["format"], FORMAT_NAMES["nd2"]),
        "manifest: format",
        stage_as=UNKNOWN_NAME,
        answer_hint="the format name",
    ),
    Spec(
        "mic-czi-z-slices",
        "zenodo7015307-Z-5-CH-2",
        "dimensions",
        "How many z-slices are in this confocal stack?",
        lambda o, m: integer(pointer(o, "/images/0/size_z")),
        "oracle: /images/0/size_z",
    ),
    Spec(
        "mic-nd2-positions",
        "aics-ND2-dims-p4z5t3c2y32x32",
        "dimensions",
        "How many different stage positions (XY points) were imaged in this experiment?",
        lambda o, m: integer(len(o["images"])),
        "oracle: len(/images) (nd2: one image per XY position)",
    ),
    Spec(
        "mic-czi-pixel-size",
        "aics-s-1-t-1-c-1-z-1",
        "pixel-size",
        "What is the pixel size of this image (micrometres per pixel)?",
        lambda o, m: number(pixel_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x",
        answer_hint="a number with its unit",
    ),
    Spec(
        "mic-svs-pixel-size",
        "openslide-aperio-cmu-1-small-region",
        "pixel-size",
        "This is a scanned slide. What is its resolution in microns per pixel?",
        lambda o, m: number(pixel_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x",
        answer_hint="a number with its unit",
    ),
    Spec(
        "mic-oir-z-step",
        "zenodo12773657-dapi-mcherry-4z-5lambda",
        "pixel-size",
        "What was the z-step (spacing between optical sections) in this stack?",
        lambda o, m: number(pointer(o, "/images/0/physical_size_um/z"), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/z (oirfile)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "mic-nd2-channel-names",
        "aics-ND2-dims-c2y32x32",
        "channels",
        "What are the names of the channels in this image?",
        lambda o, m: items(pointer(o, "/images/0/channel_names")),
        "oracle: /images/0/channel_names",
        answer_hint="the channel names, comma-separated",
    ),
    Spec(
        "mic-nd2-dyes",
        "zenodo21162526-nested-loop",
        "channels",
        "Which fluorophores / channels were acquired in this experiment?",
        lambda o, m: items(
            pointer(o, "/images/0/channel_names"),
            {"TD": ["transmitted", "brightfield", "trans"], "eGFP": ["EGFP", "GFP"]},
        ),
        "oracle: /images/0/channel_names",
        answer_hint="the channel names, comma-separated",
    ),
    Spec(
        "mic-lif-lambda-bands",
        "zenodo14976703-Convalaria-LambdaScan",
        "channels",
        "This is a spectral (lambda) scan. How many emission bands were recorded?",
        lambda o, m: integer(pointer(o, "/images/0/size_c")),
        "oracle: /images/0/size_c (liffile: lambda axis)",
    ),
    Spec(
        "mic-nd2-acquired",
        "aics-ND2-jonas-header-test2",
        "acquisition-time",
        "On what date was this image acquired?",
        lambda o, m: date(pointer(o, "/nd2_meta/acquired_at")[:10]),
        "oracle: /nd2_meta/acquired_at (nd2 package; local date in /text_info/date agrees)",
        answer_hint="the date as YYYY-MM-DD",
    ),
    Spec(
        "mic-nd2-objective",
        "aics-ND2-jonas-header-test2",
        "instrument",
        "Which objective magnification was used for this acquisition?",
        lambda o, m: string(objective_mag(pointer(o, "/text_info/optics"))),
        "oracle: /text_info/optics (magnification parsed from the objective name)",
        answer_hint="the magnification, e.g. 40x",
    ),
    # ------------------------------------------------------------ electron microscopy
    Spec(
        "em-mrc-identify",
        "mrcfile-emd-3197",
        "identify-format",
        "I found this file on our cryo-EM share without an extension. What file format is it?",
        lambda o, m: string(m["format"], FORMAT_NAMES["mrc"]),
        "manifest: format",
        stage_as=UNKNOWN_NAME,
        answer_hint="the format name",
    ),
    Spec(
        "em-mrc-voxel-size",
        "mrcfile-emd-3197",
        "pixel-size",
        "What is the voxel size of this density map, in ångström?",
        lambda o, m: number(pointer(o, "/images/0/mrcfile_voxel_size_angstrom/0"), "Å", rel=0.01),
        "oracle: /images/0/mrcfile_voxel_size_angstrom/0 (mrcfile)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "em-mrcs-class-count",
        "empiar10045-class2d-it025",
        "dimensions",
        "How many 2D class averages are in this stack?",
        lambda o, m: integer(pointer(o, "/images/0/size_t")),
        "oracle: /images/0/size_t (mrcfile: sections of an image stack)",
    ),
    Spec(
        "em-emd-pixel-size",
        "zenodo20040988-0050-STEM-15.4nm",
        "pixel-size",
        "What is the pixel size of this STEM image?",
        lambda o, m: number(pixel_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x (h5py on the Velox EMD)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "em-emd-detector",
        "zenodo20040988-0050-STEM-15.4nm",
        "instrument",
        "Which detector recorded this STEM image?",
        lambda o, m: string(pointer(o, "/images/0/detector"), ["high-angle annular dark field", "HAADF-STEM"]),
        "oracle: /images/0/detector",
        answer_hint="the detector name",
    ),
    # ------------------------------------------------------------ flow cytometry
    Spec(
        "flow-fcs-identify",
        "fcsparser-fortessa-a01",
        "identify-format",
        "Our core facility sent this file without an extension. What file format is it?",
        lambda o, m: string(m["format"], FORMAT_NAMES["fcs"]),
        "manifest: format",
        stage_as=UNKNOWN_NAME,
        answer_hint="the format name",
    ),
    Spec(
        "flow-fcs-events",
        "flowio-g11",
        "counts",
        "How many events (cells) were recorded in this sample?",
        lambda o, m: integer(pointer(o, "/tables/0/event_count")),
        "oracle: /tables/0/event_count (flowio; fcsparser agrees)",
    ),
    Spec(
        "flow-fcs-parameters",
        "flowio-3fitc-4pe-004",
        "channels",
        "Which parameters (detector channels) were recorded?",
        lambda o, m: items(pointer(o, "/tables/0/parameter_names")),
        "oracle: /tables/0/parameter_names ($PnN)",
        answer_hint="the parameter names, comma-separated",
    ),
    Spec(
        "flow-fcs-dyes",
        "flowio-g11",
        "channels",
        "Which fluorescent markers / fluorochromes were in the panel?",
        lambda o, m: items(dye_labels(o), {"Alexa Fluor 405": ["AF405", "Alexa 405", "Alexa Fluor® 405"]}),
        "oracle: /tables/0/parameter_labels ($PnS, fluorescence parameters, -A/-H/-W merged)",
        answer_hint="the marker names, comma-separated",
    ),
    # ------------------------------------------------------------ electrophysiology
    Spec(
        "ephys-abf-identify",
        "pyabf-171116sh-0011",
        "identify-format",
        "This recording lost its extension when it was copied. What file format is it?",
        lambda o, m: string(m["format"], FORMAT_NAMES["abf"]),
        "manifest: format",
        stage_as=UNKNOWN_NAME,
        answer_hint="the format name",
    ),
    Spec(
        "ephys-abf-sweeps",
        "pyabf-2018-11-16-sh-0006",
        "counts",
        "How many sweeps are in this recording?",
        lambda o, m: integer(pointer(o, "/traces/0/sweep_count")),
        "oracle: /traces/0/sweep_count (pyabf)",
    ),
    Spec(
        "ephys-abf-sample-rate",
        "pyabf-180415-aaron-temp",
        "sample-rate",
        "What was the sampling rate of this recording?",
        lambda o, m: number(pointer(o, "/traces/0/sample_rate_hz"), "Hz", rel=0.001),
        "oracle: /traces/0/sample_rate_hz (pyabf)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "ephys-abf-date",
        "pyabf-05210017-vc-abf1",
        "acquisition-time",
        "On what date was this recording made?",
        lambda o, m: date(pointer(o, "/traces/0/created")[:10]),
        "oracle: /traces/0/created (pyabf)",
        answer_hint="the date as YYYY-MM-DD",
    ),
    Spec(
        "ephys-abf-first-sample",
        "pyabf-2018-11-16-sh-0006",
        "values",
        "What is the value of the very first sample of the first sweep, in the recording's units?",
        lambda o, m: number(
            pointer(o, "/traces/0/sweeps/0/channels/0/first/0"),
            pointer(o, "/traces/0/channel_units/0"),
            abs_=0.05,
        ),
        "oracle: /traces/0/sweeps/0/channels/0/first/0 and /traces/0/channel_units/0 (pyabf)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "ephys-sglx-sample-rate",
        "sglx-5-19-2022-ci1-g0-t0-imec0-ap-bin",
        "sample-rate",
        "What is the sampling rate of this Neuropixels AP-band recording?",
        lambda o, m: number(pointer(o, "/traces/0/sample_rate_hz"), "Hz", rel=0.001),
        "oracle: /traces/0/sample_rate_hz (neo SpikeGLXRawIO)",
        stage_as="sample.imec0.ap.bin",
        extra=[("sglx-5-19-2022-ci1-g0-t0-imec0-ap-meta", "sample.imec0.ap.meta")],
        answer_hint="a number with its unit",
    ),
    Spec(
        "ephys-ncs-sample-rate",
        "nlx-cheetah-v5-4-0-csc5-trunc-ncs",
        "sample-rate",
        "What is the sampling rate of this continuously sampled channel?",
        lambda o, m: number(pointer(o, "/traces/0/sample_rate_hz"), "Hz", rel=0.001),
        "oracle: /traces/0/sample_rate_hz (neo NeuralynxRawIO)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "ephys-intan-channels",
        "intan-test-tetrode-163225-rhd",
        "channels",
        "How many amplifier (electrode) channels were recorded?",
        lambda o, m: integer(next(t["channel_count"] for t in o["traces"] if t.get("name") == "amplifier")),
        "oracle: traces[name=amplifier].channel_count (neo IntanRawIO)",
    ),
    # ------------------------------------------------------------ NMR
    Spec(
        "nmr-bruker-nucleus",
        "nmrxiv-s596-1",
        "instrument",
        "Which nucleus was observed in this NMR experiment?",
        lambda o, m: string(
            pointer(o, "/traces/0/parameters/acqus/NUC1"),
            ["carbon-13", "C-13", "13-C", "carbon 13"],
        ),
        "oracle: /traces/0/parameters/acqus/NUC1 (nmrglue)",
        answer_hint="the nucleus, e.g. 1H",
    ),
    Spec(
        "nmr-bruker-frequency",
        "nmrxiv-s846-50",
        "instrument",
        "At what spectrometer frequency was this spectrum acquired?",
        lambda o, m: number(pointer(o, "/traces/0/parameters/acqus/SFO1"), "MHz", rel=0.002),
        "oracle: /traces/0/parameters/acqus/SFO1 (nmrglue)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "nmr-bruker-solvent",
        "nmrxiv-s846-50",
        "instrument",
        "Which solvent was the sample dissolved in?",
        lambda o, m: string(
            pointer(o, "/traces/0/parameters/acqus/SOLVENT"),
            ["chloroform-d", "deuterochloroform", "deuterated chloroform", "CDCl₃"],
        ),
        "oracle: /traces/0/parameters/acqus/SOLVENT (nmrglue)",
        answer_hint="the solvent",
    ),
    Spec(
        "nmr-bruker-scans",
        "nmrglue-bruker-1d",
        "counts",
        "How many scans were averaged for this FID?",
        lambda o, m: integer(pointer(o, "/traces/0/parameters/acqus/NS")),
        "oracle: /traces/0/parameters/acqus/NS (nmrglue)",
    ),
    Spec(
        "nmr-bruker-date",
        "nmrxiv-s846-50",
        "acquisition-time",
        "On what date was this spectrum recorded?",
        lambda o, m: date(epoch_date(pointer(o, "/traces/0/parameters/acqus/DATE")), tolerance_days=1),
        "oracle: /traces/0/parameters/acqus/DATE (Unix time, nmrglue); ±1 day for the time zone",
        answer_hint="the date as YYYY-MM-DD",
    ),
    Spec(
        "nmr-bruker-temperature",
        "nmrglue-bruker-1d",
        "instrument",
        "What was the sample temperature during acquisition?",
        lambda o, m: number(pointer(o, "/traces/0/parameters/acqus/TE"), "K", abs_=0.5),
        "oracle: /traces/0/parameters/acqus/TE (kelvin, nmrglue)",
        answer_hint="a number with its unit",
    ),
    # ------------------------------------------------------------ mass spectrometry
    Spec(
        "ms-raw-identify",
        "mtbls20-caffeine-pos",
        "identify-format",
        "This mass-spec file lost its extension. What file format is it (which vendor)?",
        lambda o, m: string(m["format"], FORMAT_NAMES["thermo-raw"]),
        "manifest: format",
        stage_as=UNKNOWN_NAME,
        answer_hint="the format name",
    ),
    Spec(
        "ms-raw-scan-count",
        "mtbls20-caffeine-pos",
        "counts",
        "How many scans (spectra) are in this run?",
        lambda o, m: integer(pointer(o, "/spectra/scan_count")),
        "oracle: /spectra/scan_count (pyteomics on the depositor's mzML export)",
    ),
    Spec(
        "ms-raw-ms2-count",
        "mtbls20-caffeine-pos",
        "counts",
        "How many MS/MS (MS2) scans are in this run?",
        lambda o, m: integer(ms2_count(o)),
        "oracle: count of /spectra/scans[ms_level=2] (pyteomics on the depositor's mzML export)",
    ),
    Spec(
        "ms-raw-polarity",
        "mtbls20-hydroxymethoxycinnamic-neg",
        "instrument",
        "Was this run acquired in positive or negative ionization mode?",
        lambda o, m: string(
            o["spectra"]["scans"][0]["polarity"],
            ["neg", "negative ion", "ESI-"],
            reject=["positive"],
        ),
        "oracle: /spectra/scans/0/polarity (all scans agree; pyteomics on the depositor's mzML)",
        answer_hint="positive or negative",
    ),
    Spec(
        "ms-raw-first-precursor",
        "mtbls20-caffeine-pos",
        "values",
        "What was the precursor m/z of the first MS/MS scan?",
        lambda o, m: number(round(first_ms2_precursor(o), 4), None, abs_=0.01),
        "oracle: first /spectra/scans[ms_level=2].precursor_mz (pyteomics on the depositor's mzML)",
        answer_hint="the m/z value",
    ),
    Spec(
        "ms-mzml-last-rt",
        "mzdata-small",
        "values",
        "What is the retention time of the last spectrum in this file?",
        lambda o, m: number(o["spectra"]["scans"][-1]["rt_s"], "s", rel=0.005),
        "oracle: /spectra/scans/-1/rt_s (pyteomics)",
        answer_hint="a number with its unit",
    ),
    # ------------------------------------------------------------ chromatography
    Spec(
        "chrom-chemstation-detectors",
        "chromhandler-001f0101-d",
        "instrument",
        "Which detectors recorded signals in this GC run?",
        lambda o, m: detector_items(o),
        "oracle: /traces/*/channel_names (rainbow-api; detector prefix of the signal name)",
        stage_as="sample.D",
        answer_hint="the detector types, comma-separated",
    ),
    Spec(
        "chrom-andi-wavelength",
        "cheminfo-agilent-hplc-cdf",
        "channels",
        "At what detection wavelength was this HPLC chromatogram recorded?",
        lambda o, m: number(wavelength_from_signal(pointer(o, "/traces/0/channel_names/0")), "nm", abs_=0.5),
        "oracle: /traces/0/channel_names/0 (scipy netcdf; 'Sig=' wavelength of the DAD signal name)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "chrom-andi-peaks",
        "cheminfo-agilent-hplc-cdf",
        "counts",
        "How many integrated peaks are in this chromatogram's peak table?",
        lambda o, m: integer(pointer(o, "/tables/0/event_count")),
        "oracle: /tables/0/event_count (scipy netcdf peak table)",
    ),
    # MS/chromatography format gaps: gzip mzML, mzMLb, Thermo LC detectors, Shimadzu
    Spec(
        "ms-mzmlgz-ms2-count",
        "mzdata-timstof-gz",
        "counts",
        "This run was downloaded as a gzip-compressed mzML (.mzML.gz). How many MS2 (MS/MS) spectra does it contain?",
        lambda o, m: integer(ms2_count(o)),
        "oracle: count of /spectra/scans[ms_level=2] (pyteomics on the gzip-decompressed file)",
        stage_as="sample.mzML.gz",
    ),
    Spec(
        "ms-mzmlb-ms2-count",
        "mzdata-small-mzmlb",
        "counts",
        "This is an mzMLb file (mzML stored in HDF5). How many MS/MS spectra does it contain?",
        lambda o, m: integer(ms2_count(o)),
        "oracle: count of /spectra/scans[ms_level=2] (pyteomics mzmlb.MzMLb)",
        stage_as="sample.mzMLb",
    ),
    Spec(
        "ms-mzmlb-first-precursor",
        "mzdata-small-mzmlb",
        "values",
        "What is the precursor m/z of the first MS/MS spectrum in this mzMLb file?",
        lambda o, m: number(round(first_ms2_precursor(o), 4), None, abs_=0.01),
        "oracle: first /spectra/scans[ms_level=2].precursor_mz (pyteomics mzmlb.MzMLb)",
        stage_as="sample.mzMLb",
        answer_hint="the m/z value",
    ),
    Spec(
        "ms-raw-uv-channel-wavelength",
        "zenodo19222374-cannabis-neg-h1-1",
        "channels",
        "Besides MS, this LC-MS run recorded UV absorbance with a diode-array detector. At what wavelength was the "
        "single-wavelength UV channel UV_VIS_1 recorded?",
        lambda o, m: number(o["facts"]["uv_channel_wavelength_nm"]["UV_VIS_1"], "nm", abs_=0.5),
        "oracle: /facts/uv_channel_wavelength_nm/UV_VIS_1 (the vendor-written instrument method, read with olefile: "
        "corpus/oracle/thermo-method/)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "ms-raw-dad-rate",
        "zenodo19222374-cannabis-neg-h1-1",
        "sample-rate",
        "At what rate (in Hz) did the diode-array (UV) detector of this LC-MS run record data points?",
        lambda o, m: number(o["facts"]["uv_data_collection_rate_hz"], "Hz", rel=0.01),
        "oracle: /facts/uv_data_collection_rate_hz (the vendor-written instrument method, read with olefile: "
        "corpus/oracle/thermo-method/)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "chrom-shimadzu-main-peak-rt",
        "zenodo17868549-gp070190p-hplc",
        "values",
        "This is a Shimadzu LabSolutions HPLC run with a UV detector. At what retention time does the largest peak "
        "elute?",
        lambda o, m: number(round(o["peak_table"][0]["rt_min"], 4), "min", abs_=0.05),
        "oracle: /peak_table/0/rt_min (the LabSolutions peak table stored in the file, largest area first; read by "
        "chromConverter as a black box: corpus/oracle/shimadzu/)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "chrom-shimadzu-sample-rate",
        "zenodo17868549-gp070190p-hplc",
        "sample-rate",
        "At what rate was the detector of this Shimadzu HPLC run sampled?",
        lambda o, m: number(
            round((o["chromatogram"]["n"] - 1) / (o["chromatogram"]["last_rt_min"] * 60), 6), "Hz", rel=0.01
        ),
        "oracle: (/chromatogram/n − 1) / /chromatogram/last_rt_min (chromConverter's time axis: "
        "corpus/oracle/shimadzu/)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "chrom-andi-sample-rate",
        "mtbls390-wb-cc-bat-01-cdf",
        "sample-rate",
        "At what rate was the detector sampled?",
        lambda o, m: number(pointer(o, "/traces/0/sample_rate_hz"), "Hz", rel=0.01),
        "oracle: /traces/0/sample_rate_hz (scipy netcdf; 1 / actual_sampling_interval)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "chrom-ch-run-length",
        "entab-test-179-fid-ch",
        "values",
        "How long is this chromatogram, i.e. the retention time of the last data point?",
        lambda o, m: number(pointer(o, "/traces/0/x_last_min"), "min", abs_=0.05),
        "oracle: /traces/0/x_last_min (rainbow-api)",
        answer_hint="a number with its unit",
    ),
    # ------------------------------------------------------------ plate readers
    Spec(
        "plate-skanit-instrument",
        "skanit-elisa-steps",
        "instrument",
        "Which plate reader model produced this export?",
        lambda o, m: string(pointer(o, "/plate/header/model"), ["Varioskan"]),
        "oracle: /plate/header/model (allotropy)",
        answer_hint="the instrument model",
    ),
    Spec(
        "plate-gen5-wavelength",
        "gen5-kinetic-growth-curve",
        "channels",
        "At what wavelength was the absorbance measured?",
        lambda o, m: number(pointer(o, "/plate/groups/0/wavelengths/0"), "nm", abs_=0.5),
        "oracle: /plate/groups/0/wavelengths/0 (allotropy)",
        answer_hint="a number with its unit",
    ),
    Spec(
        "plate-gen5-kinetic-reads",
        "gen5-kinetic-growth-curve",
        "counts",
        "This is a kinetic growth curve. How many reads (time points) were taken per well?",
        lambda o, m: integer(pointer(o, "/plate/groups/0/values") // pointer(o, "/plate/groups/0/wells")),
        "oracle: /plate/groups/0/values ÷ /plate/groups/0/wells (allotropy)",
    ),
    Spec(
        "plate-bmg-wells",
        "bmg-mars-lum-1536",
        "counts",
        "How many wells were actually measured in this luminescence read?",
        lambda o, m: integer(pointer(o, "/plate/groups/0/wells")),
        "oracle: /plate/groups/0/wells (allotropy)",
    ),
]

# ---------------------------------------------------------------- experiment-level questions
# Sample identity, method, operator and instrument, answered from evals/facts/experiment.json (the
# depositor's mzML, the vendor's own text files; see facts.py), the oracle, or the manifest.


def polarity_set(o: dict) -> str:
    pols = {s["polarity"] for s in o["spectra"]["scans"]}
    return "both" if pols == {"positive", "negative"} else pols.pop()


def nth_ms2_precursor(o: dict, n: int) -> float:
    return [s["precursor_mz"] for s in o["spectra"]["scans"] if s["ms_level"] == 2][n - 1]


def numerical_aperture(description: str) -> float:
    match = re.search(r"Numerical Aperture:\s*([\d.]+)", description)
    if not match:
        raise ValueError("no numerical aperture in the description")
    return float(match.group(1))


def gradient_percent_at(steps: list, time_min: float, channel: str) -> float:
    """%channel of the gradient step at `time_min` (facts: `[time, {channel: %}, flow]`)."""
    return next(comp[channel] for t, comp, _ in steps if abs(t - time_min) < 1e-6)


def gradient_first_time(steps: list, channel: str, percent: float) -> float:
    return next(t for t, comp, _ in steps if comp.get(channel) == percent)


def gradient_flow_ul_min(steps: list, unit: str) -> float:
    factor = {"\u00b5l/min": 1.0, "ul/min": 1.0, "nl/min": 0.001, "ml/min": 1000.0}[unit]
    flows = {round(f * factor, 6) for _, _, f in steps}
    if len(flows) != 1:
        raise ValueError(f"the gradient's flow varies: {flows}")
    return flows.pop()


def file_stem(path: str) -> str:
    last = re.split(r"[\\/]", path)[-1]
    return last.rsplit(".", 1)[0]


EXPERIMENT_SPECS: list[Spec] = [
    Spec(
        "exp-ms-raw-instrument-model",
        "mtbls1822-tsq-74",
        "instrument",
        "Which mass spectrometer model acquired this run?",
        lambda f, m: string(f["instrument_model"]),
        "facts: instrument_model (the depositor's mzML instrument-model term)",
        answer_hint="the instrument model",
    ),
    Spec(
        "exp-ms-raw-run-date",
        "mtbls755-hilic-dpoly",
        "acquisition-time",
        "On what date was this LC-MS run acquired?",
        lambda f, m: date(f["run_start"][:10], tolerance_days=1),
        "facts: run_start (the depositor's mzML run/@startTimeStamp); ±1 day for the time zone",
        answer_hint="the date as YYYY-MM-DD",
    ),
    Spec(
        "exp-ms-raw-polarity-switching",
        "mtbls755-hilic-dpoly",
        "method",
        "Was this run acquired in positive ion mode, negative ion mode, or both (polarity switching)?",
        lambda o, m: string(
            polarity_set(o),
            ["positive and negative", "negative and positive", "switching", "alternating"],
            reject=["only positive", "positive only", "only negative", "negative only"],
        ),
        "oracle: set of /spectra/scans/*/polarity (pyteomics on the depositor's mzML)",
        answer_hint="positive, negative or both",
    ),
    Spec(
        "exp-ms-raw-first-msms-precursor",
        "mtbls755-hilic-dpoly",
        "values",
        "This run interleaves full scans and MS/MS scans. What was the precursor m/z of the first MS/MS scan?",
        lambda o, m: number(round(first_ms2_precursor(o), 4), None, abs_=0.01),
        "oracle: first /spectra/scans[ms_level=2].precursor_mz (pyteomics on the depositor's mzML)",
        answer_hint="the m/z value",
    ),
    Spec(
        "exp-waters-sample-name",
        "mtbls3555-bv-ix-alpha-raw",
        "sample",
        "Under what sample name was this injection acquired?",
        lambda f, m: string(f["acquired_name"]),
        "facts: acquired_name (_HEADER.TXT `Acquired Name`)",
        stage_as="sample.raw",
        answer_hint="the sample name exactly as recorded",
    ),
    Spec(
        "exp-waters-vial",
        "mtbls15166-brain-b1-raw",
        "sample",
        "From which autosampler plate position (vial) was this sample injected?",
        lambda f, m: string(f["bottle_number"], ["2:A8", "2: A,8", "2, A8"]),
        "facts: bottle_number (_HEADER.TXT `Bottle Number`)",
        stage_as="sample.raw",
        answer_hint="the position as recorded",
    ),
    Spec(
        "exp-chemstation-vial",
        "entab-chemstation-mwd-d",
        "sample",
        "Which autosampler vial was injected for this run?",
        lambda f, m: integer(f["vial"]),
        "facts: vial (ChemStation result.ini `SampleLocation`)",
        stage_as="sample.D",
        answer_hint="the vial number",
    ),
    Spec(
        "exp-nmr-sample-title",
        "nmrxiv-s846-50",
        "sample",
        "Which sample does this NMR experiment record? Give the sample label from the experiment's title.",
        lambda f, m: string(f["title"]),
        "facts: title (TopSpin pdata/1/title, first line)",
        answer_hint="the sample label",
    ),
    Spec(
        "exp-nmr-pulse-program",
        "nmrxiv-s837-21",
        "method",
        "Which pulse program was used for this 2D NMR experiment?",
        lambda f, m: string(f["pulse_program"]),
        "facts: pulse_program (TopSpin acqus ##$PULPROG)",
        answer_hint="the pulse program name",
    ),
    Spec(
        "exp-nmr-operator",
        "nmrxiv-s846-50",
        "operator",
        "Which user account acquired this NMR spectrum?",
        lambda f, m: string(f["owner"]),
        "facts: owner (TopSpin acqus ##OWNER)",
        answer_hint="the user name",
    ),
    Spec(
        "exp-fcs-well",
        "flowio-g11",
        "sample",
        "Which plate well was this flow-cytometry sample acquired from?",
        lambda f, m: string(f["well_id"]),
        "facts: well_id (FCS TEXT $WELLID)",
        answer_hint="the well, e.g. B03",
    ),
    Spec(
        "exp-fcs-tube-name",
        "flowio-100715",
        "sample",
        "What tube name was this flow-cytometry acquisition recorded under?",
        lambda f, m: string(f["tube_name"], [f["tube_name"].replace("_", " ")]),
        "facts: tube_name (FCS TEXT `TUBE NAME`)",
        answer_hint="the tube name as recorded",
    ),
    Spec(
        "exp-fcs-operator",
        "fcsparser-fortessa-a01",
        "operator",
        "Who operated the cytometer for this acquisition?",
        lambda f, m: string(f["operator"], [re.sub(r"(?<=[a-z])(?=[A-Z])", " ", f["operator"])]),
        "facts: operator (FCS TEXT $OP)",
        answer_hint="the operator name as recorded",
    ),
    Spec(
        "exp-plate-barcode",
        "bmg-mars-abs-384-qc",
        "sample",
        "What is the plate ID (barcode) of this plate read?",
        lambda f, m: string(f["id1"]),
        "facts: id1 (BMG MARS CSV `ID1:`)",
        answer_hint="the plate ID",
    ),
    Spec(
        "exp-plate-protocol",
        "bmg-mars-abs-384-qc",
        "method",
        "Which test protocol was run on the plate reader?",
        lambda f, m: string(f["test_name"]),
        "facts: test_name (BMG MARS CSV `Test name:`)",
        answer_hint="the protocol name",
    ),
    Spec(
        "exp-ms-raw-software",
        "mtbls755-hilic-dpoly",
        "instrument",
        "Which acquisition software wrote this raw file?",
        lambda f, m: string(f["software"]),
        "facts: software (the depositor's mzML softwareList/software[0])",
        answer_hint="the software name",
    ),
    Spec(
        "exp-ms-raw-nth-msms-precursor",
        "mtbls755-hilic-dpoly",
        "values",
        "What is the precursor m/z of the 500th MS/MS scan of this run (counting MS/MS scans only, from the start)?",
        lambda o, m: number(round(nth_ms2_precursor(o, 500), 4), None, abs_=0.01),
        "oracle: /spectra/scans[ms_level=2][499].precursor_mz (pyteomics on the depositor's mzML)",
        answer_hint="the m/z value",
    ),
    Spec(
        "exp-ms-raw-gradient-solvent-b",
        "mtbls404-QC1_001",
        "method",
        "What was mobile phase B of the LC gradient used for this run?",
        lambda f, m: string(f["solvents"]["B"], ["ACN", "acetonitrile"]),
        "facts: solvents.B (the instrument method's pump text `Solvent B:`)",
        answer_hint="the solvent as recorded",
    ),
    Spec(
        "exp-ms-raw-gradient-full-b",
        "mtbls404-QC1_001",
        "method",
        "At what time after injection does the LC gradient reach 100% B?",
        lambda f, m: number(gradient_first_time(f["gradient_steps"], "B", 100.0), "min", abs_=0.01),
        "facts: gradient_steps (the pump's `Pump 1 gradient table:`), first row with B% = 100",
        answer_hint="the time in minutes",
    ),
    Spec(
        "exp-ms-raw-gradient-flow",
        "mtbls404-QC1_001",
        "method",
        "What was the LC flow rate of this run, in µL/min?",
        lambda f, m: number(gradient_flow_ul_min(f["gradient_steps"], f["gradient_flow_unit"]), None, rel=0.001),
        "facts: gradient_steps flow column (constant over the gradient)",
        answer_hint="the flow rate in µL/min",
    ),
    Spec(
        "exp-ms-raw-nlc-gradient-b",
        "pxd000001-tmt-erwinia-01",
        "method",
        "In the nano-LC gradient of this run, what percentage of solvent B is reached at 40 minutes?",
        lambda f, m: number(gradient_percent_at(f["gradient_steps"], 40.0, "B"), None, abs_=0.01),
        "facts: gradient_steps (the EASY-nLC method text `Gradient:` table, `Mixture [%B]` at 40:00)",
        answer_hint="the percentage of B",
    ),
    Spec(
        "exp-ms-raw-vanquish-gradient-b",
        "mtbls1820-lumos-uplc-31",
        "method",
        "In the LC gradient of this run, what percentage of solvent B is programmed at 6 minutes?",
        lambda f, m: number(gradient_percent_at(f["gradient_steps"], 6.0, "B"), None, abs_=0.01),
        "facts: gradient_steps (the Vanquish method script, `PumpModule.Pump.%B.Value` at 6.000 min)",
        answer_hint="the percentage of B",
    ),
    Spec(
        "exp-chemstation-oven-ramp",
        "mtbls75-x-fsfa-hl-gc-o7c-1-d",
        "method",
        "What was the GC oven temperature ramp rate?",
        lambda f, m: number(f["oven_ramp_c_per_min"], None, abs_=0.01),
        "facts: oven_ramp_c_per_min (ChemStation acqmeth.txt oven program)",
        stage_as="sample.D",
        answer_hint="the ramp rate in °C/min",
    ),
    Spec(
        "exp-chemstation-oven-final",
        "mtbls75-x-fsfa-hl-gc-o7c-1-d",
        "method",
        "To what final temperature was the GC oven programmed?",
        lambda f, m: number(f["oven_final_c"], "°C", abs_=0.5),
        "facts: oven_final_c (ChemStation acqmeth.txt oven program, last ramp target)",
        stage_as="sample.D",
        answer_hint="the temperature in °C",
    ),
    Spec(
        "exp-chemstation-msd-serial",
        "mtbls75-x-fsfa-hl-gc-o7c-1-d",
        "instrument",
        "What is the serial number of the mass-selective detector that acquired this GC-MS run?",
        lambda f, m: string(f["ms_serial"]),
        "facts: ms_serial (ChemStation acqmeth.txt `TUNE PARAMETERS for SN:`)",
        stage_as="sample.D",
        answer_hint="the serial number",
    ),
    Spec(
        "exp-shimadzu-sample-name",
        "zenodo13987390-chiral-11a-ad",
        "sample",
        "Under what sample name was this chromatogram acquired?",
        lambda f, m: string(f["sample_name"]),
        "facts: sample_name (File Property XML <smpl_name>)",
        answer_hint="the sample name exactly as recorded",
    ),
    Spec(
        "exp-shimadzu-method",
        "zenodo13950130-std-501",
        "method",
        "Which LabSolutions method file was used for this run? Give its name.",
        lambda f, m: string(file_stem(f["method_file"])),
        "facts: method_file (File Property XML <methodfile>), file name without folder and extension",
        answer_hint="the method name",
    ),
    Spec(
        "exp-shimadzu-vial",
        "zenodo13950130-std-501",
        "sample",
        "From which autosampler vial was this sample injected?",
        lambda f, m: integer(int(f["vial"])),
        "facts: vial (File Property XML <szVialNum>)",
        answer_hint="the vial number",
    ),
    Spec(
        "exp-fcs-comment",
        "fcsparser-cyflow-cube-8",
        "sample",
        "What comment was saved with this flow-cytometry acquisition?",
        lambda f, m: string(f["comment"], ["Original Data"]),
        "facts: comment (FCS TEXT $COM)",
        answer_hint="the comment as recorded",
    ),
    Spec(
        "exp-czi-operator",
        "zenodo10577621-LineScan-T3500",
        "operator",
        "Which user account acquired this image?",
        lambda f, m: string(f["user_name"]),
        "facts: user_name (CZI metadata XML Information/Document/UserName)",
        answer_hint="the user name",
    ),
    Spec(
        "exp-czi-microscope",
        "zenodo10577621-LineScan-T3500",
        "instrument",
        "Which microscope system acquired this image?",
        lambda f, m: string(f["microscope_system"], ["LSM 710", "LSM710"]),
        "facts: microscope_system (CZI metadata XML Information/Instrument/Microscopes/Microscope/System)",
        answer_hint="the microscope system",
    ),
    Spec(
        "exp-lif-system",
        "bsst749-2a-ishi-hf-fshr-dmso",
        "instrument",
        "Which Leica microscope system acquired these images?",
        lambda f, m: string(f["system_type"], ["SP5", "TCS-SP5"]),
        "facts: system_type (LIF XML scanner record SystemType)",
        answer_hint="the system name",
    ),
    Spec(
        "exp-mic-nd2-objective-optics",
        "ome-jonas-control002",
        "instrument",
        "Which objective magnification was used for this acquisition?",
        lambda o, m: string(objective_mag(pointer(o, "/text_info/optics"))),
        "oracle: /text_info/optics (the nd2 library's text info; this file's frame metadata names no objective)",
        answer_hint="the magnification, e.g. 40x",
    ),
    Spec(
        "exp-mic-nd2-objective-na",
        "ome-jonas-control002",
        "instrument",
        "What was the numerical aperture of the objective?",
        lambda o, m: number(numerical_aperture(pointer(o, "/text_info/description")), None, abs_=0.005),
        "oracle: /text_info/description, `Numerical Aperture:` line",
        answer_hint="the numerical aperture",
    ),
    Spec(
        "exp-mic-lif-modality",
        "bsst749-4i-bodipy-ctl",
        "method",
        "What kind of microscopy (imaging technique) produced this image?",
        lambda o, m: string(
            "confocal",
            ["laser scanning confocal", "LSCM", "CLSM", "confocal laser scanning"],
            reject=["widefield", "wide-field", "brightfield"],
        ),
        "manifest: notes (the depositor's description: small real confocal LIFs)",
        answer_hint="the imaging technique",
    ),
]

# ---------------------------------------------------------------- integrity questions

INTEGRITY_Q = (
    "This file was copied from the acquisition PC and the copy may have been interrupted. "
    "Is the file complete and intact?"
)

INTEGRITY: list[tuple[str, str, bool, str]] = [
    # (question id, corpus id, truncate?, why)
    ("int-czi-truncated", "zenodo7015307-T-3-Z-5-CH-2", True, "harness truncation"),
    ("int-nd2-truncated", "aics-ND2-jonas-header-test2", True, "harness truncation"),
    ("int-fcs-truncated", "flowio-g11", True, "harness truncation"),
    ("int-abf-truncated", "pyabf-171116sh-0011", True, "harness truncation"),
    ("int-mrc-truncated", "mrcfile-emd-3001", True, "harness truncation"),
    ("int-fcs-header-only", "fcsparser-cytek-nl-2000-header", False, "manifest role = corrupt"),
    ("int-czi-intact", "zenodo7015307-T-2-CH-1", False, "untouched copy (control)"),
]

# ---------------------------------------------------------------- conversion tasks

TASKS: list[Spec] = [
    Spec(
        "task-czi-ome-tiff-z-step",
        "zenodo7015307-T-2-Z-5-CH-2",
        "conversion",
        "Convert this file to OME-TIFF, saved as out.ome.tiff in the current directory, and tell me "
        "the Z step of the stack.",
        lambda o, m: number(pointer(o, "/images/0/physical_size_um/z"), "µm", rel=0.01),
        "oracle: /images/0/size_* and physical_size_um (czifile); output checked with tifffile",
        task={
            "output": "out.ome.tiff",
            "kind": "ome-tiff",
            "expect": {
                "SizeX": "/images/0/size_x",
                "SizeY": "/images/0/size_y",
                "SizeZ": "/images/0/size_z",
                "SizeC": "/images/0/size_c",
                "SizeT": "/images/0/size_t",
                "PhysicalSizeZ": "/images/0/physical_size_um/z",
            },
        },
        answer_hint="a number with its unit",
    ),
    Spec(
        "task-nd2-ome-tiff-channels",
        "aics-ND2-dims-p1z5t3c2y32x32",
        "conversion",
        "Convert this file to OME-TIFF, saved as out.ome.tiff in the current directory, and tell me "
        "how many channels the converted image has.",
        lambda o, m: integer(pointer(o, "/images/0/size_c")),
        "oracle: /images/0/size_* (nd2 package); output checked with tifffile",
        task={
            "output": "out.ome.tiff",
            "kind": "ome-tiff",
            "expect": {
                "SizeX": "/images/0/size_x",
                "SizeY": "/images/0/size_y",
                "SizeZ": "/images/0/size_z",
                "SizeC": "/images/0/size_c",
                "SizeT": "/images/0/size_t",
                "PhysicalSizeX": "/images/0/physical_size_um/x",
            },
        },
    ),
    Spec(
        "task-lif-ome-tiff-pixel-size",
        "ome-michael-PR2729-frameOrderCombinedScanTypes",
        "conversion",
        "Convert this file to OME-TIFF, saved as out.ome.tiff in the current directory, and tell me "
        "the pixel size in the XY plane.",
        lambda o, m: number(pixel_um(o), "µm", rel=0.01),
        "oracle: /images/0/size_* and physical_size_um (liffile); output checked with tifffile",
        task={
            "output": "out.ome.tiff",
            "kind": "ome-tiff",
            "expect": {
                "SizeX": "/images/0/size_x",
                "SizeY": "/images/0/size_y",
                "SizeZ": "/images/0/size_z",
                "SizeC": "/images/0/size_c",
                "SizeT": "/images/0/size_t",
                "PhysicalSizeX": "/images/0/physical_size_um/x",
            },
        },
        answer_hint="a number with its unit",
    ),
    Spec(
        "task-fcs-csv-events",
        "fcsparser-fortessa-a01",
        "conversion",
        "Export the events of this file to events.csv in the current directory (one row per event, "
        "one column per parameter) and tell me how many events it contains.",
        lambda o, m: integer(pointer(o, "/tables/0/event_count")),
        "oracle: /tables/0/event_count and parameter_count (flowio); output checked with Python csv",
        task={
            "output": "events.csv",
            "kind": "csv",
            "expect": {"rows": "/tables/0/event_count", "columns": "/tables/0/parameter_count"},
        },
    ),
    Spec(
        "task-abf-csv-sweep",
        "pyabf-2018-11-16-sh-0006",
        "conversion",
        "Export the first sweep of this recording to sweep.csv in the current directory (a time "
        "column plus the recorded signal) and tell me how many samples the sweep has.",
        lambda o, m: integer(pointer(o, "/traces/0/sweeps/0/sample_count")),
        "oracle: /traces/0/sweeps/0 sample_count and first samples (pyabf); output checked with Python csv",
        task={
            "output": "sweep.csv",
            "kind": "csv",
            "expect": {
                "rows": "/traces/0/sweeps/0/sample_count",
                "first_value": "/traces/0/sweeps/0/channels/0/first/0",
                "first_value_tolerance": 0.05,
            },
        },
    ),
]


# ---------------------------------------------------------------- build


def family_of(fmt: str) -> str:
    for fam, fmts in FAMILIES.items():
        if fmt in fmts:
            return fam
    raise KeyError(f"no family for format {fmt}")


def stage_name(filename: str, stage_as: str | None) -> str:
    base = filename.rstrip("/").rsplit("/", 1)[-1]
    if stage_as is None:
        return base
    if stage_as == "auto":
        return "sample" + suffix_of(base)
    return stage_as


def file_ref(manifest: dict, corpus_id: str, stage_as: str | None) -> dict:
    m = manifest_entry(manifest, corpus_id)
    ref = {
        "corpus_id": corpus_id,
        "path": m["filename"],
        "stage_as": stage_name(m["filename"], stage_as),
    }
    # Partial copies (e.g. a few wells of a screening plate): `check` rightly reports them
    # incomplete, which preflight must not flag. The manifest says so in its notes.
    if "partial copy" in m.get("notes", ""):
        ref["partial"] = True
    return ref


def hint_for(answer: dict) -> str:
    return {
        "number": "a number",
        "string": "a short answer",
        "boolean": "yes or no",
        "list": "the items, comma-separated",
        "date": "the date as YYYY-MM-DD",
        "bbox": "the box as x, y, width, height in pixels",
        "point": "the point as x, y in pixels",
        "choice": "one of the options",
    }[answer["type"]]


def build_question(manifest: dict, spec: Spec) -> dict:
    m = manifest_entry(manifest, spec.corpus_id)
    from_facts = spec.source.startswith("facts")
    if from_facts:
        facts_kind = spec.source.split(":", 1)[0]  # "facts", "facts-analysis", "facts-quant", ...
        facts_file = FACT_FILES.get(facts_kind, FACTS).name
        entry = load_facts(facts_kind)[spec.corpus_id]
        oracle = {k: v["value"] for k, v in entry["facts"].items()}
        # the visual tier's builders also read a fact's evidence (a ground-truth mask, file names)
        oracle |= {f"{k}__evidence": v["evidence"] for k, v in entry["facts"].items() if "evidence" in v}
    else:
        oracle = load_oracle(spec.corpus_id)
    answer = spec.answer(oracle, m)
    q: dict[str, Any] = {
        "id": spec.qid,
        "family": family_of(m["format"]),
        "format": m["format"],
        "category": spec.category,
        **({"subcategory": spec.subcategory} if spec.subcategory else {}),
        "file": file_ref(manifest, spec.corpus_id, spec.stage_as),
        "question": spec.question,
        "answer_hint": spec.answer_hint or hint_for(answer),
        "answer": answer,
        "source": f"corpus/oracle/{spec.corpus_id}.json ({oracle.get('reader', 'oracle')}) — {spec.source}"
        if spec.source.startswith("oracle")
        else f"evals/facts/{facts_file} [{spec.corpus_id}] ({entry['extractor']}; {entry['file']}) — {spec.source}"
        if from_facts
        else f"corpus/manifest.toml [{spec.corpus_id}] — {spec.source}",
    }
    if spec.extra:
        q["extra_files"] = [file_ref(manifest, cid, sa) for cid, sa in spec.extra]
    if spec.prepare:
        q["prepare"] = spec.prepare
    if spec.task:
        task = dict(spec.task)
        task["expect"] = {
            k: (pointer(oracle, v) if isinstance(v, str) and v.startswith("/") else v)
            for k, v in spec.task["expect"].items()
        }
        q["task"] = task
    return q


def build_integrity(manifest: dict) -> list[dict]:
    out = []
    for qid, cid, truncate, why in INTEGRITY:
        m = manifest_entry(manifest, cid)
        intact = not truncate and m.get("role") != "corrupt"
        q: dict[str, Any] = {
            "id": qid,
            "family": family_of(m["format"]),
            "format": m["format"],
            "category": "integrity",
            "file": file_ref(manifest, cid, "auto"),
            "question": INTEGRITY_Q,
            "answer_hint": "yes or no",
            "answer": boolean(intact),
            "source": (
                f"harness: the first {int(TRUNCATE_FRACTION * 100)} % of corpus file {cid} "
                f"(corpus/manifest.toml size {m.get('size', '?')} bytes), so incomplete by construction"
                if truncate
                else f"corpus/manifest.toml [{cid}] role = {m.get('role')!r}; notes: {m.get('notes', '')}"
                if m.get("role") == "corrupt"
                else f"corpus/manifest.toml [{cid}] untouched copy (sha256 checked by corpus fetch); {why}"
            ),
        }
        if truncate:
            q["prepare"] = {"truncate_fraction": TRUNCATE_FRACTION}
        out.append(q)
    return out


def build_all() -> list[dict]:
    manifest = load_manifest()
    questions = [build_question(manifest, s) for s in SPECS]
    questions += [build_question(manifest, s) for s in EXPERIMENT_SPECS]
    questions += build_integrity(manifest)
    questions += [build_question(manifest, s) for s in TASKS]
    questions += share.build_all()
    questions += [build_question(manifest, s) for s in __import__("analysis").ANALYSIS_SPECS]  # imports generate
    questions += [build_question(manifest, s) for s in __import__("regions").REGION_SPECS]  # whole-slide regions/levels
    questions += [build_question(manifest, s) for s in __import__("qpcr_facts").specs(__import__("generate"))]
    questions += [build_question(manifest, s) for s in __import__("quant").QUANT_SPECS]  # imports generate
    questions += [build_question(manifest, s) for s in __import__("openlab_facts").OPENLAB_SPECS]
    questions += [build_question(manifest, s) for s in __import__("spectroscopy").SPECTRO_SPECS]
    questions += [build_question(manifest, s) for s in __import__("bench").BENCH_SPECS]  # imports generate
    questions += __import__("heldout").build_all(manifest)  # split "heldout" (evals/heldout.py)
    questions += [build_question(manifest, s) for s in __import__("hcs_questions").HCS_SPECS]  # imports generate
    questions += __import__("batch").build_all()  # folders plus a sample sheet (evals/batch.py)
    questions += [build_question(manifest, s) for s in __import__("agent_facts").AGENT_SPECS]  # imports generate
    questions += __import__("agent_facts").build_batch(manifest)  # an analysis command over a folder
    questions += [build_question(manifest, s) for s in __import__("routing_facts").ROUTING_SPECS]
    questions += __import__("routing_facts").build_batch(manifest)  # folder MIPs, raw/mzXML pairing
    questions += __import__("scenario_facts").build_all()  # end-to-end lab requests (evals/scenario_facts.py)
    questions += [build_question(manifest, s) for s in __import__("gaps_facts").GAPS_SPECS]
    questions += __import__("gaps_facts").build_batch(manifest)  # spectra folder with a sheet and README
    questions += [build_question(manifest, s) for s in __import__("visual").VISUAL_SPECS]  # look, then answer
    ids = [q["id"] for q in questions]
    dupes = {i for i in ids if ids.count(i) > 1}
    if dupes:
        raise SystemExit(f"duplicate question ids: {sorted(dupes)}")
    for q in questions:
        if q["category"] not in CATEGORIES:
            raise SystemExit(f"{q['id']}: unknown category {q['category']}")
        prefix = id_prefix(q)
        if prefix and not q["id"].startswith(prefix):
            raise SystemExit(f"{q['id']}: a {q['category']} question's id starts with {prefix!r} (README.md)")
        q.setdefault("split", split_of(q))  # held-out questions arrive with split "heldout"
    return questions


# The salt was chosen once (2026-09-23), among three, as the one that puts every family in both
# splits; never change it, or questions move between splits and the sealed set is no longer sealed.
SPLIT_SALT = "icli-split-1"
DEV_PERCENT = 60


def split_of(q: dict) -> str:
    """`dev` (look at it, optimize against it) or `test` (sealed: only aggregate scores are read).

    Keyed by the question's file, not its id, so every question about one file lands in the same
    split and fixing a failure on a dev file cannot quietly fix a test question about that file.
    A hash, not a list, so adding questions never moves existing ones.
    """
    key = q["id"] if "share" in q else q["file"]["corpus_id"]
    h = int(hashlib.sha256(f"{SPLIT_SALT}:{key}".encode()).hexdigest(), 16) % 100
    return "dev" if h < DEV_PERCENT else "test"


def render(questions: list[dict]) -> dict[str, str]:
    """One JSONL file per family; integrity and conversion questions get files of their own."""
    files: dict[str, list[str]] = {}
    for q in questions:
        own_file = ("integrity", "conversion", "search", "analysis", "quantitation", "batch", "scenario", "visual")
        name = q["category"] if q["category"] in own_file else q["family"]
        if q.get("split") == "heldout":
            name = "heldout"  # a file of their own: never mixed into the dev/test families
        files.setdefault(f"{name}.jsonl", []).append(json.dumps(q, ensure_ascii=False, sort_keys=False))
    return {name: "\n".join(lines) + "\n" for name, lines in sorted(files.items())}


def main() -> int:
    ap_ = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap_.add_argument("--check", action="store_true", help="exit 1 if evals/questions/ is out of date")
    args = ap_.parse_args()
    rendered = render(build_all())
    if args.check:
        stale = [n for n, text in rendered.items() if not (OUT_DIR / n).exists() or (OUT_DIR / n).read_text() != text]
        extra = [p.name for p in OUT_DIR.glob("*.jsonl") if p.name not in rendered]
        if stale or extra:
            print(
                f"evals/questions is out of date: {sorted(stale + extra)}; run evals/generate.py",
                file=sys.stderr,
            )
            return 1
        print("evals/questions is up to date")
        import readme_counts  # the README's question counts follow the files

        return readme_counts.main(["--check"])
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for old in OUT_DIR.glob("*.jsonl"):
        if old.name not in rendered:
            old.unlink()
    total = 0
    for name, text in rendered.items():
        (OUT_DIR / name).write_text(text)
        n = text.count("\n")
        total += n
        print(f"{name}: {n}")
    print(f"total: {total}")
    import readme_counts  # the README's question counts follow the files

    return readme_counts.main([])


if __name__ == "__main__":
    raise SystemExit(main())
