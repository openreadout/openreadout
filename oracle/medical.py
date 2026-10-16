#!/usr/bin/env python
"""Ground truth for the medical-imaging readers (`openreadout-medical`).

Usage: uv run python medical.py --id ID PATH [--id ID PATH ...]   (writes ../corpus/oracle/<ID>.json)

Readers, all run as black boxes:
  DICOM files and folders -> pydicom (MIT) with pylibjpeg (MIT; its libjpeg plugin is GPL-3.0,
      run only), pylibjpeg-openjpeg (MIT), pylibjpeg-rle (MIT) and pyjpegls (MIT) to decode
  NIfTI-1/2 and Analyze -> nibabel (MIT)
  Bruker ParaVision 2dseq -> brukerapi (MIT)

Images follow the rules of docs/formats/{dicom,nifti,bruker-paravision}.md, implemented here
from the documents (not from the Rust code):
  - DICOM: frames grouped by series, frame size, sample layout and orientation; multi-frame
    files and files without a position are images of their own. Positioned frames are sorted
    by ImagePositionPatient . (row x column); repeated positions make T. SEG frames: segments
    are C. The modality LUT (rescale or LUT) is applied, per frame for enhanced images, and the
    values cast to the documented type (int16/uint16/int32 for whole-number rescales of the
    Bits Stored range, else float32). Palettes are applied with pydicom's apply_color_lut.
    Whole-slide VOLUME instances: level sizes, and the smallest level's mean when lossy.
  - NIfTI: planes (z, t, c) of the data array in file order, scl_slope/scl_inter applied as
    float32 computed in float64.
  - ParaVision: brukerapi's scaled frames (float64) as float32; frame groups FG_SLICE -> Z,
    FG_CYCLE/FG_MOVIE -> T, others -> C, first group fastest.
Plane hashes are xxh3-128 of the little-endian samples (rows of x), as `openreadout` computes.
"""
import json
import os
import sys
from pathlib import Path

import numpy as np
import xxhash

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ["ORACLE_OUT"]).resolve() if os.environ.get("ORACLE_OUT") else ROOT / "corpus" / "oracle"
MAX_PLANES = int(os.environ.get("ORACLE_MAX_PLANES", "64"))
NP = {"uint8": np.uint8, "int8": np.int8, "uint16": np.uint16, "int16": np.int16, "uint32": np.uint32,
      "int32": np.int32, "float": np.float32, "double": np.float64, "int64": np.int64, "uint64": np.uint64}
NAME = {np.dtype(v).name: k for k, v in NP.items()}


def h(a) -> str:
    a = np.ascontiguousarray(a)
    if a.dtype.byteorder == ">":
        a = a.byteswap().view(a.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(a.tobytes())


def um(mm):
    return None if mm is None else float(mm) * 1000.0


# ---------------------------------------------------------------------------------------- DICOM

WSI = "1.2.840.10008.5.1.4.1.1.77.1.6"
DICOMDIR = "1.2.840.10008.1.3.10"


def _dicom_files(p: Path):
    import pydicom
    if p.is_file():
        ds = pydicom.dcmread(p, stop_before_pixels=True, force=True)
        if ds.get("SOPClassUID") == WSI:
            sib = [q for q in sorted(p.parent.iterdir()) if q.is_file() and q != p and _is_dicom(q)]
            sib = [q for q in sib if pydicom.dcmread(q, stop_before_pixels=True).get("SeriesInstanceUID") == ds.SeriesInstanceUID]
            return [p] + sib
        if "DirectoryRecordSequence" in ds:
            return _dicom_files(p.parent)
        return [p]
    out = []
    for root, dirs, files in os.walk(p):
        dirs[:] = sorted(d for d in dirs if not d.startswith("."))
        for f in sorted(files):
            q = Path(root) / f
            if f.startswith(".") or f.upper() == "DICOMDIR":
                continue
            if _is_dicom(q):
                out.append(q)
    return out


def _is_dicom(q: Path) -> bool:
    with open(q, "rb") as f:
        head = f.read(132)
    return head[128:132] == b"DICM"


def _fallback_spacing(ds):
    """Row and column spacing in mm when Pixel Spacing is absent: Imager Pixel Spacing
    (projection radiography), else the first ultrasound region calibrated in cm both ways."""
    v = getattr(ds, "ImagerPixelSpacing", None)
    if v and len(v) >= 2 and float(v[0]) > 0 and float(v[1]) > 0:
        return [float(v[0]), float(v[1])]
    for r in getattr(ds, "SequenceOfUltrasoundRegions", None) or []:
        try:
            if int(r.PhysicalUnitsXDirection) == 3 and int(r.PhysicalUnitsYDirection) == 3:
                dx, dy = float(r.PhysicalDeltaX), float(r.PhysicalDeltaY)
                if dx > 0 and dy > 0:
                    return [dy * 10.0, dx * 10.0]
        except AttributeError:
            continue
    return None


def _fg(ds, f, seq, attr=None):
    """A functional-group item (per frame first, then shared)."""
    for top in ("PerFrameFunctionalGroupsSequence", "SharedFunctionalGroupsSequence"):
        if top not in ds:
            continue
        items = ds[top].value
        item = items[f] if top.startswith("PerFrame") and f < len(items) else (items[0] if top.startswith("Shared") and items else None)
        if item is not None and seq in item and item[seq].value:
            g = item[seq].value[0]
            if attr is None or attr in g:
                return g
    return None


def _modality(ds, f):
    g = _fg(ds, f, "PixelValueTransformationSequence", "RescaleSlope")
    src = g if g is not None else ds
    if g is None and "ModalityLUTSequence" in ds:
        return ("lut", ds.ModalityLUTSequence[0])
    s = float(getattr(src, "RescaleSlope", 1) or 1)
    i = float(getattr(src, "RescaleIntercept", 0) or 0)
    return ("lin", s, i) if (s, i) != (1.0, 0.0) else ("id",)


def _out_type(ds, mods):
    signed = int(getattr(ds, "PixelRepresentation", 0)) == 1
    ba = int(getattr(ds, "BitsAllocated", 16))
    bs = int(getattr(ds, "BitsStored", ba))
    stored = {(8, False): "uint8", (8, True): "int8", (1, False): "uint8", (16, False): "uint16", (16, True): "int16",
              (32, False): "uint32", (32, True): "int32"}.get((ba, signed), "uint16")
    if "FloatPixelData" in ds:
        return "float"
    if "DoubleFloatPixelData" in ds:
        return "double"
    if int(getattr(ds, "SamplesPerPixel", 1)) != 1:
        return stored
    if all(m[0] == "id" for m in mods):
        return stored
    if any(m[0] == "lut" for m in mods):
        bits = max(int(m[1].LUTDescriptor[2]) for m in mods if m[0] == "lut")
        return "uint8" if bits <= 8 and stored == "uint8" else "uint16"
    lo, hi = ((-(2 ** (bs - 1)), 2 ** (bs - 1) - 1) if signed else (0, 2 ** bs - 1))
    mn, mx = np.inf, -np.inf
    for m in mods:
        if m[0] == "id":
            mn, mx = min(mn, lo), max(mx, hi)
            continue
        _, s, i = m
        if s != int(s) or i != int(i):
            return "float"
        a, b = lo * s + i, hi * s + i
        mn, mx = min(mn, a, b), max(mx, a, b)
    if mn >= -32768 and mx <= 32767:
        return "int16"
    if mn >= 0 and mx <= 65535:
        return "uint16"
    if mn >= -2 ** 31 and mx <= 2 ** 31 - 1:
        return "int32"
    return "float"


def _apply(ds, frame, mod, out):
    from pydicom.pixels import apply_color_lut
    if ds.PhotometricInterpretation == "PALETTE COLOR":
        return apply_color_lut(frame, ds)
    if frame.ndim == 3:
        return frame
    if mod[0] == "id":
        v = frame.astype(np.float64)
    elif mod[0] == "lin":
        v = frame.astype(np.float64) * mod[1] + mod[2]
    else:
        lut = mod[1]
        first = int(lut.LUTDescriptor[1])
        table = np.frombuffer(lut.LUTData, "<u2") if isinstance(lut.LUTData, bytes) else np.asarray(lut.LUTData)
        idx = np.clip(frame.astype(np.int64) - first, 0, len(table) - 1)
        v = table[idx].astype(np.float64)
    t = NP[out]
    if np.issubdtype(t, np.integer):
        info = np.iinfo(t)
        v = np.clip(v, info.min, info.max)
    return v.astype(t)


def _frames(ds):
    if str(ds.file_meta.get("TransferSyntaxUID", "")) == "1.2.840.10008.1.2.8.1":
        # Deflated Image Frame Compression: pydicom has no decoder; each frame is raw DEFLATE
        # of the native frame (1-bit samples packed least significant bit first)
        import zlib
        from pydicom.encaps import generate_frames
        n = int(getattr(ds, "NumberOfFrames", 1) or 1)
        r, c = int(ds.Rows), int(ds.Columns)
        out = []
        for fr in generate_frames(ds.PixelData, number_of_frames=n):
            raw = zlib.decompress(fr, -15)
            if int(ds.BitsAllocated) == 1:
                out.append(np.unpackbits(np.frombuffer(raw, np.uint8), bitorder="little")[: r * c].reshape(r, c))
            else:
                out.append(np.frombuffer(raw, f"<u{int(ds.BitsAllocated) // 8}")[: r * c].reshape(r, c))
        return np.stack(out)
    a = ds.pixel_array
    n = int(getattr(ds, "NumberOfFrames", 1) or 1)
    if n == 1:
        a = a[None]
    return a


def dicom(p: Path) -> dict:
    import pydicom
    files = _dicom_files(p)
    heads = []
    for q in files:
        ds = pydicom.dcmread(q, stop_before_pixels=True, force=True)
        if ds.get("SOPClassUID") == DICOMDIR or not any(k in ds for k in ("Rows",)):
            continue
        heads.append((q, ds))
    images = []
    wsi = [(q, ds) for q, ds in heads if ds.get("SOPClassUID") == WSI]
    rest = [(q, ds) for q, ds in heads if not (ds.get("SOPClassUID") == WSI and len(ds.ImageType) > 2 and ds.ImageType[2] == "VOLUME")]
    vol = [(q, ds) for q, ds in wsi if len(ds.ImageType) > 2 and ds.ImageType[2] == "VOLUME"]
    if vol:
        levels = {}
        for q, ds in vol:
            levels.setdefault((int(ds.TotalPixelMatrixColumns), int(ds.TotalPixelMatrixRows)), []).append((q, ds))
        order = sorted(levels, key=lambda wh: -wh[0] * wh[1])
        w0, h0 = order[0]
        sp = _fg(vol[0][1], 0, "PixelMeasuresSequence", "PixelSpacing")
        img = {"index": 0, "size_x": w0, "size_y": h0, "size_z": 1, "size_c": 1, "size_t": 1,
               "pixel_type": "uint8", "planes": [],
               "physical_size_um": {"x": um(sp.PixelSpacing[1]) if sp else None, "y": um(sp.PixelSpacing[0]) if sp else None}}
        img["levels"] = []
        for li, (w, hh) in enumerate(order[1:], start=1):
            lev = {"level": li, "size_x": w, "size_y": hh, "planes": []}
            if li == len(order) - 1:
                plane = _wsi_level(levels[(w, hh)], w, hh)
                lev["planes"] = [{"c": 0, "z": 0, "t": 0, "xxh3": h(plane), "mean": float(plane.mean())}]
            img["levels"].append(lev)
        images.append(img)
    recs = []
    for fi, (q, ds) in enumerate(rest):
        n = int(getattr(ds, "NumberOfFrames", 1) or 1)
        ori_top = getattr(ds, "ImageOrientationPatient", None)
        pos_top = getattr(ds, "ImagePositionPatient", None)
        offs = getattr(ds, "GridFrameOffsetVector", None)
        for f in range(n):
            g = _fg(ds, f, "PlanePositionSequence", "ImagePositionPatient")
            go = _fg(ds, f, "PlaneOrientationSequence", "ImageOrientationPatient")
            ori = [float(x) for x in (go.ImageOrientationPatient if go is not None else ori_top)] if (go is not None or ori_top is not None) else None
            pos = [float(x) for x in g.ImagePositionPatient] if g is not None else None
            if pos is None and pos_top is not None:
                if n > 1 and offs is not None and len(offs) == n and ori:
                    nv = np.cross(ori[:3], ori[3:])
                    pos = list(np.array(pos_top, float) + nv / np.linalg.norm(nv) * float(offs[f]))
                elif n == 1:
                    pos = [float(x) for x in pos_top]
            seg = None
            if "PerFrameFunctionalGroupsSequence" in ds:
                it = ds.PerFrameFunctionalGroupsSequence[f]
                if "SegmentIdentificationSequence" in it:
                    seg = int(it.SegmentIdentificationSequence[0].ReferencedSegmentNumber)
            content = None
            if "PerFrameFunctionalGroupsSequence" in ds and "FrameContentSequence" in ds.PerFrameFunctionalGroupsSequence[f]:
                content = ds.PerFrameFunctionalGroupsSequence[f].FrameContentSequence[0]
            temporal = float(content.TemporalPositionIndex) if content is not None and "TemporalPositionIndex" in content else float(getattr(ds, "TemporalPositionIdentifier", 0) or 0)
            recs.append({"fi": fi, "f": f, "pos": pos, "ori": ori, "n": n, "seg": seg, "temporal": temporal,
                         "trigger": float(getattr(ds, "TriggerTime", 0) or 0), "instance": float(getattr(ds, "InstanceNumber", 0) or 0),
                         "mod": _modality(ds, f)})
    groups, order = {}, []
    for r in recs:
        ds = rest[r["fi"]][1]
        positioned = all(x["pos"] is not None and x["ori"] is not None for x in recs if x["fi"] == r["fi"])
        own = r["fi"] if (r["n"] > 1 or not positioned) else None
        key = (str(ds.get("SeriesInstanceUID", "")), int(ds.Rows), int(ds.Columns), int(getattr(ds, "SamplesPerPixel", 1)),
               int(getattr(ds, "BitsAllocated", 16)), str(ds.PhotometricInterpretation),
               tuple(round(x * 1000) for x in r["ori"]) if r["ori"] else None, own)
        if key not in groups:
            order.append(key)
        groups.setdefault(key, []).append(r)
    cache = {}

    def frames_of(fi):
        if fi not in cache:
            cache.clear()
            cache[fi] = _frames(pydicom.dcmread(rest[fi][0], force=True))
        return cache[fi]

    for key in order:
        fr = groups[key]
        ds0 = rest[fr[0]["fi"]][1]
        n = len(fr)
        nv = np.cross(fr[0]["ori"][:3], fr[0]["ori"][3:]) if fr[0]["ori"] else None
        if nv is not None:
            nv = nv / np.linalg.norm(nv)
        sz, st, sc = n, 1, 1
        planes = None
        if any(r["seg"] is not None for r in fr):
            d = lambda r: float(np.dot(r["pos"], nv)) if r["pos"] is not None and nv is not None else float(r["f"])
            levels = sorted({round(d(r), 3) for r in fr})
            segs = sorted({r["seg"] for r in fr})
            sz, sc = len(levels), len(segs)
            planes = [[None] * sz for _ in range(sc)]
            for r in fr:
                c = segs.index(r["seg"])
                z = levels.index(round(d(r), 3))
                planes[c][z] = planes[c][z] or r
            ordered = [(c, z, 0, planes[c][z]) for c in range(sc) for z in range(sz)]
        elif n > 1 and nv is not None and all(r["pos"] is not None for r in fr):
            d = lambda r: float(np.dot(r["pos"], nv))
            tk = lambda r: (r["temporal"], r["trigger"], r["instance"], r["fi"], r["f"])
            fr.sort(key=lambda r: (d(r), tk(r)))
            lv = []
            for r in fr:
                if lv and abs(d(r) - lv[-1][0]) < 1e-3:
                    lv[-1][1] += 1
                else:
                    lv.append([d(r), 1])
            nz = len(lv)
            if nz != n and all(c == n // nz for _, c in lv) and n // nz > 1:
                sz, st = nz, n // nz
                fr = [fr[z * st + t] for t in range(st) for z in range(sz)]
            ordered = [(0, i % sz, i // sz, r) for i, r in enumerate(fr)]
        else:
            fr.sort(key=lambda r: (r["fi"], r["instance"], r["f"]))
            ptr = getattr(ds0, "FrameIncrementPointer", None)
            ptrs = [int(x) for x in (ptr if isinstance(ptr, (list, pydicom.multival.MultiValue)) else [ptr])] if ptr is not None else []
            temporal = any(x in (0x00181063, 0x00181065, 0x00181060) for x in ptrs) or "FrameTime" in ds0 or ds0.get("Modality") in ("US", "XA", "RF", "ES", "XC", "IVUS")
            spatial = any(x in (0x00540080, 0x3004000C) for x in ptrs)
            if n > 1 and temporal and not spatial:
                sz, st = 1, n
            ordered = [(0, i % sz, i // sz, r) for i, r in enumerate(fr)]
        mods = [r["mod"] for (_, _, _, r) in ordered if r is not None]
        out = _out_type(ds0, mods)
        first = next(r for (_, _, _, r) in ordered if r is not None)
        kindf = frames_of(first["fi"]).dtype
        if kindf.kind == "f":
            out = "double" if kindf.itemsize == 8 else "float"
        spp = int(getattr(ds0, "SamplesPerPixel", 1))
        pal = ds0.PhotometricInterpretation == "PALETTE COLOR"
        if pal:
            out = "uint8" if int(ds0.RedPaletteColorLookupTableDescriptor[2]) <= 8 else "uint16"
        sp = _fg(ds0, 0, "PixelMeasuresSequence", "PixelSpacing")
        spacing = sp.PixelSpacing if sp is not None else getattr(ds0, "PixelSpacing", None)
        spacing = spacing or _fallback_spacing(ds0)
        img = {"index": len(images), "size_x": int(ds0.Columns), "size_y": int(ds0.Rows), "size_z": sz, "size_c": sc, "size_t": st,
               "pixel_type": out, "physical_size_um": {"x": um(spacing[1]) if spacing else None, "y": um(spacing[0]) if spacing else None},
               "planes": []}
        if spp == 3 or pal:
            img["samples_per_pixel"] = 3
        for c, z, t, r in ordered:
            if len(img["planes"]) >= MAX_PLANES:
                break
            if r is None:
                plane = np.zeros((int(ds0.Rows), int(ds0.Columns)), NP[out])
            else:
                plane = _apply(rest[r["fi"]][1], frames_of(r["fi"])[r["f"]], r["mod"], out)
            img["planes"].append({"c": c, "z": z, "t": t, "xxh3": h(plane), "mean": float(np.asarray(plane, np.float64).mean())})
        images.append(img)
    import pydicom as _p
    return {"reader": f"pydicom {_p.__version__}", "images": images}


def _wsi_level(members, w, hh):
    import pydicom
    canvas = None
    for q, _ in members:
        ds = pydicom.dcmread(q)
        tw, th = int(ds.Columns), int(ds.Rows)
        frames = _frames(ds)
        if canvas is None:
            canvas = np.zeros((hh, w, 3), np.uint8)
        gx = -(-w // tw)
        org = str(getattr(ds, "DimensionOrganizationType", "TILED_FULL"))
        for f in range(frames.shape[0]):
            if org == "TILED_SPARSE" or ("PerFrameFunctionalGroupsSequence" in ds and org != "TILED_FULL"):
                pp = ds.PerFrameFunctionalGroupsSequence[f].PlanePositionSlideSequence[0]
                x0 = int(pp.ColumnPositionInTotalImagePixelMatrix) - 1
                y0 = int(pp.RowPositionInTotalImagePixelMatrix) - 1
            else:
                if f >= gx * -(-hh // th):
                    break
                x0, y0 = (f % gx) * tw, (f // gx) * th
            tile = frames[f]
            ww, hhh = min(tw, w - x0), min(th, hh - y0)
            if ww > 0 and hhh > 0:
                canvas[y0:y0 + hhh, x0:x0 + ww] = tile[:hhh, :ww]
    return canvas


# ---------------------------------------------------------------------------------------- NIfTI

def nifti(p: Path) -> dict:
    import nibabel as nib
    img = nib.load(str(p))
    hdr = img.header
    raw = np.asanyarray(img.dataobj.get_unscaled()) if hasattr(img.dataobj, "get_unscaled") else np.asanyarray(img.dataobj)
    slope = getattr(img.dataobj, "slope", 1.0)
    inter = getattr(img.dataobj, "inter", 0.0)
    rgb = raw.dtype.names is not None
    out = NAME.get(raw.dtype.newbyteorder("=").name, None) if not rgb else "uint8"
    if not rgb and (slope, inter) != (1.0, 0.0) and slope not in (0.0,) and np.isfinite(slope):
        out = "double" if raw.dtype == np.float64 else "float"
        data = (raw.astype(np.float64) * slope + inter).astype(NP[out])
    else:
        data = raw
    if rgb:
        data = np.stack([raw[n] for n in raw.dtype.names], axis=-1)
    shp = list(data.shape[:3] if not rgb else data.shape[:3]) + [1] * 3
    nd = data.ndim - (1 if rgb else 0)
    full = data.reshape(data.shape[:nd] + (1,) * (7 - nd) + ((data.shape[-1],) if rgb else ()), order="A")
    nx, ny, nz, nt = full.shape[0], full.shape[1], full.shape[2], full.shape[3]
    nc = int(np.prod(full.shape[4:7]))
    full = full.reshape((nx, ny, nz, nt, nc) + ((full.shape[-1],) if rgb else ()), order="F")
    zooms = hdr.get_zooms()
    units = hdr.get_xyzt_units()[0] if hasattr(hdr, "get_xyzt_units") else "mm"
    k = {"meter": 1e6, "mm": 1e3, "micron": 1.0}.get(units, 1e3)
    phys = {a: (float(abs(zooms[i])) * k if len(zooms) > i and zooms[i] else None) for i, a in enumerate("xyz")}
    if nz <= 1:
        phys["z"] = None
    planes = []
    for c in range(nc):
        for t in range(nt):
            for z in range(nz):
                if len(planes) >= MAX_PLANES:
                    break
                plane = full[:, :, z, t, c].swapaxes(0, 1)
                planes.append({"c": c, "z": z, "t": t, "xxh3": h(plane)})
    _ = shp
    return {"reader": f"nibabel {nib.__version__}", "images": [{
        "index": 0, "size_x": nx, "size_y": ny, "size_z": nz, "size_c": nc, "size_t": nt,
        "pixel_type": out, "physical_size_um": phys, "planes": planes}]}


# ------------------------------------------------------------------------------------ ParaVision

def _recos(p: Path):
    if p.is_file() and p.name == "2dseq":
        return [p]
    if (p / "2dseq").is_file():
        return [p / "2dseq"]
    if (p / "pdata").is_dir():
        return sorted((p / "pdata").glob("*/2dseq"), key=lambda q: int(q.parent.name) if q.parent.name.isdigit() else 1 << 30)
    out = []
    for e in sorted((d for d in p.iterdir() if d.is_dir() and d.name.isdigit()), key=lambda d: int(d.name)):
        if (e / "acqp").exists() or (e / "method").exists():
            out += _recos(e)
    return out


def paravision(p: Path) -> dict:
    import brukerapi
    from brukerapi.dataset import Dataset
    images = []
    for seq in _recos(p):
        ds = Dataset(str(seq))
        visu = ds.parameters["visu_pars"] if hasattr(ds, "parameters") else None
        core = list(ds.shape_block)
        if len(core) == 1:
            continue  # spectroscopy: a trace
        d = np.asarray(ds.data)
        frames = d.reshape(tuple(core) + (-1,), order="F")
        raw_fg = ds._parameter_value("VisuFGOrderDesc", []) if hasattr(ds, "_parameter_value") else []
        if raw_fg is None:
            raw_fg = []
        if len(raw_fg) and not isinstance(raw_fg[0], (list, tuple)):
            raw_fg = [raw_fg]
        fg = [(int(x[0]), str(x[1]).strip("<>")) for x in raw_fg]
        nframes = frames.shape[-1]
        if np.iscomplexobj(frames) and any(n == "FG_COMPLEX" for _, n in fg):
            # brukerapi folds the real and imaginary frame group into complex values. Unfold
            # it: the other groups keep their order, and FG_COMPLEX index 0 is real, 1 imaginary.
            k = next(i for i, (_, n) in enumerate(fg) if n == "FG_COMPLEX")
            rest = [c for i, (c, _) in enumerate(fg) if i != k]
            split = frames.reshape(tuple(core) + tuple(rest), order="F")
            both = np.stack([split.real, split.imag], axis=len(core) + k)
            frames = both.reshape(tuple(core) + (-1,), order="F")
            nframes = frames.shape[-1]
        if not fg or int(np.prod([c for c, _ in fg])) != nframes:
            fg = [(nframes, "frames")]
        axes = {"z": [], "t": [], "c": []}
        for i, (c, name) in enumerate(fg):
            if name == "FG_SLICE":
                axes["z" if len(core) < 3 else "c"].append(i)
            elif name in ("FG_CYCLE", "FG_MOVIE") or name == "frames":
                axes["t"].append(i)
            else:
                axes["c"].append(i)
        prod = lambda ix: int(np.prod([fg[i][0] for i in ix])) if ix else 1
        nz = int(core[2]) if len(core) == 3 else prod(axes["z"])
        nt, nc = prod(axes["t"]), prod(axes["c"])
        stride = [1] * len(fg)
        for i in range(1, len(fg)):
            stride[i] = stride[i - 1] * fg[i - 1][0]

        def frame_of(z, t, c):
            f = 0
            for a, v in (("z", z if len(core) < 3 else 0), ("t", t), ("c", c)):
                for g in axes[a]:
                    f += (v % fg[g][0]) * stride[g]
                    v //= fg[g][0]
            return f

        planes = []
        for c in range(nc):
            for t in range(nt):
                for z in range(nz):
                    if len(planes) >= MAX_PLANES:
                        break
                    fr = frames[..., frame_of(z, t, c)]
                    plane = fr[:, :, z] if len(core) == 3 else fr
                    planes.append({"c": c, "z": z, "t": t, "xxh3": h(np.asarray(plane, np.float32).T)})
        ext = np.atleast_1d(ds._parameter_value("VisuCoreExtent", None))
        desc = ds._parameter_value("VisuCoreDimDesc", None)
        desc = [] if desc is None else [desc] if isinstance(desc, str) else list(np.atleast_1d(desc))
        spatial = [i >= len(desc) or str(desc[i]).strip("<>") == "spatial" for i in range(len(core))]
        phys = {"x": float(ext[0]) / core[0] * 1000 if ext is not None and spatial[0] else None,
                "y": float(ext[1]) / core[1] * 1000 if ext is not None and len(core) > 1 and spatial[1] else None}
        images.append({"index": len(images), "size_x": int(core[0]), "size_y": int(core[1]), "size_z": nz, "size_c": nc, "size_t": nt,
                       "pixel_type": "float", "physical_size_um": phys, "planes": planes})
        _ = visu
    return {"reader": f"brukerapi {brukerapi.__version__ if hasattr(brukerapi, '__version__') else ''}".strip(), "images": images}


# ---------------------------------------------------------------------------------------- main

def kind(p: Path):
    n = p.name.lower()
    if n.endswith((".nii", ".nii.gz", ".hdr", ".hdr.gz", ".img", ".img.gz")):
        return "nifti"
    if p.name == "2dseq" or (p.is_dir() and (any(p.glob("pdata/*/2dseq")) or any(p.glob("*/pdata/*/2dseq")) or (p / "2dseq").exists())):
        return "paravision"
    if p.is_file() and (_is_dicom(p) or n.endswith(".dcm") or n == "dicomdir"):
        return "dicom"
    if p.is_dir():
        if (p / "DICOMDIR").exists():
            return "dicom"
        files = [q for q in sorted(p.rglob("*"))[:200] if q.is_file() and not q.name.startswith(".")][:24]
        if files and sum(_is_dicom(q) for q in files) * 2 >= len(files):
            return "dicom"
    return None


def oracle(p: Path) -> dict:
    """The ground truth of a medical-imaging file or folder (`kind` decides the reader)."""
    return {"dicom": dicom, "nifti": nifti, "paravision": paravision}[kind(p)](p)


def main():
    args = sys.argv[1:]
    OUT.mkdir(parents=True, exist_ok=True)
    while args:
        a = args.pop(0)
        fid = None
        if a == "--id":
            fid, a = args.pop(0), args.pop(0)
        p = Path(a)
        fid = fid or p.name.split(".")[0]
        k = kind(p)
        try:
            data = {"id": fid, "file": p.name, "max_planes_hashed": MAX_PLANES, **oracle(p)}
        except Exception as e:  # record failures too: they are information
            data = {"id": fid, "file": p.name, "error": f"{type(e).__name__}: {e}", "images": []}
        oracle_json.write_text(OUT / f"{fid}.json", json.dumps(data, indent=1, default=str))
        print(fid, k, data.get("error") or [(i["size_x"], i["size_y"], i["size_z"], i["size_t"], i.get("pixel_type")) for i in data["images"]])


if __name__ == "__main__":
    main()
