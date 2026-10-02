# Codec samples (WebP, JPEG XL, LERC)

The `.tif` files are GDAL autotest data written by GDAL's own GTiff driver, copied unchanged
from https://github.com/OSGeo/gdal/tree/7617801fa6b2c9be05b7775b536647925c1bf66f/autotest/gcore/data
(`gtiff/` and the top level). GDAL is licensed under the MIT licence ("GDAL/OGR General" in its
LICENSE.TXT): Copyright (c) the GDAL/OGR contributors; permission is granted, free of charge, to
use, copy, modify, merge, publish, distribute, sublicense and/or sell copies, provided the
copyright and permission notice are included; provided "as is", without warranty of any kind.

Each `<name>.expected` holds the page's samples (little-endian, interleaved, row-major) as
tifffile 2026.9.20 + imagecodecs 2026.8.16 decode them (libwebp, libjxl, Esri lerc).
