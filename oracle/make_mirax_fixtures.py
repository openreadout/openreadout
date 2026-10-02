"""Write the synthetic MIRAX fixture `crates/openreadout-wsi/tests/fixtures/mirax-synthetic.mrxs`
(+ its data directory), laid out as docs/formats/mirax.md describes (our own writer; no vendor
code). Deterministic: rerun to reproduce it byte for byte.

- image grid 8 x 8 cells, CameraImageDivisionsPerSide 4: 2 x 2 camera photos of 16 x 12 pixels
  (four 4 x 3 BMP images per side); camera 2 (bottom left) has no images (flag 0)
- camera positions (level-0 pixels): cam 0 (0, 0), cam 1 (15, 1) (overlapping cam 0 by a column),
  cam 3 (14, 11)
- levels 0..3 with steps 1, 2, 4, 8: level 1 and 2 images inside one photo (fractional positions
  at level 1: cam 1 at x 7.5), level 3 one image of 2 x 2 subtiles of 2 x 1.5 pixels
- a label (BMP) and the position buffer as non-hierarchical records

    python oracle/make_mirax_fixtures.py
"""
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates" / "openreadout-wsi" / "tests" / "fixtures"
NAME = "mirax-synthetic"
SLIDE_ID = "0123456789ABCDEF0123456789ABCDEF"
W, H = 4, 3
NX, NY, DIV = 8, 8, 4
CAMS = {0: (0, 0), 1: (15, 1), 3: (14, 11)}


def bmp(pixels):
    """24-bit bottom-up BMP of rows of (r, g, b)."""
    h = len(pixels)
    w = len(pixels[0])
    row = (w * 3 + 3) & ~3
    body = b""
    for y in range(h - 1, -1, -1):
        r = b"".join(bytes([b, g, rr]) for (rr, g, b) in pixels[y])
        body += r + b"\0" * (row - len(r))
    head = b"BM" + struct.pack("<IHHI", 54 + len(body), 0, 0, 54)
    info = struct.pack("<IiiHHIIiiII", 40, w, h, 1, 24, 0, len(body), 2835, 2835, 0, 0)
    return head + info + body


def image(level, gx, gy):
    """Distinct, smooth content per image: value depends on level, grid cell and pixel."""
    return [[((gx * 16 + x * 3 + level * 50) % 256, (gy * 16 + y * 5 + level * 30) % 256, (40 + level * 60 + x + y) % 256)
             for x in range(W)] for y in range(H)]


def cam_of(gx, gy):
    return (gy // DIV) * (NX // DIV) + gx // DIV


def main():
    d = OUT / NAME
    d.mkdir(parents=True, exist_ok=True)
    steps = [1, 2, 4, 8]
    data = bytearray(b"01.01" + SLIDE_ID.encode() + b"000" + b"\0" * 256)
    items = []  # per level: [(index, offset, length, file)]
    for level, s in enumerate(steps):
        lv = []
        for gy in range(0, NY, s):
            for gx in range(0, NX, s):
                # an image exists when any camera it covers has images
                cams = {cam_of(x, y) for x in range(gx, min(gx + s, NX)) for y in range(gy, min(gy + s, NY))}
                if not cams & set(CAMS):
                    continue
                b = bmp(image(level, gx, gy))
                lv.append((gy * NX + gx, len(data), len(b), 0))
                data += b
        items.append(lv)
    label = bmp([[(200, 10 * x, 10 * y) for x in range(6)] for y in range(5)])
    label_item = (0, 0, len(data), len(label), 0)
    data += label
    pos = b""
    for k in range(4):
        x, y = CAMS.get(k, (0, 0))
        pos += struct.pack("<Bii", 1 if k in CAMS else 0, x, y)
    pos_item = (0, 0, len(data), len(pos), 0)
    data += pos
    (d / "Data0000.dat").write_bytes(bytes(data))

    # Index.dat: header, root tables, then one page list per record
    idx = bytearray(b"01.02" + SLIDE_ID.encode())
    roots_at = len(idx)
    idx += b"\0" * 8
    hier_records = items + [[], []]  # zoom levels, then filter levels (empty)
    nonhier_records = [[label_item], [pos_item]]
    hroot = len(idx)
    idx += b"\0" * 4 * len(hier_records)
    nroot = len(idx)
    idx += b"\0" * 4 * len(nonhier_records)

    def lists(root, records, width):
        nonlocal idx
        for k, rec in enumerate(records):
            if not rec:
                continue  # an empty record: pointer 0
            head = len(idx)
            idx += struct.pack("<ii", 0, 0)  # the empty first page
            page = len(idx)
            idx[head + 4:head + 8] = struct.pack("<i", page)
            idx += struct.pack("<ii", len(rec), 0)
            for it in rec:
                idx += struct.pack("<%di" % width, *it)
            idx[root + 4 * k:root + 4 * k + 4] = struct.pack("<i", head)

    lists(hroot, hier_records, 4)
    lists(nroot, nonhier_records, 5)
    idx[roots_at:roots_at + 8] = struct.pack("<ii", hroot, nroot)
    (d / "Index.dat").write_bytes(bytes(idx))

    lines = ["[GENERAL]", "SLIDE_VERSION = 01.03", "SLIDE_NAME = synthetic", f"SLIDE_ID = {SLIDE_ID}",
             f"IMAGENUMBER_X = {NX}", f"IMAGENUMBER_Y = {NY}", "CURRENT_SLIDE_VERSION = 1.9",
             "SLIDE_CREATIONDATETIME = 26/09/2026 12:00:00", "CAMERA_TYPE = Synthetic camera",
             "OBJECTIVE_MAGNIFICATION = 20", "OBJECTIVE_NAME = Default objective",
             "SLIDE_TYPE = SLIDE_TYPE_BRIGHTFIELD", f"CameraImageDivisionsPerSide = {DIV}",
             "[HIERARCHICAL]", "INDEXFILE = Index.dat", "HIER_COUNT = 2", "NONHIER_COUNT = 2",
             "HIER_0_NAME = Slide zoom level", f"HIER_0_COUNT = {len(steps)}"]
    for k in range(len(steps)):
        lines += [f"HIER_0_VAL_{k} = ZoomLevel_{k}", f"HIER_0_VAL_{k}_SECTION = LAYER_0_LEVEL_{k}_SECTION"]
    lines += ["HIER_1_NAME = Slide filter level", "HIER_1_COUNT = 2",
              "HIER_1_VAL_0 = FilterLevel_0", "HIER_1_VAL_0_SECTION = LAYER_1_LEVEL_0_SECTION",
              "HIER_1_VAL_1 = FilterLevel_1", "HIER_1_VAL_1_SECTION = LAYER_1_LEVEL_1_SECTION",
              "NONHIER_0_NAME = Scan data layer", "NONHIER_0_COUNT = 1",
              "NONHIER_0_VAL_0 = ScanDataLayer_SlideBarcode", "NONHIER_0_VAL_0_SECTION = NONHIERLAYER_0_LEVEL_0_SECTION",
              "NONHIER_1_NAME = VIMSLIDE_POSITION_BUFFER", "NONHIER_1_COUNT = 1",
              "NONHIER_1_VAL_0 = default", "NONHIER_1_VAL_0_SECTION = NONHIERLAYER_1_LEVEL_0_SECTION",
              "[DATAFILE]", "FILE_COUNT = 1", "FILE_0 = Data0000.dat"]
    for k, s in enumerate(steps):
        lines += [f"[LAYER_0_LEVEL_{k}_SECTION]", "IMAGE_FILL_COLOR_BGR = 16777215",
                  f"MICROMETER_PER_PIXEL_X = {0.25 * 2 ** k}", f"MICROMETER_PER_PIXEL_Y = {0.25 * 2 ** k}",
                  f"DIGITIZER_WIDTH = {W}", f"DIGITIZER_HEIGHT = {H}", f"OVERLAP_X = {1 / 2 ** k}",
                  f"OVERLAP_Y = {1 / 2 ** k}", f"IMAGE_CONCAT_FACTOR = {0 if k == 0 else 1}",
                  "IMAGE_FORMAT = BMP", "IMAGE_COMPRESSION_FACTOR = 100"]
    for k in range(2):
        lines += [f"[LAYER_1_LEVEL_{k}_SECTION]", "FILTER_NAME = Default", f"STORING_CHANNEL_NUMBER = {k}"]
    lines += ["[NONHIERLAYER_0_SECTION]", "SCANNER_SOFTWARE_VERSION = 1,2,3,4",
              "[NONHIERLAYER_0_LEVEL_0_SECTION]", "BARCODE_IMAGE_TYPE = BMP", "BARCODE_IMAGE_WIDTH = 6",
              "BARCODE_IMAGE_HEIGHT = 5"]
    (d / "Slidedat.ini").write_bytes(("﻿" + "\r\n".join(lines) + "\r\n").encode("utf-8"))
    # the .mrxs itself: a JPEG preview in real slides; a minimal JPEG start marker suffices here
    (OUT / f"{NAME}.mrxs").write_bytes(bytes([0xFF, 0xD8, 0xFF, 0xD9]))
    print("wrote", OUT / f"{NAME}.mrxs", "and", d)


if __name__ == "__main__":
    main()
