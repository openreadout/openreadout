"""Write the synthetic iSyntax fixture `crates/openreadout-wsi/tests/fixtures/isyntax-synthetic.isyntax`,
laid out as docs/formats/philips-isyntax.md describes (our own writer; no vendor code).
Deterministic: rerun to reproduce it byte for byte.

- data model 5.0, compression method 16, code blocks of 8 x 8, 2 scales: padded image 64 x 48
  (padding 9: the image is 46 x 30), tile grids 4 x 4 (scale 0) and 2 x 2 (scale 1)
- top-scale tiles (0, 0), (1, 0) and (0, 1) exist, (1, 1) does not (white); every child of an
  existing top tile exists
- LL blocks hold a smooth pattern per colour, a few high-pass blocks hold coefficients, the
  others are empty (8 bytes)
- code blocks written with all 16 bit planes per band, a Huffman tree of all 256 byte values at
  depth 8, and no zero runs (counter width 0)
- a label and a macro JPEG (4 x 2 pixels, written with Pillow) and an ICC profile stand-in

`--check` decodes the written file with libisyntax (pyisyntax, BSD-2-Clause / MIT) and prints the
mean of each level, the values tests/isyntax_synthetic.rs pins.

    python oracle/make_isyntax_fixtures.py [--check]
"""
import base64
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates" / "openreadout-wsi" / "tests" / "fixtures" / "isyntax-synthetic.isyntax"

BW = BH = 8
LEVELS = 2
PAD = 3 * (1 << LEVELS) - 3  # 9
PW, PH = 64, 48
OFF_X, OFF_Y = 1000, 2000
GRID = (((PW + (BW << LEVELS) - 1) // (BW << LEVELS)) << (LEVELS - 1),
        ((PH + (BH << LEVELS) - 1) // (BH << LEVELS)) << (LEVELS - 1))  # (4, 4)
TOP_TILES = [(0, 0), (1, 0), (0, 1)]


def tiny_jpeg(rgb):
    """A 4 x 2 baseline JPEG of one colour (Pillow), for the label and macro attachments."""
    import io
    from PIL import Image
    buf = io.BytesIO()
    Image.new("RGB", (4, 2), rgb).save(buf, "JPEG", quality=90)
    return buf.getvalue()


class Bits:
    def __init__(self):
        self.buf = bytearray()
        self.n = 0

    def put(self, v, nbits):
        for i in range(nbits):
            if self.n % 8 == 0:
                self.buf.append(0)
            self.buf[-1] |= ((v >> i) & 1) << (self.n % 8)
            self.n += 1


def tree_walk(bits):
    """All 256 byte values at depth 8, symbol k at the k-th leaf in preorder."""
    sym = [0]

    def enc(depth):
        if depth == 8:
            bits.put(1, 1)
            bits.put(sym[0], 8)
            sym[0] += 1
        else:
            bits.put(0, 1)
            enc(depth + 1)
            enc(depth + 1)

    enc(0)


def encode_block(planes):
    """`planes`: list of BW*BH lists of signed ints (1 band for LL, 3 for high-pass)."""
    if all(v == 0 for p in planes for v in p):
        return bytes(8)  # an empty block
    area = BW * BH
    data = bytearray()
    for band in planes:
        # signed magnitude, then 4x4 areas in raster order of areas
        sm = [(abs(v) & 0x7FFF) | (0x8000 if v < 0 else 0) for v in band]
        order = []
        for ay in range(0, BH, 4):
            for ax in range(0, BW, 4):
                for r in range(4):
                    for q in range(4):
                        order.append(sm[(ay + r) * BW + ax + q])
        for bit in range(16):  # mask bit 0 = sign (sample bit 15), bit b = magnitude bit b - 1
            shift = 15 if bit == 0 else bit - 1
            plane = bytearray(area // 8)
            for i, v in enumerate(order):
                if (v >> shift) & 1:
                    plane[i // 8] |= 1 << (i % 8)
            data += plane
    b = Bits()
    b.put(len(data), 32)
    b.put(0, 8)  # zero-run symbol
    b.put(0, 8)  # counter width 0: the symbol always stands for itself
    tree_walk(b)
    for byte in data:
        # code of symbol k at depth 8: the branch decisions are the bits of k, most significant first
        code = int(f"{byte:08b}"[::-1], 2)
        b.put(code, 8)
    out = bytes(b.buf)
    return out + bytes(max(0, 9 - len(out)))


def ll_band(color, tx, ty):
    out = []
    for y in range(BH):
        for x in range(BW):
            if color == 0:
                out.append(120 + 6 * x + 4 * y + 30 * tx - 20 * ty)
            elif color == 1:
                out.append(-40 + 5 * x - 3 * tx * 8)
            else:
                out.append(20 - 4 * y + 10 * ty)
    return out


def high_bands(scale, color, tx, ty):
    if (scale, tx, ty, color) in ((0, 1, 1, 0), (1, 0, 0, 1), (0, 0, 2, 2)):
        hl = [((x * 3 + y) % 7) - 3 for y in range(BH) for x in range(BW)]
        lh = [((x + y * 5) % 5) - 2 for y in range(BH) for x in range(BW)]
        hh = [(x - y) // 3 for y in range(BH) for x in range(BW)]
        return [hl, lh, hh]
    return [[0] * (BW * BH)] * 3


def coords(scale, tx, ty, ll):
    off = (3 << scale) - 2 + ((1 << scale) if ll else 0)
    step = (2 * BW) << scale
    return OFF_X + off + tx * step, OFF_Y + off + ty * step


def tile_count(s):
    return (GRID[0] * GRID[1]) >> (2 * s)


def block_id(scale, ll, color, tx, ty):
    top = LEVELS - 1
    total = sum(tile_count(s) for s in range(LEVELS)) + tile_count(top)
    bid = sum(tile_count(s) for s in range(scale + 1 if ll else scale))
    return bid + ty * (GRID[0] >> scale) + tx + color * total


def build():
    # Chunks: one per top tile; per colour the top high-pass block, the four children, the LL block.
    blocks = []  # (x, y, color, scale, coef, template, data, block id)
    for (tx, ty) in TOP_TILES:
        for color in range(3):
            x, y = coords(1, tx, ty, False)
            blocks.append((x, y, color, 1, 1, 0, encode_block(high_bands(1, color, tx, ty)), block_id(1, False, color, tx, ty)))
            for (cx, cy) in [(2 * tx, 2 * ty), (2 * tx + 1, 2 * ty), (2 * tx, 2 * ty + 1), (2 * tx + 1, 2 * ty + 1)]:
                x, y = coords(0, cx, cy, False)
                blocks.append((x, y, color, 0, 1, 1, encode_block(high_bands(0, color, cx, cy)), block_id(0, False, color, cx, cy)))
            x, y = coords(1, tx, ty, True)
            blocks.append((x, y, color, 1, 0, 2, encode_block([ll_band(color, tx, ty)]), block_id(1, True, color, tx, ty)))
    table = bytearray()
    for (x, y, c, s, coef, tmpl, _, _) in blocks:
        table += struct.pack("<HHI", 0xFFFE, 0xE000, 40)
        table += struct.pack("<HHI", 0x301D, 0x200E, 20) + struct.pack("<5I", x, y, c, s, coef)
        table += struct.pack("<HHI", 0x301D, 0x2012, 4) + struct.pack("<I", tmpl)
    table = struct.pack("<I", len(table)) + table

    def attr(name, group, element, typ, value):
        return f'<Attribute Name="{name}" Group="{group}" Element="{element}" PMSVR="{typ}">{value}</Attribute>'

    def ranges(rs):
        items = "".join(f'<DataObject ObjectType="UFSImageDimensionRange">\n{attr("UFS_IMAGE_DIMENSION_RANGE", "0x301D", "0x200B", "IInt32Array", f"{a} {b} {c}")}</DataObject>\n' for (a, b, c) in rs)
        return f'<Attribute Name="UFS_IMAGE_DIMENSION_RANGES" Group="0x301D" Element="0x200A" PMSVR="IDataObjectArray">\n<Array>\n{items}</Array>\n</Attribute>\n'

    templates = ""
    for (s, coef) in [(1, 1), (0, 1), (1, 0)]:
        templates += ('<DataObject ObjectType="UFSImageBlockHeaderTemplate">\n'
                      + attr("UFS_IMAGE_BLOCK_COMPRESSION_METHOD", "0x301D", "0x200F", "IString", "16")
                      + ranges([(0, 1, BW - 1), (0, 1, BH - 1), (0, 1, 2), (s, 0, s), (0 if coef == 0 else 1, 1, 0 if coef == 0 else 3)])
                      + "</DataObject>\n")
    dims = ""
    for name, unit, scale in [("x", "MicroMeter", "0.25"), ("y", "MicroMeter", "0.25")]:
        dims += ('<DataObject ObjectType="UFSImageDimension">\n'
                 + attr("UFS_IMAGE_DIMENSION_NAME", "0x301D", "0x2004", "IString", name)
                 + attr("UFS_IMAGE_DIMENSION_UNIT", "0x301D", "0x2006", "IString", unit)
                 + attr("UFS_IMAGE_DIMENSION_SCALE_FACTOR", "0x301D", "0x2007", "IDouble", scale)
                 + "</DataObject>\n")
    general = ('<DataObject ObjectType="UFSImageGeneralHeader">\n'
               + attr("UFS_IMAGE_NUMBER_OF_BLOCKS", "0x301D", "0x2001", "IUInt32", str(len(blocks)))
               + f'<Attribute Name="UFS_IMAGE_DIMENSIONS" Group="0x301D" Element="0x2003" PMSVR="IDataObjectArray">\n<Array>\n{dims}</Array>\n</Attribute>\n'
               + ranges([(OFF_X, 1, OFF_X + PW - 1), (OFF_Y, 1, OFF_Y + PH - 1), (0, 1, 2), (0, 1, LEVELS - 1), (0, 1, 3)])
               + "</DataObject>\n")
    icc = base64.b64encode(b"synthetic icc profile stand-in").decode()
    wsi = ('<DataObject ObjectType="DPScannedImage">\n'
           + attr("PIM_DP_IMAGE_TYPE", "0x301D", "0x1004", "IString", "WSI")
           + attr("DICOM_DERIVATION_DESCRIPTION", "0x0008", "0x2111", "IString", "OpenReadout synthetic fixture | Compressor=16")
           + attr("DICOM_LOSSY_IMAGE_COMPRESSION", "0x0028", "0x2110", "IString", "01")
           + attr("UFS_IMAGE_BLOCK_HEADER_TABLE", "0x301D", "0x2014", "IString", base64.b64encode(table).decode())
           + f'\n<Attribute Name="UFS_IMAGE_BLOCK_HEADER_TEMPLATES" Group="0x301D" Element="0x2009" PMSVR="IDataObjectArray">\n<Array>\n{templates}</Array>\n</Attribute>\n'
           + f'<Attribute Name="UFS_IMAGE_GENERAL_HEADERS" Group="0x301D" Element="0x2000" PMSVR="IDataObjectArray">\n<Array>\n{general}</Array>\n</Attribute>\n'
           + f'<Attribute Name="DP_COLOR_MANAGEMENT" Group="0x301D" Element="0x1013" PMSVR="IDataObjectArray">\n<Array>\n<DataObject ObjectType="DPColorManagement">\n{attr("DICOM_ICCPROFILE", "0x0028", "0x2000", "IString", icc)}</DataObject>\n</Array>\n</Attribute>\n'
           + "</DataObject>\n")
    label = ('<DataObject ObjectType="DPScannedImage">\n' + attr("PIM_DP_IMAGE_TYPE", "0x301D", "0x1004", "IString", "LABELIMAGE")
             + attr("PIM_DP_IMAGE_DATA", "0x301D", "0x1005", "IString", base64.b64encode(tiny_jpeg((200, 40, 40))).decode())
             + "</DataObject>\n")
    macro = ('<DataObject ObjectType="DPScannedImage">\n' + attr("PIM_DP_IMAGE_TYPE", "0x301D", "0x1004", "IString", "MACROIMAGE")
             + attr("PIM_DP_IMAGE_DATA", "0x301D", "0x1005", "IString", base64.b64encode(tiny_jpeg((40, 40, 200))).decode())
             + "</DataObject>\n")
    top = ('<?xml version="1.0" encoding="UTF-8"?>\n<DataObject ObjectType="DPUfsImport">\n'
           + attr("PIM_DP_UFS_INTERFACE_VERSION", "0x301D", "0x1001", "IString", "5.0")
           + attr("DICOM_MANUFACTURER", "0x0008", "0x0070", "IString", "OpenReadout")
           + attr("DICOM_MANUFACTURERS_MODEL_NAME", "0x0008", "0x1090", "IString", "synthetic")
           + attr("DICOM_SOFTWARE_VERSIONS", "0x0018", "0x1020", "IStringArray", '"1.0.0"')
           + attr("DICOM_ACQUISITION_DATETIME", "0x0008", "0x002A", "IString", "20261012120000.000000")
           + attr("PIM_DP_UFS_BARCODE", "0x301D", "0x1002", "IString", base64.b64encode(b"SYNTH-1").decode())
           + f'<Attribute Name="PIM_DP_SCANNED_IMAGES" Group="0x301D" Element="0x1003" PMSVR="IDataObjectArray">\n<Array>\n{wsi}{label}{macro}</Array>\n</Attribute>\n'
           + "</DataObject>\n")
    head = top.encode() + b"\r\n\x04"
    entries = 3 * (sum(tile_count(s) for s in range(LEVELS)) + tile_count(LEVELS - 1))
    seek_start = len(head) + 8
    data_start = seek_start + entries * 40
    seek = bytearray(entries * 40)
    data = bytearray()
    for (_, _, _, _, _, _, payload, bid) in blocks:
        off = data_start + len(data)
        struct.pack_into("<HHI", seek, bid * 40, 0xFFFE, 0xE000, 32)
        struct.pack_into("<HHIQ", seek, bid * 40 + 8, 0x301D, 0x2010, 8, off)
        struct.pack_into("<HHIQ", seek, bid * 40 + 24, 0x301D, 0x2011, 8, len(payload))
        data += payload
    return head + struct.pack("<HHI", 0x301D, 0x2015, len(seek)) + bytes(seek) + bytes(data)


def main():
    OUT.write_bytes(build())
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
    if "--check" in sys.argv:
        import numpy as np
        import isyntax as pyisyntax
        with pyisyntax.ISyntax.open(OUT) as s:
            print("libisyntax dimensions", s.dimensions, s.level_dimensions)
            for lv, (w, h) in enumerate(s.level_dimensions):
                a = s.read_region(0, 0, w, h, level=lv)[..., :3]
                print(f"level {lv}: mean {a.mean():.6f}, first pixel {a[0, 0].tolist()}, last {a[-1, -1].tolist()}")
                np.save(OUT.with_suffix(f".l{lv}.npy"), a) if "--save" in sys.argv else None


if __name__ == "__main__":
    main()
