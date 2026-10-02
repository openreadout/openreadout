"""High-content screening plates (family `hcs`): lookup and integrity questions about plate
folders (a Harmony measurement folder, a CellVoyager measurement folder, an ImageXpress plate
folder). Answers come from the plates' ground truth `corpus/oracle/<id>.json`, which
`oracle/hcs.py` wrote from the plate index parsed with the Python standard library, tifffile on
the plane files and Bio-Formats (black box); never from OpenReadout. The analysis-tier plate
questions (pixel means) live in `analysis.py`.

`generate.py` imports `HCS_SPECS` lazily (this module imports `generate`).
"""

from __future__ import annotations

import generate as g


def fields_per_well(o: dict) -> int:
    """Most fields of view recorded for one well (the oracle lists the image indices per well)."""
    return max(len(w["images"]) for w in o["hcs"]["wells"])


def missing_in_well(o: dict, well: str) -> int:
    """Plane files the index names for `well` that are not in the copy."""
    return sum(1 for im in o["hcs"]["images"] if im["well"] == well for pl in im["planes"] if pl["state"] == "missing")


HCS_SPECS: list[g.Spec] = [
    g.Spec(
        "hcs-harmony-wells-imaged",
        "hcs-harmony-idr0034-folder",
        "counts",
        "This folder is a high-content screening plate exported from the imager. How many wells of the "
        "plate were imaged (have images listed in the plate's index)?",
        lambda o, m: g.integer(g.pointer(o, "/hcs/plate/wells_imaged")),
        "oracle: /hcs/plate/wells_imaged (Index.idx.xml parsed with the Python standard library)",
        stage_as="sample_plate",
        answer_hint="the number of wells",
    ),
    g.Spec(
        "hcs-harmony-fields-per-well",
        "hcs-harmony-idr0034-folder",
        "counts",
        "How many fields of view (sites) were imaged per well on this plate (the most for any well)?",
        lambda o, m: g.integer(fields_per_well(o)),
        "oracle: /hcs/wells/*/images, largest count (Index.idx.xml parsed with the Python standard library)",
        stage_as="sample_plate",
        answer_hint="the number of fields per well",
    ),
    g.Spec(
        "hcs-harmony-well-missing-files",
        "hcs-harmony-idr0034-folder",
        "integrity",
        "Some image files of this plate were not copied. In well C07 (row C, column 7), how many of the "
        "image files that the plate's index names are missing from this folder?",
        lambda o, m: g.integer(missing_in_well(o, "C07")),
        "oracle: /hcs/images (well C07) planes with state `missing` (index file names checked against the folder)",
        stage_as="sample_plate",
        answer_hint="the number of missing image files",
    ),
    g.Spec(
        "hcs-imagexpress-complete",
        "hcs-imagexpress-idr0081-folder",
        "integrity",
        "This ImageXpress plate folder was copied from the acquisition PC. Are all the images the plate "
        "description (.HTD) calls for present in this copy?",
        lambda o, m: g.boolean(g.pointer(o, "/hcs/plate/planes_missing") == 0),
        "oracle: /hcs/plate/planes_missing (HTD parsed as text; "
        "expected wells x sites x wavelengths checked against the folder)",
        stage_as="sample_plate",
        answer_hint="yes or no",
    ),
    g.Spec(
        "hcs-cellvoyager-channels",
        "hcs-cellvoyager-jump-1053601756-folder",
        "channels",
        "This folder is a Yokogawa CellVoyager measurement of a Cell Painting plate. Which channels were "
        "acquired? Give the channel names (targets) as the measurement settings record them.",
        lambda o, m: g.items(g.pointer(o, "/hcs/channels")),
        "oracle: /hcs/channels (the .mes channel targets, parsed with the Python standard library)",
        stage_as="sample_plate",
        answer_hint="the channel names, comma-separated",
    ),
    g.Spec(
        "hcs-cellvoyager-pixel-size",
        "hcs-cellvoyager-jump-1053601756-folder",
        "pixel-size",
        "What is the pixel size of the images of this plate, in micrometres?",
        lambda o, m: g.number(g.pointer(o, "/hcs/pixel_size_um/0"), "µm", rel=0.001),
        "oracle: /hcs/pixel_size_um/0 (MeasurementDetail.mrf HorizontalPixelDimension, Python standard library)",
        stage_as="sample_plate",
        answer_hint="a number in µm",
    ),
]
