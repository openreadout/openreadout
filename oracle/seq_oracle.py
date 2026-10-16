#!/usr/bin/env python
"""Ground truth for DNA sequencer output (`illumina-run`, `pod5`, `fast5`, `fastq`, `bam`) from
independent readers, run as black boxes:

- Illumina run folders: the `interop` package (Illumina, GPL-3.0) for the run summary (per read and
  lane: tiles, density, % PF, reads, % >= Q30, yield, error rate, % aligned, phasing, first-cycle
  intensity, % occupied), and, when `--picard JAR` is given, Picard `IlluminaBasecallsToFastq`
  (MIT) for FASTQ of lane 1 (pass-filter clusters, no EAMSS masking, file order): per read the
  record count and the xxh3 of the first 2000 records as `name\\nsequence\\nquality\\n` (name up to
  the first space).
- POD5: the `pod5` package (ONT, MPL-2.0): per read id, channel, well, read number, start, sample
  count, calibration, end reason, median before, and the signal in pA (sum, min, max, first 32).
- FAST5: h5py (BSD-3) with `vbz_h5py_plugin` (ONT, MPL-2.0): per read the raw ADC samples (sum,
  min, max, first 32), the read and channel attributes.
- FASTQ: pysam `FastxFile` (MIT): count, bases, lengths, N50, bases >= Q30, GC, and per read
  length and mean quality at sampled reads; Biopython (BSD-3) counts the records again.
- BAM: pysam `AlignmentFile`: records, primary records, mapped primaries, bases, N50, and per record
  flag, MAPQ, position, length, `np` and `rq` at sampled records.

Usage:  python seq_oracle.py [--out DIR] [--picard JAR] --id ID --format FMT PATH

Writes `<DIR>/<ID>.json` (default corpus/oracle/seq); compared by
`crates/openreadout-corpus-tests/tests/corpus/seq_oracle/mod.rs`.

MiSeq run folders without RunParameters.xml: `interop` refuses them ("RunParameters.xml required
for legacy run folders with missing channel names"), so the InterOp directory and RunInfo.xml are
copied to a temporary folder with a minimal runParameters.xml naming the MiSeq control software.
"""
from __future__ import annotations

import json
import math
import shutil
import subprocess
import sys
import tempfile
from importlib.metadata import version
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "seq"
FASTQ_RECORDS = 2000


def _f(x):
    if x is None:
        return None
    x = float(x)
    return None if math.isnan(x) or math.isinf(x) else x


def sample_indices(n: int, k: int = 64) -> list[int]:
    if n <= 0:
        return []
    first = list(range(min(n, 16)))
    spread = {round(j * (n - 1) / (k - 1)) for j in range(k)} if n > 1 else {0}
    return sorted(set(first) | spread)


def n50(lengths: list[int]) -> int | None:
    if not lengths:
        return None
    s = sorted(lengths, reverse=True)
    half, acc = (sum(s) + 1) // 2, 0
    for x in s:
        acc += x
        if acc >= half:
            return x
    return None


def mean_q(quals) -> float | None:
    if quals is None or len(quals) == 0:
        return None
    err = sum(10 ** (-q / 10) for q in quals) / len(quals)
    return -10 * math.log10(max(err, 1e-10))


# ---------------------------------------------------------------- Illumina run folders

def illumina(path: Path, picard: str | None) -> dict:
    from interop import py_interop_run, py_interop_run_metrics, py_interop_summary

    run_dir = path
    tmp = None
    if not any((path / n).exists() for n in ("RunParameters.xml", "runParameters.xml")):
        tmp = Path(tempfile.mkdtemp())
        shutil.copytree(path / "InterOp", tmp / "InterOp")
        shutil.copy(path / "RunInfo.xml", tmp / "RunInfo.xml")
        (tmp / "runParameters.xml").write_text(
            '<?xml version="1.0"?>\n<RunParameters><Setup><ApplicationName>MiSeq Control Software'
            "</ApplicationName></Setup></RunParameters>\n"
        )
        run_dir = tmp
    rm = py_interop_run_metrics.run_metrics()
    vf = py_interop_run.uchar_vector(py_interop_run.MetricCount, 0)
    py_interop_run_metrics.list_summary_metrics_to_load(vf)
    rm.read(str(run_dir), vf)
    s = py_interop_summary.run_summary()
    py_interop_summary.summarize_run_metrics(rm, s)
    t = s.total_summary()
    reads = []
    for r in range(s.size()):
        rs = s.at(r)
        ri = rs.read()
        lanes = []
        for k in range(rs.size()):
            ls = rs.at(k)
            lanes.append({
                "lane": ls.lane(),
                "tiles": ls.tile_count(),
                "density": _f(ls.density().mean()),
                "density_sd": _f(ls.density().stddev()),
                "percent_pf": _f(ls.percent_pf().mean()),
                "percent_pf_sd": _f(ls.percent_pf().stddev()),
                "reads": _f(ls.reads()),
                "reads_pf": _f(ls.reads_pf()),
                "percent_q30": _f(ls.percent_gt_q30()),
                "yield_g": _f(ls.yield_g()),
                "error_rate": _f(ls.error_rate().mean()),
                "percent_aligned": _f(ls.percent_aligned().mean()),
                "intensity_cycle_1": _f(ls.first_cycle_intensity().mean()),
                "phasing": _f(ls.phasing().mean()),
                "prephasing": _f(ls.prephasing().mean()),
                "percent_occupied": _f(ls.percent_occupied().mean()),
            })
        reads.append({
            "read": ri.number(),
            "indexed": bool(ri.is_index()),
            "cycles": ri.total_cycles(),
            "percent_q30": _f(rs.summary().percent_gt_q30()),
            "yield_g": _f(rs.summary().yield_g()),
            "lanes": lanes,
        })
    out = {
        "reader": f"interop {version('interop')} (GPL-3.0, run as a black box)",
        "total": {
            "yield_g": _f(t.yield_g()),
            "percent_q30": _f(t.percent_gt_q30()),
        },
        "reads": reads,
    }
    if tmp:
        shutil.rmtree(tmp)
        out["note"] = "run without RunParameters.xml: interop read a copy with a minimal runParameters.xml"
    if picard:
        out["fastq"] = picard_fastq(path, picard)
    return out


def picard_fastq(path: Path, jar: str) -> dict:
    import xml.etree.ElementTree as ET
    import xxhash

    ri = ET.parse(path / "RunInfo.xml").getroot().find("Run")
    structure = "".join(
        f"{r.get('NumCycles')}{'B' if r.get('IsIndexedRead') == 'Y' else 'T'}"
        for r in ri.find("Reads")
    )
    with tempfile.TemporaryDirectory() as tmp:
        cmd = [
            "java", "-jar", jar, "IlluminaBasecallsToFastq",
            f"BASECALLS_DIR={path / 'Data' / 'Intensities' / 'BaseCalls'}", "LANE=1",
            f"READ_STRUCTURE={structure}", f"RUN_BARCODE={ri.get('Number')}",
            f"MACHINE_NAME={ri.findtext('Instrument')}", f"FLOWCELL_BARCODE={ri.findtext('Flowcell')}",
            f"OUTPUT_PREFIX={tmp}/out", "READ_NAME_FORMAT=CASAVA_1_8", "APPLY_EAMSS_FILTER=false",
            "INCLUDE_NON_PF_READS=false", "SORT=false",
        ]
        subprocess.run(cmd, check=True, capture_output=True)
        files = sorted(Path(tmp).glob("out.*.fastq"))
        labels = {}
        for f in files:
            # out.1.fastq, out.2.fastq (template reads); out.barcode_1.fastq (index reads)
            part = f.name[len("out."):-len(".fastq")]
            label = f"I{part.split('_')[1]}" if part.startswith("barcode_") else f"R{part}"
            h = xxhash.xxh3_128()
            n = 0
            first = []
            with open(f, "rb") as fh:
                while True:
                    lines = [fh.readline() for _ in range(4)]
                    if not lines[0]:
                        break
                    if n < FASTQ_RECORDS:
                        name = lines[0][1:].split(b" ")[0].strip()
                        rec = name + b"\n" + lines[1].strip() + b"\n" + lines[3].strip() + b"\n"
                        h.update(rec)
                        if n < 3:
                            first.append(rec.decode())
                    n += 1
            labels[label] = {"records": n, "first_records_xxh3": h.hexdigest(), "first": first}
        return {
            "reader": "Picard 3.5.0 IlluminaBasecallsToFastq (MIT)",
            "lane": 1,
            "hashed_records": FASTQ_RECORDS,
            "reads": labels,
        }


# ---------------------------------------------------------------- nanopore

def pod5_(path: Path) -> dict:
    import numpy as np
    import pod5

    reads = []
    run = None
    with pod5.Reader(path) as r:
        for rec in r.reads():
            sig = rec.signal_pa.astype(np.float64)
            reads.append({
                "read_id": str(rec.read_id),
                "channel": rec.pore.channel,
                "well": rec.pore.well,
                "read_number": rec.read_number,
                "start_sample": rec.start_sample,
                "num_samples": rec.num_samples,
                "calibration_offset": _f(rec.calibration.offset),
                "calibration_scale": _f(rec.calibration.scale),
                "end_reason": rec.end_reason.name,
                "median_before": _f(rec.median_before),
                "signal_sum": float(sig.sum()),
                "signal_min": float(sig.min()) if len(sig) else None,
                "signal_max": float(sig.max()) if len(sig) else None,
                "signal_first": [float(x) for x in sig[:32]],
            })
            if run is None:
                ri = rec.run_info
                run = {
                    "acquisition_id": ri.acquisition_id,
                    "sample_rate": ri.sample_rate,
                    "flow_cell_id": ri.flow_cell_id,
                    "flow_cell_product_code": ri.flow_cell_product_code,
                    "sequencing_kit": ri.sequencing_kit,
                    "sample_id": ri.sample_id,
                    "system_type": ri.system_type,
                    "experiment_name": ri.experiment_name,
                    "acquisition_start_time": ri.acquisition_start_time.isoformat(),
                }
    return {"reader": f"pod5 {version('pod5')} (MPL-2.0, run as a black box)", "run": run, "reads": reads}


def fast5(path: Path) -> dict:
    import h5py
    import numpy as np
    import vbz_h5py_plugin  # noqa: F401  (registers the VBZ filter)

    def text(v):
        if isinstance(v, bytes):
            return v.decode(errors="replace").rstrip("\0")
        if isinstance(v, np.generic):
            return v.item()
        return v

    reads = []
    with h5py.File(path, "r") as f:
        groups = [k for k in f.keys() if k.startswith("read_")]
        if groups:
            items = [(f[g]["Raw"], f[g]["Raw/Signal"], f[g]["channel_id"]) for g in groups]
        else:
            rr = f["Raw/Reads"]
            items = [(rr[k], rr[k]["Signal"], f["UniqueGlobalKey/channel_id"]) for k in rr.keys()]
        for raw, sig, ch in items:
            s = sig[:].astype(np.int64)
            reads.append({
                "read_id": text(raw.attrs.get("read_id")),
                "read_number": text(raw.attrs.get("read_number")),
                "start_sample": text(raw.attrs.get("start_time")),
                "num_samples": int(len(s)),
                "median_before": _f(raw.attrs.get("median_before")),
                "channel": int(text(ch.attrs.get("channel_number"))),
                "digitisation": _f(ch.attrs.get("digitisation")),
                "offset": _f(ch.attrs.get("offset")),
                "range": _f(ch.attrs.get("range")),
                "sampling_rate": _f(ch.attrs.get("sampling_rate")),
                "raw_sum": int(s.sum()),
                "raw_min": int(s.min()) if len(s) else None,
                "raw_max": int(s.max()) if len(s) else None,
                "raw_first": [int(x) for x in s[:32]],
            })
    return {"reader": f"h5py {version('h5py')} (BSD-3) with vbz_h5py_plugin (MPL-2.0)", "reads": reads}


# ---------------------------------------------------------------- reads

def fastq(path: Path) -> dict:
    import pysam
    from Bio import SeqIO

    lengths, quals, gcs = [], [], []
    bases = q30 = gc = acgt = 0
    with pysam.FastxFile(str(path)) as fh:
        for rec in fh:
            seq = rec.sequence
            q = rec.get_quality_array()
            lengths.append(len(seq))
            bases += len(seq)
            q30 += sum(1 for x in q if x >= 30)
            up = seq.upper()
            g = up.count("G") + up.count("C")
            a = g + up.count("A") + up.count("T")
            gc += g
            acgt += a
            quals.append(mean_q(q))
            gcs.append(g / a * 100 if a else None)
    import gzip
    opener = gzip.open if path.name.endswith(".gz") else open
    with opener(path, "rt") as fh:
        bio_count = sum(1 for _ in SeqIO.parse(fh, "fastq"))
    idx = sample_indices(len(lengths))
    return {
        "reader": f"pysam {version('pysam')} (MIT); Biopython {version('biopython')} (BSD-3) counts",
        "reads": len(lengths),
        "reads_biopython": bio_count,
        "bases": bases,
        "length_min": min(lengths) if lengths else None,
        "length_max": max(lengths) if lengths else None,
        "n50": n50(lengths),
        "q30_bases": q30,
        "percent_gc": gc / acgt * 100 if acgt else None,
        "sampled": [{"i": i, "length": lengths[i], "mean_quality": quals[i], "gc_percent": gcs[i]} for i in idx],
    }


def bam(path: Path) -> dict:
    import pysam

    recs = []
    lengths = []
    primary = mapped = 0
    with pysam.AlignmentFile(str(path), "rb", check_sq=False) as fh:
        header = fh.header.to_dict()
        for a in fh.fetch(until_eof=True):
            q = a.query_qualities
            tags = dict(a.get_tags())
            recs.append({
                "flag": a.flag,
                "mapq": a.mapping_quality,
                "ref_id": a.reference_id,
                "pos": a.reference_start + 1 if a.reference_start >= 0 else None,
                "length": a.query_length or len(a.query_sequence or ""),
                "mean_quality": mean_q(list(q)) if q is not None else None,
                "passes": tags.get("np"),
                "accuracy": _f(tags.get("rq")),
            })
            if not (a.is_secondary or a.is_supplementary):
                primary += 1
                lengths.append(len(a.query_sequence or ""))
                if not a.is_unmapped:
                    mapped += 1
    idx = sample_indices(len(recs))
    return {
        "reader": f"pysam {version('pysam')} (MIT)",
        "records": len(recs),
        "primary": primary,
        "mapped": mapped,
        "bases": sum(lengths),
        "n50": n50(lengths),
        "sort_order": header.get("HD", {}).get("SO"),
        "platform": next((rg.get("PL") for rg in header.get("RG", [])), None),
        "references": len(header.get("SQ", [])),
        "sampled": [dict(i=i, **recs[i]) for i in idx],
    }


def main(argv: list[str]) -> int:
    out_dir, ident, fmt, picard, files = OUT, None, None, None, []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        elif a == "--format":
            fmt = next(it)
        elif a == "--picard":
            picard = next(it)
        else:
            files.append(Path(a))
    if len(files) != 1 or not ident or not fmt:
        print(__doc__, file=sys.stderr)
        return 2
    p = files[0]
    try:
        body = {
            "illumina-run": lambda: illumina(p, picard),
            "pod5": lambda: pod5_(p),
            "fast5": lambda: fast5(p),
            "fastq": lambda: fastq(p),
            "bam": lambda: bam(p),
        }[fmt]()
    except Exception as e:  # recorded so the comparison reports it instead of skipping
        body = {"error": f"{type(e).__name__}: {e}"}
    out = {"id": ident, "format": fmt, **body}
    out_dir.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(out, indent=1) + "\n")
    print(ident, fmt, "error" if "error" in out else "ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
