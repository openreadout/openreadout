#!/usr/bin/env python
"""Regenerate the synthetic CZI fixtures in corpus/files/ (all ids start with `synthetic-`).

Usage:  cd oracle && uv run python make_czi_fixtures.py [--out DIR] [--oracle]

How each fixture is made (see docs/provenance/czi.md, 2026-09-22 "synthetic fixtures"):

* Base files are WRITTEN by pylibCZIrw (LGPL, run as a black box) from NumPy arrays.
  pylibCZIrw 5 writes only uncompressed, zstd0 and zstd1 subblocks; asking it for `jpg:` or
  `jpgxr:` fails with "An unsupported compression mode was specified".
* JPEG (compression id 1) and JPEG XR (id 4) fixtures are made by `transcode()` below: it walks a
  pylibCZIrw-written file with OUR segment walker (layout from docs/formats/czi.md), re-encodes each
  subblock's pixels with imagecodecs (libjpeg-turbo / jxrlib) and writes a new container with the
  same XML and attachments. For 3-sample pixel types the JPEG stream carries R,G,B (what czifile
  returns for compression 1, which it does not channel-swap).
* `synthetic-jxr-mismatch-*` declare a pixel type in the directory that differs from what the JPEG XR
  stream decodes to, to exercise the "resolution protocol" (libCZI documentation page
  pages/resolution_protocol.html: the directory entry is authoritative; convert, crop or pad).
* `synthetic-multifile*` split one pylibCZIrw file into a master and two following parts named
  `<stem> (1).czi`, `<stem> (2).czi` (the sibling naming reported by users of split ZEN documents;
  no public document describes it: czifile only says multi-file images are not supported, the
  Bio-Formats format page is silent). The semantics (parts carry `file_part = k`, the master's
  `file_guid` as their `primary_file_guid`, and the directory lives in the master) are INFERRED: no
  public multi-file CZI was available to derive them from.

`--oracle` also runs gen.py on every fixture (czifile ground truth). For the multi-file fixture,
whose parts czifile does not follow, the oracle is czifile's reading of the unsplit twin
(`synthetic-multifile-source.czi`), re-labelled.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import struct
import subprocess
import sys
import uuid
from dataclasses import dataclass, field, replace
from pathlib import Path

import imagecodecs
import numpy as np
from pylibCZIrw import czi as pyczi

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
SEG_HDR = 32

# ---------------------------------------------------------------------------------------------
# Pixel data
# ---------------------------------------------------------------------------------------------

def pattern(h: int, w: int, seed: int, dtype, samples: int = 1) -> np.ndarray:
    """Smooth gradients + rings + mild noise: compresses like a real micrograph."""
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w].astype(np.float64)
    out = []
    for s in range(samples):
        cx, cy = w * (0.3 + 0.2 * s), h * (0.4 + 0.1 * s)
        r = np.hypot(x - cx, y - cy)
        v = 0.45 + 0.25 * np.sin(r / (5.0 + 3 * s + seed % 4)) + 0.2 * (x / w) - 0.1 * (y / h)
        v += rng.normal(0, 0.02, size=v.shape)
        out.append(np.clip(v, 0, 1))
    a = np.stack(out, axis=-1)
    top = 255 if np.dtype(dtype) == np.uint8 else 4095  # 12-bit camera range for 16-bit data
    return (a * top).round().astype(dtype)


# ---------------------------------------------------------------------------------------------
# Our own minimal CZI container reader/writer (layout: docs/formats/czi.md)
# ---------------------------------------------------------------------------------------------

@dataclass
class Dim:
    name: str
    start: int
    size: int
    start_coordinate: float
    stored_size: int


@dataclass
class SubBlock:
    pixel_type: int
    compression: int
    pyramid_type: int
    dims: list[Dim]
    metadata: bytes
    data: bytes
    attachment: bytes = b""
    file_part: int = 0

    def dim(self, n: str) -> Dim | None:
        return next((d for d in self.dims if d.name == n), None)


@dataclass
class Attachment:
    content_guid: bytes
    content_file_type: str
    name: str
    data: bytes


@dataclass
class Container:
    xml: bytes
    subblocks: list[SubBlock]
    attachments: list[Attachment] = field(default_factory=list)


def _entry_bytes(sb: SubBlock, file_position: int) -> bytes:
    b = b"DV" + struct.pack("<IQIIB", sb.pixel_type, file_position, sb.file_part, sb.compression, sb.pyramid_type)
    b += b"\0" * 5 + struct.pack("<I", len(sb.dims))
    for d in sb.dims:
        b += d.name.encode().ljust(4, b"\0") + struct.pack("<iIfI", d.start, d.size, d.start_coordinate, d.stored_size)
    return b


def read_container(path: Path) -> Container:
    raw = path.read_bytes()
    u32 = lambda o: struct.unpack_from("<I", raw, o)[0]
    u64 = lambda o: struct.unpack_from("<Q", raw, o)[0]
    dir_pos, meta_pos, att_pos = u64(SEG_HDR + 52), u64(SEG_HDR + 60), u64(SEG_HDR + 72)
    xml = raw[meta_pos + SEG_HDR + 256: meta_pos + SEG_HDR + 256 + u32(meta_pos + SEG_HDR)]
    subblocks = []
    n = u32(dir_pos + SEG_HDR)
    off = dir_pos + SEG_HDR + 128
    for _ in range(n):
        pt, fpos, part, comp, pyr = struct.unpack_from("<IQIIB", raw, off + 2)
        dc = u32(off + 28)
        dims = []
        for k in range(dc):
            o = off + 32 + 20 * k
            name = raw[o:o + 4].rstrip(b"\0").decode()
            st, sz, sc, ss = struct.unpack_from("<iIfI", raw, o + 4)
            dims.append(Dim(name, st, sz, sc, ss))
        elen = 32 + 20 * dc
        off += elen
        p = fpos + SEG_HDR
        msize, asize, dsize = struct.unpack_from("<IIQ", raw, p)
        hl = max(256, 16 + elen)
        m0 = p + hl
        subblocks.append(SubBlock(pt, comp, pyr, dims, raw[m0:m0 + msize], raw[m0 + msize:m0 + msize + dsize],
                                  raw[m0 + msize + dsize:m0 + msize + dsize + asize], part))
    atts = []
    if att_pos:
        n = u32(att_pos + SEG_HDR)
        for i in range(n):
            e = att_pos + SEG_HDR + 256 + 128 * i
            fpos = u64(e + 12)
            guid = raw[e + 24:e + 40]
            ctype = raw[e + 40:e + 48].rstrip(b"\0").decode("latin1")
            name = raw[e + 48:e + 128].split(b"\0")[0].decode("latin1")
            dsize = u32(fpos + SEG_HDR)
            atts.append(Attachment(guid, ctype, name, raw[fpos + SEG_HDR + 256: fpos + SEG_HDR + 256 + dsize]))
    return Container(xml, subblocks, atts)


def _segment(sid: str, payload: bytes, allocated: int | None = None) -> bytes:
    alloc = allocated if allocated is not None else (len(payload) + 31) // 32 * 32
    return sid.encode().ljust(16, b"\0") + struct.pack("<QQ", alloc, len(payload)) + payload.ljust(alloc, b"\0")


def _file_header(primary: bytes, fguid: bytes, part: int, dir_pos: int, meta_pos: int, att_pos: int) -> bytes:
    p = struct.pack("<II", 1, 0) + b"\0" * 8 + primary + fguid
    p += struct.pack("<IQQIQ", part, dir_pos, meta_pos, 0, att_pos)
    return _segment("ZISRAWFILE", p, 512)


def _subblock_segment(sb: SubBlock, file_position: int) -> bytes:
    entry = _entry_bytes(sb, file_position)
    head = struct.pack("<IIQ", len(sb.metadata), len(sb.attachment), len(sb.data)) + entry
    head = head.ljust(max(256, len(head)), b"\0")
    return _segment("ZISRAWSUBBLOCK", head + sb.metadata + sb.data + sb.attachment)


def write_container(c: Container, path: Path, parts: int = 1) -> list[Path]:
    """Write `c`; subblocks with file_part k > 0 go to `<stem> (k).czi` next to `path`."""
    primary = uuid.uuid5(uuid.NAMESPACE_URL, "openreadout-fixture:" + path.name).bytes  # deterministic
    out_paths = [path]
    positions: dict[int, int] = {}
    # following parts first, so the master's directory can point into them
    for k in range(1, parts):
        buf = bytearray(_file_header(primary, uuid.uuid5(uuid.NAMESPACE_URL, f"openreadout-fixture:{path.name}:{k}").bytes, k, 0, 0, 0))
        for i, sb in enumerate(c.subblocks):
            if sb.file_part == k:
                positions[i] = len(buf)
                buf += _subblock_segment(sb, len(buf))
        pk = path.with_name(f"{path.stem} ({k}){path.suffix}")
        pk.write_bytes(bytes(buf))
        out_paths.append(pk)
    buf = bytearray(b"\0" * (SEG_HDR + 512))
    meta_pos = len(buf)
    buf += _segment("ZISRAWMETADATA", struct.pack("<II", len(c.xml), 0) + b"\0" * 248 + c.xml)
    for i, sb in enumerate(c.subblocks):
        if sb.file_part == 0:
            positions[i] = len(buf)
            buf += _subblock_segment(sb, len(buf))
    att_entries = []
    for a in c.attachments:
        pos = len(buf)
        entry = b"A1" + b"\0" * 10 + struct.pack("<QI", pos, 0) + a.content_guid
        entry += a.content_file_type.encode().ljust(8, b"\0") + a.name.encode().ljust(80, b"\0")
        buf += _segment("ZISRAWATTACH", (struct.pack("<I", len(a.data)) + b"\0" * 12 + entry).ljust(256, b"\0") + a.data)
        att_entries.append(entry)
    dir_pos = len(buf)
    buf += _segment("ZISRAWDIRECTORY", struct.pack("<I", len(c.subblocks)) + b"\0" * 124
                    + b"".join(_entry_bytes(sb, positions[i]) for i, sb in enumerate(c.subblocks)))
    att_pos = 0
    if att_entries:
        att_pos = len(buf)
        buf += _segment("ZISRAWATTDIR", struct.pack("<I", len(att_entries)) + b"\0" * 252 + b"".join(att_entries))
    buf[0:SEG_HDR + 512] = _file_header(primary, primary, 0, dir_pos, meta_pos, att_pos)
    path.write_bytes(bytes(buf))
    return out_paths


# ---------------------------------------------------------------------------------------------
# Transcoding
# ---------------------------------------------------------------------------------------------

PIXEL = {0: (np.uint8, 1), 1: (np.uint16, 1), 3: (np.uint8, 3), 4: (np.uint16, 3)}


def pixels(sb: SubBlock) -> np.ndarray:
    dt, spp = PIXEL[sb.pixel_type]
    x, y = sb.dim("X"), sb.dim("Y")
    return np.frombuffer(sb.data, dt).reshape(y.stored_size, x.stored_size, spp)


def to_jpeg(sb: SubBlock, **kw) -> SubBlock:
    a = pixels(sb)
    if a.shape[-1] == 3:
        a = a[..., ::-1]  # stored B,G,R -> stream R,G,B
    else:
        a = a[..., 0]
    return replace(sb, compression=1, data=bytes(imagecodecs.jpeg8_encode(np.ascontiguousarray(a), **kw)))


def to_jxr(sb: SubBlock, *, level=1.0, declare: int | None = None, convert=None) -> SubBlock:
    a = pixels(sb)
    if a.shape[-1] == 1:
        a = a[..., 0]
    if convert is not None:
        a = convert(a)
    data = bytes(imagecodecs.jpegxr_encode(np.ascontiguousarray(a), level=level))
    return replace(sb, compression=4, data=data, pixel_type=sb.pixel_type if declare is None else declare)


def transcode(src: Path, dst: Path, fn) -> None:
    c = read_container(src)
    c.subblocks = [fn(sb) for sb in c.subblocks]
    write_container(c, dst)


# ---------------------------------------------------------------------------------------------
# Base files written by pylibCZIrw
# ---------------------------------------------------------------------------------------------

def write_base(path: Path, *, dtype, samples, sizes=dict(C=1, Z=1, T=1), tiles=((0, 0),), tile=(96, 128),
               scenes=1, compression="uncompressed:", channel_names=None) -> None:
    if path.exists():
        path.unlink()
    h, w = tile
    seed = 0
    with pyczi.create_czi(str(path), exist_ok=True, compression_options=compression) as wr:
        for s in range(scenes):
            for c in range(sizes["C"]):
                for z in range(sizes["Z"]):
                    for t in range(sizes["T"]):
                        for (tx, ty) in tiles:
                            seed += 1
                            a = pattern(h, w, seed, dtype, samples)
                            wr.write(a, plane={"C": c, "Z": z, "T": t}, scene=s,
                                     location=(tx + s * 1000, ty))
        wr.write_metadata(document_name=path.stem,
                          channel_names=channel_names or {c: f"Ch{c}" for c in range(sizes["C"])},
                          scale_x=0.5e-6, scale_y=0.5e-6, scale_z=2e-6)


FIXTURES: dict[str, str] = {}  # id -> description (printed; copy into corpus/manifest.toml)


def build(out: Path) -> list[Path]:
    out.mkdir(parents=True, exist_ok=True)
    made: list[Path] = []

    def note(p: Path, desc: str):
        FIXTURES[p.stem] = desc
        made.append(p)

    # --- pylibCZIrw-written bases (also kept as fixtures) ---
    g8 = out / "synthetic-gray8-c2z2t3-uncompressed.czi"
    write_base(g8, dtype=np.uint8, samples=1, sizes=dict(C=2, Z=2, T=3), tiles=((0, 0), (128, 0)))
    note(g8, "pylibCZIrw: gray8, C=2 Z=2 T=3, 2 mosaic tiles of 128x96, uncompressed")
    g16 = out / "synthetic-gray16-s2c2t2-zstd1.czi"
    write_base(g16, dtype=np.uint16, samples=1, sizes=dict(C=2, Z=1, T=2), scenes=2,
               compression="zstd1:ExplicitLevel=2;PreProcess=HiLoByteUnpack")
    note(g16, "pylibCZIrw: gray16 (12-bit values), 2 scenes, C=2 T=2, zstd1 with HiLo")
    g16u = out / "synthetic-gray16-s2c2t2-uncompressed.czi"
    write_base(g16u, dtype=np.uint16, samples=1, sizes=dict(C=2, Z=1, T=2), scenes=2)
    note(g16u, "pylibCZIrw: gray16, 2 scenes, C=2 T=2, uncompressed (base for JPEG XR/lossless fixtures)")
    rgb = out / "synthetic-bgr24-uncompressed.czi"
    write_base(rgb, dtype=np.uint8, samples=3, tiles=((0, 0), (128, 0)))
    note(rgb, "pylibCZIrw: bgr24, 2 mosaic tiles, uncompressed")
    rgb48 = out / "synthetic-bgr48-uncompressed.czi"
    write_base(rgb48, dtype=np.uint16, samples=3)
    note(rgb48, "pylibCZIrw: bgr48, one tile, uncompressed")

    # --- JPEG (compression id 1) ---
    p = out / "synthetic-gray8-jpeg.czi"
    transcode(g8, p, lambda sb: to_jpeg(sb, level=90))
    note(p, "gray8 C=2 Z=2 T=3 mosaic, subblocks re-encoded as baseline JPEG q90 (libjpeg-turbo)")
    p = out / "synthetic-bgr24-jpeg.czi"
    transcode(rgb, p, lambda sb: to_jpeg(sb, level=90))
    note(p, "bgr24 mosaic, subblocks re-encoded as baseline JPEG q90 4:2:0 (libjpeg-turbo), stream R,G,B")
    p = out / "synthetic-bgr24-jpeg444.czi"
    transcode(rgb, p, lambda sb: to_jpeg(sb, level=95, subsampling="444"))
    note(p, "bgr24 mosaic, baseline JPEG q95 without chroma subsampling")
    p = out / "synthetic-gray16-jpeg-lossless.czi"
    transcode(g16u, p, lambda sb: to_jpeg(sb, lossless=True, predictor=1, bitspersample=16))
    note(p, "gray16 2 scenes, subblocks re-encoded as lossless JPEG (SOF3, 16-bit, predictor 1) under compression id 1")
    p = out / "synthetic-gray16-jpeg12.czi"
    transcode(g16u, p, lambda sb: to_jpeg(sb, level=95, bitspersample=12))
    note(p, "gray16 (12-bit values) 2 scenes, subblocks re-encoded as 12-bit extended JPEG q95 under compression id 1")

    # --- JPEG XR (compression id 4), matching and mismatching the declared pixel type ---
    p = out / "synthetic-gray16-jxr.czi"
    transcode(g16u, p, lambda sb: to_jxr(sb))
    note(p, "gray16 2 scenes, lossless JPEG XR (jxrlib)")
    p = out / "synthetic-jxr-mismatch-bgr48-as-bgr24.czi"
    transcode(rgb48, p, lambda sb: to_jxr(sb, declare=3))
    note(p, "resolution protocol: JPEG XR stream is 48-bit RGB, directory declares bgr24")
    p = out / "synthetic-jxr-mismatch-gray8-as-gray16.czi"
    transcode(g16u, p, lambda sb: to_jxr(sb, convert=lambda a: (a >> 4).astype(np.uint8)))
    note(p, "resolution protocol: JPEG XR stream is 8-bit gray, directory declares gray16")
    p = out / "synthetic-jxr-mismatch-bgr24-as-gray8.czi"
    transcode(rgb, p, lambda sb: to_jxr(sb, declare=0))
    note(p, "resolution protocol: JPEG XR stream is 24-bit RGB, directory declares gray8")

    # --- multi-file ---
    src = out / "synthetic-multifile-source.czi"
    write_base(src, dtype=np.uint16, samples=1, sizes=dict(C=1, Z=1, T=3))
    note(src, "pylibCZIrw: gray16 T=3, the unsplit twin of synthetic-multifile")
    c = read_container(src)
    for sb in c.subblocks:
        t = sb.dim("T")
        sb.file_part = t.start if t else 0
    mf = out / "synthetic-multifile.czi"
    for old in out.glob("synthetic-multifile (*).czi"):
        old.unlink()
    paths = write_container(c, mf, parts=3)
    note(mf, "multi-file document: master holds T=0, parts '(1)' and '(2)' hold T=1 and T=2 (layout INFERRED)")
    for extra in paths[1:]:
        made.append(extra)
    return made


# Fixtures whose pixels czifile gets wrong (it casts a mismatched JPEG XR stream instead of
# converting it); their plane hashes come from pylibCZIrw, which applies the resolution protocol.
PYLIBCZIRW_ORACLE = {"synthetic-jxr-mismatch-bgr48-as-bgr24", "synthetic-jxr-mismatch-gray8-as-gray16"}
# Lossy JPEG fixtures: libjpeg-turbo (czifile's decoder) and a pure-Rust decoder differ by a few
# grey levels, so the harness compares these against czifile's decoded planes with a tolerance.
SIDECAR_PLANES = {"synthetic-gray8-jpeg", "synthetic-bgr24-jpeg", "synthetic-bgr24-jpeg444"}


def _xxh(a: np.ndarray) -> str:
    import xxhash
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(a).tobytes())


def oracle_pylibczirw(path: Path) -> None:
    """Replace czifile's plane hashes with pylibCZIrw's reading (RGB order, like openreadout)."""
    j = ROOT / "corpus/oracle" / f"{path.stem}.json"
    d = json.loads(j.read_text())
    with pyczi.open_czi(str(path)) as r:
        scenes = r.scenes_bounding_rectangle
        for im in d["images"]:
            for pl in im["planes"]:
                kw = {"plane": {"C": pl["c"], "Z": pl["z"], "T": pl["t"]}}
                if scenes:
                    kw["scene"] = im["index"]
                a = r.read(**kw)
                a = a[..., ::-1] if a.shape[-1] == 3 else a[..., 0]
                pl["xxh3"] = _xxh(a)
            im["pixel_type"] = {np.dtype("uint8"): "uint8", np.dtype("uint16"): "uint16"}[a.dtype]
    d["reader"] = "pylibCZIrw (geometry from czifile)"
    d["note"] = "plane hashes from pylibCZIrw, which converts a mismatched JPEG XR stream to the declared pixel type; czifile casts it instead"
    j.write_text(json.dumps(d, indent=1))


def oracle_sidecars(path: Path) -> None:
    """Write czifile's decoded planes as raw little-endian files: `<id>.oracle/image<i>_c<c>_z<z>_t<t>.bin`."""
    import czifile
    out = path.with_name(f"{path.stem}.oracle")
    out.mkdir(exist_ok=True)
    with czifile.CziFile(path) as f:
        for s in range(len(f.scenes)):
            xa = f.asxarray(scene=s)
            dims = list(xa.dims)
            arr = np.asarray(xa.values)
            shape = dict(zip(dims, arr.shape))
            for c in range(shape.get("C", 1)):
                for z in range(shape.get("Z", 1)):
                    for t in range(shape.get("T", 1)):
                        sel = tuple({"C": c, "Z": z, "T": t}.get(dd, slice(None) if dd in "YXS" else 0) for dd in dims)
                        (out / f"image{s}_c{c}_z{z}_t{t}.bin").write_bytes(np.ascontiguousarray(arr[sel]).astype(arr.dtype.newbyteorder("<")).tobytes())


def oracle(made: list[Path]) -> None:
    fixtures = [p for p in made if " (" not in p.name and p.stem != "synthetic-multifile"]
    subprocess.run([sys.executable, str(HERE / "gen.py"), *map(str, fixtures)], check=True)
    for p in fixtures:
        if p.stem in PYLIBCZIRW_ORACLE:
            oracle_pylibczirw(p)
        if p.stem in SIDECAR_PLANES:
            oracle_sidecars(p)
    # the multi-file oracle is czifile's reading of the unsplit twin
    twin = json.loads((ROOT / "corpus/oracle/synthetic-multifile-source.json").read_text())
    mf = next(p for p in made if p.stem == "synthetic-multifile")
    twin.update({"id": mf.stem, "file": mf.name, "size": mf.stat().st_size,
                 "sha256": hashlib.sha256(mf.read_bytes()).hexdigest(),
                 "note": "oracle = czifile reading synthetic-multifile-source.czi (the unsplit twin); czifile does not follow file parts"})
    (ROOT / "corpus/oracle/synthetic-multifile.json").write_text(json.dumps(twin, indent=1))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.environ.get("OPENREADOUT_CORPUS_DIR", str(ROOT / "corpus/files")))
    ap.add_argument("--oracle", action="store_true")
    a = ap.parse_args()
    made = build(Path(a.out))
    for p in made:
        print(f"{p.stat().st_size:>10}  {p.name}")
    for k, v in FIXTURES.items():
        print(f"# {k}: {v}")
    if a.oracle:
        oracle(made)


if __name__ == "__main__":
    main()
