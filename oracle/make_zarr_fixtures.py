#!/usr/bin/env python
"""Write the SYNTHETIC OME-Zarr fixtures used by crates/openreadout-zarr/tests.

Usage: uv run python make_zarr_fixtures.py [--out DIR]
       (default DIR: crates/openreadout-zarr/tests/fixtures; one `<name>.zip` per store)

Every store is written with zarr-python and ome-zarr-py (both BSD) — nothing here comes from a
microscope. Pixel values are a deterministic function of (image, t, c, z, y, x) so hashes are
stable across runs. Each store is packed into a zip with the store's root at the root of the
archive (the layout of a zipped OME-Zarr, NGFF RFC-9); `*-stored.zip` fixtures use no
compression inside the zip, the others deflate (both occur in the wild). The oracle JSON next to
them is written by `gen.py` (ORACLE_OUT=<dir>/oracle uv run python gen.py <zip>...).
"""
import argparse, json, os, shutil, sys, tempfile, zipfile
from pathlib import Path

import numpy as np
import zarr
from numcodecs import Blosc, GZip, Zlib, Zstd
from ome_zarr.format import FormatV04, FormatV05
from ome_zarr.writer import (write_label_metadata, write_multiscales_metadata, write_plate_metadata,
                             write_well_metadata)

ROOT = Path(__file__).resolve().parent.parent


def ramp(shape, dtype, seed=0):
    """Deterministic data: a mixed-radix index pattern, wrapped to the dtype's range."""
    idx = np.indices(shape).reshape(len(shape), -1)
    v = seed * 7
    for k, row in enumerate(idx):
        v = v + row * (13 + 31 * k)
    v = v.reshape(shape)
    dt = np.dtype(dtype)
    if dt.kind == "f":
        return (v * 0.25 - 3.0).astype(dt)
    info = np.iinfo(dt)
    return (v % (int(info.max) - int(info.min) + 1) + int(info.min)).astype(dt)


def down2(a):
    """2x2 mean over the last two axes (edges kept), in the input dtype (the level contents do
    not matter for the tests; they only need to be what the store holds)."""
    h, w = a.shape[-2:]
    return a[..., ::2, ::2].copy()


def v2_array(group, path, data, chunks, compressor, dimension_separator="/"):
    arr = zarr.create_array(group.store, name=f"{group.path}/{path}".strip("/"), shape=data.shape,
                            dtype=data.dtype, chunks=chunks, compressors=compressor, zarr_format=2,
                            chunk_key_encoding={"name": "v2", "separator": dimension_separator},
                            fill_value=0, overwrite=True)
    arr[...] = data
    return arr


def v3_array(group, path, data, chunks, compressors, shards=None, dimension_names=None):
    arr = zarr.create_array(group.store, name=f"{group.path}/{path}".strip("/"), shape=data.shape,
                            dtype=data.dtype, chunks=chunks, shards=shards, compressors=compressors,
                            zarr_format=3, fill_value=0, dimension_names=dimension_names, overwrite=True)
    arr[...] = data
    return arr


def axes_of(names, units=None):
    units = units or {}
    kind = {"t": "time", "c": "channel", "z": "space", "y": "space", "x": "space"}
    out = []
    for n in names:
        a = {"name": n, "type": kind[n]}
        if n in units:
            a["unit"] = units[n]
        out.append(a)
    return out


def scales(base, n):
    """Per-level scale transformations; y and x double per level."""
    out = []
    for lvl in range(n):
        s = list(base)
        s[-1] *= 2 ** lvl
        s[-2] *= 2 ** lvl
        out.append([{"type": "scale", "scale": s}])
    return out


def omero(labels, colors, dtype):
    info = np.iinfo(dtype) if np.dtype(dtype).kind in "iu" else None
    return {"channels": [{"label": l, "color": c, "active": True, "coefficient": 1, "family": "linear",
                          "inverted": False,
                          "window": {"start": 0, "end": 100, "min": int(info.min) if info else 0,
                                     "max": int(info.max) if info else 1}} for l, c in zip(labels, colors)],
            "rdefs": {"defaultT": 0, "defaultZ": 0, "model": "color"}}


def set_omero(g, om):
    """ome-zarr-py's write_multiscales_metadata drops an `omero` keyword; store it directly
    (top level for NGFF 0.4 / Zarr v2, inside `ome` for NGFF 0.5 / Zarr v3)."""
    a = dict(g.attrs)
    if "ome" in a:
        ome = dict(a["ome"]); ome["omero"] = om; g.attrs["ome"] = ome
    else:
        g.attrs["omero"] = om


def pyramid(a, n):
    out = [a]
    for _ in range(n - 1):
        out.append(down2(out[-1]))
    return out


def fixture_v04_tczyx(root):
    """NGFF 0.4, Zarr v2, TCZYX uint16, two levels, Blosc lz4 + shuffle, a label image."""
    g = zarr.open_group(zarr.storage.LocalStore(root), mode="w", zarr_format=2)
    data = ramp((2, 2, 3, 48, 64), "uint16", seed=1)
    levels = pyramid(data, 2)
    comp = Blosc(cname="lz4", clevel=5, shuffle=Blosc.SHUFFLE)
    for i, lv in enumerate(levels):
        v2_array(g, str(i), lv, (1, 1, 1, 32, 32), comp)
    fmt = FormatV04()
    write_multiscales_metadata(g, [{"path": str(i), "coordinateTransformations": ct}
                                   for i, ct in enumerate(scales([2.5, 1.0, 1.5, 0.25, 0.25], 2))],
                               fmt=fmt, axes=axes_of("tczyx", {"t": "second", "z": "micrometer", "y": "micrometer", "x": "micrometer"}),
                               name="tczyx")
    set_omero(g, omero(["DAPI", "GFP"], ["0000FF", "00FF00"], "uint16"))
    labels = g.create_group("labels")
    labels.attrs["labels"] = ["cells"]
    lg = labels.create_group("cells")
    lab = (ramp((2, 1, 3, 48, 64), "uint32", seed=5) % 4).astype("uint32")
    v2_array(lg, "0", lab, (1, 1, 1, 48, 64), Blosc(cname="zstd", clevel=3, shuffle=Blosc.BITSHUFFLE))
    write_multiscales_metadata(lg, [{"path": "0", "coordinateTransformations": scales([2.5, 1.0, 1.5, 0.25, 0.25], 1)[0]}],
                               fmt=fmt, axes=axes_of("tczyx", {"z": "micrometer", "y": "micrometer", "x": "micrometer"}), name="cells")
    write_label_metadata(labels, "cells", colors=[{"label-value": 1, "rgba": [255, 0, 0, 255]}], fmt=fmt)


def fixture_v04_zyx_codecs(root):
    """NGFF 0.4, Zarr v2, ZYX float32, three levels with a different compressor each (Blosc
    blosclz + byte shuffle, Zstd, Zlib) and `.` chunk-key separators; micrometre/nanometre units."""
    g = zarr.open_group(zarr.storage.LocalStore(root), mode="w", zarr_format=2)
    data = ramp((5, 40, 36), "float32", seed=2)
    comps = [Blosc(cname="blosclz", clevel=9, shuffle=Blosc.SHUFFLE), Zstd(level=3), Zlib(level=6)]
    for i, lv in enumerate(pyramid(data, 3)):
        v2_array(g, str(i), lv, (2, 16, 16), comps[i], dimension_separator=".")
    write_multiscales_metadata(g, [{"path": str(i), "coordinateTransformations": ct}
                                   for i, ct in enumerate(scales([2000.0, 110.0, 110.0], 3))],
                               fmt=FormatV04(), axes=axes_of("zyx", {"z": "nanometer", "y": "nanometer", "x": "nanometer"}),
                               name="zyx-float")


def fixture_v05_yx(root):
    """NGFF 0.5, Zarr v3, YX uint8, zarr-python's default codecs (bytes + zstd)."""
    g = zarr.open_group(zarr.storage.LocalStore(root), mode="w", zarr_format=3)
    data = ramp((50, 70), "uint8", seed=3)
    v3_array(g, "0", data, (32, 32), "auto", dimension_names=["y", "x"])
    write_multiscales_metadata(g, [{"path": "0", "coordinateTransformations": [{"type": "scale", "scale": [0.5, 0.5]}]}],
                               fmt=FormatV05(), axes=axes_of("yx", {"y": "micrometer", "x": "micrometer"}), name="yx-v3")


def fixture_v05_sharded(root):
    """NGFF 0.5, Zarr v3, CZYX int16, sharded (inner chunks 1x1x16x16 in 1x1x32x32 shards),
    Blosc zstd inside the shards; a second level with gzip and no sharding."""
    g = zarr.open_group(zarr.storage.LocalStore(root), mode="w", zarr_format=3)
    data = ramp((3, 2, 40, 48), "int16", seed=4)
    from zarr.codecs import BloscCodec, GzipCodec
    v3_array(g, "0", data, (1, 1, 16, 16), [BloscCodec(cname="zstd", clevel=4, shuffle="shuffle")],
             shards=(1, 1, 32, 32), dimension_names=["c", "z", "y", "x"])
    v3_array(g, "1", down2(data), (1, 1, 20, 24), [GzipCodec(level=5)], dimension_names=["c", "z", "y", "x"])
    write_multiscales_metadata(g, [{"path": str(i), "coordinateTransformations": ct}
                                   for i, ct in enumerate(scales([1.0, 0.8, 0.2, 0.2], 2))],
                               fmt=FormatV05(), axes=axes_of("czyx", {"z": "micrometer", "y": "micrometer", "x": "micrometer"}),
                               name="czyx-sharded")
    set_omero(g, omero(["a", "b", "c"], ["FF0000", "00FF00", "0000FF"], "int16"))


def fixture_v04_plate(root):
    """NGFF 0.4 HCS plate: one row, two wells, two fields per well (CYX uint8), Blosc lz4."""
    g = zarr.open_group(zarr.storage.LocalStore(root), mode="w", zarr_format=2)
    fmt = FormatV04()
    wells = ["A/1", "A/2"]
    write_plate_metadata(g, ["A"], ["1", "2"], wells, fmt=fmt, field_count=2, name="test plate",
                         acquisitions=[{"id": 0, "name": "run"}])
    k = 0
    for w in wells:
        wg = g.require_group(w)
        write_well_metadata(wg, [{"path": "0", "acquisition": 0}, {"path": "1", "acquisition": 0}], fmt=fmt)
        for f in ("0", "1"):
            fg = wg.require_group(f)
            v2_array(fg, "0", ramp((2, 24, 30), "uint8", seed=10 + k), (1, 24, 30), Blosc(cname="lz4", clevel=5, shuffle=Blosc.NOSHUFFLE))
            write_multiscales_metadata(fg, [{"path": "0", "coordinateTransformations": [{"type": "scale", "scale": [1.0, 0.65, 0.65]}]}],
                                       fmt=fmt, axes=axes_of("cyx", {"y": "micrometer", "x": "micrometer"}))
            set_omero(fg, omero(["DAPI", "Actin"], ["0000FF", "FF0000"], "uint8"))
            k += 1


OME_XML = """<?xml version="1.0" encoding="UTF-8"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06" Creator="make_zarr_fixtures.py">
  <Instrument ID="Instrument:0">
    <Microscope Manufacturer="Synthetic" Model="Fixture 1"/>
    <Objective ID="Objective:0:0" Model="Plan-Apo 20x" NominalMagnification="20.0" LensNA="0.8" Immersion="Air"/>
  </Instrument>
  <Image ID="Image:0" Name="series zero">
    <AcquisitionDate>2024-05-06T07:08:09</AcquisitionDate>
    <InstrumentRef ID="Instrument:0"/>
    <ObjectiveSettings ID="Objective:0:0"/>
    <Pixels ID="Pixels:0" DimensionOrder="XYZCT" Type="uint16" SizeX="32" SizeY="20" SizeZ="2" SizeC="2" SizeT="1"
            PhysicalSizeX="0.325" PhysicalSizeXUnit="µm" PhysicalSizeY="0.325" PhysicalSizeYUnit="µm" PhysicalSizeZ="1.0" PhysicalSizeZUnit="µm">
      <Channel ID="Channel:0:0" Name="Hoechst" SamplesPerPixel="1" Fluor="Hoechst 33342" ExcitationWavelength="405" ExcitationWavelengthUnit="nm" EmissionWavelength="461" EmissionWavelengthUnit="nm"/>
      <Channel ID="Channel:0:1" Name="mCherry" SamplesPerPixel="1" ExcitationWavelength="561" ExcitationWavelengthUnit="nm" EmissionWavelength="610" EmissionWavelengthUnit="nm"/>
      <MetadataOnly/>
    </Pixels>
  </Image>
  <Image ID="Image:1" Name="series one">
    <Pixels ID="Pixels:1" DimensionOrder="XYZCT" Type="uint8" SizeX="16" SizeY="12" SizeZ="1" SizeC="1" SizeT="3"
            PhysicalSizeX="1.3" PhysicalSizeXUnit="µm" PhysicalSizeY="1.3" PhysicalSizeYUnit="µm" TimeIncrement="30" TimeIncrementUnit="s">
      <Channel ID="Channel:1:0" Name="BF" SamplesPerPixel="1"/>
      <MetadataOnly/>
    </Pixels>
  </Image>
</OME>
"""


def fixture_bf2raw(root):
    """bioformats2raw.layout 3 collection (Zarr v2, NGFF 0.4) with OME/METADATA.ome.xml."""
    g = zarr.open_group(zarr.storage.LocalStore(root), mode="w", zarr_format=2)
    g.attrs["bioformats2raw.layout"] = 3
    fmt = FormatV04()
    comp = Blosc(cname="lz4", clevel=5, shuffle=Blosc.SHUFFLE)
    s0 = g.create_group("0")
    v2_array(s0, "0", ramp((1, 2, 2, 20, 32), "uint16", seed=20), (1, 1, 1, 20, 32), comp)
    write_multiscales_metadata(s0, [{"path": "0", "coordinateTransformations": [{"type": "scale", "scale": [1.0, 1.0, 1.0, 0.325, 0.325]}]}],
                               fmt=fmt, axes=axes_of("tczyx", {"z": "micrometer", "y": "micrometer", "x": "micrometer"}))
    s1 = g.create_group("1")
    v2_array(s1, "0", ramp((3, 1, 1, 12, 16), "uint8", seed=21), (1, 1, 1, 12, 16), comp)
    write_multiscales_metadata(s1, [{"path": "0", "coordinateTransformations": [{"type": "scale", "scale": [30.0, 1.0, 1.0, 1.3, 1.3]}]}],
                               fmt=fmt, axes=axes_of("tczyx", {"t": "second", "y": "micrometer", "x": "micrometer"}))
    ome = g.create_group("OME")
    ome.attrs["series"] = ["0", "1"]
    (Path(root) / "OME" / "METADATA.ome.xml").write_text(OME_XML, encoding="utf-8")


def fixture_bf2raw_dtypes(root):
    """bioformats2raw.layout collection (Zarr v2, NGFF 0.4) of one series per data type OpenReadout
    widens or reads natively: int64, uint64, float16, bool, complex64, complex128; Blosc with bit
    shuffle (lz4, blosclz, zstd) and byte shuffle; series 0 carries a dataset translation and a
    multiscales-wide scale and translation."""
    g = zarr.open_group(zarr.storage.LocalStore(root), mode="w", zarr_format=2)
    g.attrs["bioformats2raw.layout"] = 3
    fmt = FormatV04()
    yx = (18, 26)
    specs = [
        ("int64", ramp(yx, "int32", seed=30).astype("int64") * 1_000_003, Blosc(cname="lz4", clevel=5, shuffle=Blosc.BITSHUFFLE)),
        ("uint64", ramp(yx, "uint32", seed=31).astype("uint64") * 3 + 2**40, Blosc(cname="blosclz", clevel=5, shuffle=Blosc.BITSHUFFLE)),
        ("float16", ramp(yx, "float32", seed=32).astype("float16"), Blosc(cname="zstd", clevel=3, shuffle=Blosc.BITSHUFFLE)),
        ("bool", (ramp(yx, "uint8", seed=33) % 3 == 0), Blosc(cname="lz4", clevel=5, shuffle=Blosc.NOSHUFFLE)),
        ("complex64", (ramp(yx, "float32", seed=34) + 1j * ramp(yx, "float32", seed=35)).astype("complex64"), Blosc(cname="zstd", clevel=3, shuffle=Blosc.SHUFFLE)),
        ("complex128", (ramp(yx, "float64", seed=36) - 2j * ramp(yx, "float64", seed=37)).astype("complex128"), Zstd(level=3)),
    ]
    for i, (name, data, comp) in enumerate(specs):
        s = g.create_group(str(i))
        v2_array(s, "0", data, (9, 13), comp)
        cts = [{"type": "scale", "scale": [0.5, 0.25]}]
        if i == 0:
            cts.append({"type": "translation", "translation": [10.0, -4.0]})
        write_multiscales_metadata(s, [{"path": "0", "coordinateTransformations": cts}], fmt=fmt,
                                   axes=axes_of("yx", {"y": "micrometer", "x": "micrometer"}), name=name)
        if i == 0:
            a = dict(s.attrs)
            a["multiscales"][0]["coordinateTransformations"] = [{"type": "scale", "scale": [2.0, 2.0]},
                                                                {"type": "translation", "translation": [1.0, 3.0]}]
            s.attrs.update(a)
    ome = g.create_group("OME")
    ome.attrs["series"] = [str(i) for i in range(len(specs))]


FIXTURES = {
    "ngff04-bf2raw-dtypes": (fixture_bf2raw_dtypes, zipfile.ZIP_DEFLATED),
    "ngff04-v2-tczyx-blosc-lz4": (fixture_v04_tczyx, zipfile.ZIP_DEFLATED),
    "ngff04-v2-zyx-float-codecs": (fixture_v04_zyx_codecs, zipfile.ZIP_DEFLATED),
    "ngff05-v3-yx-zstd-stored": (fixture_v05_yx, zipfile.ZIP_STORED),
    "ngff05-v3-czyx-sharded": (fixture_v05_sharded, zipfile.ZIP_DEFLATED),
    "ngff04-v2-plate": (fixture_v04_plate, zipfile.ZIP_DEFLATED),
    "ngff04-bf2raw-collection": (fixture_bf2raw, zipfile.ZIP_STORED),
}


def pack(src: Path, dst: Path, method):
    """Zip the store with its root at the archive root; fixed timestamps for reproducible bytes."""
    with zipfile.ZipFile(dst, "w", compression=method) as z:
        for p in sorted(src.rglob("*")):
            if p.is_file():
                info = zipfile.ZipInfo(p.relative_to(src).as_posix(), date_time=(2026, 1, 1, 0, 0, 0))
                info.compress_type = method
                z.writestr(info, p.read_bytes())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(ROOT / "crates" / "openreadout-zarr" / "tests" / "fixtures"))
    ap.add_argument("only", nargs="*")
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    for name, (fn, method) in FIXTURES.items():
        if a.only and name not in a.only:
            continue
        with tempfile.TemporaryDirectory() as td:
            store = Path(td) / "store"
            fn(str(store))
            pack(store, out / f"{name}.zip", method)
        print("wrote", out / f"{name}.zip")


if __name__ == "__main__":
    main()
