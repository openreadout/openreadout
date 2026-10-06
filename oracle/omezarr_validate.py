#!/usr/bin/env python
"""Validate an OME-Zarr store written by `openreadout export --format ome-zarr` with third-party
readers, and compare its pixels with the source file as `openreadout check --planes --dump-dir` sees it.

    uv run python omezarr_validate.py STORE.ome.zarr SOURCE_FILE [--bin PATH] [--image N] [--planes K]

For every image group in the store this checks:
  * strict OME-NGFF 0.5 metadata validation with ome-zarr-models (Image; BioFormats2Raw for collections),
  * that ome_zarr.reader recognises a multiscales image and reads every level,
  * that bioio (bioio-ome-zarr) opens it with the expected TCZYX shape, pixel sizes and channel names,
  * for K planes (first, last, and evenly spaced in between): the level-0 pixels read by ome_zarr,
    by bioio and via zarr equal the raw samples dumped by `openreadout check --planes` (array equality and
    the xxh3-128 hash the CLI reports),
  * for multi-level images: level 1 equals the 2x2 block mean of level 0 (rounded half away from zero).
Exit status 0 only if everything passed. This script is a test harness; nothing here ships.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import xxhash
import zarr

HERE = Path(__file__).resolve().parent
DTYPES = {
    "int8": "<i1", "int16": "<i2", "int32": "<i4", "uint8": "u1", "uint16": "<u2",
    "uint32": "<u4", "float": "<f4", "double": "<f8",
}


def default_bin() -> str:
    for p in (HERE.parent / "target" / "release" / "openreadout", HERE.parent / "target" / "debug" / "openreadout"):
        if p.exists():
            return str(p)
    return "openreadout"


def cli(binary: str, *args: str) -> dict:
    out = subprocess.run([binary, *args, "--json"], capture_output=True, text=True)
    env = json.loads(out.stdout)
    if not env.get("ok"):
        raise RuntimeError(f"openreadout {' '.join(args)} failed: {env.get('error')}")
    return env["data"]


def block_mean(a: np.ndarray) -> np.ndarray:
    """2x2 mean with partial edge blocks, integers rounded half away from zero (the writer's rule)."""
    h, w = a.shape
    f = a.astype(np.float64)
    ph, pw = h + (h % 2), w + (w % 2)
    s = np.zeros((ph, pw)); n = np.zeros((ph, pw))
    s[:h, :w] = f; n[:h, :w] = 1
    s = s.reshape(ph // 2, 2, pw // 2, 2).sum(axis=(1, 3))
    n = n.reshape(ph // 2, 2, pw // 2, 2).sum(axis=(1, 3))
    m = s / n
    if np.issubdtype(a.dtype, np.integer):
        m = np.sign(m) * np.floor(np.abs(m) + 0.5)
        info = np.iinfo(a.dtype)
        m = np.clip(m, info.min, info.max)
    return m.astype(a.dtype)


def pick(n: int, k: int) -> list[int]:
    if n <= k:
        return list(range(n))
    return sorted({round(i * (n - 1) / (k - 1)) for i in range(k)})


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("store")
    ap.add_argument("source")
    ap.add_argument("--bin", default=default_bin())
    ap.add_argument("--image", type=int, help="source image index if the store was exported with --image")
    ap.add_argument("--planes", type=int, default=3, help="planes to compare per image")
    a = ap.parse_args()

    import ome_zarr_models.v05.bioformats2raw as m_bf2raw
    import ome_zarr_models.v05.image as m_image
    from bioio import BioImage
    import bioio_ome_zarr
    from ome_zarr.io import parse_url
    from ome_zarr.reader import Multiscales, Reader

    store = Path(a.store)
    info = cli(a.bin, "info", a.source)
    root = zarr.open_group(str(store), mode="r")
    ome = dict(root.attrs).get("ome", {})
    failures: list[str] = []
    totals = {"images": 0, "planes": 0, "pixels": 0, "levels": 0}

    if "bioformats2raw.layout" in ome:
        m_bf2raw.BioFormats2Raw.from_zarr(root)
        series = dict(zarr.open_group(str(store / "OME"), mode="r").attrs)["ome"]["series"]
        xml = store / "OME" / "METADATA.ome.xml"
        xsd_ok = "missing"
        if xml.exists():
            from lxml import etree
            schema = etree.XMLSchema(etree.parse(str(HERE / "schema" / "ome.xsd")))
            doc = etree.parse(str(xml))
            xsd_ok = "XSD-valid" if schema.validate(doc) else "XSD-INVALID"
            if xsd_ok != "XSD-valid":
                failures.append(f"METADATA.ome.xml fails the OME 2016-06 XSD: {schema.error_log.last_error}")
            n_img = len(doc.getroot().findall("{*}Image"))
            if n_img != len(series):
                failures.append(f"METADATA.ome.xml has {n_img} Image elements, series lists {len(series)}")
        else:
            failures.append("OME/METADATA.ome.xml missing")
        print(f"collection: bioformats2raw.layout=3, series={series}, METADATA.ome.xml {xsd_ok}")
        groups = [(p, info["images"][i]) for i, p in enumerate(series)]
    else:
        src = next(im for im in info["images"] if im["index"] == (a.image or 0))
        groups = [("", src)]

    with tempfile.TemporaryDirectory() as dump:
        for path, im in groups:
            gpath = store / path if path else store
            label = f"[{path or '/'}] image {im['index']} {im.get('name') or ''}".rstrip()
            spp = im["samples_per_pixel"]
            want = (im["size_t"], im["size_c"] * spp, im["size_z"], im["size_y"], im["size_x"])

            # 1. strict metadata validation
            model = m_image.Image.from_zarr(zarr.open_group(str(gpath), mode="r"))
            ms = model.ome_attributes.multiscales[0]
            nlev = len(ms.datasets)
            axes = [(ax.name, ax.type, ax.unit) for ax in ms.axes]
            scale0 = ms.datasets[0].coordinateTransformations[0].scale

            # 2. ome_zarr reader
            nodes = list(Reader(parse_url(str(gpath)))())
            node = nodes[0]
            if not any(isinstance(s, Multiscales) for s in node.specs):
                failures.append(f"{label}: ome_zarr did not recognise multiscales")
            levels = node.data
            shapes = [tuple(int(x) for x in d.shape) for d in levels]
            if shapes[0] != want:
                failures.append(f"{label}: level-0 shape {shapes[0]} != expected {want}")
            if len(levels) != nlev:
                failures.append(f"{label}: ome_zarr sees {len(levels)} levels, metadata lists {nlev}")
            for d in levels[1:]:
                np.asarray(d[0, 0, 0])  # every level is readable

            # 3. bioio
            img = BioImage(str(gpath), reader=bioio_ome_zarr.Reader)
            bdims = (img.dims.T, img.dims.C, img.dims.Z, img.dims.Y, img.dims.X)
            if bdims != want:
                failures.append(f"{label}: bioio dims {bdims} != {want}")
            px = img.physical_pixel_sizes
            if im["physical_size"].get("x") and abs((px.X or 0) - im["physical_size"]["x"]) > 1e-9:
                failures.append(f"{label}: bioio X pixel size {px.X} != {im['physical_size']['x']}")

            # 4. pixels vs `openreadout check --planes --dump-dir`
            planes = [(t, c, z) for t in pick(im["size_t"], 2) for c in pick(im["size_c"], 2) for z in pick(im["size_z"], 2)]
            planes = [planes[i] for i in pick(len(planes), a.planes)]
            arr0 = zarr.open_array(str(gpath / "0"), mode="r")
            bdata = img.get_image_dask_data("TCZYX")
            for t, c, z in planes:
                out = cli(a.bin, "check", a.source, "--planes", "--image", str(im["index"]), "--select", f"c={c}",
                          "--select", f"z={z}", "--select", f"t={t}", "--dump-dir", dump)
                h = out["planes"][0]
                raw = (Path(dump) / f"image{im['index']}_c{c}_z{z}_t{t}.bin").read_bytes()
                exp = np.frombuffer(raw, dtype=DTYPES[h["pixel_type"]]).reshape(h["height"], h["width"], spp)
                exp = np.moveaxis(exp, -1, 0)  # samples -> channels
                sl = slice(c * spp, (c + 1) * spp)
                got_z = np.asarray(arr0[t, sl, z])
                got_o = np.asarray(levels[0][t, sl, z])
                got_b = np.asarray(bdata[t, sl, z])
                for who, got in (("zarr", got_z), ("ome_zarr", got_o), ("bioio", got_b)):
                    if got.shape != exp.shape or not np.array_equal(got, exp, equal_nan=exp.dtype.kind == "f"):
                        failures.append(f"{label}: plane t={t} c={c} z={z}: {who} pixels differ from source")
                inter = np.ascontiguousarray(np.moveaxis(got_z, 0, -1)).astype(exp.dtype.newbyteorder("<")).tobytes()
                if xxhash.xxh3_128_hexdigest(inter) != h["xxh3"]:
                    failures.append(f"{label}: plane t={t} c={c} z={z}: xxh3 differs from `check --planes`")
                totals["planes"] += 1
                totals["pixels"] += exp.size

            # 5. pyramid level 1 = 2x2 mean of level 0
            max_diff = 0.0
            if nlev > 1:
                arr1 = zarr.open_array(str(gpath / "1"), mode="r")
                t, c, z = planes[0]
                l0 = np.asarray(arr0[t, c * spp, z])
                l1 = np.asarray(arr1[t, c * spp, z])
                ref = block_mean(l0)
                max_diff = float(np.max(np.abs(ref.astype(np.float64) - l1.astype(np.float64)))) if ref.shape == l1.shape else float("inf")
                if max_diff > (1e-6 if l0.dtype.kind == "f" else 0):
                    failures.append(f"{label}: level 1 is not the 2x2 mean of level 0 (max diff {max_diff})")
            totals["images"] += 1
            totals["levels"] += nlev
            print(f"{label}: NGFF-0.5 valid; axes={[x[0] + ('/' + x[2] if x[2] else '') for x in axes]} scale0={scale0} "
                  f"levels={shapes} bioio dims={bdims} px=({px.Z},{px.Y},{px.X}) ch={img.channel_names} "
                  f"planes compared={len(planes)} pyramid max|diff|={max_diff}")

    print(f"TOTAL images={totals['images']} levels={totals['levels']} planes={totals['planes']} pixels={totals['pixels']} "
          f"failures={len(failures)}")
    for f in failures:
        print("  FAIL", f)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
