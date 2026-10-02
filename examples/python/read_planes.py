#!/usr/bin/env python3
"""read_planes.py: read planes from an instrument file with the `openreadout` Python package.

Usage:   python examples/python/read_planes.py FILE [--image N]
Needs:   pip install openreadout        (NumPy only; see https://openreadout.github.io/openreadout/guides/python.html for wheels and building)
Output:  the file's images with their dimensions, then for the chosen image: per-channel
         statistics of the middle z-plane at t=0, and a maximum-intensity projection over z
         of channel 0 computed plane by plane (only one plane is in memory at a time).

The package reads the same files as the CLI with the same Rust code, returns metadata as the
same dicts `openreadout info --json` prints, and returns pixels as NumPy arrays in the file's
own dtype. Nothing is converted or written.
"""
from __future__ import annotations

import argparse
import sys

import numpy as np

import openreadout


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("file")
    ap.add_argument("--image", type=int, default=0, help="image index (see `openreadout info`)")
    args = ap.parse_args()

    try:
        f = openreadout.File(args.file)
    except openreadout.OpenReadoutError as e:
        # Same code, exit code and hint as the CLI's JSON error envelope (the message includes the hint).
        print(f"error ({e.code}, exit {e.exit_code}): {e}", file=sys.stderr)
        return e.exit_code

    with f:
        print(f"{f.path}: {f.format}, {len(f.images)} image(s)")
        for i, im in enumerate(f.images):
            px = im.get("physical_size", {}).get("x")  # absent when the file does not record it
            px_text = f", {px:.4f} µm/px" if px else ""
            print(f"  [{i}] {f.dims(i)} {f.shape(i)} {f.dtype(i)}{px_text}  {im.get('name', '')}")

        im = f.images[args.image]
        z_mid, t0 = im["size_z"] // 2, 0
        names = [ch.get("name", f"ch{ch['index']}") for ch in im.get("channels", [])]
        print(f"\nimage {args.image}, z={z_mid}, t={t0}:")
        for c in range(im["size_c"]):
            plane = f.read_plane(image=args.image, c=c, z=z_mid, t=t0)  # (Y, X) or (Y, X, S)
            name = names[c] if c < len(names) else f"c{c}"
            print(
                f"  c={c} {name:<16} shape={plane.shape} min={plane.min()} max={plane.max()} "
                f"mean={plane.mean():.2f} p99={np.percentile(plane, 99):.0f}"
            )

        mip = None
        for z in range(im["size_z"]):
            plane = f.read_plane(image=args.image, c=0, z=z, t=t0)
            mip = plane if mip is None else np.maximum(mip, plane)
        assert mip is not None
        print(f"\nmax projection of c=0 over {im['size_z']} z-plane(s): shape={mip.shape} max={mip.max()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
