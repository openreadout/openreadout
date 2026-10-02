#!/usr/bin/env python
"""Oracle JSON for the two synthetic timsTOF datasets of timsrust (`timsrust-test-dda`,
`timsrust-test-dia`), transcribed from the expected values in timsrust's own test suite
(tests/frame_readers.rs at commit 80e235a, Apache-2.0). The released timsrust 0.6.6 cannot open
these files itself (it looks for a table named `GlobalMetadata`; they spell it
`GlobalMetaData`), so oracle/timsrust-oracle is not run on them.

Usage: uv run python timsrust_expected.py   (writes ../corpus/oracle/timsrust-test-*.json)
"""
import json
from pathlib import Path

import numpy as np
import xxhash

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle"


def h(a, dtype):
    return xxhash.xxh3_128_hexdigest(np.asarray(a, dtype=dtype).tobytes())


def frame(fid, offsets, tofs):
    ints = [(x + 1) * 2 for x in tofs]
    return {
        "id": fid, "scans": len(offsets) - 1, "peaks": len(tofs),
        "xxh3_scan_offsets": h(offsets, "<u8"), "xxh3_tof": h(tofs, "<u4"), "xxh3_intensity": h(ints, "<u4"),
    }


SOURCE = "timsrust tests/frame_readers.rs (commit 80e235a): expected frames, transcribed"

dda = {
    "id": "timsrust-test-dda",
    "reader": SOURCE,
    "images": [],
    "tdf": {"frames": [
        frame(1, [0, 1, 3, 6, 10], list(range(0, 10))),
        frame(2, [0, 5, 11, 18, 26], list(range(10, 36))),
        frame(3, [0, 9, 19, 30, 42], list(range(36, 78))),
        frame(4, [0, 13, 27, 42, 58], list(range(78, 136))),
    ]},
}

# DIA: timsrust asserts 709 scans, the first TOF index / intensity and the peak count of each MS2 frame
dia = {
    "id": "timsrust-test-dia",
    "reader": SOURCE,
    "images": [],
    "tdf": {"frames": [
        {"id": fid, "scans": 709, "peaks": n, "first_tof": t, "first_intensity": i}
        for fid, t, i, n in [
            (2, 251695, 503392, 754376),
            (3, 1006071, 2012144, 1257057),
            (5, 4022866, 8045734, 2262419),
            (6, 6285285, 12570572, 2765100),
        ]
    ]},
}

for d in (dda, dia):
    (OUT / f"{d['id']}.json").write_text(json.dumps(d, indent=1) + "\n")
    print("wrote", d["id"])
