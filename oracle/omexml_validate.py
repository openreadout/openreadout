#!/usr/bin/env python
"""Validate the OME-XML embedded in OME-TIFF files against the OME 2016-06 XSD, and check
that tifffile + bioio can read the planes. Usage: uv run python omexml_validate.py file.ome.tiff ..."""
import sys
from pathlib import Path
import tifffile
from lxml import etree

XSD = Path(__file__).resolve().parent / "schema" / "ome.xsd"
schema = etree.XMLSchema(etree.parse(str(XSD)))

ok = True
for arg in sys.argv[1:]:
    with tifffile.TiffFile(arg) as tf:
        xml = tf.ome_metadata
        assert xml, f"{arg}: no OME-XML in ImageDescription"
        doc = etree.fromstring(xml.encode("utf-8"))
        valid = schema.validate(doc)
        series = [(s.shape, s.axes, s.dtype) for s in tf.series]
        big = tf.is_bigtiff
        print(f"{'VALID  ' if valid else 'INVALID'} bigtiff={big} series={series} {arg}")
        if not valid:
            ok = False
            for e in schema.error_log:
                print("   ", e.message[:200])
    try:
        from bioio import BioImage
        img = BioImage(arg)
        print("    bioio:", img.dims, img.dtype, "scenes", img.scenes, "px", img.physical_pixel_sizes, "ch", img.channel_names)
    except Exception as e:
        print("    bioio failed:", repr(e)[:200]); ok = False
sys.exit(0 if ok else 1)
