"""12-bit sequential DCT JPEG streams for the openreadout-codecs tests.

libjpeg-turbo (through imagecodecs, BSD-3) writes each stream and decodes it again; the
decoded samples are stored next to it as little-endian u16 (`<name>.expected`, interleaved
for colour). With `--tiff DIR` it also writes 12-bit JPEG TIFF pages with tifffile and
stores tifffile's decoded page (uint16, little-endian) as `<name>.expected`. Run from `oracle/`:

    uv run python make_jpeg12_fixtures.py ../crates/openreadout-codecs/tests/fixtures/jpeg12 \
        --tiff ../crates/openreadout-tiff/tests/fixtures/jpeg12
"""

import sys
from pathlib import Path

import imagecodecs
import numpy as np
import tifffile


def image(h, w, channels, seed):
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w]
    base = (x * 4095 // max(w - 1, 1) + y * 1000 // max(h - 1, 1)) % 4096
    out = []
    for c in range(channels):
        noise = rng.integers(0, 400, size=(h, w))
        out.append(np.clip(base + noise - 200 + 300 * c, 0, 4095))
    a = np.stack(out, -1).astype(np.uint16)
    return a[..., 0] if channels == 1 else a


def main(dest):
    dest = Path(dest)
    dest.mkdir(parents=True, exist_ok=True)
    cases = {
        "gray-37x29-q90": (image(29, 37, 1, 1), dict(level=90)),
        "gray-64x48-q100": (image(48, 64, 1, 2), dict(level=100)),
        "gray-40x24-q75-optimized": (image(24, 40, 1, 3), dict(level=75, optimize=True)),
        "ycbcr444-33x17-q95": (image(17, 33, 3, 4), dict(level=95, subsampling="444")),
        "rgb-20x12-q90": (image(12, 20, 3, 5), dict(level=90, colorspace="RGB", outcolorspace="RGB")),
    }
    for name, (a, kw) in cases.items():
        data = imagecodecs.jpeg8_encode(a, bitspersample=12, **kw)
        assert data[:2] == b"\xff\xd8"
        dec = imagecodecs.jpeg8_decode(data)
        assert dec.dtype == np.uint16 and dec.shape == a.shape, (dec.dtype, dec.shape)
        (dest / f"{name}.jpg").write_bytes(data)
        (dest / f"{name}.expected").write_bytes(dec.astype("<u2").tobytes())
        print(name, len(data), "bytes", a.shape)


def tiffs(dest):
    dest = Path(dest)
    dest.mkdir(parents=True, exist_ok=True)
    cases = {
        "gray12-tiled": (image(70, 90, 1, 6), dict(tile=(32, 32), compressionargs={"level": 90})),
        "rgb12-strips-ascoded": (
            image(40, 50, 3, 7),
            dict(photometric="rgb", rowsperstrip=16, compressionargs={"level": 90, "outcolorspace": "RGB"}),
        ),
    }
    for name, (a, kw) in cases.items():
        p = dest / f"{name}.tif"
        tifffile.imwrite(p, a, bitspersample=12, compression="jpeg", **kw)
        with tifffile.TiffFile(p) as t:
            page = t.pages[0]
            assert page.bitspersample == 12 and page.compression == 7
            dec = page.asarray()
        assert dec.dtype == np.uint16 and dec.shape == a.shape
        (dest / f"{name}.expected").write_bytes(dec.astype("<u2").tobytes())
        print(name, p.stat().st_size, "bytes", a.shape)


if __name__ == "__main__":
    main(sys.argv[1])
    if len(sys.argv) > 3 and sys.argv[2] == "--tiff":
        tiffs(sys.argv[3])
