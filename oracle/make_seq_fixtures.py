#!/usr/bin/env python
"""Committed test fixtures of the sequencing readers (`crates/openreadout-seq/tests/fixtures/`),
made from the synthetic fuzz seeds that `fuzz/seeds.py` (`sequencing_seeds`) writes: two tiny
Illumina run folders (BCL and CBCL, unpacked from their seed bundles), FASTQ (plain and gzip),
BAM, a one-read POD5 written by the `pod5` package, and a one-read FAST5 written here with h5py.

Usage:  python make_seq_fixtures.py   (after `python fuzz/seeds.py` or its `sequencing_seeds()`)
"""
from __future__ import annotations

import shutil
from pathlib import Path

HERE = Path(__file__).resolve().parent
SEEDS = HERE.parent / "fuzz" / "corpus"
OUT = HERE.parent / "crates" / "openreadout-seq" / "tests" / "fixtures"
SEP = b"\n=====FUZZ-NEXT-FILE=====\n"

BCL = ["RunInfo.xml", "RunParameters.xml", "InterOp/TileMetricsOut.bin", "InterOp/QMetricsOut.bin",
       "InterOp/ErrorMetricsOut.bin", "InterOp/ExtractionMetricsOut.bin", "InterOp/CorrectedIntMetricsOut.bin",
       "InterOp/ExtendedTileMetricsOut.bin", "InterOp/IndexMetricsOut.bin",
       "Data/Intensities/BaseCalls/L001/s_1_1101.filter", "Data/Intensities/BaseCalls/L001/C1.1/s_1_1101.bcl",
       "Data/Intensities/BaseCalls/L001/C2.1/s_1_1101.bcl", "Data/Intensities/L001/s_1_1101.locs"]
CBCL = ["RunInfo.xml", "InterOp/TileMetricsOut.bin", "InterOp/QMetricsOut.bin",
        "Data/Intensities/BaseCalls/L001/s_1_2101.filter", "Data/Intensities/BaseCalls/L001/C1.1/L001_2.cbcl",
        "Data/Intensities/BaseCalls/L001/C2.1/L001_2.cbcl", "Data/Intensities/s.locs"]


def unbundle(seed: Path, names: list[str], out: Path) -> None:
    parts = seed.read_bytes().split(SEP)
    for name, part in zip(names, parts):
        p = out / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(part)
    (out / "RTAComplete.txt").write_text("synthetic\n")


def fast5(path: Path) -> None:
    import h5py
    import numpy as np

    with h5py.File(path, "w") as f:
        f.attrs["file_version"] = np.bytes_(b"2.2")
        g = f.create_group("read_00000000-0000-0000-0000-000000000001")
        raw = g.create_group("Raw")
        raw.create_dataset("Signal", data=(np.arange(40, dtype=np.int16) * 5) % 211, compression="gzip")
        for k, v in {"read_id": np.bytes_(b"00000000-0000-0000-0000-000000000001"), "read_number": np.int32(7),
                     "start_time": np.uint64(4000), "duration": np.uint32(40), "median_before": np.float64(210.5),
                     "start_mux": np.uint8(2)}.items():
            raw.attrs[k] = v
        ch = g.create_group("channel_id")
        for k, v in {"channel_number": np.bytes_(b"12"), "digitisation": np.float64(8192.0), "offset": np.float64(10.0),
                     "range": np.float64(1400.0), "sampling_rate": np.float64(4000.0)}.items():
            ch.attrs[k] = v
        t = g.create_group("tracking_id")
        for k, v in {"run_id": b"r1", "device_type": b"minion", "flow_cell_id": b"FAA00000", "sample_id": b"synthetic",
                     "exp_start_time": b"2024-01-01T00:00:00Z", "version": b"1.0"}.items():
            t.attrs[k] = np.bytes_(v)
        c = g.create_group("context_tags")
        c.attrs["sequencing_kit"] = np.bytes_(b"sqk-lsk114")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, names, seed in [("run-bcl", BCL, "whole_illumina_bcl/mini.bundle"), ("run-cbcl", CBCL, "whole_illumina_cbcl/mini.bundle")]:
        d = OUT / name
        shutil.rmtree(d, ignore_errors=True)
        unbundle(SEEDS / seed, names, d)
    shutil.copy(SEEDS / "whole_fastq" / "mini.fastq", OUT / "mini.fastq")
    shutil.copy(SEEDS / "whole_fastq" / "mini.fastq.gz", OUT / "mini.fastq.gz")
    shutil.copy(SEEDS / "whole_bam" / "mini.bam", OUT / "mini.bam")
    if (SEEDS / "whole_pod5" / "mini.pod5").exists():
        shutil.copy(SEEDS / "whole_pod5" / "mini.pod5", OUT / "mini.pod5")
    fast5(OUT / "mini.fast5")
    print("wrote", sorted(p.name for p in OUT.iterdir()))


if __name__ == "__main__":
    main()
