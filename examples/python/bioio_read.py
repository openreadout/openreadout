#!/usr/bin/env python3
"""bioio_read.py: open an instrument file with bioio, two ways.

Usage:   python examples/python/bioio_read.py FILE [--via-export]
Needs:   default:        pip install bioio bioio-openreadout     (the reader plugin)
         --via-export:   pip install bioio bioio-ome-tiff           (plus the openreadout binary)
Output:  scenes, dimensions, channel names, physical pixel sizes, and the mean of one
         Z-stack (T=0, C=0), read lazily through dask.

Route 1 (default) uses the `bioio-openreadout` plugin: bioio reads the raw CZI/ND2/LIF file
directly through the OpenReadout Rust core; no conversion, no GPL dependency.
Route 2 (--via-export) is for environments that already standardize on OME-TIFF: the CLI
converts the file (verified by reading it back) and bioio opens the OME-TIFF with its own
plugin. Both routes should print the same numbers; comparing them is a cheap sanity check.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile

from bioio import BioImage


def describe(img: BioImage, label: str) -> None:
    print(f"[{label}] scenes: {list(img.scenes)}")
    print(f"[{label}] dims: {img.dims}  dtype: {img.dtype}")
    print(f"[{label}] channels: {[str(c) for c in img.channel_names]}")
    print(f"[{label}] physical pixel sizes (µm): {img.physical_pixel_sizes}")
    stack = img.get_image_dask_data("ZYX", T=0, C=0)  # lazy: nothing decoded yet
    print(f"[{label}] ZYX stack T=0 C=0: shape={stack.shape} mean={float(stack.mean().compute()):.3f}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("file")
    ap.add_argument("--via-export", action="store_true", help="convert with the CLI, then open the OME-TIFF")
    ap.add_argument("--openreadout", default=os.environ.get("OPENREADOUT", "openreadout"))
    args = ap.parse_args()

    if not args.via_export:
        import bioio_openreadout

        describe(BioImage(args.file, reader=bioio_openreadout.Reader), "bioio-openreadout")
        return 0

    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "export.ome.tiff")
        # Export the first image only; add "--select", "t=0" etc. to shrink big files.
        cmd = [args.openreadout, "export", args.file, "-o", out, "--image", "0", "--json"]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        envelope = json.loads(proc.stdout)
        if not envelope["ok"]:
            err = envelope["error"]
            print(f"export failed ({err['code']}): {err['message']}\nhint: {err.get('hint')}", file=sys.stderr)
            return proc.returncode
        report = envelope["data"]
        print(f"exported {report['planes_written']} planes, verified={report['verified']}")
        describe(BioImage(out), "bioio-ome-tiff")
    return 0


if __name__ == "__main__":
    sys.exit(main())
