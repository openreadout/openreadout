"""Write the synthetic VMS fixture `crates/openreadout-wsi/tests/fixtures/vms-synthetic/synthetic.vms` as
docs/formats/hamamatsu-vms.md describes (our own writer with Pillow's JPEG encoder; no vendor
code). Rerun to reproduce it (byte for byte with the same Pillow/libjpeg).

- a 2 x 2 grid: columns 64 and 32 pixels wide, rows 24 and 16 high (96 x 40 in all)
- baseline JPEGs without chroma subsampling, a restart marker every 4 MCUs (32-pixel intervals)
- a map at 1/8 (12 x 5), a macro photo, and an optimisation file of 40-byte entries listing each
  MCU row's offset, without the last row of the last image

    python oracle/make_vms_fixtures.py
"""
import io
import struct
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates" / "openreadout-wsi" / "tests" / "fixtures" / "vms-synthetic"
COLS, ROWS = [64, 32], [24, 16]


def jpeg(img, blocks):
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=92, subsampling=0, restart_marker_blocks=blocks)
    return buf.getvalue()


def pattern(w, h, x0, y0):
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            gx, gy = x0 + x, y0 + y
            px[x, y] = ((gx * 5) % 256, (gy * 9) % 256, ((gx + gy) * 3) % 256)
    return im


def row_offsets(data, per_row, rows):
    """Offsets where MCU rows start: after SOS, then after every `per_row`-th restart marker."""
    i = 2
    while True:
        m, ln = data[i + 1], struct.unpack(">H", data[i + 2:i + 4])[0]
        i += 2 + ln
        if m == 0xDA:
            break
    out, n, j = [i], 0, i
    while len(out) < rows and j + 1 < len(data):
        if data[j] == 0xFF and 0xD0 <= data[j + 1] <= 0xD7:
            n += 1
            if n % per_row == 0:
                out.append(j + 2)
            j += 2
        else:
            j += 1
    return out


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    full = Image.new("RGB", (sum(COLS), sum(ROWS)))
    names, opt = {}, b""
    y0 = 0
    for r, h in enumerate(ROWS):
        x0 = 0
        for c, w in enumerate(COLS):
            im = pattern(w, h, x0, y0)
            full.paste(im, (x0, y0))
            data = jpeg(im, 4)
            name = "synthetic.jpg" if (c, r) == (0, 0) else f"synthetic({c},{r}).jpg"
            (OUT / name).write_bytes(data)
            names[(c, r)] = name
            per_row = (w // 8 + 3) // 4
            offs = row_offsets(data, per_row, h // 8)
            if (c, r) == (len(COLS) - 1, len(ROWS) - 1):
                offs = offs[:-1]  # the last row of the last image is not listed
            opt += b"".join(struct.pack("<Q", o) + bytes(32) for o in offs)
            x0 += w
        y0 += h
    (OUT / "synthetic.opt").write_bytes(opt)
    (OUT / "synthetic_map2.jpg").write_bytes(jpeg(full.resize((sum(COLS) // 8, sum(ROWS) // 8), Image.BOX), 1))
    (OUT / "synthetic_macro.jpg").write_bytes(jpeg(Image.new("RGB", (16, 8), (90, 60, 120)), 0))
    lines = ["[Virtual Microscope Specimen]", "NoLayers=1", f"NoJpegColumns={len(COLS)}", f"NoJpegRows={len(ROWS)}",
             f"ImageFile={names[(0, 0)]}"]
    lines += [f"ImageFile({c},{r})={n}" for (c, r), n in sorted(names.items()) if (c, r) != (0, 0)]
    lines += ["MapFile=synthetic_map2.jpg", "OptimisationFile=synthetic.opt", "AuthCode=0", "SourceLens=20.000000",
              f"PhysicalWidth={sum(COLS) * 500}", f"PhysicalHeight={sum(ROWS) * 500}", "LayerSpacing=0",
              "MacroImage=synthetic_macro.jpg", "PhysicalMacroWidth=76000000", "PhysicalMacroHeight=26000000",
              "XOffsetFromSlideCentre=1000", "YOffsetFromSlideCentre=-2000", "Reference=synthetic"]
    (OUT / "synthetic.vms").write_bytes(("\r\n".join(lines) + "\r\n").encode())
    print(f"wrote {len(list(OUT.iterdir()))} files in {OUT}")


if __name__ == "__main__":
    main()
