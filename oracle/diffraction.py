"""Ground truth for diffraction images and SerialEM metadata (called from gen.py).

- CBF / miniCBF: FabIO (MIT) decodes the byte-offset image; its PILATUS header parser gives the
  geometry, converted to our units (mm, Å, pixels).
- SMV (.img): FabIO reads the header and the image; the beam centre follows the convention dxtbx
  (BSD-3) documents: millimetres with X along the slow axis on ADSC-style files, pixels along the
  fast and slow axes on the Stockholm University TimePix (`BEAMLINE=TimePix_SU`).
- NeXus NXmx (EIGER master + data): h5py with hdf5plugin (bitshuffle/LZ4) through the master's
  external links; geometry from the NXmx fields.
- SerialEM .mdoc: mdocfile (BSD-3) parses the sections; the table columns we check are hashed
  as little-endian float64.
"""
from pathlib import Path

import numpy as np
import xxhash


def _h(a):
    a = np.ascontiguousarray(a)
    if a.dtype.byteorder == ">":
        a = a.byteswap().view(a.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(a.tobytes())


_DT = {"int32": "int32", "uint16": "uint16", "uint32": "uint32", "int16": "int16", "uint8": "uint8", "float32": "float"}


def cbf(p: Path) -> dict:
    import fabio

    im = fabio.open(str(p))
    a = im.data
    ph = getattr(im, "pilatus_headers", None)
    geo = {}
    if ph is not None:
        get = lambda k: ph[k] if k in ph else None  # noqa: E731
        if get("Wavelength") is not None:
            geo["wavelength_a"] = float(get("Wavelength"))
        if get("Detector_distance") is not None:
            geo["distance_mm"] = float(get("Detector_distance")) * 1e3
        if get("Beam_xy") is not None:
            geo["beam_center_px"] = [float(v) for v in get("Beam_xy")]
        if get("Pixel_size") is not None:
            geo["pixel_size_mm"] = [float(v) * 1e3 for v in get("Pixel_size")]
        if get("Start_angle") is not None:
            geo["oscillation_start_deg"] = float(get("Start_angle"))
        if get("Angle_increment") is not None:
            geo["oscillation_width_deg"] = float(get("Angle_increment"))
        if get("Exposure_time") is not None:
            geo["exposure_s"] = float(get("Exposure_time"))
        if get("Count_cutoff") is not None:
            geo["overload"] = float(get("Count_cutoff"))
    img = {"index": 0, "size_x": int(a.shape[1]), "size_y": int(a.shape[0]), "size_z": 1, "size_c": 1, "size_t": 1,
           "pixel_type": _DT[a.dtype.name], "physical_size_um": {},
           "planes": [{"c": 0, "z": 0, "t": 0, "xxh3": _h(a)}], "check_diffraction": geo}
    return {"reader": f"fabio {fabio.version}", "kind": "cbf", "images": [img],
            "counts": {"min": int(a.min()), "max": int(a.max()), "negative": int((a < 0).sum())}}


def smv(p: Path) -> dict:
    import fabio

    im = fabio.open(str(p))
    hd = {k.upper(): v for k, v in im.header.items()}
    a = im.data
    num = lambda k: float(hd[k].strip().rstrip(";")) if k in hd else None  # noqa: E731
    geo = {}
    for ours, key in (("wavelength_a", "WAVELENGTH"), ("distance_mm", "DISTANCE"), ("oscillation_width_deg", "OSC_RANGE"),
                      ("exposure_s", "TIME")):
        if num(key) is not None:
            geo[ours] = num(key)
    if num("OSC_START") is not None:
        geo["oscillation_start_deg"] = num("OSC_START")
    px = num("PIXEL_SIZE")
    if px:
        geo["pixel_size_mm"] = [px, px]
    bx, by = num("BEAM_CENTER_X"), num("BEAM_CENTER_Y")
    if bx is not None and by is not None:
        if hd.get("BEAMLINE", "").strip().upper() == "TIMEPIX_SU":
            geo["beam_center_px"] = [bx, by]
        elif px:
            geo["beam_center_px"] = [by / px, bx / px]
    img = {"index": 0, "size_x": int(a.shape[1]), "size_y": int(a.shape[0]), "size_z": 1, "size_c": 1, "size_t": 1,
           "pixel_type": _DT[a.dtype.name], "physical_size_um": {},
           "planes": [{"c": 0, "z": 0, "t": 0, "xxh3": _h(a)}], "check_diffraction": geo}
    return {"reader": f"fabio {fabio.version} ({type(im).__name__})", "kind": "smv", "images": [img]}


def nxmx(p: Path, max_planes: int) -> dict:
    import h5py
    import hdf5plugin  # noqa: F401  (bitshuffle/LZ4 filters)

    f = h5py.File(p, "r")
    data = f["/entry/data"]
    links = sorted(k for k in data.keys() if k.startswith("data_") and isinstance(data.get(k, getlink=True), h5py.ExternalLink))
    planes, total, shape, dtype = [], 0, None, None
    for k in links:
        d = data[k]
        shape, dtype = d.shape, d.dtype
        for i in range(d.shape[0]):
            if len(planes) < max_planes:
                planes.append({"c": 0, "z": 0, "t": total + i, "xxh3": _h(d[i])})
        total += d.shape[0]
    det = f["/entry/instrument/detector"]

    def val(g, k):
        return float(np.asarray(g[k][()]).ravel()[0]) if k in g else None

    def units(g, k):
        u = g[k].attrs.get("units") if k in g else None
        return u.decode() if isinstance(u, bytes) else u

    geo = {}
    w = val(f["/entry/instrument/beam"], "incident_wavelength") if "/entry/instrument/beam" in f else None
    if w is not None:
        geo["wavelength_a"] = w
    d_m = val(det, "detector_distance")
    if d_m is not None:
        geo["distance_mm"] = d_m * (1.0 if units(det, "detector_distance") == "mm" else 1e3)
    bx, by = val(det, "beam_center_x"), val(det, "beam_center_y")
    if bx is not None and by is not None:
        geo["beam_center_px"] = [bx, by]
    xs, ys = val(det, "x_pixel_size"), val(det, "y_pixel_size")
    if xs and ys:
        geo["pixel_size_mm"] = [xs * 1e3, ys * 1e3]
    if val(det, "count_time") is not None:
        geo["exposure_s"] = val(det, "count_time")
    img = {"index": 0, "size_x": int(shape[2]), "size_y": int(shape[1]), "size_z": 1, "size_c": 1, "size_t": total,
           "pixel_type": _DT[np.dtype(dtype).name], "physical_size_um": {}, "planes": planes, "check_diffraction": geo}
    return {"reader": f"h5py {h5py.__version__} + hdf5plugin {hdf5plugin.version}", "kind": "nxmx",
            "data_links": links, "images": [img]}


# mdocfile column -> our column name (docs/formats/mdoc.md)
_MDOC_COLUMNS = {"TiltAngle": "tilt_angle_deg", "ExposureDose": "exposure_dose_e_per_a2", "ExposureTime": "exposure_time_s",
                 "Defocus": "defocus_um", "Magnification": "magnification", "PixelSpacing": "pixel_spacing_a"}


def mdoc(p: Path) -> dict:
    import mdocfile

    df = mdocfile.read(p)
    hashes = {}
    for col, ours in _MDOC_COLUMNS.items():
        if col in df.columns and df[col].notna().all():
            hashes[ours] = xxhash.xxh3_128_hexdigest(np.asarray(df[col], dtype="<f8").tobytes())
    return {"reader": f"mdocfile {getattr(mdocfile, '__version__', '?')}", "kind": "mdoc", "section_count": int(len(df)),
            "tables": [{"index": 0, "event_count": int(len(df)), "parameter_names": [], "column_hashes": hashes}]}
