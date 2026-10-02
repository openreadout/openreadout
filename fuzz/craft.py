#!/usr/bin/env python3
"""Write hand-crafted malformed files that exercise the allocation guards and overflow
robustness checks. Each one crashed (panic, overflow, OOM or stack
exhaustion) the readers before the hardening; they are kept as regression fixtures in
crates/<crate>/tests/fixtures/malformed/ next to the inputs the fuzzer found.

    python3 fuzz/craft.py            # (re)writes the crafted fixtures
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from seeds import czi_dir_entry, czi_segment, lv_i32, lv_level, lv_u32, nd2_chunk  # noqa: E402

REPO = Path(__file__).resolve().parent.parent


def out(crate: str, name: str, data: bytes) -> None:
    d = REPO / "crates" / f"openreadout-{crate}" / "tests" / "fixtures" / "malformed"
    d.mkdir(parents=True, exist_ok=True)
    (d / name).write_bytes(data)


# ------------------------------------------------------------------------------ CZI


def czi(subblocks: list[tuple[list, bytes]], xml: bytes = b"<ImageDocument/>", compression: int = 0, pixel_type: int = 0) -> bytes:
    """A CZI with one directory entry per (dims, pixels) subblock."""
    fh_len = 32 + 512
    meta = struct.pack("<ii", len(xml), 0).ljust(256, b"\0") + xml
    off = fh_len + 32 + len(meta)
    body = b""
    entries = []
    for dims, pixels in subblocks:
        entry = czi_dir_entry(pixel_type, off, compression, dims)
        sb = (struct.pack("<iiq", 0, 0, len(pixels)) + entry).ljust(256, b"\0") + pixels
        seg = czi_segment(b"ZISRAWSUBBLOCK", sb)
        body += seg
        entries.append(entry)
        off += len(seg)
    dir_off = off
    directory = struct.pack("<i", len(entries)).ljust(128, b"\0") + b"".join(entries)
    dseg = czi_segment(b"ZISRAWDIRECTORY", directory)
    guid = b"\x11" * 16
    fh = struct.pack("<ii", 1, 0) + b"\0" * 8 + guid + guid + struct.pack("<iqqiq", 0, dir_off, fh_len, 0, 0)
    return czi_segment(b"ZISRAWFILE", fh.ljust(512, b"\0")) + czi_segment(b"ZISRAWMETADATA", meta) + body + dseg


def tile(x: int, y: int, w: int = 4, h: int = 4, extra: list | None = None):
    dims = [("X", x, w, w), ("Y", y, h, h)] + (extra or [])
    return dims, bytes(w * h)


# ------------------------------------------------------------------------------ ND2


def nd2(attrs: list[bytes], loop: bytes | None = None, frames: list[bytes] | None = None, seq: bytes | None = None) -> bytes:
    chunks = [("ND2 FILE SIGNATURE CHUNK NAME01!", b"Ver3.0".ljust(64, b"\0")), ("ImageAttributesLV!", lv_level("SLxImageAttributes", attrs))]
    if loop is not None:
        chunks.append(("ImageMetadataLV!", loop))
    if seq is not None:
        chunks.append(("ImageMetadataSeqLV|0!", seq))
    for i, f in enumerate(frames or []):
        chunks.append((f"ImageDataSeq|{i}!", f))
    body = b""
    cm = b""
    for name, data in chunks:
        cm += name.encode() + struct.pack("<QQ", len(body), len(data))
        body += nd2_chunk(name, data)
    cm += b"ND2 CHUNK MAP SIGNATURE 0000001!" + struct.pack("<QQ", 0, 0)
    cm_off = len(body)
    return body + nd2_chunk("ND2 FILEMAP SIGNATURE NAME 0001!", cm) + b"ND2 CHUNK MAP SIGNATURE 0000001!" + struct.pack("<Q", cm_off)


def attrs(w=8, h=8, comp=1, stride=16, frames=1, extra=None):
    return [lv_u32("uiWidth", w), lv_u32("uiHeight", h), lv_u32("uiWidthBytes", stride), lv_u32("uiComp", comp),
            lv_u32("uiBpcInMemory", 16), lv_u32("uiSequenceCount", frames), lv_i32("ePixelType", 1)] + (extra or [])


def loop_tree(counts: list[int]) -> bytes:
    """A time loop with further time loops below it (counts outermost first)."""
    return lv_level("SLxExperiment", [lv_i32("eType", 1), lv_level("uLoopPars", [lv_u32("uiCount", counts[0])]),
                                      lv_level("ppNextLevelEx", [lv_level(f"i{k:010d}", [lv_i32("eType", 1), lv_level("uLoopPars", [lv_u32("uiCount", c)])]) for k, c in enumerate(counts[1:])])])


def frame(n: int) -> bytes:
    return b"\0" * 8 + bytes(n)


# ------------------------------------------------------------------------------ LIF


def lif(dims: str, channels: str = '<ChannelDescription DataType="0" Resolution="8" BytesInc="0"/>', memory: int = 64, pixels: int = 64) -> bytes:
    xml = ('<LMSDataContainerHeader Version="2"><Element Name="img"><Data><Image><ImageDescription>'
           f"<Channels>{channels}</Channels><Dimensions>{dims}</Dimensions></ImageDescription></Image></Data>"
           f'<Memory Size="{memory}" MemoryBlockID="MemBlock_1"/></Element></LMSDataContainerHeader>')
    xb = xml.encode("utf-16-le")
    head = struct.pack("<II", 0x70, len(xb) + 5) + b"\x2a" + struct.pack("<I", len(xb) // 2) + xb
    bid = "MemBlock_1".encode("utf-16-le")
    blk = struct.pack("<II", 0x70, len(bid) + 14) + b"\x2a" + struct.pack("<Q", pixels) + b"\x2a" + struct.pack("<I", len(bid) // 2)
    return head + blk + bid + bytes(pixels)


def dim(i: int, n: int, inc: int) -> str:
    return f'<DimensionDescription DimID="{i}" NumberOfElements="{n}" Length="1e-6" Unit="m" BytesInc="{inc}"/>'


def main() -> None:
    # CZI: a tile ~2^30 px away makes the stitched canvas 2^60 bytes.
    out("czi", "file-crafted-far-tile.czi", czi([tile(0, 0), tile(1 << 30, 1 << 30)]))
    # CZI: Z start at i32::MAX with size 2 overflows the scene extent.
    out("czi", "file-crafted-z-extent-overflow.czi", czi([tile(0, 0, extra=[("Z", 0x7FFFFFFF, 2, 2)])]))
    # CZI: channel colour with an 8-byte non-ASCII value (string slicing mid-character).
    xml = '<ImageDocument><Metadata><Information><Image><Dimensions><Channels><Channel Id="c"><Color>#a€€b</Color></Channel></Channels></Dimensions></Image></Information></Metadata></ImageDocument>'
    out("czi", "file-crafted-non-ascii-colour.czi", czi([tile(0, 0)], xml=xml.encode()))
    # CZI: a zstd tile that declares 65535 x 65535 pixels (4 GiB) in a tiny file.
    out("czi", "file-crafted-huge-zstd-tile.czi", czi([([("X", 0, 65535, 65535), ("Y", 0, 65535, 65535)], b"\x28\xb5\x2f\xfd" + bytes(16))], compression=5))

    # ND2: uiComp = 2^32 - 1 (one channel entry per component).
    out("nd2", "file-crafted-huge-components.nd2", nd2(attrs(comp=0xFFFFFFFF), frames=[frame(128)]))
    # ND2: row stride shorter than one row of pixels.
    out("nd2", "file-crafted-short-stride.nd2", nd2(attrs(stride=2), frames=[frame(128)]))
    # ND2: zlib frames declaring a 64 Ki x 64 Ki plane (8 GiB).
    out("nd2", "file-crafted-huge-zlib-frame.nd2", nd2(attrs(w=65536, h=65536, stride=131072, extra=[lv_i32("eCompression", 0)]), frames=[b"\0" * 8 + b"\x78\x9c\x03\x00\x00\x00\x00\x01"]))
    # ND2: nested loops whose counts multiply past u32::MAX.
    out("nd2", "file-crafted-loop-overflow.nd2", nd2(attrs(frames=1), loop=loop_tree([65536, 65536, 65536]), frames=[frame(128)]))
    # ND2: uiSequenceCount = 4e9 with one frame (exporters enumerate every declared plane).
    out("nd2", "file-crafted-huge-frame-count.nd2", nd2(attrs(frames=4000000000), frames=[frame(128)]))
    # LV: 6 000 nested levels (stack exhaustion when decoded recursively).
    deep = b""
    for _ in range(6000):
        deep = lv_level("L", [deep]) if deep else lv_u32("n", 1)
    out("nd2", "lv-crafted-deep-nesting.bin", deep)
    # LV: a byte array whose declared length is 2^64 - 1.
    out("nd2", "lv-crafted-huge-array.bin", bytes([9]) + b"\x02a\x00\x00\x00" + struct.pack("<Q", 2**64 - 1))

    # LIF: X has zero elements.
    out("lif", "file-crafted-zero-width.lif", lif(dim(1, 0, 1) + dim(2, 8, 8)))
    # LIF: 4 billion mosaic tiles (one exposed image per tile).
    out("lif", "file-crafted-huge-mosaic.lif", lif(dim(1, 8, 1) + dim(2, 8, 8) + dim(10, 4000000000, 64)))
    # LIF: Z byte increment near 2^64 overflows the plane offset.
    out("lif", "file-crafted-increment-overflow.lif", lif(dim(1, 8, 1) + dim(2, 8, 8) + dim(3, 4, 18446744073709551000)))
    print("crafted fixtures written")


if __name__ == "__main__":
    main()
