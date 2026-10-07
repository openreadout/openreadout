#!/usr/bin/env python
"""Generate ground-truth JSON for a corpus file using third-party readers.

Usage: uv run python gen.py [--id ID] <file-or-dir> [...]   (writes ../corpus/oracle/<id>.json)
       (--id names the output for the path that follows it; needed for bundle members such as
        Bruker experiment directories `nmrxiv-s275/1`)

Readers used (all run as black boxes):
  .lif .lifext .lof .xlif .xlef .xlcf -> liffile (BSD-3); bioio-lif (GPL) is run separately, by hand, as a second opinion
  .czi  -> czifile (BSD-3)      + pylibCZIrw (LGPL, second opinion) when installed
  .nd2  -> nd2 (BSD-3)          + bioio-nd2 (BSD-3, second opinion)
  .fcs/.lmd -> flowio (BSD-3)   + fcsparser (MIT, second opinion on data set 0)
  .mrc .mrcs .map .rec .st .ali -> mrcfile (BSD-3)
  .dm3 .dm4 -> pyDM3reader dm3_lib (MIT) tag dictionary + NumPy
  .ser      -> ncempy.io.ser (GPL-3.0-or-later, black box)
  .emd      -> h5py (BSD-3) on the Velox layout
  .oir  -> oirfile (BSD-3), continuation files included
  OME-Zarr (a .zarr directory or a zip store) -> zarr-python (MIT); NGFF metadata read in gen.py
  .ims  -> h5py (BSD-3) + hdf5plugin (LZ4); Bio-Formats showinf geometry recorded as a second opinion
  .nwb  -> h5py (BSD-3): acquisition/ TimeSeries as traces
  .zvi  -> Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box)
  .oib .oif -> Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box), planes cross-checked with oiffile (BSD-3)
  .vsi  -> Bio-Formats 8.5.0 bfconvert (GPL, black box: run) from oracle/bftools
  directory (Bruker TopSpin experiment) -> nmrglue (BSD-3) bruker.read / read_pdata (all
        processed components, 2D and 3D) / read_nuslist
  directory with fid (+ procpar), no acqus (Varian/Agilent VnmrJ) -> nmrglue (BSD-3)
        varian.read(as_2d=True) (read_fid when there is no procpar)
  .jdf  -> nmrglue (BSD-3) jeol.read, parse_jeol (conjugate undone: values as stored)
  .jdx/.dx/.jcamp/.jcm -> nmrglue (BSD-3) jcampdx + jcamp (MIT, second opinion)
  .dx that is a zip (Agilent OpenLab CDS) -> oracle/openlab_cds.py: rainbow-api (LGPL, black box)
        on the .CH parts + chromConverter (GPL, R, black box) on the whole container
  .abf  -> pyabf (MIT)
  Bruker OPUS, Thermo OMNIC .spa/.spg, Renishaw .wdf, PerkinElmer .sp (recognised by their first bytes)
        -> oracle/spectro.py: brukeropus (MIT), SpectroChemPy (CeCILL-B), renishawWiRE (MIT), specio (BSD-3)
  .txt .csv .xlsx .xls (plate-reader exports) -> allotropy (MIT) ASM JSON summarized per detection
        mode (oracle/plate.py); Tecan i-control exports -> an independent reader in plate.py
  TIFF family (.tif .tiff .btf .tf2 .tf8 .svs .ndpi .lsm .qptiff .stk, *.companion.ome)
        -> tifffile (BSD-3); see tiff() for the series/axes mapping
  .nd   (MetaMorph series) -> Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box), planes
        cross-checked with tifffile on the member STK/TIFF files
  .dcimg -> Bio-Formats 8.5.0 bfconvert (GPL, black box; rows flipped back to stored order),
        dcimg (MIT) second opinion and frame counters / time stamps
  .raw  -> (Thermo) no reader of the .raw is run: the depositor's own mzML/mzXML (or ANDI-MS) conversion with
           the same stem is read with pyteomics (Apache-2.0) and every scan is recorded
  .mzML .mzXML -> pyteomics (Apache-2.0): every spectrum/scan and every chromatogram
  Agilent MassHunter .d (a directory with AcqData/MSScan.bin) -> no reader of the .d is run: the
           depositor's own mzML (`--export FILE` before the path) is read with pyteomics, spectra
           addressed by position and matched by native id (`scanId=N`)
  Waters .raw with `--export FILE` -> no reader of the .raw is run: the depositor's mzML, spectra
           by scan number (one function) or by position (several functions merged by time)
  Sciex .wiff (`--export FILE` required) -> no reader of the .wiff is run: the depositor's mzML;
           spectra by position and native id, SRM chromatograms (placeholder CE 0.0 dropped)
Per-plane hashes are xxh3-128 of the raw little-endian sample bytes in (c, z, t) order,
which is exactly what `openreadout` computes, so any disagreement is a real bug.

NMR traces (Bruker TopSpin directories, JCAMP-DX): `traces[]`, one per trace in openreadout's
order. `xxh3` is xxh3-128 over every value of the trace as little-endian float64, sweep by sweep,
sample by sample, channels interleaved (real, imag, real, imag, ... for complex data; x, y, x, y
for peak tables). Bruker time-domain values are nmrglue's raw fid/ser values times 2**NC (the
scaling openreadout applies), each row cut to TD values; processed values are nmrglue's
read_pdata(scale_data=True). JCAMP-DX values are the decoded table values times the factor.

FCS tables: one entry per data set (following $NEXTDATA). `xxh3` is xxh3-128 over the raw event
matrix (uncompensated, unscaled; $DATATYPE I values bit-masked per $PnR) converted to float64 and
laid out column-major: all events of parameter 1, then all events of parameter 2, ..., each value
as 8 little-endian IEEE-754 bytes. `openreadout` hashes `Table.columns` the same way.
"""
import json, os, sys, hashlib, re, zipfile
from pathlib import Path
import numpy as np, xxhash

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB)

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ["ORACLE_OUT"]).resolve() if os.environ.get("ORACLE_OUT") else ROOT / "corpus" / "oracle"  # ORACLE_OUT: e.g. test-fixture oracles

def h(a: np.ndarray) -> str:
    a = np.ascontiguousarray(a)
    if a.dtype.byteorder == ">":
        a = a.byteswap().view(a.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(a.tobytes())

def dtype_name(dt) -> str:
    return {"uint8":"uint8","uint16":"uint16","uint32":"uint32","int8":"int8","int16":"int16","int32":"int32","float32":"float","float64":"double",
            "int64":"int64","uint64":"uint64","complex64":"complex","complex128":"double-complex"}[np.dtype(dt).name]

def sha256(p: Path) -> str:
    s = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            s.update(chunk)
    return s.hexdigest()

MAX_PLANES = int(os.environ.get("ORACLE_MAX_PLANES", "64"))  # hash at most this many planes per image (first N in c,z,t order)

def lif(p: Path) -> dict:
    """One oracle image per image openreadout exposes (docs/formats/lif.md), read with liffile:

    - tile scans ('M') are stitched with OUR placement rule (liffile does not stitch): stage
      positions from liffile's TileScanInfo, flip then swap, minus the minimum, divided by the X/Y
      pixel step, rounded half up, later tiles pasting over earlier ones (see `_lif_layout`). Each
      tile's offset and the hash of its (c0, z0, t0) plane are recorded too, so the harness can
      check placement independently of the stitched hash;
    - lambda ('λ') is folded into C, channel-major (c = channel * n_lambda + lambda);
    - XT slices ('N') and T slices ('Q') are folded into T (t innermost, then N, then Q);
    - rotation ('A') and any other axis become separate images (first XML axis innermost);
    - FLIM/TCSPC images are listed with their geometry and no planes (not decoded);
    - float16 samples are hashed after widening to float32 (what openreadout returns).
    """
    import liffile
    images = []
    parent_scans = _lifext_parent_tilescans(p)
    with liffile.LifFile(p) as f:
        version = getattr(f, "version", None)
        for im in _lif_images(f):
            try:
                images.extend(_lif_image(im, len(images), parent_scans))
            except Exception as e:  # one bad image must not hide the others
                images.append({"index": len(images), "name": im.name, "path": im.path, "skip": f"{type(e).__name__}: {e}"})
    return {"reader": f"liffile {liffile.__version__}", "format_version": version, "images": images}


def _lif_images(f):
    """liffile's image list, except for LIFEXT: liffile follows only the first ChildrenOf group,
    so walk every group with liffile's own element iterator and image classes."""
    import liffile
    if f.type != liffile.LifFileType.LIFEXT:
        return list(f.images)
    out = []
    for group in f.xml_element.findall("./ChildrenOf"):
        for path, el in liffile.LifImageSeries._image_iter(group, group.get("MemoryBlockID", "")):
            cls = liffile.LifImage if el.find("./Data/Image") is not None else liffile.LifFlimImage
            out.append(cls(f, el, path))
    return out


def _lifext_parent_tilescans(p: Path) -> dict:
    """For a .lifext: memory block id -> TileScanInfo of the parent image in <stem>.lif."""
    if p.suffix.lower() != ".lifext":
        return {}
    import liffile
    parent = p.with_suffix(".lif")
    if not parent.exists():
        return {}
    out = {}
    with liffile.LifFile(parent) as f:
        for im in f.images:
            try:
                out[im.memory_block.id] = im.tilescan
            except Exception:
                pass
    return out


_FOLD = {"N": 1, "Q": 2}  # T is 0 (innermost)


def _lif_image(im, index0, parent_scans):
    import liffile
    if isinstance(im, liffile.LifFlimImage):
        s = im.sizes
        return [{"index": index0, "name": im.name, "path": im.path, "flim": True,
                 "size_x": s.get("X", 1), "size_y": s.get("Y", 1), "size_z": s.get("Z", 1), "size_c": s.get("C", 1), "size_t": s.get("T", 1),
                 "dims": "".join(im.dims), "shape": list(im.shape), "pixel_type": "uint16",
                 "physical_size_um": {"x": _lif_um(im, "X"), "y": _lif_um(im, "Y"), "z": _lif_um(im, "Z")}, "planes": []}]
    order = list(im.dims)
    sizes = dict(zip(order, im.shape))
    # XML order of dimension labels (liffile orders array axes by stride instead)
    from liffile.liffile import DIMENSION_ID  # not re-exported at package level
    xml_labels = [DIMENSION_ID.get(int(d.attrib["DimID"]), "?")
                  for d in im.xml_element.findall("./Data/Image/ImageDescription/Dimensions/DimensionDescription")]
    # RGB images (a trailing 'S' of 3 samples, no C axis): openreadout exposes the samples as
    # channels in storage order (by BytesInc), liffile returns them R, G, B (it reverses LIF
    # memory blocks tagged blue, green, red; XLIF image-file frames are RGB already). Our
    # channel with ChannelTag t (1 red, 2 green, 3 blue) is liffile's sample t - 1.
    s_index = None
    if sizes.get("S") == 3 and "C" not in sizes:
        chans = im.xml_element.findall("./Data/Image/ImageDescription/Channels/ChannelDescription")
        tags = [int(c.get("ChannelTag", "0")) for c in sorted(chans, key=lambda c: int(c.get("BytesInc", "0")))]
        if sorted(tags) == [1, 2, 3]:
            s_index = [t - 1 for t in tags]
    unhashable = [d for d in order if (d == "S" and s_index is None) or (d.startswith("C") and d != "C")]
    fold = sorted([d for d in order if d in _FOLD], key=lambda d: _FOLD[d])
    rgb = {"S"} if s_index else set()
    split = [d for d in xml_labels if d in order and d not in "XYZCTMλ" and d not in _FOLD and d not in unhashable and d not in rgb]
    split += [d for d in order if d not in split and d not in "XYZCTMλ" and d not in _FOLD and d not in unhashable and d not in rgb]
    L = sizes.get("λ", 1)
    C = sizes.get("C", 1) * L * (3 if s_index else 1)
    Z = sizes.get("Z", 1)
    T = sizes.get("T", 1)
    for d in fold:
        T *= sizes[d]
    n_split = 1
    for d in split:
        n_split *= sizes[d]
    tiles = sizes.get("M", 1)
    tw, th = sizes.get("X", 1), sizes.get("Y", 1)
    dtype = np.dtype(im.dtype)
    out_dtype = np.float32 if dtype == np.float16 else dtype
    scan = im.tilescan
    if scan is None and tiles > 1 and parent_scans:
        scan = parent_scans.get(im.path.split("/", 1)[0])
    lay = _lif_layout(tiles, tw, th, _lif_step(im, "X"), _lif_step(im, "Y"), scan) if tiles > 1 else None
    W, H = (lay["width"], lay["height"]) if lay else (tw, th)
    base = {"name": im.name, "path": im.path, "size_x": W, "size_y": H, "size_z": Z, "size_c": C, "size_t": T,
            "dims": "".join(order), "shape": list(im.shape), "pixel_type": dtype_name(out_dtype),
            "physical_size_um": {"x": _lif_um(im, "X"), "y": _lif_um(im, "Y"), "z": _lif_um(im, "Z")},
            "lambda": L if L > 1 else None, "folded_into_t": fold or None, "split_axes": split or None}
    arr = None if unhashable else im.asarray(out="memmap")

    def sel_for(c, z, t, s, m):
        idx = {"C": c // L, "λ": c % L, "Z": z, "M": m}
        if s_index:
            idx = {"S": s_index[c], "Z": z, "M": m}
        rest = t
        for d in ["T"] + fold:
            n = sizes.get(d, 1)
            idx[d] = rest % n
            rest //= n
        rest = s
        for d in split:
            idx[d] = rest % sizes[d]
            rest //= sizes[d]
        return tuple(idx.get(d, 0) if d not in "YX" else slice(None) for d in order)

    def tile_plane(c, z, t, s, m):
        a = np.asarray(arr[sel_for(c, z, t, s, m)]).reshape(th, tw)  # liffile squeezes a size-1 Y
        return a.astype(np.float32) if dtype == np.float16 else a

    out = []
    for s in range(n_split):
        entry = {"index": index0 + s, **base, "split": s if split else None, "planes": []}
        if unhashable:
            entry["unhashed_reason"] = f"axes {unhashable} (interleaved samples) are not compared"
        if lay:
            recs = []
            for i, (x, y) in enumerate(lay["offsets"]):
                rec = {"index": i, "x_px": x, "y_px": y, "fully_visible": not _lif_overlapped(lay, i)}
                vis = _lif_visible_rect(lay, i)
                if arr is not None:
                    tp = tile_plane(0, 0, 0, s, i)
                    rec["xxh3_c0z0t0"] = h(tp)
                    if vis:
                        vx, vy, vw, vh = vis
                        rec["visible"] = [vx, vy, vw, vh]
                        rec["xxh3_visible"] = h(tp[vy:vy + vh, vx:vx + vw])
                recs.append(rec)
            entry["mosaic"] = {"tile_count": tiles, "tile_width": tw, "tile_height": th, "method": lay["method"], "tiles": recs}
        if arr is not None:
            n = 0
            for c in range(C):
                for z in range(Z):
                    for t in range(T):
                        if n >= MAX_PLANES: break
                        if lay:
                            canvas = np.zeros((H, W), dtype=out_dtype)
                            for i, (x, y) in enumerate(lay["offsets"]):
                                canvas[y:y + th, x:x + tw] = tile_plane(c, z, t, s, i)[: H - y, : W - x]
                            plane = canvas
                        else:
                            plane = tile_plane(c, z, t, s, 0)
                        entry["planes"].append({"c": c, "z": z, "t": t, "xxh3": h(plane)}); n += 1
        out.append(entry)
    return out


def _lif_step(im, ax):
    """Pixel step in metres (Length / (N - 1)) when the unit is metres, else None."""
    for d in im._dimensions:
        if d.label == ax and d.unit.lower() == "m" and d.number_elements > 1:
            s = abs(d.length / (d.number_elements - 1))
            if s > 0 and np.isfinite(s):
                return s
    return None


_MAX_CANVAS_PIXELS = 1 << 34


def _lif_from_offsets(tw, th, method, offs):
    W = max(o[0] for o in offs) + tw
    H = max(o[1] for o in offs) + th
    if W >= 2**32 or H >= 2**32 or W * H > _MAX_CANVAS_PIXELS:
        return None
    if W * H > max(len(offs) * tw * th * 64, 1):
        return None
    return {"width": W, "height": H, "tile_width": tw, "tile_height": th, "method": method, "offsets": offs}


def _lif_layout(n, tw, th, step_x, step_y, scan):
    """Mirror of crates/openreadout-lif/src/mosaic.rs::layout."""
    if scan is not None and len(scan.tiles) == n:
        t = scan.tiles
        fx = [int(v) for v in t["field_x"]]; fy = [int(v) for v in t["field_y"]]
        px = [float(v) for v in t["pos_x"]]; py = [float(v) for v in t["pos_y"]]
        if step_x and step_y:
            pts = []
            for x, y in zip(px, py):
                sx = -x if scan.flip_x else x
                sy = -y if scan.flip_y else y
                pts.append((sy, sx) if scan.swap_xy else (sx, sy))
            if all(np.isfinite(a) and np.isfinite(b) for a, b in pts):
                mx = min(a for a, _ in pts); my = min(b for _, b in pts)
                offs = []
                for a, b in pts:
                    ox = np.floor((a - mx) / step_x + 0.5); oy = np.floor((b - my) / step_y + 0.5)
                    if not (0 <= ox < 4e9 and 0 <= oy < 4e9):
                        offs = None; break
                    offs.append((int(ox), int(oy)))
                if offs is not None:
                    distinct = any(a != fx[0] or b != fy[0] for a, b in zip(fx, fy))
                    if not (distinct and all(o == offs[0] for o in offs)):
                        lay = _lif_from_offsets(tw, th, "stage_position", offs)
                        if lay: return lay
        gx = [(max(fx) - v) if scan.flip_x else (v - min(fx)) for v in fx]
        gy = [(max(fy) - v) if scan.flip_y else (v - min(fy)) for v in fy]
        if scan.swap_xy:
            gx, gy = gy, gx
        lay = _lif_from_offsets(tw, th, "field_grid", [(a * tw, b * th) for a, b in zip(gx, gy)])
        if lay: return lay
    lay = _lif_from_offsets(tw, th, "row", [(i * tw, 0) for i in range(max(n, 1))])
    return lay or {"width": tw, "height": th, "tile_width": tw, "tile_height": th, "method": "row", "offsets": [(0, 0)]}


def _lif_visible_rect(lay, i):
    """A rectangle of tile i (tile coordinates: x, y, w, h) that no later tile covers, so the
    harness can find those pixels unchanged in the stitched plane. Each overlapping later tile
    cuts the rectangle down to the largest of the four remaining side strips. None if empty."""
    offs, tw, th = lay["offsets"], lay["tile_width"], lay["tile_height"]
    ax, ay = offs[i]
    x0, y0, x1, y1 = 0, 0, tw, th
    for bx, by in offs[i + 1:]:
        ox0, oy0 = max(x0, bx - ax), max(y0, by - ay)
        ox1, oy1 = min(x1, bx - ax + tw), min(y1, by - ay + th)
        if ox0 >= ox1 or oy0 >= oy1:
            continue
        cands = [(x0, y0, ox0, y1), (ox1, y0, x1, y1), (x0, y0, x1, oy0), (x0, oy1, x1, y1)]
        x0, y0, x1, y1 = max(cands, key=lambda r: max(r[2] - r[0], 0) * max(r[3] - r[1], 0))
        if x0 >= x1 or y0 >= y1:
            return None
    return (x0, y0, x1 - x0, y1 - y0)


def _lif_overlapped(lay, i):
    """Some pixel of tile i is covered by a later tile (mirror of MosaicLayout::overlapped_by_later)."""
    offs, tw, th = lay["offsets"], lay["tile_width"], lay["tile_height"]
    ax, ay = offs[i]
    return any(ax < bx + tw and bx < ax + tw and ay < by + th and by < ay + th for bx, by in offs[i + 1:])


def _lif_um(im, ax):
    try:
        c = im.coords.get(ax)
        if c is None or len(c) < 2: return None
        step = abs(float(c[1] - c[0]))
        # liffile coords are in metres. A step of 0, or of 1 cm or more per pixel, is not a
        # calibration (LAS X writes pixel indices labelled "m" for FLIM result images): None.
        return step * 1e6 if 0 < step < 1e-2 else None
    except Exception:
        return None

def czi(p: Path) -> dict:
    """One oracle image per CZI scene (czifile >= 2026 API: f.scenes / f.asxarray(scene=i)).
    Plane hashes: for each (c, z, t) the full-resolution scene plane, mosaics stitched by czifile.
    RGB pixel types produce a trailing 'S' sample axis; we hash the interleaved (Y, X, S) block."""
    import czifile
    images = []
    with czifile.CziFile(p) as f:
        meta = f.metadata() if callable(getattr(f, "metadata", None)) else getattr(f, "metadata", None)
        rendered = _czi_rendered(f, meta)
        if rendered is not None:
            return rendered
        comps = sorted({int(e.compression) for e in f.subblock_directory})
        # JPEG subblocks (compression 1): decoders may differ by one count (IDCT rounding); record
        # plane means for files marked `lossy` in the manifest, as for TIFF.
        lossy = 1 in comps
        n_scenes = len(f.scenes)
        for s in range(n_scenes):
            xa = f.asxarray(scene=s)
            dims = list(xa.dims); shape = dict(zip(dims, xa.shape))
            C = shape.get("C", 1); Z = shape.get("Z", 1); T = shape.get("T", 1)
            spp = shape.get("S", 1)  # czifile uses 'S' here for RGB samples (not scene)
            other = {d: n for d, n in shape.items() if d not in "CZTYXS" and n > 1}
            arr = np.asarray(xa.values)
            sc = f.scenes[s]
            # Extra dimensions (H, I, R, V, B) that vary: one image per combination of their
            # indices, letters in alphabetical order, the last varying fastest (docs/formats/czi.md).
            import itertools
            axes = sorted(other)
            for combo in itertools.product(*(range(other[d]) for d in axes)):
                fixed = dict(zip(axes, combo))
                planes = []; n = 0
                for c in range(C):
                    for z in range(Z):
                        for t in range(T):
                            if n >= MAX_PLANES: break
                            sel = []
                            for d in dims:
                                sel.append({"C": c, "Z": z, "T": t, **fixed}.get(d, slice(None) if d in "YXS" else 0))
                            plane = arr[tuple(sel)]
                            # czifile returns R,G,B sample order for Bgr* pixel types (as does openreadout).
                            rec = {"c": c, "z": z, "t": t, "xxh3": h(plane)}
                            if lossy:
                                rec["mean"] = float(np.asarray(plane, dtype=np.float64).mean())
                            planes.append(rec); n += 1
                img = {"index": len(images), "name": getattr(sc, "name", None), "size_x": shape.get("X", 1), "size_y": shape.get("Y", 1), "size_z": Z, "size_c": C, "size_t": T,
                       "samples_per_pixel": spp, "pixel_type": dtype_name(xa.dtype), "dims": "".join(dims), "shape": list(xa.shape), "other_dims": other,
                       "pyramid_levels": 1 + len(_czi_level_groups(sc)),
                       "physical_size_um": _czi_scaling(meta), "planes": planes}
                if fixed:
                    img["dimension_index"] = fixed
                    img["levels"] = []  # pyramid levels of a split scene are not composed by czi_levels
                else:
                    img["levels"] = _czi_levels(f, sc)
                images.append(img)
        return {"reader": f"czifile {czifile.__version__}", "images": images, "physical_size_um": _czi_scaling(meta),
                "subblock_count": len(f.subblock_directory), "compression_ids": comps}

def _czi_rendered(f, meta):
    """Super-resolved renderings (full-resolution subblocks stored larger than their logical
    extent, e.g. PALM): one oracle image per stored-to-logical ratio (ascending), each channel
    read by czifile at its stored size (`asarray(storedsize=True, C=c)`), pixel size = the
    logical one divided by the ratio. None when the file has no such subblocks (single scene only)."""
    import czifile
    level0 = [e for e in f.subblock_directory if int(e.pyramid_type) == 0]
    def ratio(e):
        d = dict(zip(e.dims, zip(e.shape, e.stored_shape)))
        s, ss = d.get("X", (1, 1))
        return round(ss / s, 6) if s else 1.0
    if not any(ratio(e) > 1.0 for e in level0) or len(f.scenes) > 1:
        return None
    groups = {}
    for e in level0:
        c = dict(zip(e.dims, e.start)).get("C", 0)
        groups.setdefault(ratio(e), set()).add(c)
    sc = _czi_scaling(meta)
    images = []
    for r in sorted(groups):
        chans = sorted(groups[r])
        planes, shape = [], None
        for k, c in enumerate(chans):
            a = np.squeeze(f.asarray(storedsize=True, C=c))
            shape = a.shape
            planes.append({"c": k, "z": 0, "t": 0, "xxh3": h(a)})
        exact = next(dict(zip(e.dims, e.stored_shape))["X"] / dict(zip(e.dims, e.shape))["X"] for e in level0 if ratio(e) == r)
        phys = {ax: (v / exact if ax in "xy" and v else v) for ax, v in (sc or {}).items()}
        images.append({"index": len(images), "size_x": int(shape[-1]), "size_y": int(shape[-2]), "size_z": 1,
                       "size_c": len(chans), "size_t": 1, "samples_per_pixel": 1, "pixel_type": dtype_name(np.dtype(level0[0].dtype)),
                       "physical_size_um": phys, "rendering_scale": r, "planes": planes})
    return {"reader": f"czifile {czifile.__version__} (asarray storedsize=True per channel)", "images": images,
            "physical_size_um": sc}

MAX_LEVEL_PIXELS = int(os.environ.get("ORACLE_MAX_LEVEL_PIXELS", str(200_000_000)))  # skip pyramid levels larger than this (Y*X)

def _czi_level_groups(sc):
    import czi_levels
    return czi_levels.level_groups(sc)

def _czi_levels(f, sc):
    """Downsampled pyramid levels as `czi_levels.py` defines them: one level per power-of-two
    factor, pylibCZIrw's level grid (`floor(w / f) x floor(h / f)`, anchored at the scene
    origin), czifile's pixels. Hashes use the same (c, z, t) order as level 0."""
    import czi_levels

    out = []
    groups = czi_levels.level_groups(sc)
    shape0 = dict(sc.sizes)
    C = shape0.get("C", 1); Z = shape0.get("Z", 1); T = shape0.get("T", 1)
    for li, ((fk, es), (w, hh)) in enumerate(zip(groups, czi_levels.level_sizes(sc, groups)), 1):
        if w * hh > MAX_LEVEL_PIXELS:
            out.append({"level": li, "size_x": w, "size_y": hh, "skipped": "larger than ORACLE_MAX_LEVEL_PIXELS", "planes": []})
            continue
        lv = czi_levels.Level(f, sc, fk, es)
        planes = []; n = 0
        for c in range(C):
            for z in range(Z):
                for t in range(T):
                    if n >= MAX_PLANES: break
                    planes.append({"c": c, "z": z, "t": t, "xxh3": h(lv.read(c, z, t, 0, 0, w, hh))}); n += 1
        out.append({"level": li, "size_x": w, "size_y": hh, "planes": planes})
    return out

def _czi_scaling(meta):
    out = {"x": None, "y": None, "z": None}
    try:
        if isinstance(meta, str):
            import xml.etree.ElementTree as ET
            root = ET.fromstring(meta)
            for d in root.findall("Metadata/Scaling/Items/Distance"):
                v = d.findtext("Value")
                if v is not None:
                    out[d.get("Id").lower()] = float(v) * 1e6
        else:
            items = meta["ImageDocument"]["Metadata"]["Scaling"]["Items"]["Distance"]
            if isinstance(items, dict): items = [items]
            for d in items:
                out[d["Id"].lower()] = float(d["Value"]) * 1e6
    except Exception:
        pass
    return out

def _spread(total: int, n: int) -> list:
    """Indices 0..total-1, or n evenly spaced ones (first and last included) when total > n."""
    if total <= n:
        return list(range(total))
    if n <= 1:
        return [0]
    return sorted({round(i * (total - 1) / (n - 1)) for i in range(n)})

JDN_UNIX_EPOCH = 2440587.5

def _jdn_fields(jdn):
    """ISO-8601 UTC (ms) and unix ms for a plausible Julian day number (1900..2100), else {}."""
    import datetime
    if not isinstance(jdn, (int, float)) or not (2415020.5 < jdn < 2488069.5):
        return {}
    ms = round((jdn - JDN_UNIX_EPOCH) * 86400000.0)
    dt = datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc) + datetime.timedelta(milliseconds=ms)
    return {"acquired_at": dt.strftime("%Y-%m-%dT%H:%M:%S.") + f"{ms % 1000:03d}Z", "acquired_at_unix_ms": ms}

def _nd2_meta(f) -> dict:
    """Metadata the ND2 reader normalizes: channels, acquisition start, per-frame records."""
    import numpy as np
    out = {}
    rdr = f._rdr
    try:
        chans = []
        for ch in f.metadata.channels or []:
            c = ch.channel
            chans.append({"name": c.name, "color": "#%02X%02X%02X" % (c.color.r, c.color.g, c.color.b),
                          "excitation_nm": c.excitationLambdaNm, "emission_nm": c.emissionLambdaNm,
                          "component_count": ch.volume.componentCount,
                          "modality": list(ch.microscope.modalityFlags or [])})
        out["channels"] = chans
        # Adjudicated 2026-09-26 (docs/provenance/nd2.md): when a plane's emission is described
        # only by a filter band (a rising and a falling edge, no probe spectrum), nd2 reports the
        # rising edge; the band centre is the emission wavelength (Bio-Formats agrees).
        if not f.is_legacy:
            pp = f.unstructured_metadata().get("ImageMetadataSeqLV|0", {}).get("SLxPictureMetadata", {}).get("PicturePlanes", {})
            planes = pp.get("Plane") or pp.get("PlaneNew") or {}
            for c, key in zip(chans, list(planes)):
                pl = planes[key]
                if (pl.get("FluorescentProbe") or {}).get("EmissionSpectrum", {}).get("Count"):
                    continue
                flt = next(iter(((pl.get("FilterPath") or {}).get("Filter") or {}).values()), {})
                pts = list(((flt.get("EmissionSpectrum") or {}).get("Point") or {}).values())
                lo = [p["Wavelength"] for p in pts if p.get("Type") == 2 and p.get("Wavelength")]
                hi = [p["Wavelength"] for p in pts if p.get("Type") == 3 and p.get("Wavelength")]
                if lo and hi:
                    c["emission_nm"] = (min(lo[0], hi[0]) + max(lo[0], hi[0])) / 2
                    c["emission_note"] = "band centre (adjudicated; nd2 reports the rising edge)"
    except Exception as e:
        out["channels_error"] = f"{type(e).__name__}: {e}"
    frames = []
    try:
        if f.is_legacy:
            jdn = rdr._frame0_meta().get("TimeAbsolute")
            n = len(rdr.chunkmap.get(b"VCAL", []))
            for i in _spread(n, FRAME_RECORDS):
                fm = rdr.frame_metadata(i)
                frames.append({"index": i, "time_ms": fm.get("TimeMSec"),
                               "stage_x_um": fm.get("XPos"), "stage_y_um": fm.get("YPos"), "stage_z_um": fm.get("ZPos")})
        else:
            jdn = rdr._cached_raw_metadata().get("dTimeAbsolute")
            id_by_header = {}
            try:
                cd = rdr._decode_chunk(b"CustomDataVar|CustomDataV2_0!")
                for t in cd.get("CustomTagDescription_v1.0", {}).values():
                    head = t["Desc"] + (f" [{t['Unit']}]" if t["Unit"].strip() else "")
                    id_by_header.setdefault(head, t["ID"])
            except Exception:
                pass
            ev = f.events(orient="list")
            n = len(ev.get("Index", []))
            for i in _spread(n, FRAME_RECORDS):
                rec = {"index": int(ev["Index"][i])}
                if "Time [s]" in ev:
                    rec["time_ms"] = float(ev["Time [s]"][i]) * 1000.0
                tags = {}
                for head, tid in id_by_header.items():
                    if head in ev:
                        v = ev[head][i]
                        tags[tid] = v.item() if hasattr(v, "item") else v
                rec["tags"] = tags
                frames.append(rec)
        out["frame_count"] = n
        out["frames"] = frames
        out.update(_jdn_fields(jdn))
    except Exception as e:
        out["frames_error"] = f"{type(e).__name__}: {e}"
    return out

FRAME_RECORDS = int(os.environ.get("ORACLE_FRAME_RECORDS", "24"))

def nd2_(p: Path) -> dict:
    """One oracle image per XY position. Planes hashed: all (c, z, t) when there are at most
    MAX_PLANES, otherwise MAX_PLANES evenly spaced ones (first and last included) so the tail of
    the frame order is covered too."""
    import nd2
    images = []
    with nd2.ND2File(p) as f:
        sizes = f.sizes  # e.g. {'P':4,'T':3,'Z':5,'C':2,'Y':32,'X':32} (+ 'S' for rgb)
        a = f.asarray()
        axes = list(sizes.keys())
        P = sizes.get("P", 1); T = sizes.get("T", 1); Z = sizes.get("Z", 1); C = sizes.get("C", 1)
        def get(pi, t, z, c):
            idx = []
            for d in axes:
                if d == "P": idx.append(pi)
                elif d == "T": idx.append(t)
                elif d == "Z": idx.append(z)
                elif d == "C": idx.append(c)
                elif d in ("Y", "X", "S"): idx.append(slice(None))
                else: idx.append(0)
            plane = a[tuple(idx)]
            # nd2 returns the samples of a colour-camera (3-component) plane in stored order,
            # which in modern (chunked) files is B, G, R: zenodo8161776-VPA002's DAPI plane lights
            # sample 0 and its Texas Red plane sample 2, and nd2 itself gives component 0 the
            # pseudo-wavelength 420 nm. The oracle compares R, G, B (docs/provenance/nd2.md,
            # 2026-09-24). Legacy (JPEG 2000) files keep the codestream order: their JP2 colour
            # box declares sRGB and no file shows otherwise.
            if sizes.get("S") == 3 and not f.is_legacy:
                plane = plane[..., ::-1]
            return plane
        try:
            vox = f.voxel_size()
            try:
                calibrated = f.metadata.channels[0].volume.axesCalibrated
            except Exception:
                calibrated = (True, True, True)
            vx = vox.x if calibrated[0] else None
            vy = vox.y if calibrated[1] else None
            vz = vox.z if calibrated[2] else None
        except Exception:
            # nd2 0.11.3 cannot build channel metadata for some files (IndexError on more than
            # three RGB planes); geometry and pixels are still compared.
            vx = vy = vz = "unknown"
        try:
            names = [ch.channel.name for ch in f.metadata.channels] if f.metadata and f.metadata.channels else None
        except Exception:
            names = None
        order = [(c, z, t) for c in range(C) for z in range(Z) for t in range(T)]
        pick = _spread(len(order), MAX_PLANES)
        for pi in range(P):
            planes = []
            for k in pick:
                c, z, t = order[k]
                planes.append({"c": c, "z": z, "t": t, "xxh3": h(get(pi, t, z, c))})
            images.append({"index": pi, "size_x": sizes["X"], "size_y": sizes["Y"], "size_z": Z, "size_c": C, "size_t": T,
                           "samples_per_pixel": sizes.get("S", 1), "pixel_type": dtype_name(f.dtype), "sizes": sizes,
                           "physical_size_um": {} if vx == "unknown" else {"x": vx, "y": vy, "z": vz if Z > 1 else None},
                           "channel_names": names,
                           "planes": planes})
        return {"reader": f"nd2 {nd2.__version__}", "version": ".".join(map(str, f.version)), "is_legacy": f.is_legacy, "images": images,
                "nd2_meta": _nd2_meta(f),
                "text_info": f.text_info, "attributes": _jsonable(f.attributes.__dict__ if hasattr(f.attributes, "__dict__") else str(f.attributes))}

# ---------------------------------------------------------------------------------- TIFF family
#
# Which tifffile series are our images, and how tifffile axes map to (c, z, t):
#   * OME-TIFF, Micro-Manager stacks, generic/uniform/shaped TIFF: every series is an image.
#   * ImageJ, LSM, Aperio SVS, Hamamatsu NDPI, PerkinElmer QPI: only series[0] (the data or
#     baseline); thumbnails, labels, macros and pyramid levels are attachments/levels.
#   * Axes of the (squeezed) series:
#       Y, X            -> the plane
#       S last          -> interleaved samples (samples_per_pixel), e.g. RGB
#       C after X       -> interleaved samples as well (LSM chunky pages)
#       C               -> channel; S not last (planar samples) -> channel too, c = c_C * S + s
#       Z, I, Q         -> z  (I/Q are tifffile's "unidentified sequence": pages as Z)
#       T               -> t
#       P, M            -> separate images (positions / mosaic tiles), in row-major order
#       anything else   -> not mappable; recorded as an error for that file
#   * Physical sizes come from tifffile's own coordinate parsing (series.coord_scales, in
#     micrometres); axes tifffile gives no calibrated scale for are omitted (not compared).
#   * BinaryOnly OME-TIFFs are followed to their metadata file; *.companion.ome files are
#     resolved per the OME-TIFF specification with tifffile reading each referenced page.
TIFF_MAX_BYTES = int(os.environ.get("ORACLE_TIFF_MAX_BYTES", str(1 << 30)))  # skip hashing larger series
TIFF_SUFFIXES = (".tif", ".tiff", ".btf", ".tf2", ".tf8", ".svs", ".ndpi", ".lsm", ".qptiff", ".stk", ".eer", ".gain", ".scn", ".bif", ".gel")
_UNIT_UM = {"micrometer": 1.0, "micron": 1.0, "um": 1.0, "µm": 1.0, "\\u00b5m": 1.0, "nanometer": 1e-3, "nm": 1e-3, "millimeter": 1e3, "mm": 1e3, "centimeter": 1e4, "cm": 1e4, "meter": 1e6, "m": 1e6}

def _scales_um(s, axes, sizes):
    out = {}
    try:
        scales, units = s.coord_scales, s.coord_units
    except Exception:
        return out
    explicit = set(getattr(s, "_coords", {}) or {}) | set(getattr(s, "_units", None) or {})
    # tifffile labels an ImageJ series' X/Y scales micrometer whatever the description's `unit`
    # (a `.gel` scan re-saved by ImageJ says unit=cm); the description's unit is the one meant
    ij_unit = None
    if getattr(s, "kind", "") == "imagej":
        try:
            ij_unit = (s.parent.imagej_metadata or {}).get("unit")
        except Exception:
            ij_unit = None
    for ax in "XYZ":
        # only scales the format code set explicitly (not tifffile's fallback to the
        # resolution tags of the first page, which carry no unit meaning in most files)
        if ax not in axes or ax not in scales or ax not in explicit:
            continue
        unit = ij_unit if (ij_unit and ax in "XY") else units.get(ax, "")
        f = _UNIT_UM.get(str(unit).lower())
        if f is None or scales[ax] <= 0:
            continue
        if ax == "Z" and sizes.get("z", 1) <= 1:
            continue
        out[ax.lower()] = float(scales[ax]) * f
    return out

def _map_axes(axes, shape):
    """Return (roles, spp) where roles[i] in {'y','x','s','c','cs','z','t','split'}."""
    roles, spp = [], 1
    xi = axes.index("X")
    for i, a in enumerate(axes):
        if a in "YX":
            roles.append(a.lower())
        elif a == "S" and i == len(axes) - 1:
            roles.append("s"); spp = shape[i]
        elif a == "C" and i > xi:
            roles.append("s"); spp = shape[i]
        elif a == "S":
            roles.append("cs")
        elif a == "C":
            roles.append("c")
        elif a in "ZIQ":
            roles.append("z")
        elif a == "T":
            roles.append("t")
        elif a in "PM":
            roles.append("split")
        else:
            raise ValueError(f"unmappable tifffile axis {a!r} in {axes}")
    if roles.count("z") > 1 or roles.count("t") > 1 or roles.count("c") > 1:
        raise ValueError(f"ambiguous axes {axes}")
    return roles, spp

_MODULO_LETTER = {"angle": "A", "phase": "P", "tile": "R", "lifetime": "H", "lambda": "E", "other": "Q"}


def _fold_modulo(s, axes, shape):
    """OME Modulo: tifffile shows a sub-dimension folded into Z, C or T as its own axis (A, P, R,
    H, E, Q) right after its parent, or instead of a parent of size 1. Fold it back (the
    sub-dimension varies fastest within its parent), so planes are indexed by the stored C, Z, T
    as OpenReadout indexes them. Returns (axes, shape, modulo sizes {parent: size})."""
    import re
    try:
        xml = s.parent.ome_metadata or ""
    except Exception:
        xml = ""
    if "omero/dimension/modulo" not in xml:
        return axes, tuple(shape), {}
    axes, shape, sizes = list(axes), list(shape), {}
    for parent, kind in re.findall(r'<(?:\w+:)?ModuloAlong([ZCT])\b[^>]*?\bType="([^"]+)"', xml):
        m = _MODULO_LETTER.get(kind.lower(), "Q")
        if m not in axes:
            continue
        i = axes.index(m)
        sizes[parent] = shape[i]
        if i > 0 and axes[i - 1] == parent:
            shape[i - 1] *= shape[i]
            del axes[i], shape[i]
        elif parent not in axes:
            axes[i] = parent
    return "".join(axes), tuple(shape), sizes


def _series_images(s, first_index):
    import itertools
    axes, shape, modulo = _fold_modulo(s, s.axes, s.shape)
    roles, spp = _map_axes(axes, shape)
    size = lambda r: int(np.prod([n for rr, n in zip(roles, shape) if rr == r])) if r in roles else 1
    C = size("c") * size("cs"); Z = size("z"); T = size("t")
    split_axes = [i for i, r in enumerate(roles) if r == "split"]
    splits = list(itertools.product(*[range(shape[i]) for i in split_axes])) or [()]
    nbytes = int(np.prod(shape)) * np.dtype(s.dtype).itemsize
    arr = np.asarray(s.asarray()).reshape(shape) if nbytes <= TIFF_MAX_BYTES else None
    # our conventions for sample types NumPy spells differently: 1-bit samples (bool) are
    # uint8 0/1, half floats are widened to float32 (docs/formats/tiff.md § Sample formats)
    out_dtype = {np.dtype(bool): np.dtype(np.uint8), np.dtype(np.float16): np.dtype(np.float32)}.get(np.dtype(s.dtype), np.dtype(s.dtype))
    if arr is not None and arr.dtype != out_dtype:
        arr = arr.astype(out_dtype)
    # JPEG decoders may differ by one count in some samples (IDCT rounding); record plane means
    # so the harness can apply a tolerance to files marked `lossy` in the manifest.
    lossy = s.keyframe.compression in (6, 7, 33003, 33004, 33005, 34712)
    ci = roles.index("c") if "c" in roles else None
    si = roles.index("cs") if "cs" in roles else None
    S = shape[si] if si is not None else 1
    images = []
    for k, sp in enumerate(splits):
        planes = []; n = 0
        if arr is not None:
            for c in range(C):
                for z in range(Z):
                    for t in range(T):
                        if n >= MAX_PLANES: break
                        sel = []
                        for i, r in enumerate(roles):
                            if r == "c": sel.append(c // S if si is not None else c)
                            elif r == "cs": sel.append(c % S)
                            elif r == "z": sel.append(z)
                            elif r == "t": sel.append(t)
                            elif r == "split": sel.append(sp[split_axes.index(i)])
                            else: sel.append(slice(None))
                        plane = arr[tuple(sel)]
                        rec = {"c": c, "z": z, "t": t, "xxh3": h(plane)}
                        if lossy:
                            rec["mean"] = float(np.asarray(plane, dtype=np.float64).mean())
                        planes.append(rec); n += 1
        sizes = {"z": Z}
        images.append({
            "index": first_index + k, "name": s.name, "kind": s.kind, "axes": axes, "shape": list(shape),
            "size_x": shape[axes.index("X")], "size_y": shape[axes.index("Y")], "size_z": Z, "size_c": C, "size_t": T,
            "samples_per_pixel": spp, "pixel_type": dtype_name(out_dtype), "pyramid_levels": len(s.levels), "lossy": lossy,
            "physical_size_um": _scales_um(s, axes, sizes),
            "planes_skipped": None if arr is not None else f"series is {nbytes} bytes (> ORACLE_TIFF_MAX_BYTES)",
            "planes": planes,
        })
        if modulo:
            images[-1]["modulo"] = modulo
            images[-1]["axes_unfolded"] = s.axes
    return images

def _ome_companion(xml_path: Path) -> dict:
    """Resolve a companion OME-XML per the OME-TIFF specification (TiffData IFD/First*/PlaneCount,
    UUID/@FileName) and read each referenced page with tifffile."""
    import tifffile, xml.etree.ElementTree as ET
    root = ET.fromstring(xml_path.read_text())
    local = lambda e: e.tag.rsplit("}", 1)[-1]
    images = []
    for im in [e for e in root if local(e) == "Image"]:
        px = next((e for e in im if local(e) == "Pixels"), None)
        tds = [e for e in px if local(e) == "TiffData"] if px is not None else []
        if not tds:
            continue
        order = [a for a in px.get("DimensionOrder", "XYZCT") if a in "ZCT"]
        sz = {a: int(px.get("Size" + a, "1")) for a in "XYZCT"}
        chans = [e for e in px if local(e) == "Channel"]
        spp = int(chans[0].get("SamplesPerPixel", "1")) if chans else 1
        sz["C"] //= spp
        total = sz["Z"] * sz["C"] * sz["T"]
        def linear(c, z, t):
            idx, stride = 0, 1
            for a in order:
                idx += {"C": c, "Z": z, "T": t}[a] * stride; stride *= sz[a]
            return idx
        def unravel(l):
            out = {}
            for a in order:
                out[a] = l % sz[a]; l //= sz[a]
            return out["C"], out["Z"], out["T"]
        where = {}
        for td in tds:
            u = next((e for e in td if local(e) == "UUID"), None)
            fname = u.get("FileName") if u is not None else xml_path.name
            ifd = int(td.get("IFD", "0"))
            count = int(td.get("PlaneCount", "1" if "IFD" in td.attrib else "0")) or None
            l0 = linear(int(td.get("FirstC", "0")), int(td.get("FirstZ", "0")), int(td.get("FirstT", "0")))
            for i in range(count or total):
                if l0 + i < total:
                    where[l0 + i] = (fname, ifd + i)
        planes = []; n = 0; dtype = None
        for c in range(sz["C"]):
            for z in range(sz["Z"]):
                for t in range(sz["T"]):
                    ref = where.get(linear(c, z, t))
                    if ref is None or n >= MAX_PLANES: continue
                    with tifffile.TiffFile(xml_path.parent / ref[0]) as tf:
                        a = tf.pages[ref[1]].asarray()
                    dtype = a.dtype
                    planes.append({"c": c, "z": z, "t": t, "xxh3": h(a)}); n += 1
        um = {}
        for ax in "XYZ":
            v = px.get("PhysicalSize" + ax)
            unit = px.get("PhysicalSize" + ax + "Unit", "µm")
            f = {"µm": 1.0, "um": 1.0, "nm": 1e-3, "mm": 1e3, "cm": 1e4, "m": 1e6}.get(unit)
            if v is not None and f is not None and not (ax == "Z" and sz["Z"] <= 1):
                um[ax.lower()] = float(v) * f
        images.append({"index": len(images), "name": im.get("Name"), "kind": "ome-companion",
                       "size_x": sz["X"], "size_y": sz["Y"], "size_z": sz["Z"], "size_c": sz["C"], "size_t": sz["T"],
                       "samples_per_pixel": spp, "pixel_type": dtype_name(dtype) if dtype is not None else px.get("Type"),
                       "physical_size_um": um, "planes": planes})
    return {"reader": f"tifffile {tifffile.__version__} (companion resolved per OME-TIFF spec)", "images": images}

def tiff(p: Path) -> dict:
    import tifffile
    if p.name.lower().endswith(".companion.ome"):
        return _ome_companion(p)
    with tifffile.TiffFile(p) as tf:
        nis = _is_nis(tf)
        is_eer = tf.pages[0].compression in (65000, 65001, 65002)
    if nis:
        return nis_export(p)
    if is_eer:
        return eer_movie(p)
    with tifffile.TiffFile(p) as tf:
        ome = tf.ome_metadata if tf.is_ome else None
        if ome and "<BinaryOnly" in ome:
            m = re.search(r'MetadataFile="([^"]+)"', ome)
            meta = p.parent / m.group(1)
            if meta.name.lower().endswith(".companion.ome"):
                out = _ome_companion(meta)
            else:
                out = tiff(meta)
            out["binary_only_metadata_file"] = meta.name
            return out
        series = tf.series
        kind = series[0].kind if series else None
        if kind == "mmstack" and tf.is_ome:
            # openreadout follows the OME-XML (the open standard) before Micro-Manager's own
            # index map; ask tifffile for its OME series of the same file.
            ome_series = tf._get_series("ome", squeeze=True)
            if ome_series:
                series, kind = ome_series, "ome"
        # Leica SCN: every <image> of the collection is an image (overview and scanned regions)
        pick = series if kind in ("ome", "generic", "uniform", "shaped", "mmstack", "scn") else series[:1]
        images = []
        for s in pick:
            images.extend(_series_images(s, len(images)))
        if 33550 in tf.pages[0].tags:
            # GeoTIFF (ModelPixelScaleTag): tifffile's scales are map units, not a microscope's
            # pixel size; openreadout reports none for them
            for im in images:
                im["physical_size_um"] = {}
        flags = [f for f in ("ome", "imagej", "lsm", "svs", "ndpi", "qpi", "micromanager", "bigtiff", "shaped", "scn") if getattr(tf, "is_" + f, False)]
        out = {"reader": f"tifffile {tifffile.__version__}", "kind": kind, "flags": flags,
               "page_count": len(tf.pages), "series_count": len(series), "images": images}
    if kind == "philips" and images:
        # Philips TIFF holds two pixel sizes: the WSI's DICOM_PIXEL_SPACING (OpenSlide's mpp,
        # and openreadout's) and level 0's representation spacing / TIFF resolution (tifffile's
        # scale; a rounded nominal value in converted files). The physical size is taken from
        # OpenSlide (LGPL, black box); tifffile's is kept for the record.
        import openslide
        o = openslide.OpenSlide(str(p))
        images[0]["physical_size_tifffile_um"] = images[0].get("physical_size_um")
        images[0]["physical_size_um"] = {"x": float(o.properties["openslide.mpp-x"]), "y": float(o.properties["openslide.mpp-y"])}
        out["reader"] += f" (physical size: OpenSlide {openslide.__library_version__})"
    return out

def eer_movie(p: Path) -> dict:
    """Thermo Fisher EER movie (docs/formats/tiff.md § EER): every page is a frame (T), decoded by
    tifffile + imagecodecs (BSD-3) at the sensor resolution (superres 0; tifffile returns bool,
    which is our uint8 0/1). The first MAX_PLANES frames are hashed; every frame's event count
    is compared with the dose (events / pixels to 6 decimals) the writer records in tag 65002."""
    import tifffile
    with tifffile.TiffFile(p) as tf:
        meta = tf.eer_metadata or {}
        pages = [pg for pg in tf.pages if pg.compression == tf.pages[0].compression and pg.shape == tf.pages[0].shape]
        first = pages[0]
        planes, dose_ok, dose_n, events = [], True, 0, 0
        for t, pg in enumerate(pages):
            a = pg.asarray()
            ev = int(np.count_nonzero(a))
            events += ev
            if t < MAX_PLANES:
                planes.append({"c": 0, "z": 0, "t": t, "xxh3": h(a.astype(np.uint8))})
            tag = pg.tags.get(65002)
            if tag is not None:
                m = re.search(rb'name="dose"[^>]*>([^<]*)<', bytes(tag.value))
                if m:
                    dose_n += 1
                    dose_ok &= abs(ev / a.size - float(m.group(1))) <= 1e-6
        px = lambda k: meta.get(k) * 1e6 if meta.get(k) and meta.get(k + ".unit") == "m" else None
        phys = {k: v for k, v in (("x", px("sensorPixelSize.width")), ("y", px("sensorPixelSize.height"))) if v}
        images = [{"index": 0, "size_x": first.shape[1], "size_y": first.shape[0], "size_z": 1, "size_c": 1,
                   "size_t": len(pages), "samples_per_pixel": 1, "pixel_type": "uint8",
                   "physical_size_um": phys, "planes": planes}]
    return {"reader": f"tifffile {tifffile.__version__} + imagecodecs (eer_decode, superres 0)", "kind": "eer",
            "page_count": len(tf.pages), "images": images, "eer_compression": first.compression,
            "events_total": events, "frames_with_dose": dose_n, "dose_matches_events": dose_ok}

_NIS_TOKEN = re.compile(r"(xy|[tzc])(\d{1,9})$", re.I)


def _nis_name(name: str):
    """Split a NIS-Elements export file name into (prefix, [(axis, n), ...], extension), reading
    the index tokens xy<n>, t<n>, z<n>, c<n> from the end of the stem (docs/formats/tiff.md
    § NIS-Elements); None without tokens, without a prefix or with an axis twice."""
    if "." not in name:
        return None
    stem, ext = name.rsplit(".", 1)
    toks = []
    while True:
        m = _NIS_TOKEN.search(stem)
        if not m or m.start() == 0:
            break
        toks.append((m.group(1).lower(), int(m.group(2))))
        stem = stem[:m.start()]
    axes = [a for a, _ in toks]
    if not toks or len(set(axes)) != len(axes):
        return None
    return stem, list(reversed(toks)), ext.lower()


def _is_nis(tf) -> bool:
    t = tf.pages[0].tags.get(65331)
    return t is not None and bytes(t.value)[8:40].decode("utf-16-le", "replace") == "MetadataTiffV1_0"


def nis_export(p: Path) -> dict:
    """Nikon NIS-Elements TIFF export: the files that share the opened file's prefix, extension
    and index-token letters are one data set (positions `xy` as images, `c`, `z`, `t` sorted by
    number). Every plane is read with tifffile (BSD-3) from its own file; Bio-Formats 8.5.0
    (GPL, black box; its NikonElementsTiffReader reads each file on its own) reads every file
    too, and `bioformats_agrees` records whether its planes are the same."""
    import tempfile, shutil, subprocess, tifffile
    own = _nis_name(p.name)
    files = {}
    if own is None:
        files[(0, 0, 0, 0)] = p
        pos = ch = zs = ts = [0]
    else:
        prefix, toks, ext = own
        axes = [a for a, _ in toks]
        members = []
        for q in sorted(p.parent.iterdir()):
            n = _nis_name(q.name)
            if q.is_file() and n and n[0] == prefix and n[2] == ext and [a for a, _ in n[1]] == axes:
                members.append((dict(n[1]), q))
        vals = lambda a: sorted({d.get(a, 0) for d, _ in members}) or [0]
        pos, ch, zs, ts = vals("xy"), vals("c"), vals("z"), vals("t")
        for d, q in members:
            files[(pos.index(d.get("xy", 0)), ch.index(d.get("c", 0)), zs.index(d.get("z", 0)), ts.index(d.get("t", 0)))] = q
    agree = []
    with tempfile.TemporaryDirectory() as td:
        images = []
        for pi, _ in enumerate(pos):
            planes = []
            dtype = shape = None
            for c in range(len(ch)):
                for z in range(len(zs)):
                    for t in range(len(ts)):
                        q = files.get((pi, c, z, t))
                        if q is None:
                            continue
                        with tifffile.TiffFile(q) as tf:
                            a = tf.pages[0].asarray()
                        dtype, shape = a.dtype, a.shape
                        planes.append({"c": c, "z": z, "t": t, "xxh3": h(a)})
                        solo = Path(td) / f"p{pi}c{c}z{z}t{t}"
                        solo.mkdir()
                        shutil.copyfile(q, solo / q.name)
                        out = solo / "bf.ome.tif"
                        subprocess.run([_bftools("bfconvert"), "-no-upgrade", "-overwrite", str(solo / q.name), str(out)],
                                       capture_output=True, text=True, timeout=3600, check=True)
                        with tifffile.TiffFile(out) as tb:
                            agree.append(h(tb.pages[0].asarray()) == planes[-1]["xxh3"])
                        shutil.rmtree(solo)
            images.append({"index": pi, "size_x": shape[1], "size_y": shape[0], "size_z": len(zs), "size_c": len(ch),
                           "size_t": len(ts), "samples_per_pixel": 1, "pixel_type": dtype_name(dtype),
                           "physical_size_um": {}, "planes": planes})
    return {"reader": f"tifffile {tifffile.__version__} per file, grouped by NIS-Elements export names; Bio-Formats 8.5.0 (GPL, black box) second opinion per file",
            "member_files": len(files), "bioformats_agrees": all(agree), "images": images}

def _corpus_id(p: Path) -> str:
    """Oracle id of a TIFF-family file: its manifest id (oracle files are named by id, and TIFF
    file names are not unique once extensions are stripped: .ome.tiff / .ome.btf / .ome.tf8
    variants, multi-file sets in sub-directories). Falls back to the file stem."""
    import tomllib
    base = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files")).resolve()
    try:
        rel = p.resolve().relative_to(base).as_posix()
    except ValueError:
        rel = p.name
    with open(ROOT / "corpus" / "manifest.toml", "rb") as f:
        for e in tomllib.load(f)["file"]:
            if e.get("filename") == rel:
                return e["id"]
    return p.name.rsplit(".", 1)[0]

def _fcs_hash(events) -> str:
    """xxh3-128 of an (events x parameters) matrix as column-major little-endian float64."""
    a = np.asarray(events, dtype="<f8")
    return xxhash.xxh3_128_hexdigest(np.asfortranarray(a).tobytes(order="F"))

def _fcs_dtype(datatype: str, bits) -> str:
    """Storage dtype derived from $DATATYPE and $PnB (the rule documented in docs/formats/fcs.md)."""
    dt = (datatype or "").strip().upper()
    if dt == "F": return "float32"
    if dt == "D": return "float64"
    if dt == "A": return "float64"
    if dt == "I":
        try: b = int(str(bits).strip())
        except ValueError: return "unknown"
        return "uint8" if b <= 8 else "uint16" if b <= 16 else "uint32" if b <= 32 else "uint64"
    return "unknown"

def _fcsparser_numpy2_shim(fcsparser):
    """fcsparser 0.2.4 calls ndarray.newbyteorder(), which NumPy 2 removed, on big-endian files.
    Wrap its module-level `fromfile` so the arrays it builds keep that method. Black-box shim only:
    the byte decoding is still fcsparser's own."""
    api = fcsparser.api
    if getattr(api, "_openreadout_shim", False):
        return
    class _Compat(np.ndarray):
        def newbyteorder(self, order="S"):
            return self.view(self.dtype.newbyteorder(order))
    orig = api.fromfile
    api.fromfile = lambda *a, **k: orig(*a, **k).view(_Compat)
    api._openreadout_shim = True

def fcs(p: Path) -> dict:
    """One oracle table per FCS data set. flowio is the primary reader (it follows $NEXTDATA as a
    relative offset, prefers TEXT offsets, applies $PnR bit masks); fcsparser is a second opinion for
    data set 0. Data sets neither reader can decode keep their metadata with `xxh3: null`."""
    import flowio, fcsparser
    tables, notes = [], []
    try:
        sets = flowio.read_multiple_data_sets(str(p), ignore_offset_error=True, ignore_offset_discrepancy=True)
    except Exception as e:
        notes.append(f"flowio (events): {type(e).__name__}: {e}")
        sets = None
    if sets is None:
        try:
            sets = flowio.read_multiple_data_sets(str(p), ignore_offset_error=True, ignore_offset_discrepancy=True, only_text=True)
        except Exception as e:
            notes.append(f"flowio (text only): {type(e).__name__}: {e}")
            sets = []
    for i, fd in enumerate(sets):
        text = fd.text
        par = int(text["par"])
        names = [fd.channels[n]["pnn"] for n in sorted(fd.channels)]
        labels = [fd.channels[n]["pns"] or None for n in sorted(fd.channels)]
        # FCS 3.2 $PnDATATYPE: a measurement stored with its own data type.
        ptypes = [(text.get(f"p{n}datatype") or text.get("datatype")) for n in range(1, par + 1)]
        dtypes = [_fcs_dtype(ptypes[n - 1], text.get(f"p{n}b")) for n in range(1, par + 1)]
        h = None
        reader = f"flowio {flowio.__version__}"
        if fd.events is not None:
            try:
                arr = fd.as_array(preprocess=False)
                if any(pt != text.get("datatype") for pt in ptypes):
                    arr = _fcs32_mixed(p, fd, text, ptypes, par, arr)
                    reader += " (FCS 3.2 $PnDATATYPE columns re-read from DATA with numpy)"
                h = _fcs_hash(arr)
            except Exception as e:
                notes.append(f"flowio data set {i}: {type(e).__name__}: {e}")
        tables.append({"index": i, "version": fd.version, "parameter_count": par, "event_count": int(text["tot"].strip()),
                       "parameter_names": names, "parameter_labels": labels, "dtypes": dtypes, "xxh3": h,
                       "xxh3_reader": reader if h else None})
    # second opinion: fcsparser, data set 0 (its $NEXTDATA handling is absolute, so later sets are not used)
    second = None
    try:
        _fcsparser_numpy2_shim(fcsparser)
        meta, data = fcsparser.parse(str(p), channel_naming="$PnN", dtype="float64", reformat_meta=False)
        second = {"reader": "fcsparser", "event_count": int(data.shape[0]), "xxh3": _fcs_hash(data.to_numpy())}
        if not tables:
            par = int(meta["$PAR"])
            key = lambda n, c: meta.get(f"$P{n}{c}")
            tables.append({"index": 0, "version": None,
                           "parameter_count": par, "event_count": int(str(meta["$TOT"]).strip()),
                           "parameter_names": [key(n, "N") for n in range(1, par + 1)],
                           "parameter_labels": [key(n, "S") for n in range(1, par + 1)],
                           "dtypes": [_fcs_dtype(meta.get("$DATATYPE"), key(n, "B")) for n in range(1, par + 1)],
                           "xxh3": second["xxh3"], "xxh3_reader": "fcsparser"})
    except Exception as e:
        second = {"reader": "fcsparser", "error": f"{type(e).__name__}: {e}"}
    if tables and second.get("xxh3") and tables[0].get("xxh3_reader") != "fcsparser":
        if tables[0]["xxh3"] is None:
            tables[0]["xxh3"] = second["xxh3"]; tables[0]["xxh3_reader"] = "fcsparser"
        else:
            second["agrees_with_flowio"] = second["xxh3"] == tables[0]["xxh3"]
    if not tables:
        raise RuntimeError("; ".join(notes + [f"fcsparser: {second.get('error')}"]))
    out = {"reader": f"flowio {flowio.__version__} + fcsparser (second opinion)", "tables": tables, "second_opinion": second}
    if notes: out["notes"] = notes
    return out

# ---------------------------------------------------------------- electron microscopy

_LENGTH_UM = {"nm": 1e-3, "µm": 1.0, "μm": 1.0, "um": 1.0, "micron": 1.0, "Å": 1e-4, "A": 1e-4,
              "angstrom": 1e-4, "Angstrom": 1e-4, "pm": 1e-6, "mm": 1e3, "m": 1e6}


def _fcs32_mixed(p, fd, text, ptypes, par, arr):
    """FlowIO 1.4 decodes every measurement with $DATATYPE and ignores FCS 3.2's $PnDATATYPE.
    Re-read the whole DATA segment (located by FlowIO's $BEGINDATA/$ENDDATA) with a numpy
    structured dtype built from the per-measurement types and widths, integer columns masked by
    $PnR as in FCS 3.1 §3.3; FlowIO's values are kept for every other column."""
    import numpy as np
    order = "<" if text.get("byteord", "").strip().startswith("1") else ">"
    fields = []
    for n in range(1, par + 1):
        bits = int(text[f"p{n}b"])
        code = {"I": f"u{bits // 8}", "F": "f4", "D": "f8"}[ptypes[n - 1].strip().upper()]
        fields.append((f"p{n}", order + code))
    begin = int(text["begindata"]); end = int(text["enddata"])
    raw = open(p, "rb").read()[begin:end + 1]
    rec = np.dtype(fields)
    tot = int(text["tot"].strip())
    data = np.frombuffer(raw[: tot * rec.itemsize], dtype=rec)
    out = arr.copy()
    for n in range(1, par + 1):
        if ptypes[n - 1].strip().upper() == text.get("datatype", "").strip().upper():
            continue
        col = data[f"p{n}"]
        if ptypes[n - 1].strip().upper() == "I":
            bits = int(text[f"p{n}b"]); r = float(text.get(f"p{n}r", "0"))
            full = (1 << bits) - 1
            if r >= 1:
                mask = (1 << max(0, (int(np.ceil(r)) - 1).bit_length())) - 1
                if mask < full:
                    col = col & np.array(mask, dtype=col.dtype)
        out[:, n - 1] = col.astype(np.float64)
    return out


def _planes_ct(get, C, Z, T):
    """Hash planes in (c, z, t) order, at most MAX_PLANES; get(c, z, t) -> 2-D array."""
    out, n = [], 0
    for c in range(C):
        for z in range(Z):
            for t in range(T):
                if n >= MAX_PLANES:
                    return out
                out.append({"c": c, "z": z, "t": t, "xxh3": h(get(c, z, t))}); n += 1
    return out


def mrc(p: Path) -> dict:
    """MRC/CCP4 read with mrcfile (BSD-3). One image. Section organisation follows our rule
    (docs/formats/mrc.md): ISPG 401-630 with NZ % MZ == 0 -> MZ sections per volume, NZ/MZ volumes (T);
    ISPG 0 -> image stack (T) unless the extension is .map/.rec/.ccp4; otherwise a volume (Z).
    Mode 0 is int8 unless the IMOD stamp is present without the signed-bytes flag (then uint8);
    mode 12 half floats are hashed widened to float32. Pixel sizes: CELLA/(MX,MY,MZ) of the cell axis
    each file axis maps to (MAPC/MAPR/MAPS), in µm; mrcfile's own voxel_size is recorded too."""
    import mrcfile, struct
    with mrcfile.mmap(p, mode="r", permissive=True) as m:
        hd = m.header
        nx, ny, nz, mz, ispg = int(hd.nx), int(hd.ny), int(hd.nz), int(hd.mz), int(hd.ispg)
        mode = int(hd.mode)
        ext = p.suffix.lower().lstrip(".")
        if 401 <= ispg <= 630 and mz > 0 and nz % mz == 0:
            Z, T, layout = mz, nz // mz, "volume-stack"
        elif ispg == 0 and ext not in ("map", "rec", "ccp4"):
            Z, T, layout = 1, nz, "image-stack"
        else:
            Z, T, layout = nz, 1, "volume"
        extra = bytes(hd.extra1) + bytes(hd.exttyp) + np.asarray(hd.nversion).astype(hd.nversion.dtype).tobytes() + bytes(hd.extra2)
        bo = "<" if hd.mode.dtype.byteorder in "<=|" else ">"
        stamp, flags = struct.unpack(bo + "ii", extra[152 - 96:160 - 96])
        unsigned = mode == 0 and stamp == 1146047817 and not (flags & 1)
        cella = [float(hd.cella.x), float(hd.cella.y), float(hd.cella.z)]
        grid = [int(hd.mx), int(hd.my), int(hd.mz)]
        maps = [int(hd.mapc), int(hd.mapr), int(hd.maps)]
        amap = [a - 1 for a in maps] if sorted(maps) == [1, 2, 3] else [0, 1, 2]
        def step(a):
            return cella[a] / grid[a] if grid[a] > 0 and cella[a] > 0 else None
        um = [None if step(a) is None else step(a) / 1e4 for a in amap]
        data = m.data
        if mode == 12:
            ptype = "float"
        elif mode == 0:
            ptype = "uint8" if unsigned else "int8"
        else:
            ptype = dtype_name(data.dtype) if data is not None and data.dtype.kind != "c" else None
        sections = data.reshape(nz, ny, nx) if data is not None and data.dtype.kind != "c" else None

        def get(c, z, t):
            a = np.asarray(sections[t * Z + z])
            if mode == 12:
                a = a.astype(np.float32)
            if unsigned:
                a = a.view(np.uint8)
            return a

        img = {"index": 0, "size_x": nx, "size_y": ny, "size_z": Z, "size_c": 1, "size_t": T,
               "pixel_type": ptype, "layout": layout,
               "physical_size_um": {"x": um[0], "y": um[1], "z": um[2] if layout != "image-stack" else None},
               "mrcfile_voxel_size_angstrom": [float(v) for v in m.voxel_size.item()],
               "planes": _planes_ct(get, 1, Z, T) if sections is not None else []}
        if sections is None:
            img["skip"] = f"mode {mode} not read by mrcfile"
        ext_h = m.indexed_extended_header if bytes(hd.exttyp) in (b"FEI1", b"FEI2") else None  # mrcfile 1.5.4 fails on other types
        fei = None
        if ext_h is not None and len(ext_h):
            e0 = ext_h[0]
            fei = {"microscope_type": bytes(e0["Microscope type"]).decode("latin-1").strip("\0 "),
                   "application": bytes(e0["Application"]).decode("latin-1").strip("\0 "),
                   "high_tension": float(e0["HT"]), "pixel_size_x": float(e0["Pixel size X"]),
                   "camera_name": bytes(e0["Camera name"]).decode("latin-1").strip("\0 ")}
        return {"reader": f"mrcfile {mrcfile.__version__}", "images": [img],
                "header": {"mode": mode, "ispg": ispg, "nsymbt": int(hd.nsymbt), "exttyp": bytes(hd.exttyp).decode("latin-1"),
                           "nversion": int(hd.nversion), "mapcrs": maps, "imod_stamp": stamp == 1146047817, "imod_flags": flags},
                "fei_first_block": fei}


def _dm_units_um(u):
    return _LENGTH_UM.get((u or "").strip())


_DM_ENERGY = ("eV", "keV", "meV")


def _dm_axes(dims, units, fmt, is_seq):
    """Our axis rule (docs/formats/dm.md, "Axes"): which stored dimension is X, Y, the spectral
    axis (channels), and which are stacked into Z or T. Returns (x, y, spectral, single, stack, stack_z)."""
    n = len(dims)
    f = (fmt or "").strip().lower()
    u = lambda k: (units[k] or "").strip() if k < len(units) else ""
    if f == "spectrum image":
        spec = {3: 2, 2: 0}.get(n)
    elif f == "spectrum":
        spec = 0 if n in (1, 2) else None
    elif n == 3 and u(2) in _DM_ENERGY:
        spec = 2
    elif n in (1, 2) and u(0) in _DM_ENERGY:
        spec = 0
    else:
        spec = None
    rest = [k for k in range(n) if k != spec]
    if spec is not None and all(dims[k] == 1 for k in rest):
        return spec, (rest[0] if rest else None), None, spec, rest[1:], False
    stack_z = spec is None and not is_seq and n == 3 and _dm_units_um(u(2)) is not None
    return (rest[0] if rest else None), (rest[1] if len(rest) > 1 else None), spec, None, rest[2:], stack_z


def _dm_image(i, k, name, dims, cal, fmt, is_seq, code, nd, bunit):
    """One exposed DM image (and its trace when it is a single spectrum) from the stored array
    `nd` (NumPy order: stored dimensions reversed, plus a trailing colour axis for RGB)."""
    units = [c[2] for c in cal]
    sdims = list(dims)
    if code in (27, 28) and sdims:
        sdims[0] = sdims[0] // 2 + 1
    x, y, spec, single, stack, stack_z = _dm_axes(dims, units, fmt, is_seq)
    size = lambda a: sdims[a] if a is not None else 1
    X, Y, C = size(x), size(y), size(spec)
    S = int(np.prod([sdims[a] for a in stack])) if stack else 1
    Z, T = (S, 1) if stack_z else (1, S)
    def um(j):
        if j is None or single is not None or j >= len(cal) or _dm_units_um(cal[j][2]) is None or cal[j][0] <= 0:
            return None
        return cal[j][0] * _dm_units_um(cal[j][2])
    img = {"index": i, "image_list_index": k, "name": name,
           "size_x": X, "size_y": Y, "size_z": Z, "size_c": C, "size_t": T, "data_type": code,
           "physical_size_um": {"x": um(x), "y": um(y), "z": um(stack[0]) if stack_z else None},
           "planes": []}
    if nd is None:
        img["skip"] = f"DataType {code} not decoded"
        img["pixel_type"] = None
        return img, None
    img["pixel_type"] = "uint8" if code == 23 else dtype_name(nd.dtype.newbyteorder("<"))
    n = len(sdims)
    ax = lambda a: n - 1 - a  # NumPy axis of stored dimension a
    def get(c, z, t):
        idx = [0] * n
        s_ = z if stack_z else t
        for a in stack:
            idx[ax(a)] = s_ % sdims[a]; s_ //= sdims[a]
        if spec is not None:
            idx[ax(spec)] = c
        keep = [a for a in (y, x) if a is not None]
        for a in keep:
            idx[ax(a)] = slice(None)
        plane = nd[tuple(idx)]
        if len(keep) == 2 and ax(y) > ax(x):  # Y must be the slower (outer) axis of the plane
            plane = np.swapaxes(plane, 0, 1)
        return np.ascontiguousarray(plane)
    img["planes"] = _planes_ct(get, C, Z, T)
    trace = None
    if single is not None:
        v = np.asarray(nd).reshape(-1)
        v = np.abs(v).astype("<f8") if v.dtype.kind == "c" else v.astype("<f8")
        sc, org, _ = cal[single] if single < len(cal) else (None, 0.0, "")
        trace = {"name": name, "sweep_count": 1, "channel_count": 1, "channel_names": ["intensity"],
                 "channel_units": [bunit] if bunit else [],
                 "ppm_first": (0 - org) * sc if sc else None,
                 "ppm_last": (dims[single] - 1 - org) * sc if sc else None,
                 "sweeps": [{"sweep": 0, "sample_count": int(v.size),
                             "channels": [{"xxh3": _trace_hash(v), "first": _first(v)}]}]}
    return img, trace


_DM_NUMPY = {1: "i2", 2: "f4", 3: "c8", 6: "u1", 7: "i4", 9: "i1", 10: "u2", 11: "u4", 12: "f8", 13: "c16", 14: "u1",
             23: "u1", 27: "c8", 28: "c16", 35: "i8", 36: "u8"}


def dm(p: Path) -> dict:
    """DM3/DM4 read with pyDM3reader's dm3_lib (MIT): its tag dictionary gives each ImageList entry's
    DataType, Dimensions, calibrations and the byte offset/size of ImageData.Data; the pixels are then
    read with NumPy from that offset (complex types as NumPy complex64/128, RGBA as bytes R, G, B, A with
    alpha dropped, DataType 27/28 as the stored (X/2 + 1)-column half plane). Thumbnails
    (Thumbnails.*.ImageIndex) are not images. The axis rule is ours (docs/formats/dm.md, `_dm_axes`);
    calibrated values are (index - Origin) x Scale. Single spectra are also traces (raw values as
    float64; `ppm_first`/`ppm_last` hold the spectral axis ends, compared with `extra.axis`).
    DM5 files (HDF5) go to `dm5`."""
    if p.suffix.lower() == ".dm5":
        return dm5(p)
    import dm3_lib._dm3_lib as L
    obj = L.DM3.__new__(L.DM3)
    note = None
    try:
        L.DM3.__init__(obj, str(p))
    except Exception as e:  # 1-D data: the tags are parsed before the constructor gives up
        note = f"dm3_lib constructor: {type(e).__name__}: {e}"
    tags = obj._tagDict
    if not tags:
        raise RuntimeError(note or "dm3_lib parsed no tags")
    head = open(p, "rb").read(16)
    ver = int.from_bytes(head[0:4], "big")
    little = int.from_bytes(head[8:12] if ver == 3 else head[12:16], "big") == 1  # header byte-order word
    thumbs = {int(v) for k, v in tags.items() if k.startswith("root.Thumbnails.") and k.endswith(".ImageIndex")}
    ks = sorted({int(k.split(".")[2]) for k in tags if k.startswith("root.ImageList.") and k.split(".")[2].isdigit()})
    exposed = [k for k in ks if k not in thumbs] or ks
    images, traces = [], []
    raw = open(p, "rb")
    for i, k in enumerate(exposed):
        pre = f"root.ImageList.{k}.ImageData"
        dims = []
        while f"{pre}.Dimensions.{len(dims)}" in tags:
            dims.append(max(1, int(tags[f"{pre}.Dimensions.{len(dims)}"])))
        cal = []
        while f"{pre}.Calibrations.Dimension.{len(cal)}.Scale" in tags:
            j = len(cal)
            cal.append((float(tags[f"{pre}.Calibrations.Dimension.{j}.Scale"]),
                        float(tags.get(f"{pre}.Calibrations.Dimension.{j}.Origin", 0.0)),
                        tags.get(f"{pre}.Calibrations.Dimension.{j}.Units", "")))
        meta = f"root.ImageList.{k}.ImageTags.Meta Data."
        fmt = tags.get(meta + "Format")
        is_seq = str(tags.get(meta + "IsSequence", "")).strip().lower() in ("1", "true")
        code = int(tags[f"{pre}.DataType"])
        nd = None
        if code in _DM_NUMPY:
            dt = _DM_NUMPY[code]
            npdt = np.dtype(dt) if dt in ("u1", "i1") else np.dtype(("<" if little else ">") + dt)
            raw.seek(int(tags[f"{pre}.Data.Offset"]))
            arr = np.frombuffer(raw.read(int(tags[f"{pre}.Data.Size"])), dtype=npdt)
            sdims = list(dims)
            if code in (27, 28):
                sdims[0] = sdims[0] // 2 + 1
            if code == 23:
                nd = arr.reshape(-1, 4)[:, :3].reshape(tuple(sdims[::-1]) + (3,))
            else:
                if code == 14:
                    arr = (arr != 0).astype(np.uint8)
                nd = arr.reshape(tuple(sdims[::-1]))
        img, trace = _dm_image(i, k, tags.get(f"root.ImageList.{k}.Name"), dims, cal, fmt, is_seq, code, nd,
                               tags.get(f"{pre}.Calibrations.Brightness.Units") or None)
        images.append(img)
        if trace:
            traces.append(dict(index=len(traces), **trace))
    raw.close()
    out = {"reader": "dm3_lib (pyDM3reader, MIT) tag dictionary + NumPy", "images": images,
           "file_version": getattr(obj, "_fileVersion", None), "thumbnails": sorted(thumbs)}
    if traces:
        out["traces"] = traces
    if note:
        out["notes"] = [note]
    return out


def dm5(p: Path) -> dict:
    """DM5 (HDF5) read with h5py (BSD-3): `ImageList/[k]/ImageData` attributes DataType, the
    `Dimensions` attributes `[0]`, `[1]`, ..., `Calibrations/Dimension/[j]` Scale/Origin/Units, the
    `Data` dataset (its NumPy shape is the stored dimensions reversed; an RGBA thumbnail may be an
    HDF5 RGB image with a trailing axis of 3), `ImageTags/Meta Data` Format/IsSequence; thumbnails
    from `Thumbnails/[i]` ImageIndex. Then the same axis rule as `dm`."""
    import h5py
    txt = lambda v: v.decode("utf-8", "replace") if isinstance(v, bytes) else (str(v) if v is not None else None)
    images, traces = [], []
    with h5py.File(p, "r") as f:
        thumbs = {int(g.attrs["ImageIndex"]) for g in f["Thumbnails"].values()} if "Thumbnails" in f else set()
        ks = sorted(int(n.strip("[]")) for n in f["ImageList"].keys())
        exposed = [k for k in ks if k not in thumbs] or ks
        for i, k in enumerate(exposed):
            e = f[f"ImageList/[{k}]"]
            d = e["ImageData"]
            code = int(d.attrs["DataType"])
            dims = [max(1, int(d["Dimensions"].attrs[f"[{j}]"])) for j in range(len(d["Dimensions"].attrs))]
            cal = []
            if "Calibrations/Dimension" in d:
                g = d["Calibrations/Dimension"]
                for j in range(len(g)):
                    a = g[f"[{j}]"].attrs
                    cal.append((float(a.get("Scale", 1.0)), float(a.get("Origin", 0.0)), txt(a.get("Units", b""))))
            md = e["ImageTags/Meta Data"].attrs if "ImageTags/Meta Data" in e else {}
            fmt = txt(md.get("Format")) if md else None
            is_seq = bool(md.get("IsSequence", 0)) if md else False
            bunit = txt(d["Calibrations/Brightness"].attrs.get("Units", b"")) if "Calibrations/Brightness" in d else None
            nd = None
            if code in _DM_NUMPY:
                arr = d["Data"][()]
                if code == 23 and arr.dtype == np.uint8 and arr.shape[-1] in (3, 4):
                    nd = arr[..., :3]
                elif code == 14:
                    nd = (arr != 0).astype(np.uint8)
                else:
                    nd = arr.astype(arr.dtype.newbyteorder("<")) if arr.dtype.names is None else arr.view(np.dtype("<" + _DM_NUMPY[code]))
            img, trace = _dm_image(i, k, txt(e.attrs.get("Name")), dims, cal, fmt, is_seq, code, nd, bunit or None)
            images.append(img)
            if trace:
                traces.append(dict(index=len(traces), **trace))
    out = {"reader": f"h5py {h5py.__version__} (DM5 groups and attributes)", "images": images, "thumbnails": sorted(thumbs)}
    if traces:
        out["traces"] = traces
    return out


def _ser_dimensions(p: Path):
    """Scan dimensions from the .ser header (sizes and units; docs/formats/ser.md): header metadata
    only, the element data is decoded by ncempy."""
    import struct
    b = open(p, "rb").read(1 << 16)
    ver = struct.unpack_from("<H", b, 4)[0]
    w = 8 if ver >= 0x220 else 4
    ndim = struct.unpack_from("<I", b, 22 + w)[0]
    pos, dims = 26 + w, []
    for _ in range(ndim):
        size, coff, cdelta, celem, dlen = struct.unpack_from("<IddiI", b, pos)
        pos += 28 + dlen
        ulen = struct.unpack_from("<I", b, pos)[0]
        units = b[pos + 4:pos + 4 + ulen].decode("latin-1")
        pos += 4 + ulen
        dims.append({"size": size, "delta": cdelta, "units": units})
    return dims


def ser(p: Path) -> dict:
    """TIA series files read with ncempy.io.ser (openNCEM, GPL-3.0-or-later; run as a black box).
    One image per .ser. 2-D elements are T (element order). 1-D elements (spectra) that fill the
    header's scan dimensions (1 or 2 of them, every element valid) are an image of the scan with
    X = dimension 0, Y = dimension 1 and one channel per bin; other 1-D series are one row per
    element. Every 1-D series is also a trace with one sweep per element, raw values as float64,
    energy axis offset + (i - element) x delta (compared through `ppm_first`/`ppm_last`).
    Pixel size: 2-D element calibration delta taken as metres when 0 < delta < 1e-3; scan
    dimensions in metres when their unit is `meters` and their step below 1 mm; in µm."""
    import ncempy.io.ser as S, shutil, tempfile
    try:
        f = S.fileSER(str(p))
    except TypeError:
        # ncempy fails on some .emi sidecars (no AcquireDate): read the .ser alone
        tmp = Path(tempfile.mkdtemp())
        shutil.copy(p, tmp / p.name)
        f = S.fileSER(str(tmp / p.name))
    head = f.head
    valid = int(head["ValidNumberElements"])
    total = int(head["TotalNumberElements"])
    def um(c):
        d = float(c["CalibrationDelta"])
        return d * 1e6 if 0 < d < 1e-3 else None
    out = {"reader": "ncempy.io.ser (GPL, black box)", "series_version": int(head["SeriesVersion"])}
    if int(head["DataTypeID"]) == 0x4122:
        d0, m0 = f.getDataset(0)
        ny, nx = d0.shape
        img = {"index": 0, "size_x": int(nx), "size_y": int(ny), "size_z": 1, "size_c": 1, "size_t": valid,
               "pixel_type": dtype_name(d0.dtype),
               "physical_size_um": {"x": um(m0["Calibration"][0]), "y": um(m0["Calibration"][1]), "z": None},
               "planes": _planes_ct(lambda c, z, t: f.getDataset(t)[0], 1, 1, valid)}
    else:
        specs = [np.asarray(f.getDataset(i)[0]).reshape(-1) for i in range(valid)]
        m0 = f.getDataset(0)[1]
        cal = m0["Calibration"][0] if isinstance(m0["Calibration"], (list, tuple)) else m0["Calibration"]
        n = specs[0].size
        dims = _ser_dimensions(p)
        prod = int(np.prod([d["size"] for d in dims])) if dims else 0
        scan = 1 <= len(dims) <= 2 and valid > 1 and valid == total and prod == total
        # TIA writes a step of exactly 1 m on uncalibrated positions (point spectra): a step of
        # 1 mm or more is not a pixel size, as for element deltas
        mum = lambda d: abs(d["delta"]) * 1e6 if d["units"].strip().lower() in ("meters", "m") and 0 < abs(d["delta"]) < 1e-3 else None
        if scan:
            W = dims[0]["size"]; H = dims[1]["size"] if len(dims) > 1 else 1
            cube = np.stack(specs).reshape(H, W, n)
            img = {"index": 0, "size_x": W, "size_y": H, "size_z": 1, "size_c": n, "size_t": 1,
                   "pixel_type": dtype_name(cube.dtype),
                   "physical_size_um": {"x": mum(dims[0]), "y": mum(dims[1]) if H > 1 else None, "z": None},
                   "planes": _planes_ct(lambda c, z, t: np.ascontiguousarray(cube[:, :, c]), n, 1, 1)}
        else:
            rows = np.stack(specs)
            img = {"index": 0, "size_x": n, "size_y": valid, "size_z": 1, "size_c": 1, "size_t": 1,
                   "pixel_type": dtype_name(rows.dtype), "physical_size_um": {"x": None, "y": None, "z": None},
                   "planes": _planes_ct(lambda c, z, t: rows, 1, 1, 1)}
        off, delta, el = float(cal["CalibrationOffset"]), float(cal["CalibrationDelta"]), int(cal["CalibrationElement"])
        sweeps = []
        for i, v in enumerate(specs[:MAX_SWEEPS]):
            v = v.astype("<f8")
            sweeps.append({"sweep": i, "sample_count": int(v.size), "channels": [{"xxh3": _trace_hash(v), "first": _first(v)}]})
        out["traces"] = [{"index": 0, "name": p.stem, "sweep_count": valid, "channel_count": 1, "channel_names": ["counts"],
                          "ppm_first": off + (0 - el) * delta, "ppm_last": off + (n - 1 - el) * delta, "sweeps": sweeps}]
    out["images"] = [img]
    emi = getattr(f, "_emi", None)
    out["emi"] = {k: _jsonable(emi[k]) for k in ("AcceleratingVoltage", "Microscope []", "Magnification [x]") if emi and k in emi} or None
    return out


_EMD_LEN_UM = {"m": 1e6, "mm": 1e3, "um": 1.0, "µm": 1.0, "μm": 1.0, "u_m": 1.0, "micrometer": 1.0, "nm": 1e-3, "n_m": 1e-3,
               "nanometer": 1e-3, "A": 1e-4, "Å": 1e-4, "angstrom": 1e-4, "Angstrom": 1e-4, "pm": 1e-6, "p_m": 1e-6}


def emd_berkeley(p: Path) -> dict:
    """Berkeley EMD (0.2 / 1.0; open convention, emdatasets.com) read with h5py (BSD-3): every group
    whose `emd_group_type` is 1 or "array", in path order; its `data` dataset (else the one dataset
    that is not a `dimK` vector); `dimK` vectors in K order calibrate the dimensions (step =
    v[1] - v[0], units without brackets; a step of 1 mm or more per pixel is not a pixel size). Our axis rule (docs/formats/emd.md): X = last dimension,
    Y = the one before, leading dimensions flattened into T (Z for a 3-D array whose first
    calibration is a length)."""
    import h5py, re as _re
    txt = lambda v: (v.decode("utf-8", "replace") if isinstance(v, bytes) else str(v)).strip("\0 ").strip()
    groups = []
    with h5py.File(p, "r") as f:
        def visit(name, obj):
            if isinstance(obj, h5py.Group) and "emd_group_type" in obj.attrs:
                t = obj.attrs["emd_group_type"]
                t = txt(t) if isinstance(t, (bytes, str)) else str(int(t))
                if t in ("1", "array"):
                    groups.append(name)
        f.visititems(visit)
        images = []
        for gname in sorted(groups):
            g = f[gname]
            names = [k for k, v in g.items() if isinstance(v, h5py.Dataset)]
            dims = sorted((int(m.group(1)), k) for k in names for m in [_re.fullmatch(r"dim(\d+)", k)] if m)
            if "data" in names:
                dn = "data"
            else:
                c = [k for k in names if not k.startswith("dim")]
                if len(c) != 1:
                    continue
                dn = c[0]
            d = g[dn]
            shape = d.shape
            n = len(shape)
            X = shape[-1] if n >= 1 else 1
            Y = shape[-2] if n >= 2 else 1
            lead = int(np.prod(shape[:-2])) if n > 2 else 1
            cal = []
            for i, (_, k) in enumerate(dims):
                raw = np.asarray(g[k][()])
                v = raw.astype(np.float64).reshape(-1) if raw.dtype.kind in "iuf" else np.zeros(0)
                u = g[k].attrs.get("units")
                u = txt(u).strip("[]").strip() if u is not None else ""
                step = float(v[1] - v[0]) if v.size >= 2 and (v.size == shape[i] or v.size == 2) else None
                cal.append((step, u))
            def um(i):
                if i < 0 or i >= len(cal) or cal[i][0] is None or cal[i][1] not in _EMD_LEN_UM:
                    return None
                v = abs(cal[i][0]) * _EMD_LEN_UM[cal[i][1]]
                # a step of 1 mm or more per pixel is not a length (a TIA diffraction pattern
                # converted by ncempy labels its reciprocal-metre steps `m`)
                return v if 0 < v < 1e3 else None
            is_len = lambda i: i < len(cal) and cal[i][1] in _EMD_LEN_UM
            chan = n == 3 and is_len(0) and is_len(1) and not is_len(2)  # (Y, X, detector bins)
            lead_z = n == 3 and not chan and is_len(0)
            Z, T = (lead, 1) if lead_z else (1, lead)
            C = 1
            if chan:
                X, Y, C, Z, T = shape[1], shape[0], shape[2], 1, 1
            kind = d.dtype.kind
            if d.dtype.names and len(d.dtype.names) == 2:
                ptype = {4: "complex", 8: "double-complex"}.get(d.dtype[0].itemsize)
            else:
                ptype = dtype_name(d.dtype) if kind in "iuf" else None
            img = {"index": len(images), "emd_group": gname, "size_x": int(X), "size_y": int(Y), "size_z": int(Z),
                   "size_c": int(C), "size_t": int(T), "pixel_type": ptype,
                   "physical_size_um": {"x": um(1 if chan else n - 1), "y": um(0 if chan else n - 2), "z": um(0) if lead_z else None}, "planes": []}
            if ptype is None:
                img["skip"] = f"HDF5 type {d.dtype} not read"
            else:
                arr = d[()]
                if d.dtype.names:
                    arr = arr.view(np.dtype("<c8" if ptype == "complex" else "<c16"))
                if chan:
                    a3 = np.asarray(arr)
                    img["planes"] = _planes_ct(lambda c, z, t: np.ascontiguousarray(a3[:, :, c]), C, 1, 1)
                else:
                    flat = np.asarray(arr).reshape(lead, Y, X)
                    img["planes"] = _planes_ct(lambda c, z, t: flat[z if lead_z else t], 1, Z, T)
            images.append(img)
        ver = (f.attrs.get("version_major"), f.attrs.get("version_minor"))
    return {"reader": f"h5py {h5py.__version__} (Berkeley EMD groups)", "images": images,
            "version": None if ver[0] is None else f"{txt(ver[0]) if isinstance(ver[0], (bytes, str)) else int(ver[0])}.{txt(ver[1]) if isinstance(ver[1], (bytes, str)) else int(ver[1])}"}


def emd(p: Path) -> dict:
    """Velox EMD read with h5py (BSD-3): every Data/Image/<id>/Data (rows, columns, frames), frames as T,
    in group-name order; physical size from the Metadata JSON (BinaryResult.PixelSize, metres → µm).
    Files without Velox data but with EMD data groups go to `emd_berkeley`."""
    import h5py, json as _json
    with h5py.File(p, "r") as f:
        velox = any(k in f for k in ("Data/Image", "Data/Spectrum", "Data/SpectrumStream", "Data/EelsSpectrumImage"))
    if not velox:
        return emd_berkeley(p)
    images = []
    with h5py.File(p, "r") as f:
        ver = f["Version"][0] if "Version" in f else None
        for i, k in enumerate(sorted(f["Data/Image"].keys()) if "Data/Image" in f else []):
            d = f[f"Data/Image/{k}/Data"]
            r, c = d.shape[:2]
            n = d.shape[2] if d.ndim == 3 else 1
            md = None
            if f"Data/Image/{k}/Metadata" in f:
                raw = f[f"Data/Image/{k}/Metadata"][:, 0]
                md = _json.loads(bytes(raw).split(b"\0")[0].decode())
            def um(key):
                try:
                    v = float(md["BinaryResult"]["PixelSize"][key])
                    unit = md["BinaryResult"]["PixelUnitX" if key == "width" else "PixelUnitY"]
                    return v * {"m": 1e6, "nm": 1e-3, "um": 1.0, "µm": 1.0}[unit] if v > 0 else None
                except Exception:
                    return None
            arr = d[()] if d.ndim == 2 else None
            get = (lambda c_, z, t: d[()]) if d.ndim == 2 else (lambda c_, z, t: d[:, :, t])
            if d.dtype.names and len(d.dtype.names) == 2:  # complex Fourier transform (real, imaginary)
                cdt = np.dtype("<c8" if d.dtype[0].itemsize == 4 else "<c16")
                raw_get = get
                get = lambda c_, z, t: np.ascontiguousarray(raw_get(c_, z, t)).view(cdt)
                ptype = "complex" if cdt.itemsize == 8 else "double-complex"
            else:
                ptype = dtype_name(d.dtype)
            images.append({"index": i, "velox_id": k, "size_x": int(c), "size_y": int(r), "size_z": 1, "size_c": 1,
                           "size_t": int(n), "pixel_type": ptype,
                           "physical_size_um": {"x": um("width"), "y": um("height"), "z": None},
                           "detector": (md or {}).get("BinaryResult", {}).get("Detector"),
                           "planes": _planes_ct(get, 1, 1, int(n))})
        # EDS detector spectra: one trace each (Data/Spectrum/<id>/Data, (channels, 1))
        traces = []
        for i, k in enumerate(sorted(f["Data/Spectrum"].keys()) if "Data/Spectrum" in f else []):
            v = f[f"Data/Spectrum/{k}/Data"][()].reshape(-1).astype("<f8")
            traces.append({"index": i, "sweep_count": 1, "channel_count": 1, "channel_names": ["counts"],
                           "channel_units": ["counts"],
                           "sweeps": [{"sweep": 0, "sample_count": int(v.size),
                                       "channels": [{"xxh3": _trace_hash(v), "first": [float(x) for x in v[:8]]}]}]})
        # EDS spectrum images from the event streams (docs/formats/emd.md: 65535 ends a pixel,
        # other values are energy channels of events; raster order, frames summed), streams with
        # the same raster summed. Implemented here with numpy from our format notes: the check
        # that it is right is that each stream's histogram equals a stored detector spectrum.
        streams = []
        for k in sorted(f["Data/SpectrumStream"].keys()) if "Data/SpectrumStream" in f else []:
            st = _json.loads(f[f"Data/SpectrumStream/{k}/AcquisitionSettings"][0])
            ras = st.get("RasterScanDefinition")
            if ras:
                streams.append((k, int(ras["Width"]), int(ras["Height"]), int(st["bincount"])))
        if streams:
            w, h_, bins = streams[0][1], streams[0][2], streams[0][3]
            npx = w * h_
            chans, pix = [], []
            stored = [f[f"Data/Spectrum/{k}/Data"][()].reshape(-1) for k in f["Data/Spectrum"]] if "Data/Spectrum" in f else []
            matched = 0
            for k, *_ in streams:
                d = f[f"Data/SpectrumStream/{k}/Data"][()].reshape(-1)
                m = d == 65535
                before = np.cumsum(m) - m  # markers before each value: the pixel an event belongs to
                ev = ~m
                chans.append(d[ev].astype(np.int64))
                pix.append((before[ev] % npx).astype(np.int64))
                hist = np.bincount(d[ev], minlength=bins)[:bins]
                matched += any(np.array_equal(hist, s[:bins]) for s in stored)
            ch = np.concatenate(chans); px = np.concatenate(pix)
            totals = np.bincount(ch, minlength=bins)
            picks = sorted(set(np.argsort(totals)[-12:].tolist()) | {0, bins // 2, bins - 1})
            planes = []
            for c in picks:
                a = np.bincount(px[ch == c], minlength=npx).astype("<u4").reshape(h_, w)
                planes.append({"c": int(c), "z": 0, "t": 0, "xxh3": h(a)})
            images.append({"index": len(images), "name": "EDS spectrum image", "size_x": w, "size_y": h_, "size_z": 1,
                           "size_c": bins, "size_t": 1, "pixel_type": "uint32", "planes": planes,
                           "streams_matching_stored_spectra": f"{matched}/{len(streams)}"})
        # STEM-EELS spectrum images (Data/EelsSpectrumImage/<id>/Data, (columns, channels, rows);
        # docs/formats/emd.md). The axis order is checked here, not assumed: the energy-summed map,
        # read as (rows, columns), must correlate with an image of the same raster more strongly
        # than its transpose does (recorded in `axis_check`).
        imgs = [(k, f[f"Data/Image/{k}/Data"]) for k in (sorted(f["Data/Image"].keys()) if "Data/Image" in f else [])]
        for k in sorted(f["Data/EelsSpectrumImage"].keys()) if "Data/EelsSpectrumImage" in f else []:
            d = f[f"Data/EelsSpectrumImage/{k}/Data"]
            cols, bins, rows = d.shape
            a = d[()]
            tot = a.astype(np.float64).sum(axis=1).T  # (rows, columns)
            best = None
            for ik, im in imgs:
                if im.shape[0] == rows and im.shape[1] == cols:
                    ref = im[:, :, 0] if im.ndim == 3 else im[()]
                    r_ok = abs(np.corrcoef(tot.ravel(), ref.astype(np.float64).ravel())[0, 1])
                    r_t = abs(np.corrcoef(tot.T.ravel(), ref.astype(np.float64).ravel())[0, 1]) if rows == cols else 0.0
                    if best is None or r_ok > best[1]:
                        best = (ik, round(float(r_ok), 4), round(float(r_t), 4))
            am = f[f"Data/EelsSpectrumImage/{k}/AcquisitionMetadata"]
            docs = [_json.loads(bytes(am[:, j]).split(b"\0")[0]) for j in range(am.shape[1])]
            cal = {(x["Data"]["offset"], x["Data"]["dispersion"]) for x in docs}
            totals = a.astype(np.float64).sum(axis=(0, 2))
            picks = sorted(set(np.argsort(totals)[-12:].tolist()) | {0, bins // 2, bins - 1})
            planes = [{"c": int(c), "z": 0, "t": 0, "xxh3": h(np.ascontiguousarray(a[:, c, :].T))} for c in picks]
            first, step = next(iter(cal)) if len(cal) == 1 else (None, None)
            images.append({"index": len(images), "name": "EELS spectrum image", "size_x": int(cols), "size_y": int(rows),
                           "size_z": 1, "size_c": int(bins), "size_t": 1, "pixel_type": dtype_name(d.dtype),
                           "energy_axis": {"first": first, "step": step, "size": int(bins)}, "planes": planes,
                           "axis_check": {"image": best[0], "r_rows_columns": best[1], "r_transposed": best[2]} if best else None})
    return {"reader": f"h5py {h5py.__version__}", "images": images, "traces": traces,
            "version": ver.decode() if isinstance(ver, bytes) else ver}

# ---------------------------------------------------------------- OME-Zarr (NGFF)
# zarr-python (MIT) decodes every array; the NGFF metadata (open specification,
# https://ngff.openmicroscopy.org) is interpreted here, independently of openreadout: images are
# the root multiscales, or the fields of an HCS plate (wells in `plate.wells` order, fields in
# `well.images` order), or the series of a bioformats2raw.layout collection (`OME` group
# `series`, else 0, 1, ... while present). Axes map by name to t/c/z/y/x; physical sizes come from
# the first dataset's scale (times any multiscales-level scale) converted to micrometres.

_NGFF_LEN_UM = {"angstrom": 1e-4, "picometer": 1e-6, "nanometer": 1e-3, "micrometer": 1.0, "millimeter": 1e3,
                "centimeter": 1e4, "meter": 1e6}
_NGFF_TIME_S = {"nanosecond": 1e-9, "microsecond": 1e-6, "millisecond": 1e-3, "second": 1.0, "minute": 60.0, "hour": 3600.0}


def _zarr_open_root(p: Path):
    """(zarr root group, store) for a directory store or a zip store (root at the archive root or
    inside one top-level directory, as Zenodo zips of `.zarr` folders are laid out)."""
    import zarr, zipfile
    if p.is_dir():
        store = zarr.storage.LocalStore(str(p), read_only=True)
        return zarr.open_group(store, mode="r"), store
    names = zipfile.ZipFile(p).namelist()
    top = ""
    if not any(n in ("zarr.json", ".zgroup", ".zattrs") for n in names):
        tops = {n.split("/", 1)[0] for n in names if "/" in n} - {"__MACOSX"}  # macOS archive metadata
        if len(tops) == 1:
            top = tops.pop()
    store = zarr.storage.ZipStore(str(p), mode="r")
    return zarr.open_group(store, mode="r", path=top or None), store


def _ngff_attrs(g) -> dict:
    a = dict(g.attrs)
    return dict(a.get("ome", a))


def _ngff_image(g, index, name):
    import zarr
    ms = _ngff_attrs(g)["multiscales"][0]
    axes = [a if isinstance(a, str) else a["name"] for a in ms.get("axes", ["t", "c", "z", "y", "x"])]
    units = {(a["name"]): a.get("unit") for a in ms.get("axes", []) if isinstance(a, dict)}
    ds = ms["datasets"]
    arrs = [g[d["path"]] for d in ds]
    shape = dict(zip(axes, arrs[0].shape))
    scale = [1.0] * len(axes)
    for t in ds[0].get("coordinateTransformations", []) + ms.get("coordinateTransformations", []):
        if t.get("type") == "scale":
            scale = [a * b for a, b in zip(scale, t["scale"])]
    sc = dict(zip(axes, scale))
    def um(ax):
        u = units.get(ax)
        f = _NGFF_LEN_UM.get(u) if u else None
        return sc[ax] * f if (ax in sc and f) else None
    tu = units.get("t")
    X, Y = shape["x"], shape["y"]
    C, Z, T = shape.get("c", 1), shape.get("z", 1), shape.get("t", 1)
    # our conventions for NumPy types we return differently: bool as uint8 0/1, float16 widened
    out_dt = {np.dtype(bool): np.dtype(np.uint8), np.dtype(np.float16): np.dtype(np.float32)}.get(np.dtype(arrs[0].dtype), np.dtype(arrs[0].dtype))
    def getter(arr):
        def get(c, z, t):
            idx = tuple({"c": c, "z": z, "t": t}.get(a, slice(None)) if a in "czt" else (slice(None) if a in "yx" else 0) for a in axes)
            return np.asarray(arr[idx]).astype(out_dt.newbyteorder("<"))
        return get
    omero = _ngff_attrs(g).get("omero") or {}
    img = {"index": index, "name": name, "size_x": X, "size_y": Y, "size_z": Z, "size_c": C, "size_t": T,
           "pixel_type": dtype_name(out_dt), "axes": axes,
           "physical_size_um": {"x": um("x"), "y": um("y"), "z": um("z") if Z > 1 else None},
           "time_increment_s": sc["t"] * _NGFF_TIME_S[tu] if tu in _NGFF_TIME_S and "t" in sc else None,
           "channel_names": [c.get("label") for c in omero.get("channels", [])],
           "level_sizes": [[int(dict(zip(axes, a.shape))["x"]), int(dict(zip(axes, a.shape))["y"])] for a in arrs],
           "planes": _planes_ct(getter(arrs[0]), C, Z, T), "levels": []}
    for lvl, a in enumerate(arrs[1:], start=1):
        s = dict(zip(axes, a.shape))
        img["levels"].append({"level": lvl, "size_x": int(s["x"]), "size_y": int(s["y"]),
                              "planes": _planes_ct(getter(a), 1, 1, 1)})
    return img


def ome_zarr_(p: Path) -> dict:
    import zarr, zipfile
    root, store = _zarr_open_root(p)
    ra = _ngff_attrs(root)
    images, kind = [], "image"
    if "multiscales" in ra:
        images.append(_ngff_image(root, 0, ra["multiscales"][0].get("name")))
    elif "plate" in ra:
        kind = "plate"
        for w in ra["plate"]["wells"]:
            wg = root[w["path"]]
            for f in _ngff_attrs(wg)["well"]["images"]:
                images.append(_ngff_image(wg[f["path"]], len(images), f"{w['path']}/{f['path']}"))
    elif "bioformats2raw.layout" in ra:
        kind = "bioformats2raw"
        series = None
        if "OME" in root:
            series = _ngff_attrs(root["OME"]).get("series")
        if series is None:
            series, k = [], 0
            while str(k) in root:
                series.append(str(k)); k += 1
        for s in series:
            images.append(_ngff_image(root[s], len(images), s))
    labels = []
    if "labels" in root:
        labels = list(_ngff_attrs(root["labels"]).get("labels", []))
    # label images (NGFF image-label) follow the images: each image group's labels in image
    # order, then the root labels (of the single root image, or of the whole store)
    parents = []
    if "multiscales" in ra:
        parents.append((root, 0, ""))
    else:
        for i, im in enumerate(list(images)):
            grp = root[im["name"]] if im.get("name") and im["name"] in root else None
            if grp is not None and "labels" in grp:
                parents.append((grp, i, im["name"]))
        if "labels" in root:
            parents.append((root, None, ""))
    for grp, of, gname in parents:
        if "labels" not in grp:
            continue
        for lab in dict.fromkeys(_ngff_attrs(grp["labels"]).get("labels", [])):  # a name listed twice is one label
            lg = grp["labels"][lab]
            path = "/".join(x for x in (gname, "labels", lab) if x)
            li = _ngff_image(lg, len(images), path)
            li["label_of"] = of
            images.append(li)
    version = ra.get("version") or (ra.get("multiscales") or [{}])[0].get("version") or (ra.get("plate") or {}).get("version")
    return {"reader": f"zarr-python {zarr.__version__} (NGFF metadata interpreted in gen.py)", "kind": kind,
            "ngff_version": version, "zarr_format": root.metadata.zarr_format, "labels": labels, "images": images}


# ---------------------------------------------------------------- Imaris IMS / NWB (HDF5)

def _h5_text(v):
    """HDF5 attribute as text: Imaris stores strings as arrays of one-character strings."""
    if isinstance(v, bytes):
        return v.decode("latin-1")
    if isinstance(v, np.ndarray) and v.dtype.kind == "S":
        return b"".join(v.tolist()).decode("latin-1")
    if isinstance(v, np.ndarray) and v.size == 1:
        return str(v.reshape(-1)[0])
    return str(v)


def ims(p: Path) -> dict:
    """Imaris 5 (HDF5) via h5py (BSD-3) with hdf5plugin (for the LZ4 filter 32004): one image;
    each level/time point/channel `Data` volume cropped to the channel group's ImageSizeX/Y/Z;
    physical size = (ExtMax - ExtMin) / size in DataSetInfo/Image `Unit` (empty = µm). Bio-Formats
    `showinf -nopix` (GPL, black box) is recorded as a second opinion on the geometry."""
    import h5py, hdf5plugin  # noqa: F401  (registers the LZ4 filter)
    f = h5py.File(p, "r")
    num = lambda k, a: int(_h5_text(a[k]).strip("\0 ")) if k in a else None
    levels = sorted((int(k.split()[-1]) for k in f["DataSet"].keys() if k.startswith("ResolutionLevel")))
    l0 = f[f"DataSet/ResolutionLevel {levels[0]}"]
    T = len([k for k in l0.keys() if k.startswith("TimePoint")])
    C = len([k for k in l0["TimePoint 0"].keys() if k.startswith("Channel")])
    def size(l):
        a = f[f"DataSet/ResolutionLevel {l}/TimePoint 0/Channel 0"].attrs
        return num("ImageSizeX", a), num("ImageSizeY", a), num("ImageSizeZ", a)
    X, Y, Z = size(levels[0])
    img = f["DataSetInfo/Image"].attrs
    unit = _h5_text(img["Unit"]).strip("\0 ") if "Unit" in img else ""
    fac = {"": 1.0, "um": 1.0, "nm": 1e-3, "mm": 1e3}.get(unit)
    def ext(i, n):
        try:
            return (float(_h5_text(img[f"ExtMax{i}"])) - float(_h5_text(img[f"ExtMin{i}"]))) / n * fac
        except Exception:
            return None
    d0 = f[f"DataSet/ResolutionLevel {levels[0]}/TimePoint 0/Channel 0/Data"]
    def getter(l, sx, sy):
        def get(c, z, t):
            return f[f"DataSet/ResolutionLevel {l}/TimePoint {t}/Channel {c}/Data"][z, :sy, :sx]
        return get
    names = []
    for c in range(C):
        a = f[f"DataSetInfo/Channel {c}"].attrs if f"DataSetInfo/Channel {c}" in f else {}
        names.append(_h5_text(a["Name"]).strip("\0 ") if "Name" in a else None)
    image = {"index": 0, "size_x": X, "size_y": Y, "size_z": Z, "size_c": C, "size_t": T,
             "pixel_type": dtype_name(d0.dtype),
             "physical_size_um": {"x": ext(0, X), "y": ext(1, Y), "z": ext(2, Z) if Z > 1 else None},
             "channel_names": names, "filters": [d0.id.get_create_plist().get_filter(i)[0] for i in range(d0.id.get_create_plist().get_nfilters())],
             "planes": _planes_ct(getter(levels[0], X, Y), C, Z, T), "levels": []}
    for l in levels[1:]:
        sx, sy, sz = size(l)
        image["levels"].append({"level": l, "size_x": sx, "size_y": sy,
                                "planes": _planes_ct(getter(l, sx, sy), 1, min(sz, 2), 1)})
    out = {"reader": f"h5py {h5py.__version__} + hdf5plugin", "images": [image],
           "imaris_version": _h5_text(f.attrs.get("ImarisVersion", b"")).strip("\0 ")}
    tables = _ims_scene_tables(f)
    if tables:
        out["tables"] = tables
    try:
        out["bioformats"] = _bf_series(_showinf(p, "-nometa"))
    except Exception as e:
        out["bioformats"] = {"error": str(e)}
    return out


def _ims_scene_tables(f) -> list:
    """Scene8 objects as docs/formats/ims.md § Scene objects describes, read with h5py:
    per object (natural name order), one statistics table per category (rows = (time, object)
    pairs sorted; columns ID, Time, the object-attribute factors, then the statistics sorted by
    (name, factors, type id) and merged by name + splitting factors), then every 1-D compound
    dataset of numeric fields (text fields dropped) of the object group and its direct
    subgroups, sorted by path. Values hashed as column-major little-endian f64."""
    import collections
    import h5py
    if "Scene8/Content" not in f:
        return []

    def natural(s):
        m = re.match(r"(.*?)(\d*)$", s)
        return (m.group(1), int(m.group(2) or 0))

    def text(b):
        return b.split(b"\0")[0].decode("utf-8", "replace").rstrip() if isinstance(b, bytes) else str(b)

    def numeric_fields(ds):
        return [n for n in ds.dtype.names if ds.dtype[n].kind in "iuf"]

    def colhash(cols):
        a = np.concatenate([np.asarray(c, dtype="<f8") for c in cols]) if cols else np.zeros(0, "<f8")
        return h(a)

    out = []
    for obj in sorted(f["Scene8/Content"].keys(), key=natural):
        g = f[f"Scene8/Content/{obj}"]
        if not isinstance(g, h5py.Group):
            continue
        label = _h5_text(g.attrs["Name"]).strip("\0 ") if "Name" in g.attrs else obj
        if "StatisticsType" in g and "StatisticsValue" in g:
            cats = {int(r["ID"]): text(r["CategoryName"]) for r in g["Category"][:]} if "Category" in g else {}
            flist = collections.defaultdict(list)
            if "Factor" in g:
                for r in g["Factor"][:]:
                    flist[int(r["ID_List"])].append((text(r["Name"]), text(r["Level"])))
            types = {int(r["ID"]): (text(r["Name"]), text(r["Unit"]), int(r["ID_Category"]), flist.get(int(r["ID_FactorList"]), []))
                     for r in g["StatisticsType"][:]}
            vals = [(int(r["ID_Time"]), int(r["ID_Object"]), int(r["ID_StatisticsType"]), float(r["Value"])) for r in g["StatisticsValue"][:]]
            for cat in sorted({t[2] for t in types.values()}):
                ids = {k for k, t in types.items() if t[2] == cat}
                name_levels = collections.defaultdict(set)
                for k in ids:
                    for fa, lv in types[k][3]:
                        name_levels[(types[k][0], fa)].add(lv)
                obj_levels = collections.defaultdict(set)
                for tm, ob, ty, _ in vals:
                    if ty in ids:
                        for fa, lv in types[ty][3]:
                            obj_levels[(tm, ob, types[ty][0], fa)].add(lv)
                split = {k[3] for k, v in obj_levels.items() if len(v) > 1}
                allf = sorted({k[1] for k in name_levels})
                attrs = [fa for fa in allf if fa not in split and any(len(v) > 1 for (n, g2), v in name_levels.items() if g2 == fa)]
                attr_levels = {fa: sorted({lv for (n, g2), v in name_levels.items() if g2 == fa for lv in v}) for fa in attrs}
                order = sorted(ids, key=lambda k: (types[k][0], types[k][3], k))
                cols = {}
                for k in order:
                    qual = tuple((fa, lv) for fa, lv in types[k][3] if fa in split)
                    cols.setdefault((types[k][0], qual), (types[k][1], []))[1].append(k)
                rows = sorted({(tm, ob) for tm, ob, ty, _ in vals if ty in ids})
                if not rows:
                    continue
                ri = {r: i for i, r in enumerate(rows)}
                names = ["ID", "Time"] + attrs + [n if not q else f"{n} [{', '.join(f'{a}={b}' for a, b in q)}]" for (n, q) in cols]
                dtypes = ["int64", "int64"] + ["float64" if all(_isnum(l) for l in attr_levels[fa]) else "uint32" for fa in attrs] + ["float64"] * len(cols)
                data = [[float(ob) for tm, ob in rows], [float(tm) for tm, ob in rows]] + [[np.nan] * len(rows) for _ in attrs] + [[np.nan] * len(rows) for _ in cols]
                colof = {}
                for ci, (key, (unit, tids)) in enumerate(cols.items()):
                    for tid in tids:
                        colof[tid] = 2 + len(attrs) + ci
                for tm, ob, ty, v in vals:
                    if ty not in colof:
                        continue
                    r = ri[(tm, ob)]
                    data[colof[ty]][r] = v
                    for fa, lv in types[ty][3]:
                        if fa in attrs:
                            ai = 2 + attrs.index(fa)
                            data[ai][r] = float(lv) if dtypes[ai] == "float64" else float(attr_levels[fa].index(lv))
                out.append({"index": len(out), "name": f"{label}: {cats.get(cat, f'category {cat}')} statistics",
                            "event_count": len(rows), "parameter_names": names, "dtypes": dtypes, "xxh3": colhash(data)})
        paths = []
        for k in g.keys():
            if isinstance(g[k], h5py.Dataset):
                paths.append(k)
            elif isinstance(g[k], h5py.Group):
                paths += [f"{k}/{d}" for d in g[k].keys() if isinstance(g[k][d], h5py.Dataset)]
        for short in sorted(paths):
            leaf = short.rsplit("/", 1)[-1]
            if leaf in ("StatisticsType", "StatisticsValue", "StatisticsValueTimeOffset", "Factor", "FactorList", "Category", "CreationParameters") or leaf.startswith("Label"):
                continue
            ds = g[short]
            if ds.dtype.names is None or ds.ndim != 1 or ds.shape[0] == 0:
                continue
            fields = numeric_fields(ds)
            if not fields:
                continue
            arr = ds[:]
            data = [arr[n].astype(np.float64) for n in fields]
            out.append({"index": len(out), "name": f"{label}: {short}", "event_count": int(ds.shape[0]),
                        "parameter_names": fields, "dtypes": ["float64"] * len(fields), "xxh3": colhash(data)})
    return out


def _isnum(s: str) -> bool:
    try:
        float(s)
        return True
    except ValueError:
        return False


NWB_TRACE_TYPES = ("TimeSeries", "ElectricalSeries", "SpatialSeries")
# intracellular (icephys) series, also read under /stimulus/presentation/
NWB_ICEPHYS_TYPES = ("PatchClampSeries", "CurrentClampSeries", "IZeroClampSeries", "CurrentClampStimulusSeries",
                     "VoltageClampSeries", "VoltageClampStimulusSeries")

def nwb(p: Path) -> dict:
    """NWB 2.x via h5py (BSD-3), following the NWB/HDMF-common schemas (open standards).

    Groups are visited breadth-first with names sorted (openreadout's order). Traces: every
    TimeSeries, ElectricalSeries and SpatialSeries under acquisition/ or processing/ (and the
    patch-clamp series family there and under stimulus/presentation/, with their sweep_number), values
    data * conversion (* channel_conversion[c] for an ElectricalSeries) + offset per column;
    irregular timestamps become a leading `time` channel. Tables, in the same order: every
    group with a `colnames` attribute and an `id` dataset (a DynamicTable: `id`, then the
    colnames columns; numeric and boolean as numbers, text as codes in first-appearance order,
    ragged columns as `<name>_count`, 2-D numeric columns up to 64 wide as `<name>[k]`, object
    references left out), a units table's spike times as their own table (unit_row, unit_id,
    spike_time), and every SpikeEventSeries (time_s, then its values * conversion + offset).
    Column hashes are xxh3-128 of each column as float64."""
    import h5py
    f = h5py.File(p, "r")
    t = lambda v: v.decode() if isinstance(v, bytes) else (v if not isinstance(v, np.ndarray) else [x.decode() if isinstance(x, bytes) else x for x in v.tolist()])
    session = {k: t(f[k][()]) for k in ("session_description", "identifier", "session_start_time") if k in f}
    order = []
    queue = ["/"]
    while queue:
        g = queue.pop(0)
        order.append(g)
        grp = f[g]
        for k in sorted(k for k in grp.keys() if isinstance(grp[k], h5py.Group)):
            queue.append(g.rstrip("/") + "/" + k)
    traces, tables = [], []
    has_icephys = False
    for g in order:
        if g == "/" or g.startswith("/specifications"):
            continue
        obj = f[g]
        kind = t(obj.attrs.get("neurodata_type", b""))
        in_data = g.startswith("/acquisition/") or g.startswith("/processing/")
        if (kind in NWB_TRACE_TYPES and in_data) or (
                kind in NWB_ICEPHYS_TYPES and (in_data or g.startswith("/stimulus/presentation/"))):
            tr = _nwb_trace(obj, g, len(traces))
            if kind in NWB_ICEPHYS_TYPES and "sweep_number" in obj.attrs:
                tr["sweep_number"] = int(obj.attrs["sweep_number"])
            has_icephys |= kind in NWB_ICEPHYS_TYPES
            traces.append(tr)
        elif kind == "SpikeEventSeries":
            tables.append(_nwb_spike_events(obj, g, len(tables)))
        elif "colnames" in obj.attrs and "id" in obj and isinstance(obj["id"], h5py.Dataset):
            tables += _nwb_dynamic(obj, g, len(tables))
    out = {"reader": f"h5py {h5py.__version__}", "nwb_version": t(f.attrs.get("nwb_version", b"")), "session": session,
           "traces": traces}
    if has_icephys:
        out.update(_nwb_icephys_pynwb(p, traces))
    if tables:
        out["tables"] = tables
    return out

def _nwb_icephys_pynwb(p, traces):
    """Second opinion for patch-clamp series: pynwb (BSD-3) `get_data_in_units()` per series,
    compared with the h5py first values to float32 precision (pynwb multiplies float32 data by
    a float32 `conversion` in float32; h5py values here are float64 products)."""
    try:
        import pynwb
        with pynwb.NWBHDF5IO(str(p), "r", load_namespaces=True) as io:
            nwb = io.read()
            series = dict(nwb.acquisition)
            series.update({k: v for k, v in nwb.stimulus.items()})
            agree = True
            checked = 0
            for tr in traces:
                s = series.get(tr["name"])
                if s is None or not isinstance(s, pynwb.icephys.PatchClampSeries):
                    continue
                v = np.asarray(s.get_data_in_units(), dtype=np.float64).reshape(-1)
                first = tr["sweeps"][0]["channels"][-1]["first"]
                n = min(len(first), len(v))
                agree &= bool(np.allclose(v[:n], first[:n], rtol=1e-6, atol=0))
                checked += 1
            return {"pynwb_version": pynwb.__version__, "pynwb_series_checked": checked,
                    "pynwb_values_agree": agree}
    except Exception as e:  # a second opinion only
        return {"pynwb_error": str(e)[:200]}

def _nwb_float(v):
    """A scalar attribute as float; a float32 one at its shortest decimal (the value the writer
    set: MIES stores `conversion` 0.001 as float32)."""
    v = np.asarray(v).reshape(-1)[0] if np.ndim(v) else v
    return float(str(v)) if isinstance(v, np.float32) else float(v)

def _nwb_trace(s, g, index):
    data = s["data"]
    conv = _nwb_float(data.attrs.get("conversion", 1.0))
    off = _nwb_float(data.attrs.get("offset", 0.0))
    arr = np.asarray(data[()], dtype=np.float64)
    cols = arr.reshape(arr.shape[0], -1)
    cc = np.asarray(s["channel_conversion"][()], dtype=np.float64) if "channel_conversion" in s else None
    chans = [(cols[:, c] * conv) * cc[c] + off if cc is not None else cols[:, c] * conv + off for c in range(cols.shape[1])]
    rate = None
    irregular = False
    if "starting_time" in s:
        rate = _nwb_float(s["starting_time"].attrs["rate"])
    elif "timestamps" in s:
        ts = np.asarray(s["timestamps"][()], dtype=np.float64)
        if len(ts) > 1:
            dt = (ts[-1] - ts[0]) / (len(ts) - 1)
            if dt > 0 and np.all(np.abs(np.diff(ts) - dt) <= 1e-6 * dt):
                rate = 1.0 / dt
            else:
                irregular = True
        if irregular:
            chans = [ts] + chans
    name = g.rsplit("/", 1)[-1]
    names = (["time"] if irregular else []) + ([name] if cols.shape[1] == 1 else [f"{name}[{c}]" for c in range(cols.shape[1])])
    return {"index": index, "name": name, "sweep_count": 1, "channel_count": len(chans),
            "channel_names": names, "sample_rate_hz": rate if rate is not None else 0.0,
            "sweeps": [{"sweep": 0, "sample_count": int(cols.shape[0]),
                        "channels": [{"xxh3": _trace_hash(ch), "first": _first(ch)} for ch in chans]}]}

def _nwb_table(index, name, rows, cols):
    return {"index": index, "name": name, "event_count": int(rows), "parameter_names": list(cols.keys()),
            "dtypes": [], "xxh3": None, "column_hashes": {k: _col_hash(v) for k, v in cols.items()}}

def _nwb_dynamic(obj, g, index):
    import h5py
    rows = obj["id"].shape[0]
    names = [x.decode() if isinstance(x, bytes) else str(x) for x in np.atleast_1d(obj.attrs["colnames"])]
    if not names:
        names = sorted(k for k in obj.keys() if isinstance(obj[k], h5py.Dataset) and k != "id" and not k.endswith("_index"))
    cols = {"id": np.asarray(obj["id"][()], dtype=np.float64)}
    spike = None
    for n in names:
        if n not in obj or not isinstance(obj[n], h5py.Dataset):
            continue
        d = obj[n]
        if n + "_index" in obj:
            ends = np.asarray(obj[n + "_index"][()], dtype=np.float64)
            cols[f"{n}_count"] = np.diff(np.concatenate([[0.0], ends]))
            if n == "spike_times":
                spike = (d, ends)
            continue
        if d.dtype.kind == "O" and h5py.check_dtype(ref=d.dtype) is not None:
            continue
        if d.ndim == 1 and d.shape[0] == rows and (d.dtype.kind in "iuf" or d.dtype.kind == "b" or h5py.check_dtype(enum=d.dtype) is not None):
            cols[n] = np.asarray(d[()]).astype(np.float64)
        elif d.ndim == 2 and d.shape[0] == rows and d.dtype.kind in "iuf" and d.shape[1] <= 64:
            a = np.asarray(d[()], dtype=np.float64)
            for k in range(d.shape[1]):
                cols[f"{n}[{k}]"] = a[:, k]
        elif d.ndim == 1 and d.shape[0] == rows and (d.dtype.kind in "OSU"):
            vals = [x.decode() if isinstance(x, bytes) else str(x) for x in d[()]]
            cats = {}
            for v in vals:
                cats.setdefault(v, len(cats))
            if len(cats) <= 10000:
                cols[n] = np.asarray([cats[v] for v in vals], dtype=np.float64)
    name = g.lstrip("/")
    out = [_nwb_table(index, name, rows, cols)]
    if spike is not None:
        d, ends = spike
        e = ends.astype(np.int64)
        ids = cols["id"]
        unit_row = np.repeat(np.arange(len(e), dtype=np.float64), np.diff(np.concatenate([[0], e])))
        unit_id = np.repeat(ids, np.diff(np.concatenate([[0], e])))
        times = np.asarray(d[()], dtype=np.float64)[: int(e[-1]) if len(e) else 0]
        out.append(_nwb_table(index + 1, f"{name}/spike_times", len(times),
                              {"unit_row": unit_row, "unit_id": unit_id, "spike_time": times}))
    return out

def _nwb_spike_events(s, g, index):
    data = s["data"]
    conv = float(data.attrs.get("conversion", 1.0))
    off = float(data.attrs.get("offset", 0.0))
    arr = np.asarray(data[()], dtype=np.float64)
    n = arr.shape[0]
    cols = {"time_s": np.asarray(s["timestamps"][()], dtype=np.float64)}
    if arr.ndim == 1:
        cols["value"] = arr * conv + off
    elif arr.ndim == 2:
        for k in range(arr.shape[1]):
            cols[f"w{k}"] = arr[:, k] * conv + off
    else:
        for c in range(arr.shape[1]):
            for k in range(arr.shape[2]):
                cols[f"c{c}_w{k}"] = arr[:, c, k] * conv + off
    return _nwb_table(index, g.lstrip("/"), n, cols)


def oir(p: Path) -> dict:
    """Olympus/Evident OIR via oirfile (BSD-3), companion (continuation) files included.

    One oracle image for the main image and one for the reference image when the file has one,
    in that order (docs/formats/oir.md). oirfile's L (lambda) axis is folded into C channel-major,
    c = channel * n_lambda + lambda, as openreadout exposes it; planes are hashed in (c, z, t)
    order (MAX_PLANES evenly spaced ones for large images).
    """
    import oirfile
    images = []
    with oirfile.OirFile(p, squeeze=False, multifile=True) as f:
        sizes = dict(f.sizes)
        extra = {"reader_sizes": sizes, "datetime": f.datetime,
                 "companion_files": [Path(c).name for c in f.companion_files],
                 "channel_names": [c.name for c in f.channels]}
        if sizes:
            a = f.asarray()
            dims = list(sizes)
            T, L, Z = sizes.get("T", 1), sizes.get("L", 1), sizes.get("Z", 1)
            C = sizes.get("C", sizes.get("S", 1))
            def get(t, l, z, c):
                idx = []
                for d in dims:
                    idx.append({"T": t, "L": l, "Z": z, "C": c, "S": c}.get(d, slice(None)))
                return a[tuple(idx)]
            order = [(c, z, t) for c in range(C * L) for z in range(Z) for t in range(T)]
            planes = []
            for k in _spread(len(order), MAX_PLANES):
                c, z, t = order[k]
                planes.append({"c": c, "z": z, "t": t, "xxh3": h(get(t, c % L, z, c // L))})
            sc = f.coord_scales
            images.append({"index": 0, "size_x": sizes["X"], "size_y": sizes["Y"], "size_z": Z,
                           "size_c": C * L, "size_t": T, "pixel_type": dtype_name(f.dtype),
                           "physical_size_um": {"x": sc.get("X"), "y": sc.get("Y"), "z": sc.get("Z") if Z > 1 else None},
                           "time_increment_s": sc.get("T"), "lambda_nm": [float(v) for v in f.coords.get("L", [])],
                           "planes": planes})
            del a
        r = f.reference
        if r is not None:
            ra = r.asarray()
            rs = r.sizes
            RC = rs.get("C", 1)
            planes = [{"c": c, "z": 0, "t": 0, "xxh3": h(ra[c] if "C" in rs else ra)} for c in range(RC)]
            sc = r.coord_scales
            images.append({"index": len(images), "kind": "reference", "size_x": rs["X"], "size_y": rs["Y"],
                           "size_z": 1, "size_c": RC, "size_t": 1, "pixel_type": dtype_name(r.dtype),
                           "physical_size_um": {"x": sc.get("X"), "y": sc.get("Y")}, "planes": planes})
    return {"reader": f"oirfile {oirfile.__version__}", "images": images, **extra}

def _bftools(tool: str) -> str:
    """Path of a Bio-Formats command-line tool: $BFTOOLS_DIR, oracle/bftools/bftools, or the
    same directory in the main checkout when running from a git worktree."""
    cands = []
    if os.environ.get("BFTOOLS_DIR"):
        cands.append(Path(os.environ["BFTOOLS_DIR"]))
    here = Path(__file__).resolve().parent
    cands.append(here / "bftools" / "bftools")
    for parent in here.parents:
        if parent.name == "worktrees" and parent.parent.name == ".claude":
            cands.append(parent.parent.parent / "oracle" / "bftools" / "bftools")
    for c in cands:
        if (c / tool).exists():
            return str(c / tool)
    raise RuntimeError("Bio-Formats bftools not found: unzip bftools.zip into oracle/bftools/ or set BFTOOLS_DIR")

def _showinf(p: Path, *args) -> str:
    import subprocess
    out = subprocess.run([_bftools("showinf"), "-nopix", "-no-upgrade", *args, str(p)],
                         capture_output=True, text=True, timeout=3600)
    return out.stdout

def _bf_series(text: str) -> list:
    """Parse `showinf` core metadata: one dict per series."""
    series = []
    cur = None
    for line in text.splitlines():
        m = re.match(r"Series #(\d+)", line)
        if m:
            cur = {"series": int(m.group(1))}
            series.append(cur)
            continue
        if cur is None:
            continue
        s = line.strip()
        for key, pat, conv in [("width", r"Width = (\d+)", int), ("height", r"Height = (\d+)", int),
                               ("size_z", r"SizeZ = (\d+)", int), ("size_t", r"SizeT = (\d+)", int),
                               ("size_c", r"SizeC = (\d+)", int), ("pixel_type", r"Pixel type = (\w+)", str),
                               ("rgb", r"RGB = (\w+) \((\d+)\)", None), ("resolutions", r"Resolutions = (\d+)", int),
                               ("thumbnail", r"Thumbnail series = (\w+)", str),
                               ("dimension_order", r"Dimension order = (\w+)", str)]:
            mm = re.match(pat, s)
            if mm:
                if key == "rgb":
                    cur["samples"] = int(mm.group(2))
                else:
                    cur[key] = conv(mm.group(1))
    return series

def _bf_images(text: str) -> list:
    """Image name, physical sizes and channels from the OME-XML `showinf -omexml` prints."""
    import xml.etree.ElementTree as ET
    i = text.find("<OME ")
    j = text.rfind("</OME>")
    if i < 0 or j < 0:
        return []
    root = ET.fromstring(text[i:j + 6])
    ns = {"o": root.tag.split("}")[0].strip("{")}
    out = []
    for im in root.findall("o:Image", ns):
        px = im.find("o:Pixels", ns)
        chans = [{"name": c.get("Name"), "emission_nm": float(c.get("EmissionWavelength")) if c.get("EmissionWavelength") else None}
                 for c in px.findall("o:Channel", ns)]
        phys = {}
        for ax in "XYZ":
            v = px.get(f"PhysicalSize{ax}")
            if v is not None:
                phys[ax.lower()] = float(v)
        out.append({"name": im.get("Name"), "acquired": (im.findtext("o:AcquisitionDate", None, ns)),
                    "physical_size_um": phys, "channels": chans, "type": px.get("Type")})
    return out

VSI_MAX_BYTES = int(os.environ.get("ORACLE_VSI_MAX_BYTES", str(300 << 20)))  # largest plane set converted per series

# Planes Bio-Formats 8.5.0 is observed to get wrong (black box; docs/provenance/vsi.md, 2026-09-25):
# file name -> {"levels": [pyramid levels whose planes are left out], "why": ...}; "all" drops every
# plane but keeps the geometry of level 0. Such files are checked against the depositor's own
# export instead (crates/openreadout-corpus-tests/tests/vsi_exports.rs).
VSI_BF_WRONG = {
    "stitch.vsi": {"levels": "all", "why": "tiles with negative grid indices are dropped: level 0 is mostly zeros and levels 1-5 are clipped to the non-negative tiles"},
    "MBF_EXP_spleen_prussian_717.vsi": {"levels": [4], "why": "the tile-grid offset (-42 px) is divided by 16 rounding toward zero (-2); the nearest pixel (-3) matches the block-averaged level 0 better"},
}

def _bf_planes(p: Path, series: int, info: dict, tmp: Path, sidecar=None) -> list:
    """Convert one flattened series with bfconvert to a multi-page TIFF and hash its planes in
    (c, z, t) order (bfconvert keeps the reader's dimension order: XYCZT gives page = c + C * (z + Z * t),
    XYZCT page = z + Z * (c + C * t), ...; `info["dimension_order"]`, default XYCZT). With `sidecar`
    (a (directory, name prefix) pair) the hashed planes are also written as raw little-endian
    files `<prefix>_c<c>_z<z>_t<t>.bin`, which the corpus harness compares within the manifest's
    pixel_tolerance (lossy codecs decode slightly differently in every implementation). Each
    plane also carries its mean, which the harness compares for files marked `lossy` when the
    (uncommitted) sidecars are absent, as on CI."""
    import subprocess, tifffile
    out = tmp / f"s{series}.ome.tif"
    subprocess.run([_bftools("bfconvert"), "-no-upgrade", "-overwrite", "-series", str(series), str(p), str(out)],
                   capture_output=True, text=True, timeout=7200, check=True)
    C = info.get("size_c", 1) if info.get("samples", 1) == 1 else 1
    Z, T = info.get("size_z", 1), info.get("size_t", 1)
    planes = []
    with tifffile.TiffFile(out) as tf:
        pages = tf.pages
        order = [(c, z, t) for c in range(C) for z in range(Z) for t in range(T)]
        for k in _spread(len(order), MAX_PLANES):
            c, z, t = order[k]
            dim_order = info.get("dimension_order", "XYCZT")[2:]
            size = {"C": C, "Z": Z, "T": T}
            idx = {"C": c, "Z": z, "T": t}
            page, stride = 0, 1
            for ax in dim_order:
                page += idx[ax] * stride
                stride *= size[ax]
            a = pages[page].asarray()
            planes.append({"c": c, "z": z, "t": t, "xxh3": h(a),
                           "mean": float(np.asarray(a, dtype=np.float64).mean())})
            if sidecar is not None:
                sidecar[0].mkdir(parents=True, exist_ok=True)
                raw = np.ascontiguousarray(a).astype(a.dtype.newbyteorder("<")).tobytes()
                (sidecar[0] / f"{sidecar[1]}_c{c}_z{z}_t{t}.bin").write_bytes(raw)
    out.unlink()
    return planes

def vsi(p: Path) -> dict:
    """cellSens VSI via Bio-Formats (GPL; run as a black box).

    One oracle image per ETS stack (Bio-Formats' non-thumbnail series with -noflat, in order);
    the .vsi's own TIFF preview (Bio-Formats' thumbnail series) is an attachment in openreadout
    and is left out. Planes are hashed from bfconvert output: full resolution when the series is
    at most ORACLE_VSI_MAX_BYTES, and every downsampled pyramid level up to that size.
    ORACLE_SIDECARS=1 also writes the planes next to the file (`<stem>.oracle/`, not committed).
    A level Bio-Formats fails to convert is listed in `bf_failed_levels` and not compared.
    """
    import subprocess, tempfile
    noflat = _bf_series(_showinf(p, "-noflat", "-nometa"))
    flat = _bf_series(_showinf(p, "-nometa"))
    meta = _bf_images(_showinf(p, "-noflat", "-omexml"))
    images = []
    flat_i = 0
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        for k, s in enumerate(noflat):
            nres = s.get("resolutions", 1)
            first = flat_i
            flat_i += nres
            m = meta[k] if k < len(meta) else {}
            # The .vsi's own preview: flagged as a thumbnail series, or (a .vsi deposited without
            # its _<name>_ ETS directory) the only series, named "macro image".
            if s.get("thumbnail") == "true" or m.get("name") == "macro image":
                continue
            spp = s.get("samples", 1)
            C = s.get("size_c", 1) if spp == 1 else 1
            bps = {"uint8": 1, "int8": 1, "uint16": 2, "int16": 2, "float": 4, "uint32": 4, "int32": 4, "double": 8}.get(s.get("pixel_type"), 1)
            img = {"index": len(images), "name": m.get("name"), "size_x": s["width"], "size_y": s["height"],
                   "size_z": s.get("size_z", 1), "size_c": C, "size_t": s.get("size_t", 1),
                   "samples_per_pixel": spp, "pixel_type": dtype_name(s.get("pixel_type", "uint8")) if s.get("pixel_type") != "float" else "float",
                   "physical_size_um": {k2: v for k2, v in m.get("physical_size_um", {}).items() if not (k2 == "z" and s.get("size_z", 1) <= 1)},
                   "channel_names": [c["name"] for c in m.get("channels", [])],
                   "emission_nm": [c["emission_nm"] for c in m.get("channels", [])],
                   "acquired": m.get("acquired"), "resolutions": nres, "planes": [], "levels": []}
            for r in range(nres):
                fs = flat[first + r]
                nbytes = fs["width"] * fs["height"] * spp * bps * C * img["size_z"] * img["size_t"]
                if r == 0:
                    img["level_sizes"] = []
                img["level_sizes"].append([fs["width"], fs["height"]])
                if nbytes > VSI_MAX_BYTES:
                    continue
                side = None
                if os.environ.get("ORACLE_SIDECARS"):
                    pre = f"image{img['index']}" if r == 0 else f"image{img['index']}_l{r}"
                    side = (p.with_name(f"{p.stem}.oracle"), pre)
                try:
                    planes = _bf_planes(p, first + r, fs, tmp, side)
                except subprocess.CalledProcessError as e:
                    # Bio-Formats fails on this level (an exception inside its reader): the
                    # level is left out of the comparison rather than failing the whole oracle.
                    img.setdefault("bf_failed_levels", []).append({"level": r, "error": (e.stderr or "")[-300:] if isinstance(e.stderr, str) else "bfconvert failed"})
                    continue
                if r == 0:
                    img["planes"] = planes
                else:
                    img["levels"].append({"level": r, "size_x": fs["width"], "size_y": fs["height"], "planes": planes})
            images.append(img)
    wrong = VSI_BF_WRONG.get(p.name)
    if wrong:
        for img in images:
            if wrong["levels"] == "all":
                img["planes"], img["levels"] = [], []
                img["level_sizes"] = img.get("level_sizes", [])[:1]
            else:
                img["levels"] = [l for l in img["levels"] if l["level"] not in wrong["levels"]]
            img["bf_wrong"] = wrong["why"]
    out = {"reader": "Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box)", "images": images}
    if not images:
        # A .vsi without its ETS directory: Bio-Formats reads no image either, so agreeing on
        # "no images" confirms nothing about how values are read.
        out["independent"] = False
    return out
MIRAX_MAX_BYTES = int(os.environ.get("ORACLE_MIRAX_MAX_BYTES", str(48 << 20)))  # largest level read whole


def _mirax_storing_channels(p: Path):
    """STORING_CHANNEL_NUMBER of each filter of a fluorescence MIRAX slide (None: brightfield).

    Read from Slidedat.ini with configparser; OpenSlide returns the stored images' decoded
    R, G, B, and filter k lives in component 2 - STORING_CHANNEL_NUMBER (B, G, R storage,
    docs/provenance/mirax.md: the UV filter of Mirax2-Fluorescence-1 is the red component).
    """
    import configparser
    raw = (p.with_suffix("") / "Slidedat.ini").read_bytes()
    text = raw.decode("utf-8-sig", "replace")
    cp = configparser.RawConfigParser(strict=False)
    cp.optionxform = str
    cp.read_string(text)
    if "FLUORESCENCE" not in cp.get("GENERAL", "SLIDE_TYPE", fallback="").upper():
        return None
    h = cp["HIERARCHICAL"]
    for i in range(int(h["HIER_COUNT"])):
        if h[f"HIER_{i}_NAME"] == "Slide filter level":
            return [int(cp[h[f"HIER_{i}_VAL_{j}_SECTION"]]["STORING_CHANNEL_NUMBER"]) for j in range(int(h[f"HIER_{i}_COUNT"]))]
    return None


def mirax(p: Path) -> dict:
    """3DHISTECH MIRAX via OpenSlide (LGPL; openslide-python, run as a black box).

    One image: OpenSlide's level-0 size, mpp and level sizes. Whole levels up to
    ORACLE_MIRAX_MAX_BYTES are read with `read_region` (RGBA, alpha composited over
    `openslide.background-color` and rounded, as openreadout returns uncovered and partly covered
    pixels) and hashed; a fluorescence slide's RGB is split into one plane per filter (see
    `_mirax_storing_channels`). Levels are lossy (JPEG decoders and OpenSlide's 8-bit compositing):
    each plane carries its mean. ORACLE_SIDECARS=1 writes the planes next to the file.
    """
    import openslide
    o = openslide.OpenSlide(str(p))
    props = o.properties
    bg = np.array([int(props.get("openslide.background-color", "FFFFFF")[i:i + 2], 16) for i in (0, 2, 4)], float)
    storing = _mirax_storing_channels(p)
    fluor = storing is not None
    C = len(storing) if fluor else 1
    width, height = o.dimensions
    img = {"index": 0, "size_x": width, "size_y": height, "size_z": 1, "size_c": C, "size_t": 1,
           "samples_per_pixel": 1 if fluor else 3, "pixel_type": "uint8", "lossy": True,
           "physical_size_um": {"x": float(props["openslide.mpp-x"]), "y": float(props["openslide.mpp-y"])},
           "pyramid_levels": o.level_count,
           "level_sizes": [list(d) for d in o.level_dimensions],
           "planes": [], "levels": []}
    side = None
    if os.environ.get("ORACLE_SIDECARS"):
        side = p.with_name(f"{p.stem}.oracle")
        side.mkdir(exist_ok=True)
    for lv in range(1, o.level_count):
        lw, lh = o.level_dimensions[lv]
        if lw * lh * 3 > MIRAX_MAX_BYTES:
            continue
        rgba = np.asarray(o.read_region((0, 0), lv, (lw, lh))).astype(np.float64)
        a = rgba[..., 3:4] / 255.0
        rgb = np.floor(rgba[..., :3] * a + (1 - a) * bg + 0.5).clip(0, 255).astype(np.uint8)
        planes = []
        arrays = [rgb] if not fluor else [np.ascontiguousarray(rgb[..., 2 - s]) for s in storing]
        for c, arr in enumerate(arrays):
            planes.append({"c": c, "z": 0, "t": 0, "xxh3": h(arr), "mean": float(arr.mean())})
            if side is not None:
                arr.tofile(side / f"image0_l{lv}_c{c}_z0_t0.bin")
        img["levels"].append({"level": lv, "size_x": lw, "size_y": lh, "planes": planes})
    return {"reader": f"OpenSlide {openslide.__library_version__} (openslide-python {openslide.__version__}; LGPL, black box)",
            "images": [img]}


def zvi(p: Path) -> dict:
    """Zeiss AxioVision ZVI via Bio-Formats 8.5.0 (GPL; run as a black box):
    `showinf -omexml` for geometry, physical sizes and channel names, `bfconvert` for the planes
    (hashed in (c, z, t) order). Bio-Formats reports 1.0 µm for axes whose ZVI scale unit is 0
    (uncalibrated); openreadout omits those, so a physical size of exactly 1.0 is dropped here
    (see docs/formats/zvi.md)."""
    import tempfile
    series = _bf_series(_showinf(p, "-nometa"))
    meta = _bf_images(_showinf(p, "-omexml"))
    if not series:
        # Bio-Formats 8.5.0 cannot open some files (its compound-file parser fails); fall back
        # to olefile (BSD-2) for the container and the plane layout of docs/formats/zvi.md.
        return _zvi_olefile(p)
    images = []
    with tempfile.TemporaryDirectory() as td:
        for k, s in enumerate(series):
            m = meta[k] if k < len(meta) else {}
            spp = s.get("samples", 1)
            C = s.get("size_c", 1) if spp == 1 else 1
            phys = {a: v for a, v in m.get("physical_size_um", {}).items()
                    if v != 1.0 and not (a == "z" and s.get("size_z", 1) <= 1)}
            images.append({"index": k, "name": m.get("name"), "size_x": s["width"], "size_y": s["height"],
                           "size_z": s.get("size_z", 1), "size_c": C, "size_t": s.get("size_t", 1),
                           "samples_per_pixel": spp, "pixel_type": dtype_name(s.get("pixel_type", "uint8")) if s.get("pixel_type") != "float" else "float",
                           "physical_size_um": phys, "channel_names": [c["name"] for c in m.get("channels", [])],
                           "planes": _bf_planes(p, k, s, Path(td))})
    return {"reader": "Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box)", "images": images}


def _zvi_tags(b: bytes) -> dict:
    """ZVI tag list (docs/formats/zvi.md): i32 version, i32 count, then (typed value, i32 id,
    i32 attribute); typed values are a u16 VARENUM code and its data. Returns {id: value}."""
    import struct
    sizes = {2: 2, 3: 4, 4: 4, 5: 8, 7: 8, 11: 2, 16: 1, 17: 1, 18: 2, 19: 4, 20: 8, 21: 8, 22: 4, 23: 4, 6: 8}
    def val(i):
        (code,) = struct.unpack_from("<H", b, i)
        i += 2
        if code in (0, 1):
            return None, i
        if code in (8, 65, 66, 67, 68, 69, 70):
            (n,) = struct.unpack_from("<I", b, i)
            raw = b[i + 4:i + 4 + n]
            return (raw.decode("utf-16-le", "replace").rstrip("\0") if code == 8 else raw), i + 4 + n
        if code in (9, 13):
            return None, i + 16
        n = sizes[code]
        fmt = {2: "<h", 3: "<i", 4: "<f", 5: "<d", 7: "<d", 11: "<h", 16: "<b", 17: "<B", 18: "<H", 19: "<I", 20: "<q", 21: "<Q", 22: "<i", 23: "<I", 6: "<q"}[code]
        return struct.unpack_from(fmt, b, i)[0], i + n
    out = {}
    try:
        _, i = val(0)
        count, i = val(i)
        for _ in range(count):
            v, i = val(i)
            (tid,) = struct.unpack_from("<i", b, i + 2)
            i += 6
            (_attr,) = struct.unpack_from("<i", b, i + 2)
            i += 6
            out[tid] = v
    except (struct.error, KeyError):
        pass
    return out


def _zvi_olefile(p: Path) -> dict:
    """ZVI planes read with olefile (BSD-2) as the compound-file reader and the raw-image header
    of docs/formats/zvi.md (u32 0x10002000, width, height, depth, bytes per pixel, format, valid
    bits, then the samples): used where Bio-Formats cannot open the file. Not independent of our
    format notes for the plane layout, hence `independent: false`."""
    import olefile, struct
    o = olefile.OleFileIO(str(p))
    items = []
    for e in o.listdir():
        if len(e) == 3 and e[0] == "Image" and e[1].startswith("Item(") and e[2] == "Contents":
            n = int(e[1][5:-1])
            data = o.openstream("/".join(e)).read()
            k = data.find(struct.pack("<I", 0x10002000), 0, 65536)
            w, hh, _d, bpp, fmt_, _bits = struct.unpack_from("<6I", data, k + 4)
            pix = data[k + 28:k + 28 + w * hh * bpp]
            tags = _zvi_tags(o.openstream(f"Image/Item({n})/Tags/Contents").read()) if o.exists(f"Image/Item({n})/Tags/Contents") else {}
            items.append((n, w, hh, bpp, fmt_, pix, tags.get(2819, 0), tags.get(2820, 0), tags.get(2821, 0)))
    zs = sorted({i[6] for i in items}); cs = sorted({i[7] for i in items}); ts = sorted({i[8] for i in items})
    w, hh, bpp, fmt_ = items[0][1], items[0][2], items[0][3], items[0][4]
    spp = 3 if bpp in (3, 6) else 1
    dt = np.uint16 if bpp in (2, 6) else np.uint8
    planes = []
    for it in sorted(items, key=lambda i: (cs.index(i[7]), zs.index(i[6]), ts.index(i[8]))):
        a = np.frombuffer(it[5], dtype=np.dtype(dt).newbyteorder("<"))
        if spp == 3:
            a = a.reshape(hh, w, 3)[:, :, ::-1]  # stored B, G, R
        else:
            a = a.reshape(hh, w)
        planes.append({"c": cs.index(it[7]), "z": zs.index(it[6]), "t": ts.index(it[8]), "xxh3": h(np.ascontiguousarray(a))})
    return {"reader": "olefile 0.47 (BSD-2) + docs/formats/zvi.md plane layout (Bio-Formats 8.5.0 cannot open this file)",
            "independent": False,
            "images": [{"index": 0, "size_x": w, "size_y": hh, "size_z": len(zs), "size_c": len(cs) if spp == 1 else 1,
                        "size_t": len(ts), "samples_per_pixel": spp, "pixel_type": "uint16" if dt is np.uint16 else "uint8",
                        "planes": planes[:MAX_PLANES]}]}


def biorad_scn(p: Path) -> dict:
    """Bio-Rad Image Lab `.scn` via Bio-Formats 8.5.0 (GPL; run as a black box):
    `showinf -omexml` for the image size, pixel type and physical size, `bfconvert` for the plane.
    Channel names are left out (Bio-Formats does not name the Image Lab application). Bio-Formats
    also derives a pixel size from `size_mm` when Image Lab marks it `known="false"` (an imported
    TIFF: 88.9 mm is a 300-dpi default, not a measurement); openreadout reports none, so that
    physical size is dropped here (see docs/formats/biorad-scn.md)."""
    import tempfile
    series = _bf_series(_showinf(p, "-nometa"))
    meta = _bf_images(_showinf(p, "-omexml"))
    unknown_size = re.search(rb'<size_mm[^>]*known="false"', p.read_bytes()[:1 << 20] + _scn_headers(p)) is not None
    images = []
    with tempfile.TemporaryDirectory() as td:
        for k, s in enumerate(series):
            m = meta[k] if k < len(meta) else {}
            images.append({"index": k, "size_x": s["width"], "size_y": s["height"],
                           "size_z": s.get("size_z", 1), "size_c": s.get("size_c", 1), "size_t": s.get("size_t", 1),
                           "samples_per_pixel": s.get("samples", 1), "pixel_type": dtype_name(s.get("pixel_type", "uint16")),
                           "physical_size_um": {} if unknown_size else {a: v for a, v in m.get("physical_size_um", {}).items() if a != "z"},
                           "planes": _bf_planes(p, k, s, Path(td))})
    return {"reader": "Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box)", "images": images}


def _scn_headers(p: Path) -> bytes:
    """The last megabyte of an Image Lab file (its XML headers follow the image data)."""
    n = p.stat().st_size
    with open(p, "rb") as f:
        f.seek(max(0, n - (1 << 20)))
        return f.read()


def _is_image_lab(p: Path) -> bool:
    try:
        with open(p, "rb") as f:
            head = f.read(128)
    except OSError:
        return False
    return head.startswith(b"MIME-Version:") and b"Image Lab" in head


def _oif_oiffile_planes(p: Path) -> list:
    """Per-image {(c, z, t): xxh3} from oiffile (BSD-3): every plane TIFF of the OIF/OIB read with
    oiffile.OifFile.asarray(name) and placed by the axis indices in its file name (s_C001Z002T003,
    lambda L folded into C channel-major, `-R###` reference files as a separate image after the
    main ones), the convention oiffile's `series` grouping documents."""
    import oiffile
    groups = {}
    with oiffile.OifFile(p) as f:
        for n in f.glob("*.tif"):
            base = n.replace("\\", "/").split("/")[-1]
            stem = base[:base.rfind(".")]
            body = stem.split("_", 1)[1] if "_" in stem else stem
            parts = body.split("-")
            axes = re.findall(r"([A-Za-z])(\d+)", parts[0])
            if not axes:
                continue
            key = (len(parts) > 1, "".join(a.upper() for a, _ in axes))
            groups.setdefault(key, []).append(({a.upper(): int(v) for a, v in axes}, n))
        out = []
        for key in sorted(groups):
            files = groups[key]
            def ords(ax):
                vals = sorted({d.get(ax, 1) for d, _ in files})
                return {v: i for i, v in enumerate(vals)}
            C, Z, T, L = (ords(a) for a in "CZTL")
            planes = {}
            for d, n in files:
                c = C[d.get("C", 1)] * len(L) + L[d.get("L", 1)]
                planes[(c, Z[d.get("Z", 1)], T[d.get("T", 1)])] = h(f.asarray(n))
            # the unit of a row step, from the first plane's .pty: "ms" for line scans (XT)
            first = sorted(n for _, n in files)[0]
            try:
                pty = oiffile.SettingsFile(f.open_file(first[:first.rfind(".")] + ".pty"), "pty")
                row_unit = pty.get("Image Parameters", {}).get("HeightUnit")
            except Exception:
                row_unit = None
            out.append({"planes": planes, "row_unit": row_unit})
    return out

def oif_(p: Path) -> dict:
    """Olympus FluoView OIB/OIF: Bio-Formats 8.5.0 (GPL; run as a black box)
    for geometry, physical sizes, channel names and the planes (bfconvert, hashed in (c, z, t)
    order), cross-checked plane by plane against oiffile (BSD-3) reading every plane TIFF; the
    result records whether the two agree. Bio-Formats lists the reference images of line scans
    (s_C###-R###.tif) as a second series, as openreadout does."""
    import tempfile
    series = _bf_series(_showinf(p, "-nometa"))
    meta = _bf_images(_showinf(p, "-omexml"))
    try:
        ref = _oif_oiffile_planes(p)
        ref_error = None
    except Exception as e:  # recorded, not fatal: Bio-Formats is the plane oracle
        ref, ref_error = [], f"{type(e).__name__}: {e}"
    images = []
    with tempfile.TemporaryDirectory() as td:
        for k, s in enumerate(series):
            m = meta[k] if k < len(meta) else {}
            spp = s.get("samples", 1)
            C = s.get("size_c", 1) if spp == 1 else 1
            phys = {a: v for a, v in m.get("physical_size_um", {}).items()
                    if not (a == "z" and s.get("size_z", 1) <= 1)}
            planes = _bf_planes(p, k, s, Path(td))
            agree = None
            if k < len(ref):
                agree = all(ref[k]["planes"].get((q["c"], q["z"], q["t"])) == q["xxh3"] for q in planes)
                # Bio-Formats repeats the X pixel size as Y for line scans (XT), whose rows are
                # successive lines in time (the plane's .pty gives the row unit as ms);
                # openreadout reports no Y size there, so the oracle's Y is dropped.
                if ref[k]["row_unit"] not in (None, "um"):
                    phys.pop("y", None)
            images.append({"index": k, "name": m.get("name"), "size_x": s["width"], "size_y": s["height"],
                           "size_z": s.get("size_z", 1), "size_c": C, "size_t": s.get("size_t", 1),
                           "samples_per_pixel": spp, "pixel_type": dtype_name(s.get("pixel_type", "uint8")) if s.get("pixel_type") != "float" else "float",
                           "physical_size_um": phys, "channel_names": [c["name"] for c in m.get("channels", [])],
                           "oiffile_agrees": agree, "planes": planes})
    import oiffile
    out = {"reader": f"Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box); planes cross-checked with oiffile {oiffile.__version__}",
           "images": images}
    if ref_error:
        out["oiffile_error"] = ref_error
    return out


def metamorph_nd(p: Path) -> dict:
    """MetaMorph `.nd` series (a text file naming many STK/TIFF files): Bio-Formats 8.5.0 (GPL;
    run as a black box) for geometry, physical sizes, channel names and the
    planes (bfconvert, hashed in (c, z, t) order; one series per stage position). Cross-check:
    tifffile (BSD-3) reads every STK/TIFF next to the .nd whose name starts with its stem, and
    every hashed Bio-Formats plane must be one of those planes (`tifffile_agrees`). Stage
    series are named `Stage<n> "<label>"` by Bio-Formats; the label is recorded as
    `check_name` (openreadout names the image by the label). Channel names (`WaveName<n>`)
    are recorded as `check_channel_names`."""
    import tempfile, tifffile
    series = _bf_series(_showinf(p, "-nometa"))
    meta = _bf_images(_showinf(p, "-omexml"))
    stem = p.stem
    known = set()
    members = [q for q in p.parent.iterdir()
               if q.name.startswith(stem + "_") and q.suffix.lower() in (".tif", ".tiff", ".stk")]
    for q in members:
        try:
            with tifffile.TiffFile(q) as tf:
                a = tf.series[0].asarray()
            a = a.reshape((-1,) + a.shape[-2:]) if a.ndim > 2 else a[None]
            for plane in a:
                known.add(h(plane))
        except Exception:
            pass  # a damaged member (the 8-byte STK of figshare-7583960) holds no planes
    images = []
    with tempfile.TemporaryDirectory() as td:
        for k, s in enumerate(series):
            m = meta[k] if k < len(meta) else {}
            spp = s.get("samples", 1)
            C = s.get("size_c", 1) if spp == 1 else 1
            phys = {a: v for a, v in m.get("physical_size_um", {}).items()
                    if not (a == "z" and s.get("size_z", 1) <= 1)}
            planes = _bf_planes(p, k, s, Path(td))
            label = re.match(r'Stage\d+ "(.*)"$', m.get("name") or "")
            img = {"index": k, "name": m.get("name"), "size_x": s["width"], "size_y": s["height"],
                   "size_z": s.get("size_z", 1), "size_c": C, "size_t": s.get("size_t", 1),
                   "samples_per_pixel": spp,
                   "pixel_type": dtype_name(s.get("pixel_type", "uint8")) if s.get("pixel_type") != "float" else "float",
                   "physical_size_um": phys,
                   "tifffile_agrees": all(q["xxh3"] in known for q in planes),
                   "planes": planes}
            if label:
                img["check_name"] = label.group(1)
            names = [c["name"] for c in m.get("channels", [])]
            if names and all(names):  # unnamed channels (no wavelengths) are not compared
                img["check_channel_names"] = names
            images.append(img)
    return {"reader": f"Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box); planes cross-checked with tifffile {tifffile.__version__}",
            "member_files": len(members), "images": images}


def _dcimg_numpy2_shim(dcimg):
    """dcimg 0.6.0 converts one-element structured arrays with int(), which NumPy 2 rejects.
    Give those arrays an __int__/__index__ that takes their single element. Black-box shim only:
    the byte decoding is still dcimg's own."""
    if getattr(dcimg, "_openreadout_shim", False):
        return

    class _One(np.ndarray):
        def __int__(self):
            return int(self.reshape(-1)[0])

        def __index__(self):
            return int(self.reshape(-1)[0])

    class _NP:
        def __getattr__(self, k):
            return getattr(np, k)

        @staticmethod
        def ndarray(shape, dtype, buffer=None, offset=0, strides=None):
            a = np.ndarray(shape, dtype, buffer, offset, strides)
            return a.view(_One) if tuple(np.atleast_1d(shape)) == (1,) else a

    dcimg.np = _NP()
    dcimg._openreadout_shim = True


def dcimg_(p: Path) -> dict:
    """Hamamatsu DCIMG, two black-box readers: Bio-Formats 8.5.0 (GPL), run on
    a copy of the file alone in an empty directory (next to its siblings Bio-Formats groups
    `<stem>_NNN_NNN.dcimg` files into one Z stack), and dcimg 0.6.0 (MIT). Bio-Formats returns
    the rows bottom first; openreadout returns them in stored order, so each Bio-Formats plane
    is flipped vertically first (`bioformats_rows_flipped`).
    - Version 7 (frames back to back, per-frame tables in a footer): the planes are dcimg's
      full-frame reads, which put each frame's own stored values into the overwritten pixels.
      Bio-Formats puts frame 0's values into every frame; the pixels in which it differs are
      counted per plane (`bioformats_diff_pixels`: 0 for frame 0, 4 for the others).
    - Version 0x1000000 (a trailer after every frame): the planes are Bio-Formats' (flipped);
      dcimg reads the stored rows without correction here (its correction position assumes a
      fixed header layout), and the pixels in which they differ are counted
      (`dcimg_raw_diff_pixels`: the corrected pixels, 0 when the file stores none).
    Frame counters and time stamps come from dcimg (`check_frames`)."""
    import shutil, subprocess, tempfile, tifffile, dcimg
    _dcimg_numpy2_shim(dcimg)
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        solo = tmp / "solo"
        solo.mkdir()
        q = solo / p.name
        shutil.copyfile(p, q)
        series = _bf_series(_showinf(q, "-nometa"))
        if not series:
            raise RuntimeError("Bio-Formats could not open the file (its DCIMG reader fails during initialization), "
                               "and no other permissive reader is run without it")
        s = series[0]
        out = tmp / "planes.ome.tif"
        subprocess.run([_bftools("bfconvert"), "-no-upgrade", "-overwrite", str(q), str(out)],
                       capture_output=True, text=True, timeout=3600, check=True)
        with tifffile.TiffFile(out) as tf:
            bf = [np.flipud(pg.asarray()) for pg in tf.pages]
    f = dcimg.DCIMGFile(str(p))
    T = int(f.nfrms)
    old = f.fmt_version == dcimg.DCIMGFile.FMT_OLD
    planes, bf_diff, raw_diff = [], [], []
    for t in _spread(T, MAX_PLANES):
        if old:
            a = np.asarray(f[t])
            bf_diff.append(int((a != bf[t]).sum()))
        else:
            a = bf[t]
            raw_diff.append(int((np.asarray(f.mma[t]) != a).sum()))
        planes.append({"c": 0, "z": 0, "t": t, "xxh3": h(a)})
    frames = []
    ts = f._ts_data
    for t in range(T):
        sec, us = int(ts[t, 0]), int(ts[t, 1])
        stamp = np.datetime64(sec * 10**6 + us, "us")
        frames.append({"t": t, "frame": int(f.framestamps[t]), "acquired_at": str(stamp) + "Z"})
    img = {"index": 0, "name": p.stem, "size_x": s["width"], "size_y": s["height"],
           "size_z": s.get("size_z", 1), "size_c": s.get("size_c", 1), "size_t": s.get("size_t", 1),
           "samples_per_pixel": 1, "pixel_type": dtype_name(s.get("pixel_type", "uint16")),
           "physical_size_um": {}, "bioformats_rows_flipped": True, "planes": planes}
    if old:
        img["bioformats_diff_pixels"] = bf_diff
    else:
        img["dcimg_raw_diff_pixels"] = raw_diff
    reader = (f"dcimg {dcimg.__version__} (MIT) planes and frame records; Bio-Formats 8.5.0 (GPL, black box, rows flipped) second opinion" if old
              else f"Bio-Formats 8.5.0 bfconvert/showinf (GPL, black box), rows flipped; dcimg {dcimg.__version__} (MIT) second opinion and frame records")
    return {"reader": reader,
            "dcimg_version": hex(int(f._file_header['format_version'][0])), "images": [img], "check_frames": frames}


MAX_SWEEPS = int(os.environ.get("ORACLE_MAX_SWEEPS", "1000"))  # hash at most this many sweeps per trace

def _trace_hash(values) -> str:
    """xxh3-128 of one channel of one sweep as little-endian float64 (what openreadout hashes)."""
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(values, dtype="<f8")).tobytes())

def _first(values, n=8):
    """The first n values, cut before the first non-finite one (JSON has no NaN)."""
    out = []
    for v in np.asarray(values, dtype=np.float64)[:n]:
        if not np.isfinite(v):
            break
        out.append(float(v))
    return out

def atf(p: Path) -> dict:
    """Axon Text File via pyABF's ATF class (MIT): header, signals, sweep count and time column.
    pyABF parses the table as float32 and orders channels through a set (unordered); the oracle
    re-reads pyABF's same table rows with numpy as float64 and keeps channels in the order the
    `Signals=` record first names them. Column k of the table (after time) belongs to that signal's
    n-th sweep when it is the n-th column naming the signal. One trace; sweeps x channels hashed."""
    import pyabf
    a = pyabf.ATF(str(p))
    n_header = int(open(p, encoding="latin-1").readlines()[1].split()[0])
    table = np.genfromtxt(str(p), dtype=np.float64, skip_header=3 + n_header, invalid_raise=True)
    table = table.reshape(len(table), -1)
    signals = a.header.get("Signals") or []
    if isinstance(signals, str):
        signals = [signals]
    ncols = table.shape[1] - 1
    if not signals:
        signals = a.columnLabelsY[:ncols]
    order, where = [], {}
    for k in range(ncols):
        sig = signals[k] if k < len(signals) else a.columnLabelsY[k]
        if sig not in order:
            order.append(sig)
        where.setdefault(sig, []).append(k + 1)
    sweeps = []
    for s in range(min(a.sweepCount, MAX_SWEEPS)):
        chans = []
        for sig in order:
            y = table[:, where[sig][s]]
            chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
        sweeps.append({"sweep": s, "sample_count": int(table.shape[0]), "channels": chans})
    t = table[:, 0]
    rate = 1.0 / (t[1] - t[0])
    units = []
    for sig in order:
        title = a.columnLabelsY[where[sig][0] - 1]
        units.append(title[title.rfind("(") + 1:title.rfind(")")].strip() if "(" in title else "")
    return {"reader": f"pyabf {pyabf.__version__} ATF (values re-read as float64)",
            "traces": [{"index": 0, "sweep_count": a.sweepCount, "channel_count": len(order), "channel_names": order,
                        "channel_units": units, "sample_rate_hz": rate, "pyabf_rate_hz": a.dataRate,
                        "sweeps": sweeps}]}

def abf(p: Path) -> dict:
    """Axon ABF via pyABF (MIT). One trace per file.

    pyABF scales in float32 (`data = raw.astype(float32) * gain + offset`), which loses about 7
    significant digits. So the hashed values are recomputed in float64 from pyABF's own raw
    samples (read at pyABF's `dataByteStart`, `dataPointCount`, dtype) and pyABF's own per-channel
    `_dataGain`/`_dataOffset` as `raw * gain + offset`; `f32_max_abs_diff` records how far that is
    from pyABF's float32 `sweepY` (it must be within float32 rounding). Sweeps are split exactly as
    pyABF's setSweep does (fixed length, or the synch array for variable-length ABF2 sweeps).
    `sweeps[s].channels[c]` = {xxh3 of the sweep's samples as float64 LE, first 8 values}; sweeps
    in ascending order, channels in ADC order (pyABF `channelList`)."""
    import pyabf
    try:
        a = pyabf.ABF(str(p))
    except ValueError as e:  # pyABF 2.3.8 refuses ABF 1 files with float samples
        return _abf_neo(p, f"pyABF refused the file ({e}); read with Neo's AxonRawIO instead")
    nch = a.channelCount
    with open(p, "rb") as fb:
        fb.seek(a.dataByteStart)
        raw = np.fromfile(fb, dtype=a._dtype, count=a.dataPointCount)
    if raw.size != a.dataPointCount:
        raise RuntimeError(f"file holds {raw.size} of {a.dataPointCount} samples (truncated)")
    raw = raw.reshape((-1, nch)).T.astype(np.float64)
    if a._dtype == np.int16:
        scaled = np.stack([raw[c] * float(a._dataGain[c]) + float(a._dataOffset[c]) for c in range(nch)])
    else:
        scaled = raw
    sweeps = []
    worst = 0.0
    for s in range(min(a.sweepCount, MAX_SWEEPS)):
        chans = []
        count = None
        for c in range(nch):
            a.setSweep(s, channel=c)
            y32 = np.asarray(a.sweepY, dtype=np.float32)
            # locate the sweep inside the de-interleaved data exactly like setSweep does
            if a.sweepCount > 1 and hasattr(a, "_synchArraySection") and len(set(a._synchArraySection.lLength)) != 1:
                start = sum(a._synchArraySection.lLength[i] // nch for i in range(s))
                n = a._synchArraySection.lLength[s] // nch
            else:
                start = a.sweepPointCount * s
                n = a.sweepPointCount
            y = scaled[c, start:start + n]
            if len(y) != len(y32):
                raise RuntimeError(f"sweep {s} channel {c}: recomputed {len(y)} samples, pyABF {len(y32)}")
            if len(y):
                worst = max(worst, float(np.max(np.abs(y.astype(np.float32) - y32))))
            count = len(y)
            chans.append({"xxh3": _trace_hash(y), "first": _first(y), "first_f32_pyabf": _first(y32)})
        sweeps.append({"sweep": s, "sample_count": count, "channels": chans})
    units = [u for u in a.adcUnits]
    trace = {
        "index": 0,
        "abf_version": a.abfVersionString,
        "sweep_count": a.sweepCount,
        "channel_count": nch,
        "channel_names": list(a.adcNames),
        "channel_units": units,
        "sample_rate_hz": a.dataRate,
        "sample_rate_tolerance_hz": 1.0,  # pyABF truncates the rate to an integer (int(1e6 / interval))
        "sample_count": a.sweepPointCount,
        "data_format": "int16" if a._dtype == np.int16 else "float32",
        "gain": [float(g) for g in a._dataGain],
        "offset": [float(o) for o in a._dataOffset],
        "protocol": a.protocol,
        "created": a.abfDateTimeString,
        "tag_comments": list(a.tagComments),
        "f32_max_abs_diff": worst,
        "sweeps": sweeps,
    }
    traces = [trace]
    cmd = _abf_command(a)
    if cmd is not None:
        traces.append(cmd)
    return {"reader": f"pyabf {pyabf.__version__}", "traces": traces}

def _abf_command(a):
    """Trace 1: the DAC command waveforms from pyABF's epoch-table synthesis
    (pyabf.waveform.EpochTable(abf, dac).epochWaveformsBySweep[s].getWaveform(), what pyABF's
    sweepC returns for an epoch-table waveform), for the DACs openreadout synthesizes: ABF 2,
    episodic stimulation, fixed-length sweeps, no user list, no alternating DAC output; per DAC
    waveform enabled with the epoch table as source, no conditioning train, only off/step/ramp/
    pulse-train epochs (pulse period > 0), epochs ending inside the sweep. None otherwise."""
    import pyabf.waveform
    if a.abfVersion["major"] != 2 or a.nOperationMode != 5:
        return None
    if hasattr(a, "_synchArraySection") and len(set(a._synchArraySection.lLength)) > 1:
        return None
    if any(v for v in getattr(a._userListSection, "nULEnable", []) if v):
        return None
    if a._protocolSection.nAlternateDACOutputState:
        return None
    dacs = []
    ds = a._dacSection
    for d in range(len(ds.nDACNum)):
        if not (ds.nWaveformEnable[d] and ds.nWaveformSource[d]):
            continue
        if ds.nWaveformSource[d] != 1 or ds.nConditEnable[d]:
            continue
        ep = a._epochPerDacSection
        rows = [i for i, n in enumerate(ep.nDACNum) if n == ds.nDACNum[d]]
        types = [ep.nEpochType[i] for i in rows]
        if any(t not in (0, 1, 2, 3) for t in types):
            continue
        if any(ep.nEpochType[i] == 3 and (ep.lEpochPulsePeriod[i] <= 0 or ep.lEpochPulseWidth[i] < 0) for i in rows):
            continue
        if any(ep.lEpochInitDuration[i] < 0 for i in rows):
            continue
        dacs.append(d)
    if not dacs:
        return None
    n = a.sweepPointCount
    sweeps = []
    for s in range(min(a.sweepCount, MAX_SWEEPS)):
        chans = []
        for d in dacs:
            w = pyabf.waveform.EpochTable(a, ds.nDACNum[d]).epochWaveformsBySweep[s].getWaveform()
            y = np.asarray(w, dtype=np.float64)
            if len(y) != n:
                return None  # epochs past the end of the sweep: openreadout does not synthesize
            chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
        sweeps.append({"sweep": s, "sample_count": n, "channels": chans})
    names = [a.dacNames[d] if d < len(a.dacNames) and a.dacNames[d] else f"DAC{ds.nDACNum[d]}" for d in dacs]
    units = [a.dacUnits[d] if d < len(a.dacUnits) else "" for d in dacs]
    return {"index": 1, "name": "command", "sweep_count": a.sweepCount, "channel_count": len(dacs),
            "channel_names": names, "channel_units": units, "sample_rate_hz": a.dataRate,
            "sample_rate_tolerance_hz": 1.0, "sweeps": sweeps}

def _abf_neo(p: Path, why: str) -> dict:
    """Axon ABF via Neo's AxonRawIO (BSD-3) for files pyABF cannot read: segments are sweeps,
    values Neo's float64 rescaling (raw x gain + offset; float samples as stored)."""
    import neo
    from neo.rawio import AxonRawIO
    r = AxonRawIO(filename=str(p))
    r.parse_header()
    if len(r.header["signal_streams"]) != 1:
        raise RuntimeError(f"Neo reports {len(r.header['signal_streams'])} signal streams")
    sc = r.header["signal_channels"]
    sweeps = []
    nseg = r.segment_count(0)
    for s in range(min(nseg, MAX_SWEEPS)):
        raw = r.get_analogsignal_chunk(0, s, None, None, 0)
        y = r.rescale_signal_raw_to_float(raw, dtype="float64", stream_index=0)
        sweeps.append({"sweep": s, "sample_count": int(y.shape[0]),
                       "channels": [{"xxh3": _trace_hash(y[:, c]), "first": _first(y[:, c])} for c in range(y.shape[1])]})
    trace = {"index": 0, "sweep_count": nseg, "channel_count": len(sc),
             # Neo removes the spaces inside ABF 1 channel names ("IN 0" -> "IN0"): recorded, not compared
             "neo_channel_names": [str(c["name"]) for c in sc], "channel_units": [str(c["units"]) for c in sc],
             "sample_rate_hz": float(sc[0]["sampling_rate"]), "sweeps": sweeps}
    return {"reader": f"neo {neo.__version__} AxonRawIO", "oracle_note": why, "traces": [trace]}

def _col_hash(values) -> str:
    """xxh3-128 of one table column as little-endian float64 (what openreadout hashes)."""
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(values, dtype="<f8")).tobytes())

def _neo_single(cls, p: Path, **kw):
    """Run a directory-based Neo reader on one file: copy it alone into a temporary directory."""
    import tempfile, shutil
    d = tempfile.mkdtemp(prefix="openreadout-oracle-")
    shutil.copy(p, d)
    r = cls(dirname=d, **kw)
    r.parse_header()
    return r

def _neo_trace_sweeps(r, stream: int, chan_gain, chan_offset, max_sweeps=MAX_SWEEPS):
    """Neo segments of one stream as oracle sweeps: raw int chunk x gain + offset, float64."""
    nseg = r.header["nb_segment"][0]
    sweeps = []
    for s in range(min(nseg, max_sweeps)):
        n = r.get_signal_size(0, s, stream)
        raw = np.asarray(r.get_analogsignal_chunk(0, s, 0, n, stream), dtype=np.float64)
        chans = []
        for c in range(raw.shape[1]):
            y = raw[:, c] * chan_gain[c] + chan_offset[c]
            chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
        sweeps.append({"sweep": s, "sample_count": int(n), "channels": chans})
    return sweeps

def neuralynx(p: Path) -> dict:
    """Neuralynx via Neo's NeuralynxRawIO (BSD-3), one file at a time.

    NCS: one trace; Neo segments are sweeps; values raw x |gain| (Neo negates the gain when the
    header says -InputInverted True; the vendor's documentation says stored samples are already
    inverted, so openreadout applies no sign change). NEV/NSE/NTT: one table; Neo groups events
    by (event_id, ttl) and spikes by unit, so rows are merged back into record order by timestamp
    and compared column by column (`column_hashes`)."""
    import neo
    from neo.rawio import NeuralynxRawIO
    r = _neo_single(NeuralynxRawIO, p)
    h = r.header
    ext = p.suffix.lower()
    out = {"reader": f"neo {neo.__version__} NeuralynxRawIO"}
    if ext == ".ncs":
        sc = h["signal_channels"]
        gain = [abs(float(c["gain"])) for c in sc]
        off = [float(c["offset"]) for c in sc]
        names = [str(c["name"]) for c in sc]
        trace = {"index": 0, "sweep_count": int(h["nb_segment"][0]), "channel_count": len(sc),
                 "channel_names": [] if names == ["unknown"] else names,
                 "channel_units": [str(c["units"]) for c in sc],
                 "sample_rate_hz": float(sc[0]["sampling_rate"]),
                 "neo_gain": [float(c["gain"]) for c in sc],
                 "sweeps": _neo_trace_sweeps(r, 0, gain, off)}
        out["traces"] = [trace]
        return out
    if ext == ".nev":
        rows = []
        for i, ch in enumerate(h["event_channels"]):
            name = str(ch["name"])
            eid = int(name.split("event_id=")[1].split()[0])
            ttl = int(name.split("ttl=")[1].split()[0])
            ts, _, labels = r.get_event_timestamps(0, 0, i, None, None)
            rows += [(int(t), eid, ttl) for t in ts]
        rows.sort(key=lambda x: x[0])
        # Records that share a timestamp (a TTL word and a strobe in the same microsecond) come
        # back grouped by Neo's (event_id, ttl) channels, so a sort by timestamp cannot restore
        # their order. Neo's own parsed record array (`_nev_memmap`, file order) gives it; its
        # rows must be the same multiset as the API's.
        maps = list(r._nev_memmap.values())
        if len(maps) == 1:
            m = maps[0]
            ordered = [(int(t), int(e), int(x)) for t, e, x in zip(m["timestamp"], m["event_id"], m["ttl_input"])]
            if sorted(ordered) != sorted(rows):
                raise RuntimeError("Neo's record array and its event API disagree")
            rows = ordered
        cols = list(zip(*rows)) if rows else [[], [], []]
        out["tables"] = [{"index": 0, "event_count": len(rows), "parameter_names": [], "dtypes": [], "xxh3": None,
                          "column_hashes": {"timestamp_us": _col_hash(cols[0]), "event_id": _col_hash(cols[1]), "ttl": _col_hash(cols[2])}}]
        return out
    # spikes
    rows = []
    seen = set()
    for u, ch in enumerate(h["spike_channels"]):
        cell = int(str(ch["name"]).rsplit("#", 1)[1])
        # Neo lists a tetrode's spikes once per electrode (one spike channel per A/D channel and
        # unit, all with the full 4-electrode waveform): keep one copy per unit.
        if cell in seen:
            continue
        seen.add(cell)
        ts = r.get_spike_timestamps(0, 0, u, None, None)
        wf = r.get_spike_raw_waveforms(0, 0, u, None, None)  # (n, electrodes, 32)
        g = abs(float(ch["wf_gain"]))
        for k in range(len(ts)):
            rows.append((int(ts[k]), cell, np.asarray(wf[k], dtype=np.float64) * g))
    rows.sort(key=lambda x: x[0])
    hashes = {"timestamp_us": _col_hash([x[0] for x in rows]), "cell": _col_hash([x[1] for x in rows])}
    if rows:
        ne, ns = rows[0][2].shape
        for e in range(ne):
            for s in range(ns):
                hashes[f"w{e}_{s}"] = _col_hash([x[2][e, s] for x in rows])
    out["tables"] = [{"index": 0, "event_count": len(rows), "parameter_names": [], "dtypes": [], "xxh3": None,
                      "column_hashes": hashes}]
    return out

# ---------- NMR ----------

def _sweeps(rows) -> list:
    """Oracle `sweeps` from a list of sweeps, each a list of equal-length channel arrays:
    `[{sweep, sample_count, channels: [{xxh3, first}]}]` (at most MAX_SWEEPS)."""
    return [{"sweep": s, "sample_count": int(len(chans[0])),
             "channels": [{"xxh3": _trace_hash(c), "first": _first(c)} for c in chans]}
            for s, chans in enumerate(rows[:MAX_SWEEPS])]

def _dir_size(p: Path) -> int:
    return sum(f.stat().st_size for f in p.rglob("*") if f.is_file())

_BRUKER_KEYS = ["TD", "NS", "DS", "NUC1", "SFO1", "BF1", "SW_h", "SW", "DTYPA", "BYTORDA", "NC", "AQ_mod",
                "DECIM", "DSPFVS", "GRPDLY", "PULPROG", "SOLVENT", "TE", "DATE", "INSTRUM", "PROBHD",
                "FnTYPE", "FnMODE", "NusTD", "RG"]
_PROC_KEYS = ["SI", "XDIM", "NC_proc", "OFFSET", "SW_p", "SF", "AXNUC", "BYTORDP", "DTYPP", "FTSIZE"]

def _pick_keys(d, keys):
    return {k: _jsonable(d[k]) for k in keys if k in d}

def bruker(p: Path) -> dict:
    """nmrglue (BSD-3) as the oracle for Bruker experiment directories: bruker.read for fid/ser
    (unscaled; we multiply by 2**NC and cut each row to TD values), bruker.read_pdata for each
    pdata/<procno> holding 1r or 2rr (scaled by 2**NC_proc), and unit_conversion for the ppm axis."""
    import nmrglue as ng
    import warnings
    warnings.simplefilter("ignore")
    traces, notes = [], []
    dic, data = ng.bruker.read(str(p))
    ac = dic["acqus"]
    td = int(ac["TD"])
    aq = int(ac.get("AQ_mod", 0))
    complex_ = aq in (1, 3)
    n = td // 2 if complex_ else td
    arr = np.asarray(data)
    rows = arr.reshape(-1, arr.shape[-1]) if arr.ndim > 1 else arr.reshape(1, -1)
    scale = 2.0 ** int(ac.get("NC", 0))
    sweeps = []
    for r in rows:
        r = r[:n]
        if complex_:
            sweeps.append([r.real * scale, r.imag * scale])
        else:
            sweeps.append([np.real(r) * scale])
    if any(len(s[0]) != n for s in sweeps):
        notes.append(f"nmrglue returned rows shorter than TD ({[len(s[0]) for s in sweeps][:3]}...)")
    dims = {f: _pick_keys(dic[f], _BRUKER_KEYS) for f in ("acqu2s", "acqu3s", "acqu4s") if f in dic}
    traces.append({
        "index": 0, "name": "ser" if (p / "ser").exists() and not (p / "fid").exists() else "fid",
        "sample_count": n, "sweep_count": len(sweeps), "channel_count": 2 if complex_ else 1,
        "sample_rate_hz": float(ac["SW_h"]),
        "nmrglue_shape": list(arr.shape), "nmrglue_dtype": str(arr.dtype),
        "reader": f"nmrglue {ng.__version__} bruker.read",
        "parameters": {"acqus": _pick_keys(ac, _BRUKER_KEYS), **dims},
        "sweeps": _sweeps(sweeps),
    })
    pdata = p / "pdata"
    procnos = sorted((int(d.name), d) for d in pdata.iterdir() if d.is_dir() and d.name.isdigit()) if pdata.is_dir() else []
    for procno, d in procnos:
        if not any((d / f).exists() for f in ("1r", "2rr", "3rrr")) or not (d / "procs").exists():
            continue
        entry = {"index": len(traces), "name": f"pdata/{procno}"}
        try:
            pdic, pd = ng.bruker.read_pdata(str(d), all_components=True, scale_data=True)
            comps = pd if isinstance(pd, list) else [pd]
            comps = [np.asarray(c, dtype="<f8") for c in comps]
            if comps[0].ndim == 1:
                sweeps = [comps]
                entry.update(sample_count=int(comps[0].shape[0]), sweep_count=1, channel_count=len(comps))
            else:
                # every component (2rr 2ri 2ir 2ii / 3rrr ... 3iii) as a channel; rows of 3D data
                # flattened with the second dimension fastest
                rows = [c.reshape(-1, c.shape[-1]) for c in comps]
                sweeps = [[r[i] for r in rows] for i in range(rows[0].shape[0])]
                entry.update(sample_count=int(rows[0].shape[1]), sweep_count=int(rows[0].shape[0]), channel_count=len(comps))
            entry["sweeps"] = _sweeps(sweeps)
            entry["reader"] = f"nmrglue {ng.__version__} bruker.read_pdata(scale_data=True)"
            entry["parameters"] = {"procs": _pick_keys(pdic["procs"], _PROC_KEYS)}
            # ppm axis from procs alone (OFFSET, SW_p, SF, SI), as unit_conversion computes it
            adic, adata = ng.bruker.read_pdata(str(d), read_acqus=False, scale_data=False)
            udic = ng.bruker.guess_udic(adic, adata)
            last_dim = udic[udic["ndim"] - 1]
            if last_dim.get("obs") == 999.99 and "procs" in adic:
                # guess_udic gives up on procs without AXNUC (older TopSpin); nmrglue's
                # unit_conversion from the procs values it would otherwise use
                pr = adic["procs"]
                sw, sf, si = float(pr["SW_p"]), float(pr["SF"]), int(pr["SI"])
                uc = ng.fileiobase.unit_conversion(si, True, sw, sf, float(pr["OFFSET"]) * sf - sw / 2)
            else:
                uc = ng.fileiobase.uc_from_udic(udic, dim=udic["ndim"] - 1)
            entry["ppm_first"], entry["ppm_last"] = [float(v) for v in uc.ppm_limits()]
        except Exception as e:
            entry["error"] = f"{type(e).__name__}: {e}"
        traces.append(entry)
    out = {"reader": f"nmrglue {ng.__version__}", "traces": traces}
    if (p / "nuslist").exists():
        rows = ng.bruker.read_nuslist(str(p))
        cols = np.asarray(rows, dtype="<f8").reshape(len(rows), -1)
        out["tables"] = [{"index": 0, "event_count": len(rows),
                          "parameter_names": [f"index_{k + 1}" for k in range(cols.shape[1])],
                          "dtypes": ["uint32"] * cols.shape[1],
                          "xxh3": xxhash.xxh3_128_hexdigest(np.ascontiguousarray(cols.T).tobytes()),
                          "reader": f"nmrglue {ng.__version__} bruker.read_nuslist"}]
    if notes:
        out["notes"] = notes
    return out

def _is_varian_dir(p: Path) -> bool:
    return p.is_dir() and (p / "fid").is_file() and not (p / "acqus").exists() and not (p / "acqu").exists()

def _varian_nucleus(tn: str) -> str:
    m = re.fullmatch(r"([A-Za-z]+)(\d+)", tn.strip())
    return f"{m.group(2)}{m.group(1)}" if m else tn.strip()

def varian(p: Path) -> dict:
    """nmrglue (BSD-3) varian.read(as_2d=True) for VnmrJ directories: every trace of fid in disk
    order, uninterleaved into real/imag (complex64 for int16/float32, complex128 for int32: exact),
    unscaled. Without procpar: varian.read_fid(as_2d=True)."""
    import nmrglue as ng
    import warnings
    warnings.simplefilter("ignore")
    if (p / "procpar").exists():
        dic, data = ng.varian.read(str(p), as_2d=True)
        pp = dic["procpar"]
    else:
        dic, data = ng.varian.read_fid(str(p / "fid"), as_2d=True)
        pp = {}
    arr = np.atleast_2d(np.asarray(data))
    sweeps = [[row.real.astype("<f8"), row.imag.astype("<f8")] for row in arr]
    def v(name):
        return pp[name]["values"][0] if name in pp and pp[name]["values"] else None
    extra = {}
    if v("tn") is not None:
        extra["nucleus"] = _varian_nucleus(v("tn"))
    for key, name, conv in (("spectrometer_frequency_mhz", "sfrq", float), ("spectral_width_hz", "sw", float),
                            ("scans", "nt", lambda x: int(float(x))), ("time_domain_size", "np", lambda x: int(float(x))),
                            ("pulse_program", "seqfil", str), ("solvent", "solvent", str)):
        if v(name) not in (None, ""):
            extra[key] = conv(v(name))
    entry = {"index": 0, "name": "fid", "sample_count": int(arr.shape[1]), "sweep_count": int(arr.shape[0]),
             "channel_count": 2, "reader": f"nmrglue {ng.__version__} varian.read(as_2d=True)",
             "nmrglue_dtype": str(arr.dtype), "sweeps": _sweeps(sweeps)}
    if v("sw") is not None:
        entry["sample_rate_hz"] = float(v("sw"))
    if extra:
        entry["parameters"] = {"extra": extra}
    return {"reader": f"nmrglue {ng.__version__}", "traces": [entry],
            "file_header": {k: dic[k] for k in ("nblocks", "ntraces", "np", "ebytes", "tbytes", "bbytes", "status", "nbheaders")}}

_JEOL_NUCLEI = {"proton": "1H", "carbon13": "13C", "silicon29": "29Si", "nitrogen15": "15N", "fluorine19": "19F",
                "phosphorus31": "31P", "deuterium": "2H"}

def jeol(p: Path) -> dict:
    """nmrglue (BSD-3) jeol.read for .jdf files. nmrglue returns section0 - 1j*section1 (the
    complex conjugate of the stored pair); for 2D complex/complex data rows alternate
    s0 - 1j*s1 and -(s2 - 1j*s3). The oracle undoes the sign changes (exact) so that the hashes
    cover the values as stored, which is what openreadout returns."""
    import nmrglue as ng
    from nmrglue.fileio import jeol as nj
    import warnings
    warnings.simplefilter("ignore")
    dic, data = nj.read(str(p))
    h, pr = dic["header"], dic["parameters"]
    types = [t for t in h["data_axis_type"] if t]
    arr = np.asarray(data)
    if arr.ndim == 1:
        sweeps = [[arr.real, -arr.imag]] if np.iscomplexobj(arr) else [[arr]]
    elif types == ["complex", "complex"]:
        sweeps = []
        for i, row in enumerate(arr):
            sweeps.append([row.real, -row.imag] if i % 2 == 0 else [-row.real, row.imag])
    elif np.iscomplexobj(arr):
        sweeps = [[row.real, -row.imag] for row in arr]
    else:
        sweeps = [[row] for row in arr]
    time_domain = h["units"][0][2] == "second"
    entry = {"index": 0, "name": "fid" if time_domain else "spectrum",
             "sample_count": int(len(sweeps[0][0])), "sweep_count": len(sweeps), "channel_count": len(sweeps[0]),
             "reader": f"nmrglue {ng.__version__} jeol.read",
             "sweeps": _sweeps([[np.asarray(c, dtype="<f8") for c in s_] for s_ in sweeps])}
    if time_domain:
        entry["sample_rate_hz"] = float(pr["x_sweep"])
    else:
        # ppm axis of the valid range, from the header's axis ends over all stored points
        n = h["data_points"][0]
        step = (h["data_axis_stop"][0] - h["data_axis_start"][0]) / (n - 1)
        a, b = h["data_offset_start"][0], h["data_offset_stop"][0]
        entry["ppm_first"] = h["data_axis_start"][0] + step * a
        entry["ppm_last"] = h["data_axis_start"][0] + step * b
    extra = {"spectrometer_frequency_mhz": pr["x_freq"] / 1e6, "spectral_width_hz": pr["x_sweep"],
             "scans": int(pr["scans"]), "time_domain_size": int(pr["x_points"])}
    dom = str(pr.get("x_domain", ""))
    if dom.lower() in _JEOL_NUCLEI:
        extra["nucleus"] = _JEOL_NUCLEI[dom.lower()]
    for key, name in (("solvent", "solvent"), ("pulse_program", "experiment")):
        if pr.get(name):
            extra[key] = str(pr[name])
    entry["parameters"] = {"extra": extra}
    return {"reader": f"nmrglue {ng.__version__}", "traces": [entry],
            "header": {k: _jsonable(h[k]) for k in ("major_version", "minor_version", "data_format", "data_axis_type", "data_points",
                                                    "data_offset_start", "data_offset_stop", "endian", "data_type")}}

def _jcamp_blocks(p: Path):
    """Block-level walk (labels only, no data decoding) giving the order in which data tables occur:
    one entry per ##TITLE= block that holds ##XYDATA=, ##NTUPLES=, ##PEAK TABLE= or ##XYPOINTS=."""
    norm = lambda s: re.sub(r"[\s\-/_]", "", s).upper()
    blocks, stack = [], []
    for raw in p.read_bytes().decode("latin-1").replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        line = raw.split("$$", 1)[0].strip()
        if not line.startswith("##") or "=" not in line:
            continue
        label, value = line[2:].split("=", 1)
        key = norm(label)
        if key == "TITLE":
            blocks.append({"title": value.strip(), "keys": set(), "data_type": None})
            stack.append(len(blocks) - 1)
        elif key == "END":
            if stack:
                stack.pop()
        elif stack:
            b = blocks[stack[-1]]
            b["keys"].add(key)
            if key == "DATATYPE":
                b["data_type"] = value.strip().upper()
    out = []
    for b in blocks:
        if (b["data_type"] or "").strip() == "LINK":
            continue
        if "NTUPLES" in b["keys"]:
            out.append((b, "ntuples"))
        else:
            if "XYDATA" in b["keys"]:
                out.append((b, "xydata"))
            for k, kind in (("PEAKTABLE", "peak_table"), ("XYPOINTS", "xypoints")):
                if k in b["keys"]:
                    out.append((b, kind))
    return out

def jcampdx(p: Path) -> dict:
    """JCAMP-DX oracles: nmrglue.fileio.jcampdx (BSD-3) for the first NMR SPECTRUM / NMR FID block
    (XYDATA or NTUPLES real/imaginary), and the jcamp package (MIT) for every XYDATA / PEAK TABLE /
    XYPOINTS block (compound files through its children). Tables neither decodes (e.g. 2D NTUPLES)
    keep their position without `sweeps` (metadata only)."""
    import nmrglue as ng, jcamp, warnings, io, contextlib
    warnings.simplefilter("ignore")
    order = _jcamp_blocks(p)
    traces = [{"index": i, "name": b["title"], "kind": kind, "data_type": b["data_type"]}
              for i, (b, kind) in enumerate(order)]
    notes = []
    # jcamp package: children in order; those with data map onto non-NTUPLES tables in order
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            d = jcamp.jcamp_readfile(str(p)) if hasattr(jcamp, "jcamp_readfile") else jcamp.readfile(str(p))
        kids = d.get("children") or [d]
        flat = []
        def walk(k):
            for c in k:
                if c.get("children"):
                    walk(c["children"])
                elif len(c.get("y", [])):
                    flat.append(c)
        walk(kids)
        slots = [t for t in traces if t["kind"] != "ntuples"]
        for t, c in zip(slots, flat):
            x = np.asarray(c["x"], dtype="<f8"); y = np.asarray(c["y"], dtype="<f8")
            if t["kind"] == "xydata":
                t.update(sweeps=_sweeps([[y]]), sweep_count=1, sample_count=int(len(y)), channel_count=1, reader=f"jcamp {getattr(jcamp, '__version__', '')}".strip())
            else:
                t.update(sweeps=_sweeps([[x, y]]), sweep_count=1, sample_count=int(len(y)), channel_count=2, reader=f"jcamp {getattr(jcamp, '__version__', '')}".strip())
        if len(flat) != len(slots):
            notes.append(f"jcamp decoded {len(flat)} tables, the block walk found {len(slots)}")
    except Exception as e:
        notes.append(f"jcamp: {type(e).__name__}: {e}")
    # nmrglue: first NMR SPECTRUM / NMR FID with data (its own selection rule)
    try:
        dic, data = ng.jcampdx.read(str(p))
        if data is not None:
            title = (dic.get("TITLE") or [""])[0].strip()
            dtype = (dic.get("DATATYPE") or [""])[0].strip().upper()
            chans = [np.asarray(c, dtype="<f8") for c in (data if isinstance(data, list) else [data]) if c is not None]
            sw = _sweeps([chans])
            cands = [t for t in traces if t["name"] == title and (t["data_type"] or "").replace(" ", "") == dtype.replace(" ", "")]
            if cands:
                t = cands[0]
                if "sweeps" in t and t["sweeps"] != sw:
                    t["second_opinion"] = {"reader": t.get("reader"), "sweeps": t["sweeps"]}
                elif "sweeps" in t:
                    t["second_opinion"] = {"reader": t.get("reader"), "agrees": True}
                t.update(sweeps=sw, sweep_count=1, sample_count=int(len(chans[0])), channel_count=len(chans), reader=f"nmrglue {ng.__version__} jcampdx.read")
            else:
                notes.append(f"nmrglue decoded block {title!r} ({dtype}) but no table block matched it")
    except Exception as e:
        notes.append(f"nmrglue: {type(e).__name__}: {e}")
    out = {"reader": "nmrglue jcampdx + jcamp (second opinion)", "traces": traces}
    if notes:
        out["notes"] = notes
    return out

def _jsonable(x):
    try:
        json.dumps(x); return x
    except Exception:
        return str(x)

def _scan_number(native_id, fallback):
    m = re.search(r"scan=(\d+)", native_id or "")
    return int(m.group(1)) if m else fallback

def _peaks(mz, it, mz_bits):
    """Hashes and summaries of a peak list, as openreadout computes them (m/z f64 LE, intensity f32 LE)."""
    mz = np.asarray(mz, dtype=np.float64)
    it = np.asarray(it, dtype=np.float32)
    nz = it != 0
    return {
        "n_peaks": int(mz.size),
        "n_nonzero": int(nz.sum()),
        "mz_bits": mz_bits,
        "xxh3_mz": xxhash.xxh3_128_hexdigest(mz.astype("<f8").tobytes()),
        "xxh3_intensity": xxhash.xxh3_128_hexdigest(it.astype("<f4").tobytes()),
        "xxh3_mz_nonzero": xxhash.xxh3_128_hexdigest(mz[nz].astype("<f8").tobytes()),
        "xxh3_intensity_nonzero": xxhash.xxh3_128_hexdigest(it[nz].astype("<f4").tobytes()),
        "sum_intensity": float(it.astype(np.float64).sum()),
        "mz_min": float(mz.min()) if mz.size else None,
        "mz_max": float(mz.max()) if mz.size else None,
        "first_peaks": [[float(a), float(b)] for a, b in zip(mz[:5], it[:5])],
        "first_nonzero_peaks": [[float(a), float(b)] for a, b in zip(mz[nz][:5], it[nz][:5])],
    }

def _paired_export(p: Path):
    for ext in (".mzML", ".mzml", ".mzXML", ".mzxml"):
        q = p.with_suffix(ext)
        if q.exists():
            return q
    raise FileNotFoundError(f"no depositor mzML/mzXML next to {p.name}")

def _mzml_scans(q: Path, reader=None, skip_detectors=False):
    """Walk an mzML with pyteomics; also read each m/z array's bit depth from the raw XML.
    `skip_detectors`: leave out LC-detector spectra (native id `controllerType=N`, N != 0)."""
    from pyteomics import mzml
    from thermo_detectors import is_detector_id
    scans = []
    with (reader or mzml.MzML(str(q), decode_binary=True)) as f:
        for i, sp in enumerate(f):
            if skip_detectors and is_detector_id(sp.get("id")):
                continue
            # UV/Vis (photodiode-array) spectra: a wavelength array and no m/z array
            if sp.get("m/z array") is None and sp.get("wavelength array") is not None:
                continue
            scan = sp.get("scanList", {}).get("scan", [{}])[0]
            rt = scan.get("scan start time")
            unit = getattr(rt, "unit_info", "minute")
            rt_s = float(rt) * (60.0 if unit in ("minute", None) else 1.0) if rt is not None else None
            prec = charge = iso = energy = None
            act = None
            pl = sp.get("precursorList", {}).get("precursor", [])
            if pl:
                si = pl[-1].get("selectedIonList", {}).get("selectedIon", [{}])[0]
                prec = si.get("selected ion m/z")
                charge = si.get("charge state")
                iso = pl[-1].get("isolationWindow", {}).get("isolation window target m/z")
                a = pl[-1].get("activation", {})
                energy = a.get("collision energy")
                act = sorted(k for k in a if k != "collision energy")
            # ProteoWizard gives a constant neutral loss scan's loss as its selected ion: it is
            # no precursor m/z
            loss = None
            if "constant neutral loss spectrum" in sp and prec is not None:
                loss, prec = prec, None
            mza = sp.get("m/z array")
            bits = int(np.asarray(mza).dtype.itemsize * 8) if mza is not None and len(mza) else None
            polarity = "positive" if "positive scan" in sp else "negative" if "negative scan" in sp else "unknown"
            scans.append({
                "index": i,
                "scan_number": _scan_number(sp.get("id"), i + 1),
                "native_id": sp.get("id"),
                "ms_level": int(sp.get("ms level", 0)),
                "rt_s": rt_s,
                "polarity": polarity,
                "centroided": "centroid spectrum" in sp,
                "filter": scan.get("filter string"),
                "precursor_mz": float(prec) if prec is not None else None,
                "precursor_charge": int(charge) if charge is not None else None,
                **({"neutral_loss_mz": float(loss)} if loss is not None else {}),
                "isolation_target_mz": float(iso) if iso is not None else None,
                "activation": act,
                "collision_energy": float(energy) if energy is not None else None,
                "total_ion_current": float(sp["total ion current"]) if "total ion current" in sp else None,
                "base_peak_mz": float(sp["base peak m/z"]) if "base peak m/z" in sp else None,
                **_peaks(sp.get("m/z array", []), sp.get("intensity array", []), bits),
            })
    return scans

def _mzml_chromatograms(q: Path):
    """Chromatograms of an mzML (SRM runs are exported as these instead of spectra): per trace
    the kind, polarity, precursor/product targets and a summary of its (time, intensity) points."""
    from pyteomics import mzml
    out = []
    with mzml.MzML(str(q), decode_binary=True) as f:
        for i, c in enumerate(f.iterfind("chromatogram")):
            t = np.asarray(c.get("time array", []), dtype=np.float64)
            it = np.asarray(c.get("intensity array", []), dtype=np.float32)
            def first(v):
                return (v[0] if v else {}) if isinstance(v, list) else (v or {})
            prec = first(c.get("precursor")).get("isolationWindow", {})
            energy = first(c.get("precursor")).get("activation", {}).get("collision energy")
            prod = first(c.get("product")).get("isolationWindow", {})
            kind = ("tic" if "total ion current chromatogram" in c
                    else "bpc" if "basepeak chromatogram" in c
                    else "srm" if "selected reaction monitoring chromatogram" in c else "other")
            polarity = "positive" if "positive scan" in c else "negative" if "negative scan" in c else "unknown"
            def f_(d, k):
                return float(d[k]) if k in d else None
            out.append({
                "index": i,
                "id": c.get("id"),
                "kind": kind,
                "polarity": polarity,
                "precursor_mz": f_(prec, "isolation window target m/z"),
                "product_mz": f_(prod, "isolation window target m/z"),
                "product_lower_offset": f_(prod, "isolation window lower offset"),
                "product_upper_offset": f_(prod, "isolation window upper offset"),
                "collision_energy": float(energy) if energy is not None else None,
                "n_points": int(t.size),
                "xxh3_time_min": xxhash.xxh3_128_hexdigest(t.astype("<f8").tobytes()),
                "xxh3_intensity": xxhash.xxh3_128_hexdigest(it.astype("<f4").tobytes()),
                "sum_intensity": float(it.astype(np.float64).sum()),
                "first_points": [[float(a), float(b)] for a, b in zip(t[:5], it[:5])],
                # ProteoWizard pads some traces with zero points from neighbouring transitions;
                # the non-zero points alone are what a reader has to reproduce.
                "n_nonzero": int((it != 0).sum()),
                "xxh3_time_min_nonzero": xxhash.xxh3_128_hexdigest(t[it != 0].astype("<f8").tobytes()),
                "xxh3_intensity_nonzero": xxhash.xxh3_128_hexdigest(it[it != 0].astype("<f4").tobytes()),
            })
    return out

def _mzxml_reader():
    """pyteomics' MzXML with msInstrumentID read as text: ProteoWizard writes ids like "IC1"
    where the mzXML 3.2 schema (and pyteomics' defaults) expect an integer."""
    import copy
    from pyteomics import mzxml

    class LenientMzXML(mzxml.MzXML):
        _default_schema = copy.deepcopy(mzxml.MzXML._default_schema)
        _default_schema["ints"] = {k for k in _default_schema["ints"] if k[1] != "msInstrumentID"}

    return LenientMzXML

def _mzxml_scans(q: Path, bits):
    # mzXML prints retention times as xs:duration text with limited digits (e.g. PT1000.58S);
    # record half a unit of the last printed digit so comparisons can allow for the rounding.
    import mmap
    rt_tol = []
    with open(q, "rb") as fh, mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ) as mm:
        for m in re.finditer(rb'retentionTime="PT([0-9.]+)S"', mm):
            txt = m.group(1).decode()
            dec = len(txt.split(".")[1]) if "." in txt else 0
            rt_tol.append(0.5 * 10 ** -dec)
    scans = []
    with _mzxml_reader()(str(q), decode_binary=True) as f:
        for i, sp in enumerate(f):
            pl = sp.get("precursorMz") or []
            prec = pl[-1] if pl else {}
            rt = sp.get("retentionTime")
            scans.append({
                "index": i,
                "scan_number": int(sp.get("num", i + 1)),
                "ms_level": int(sp.get("msLevel", 0)),
                "rt_s": float(rt) * 60.0 if rt is not None else None,  # pyteomics reports minutes
                "rt_tolerance_s": rt_tol[i] if i < len(rt_tol) else None,
                "polarity": {"+": "positive", "-": "negative"}.get(sp.get("polarity"), "unknown"),
                "centroided": bool(int(sp.get("centroided", 0))),
                "filter": sp.get("filterLine"),
                "precursor_mz": float(prec["precursorMz"]) if prec.get("precursorMz") is not None else None,
                "precursor_charge": int(prec["precursorCharge"]) if prec.get("precursorCharge") is not None else None,
                "activation": prec.get("activationMethod"),
                "collision_energy": float(sp["collisionEnergy"]) if sp.get("collisionEnergy") is not None else None,
                "total_ion_current": float(sp["totIonCurrent"]) if sp.get("totIonCurrent") is not None else None,
                "base_peak_mz": float(sp["basePeakMz"]) if sp.get("basePeakMz") is not None else None,
                **_peaks(sp.get("m/z array", []), sp.get("intensity array", []), bits),
            })
    return scans

def _mzml_traces(q: Path, reader=None):
    """Chromatograms of an mzML read with pyteomics, as oracle traces: channels `time` (seconds),
    `intensity`, then any other arrays in document order (named as pyteomics names them). The
    time unit is read from the time array's cvParam with ElementTree (pyteomics does not attach
    units to arrays); a missing unit is taken as minutes, as for scan start times."""
    import xml.etree.ElementTree as ET
    from pyteomics import mzml
    units = []
    for _, el in ET.iterparse(str(q), events=("end",)):
        tag = el.tag.rsplit("}", 1)[-1]
        if tag == "chromatogram":
            u = None
            for cv in el.iter():
                if cv.tag.endswith("cvParam") and cv.get("accession") == "MS:1000595":
                    u = cv.get("unitName") or cv.get("unitAccession")
            units.append(u)
            el.clear()
        elif tag == "spectrum":
            el.clear()
    scale = {"minute": 60.0, "UO:0000031": 60.0, None: 60.0, "second": 1.0, "UO:0000010": 1.0, "millisecond": 0.001, "UO:0000028": 0.001}
    out = []
    with (reader or mzml.MzML(str(q), decode_binary=True)) as f:
        for i, c in enumerate(f.iterfind("chromatogram")):
            arrays = [(k, np.asarray(v, dtype=np.float64)) for k, v in c.items() if isinstance(v, np.ndarray)]
            order = {"time array": 0, "intensity array": 1}
            arrays.sort(key=lambda kv: order.get(kv[0], 2))
            names, chans = [], []
            n = int(c.get("defaultArrayLength", 0))
            if n == 0 and arrays:
                # mzMLb writers may leave defaultArrayLength 0 when the arrays are external
                n = max(len(v) for _, v in arrays)
            for k, v in arrays:
                if k == "time array":
                    v = v * scale.get(units[i] if i < len(units) else None, 60.0)
                names.append({"time array": "time", "intensity array": "intensity"}.get(k, k))
                chans.append({"xxh3": _trace_hash(v), "first": _first(v)})
            out.append({"index": i, "id": c.get("id"), "sweep_count": 1, "channel_count": len(chans), "channel_names": names,
                        "sample_rate_hz": 0.0, "sweeps": [{"sweep": 0, "sample_count": n, "channels": chans}]})
    return out

def mzml_(p: Path) -> dict:
    """mzML read with pyteomics (Apache-2.0): every spectrum (see _mzml_scans) and chromatogram.
    `by_index`: the harness addresses spectra by position, not by scan number."""
    from importlib.metadata import version as pkg_version
    scans = _mzml_scans(p)
    return {
        "reader": f"pyteomics {pkg_version('pyteomics')} MzML",
        "images": [],
        "traces": _mzml_traces(p),
        "spectra": {"scan_count": len(scans), "by_index": True, "scans": scans},
    }

def mzmlb_(p: Path) -> dict:
    """mzMLb read with pyteomics' `mzmlb.MzMLb` (Apache-2.0; h5py BSD-3, hdf5plugin MIT for the
    Blosc filter). pyteomics returns MS-Numpress arrays of mzMLb as their stored bytes; those
    are decoded with pynumpress (the reference MS-Numpress implementation, Apache-2.0), chosen
    by the compression term the XML gives the array. Chromatogram time units come from the XML
    (the `mzML` dataset, written to a temporary file for ElementTree)."""
    import tempfile
    import h5py
    try:
        import hdf5plugin  # noqa: F401  (registers the Blosc filter with h5py)
    except ImportError:
        pass
    from importlib.metadata import version as pkg_version
    from pyteomics import mzmlb
    with h5py.File(p, "r") as h:
        xml = bytes(np.asarray(h["mzML"][:], dtype=np.int8).view(np.uint8))
    codecs = {}
    for m in re.finditer(rb"<binaryDataArray\b.*?</binaryDataArray>", xml, re.S):
        block = m.group(0)
        kind = re.search(rb'accession="(MS:100051[45]|MS:1000595|MS:1000786)"', block)
        np_term = re.search(rb'name="MS-Numpress (linear prediction|positive integer|short logged float) compression"', block)
        if kind and np_term:
            codecs[kind.group(1).decode()] = np_term.group(1).decode()
    names = {"MS:1000514": "m/z array", "MS:1000515": "intensity array", "MS:1000595": "time array"}

    def decode(arr, how):
        import pynumpress
        raw = np.asarray(arr, dtype=np.uint8)
        f = {"linear prediction": pynumpress.decode_linear, "positive integer": pynumpress.decode_pic,
             "short logged float": pynumpress.decode_slof}[how]
        return np.asarray(f(raw), dtype=np.float64)

    class Decoded:
        def __init__(self, reader):
            self.r = reader
        def __enter__(self):
            self.r.__enter__()
            return self
        def __exit__(self, *a):
            return self.r.__exit__(*a)
        def _fix(self, sp):
            # an array that names no dataset (see lenient) holds nothing: leave it out
            for k in [k for k, v in sp.items() if isinstance(v, np.ndarray) and v.size == 0]:
                if any(isinstance(v, np.ndarray) and v.size for v in sp.values()):
                    del sp[k]
            for acc, how in codecs.items():
                k = names.get(acc)
                if k in sp and np.asarray(sp[k]).dtype == np.uint8:
                    sp[k] = decode(sp[k], how)
            return sp
        def __iter__(self):
            for sp in self.r:
                yield self._fix(sp)
        def iterfind(self, what):
            for c in self.r.iterfind(what):
                yield self._fix(c)

    def lenient(reader):
        # mzdata's writer names no dataset (value "") for an empty non-standard array
        # (arrayLength="0"); h5py refuses the empty name, so such arrays read as empty.
        get = reader._array_registry.get
        reader._array_registry.get = lambda name, length, offset=0: (
            np.array([], dtype=np.float64) if name == "" else get(name, length, offset))
        return reader

    scans = _mzml_scans(p, Decoded(lenient(mzmlb.MzMLb(str(p)))))
    with tempfile.TemporaryDirectory() as td:
        xp = Path(td) / (p.stem + ".mzML")
        xp.write_bytes(xml)
        traces = _mzml_traces(xp, Decoded(lenient(mzmlb.MzMLb(str(p)))))
    return {
        "reader": f"pyteomics {pkg_version('pyteomics')} mzmlb.MzMLb"
                  + (" + pynumpress" if codecs else ""),
        "images": [],
        "traces": traces,
        "spectra": {"scan_count": len(scans), "by_index": True, "scans": scans},
    }

def gz_(p: Path) -> dict:
    """A gzip-compressed mzML/mzXML: decompressed with Python's `gzip` module to a temporary
    file, then read with pyteomics exactly as the plain file would be (mzml_ / mzxml_)."""
    import gzip
    import shutil
    import tempfile
    inner = p.name[:-3] if p.name.lower().endswith(".gz") else p.name + ".raw"
    with tempfile.TemporaryDirectory() as td:
        q = Path(td) / inner
        with gzip.open(p, "rb") as src, q.open("wb") as dst:
            shutil.copyfileobj(src, dst, 1 << 20)
        ext = q.suffix.lower()
        if ext.lstrip(".") in ("mrc", "mrcs", "map", "rec", "st", "ali", "ccp4"):
            data = mrc(q)
        else:
            data = mzxml_(q) if ext == ".mzxml" else mzml_(q)
    data["reader"] += " on the gzip-decompressed file (Python gzip module)"
    return data

def imzml_(p: Path) -> dict:
    """imzML read with pyimzML (Apache-2.0): every pixel's spectrum in file order, with its
    coordinates. pyimzML returns the arrays in their stored dtype (32-bit floats in the corpus)."""
    from importlib.metadata import version as pkg_version
    from pyimzml.ImzMLParser import ImzMLParser
    scans = []
    # pyimzML does not report the MS level: take it from the file's own cvParams (`MS1 spectrum`
    # or a non-zero `ms level`, usually in a referenceableParamGroup); 0 when the file states neither
    text = p.read_text(encoding="utf-8", errors="replace")
    lv = re.search(r'accession="MS:1000511"[^>]*value="(\d+)"', text)
    ms_level = int(lv.group(1)) if lv else 0
    if ms_level == 0 and 'accession="MS:1000579"' in text:
        ms_level = 1
    with ImzMLParser(str(p)) as parser:
        for i, xyz in enumerate(parser.coordinates):
            mz, it = parser.getspectrum(i)
            scans.append({
                "index": i, "scan_number": i + 1, "ms_level": ms_level, "rt_s": None, "polarity": None,
                "centroided": None, "filter": None, "precursor_mz": None, "precursor_charge": None,
                "activation": None, "position": list(xyz),
                **_peaks(mz, it, int(np.asarray(mz).dtype.itemsize * 8)),
            })
    return {
        "reader": f"pyimzml {pkg_version('pyimzml')} ImzMLParser",
        "images": [],
        "spectra": {"scan_count": len(scans), "by_index": True, "scans": scans},
    }

def mzxml_(p: Path) -> dict:
    """mzXML read with pyteomics (Apache-2.0). pyteomics reports a scan's `centroided` attribute
    only; the mzXML schema makes `dataProcessing centroided="1"` the default for scans without
    one, so that default is applied here explicitly (from the raw XML)."""
    from importlib.metadata import version as pkg_version
    head = p.open("rb").read(200_000).decode("utf-8", "replace")
    scans = _mzxml_scans(p, 64 if 'precision="64"' in head else 32)
    default_centroided = bool(re.search(r'<dataProcessing[^>]*centroided="1"', head))
    raw = p.read_bytes()
    for s_, m in zip(scans, re.finditer(rb"<scan\b[^>]*>", raw)):
        if b"centroided=" not in m.group(0):
            s_["centroided"] = default_centroided
    return {
        "reader": f"pyteomics {pkg_version('pyteomics')} MzXML",
        "images": [],
        "spectra": {"scan_count": len(scans), "by_index": True, "scans": scans},
    }

def thermo(p: Path, q: Path = None) -> dict:
    """Ground truth for a Thermo .raw file = the depositor's own open-format conversion of it.
    No reader of the .raw itself is run (none may be: see docs/provenance/thermo-raw.md)."""
    from importlib.metadata import version as pkg_version
    q = q or _paired_export(p)
    head = q.open("rb").read(200_000).decode("utf-8", "replace")
    software = [list(x) for x in re.findall(r'<software id="([^"]*)" version="([^"]*)"', head)]
    software += [list(x) for x in re.findall(r'<software type="([^"]*)" name="([^"]*)" version="([^"]*)"', head)]
    chromatograms = []
    detectors = None
    if q.suffix.lower() == ".cdf":
        return thermo_andi(p, q)
    if q.suffix.lower() == ".mzml":
        # LC-detector spectra (native ids `controllerType=N`, N != 0; e.g. a PDA's absorbance
        # spectra) share scan numbers with the MS scans: summarized apart (oracle/thermo_detectors.py)
        scans = _mzml_scans(q, skip_detectors=True)
        from thermo_detectors import detector_export, has_detector_ids
        if has_detector_ids(q):
            detectors = detector_export(q)
        if not scans:
            chromatograms = _mzml_chromatograms(q)
    else:
        scans = _mzxml_scans(q, 32 if 'precision="32"' in head else 64)
        # A scan without a `centroided` attribute takes the dataProcessing default; when that
        # is absent too (ReAdW writes neither), the export does not say, and nothing is compared.
        default_centroided = re.search(r'<dataProcessing[^>]*centroided="([01])"', head)
        for s_, m in zip(scans, re.finditer(rb"<scan\b[^>]*>", q.read_bytes())):
            if b"centroided=" not in m.group(0):
                s_["centroided"] = default_centroided.group(1) == "1" if default_centroided else None
    # ORACLE_THERMO_MAX_SCANS keeps the committed JSON small for long runs: the leading part of
    # the export (the harness compares an export's scans by scan number, so a prefix is enough).
    keep = int(os.environ.get("ORACLE_THERMO_MAX_SCANS", "0"))
    truncated = bool(keep) and len(scans) > keep
    if truncated:
        scans = scans[:keep]
    out = {
        "reader": f"pyteomics {pkg_version('pyteomics')} on the depositor's {q.suffix[1:]} ({q.name})",
        "paired_export": q.name,
        "export_software": software,
        "peak_picking_in_export": "peak picking" in head.lower(),
        "images": [],
    }
    if detectors is not None:
        out["detector_export"] = detectors
    if scans:
        if truncated:  # (before `spectra`: the writer keeps `spectra` last)
            out["oracle_note"] = f"the first {keep} spectra of the export (ORACLE_THERMO_MAX_SCANS)"
        out["spectra"] = {"scan_count": len(scans), "scans": scans}
        # MS-Numpress positive-integer compression rounds intensities to whole numbers
        # (the harness rounds ours the same way before comparing).
        if "MS-Numpress positive integer compression" in head:
            out["spectra"] = {"intensity_encoding": "numpress-pic", **out["spectra"]}
    else:
        out["chromatograms"] = chromatograms
    return out
def thermo_andi(p: Path, q: Path) -> dict:
    """Ground truth for a Thermo .raw whose depositor exported it to ANDI-MS (AIA netCDF, the
    vendor's own converter), read with scipy.io.netcdf_file (BSD-3). The export stores intensities
    as long integers and keeps only the points inside each scan's mass range: both are recorded so
    the comparison treats our values the same way. ANDI-MS has no MS level (its scans are full
    scans)."""
    import scipy.io
    f = scipy.io.netcdf_file(str(q), "r", mmap=False)
    A = {k: (v.decode("latin-1") if isinstance(v, bytes) else v) for k, v in f._attributes.items()}
    V = f.variables
    t = np.asarray(V["scan_acquisition_time"].data, dtype=np.float64)
    idx = np.asarray(V["scan_index"].data, dtype=np.int64)
    cnt = np.asarray(V["point_count"].data, dtype=np.int64)
    mz = np.asarray(V["mass_values"].data, dtype=np.float64)
    it = np.asarray(V["intensity_values"].data, dtype=np.float64)
    lo = np.asarray(V["mass_range_min"].data, dtype=np.float64)
    hi = np.asarray(V["mass_range_max"].data, dtype=np.float64)
    tic = np.asarray(V["total_intensity"].data, dtype=np.float64)
    pol = str(A.get("test_ionization_polarity", "")).lower()
    pol = "positive" if pol.startswith("pos") else "negative" if pol.startswith("neg") else None
    cen = str(A.get("experiment_type", "")).lower().startswith("centroid")
    scans = []
    for i in range(len(t)):
        a, b = idx[i], idx[i] + cnt[i]
        scans.append({
            "index": i, "scan_number": i + 1, "ms_level": 1, "rt_s": float(t[i]), "polarity": pol,
            "centroided": cen, "filter": None, "precursor_mz": None, "precursor_charge": None,
            "activation": None, "total_ion_current": float(tic[i]),
            "mz_window": [float(lo[i]), float(hi[i])],
            **_peaks(mz[a:b], it[a:b], 64),
        })
    return {
        "reader": f"scipy.io.netcdf_file (scipy {__import__('scipy').__version__}) on the depositor's ANDI-MS export ({q.name})",
        "paired_export": q.name,
        "export_software": [[str(A.get("source_file_format", "")), str(A.get("dataset_origin", ""))]],
        "images": [],
        "spectra": {"intensity_encoding": "integer", "scan_count": len(scans), "scans": scans},
    }


def agilent_ms(p: Path, q: Path) -> dict:
    """Ground truth for an Agilent MassHunter .d = the depositor's own mzML conversion of it.
    No reader of the .d is run (none may be: see docs/provenance/agilent-masshunter.md). The
    export numbers scans `scanId=N`; they are compared by position and native id."""
    out = thermo(p, q)
    if "spectra" in out:
        out["spectra"]["by_index"] = True
    return out


def waters_chromatogram_export(p: Path, q: Path) -> dict:
    """A Waters MRM .raw whose depositor export (mzXML) holds its chromatograms as one-point
    "scans" with m/z 0 and precursor 0: the stored TIC, then one block per transition, each
    block with the function's scan times (minutes, although labelled seconds). Rebuilt as the
    function-1 table openreadout reports; column names from _FUNCTNS.INF (as in waters()), the
    values from the export only. No reader of the .raw is run."""
    import struct
    rd = _mzxml_reader()
    ints = []
    with rd(str(q), decode_binary=True) as f:
        for sp in f:
            it = sp.get("intensity array", [])
            ints.append(float(it[0]) if len(it) else 0.0)
    # the printed values (pyteomics divides the "seconds" by 60); they are minutes
    times = [float(t) for t in re.findall(rb'retentionTime="PT([0-9.eE+-]+)S"', q.read_bytes())]
    n = next((i for i in range(1, len(times)) if times[i] < times[i - 1]), len(times))
    if len(ints) % n:
        raise ValueError(f"{len(ints)} export scans are not whole blocks of {n}")
    blocks = np.asarray(ints, dtype=np.float64).reshape(-1, n)
    fn = (p / "_FUNCTNS.INF").read_bytes()
    a = [x for x in struct.unpack_from("<32f", fn, 0xA0)]
    b = [x for x in struct.unpack_from("<32f", fn, 0x120)]
    a = a[:next((i for i, v in enumerate(a) if v == 0), len(a))]
    b = b[:next((i for i, v in enumerate(b) if v == 0), len(b))]
    if len(a) != blocks.shape[0] - 1:
        raise ValueError(f"{blocks.shape[0] - 1} transition blocks, _FUNCTNS.INF lists {len(a)} masses")
    fm = lambda v: str(np.float32(v)).removesuffix(".0")
    names = [f"{fm(a[i])} > {fm(b[i])}" if i < len(b) else fm(a[i]) for i in range(len(a))]
    hashes = {
        "rt_min": _col_hash(np.asarray(times[:n], dtype=np.float32).astype(np.float64)),
        "tic_stored": _col_hash(blocks[0]),
    }
    for k, nme in enumerate(names):
        hashes[nme] = _col_hash(blocks[k + 1])
    return {"reader": f"pyteomics MzXML on the depositor's export ({q.name})",
            "paired_export": q.name,
            "tables": [{"index": 0, "event_count": n, "parameter_names": [], "column_hashes": hashes}],
            "oracle_note": "chromatograms exported as one-point scans (m/z 0); times are minutes"}


def waters_ms(p: Path, q: Path) -> dict:
    """Ground truth for a Waters MassLynx .raw with scanning functions = the depositor's own
    mzML conversion of it. No reader of the .raw is run for spectra (see
    docs/provenance/waters-raw.md). The export lists the scans of all functions merged by
    retention time with native ids `function=F process=0 scan=S`; when it holds more than one
    function the spectra are compared by position (scan numbers restart per function)."""
    head = q.open("rb").read(200_000)
    if q.suffix.lower() == ".mzxml" and b'<precursorMz precursorIntensity="0">0</precursorMz>' in head:
        return waters_chromatogram_export(p, q)
    out = thermo(p, q)
    sp = out.get("spectra")
    if sp:
        funcs = {str(s.get("native_id", "")).split(" ")[0] for s in sp["scans"]}
        if len(funcs) > 1:
            sp["by_index"] = True
    return out


def sciex_wiff(p: Path, q: Path) -> dict:
    """Ground truth for a Sciex .wiff (+ .wiff.scan) = the depositor's own conversion of it.
    No reader of the .wiff is run (see docs/provenance/sciex-wiff.md). Spectra are compared by
    position (native ids `sample=1 period=1 cycle=C experiment=E`). ProteoWizard writes
    collision energy 0.0 on every SRM chromatogram of Analyst MRM files while the method holds
    the real values: that placeholder is dropped here."""
    out = thermo(p, q)
    if "spectra" in out:
        out["spectra"]["by_index"] = True
    for c in out.get("chromatograms", []):
        if c.get("collision_energy") == 0.0:
            c["collision_energy"] = None
    return out


def _is_blackrock(p: Path) -> bool:
    with open(p, "rb") as f:
        return f.read(8) in (b"NEURALEV", b"BREVENTS")

def _sorted_hash(values) -> str:
    return _col_hash(np.sort(np.asarray(values, dtype=np.float64)))

def blackrock(p: Path) -> dict:
    """Blackrock via Neo's BlackrockRawIO (BSD-3). The file is copied alone into a temporary
    directory (plus the same-name .nev for spec 2.1 NSx files, whose scaling lives there).

    NSx: one trace; Neo segments (data packets) are sweeps; values raw x gain + offset. Neo drops
    the last whole sample of spec 2.1 files, so for those `sample_count` is the file's own count
    ((size - header) / frame) and the hash covers Neo's samples (`hashed_samples`).
    NEV: one table of packets; Neo regroups spikes by unit, so spike rows are compared as sorted
    columns (time, electrode, unit class, first waveform sample) with their count; the total row
    count is the file's (size - bytes in headers) / bytes per packet."""
    import neo, tempfile, shutil, struct
    from neo.rawio import BlackrockRawIO
    ext = p.suffix.lower()
    d = tempfile.mkdtemp(prefix="openreadout-oracle-")
    shutil.copy(p, d)
    with open(p, "rb") as f:
        head = f.read(64)
    out = {"reader": f"neo {neo.__version__} BlackrockRawIO"}
    if ext.startswith(".ns"):
        spec21 = head[:8] == b"NEURALSG"
        nev = p.with_suffix(".nev")
        if spec21:
            if not nev.exists():
                raise RuntimeError("spec 2.1 NSx without its .nev: Neo cannot scale it")
            shutil.copy(nev, d)
        r = BlackrockRawIO(filename=str(Path(d) / p.stem), nsx_to_load=int(ext[3:]), load_nev=spec21)
        r.parse_header()
        sc = r.header["signal_channels"]
        gain = [float(c["gain"]) for c in sc]
        off = [float(c["offset"]) for c in sc]
        sweeps = _neo_trace_sweeps(r, 0, gain, off)
        if spec21:
            nch = struct.unpack_from("<I", head, 28)[0]
            exact = (p.stat().st_size - (32 + 4 * nch)) // (2 * nch)
            for s in sweeps:
                s["hashed_samples"] = s["sample_count"]
                s["sample_count"] = int(exact)
        out["traces"] = [{"index": 0, "sweep_count": int(r.header["nb_segment"][0]), "channel_count": len(sc),
                          "channel_names": [str(c["name"]) for c in sc], "channel_units": [str(c["units"]) for c in sc],
                          "sample_rate_hz": float(sc[0]["sampling_rate"]), "sweeps": sweeps}]
        return out
    # NEV alone
    r = BlackrockRawIO(filename=str(Path(d) / p.stem), nsx_to_load=[], load_nev=True)
    r.parse_header()
    h = r.header
    res = float(struct.unpack_from("<I", head, 20)[0])
    hdr, plen = struct.unpack_from("<II", head, 12)
    t, e, u, w0 = [], [], [], []
    for ui, ch in enumerate(h["spike_channels"]):
        name = str(ch["name"])  # ch<electrode>#<unit>
        elec = int(name[2:].split("#")[0])
        unit = int(name.split("#")[1])
        g = float(ch["wf_gain"])
        for s in range(h["nb_segment"][0]):
            ts = r.get_spike_timestamps(0, s, ui, None, None)
            wf = r.get_spike_raw_waveforms(0, s, ui, None, None)
            t += [int(x) / res for x in ts]
            e += [elec] * len(ts)
            u += [unit] * len(ts)
            if wf is not None and len(ts):
                w0 += list(np.asarray(wf[:, 0, 0], dtype=np.float64) * g)
    spikes = [
        {"column": "time_s", "where_column": "kind", "where_value": 1.0, "count": len(t), "xxh3": _sorted_hash(t)},
        {"column": "packet_id", "where_column": "kind", "where_value": 1.0, "count": len(e), "xxh3": _sorted_hash(e)},
        {"column": "code", "where_column": "kind", "where_value": 1.0, "count": len(u), "xxh3": _sorted_hash(u)},
        {"column": "w0", "where_column": "kind", "where_value": 1.0, "count": len(w0), "xxh3": _sorted_hash(w0)},
    ]
    out["tables"] = [{"index": 0, "event_count": (p.stat().st_size - hdr) // plen, "parameter_names": [], "dtypes": [],
                      "xxh3": None, "sorted_column_hashes": spikes}]
    return out

def spikeglx(p: Path) -> dict:
    """SpikeGLX via Neo's SpikeGLXRawIO (BSD-3): the .bin and its .meta are copied alone into a
    temporary directory. One trace; channels = Neo's main stream followed by its `-SYNC` stream.
    Status (SY) and digital (XD) words are compared raw (Neo scales them like voltages); for an
    imec stream without a SY channel Neo gives the last channel the SY gain, so that channel takes
    the gain of the channel before it."""
    import neo, tempfile, shutil
    from neo.rawio import SpikeGLXRawIO
    d = tempfile.mkdtemp(prefix="openreadout-oracle-")
    shutil.copy(p, d)
    shutil.copy(p.with_suffix(".meta"), d)
    r = SpikeGLXRawIO(dirname=d)
    r.parse_header()
    streams = [str(s) for s in r.header["signal_streams"]["id"]]
    main = [s for s in streams if not s.endswith("-SYNC")]
    if len(main) != 1:
        raise RuntimeError(f"expected one stream, Neo found {streams}")
    trace = _sglx_trace(r, main[0], 0)
    trace.update(_sglx_sites(p.with_suffix(".meta"), trace["channel_names"]))
    return {"reader": f"neo {neo.__version__} SpikeGLXRawIO", "traces": [trace]}

def _sglx_sites(meta: Path, names) -> dict:
    """Probe sites of the neural channels from probeinterface's `read_spikeglx` (MIT), which
    places contacts from its own probe tables and the imro table: `shank` and `z_um` (its y)
    exactly; `x_um` up to one constant (probeinterface measures x from the left-most column
    and adds shank × pitch; SpikeGLX's ~snsGeomMap from the shank's edge). Only for streams
    whose .meta has a ~snsGeomMap (the µm sites openreadout reports); nothing otherwise."""
    text = meta.read_text(errors="replace")
    if "~snsGeomMap=" not in text or not any(n.startswith(("AP", "LF")) for n in names):
        return {}
    import probeinterface as pi
    pr = pi.read_spikeglx(str(meta))
    pos = np.asarray(pr.contact_positions, dtype=np.float64)
    shank = [int(x) for x in pr.shank_ids] if pr.shank_ids is not None and len(pr.shank_ids) and pr.shank_ids[0] != "" \
        else [0] * len(pos)
    header = text.split("~snsGeomMap=(", 1)[1].split(")", 1)[0].split(",")
    pitch = float(header[2])
    neural = [i for i, n in enumerate(names) if n.startswith(("AP", "LF"))]
    if len(neural) != len(pos):
        raise RuntimeError(f"probeinterface found {len(pos)} contacts for {len(neural)} neural channels")
    extra = [{} for _ in names]
    for k, i in enumerate(neural):
        extra[i] = {"shank": shank[k], "x_um": float(pos[k, 0] - shank[k] * pitch), "z_um": float(pos[k, 1])}
    return {"channel_extra": extra, "channel_extra_offset_keys": ["x_um"],
            "sites_reader": f"probeinterface {pi.__version__}"}

def _sglx_trace(r, stream: str, index: int) -> dict:
    """One openreadout trace from Neo stream `stream` (plus its `-SYNC` stream), segment 0."""
    h = r.header
    streams = [str(s) for s in h["signal_streams"]["id"]]
    order = [streams.index(stream)] + [i for i, s in enumerate(streams) if s == stream + "-SYNC"]
    sc_all = h["signal_channels"]
    names, units, gains, offs, raws = [], [], [], [], []
    n = r.get_signal_size(0, 0, order[0])
    # long streams (> 2^20 samples): hash the first 2^17 samples of every channel (`hashed_samples`)
    keep = n if n <= (1 << 20) else (1 << 17)
    for si in order:
        sc = sc_all[sc_all["stream_id"] == streams[si]]
        raw = np.asarray(r.get_analogsignal_chunk(0, 0, 0, keep, si), dtype=np.float64)
        for c in range(len(sc)):
            name = str(sc[c]["name"])
            bits = name.startswith(("SY", "XD"))
            names.append(name)
            units.append("" if bits else str(sc[c]["units"]))
            gains.append(1.0 if bits else float(sc[c]["gain"]))
            offs.append(0.0 if bits else float(sc[c]["offset"]))
            raws.append(raw[:, c])
    if names and not names[-1].startswith("SY") and stream.startswith("imec") and len(gains) > 1:
        gains[-1] = gains[-2]
    chans = []
    for c in range(len(names)):
        y = raws[c] * gains[c] + offs[c]
        chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
    rate = float(sc_all[sc_all["stream_id"] == stream][0]["sampling_rate"])
    sweep = {"sweep": 0, "sample_count": int(n), "channels": chans}
    if keep < n:
        sweep["hashed_samples"] = int(keep)
    return {"index": index, "sweep_count": 1, "channel_count": len(names), "channel_names": names,
            "channel_units": units, "sample_rate_hz": rate, "sweeps": [sweep]}

def _session_files(p: Path, exts, depth=0) -> list:
    """Data files of a recording directory in openreadout's member order (sorted paths)."""
    out = []
    for q in (p.rglob("*") if depth else p.iterdir()):
        rel = q.relative_to(p)
        if len(rel.parts) > depth + 1 or not q.is_file() or q.name.startswith(".") or q.name.endswith(".ok"):
            continue
        if q.suffix.lower()[1:] in exts:
            out.append(q)
    return sorted(out)

def spikeglx_dir(p: Path) -> dict:
    """A SpikeGLX run directory via Neo's SpikeGLXRawIO(dirname=run directory) (BSD-3): one
    trace per stream in openreadout's member order (stream .bin files sorted by path), each
    built like the single-file oracle (main stream + its -SYNC stream, segment 0)."""
    import neo
    from neo.rawio import SpikeGLXRawIO
    r = SpikeGLXRawIO(dirname=str(p))
    r.parse_header()
    traces = []
    for b in _session_files(p, {"bin"}, depth=1):
        if not b.with_suffix(".meta").exists():
            continue
        stream = b.name[:-4].rsplit("_t", 1)[1].split(".", 1)[1]  # <run>_g0_t0.<stream>.bin
        traces.append(_sglx_trace(r, stream, len(traces)))
    return {"reader": f"neo {neo.__version__} SpikeGLXRawIO (run directory)", "traces": traces}

def neuralynx_dir(p: Path) -> dict:
    """A Neuralynx recording directory via Neo's NeuralynxRawIO(dirname=directory) (BSD-3).

    Continuous channels: openreadout joins .ncs files with an identical sample grid (rate, and
    per segment the length and start) into one trace, in file-name order, the trace appearing where
    its first file sorts. The oracle rebuilds that grouping from Neo's per-channel rate and
    segment sizes / start times and hashes Neo's samples (raw x |gain|, as the single-file
    oracle). Event and spike files: one table each, in file-name order, compared with the
    single-file oracle of that file (Neo on the file alone)."""
    import neo
    from neo.rawio import NeuralynxRawIO
    r = NeuralynxRawIO(dirname=str(p))
    try:
        r.parse_header()
    except ValueError as e:
        if "Incompatible section structures" not in str(e):
            raise
        return _neuralynx_dir_per_file(p, str(e))
    h = r.header
    sc = h["signal_channels"]
    streams = [str(s) for s in h["signal_streams"]["id"]]
    nseg = int(h["nb_segment"][0])
    by_file = {}
    for c in sc:
        fn = Path(r.ncs_filenames[(str(c["name"]), str(c["id"]))]).name
        by_file[fn] = c
    groups = []  # [(key, [channel rows])]
    for fn in sorted(by_file):
        c = by_file[fn]
        si = streams.index(str(c["stream_id"]))
        key = (float(c["sampling_rate"]),
               tuple((int(r.get_signal_size(0, s, si)), float(r.get_signal_t_start(0, s, si))) for s in range(nseg)))
        for k, members in groups:
            if k == key:
                members.append(c)
                break
        else:
            groups.append((key, [c]))
    traces = []
    for key, members in groups:
        sweeps = []
        for s in range(min(nseg, MAX_SWEEPS)):
            chans = []
            n = 0
            for c in members:
                si = streams.index(str(c["stream_id"]))
                stream_chans = sc[sc["stream_id"] == c["stream_id"]]
                ci = [str(x["id"]) for x in stream_chans].index(str(c["id"]))
                n = r.get_signal_size(0, s, si)
                raw = np.asarray(r.get_analogsignal_chunk(0, s, 0, n, si, channel_indexes=[ci]), dtype=np.float64)[:, 0]
                y = raw * abs(float(c["gain"])) + float(c["offset"])
                chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
            sweeps.append({"sweep": s, "sample_count": int(n), "channels": chans})
        traces.append({"index": len(traces), "sweep_count": nseg, "channel_count": len(members),
                       "channel_names": [str(c["name"]) for c in members],
                       "channel_units": [str(c["units"]) for c in members],
                       "sample_rate_hz": key[0], "sweeps": sweeps})
    tables = []
    for q in _session_files(p, {"nev", "nse", "nst", "ntt"}):
        t = neuralynx(q)["tables"][0]
        t["index"] = len(tables)
        tables.append(t)
    out = {"reader": f"neo {neo.__version__} NeuralynxRawIO (directory; tables from each file alone)", "traces": traces}
    if tables:
        out["tables"] = tables
    return out

def _neuralynx_dir_per_file(p: Path, why: str) -> dict:
    """Neo refuses a directory whose channels have different segment structures; openreadout
    keeps them as separate traces. Then each .ncs goes through the single-file oracle and the
    traces are grouped as openreadout groups them (same rate, sweep lengths)."""
    import neo
    groups = []
    for q in _session_files(p, {"ncs"}):
        if q.stat().st_size <= 16384:
            continue
        t = neuralynx(q)["traces"][0]
        key = (t["sample_rate_hz"], tuple(sw["sample_count"] for sw in t["sweeps"]))
        for k, g in groups:
            if k == key:
                g["channel_names"] += t["channel_names"]
                g["channel_units"] += t["channel_units"]
                g["channel_count"] += t["channel_count"]
                for a, b in zip(g["sweeps"], t["sweeps"]):
                    a["channels"] += b["channels"]
                break
        else:
            t.pop("neo_gain", None)
            groups.append((key, t))
    traces = []
    for _, t in groups:
        t["index"] = len(traces)
        traces.append(t)
    tables = []
    for q in _session_files(p, {"nev", "nse", "nst", "ntt"}):
        t = neuralynx(q)["tables"][0]
        t["index"] = len(tables)
        tables.append(t)
    out = {"reader": f"neo {neo.__version__} NeuralynxRawIO (each file alone: {why})", "traces": traces}
    if tables:
        out["tables"] = tables
    return out

def blackrock_dir(p: Path) -> dict:
    """A Blackrock recording directory: each NSx file is a trace and each NEV a table, in file-name
    order; each file through the single-file oracle (Neo's BlackrockRawIO on that file, with the
    NEV of the same name for spec 2.1 NSx scaling)."""
    import neo
    traces, tables = [], []
    for q in _session_files(p, {"nev", "ns1", "ns2", "ns3", "ns4", "ns5", "ns6"}):
        one = blackrock(q)
        for t in one.get("traces", []):
            t["index"] = len(traces)
            traces.append(t)
        for t in one.get("tables", []):
            t["index"] = len(tables)
            tables.append(t)
    out = {"reader": f"neo {neo.__version__} BlackrockRawIO (per file)", "traces": traces}
    if tables:
        out["tables"] = tables
    return out

# openreadout's Intan trace order (data-block order) and Neo's stream ids for each kind
_INTAN_ORDER = {
    ".rhd": [("amplifier", "0"), ("auxiliary", "1"), ("supply", "2"), ("board_adc", "3"), ("digital_in", "4"), ("digital_out", "5")],
    ".rhs": [("amplifier", "0"), ("dc_amplifier", "10"), ("stimulation", "11"), ("board_adc", "3"), ("board_dac", "4"), ("digital_in", "5"), ("digital_out", "6")],
}

def intan(p: Path) -> dict:
    """Intan via Neo's IntanRawIO (BSD-3). One oracle trace per Neo stream, numbered in
    openreadout's data-block order (amplifier, auxiliary, supply, board ADC, digital in/out for
    RHD; amplifier, DC amplifier, stimulation, board ADC/DAC, digital in/out for RHS); values raw
    x gain + offset; digital lines are 0/1 with no unit.

    A directory (the "one file per signal type" / "one file per channel" layouts) is read through
    its info.rhd / info.rhs. In those layouts Neo labels the auxiliary and supply streams with the
    traditional block rates (rate/4, rate/block) although their .dat files hold one sample per
    time index; the oracle leaves those two rates out (`sample_rate_note`)."""
    import neo
    from neo.rawio import IntanRawIO
    split = p.is_dir()
    if split:
        p = next(q for q in (p / "info.rhd", p / "info.rhs") if q.exists())
    r = IntanRawIO(filename=str(p))
    r.parse_header()
    h = r.header
    ids = [str(s) for s in h["signal_streams"]["id"]]
    traces = []
    for kind, sid in _INTAN_ORDER[p.suffix.lower()]:
        if sid not in ids:
            continue
        si = ids.index(sid)
        sc = h["signal_channels"][h["signal_channels"]["stream_id"] == sid]
        digital = kind.startswith("digital")
        gain = [float(c["gain"]) for c in sc]
        off = [float(c["offset"]) for c in sc]
        n = r.get_signal_size(0, 0, si)
        raw = np.asarray(r.get_analogsignal_chunk(0, 0, 0, n, si), dtype=np.float64)
        chans = []
        for c in range(len(sc)):
            y = raw[:, c] * gain[c] + off[c]
            chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
        t = {"index": len(traces), "name": kind, "sweep_count": 1, "channel_count": len(sc),
             "channel_names": [str(c["name"]).removesuffix("_STIM").removesuffix("_DC") for c in sc],
             "channel_units": ["" if digital else str(c["units"]) for c in sc],
             "sample_rate_hz": float(sc[0]["sampling_rate"]),
             "sweeps": [{"sweep": 0, "sample_count": int(n), "channels": chans}]}
        if split and kind in ("auxiliary", "supply"):
            t["sample_rate_hz"] = None
            t["sample_rate_note"] = f"Neo labels this stream {float(sc[0]['sampling_rate'])} Hz but returns one sample per time index"
        traces.append(t)
    other = [s for s in ids if s not in {sid for _, sid in _INTAN_ORDER[p.suffix.lower()]}]
    if other:
        raise RuntimeError(f"Neo streams without an openreadout counterpart: {other}")
    return {"reader": f"neo {neo.__version__} IntanRawIO", "traces": traces}

def _oe_unit(name: str, units: str):
    """OpenReadout's unit rule for Open Ephys binary channels with no unit (docs/formats/open-ephys.md)."""
    if units:
        return units
    if name.startswith(("CH", "AP", "LFP")):
        return "uV"
    if "ADC" in name or name.startswith("AI"):
        return "V"
    return None


def openephys(p: Path) -> dict:
    """Open Ephys recording directory.

    Binary format: the folder structure and every stream's metadata come from Neo's
    OpenEphysBinaryRawIO._parse_folder_structure (BSD-3; `structure.oebin` read by Neo), the
    samples from NumPy memory maps of continuous.dat (int16 x bit_volts, Neo's buffer shape rule),
    grouped the way docs/formats/open-ephys.md describes (node, experiment, folder; recordings as
    sweeps). Where Neo's full API parses the directory, each Neo stream's rescaled chunk is compared
    with the same channels here (`neo_api_agrees`). Events: the .npy files loaded with NumPy.

    Legacy format: Neo's OpenEphysRawIO record layout (`continuous_dtype`, `events_dtype`,
    `read_file_header`, `make_spikes_dtype`) read with NumPy, grouped by node, source, segment and
    run grid (records that step by exactly 1024 samples within one recording number); channels
    whose records are not such a series are left out. Neo's API is compared where it parses."""
    import neo
    from neo.rawio import openephysbinaryrawio as oeb
    from neo.rawio import openephysrawio as oel
    if any(True for _ in p.rglob("structure.oebin")):
        return _oe_binary(p, neo, oeb)
    return _oe_legacy(p, neo, oel)


def _oe_binary(p: Path, neo, oeb) -> dict:
    fs_, _ = oeb.OpenEphysBinaryRawIO._parse_folder_structure(str(p))
    recs = []
    for node, nd in fs_.items():
        for eid, exp in nd["experiments"].items():
            for rid, rec in exp["recordings"].items():
                recs.append((node, eid, rid, rec))
    recs.sort(key=lambda r: (r[0], r[1], r[2]))
    groups = []  # (node, eid, folder, channels signature, rate) -> [(rid, stream info)]
    for node, eid, rid, rec in recs:
        for sname, info in rec["streams"].get("continuous", {}).items():
            folder = info["folder_name"].rstrip("/")
            sig = tuple((c["channel_name"], float(c["bit_volts"]), c.get("units", "")) for c in info["channels"])
            key = (node, eid, folder, sig, float(info["sample_rate"]))
            g = next((g for g in groups if g[0] == key), None)
            if g is None:
                groups.append((key, [(rid, info)]))
            else:
                g[1].append((rid, info))
    traces = []
    for (node, eid, folder, sig, rate), items in groups:
        sweeps = []
        for k, (rid, info) in enumerate(items[:MAX_SWEEPS]):
            nch = len(info["channels"])
            shape = oeb.get_memmap_shape(info["raw_filename"], "int16", num_channels=nch, offset=0)
            raw = np.memmap(info["raw_filename"], dtype="<i2", mode="r", shape=shape)
            chans = []
            for c, ch in enumerate(info["channels"]):
                y = raw[:, c].astype(np.float64) * float(ch["bit_volts"])
                chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
            sweeps.append({"sweep": k, "sample_count": int(shape[0]), "channels": chans})
        units = [_oe_unit(n, u) for n, _, u in sig]
        t = {"index": len(traces), "sweep_count": len(items), "channel_count": len(sig),
             "channel_names": [n for n, _, _ in sig], "sample_rate_hz": rate, "sweeps": sweeps,
             "name": f"{node}/{folder}" if node else folder}
        if all(u is not None for u in units):
            t["channel_units"] = units
        traces.append(t)
    out = {"reader": f"neo {neo.__version__} OpenEphysBinaryRawIO folder parse + NumPy", "traces": traces}
    # events
    cols = {k: [] for k in ("stream", "sample_number", "time_s", "timestamp_s", "state", "line", "full_word", "text")}
    texts = []
    nstreams = 0
    for node, eid, rid, rec in recs:
        rdir = None
        for info in rec["streams"].get("continuous", {}).values():
            rdir = Path(info["raw_filename"]).parents[2]
        for info in rec["streams"].get("events", {}).values():
            d = Path(info.get("timestamps_npy") or info.get("sample_numbers_npy") or info.get("text_npy") or "").parent
            has = lambda f: (d / f).is_file()
            is_text, is_ttl = has("text.npy"), has("states.npy") or has("channel_states.npy")
            if not (is_text or is_ttl):
                continue
            sn = np.load(d / ("sample_numbers.npy" if has("sample_numbers.npy") else "timestamps.npy")).astype(np.float64)
            n = len(sn)
            load = lambda f: np.load(d / f).astype(np.float64)[:n] if has(f) else np.full(n, np.nan)
            ts = load("timestamps.npy") if has("sample_numbers.npy") else load("synchronized_timestamps.npy")
            rate = float(info.get("sample_rate", 0) or 0)
            cols["stream"] += [float(nstreams)] * n
            cols["sample_number"] += list(sn)
            cols["time_s"] += list(sn / rate) if rate > 0 else [float("nan")] * n
            cols["timestamp_s"] += list(ts)
            if is_ttl:
                cols["state"] += list(load("states.npy") if has("states.npy") else load("channel_states.npy"))
                cols["line"] += list(load("channels.npy"))
                cols["full_word"] += list(load("full_words.npy"))
            else:
                cols["state"] += [float("nan")] * n
                cols["line"] += [float("nan")] * n
                cols["full_word"] += [float("nan")] * n
            if is_text:
                for s in np.load(d / "text.npy"):
                    s = bytes(s).split(b"\0")[0].decode("utf-8", "replace")
                    if s not in texts:
                        texts.append(s)
                    cols["text"].append(float(texts.index(s)))
            else:
                cols["text"] += [float("nan")] * n
            nstreams += 1
    if nstreams:
        out["tables"] = [{"index": 0, "event_count": len(cols["stream"]), "parameter_names": [], "dtypes": [], "xxh3": None,
                          "column_hashes": {k: _col_hash(v) for k, v in cols.items()}}]
    # Neo's own API, where it parses the directory
    # Neo's own API, where it parses the directory: block 0 / segment 0 of every Neo stream,
    # channel by channel, against the first sweep of the matching trace above
    first = {}
    for (node, eid, folder, sig, rate), items in groups:
        if (node, folder) in first:
            continue
        info = items[0][1]
        nch = len(info["channels"])
        shape = oeb.get_memmap_shape(info["raw_filename"], "int16", num_channels=nch, offset=0)
        raw = np.memmap(info["raw_filename"], dtype="<i2", mode="r", shape=shape)
        first[(node, folder)] = {ch["channel_name"]: raw[:, c].astype(np.float64) * float(ch["bit_volts"])
                                 for c, ch in enumerate(info["channels"])}
    try:
        r = oeb.OpenEphysBinaryRawIO(dirname=str(p))
        r.parse_header()
        compared, agree = 0, True
        for si, st in enumerate(r.header["signal_streams"]):
            chans = r.header["signal_channels"][r.header["signal_channels"]["stream_id"] == st["id"]]
            name = str(st["name"])
            node, _, folder = name.rpartition("#")
            for suffix in ("_ADC", "SYNC"):
                if folder.endswith(suffix) and (node, folder[: -len(suffix)]) in first:
                    folder = folder[: -len(suffix)]
            ours = first.get((node, folder))
            if ours is None:
                continue
            n = r.get_signal_size(0, 0, si)
            y = r.rescale_signal_raw_to_float(r.get_analogsignal_chunk(0, 0, 0, n, si), dtype="float64", stream_index=si)
            for c, ch in enumerate(chans):
                v = ours.get(str(ch["name"]))
                if v is None:
                    continue
                compared += 1
                agree &= len(v) == n and bool(np.array_equal(v, y[:, c]))
        out["second_opinion"] = {"reader": f"neo {neo.__version__} OpenEphysBinaryRawIO API", "channels_compared": compared,
                                 "values_agree": agree}
    except Exception as e:  # noqa: BLE001 - recorded
        out["second_opinion"] = {"reader": f"neo {neo.__version__} OpenEphysBinaryRawIO API", "error": f"{type(e).__name__}: {e}"}
    return out


def _oe_legacy_runs(data):
    ts = data["timestamp"].astype(np.int64)
    rec = data["rec_num"].astype(np.int64)
    runs = []
    for k in range(len(data)):
        if k and rec[k] == rec[k - 1] and ts[k] - ts[k - 1] == 1024:
            runs[-1][1] += 1
        else:
            runs.append([k, 1, int(ts[k]), int(rec[k])])
    return runs


OE_HASHED_SAMPLES = 1 << 24

def _oe_legacy(p: Path, neo, oel) -> dict:
    dirs = [("", p)] + sorted((d.name, d) for d in p.iterdir() if d.is_dir() and d.name.startswith("Record"))
    chans = []
    for node, d in dirs:
        for f in sorted(d.glob("*.continuous")):
            hdr = oel.read_file_header(f)
            parts = f.stem.split("_")
            if parts[-1].isdigit() and len(parts) >= 3:
                seg, ch, src = int(parts[-1]), parts[-2], "_".join(parts[:-2])
            else:
                seg, ch, src = 1, parts[-1], "_".join(parts[:-1])
            size = (f.stat().st_size - 1024) // 2070
            if (f.stat().st_size - 1024) % 2070:
                continue
            data = np.memmap(f, dtype=oel.continuous_dtype, mode="r", offset=1024, shape=(size,))
            if not (data["markers"] == np.array([0, 1, 2, 3, 4, 5, 6, 7, 8, 255], "u1")).all() or (data["nb_sample"] != 1024).any():
                continue
            runs = _oe_legacy_runs(data)
            if len(runs) > 2 and len(runs) * 2 > size:
                continue
            name = str(hdr.get("channel") or ch).strip().strip("'").strip() or ch
            digits = next((i for i, c in enumerate(name) if c.isdigit()), len(name))
            rank = {"CH": 0, "AUX": 1, "ADC": 2}.get(name[:digits], 3)
            order = (node, src, seg, rank, int(name[digits:]) if name[digits:].isdigit() else 1 << 62, name)
            chans.append({"order": order, "node": node, "src": src, "seg": seg, "name": name, "data": data,
                          "runs": runs, "rate": float(hdr["sampleRate"]), "bv": float(hdr["bitVolts"])})
    chans.sort(key=lambda c: c["order"])
    groups = []
    for c in chans:
        g = next((g for g in groups if g[0]["node"] == c["node"] and g[0]["src"] == c["src"] and g[0]["seg"] == c["seg"]
                  and g[0]["rate"] == c["rate"] and [r[1:] for r in g[0]["runs"]] == [r[1:] for r in c["runs"]]
                  and [r[0] for r in g[0]["runs"]] == [r[0] for r in c["runs"]]), None)
        if g is None:
            groups.append([c])
        else:
            g.append(c)
    traces = []
    for g in groups:
        c0 = g[0]
        sweeps = []
        for k, (first, nrec, _, _) in enumerate(c0["runs"][:MAX_SWEEPS]):
            cols = []
            n = int(nrec * 1024)
            keep = min(n, OE_HASHED_SAMPLES)  # long recordings: hash the first 2^24 samples
            for c in g:
                y = c["data"]["samples"][first:first + nrec].reshape(-1)[:keep].astype(np.float64) * c["bv"]
                cols.append({"xxh3": _trace_hash(y), "first": _first(y)})
            sw = {"sweep": k, "sample_count": n, "channels": cols}
            if keep < n:
                sw["hashed_samples"] = keep
            sweeps.append(sw)
        traces.append({"index": len(traces), "sweep_count": len(c0["runs"]), "channel_count": len(g),
                       "channel_names": [c["name"] for c in g],
                       "channel_units": ["uV" if c["name"].startswith("CH") else "V" for c in g],
                       "sample_rate_hz": c0["rate"], "sweeps": sweeps})
    out = {"reader": f"neo {neo.__version__} OpenEphysRawIO record layout + NumPy", "traces": traces}
    tables = []
    evfiles = [f for _, d in dirs for f in sorted(d.glob("*.events")) if not f.name.startswith("messages")]
    if evfiles:
        cols = {k: [] for k in ("file", "timestamp", "time_s", "sample_position", "event_type", "processor_id",
                                "event_id", "channel", "recording")}
        for fi, f in enumerate(evfiles):
            hdr = oel.read_file_header(f)
            n = (f.stat().st_size - 1024) // 16
            if n == 0:
                continue
            e = np.memmap(f, dtype=oel.events_dtype, mode="r", offset=1024, shape=(n,))
            rate = float(hdr.get("sampleRate", "nan"))
            cols["file"] += [float(fi)] * n
            cols["timestamp"] += list(e["timestamp"].astype(np.float64))
            cols["time_s"] += list(e["timestamp"].astype(np.float64) / rate)
            for ours, theirs in (("sample_position", "sample_pos"), ("event_type", "event_type"),
                                 ("processor_id", "processor_id"), ("event_id", "event_id"),
                                 ("channel", "chan_id"), ("recording", "record_num")):
                cols[ours] += list(e[theirs].astype(np.float64))
        tables.append({"index": len(tables), "event_count": len(cols["file"]), "parameter_names": [], "dtypes": [],
                       "xxh3": None, "column_hashes": {k: _col_hash(v) for k, v in cols.items()}})
    for _, d in dirs:
        for f in sorted(d.glob("*.spikes")):
            hdr = oel.read_file_header(f)
            size = f.stat().st_size - 1024
            dt = np.dtype(oel.make_spikes_dtype(f)) if size >= 23 else None
            n = size // dt.itemsize if dt is not None and dt.itemsize else 0
            cols = {}
            if n:
                s = np.memmap(f, dtype=dt, mode="r", offset=1024, shape=(n,))
                rate = float(hdr["sampleRate"])
                cols = {"timestamp": s["timestamp"].astype(np.float64), "time_s": s["timestamp"].astype(np.float64) / rate,
                        "sorted_id": s["sorted_id"].astype(np.float64), "electrode_id": s["electrode_id"].astype(np.float64),
                        "channel": s["within_chan_index"].astype(np.float64), "recording": s["rec_num"].astype(np.float64)}
                nch = int(s["nb_channel"][0])
                wav = s["samples"].astype(np.float64).reshape(n, nch, -1)
                gains = s["gains"].astype(np.float64)
                for c in range(nch):
                    for j in range(wav.shape[2]):
                        cols[f"w{c}_{j}"] = (wav[:, c, j] - 32768.0) / gains[:, c] * 1000.0
            tables.append({"index": len(tables), "event_count": int(n), "parameter_names": [], "dtypes": [], "xxh3": None,
                           "column_hashes": {k: _col_hash(v) for k, v in cols.items()}})
    if tables:
        out["tables"] = tables
    try:
        if len(dirs) > 1:
            raise RuntimeError("several Record Node directories (Neo globs them together)")
        r = oel.OpenEphysRawIO(dirname=str(p))
        r.parse_header()
        compared, agree = 0, True
        seg1 = {c["name"]: c for g in groups for c in g if c["seg"] == 1 and len(c["runs"]) == 1}
        for si, st in enumerate(r.header["signal_streams"]):
            chans = r.header["signal_channels"][r.header["signal_channels"]["stream_id"] == st["id"]]
            n = r.get_signal_size(0, 0, si)
            y = r.rescale_signal_raw_to_float(r.get_analogsignal_chunk(0, 0, 0, n, si), dtype="float64", stream_index=si)
            for c, ch in enumerate(chans):
                o = seg1.get(str(ch["name"]))
                if o is None:
                    continue
                v = o["data"]["samples"].reshape(-1).astype(np.float64) * o["bv"]
                compared += 1
                agree &= len(v) == n and bool(np.array_equal(v, y[:, c]))
        out["second_opinion"] = {"reader": f"neo {neo.__version__} OpenEphysRawIO API", "channels_compared": compared,
                                 "values_agree": agree}
    except Exception as e:  # noqa: BLE001 - recorded
        out["second_opinion"] = {"reader": f"neo {neo.__version__} OpenEphysRawIO API", "error": f"{type(e).__name__}: {e}"}
    return out


def _ephys_dir(p: Path):
    """The oracle for an electrophysiology recording directory (a session), or None."""
    names = [q for q in p.iterdir() if q.is_file()]
    exts = {q.suffix.lower()[1:] for q in names}
    if any(True for _ in p.rglob("structure.oebin")) or "continuous" in exts or any(
            q.suffix == ".continuous" for d in p.iterdir() if d.is_dir() and d.name.startswith("Record") for q in d.iterdir()):
        return openephys
    if (p / "info.rhd").exists() or (p / "info.rhs").exists():
        return intan
    if exts & {"ns1", "ns2", "ns3", "ns4", "ns5", "ns6"} or any(q.suffix.lower() == ".nev" and _is_blackrock(q) for q in names):
        return blackrock_dir
    if exts & {"ncs", "nse", "nst", "ntt", "nev"}:
        return neuralynx_dir
    if any(q.suffix.lower() == ".meta" for q in p.rglob("*.meta")):
        return spikeglx_dir
    return None

def plexon(p: Path) -> dict:
    """Plexon PLX via Neo's PlexonRawIO (BSD-3).

    Traces: Neo's streams (channels grouped by the alphabetic prefix of their names), in the
    order of each stream's first channel in the file (openreadout groups channels on one sample
    grid, in header order; on the corpus the groupings coincide). One segment; values raw x gain;
    Neo reports no unit (openreadout: mV), so units are not compared.
    Tables: spikes (openreadout table 0 when the file has spike channels) and events. Neo
    regroups both per channel and unit, so columns are compared sorted, with their counts:
    spikes time_s (ticks / clock), channel, unit, w0 (first waveform sample x Neo's gain);
    events time_s, channel, value."""
    import neo
    from neo.rawio import PlexonRawIO
    r = PlexonRawIO(filename=str(p), progress_bar=False)
    r.parse_header()
    # Neo 0.14.5's chunk reader mis-slices a final continuous block that is longer than the
    # distance its running sample index leaves for it (the 86-sample last block of
    # plexon-file-plexon-3 comes back as 34 samples and zeros). The oracle therefore
    # concatenates Neo's own parsed blocks (Neo's block positions and sizes, bytes through Neo's
    # memory map) instead of calling get_analogsignal_chunk; the number of channels where that
    # differs from Neo's chunk reader is recorded in oracle_note.
    fixed = 0
    h = r.header
    sc = h["signal_channels"]
    streams = [str(s) for s in h["signal_streams"]["id"]]
    order = sorted(range(len(streams)), key=lambda si: min(i for i, c in enumerate(sc) if str(c["stream_id"]) == streams[si]))
    traces = []
    for si in order:
        chans_hdr = sc[sc["stream_id"] == streams[si]]
        n = r.get_signal_size(0, 0, si)
        neo_chunk = np.asarray(r.get_analogsignal_chunk(0, 0, 0, n, si), dtype=np.float64)
        chans = []
        for c in range(len(chans_hdr)):
            blocks = r._data_blocks[5][int(chans_hdr[c]["id"])]
            raw = np.concatenate([r._memmap[b["pos"]:b["pos"] + b["size"]].view("<i2") for b in blocks]).astype(np.float64)
            if len(raw) != n:
                raise RuntimeError(f"{chans_hdr[c]['name']}: {len(raw)} samples in Neo's blocks, Neo's size {n}")
            fixed += int(not np.array_equal(raw, neo_chunk[:, c]))
            y = raw * float(chans_hdr[c]["gain"]) + float(chans_hdr[c]["offset"])
            chans.append({"xxh3": _trace_hash(y), "first": _first(y)})
        traces.append({"index": len(traces), "sweep_count": 1, "channel_count": len(chans_hdr),
                       "channel_names": [str(c["name"]) for c in chans_hdr],
                       "sample_rate_hz": float(chans_hdr[0]["sampling_rate"]),
                       "sweeps": [{"sweep": 0, "sample_count": int(n), "channels": chans}]})
    clock = float(r._global_ssampling_rate)
    tables = []
    if len(h["spike_channels"]) or r._data_blocks[1]:
        t, ch, un, w0 = [], [], [], []
        for u, row in enumerate(h["spike_channels"]):
            chan, unit = (int(x) for x in str(row["id"])[2:].split("#"))
            ts = r.get_spike_timestamps(0, 0, u, None, None)
            wf = r.get_spike_raw_waveforms(0, 0, u, None, None)
            t += list(np.asarray(ts, dtype=np.float64) / clock)
            ch += [chan] * len(ts)
            un += [unit] * len(ts)
            if len(ts):
                w0 += list(np.asarray(wf[:, 0, 0], dtype=np.float64) * float(row["wf_gain"]))
        cols = [("time_s", t), ("channel", ch), ("unit", un), ("w0", w0)]
        tables.append({"index": len(tables), "event_count": len(t), "parameter_names": [], "dtypes": [], "xxh3": None,
                       "sorted_column_hashes": [{"column": c, "count": len(v), "xxh3": _sorted_hash(v)} for c, v in cols]})
    if len(h["event_channels"]):
        t, ch, val = [], [], []
        for e, row in enumerate(h["event_channels"]):
            ts, _, labels = r.get_event_timestamps(0, 0, e, None, None)
            t += list(np.asarray(ts, dtype=np.float64) / clock)
            ch += [int(row["id"])] * len(ts)
            val += [int(x) for x in labels]
        cols = [("time_s", t), ("channel", ch), ("value", val)]
        tables.append({"index": len(tables), "event_count": len(t), "parameter_names": [], "dtypes": [], "xxh3": None,
                       "sorted_column_hashes": [{"column": c, "count": len(v), "xxh3": _sorted_hash(v)} for c, v in cols]})
    out = {"reader": f"neo {neo.__version__} PlexonRawIO", "traces": traces}
    if fixed:
        out["oracle_note"] = f"samples of {fixed} channel(s) taken from Neo's parsed blocks, where Neo's chunk reader truncates the final block (see plexon() in oracle/gen.py)"
    if tables:
        out["tables"] = tables
    return out

def pl2(p: Path) -> dict:
    """Plexon PL2: NO independent reader exists that the clean-room rules allow (Neo's PL2 path
    loads Plexon's DLL). This is a second implementation of the layout recorded in
    docs/formats/plexon.md, written in NumPy from the same hex-dump observations: it catches
    Rust-side decoding errors and regressions, not misreadings of the format itself
    (`independent: false`). Records are walked from the header's data offset to its footer
    offset; analog channels with records are grouped by rate and record grid like
    openreadout; spike and event tables are hashed column by column in file order."""
    import struct
    b = p.read_bytes()
    u64 = lambda o: struct.unpack_from("<Q", b, o)[0]
    clock = struct.unpack_from("<d", b, 0x258)[0]
    ns, na, nd = struct.unpack_from("<I", b, 0x264)[0], struct.unpack_from("<I", b, 0x26C)[0], struct.unpack_from("<I", b, 0x274)[0]
    heads = {"spike": [], "analog": []}
    off = 0x480
    for kind, n, size in (("spike", ns, 2592), ("analog", na, 512), ("digital", nd, 368)):
        for _ in range(n):
            if kind != "digital":
                name = b[off + 16:off + 80].split(b"\0")[0].decode("latin-1")
                src, ch = b[off + 1], struct.unpack_from("<I", b, off + 0x54)[0]
                rate, vpc = struct.unpack_from("<dd", b, off + 0x70)
                heads[kind].append((name, src, ch, rate, vpc))
            off += size
    pos, stop = u64(0x28), u64(0x30)
    if stop <= pos or stop > len(b):
        stop = len(b)  # offline-written (merged) files leave the footer offset 0: walk to the end
    ana, spk, evt = {}, [], []
    r16 = lambda x: (x + 15) // 16 * 16
    while pos < stop:
        t, src, words = b[pos], b[pos + 1], struct.unpack_from("<H", b, pos + 2)[0]
        ch = struct.unpack_from("<H", b, pos + 4)[0]
        if t == 0x42:
            n = struct.unpack_from("<H", b, pos + 6)[0]
            ana.setdefault((src, ch), []).append((u64(pos + 8), n, np.frombuffer(b, "<i2", n, pos + 16)))
            size = 16 + r16(2 * words)
        elif t == 0x31:
            wf, n = struct.unpack_from("<HI", b, pos + 6)
            ts = np.frombuffer(b, "<u8", n, pos + 16)
            units = np.frombuffer(b, "<u2", n, pos + 16 + 8 * n)
            waves = np.frombuffer(b, "<i2", n * wf, pos + 16 + 10 * n).reshape(n, wf)
            spk.append((src, ch, ts, units, waves))
            size = 16 + r16(n * (10 + 2 * wf))
        elif t == 0x5A:
            n = struct.unpack_from("<H", b, pos + 6)[0]
            evt.append((src, ch, np.frombuffer(b, "<u8", n, pos + 16), np.frombuffer(b, "<u2", n, pos + 16 + 8 * n)))
            size = 16 + r16(10 * n)
        else:
            size = 16 + r16(2 * words)
        pos += size
    groups = []
    for name, src, ch, rate, vpc in heads["analog"]:
        recs = ana.get((src, ch))
        if not recs:
            continue
        runs, cur = [], None
        for ts, n, data in recs:
            if cur is not None and abs(ts - cur["end"]) <= clock / rate / 2:
                cur["data"].append(data)
                cur["n"] += n
            else:
                cur = {"ts": ts, "n": n, "data": [data]}
                runs.append(cur)
            cur["end"] = ts + n * clock / rate
        key = (rate, tuple((r["ts"], r["n"]) for r in runs))
        g = next((g for g in groups if g["key"] == key), None)
        if g is None:
            g = {"key": key, "names": [], "chans": []}
            groups.append(g)
        g["names"].append(name)
        g["chans"].append([np.concatenate(r["data"]).astype(np.float64) * vpc for r in runs])
    traces = []
    for g in groups:
        rate, grid = g["key"]
        sweeps = [{"sweep": k, "sample_count": int(grid[k][1]),
                   "channels": [{"xxh3": _trace_hash(c[k]), "first": _first(c[k])} for c in g["chans"]]}
                  for k in range(len(grid))]
        traces.append({"index": len(traces), "sweep_count": len(grid), "channel_count": len(g["names"]),
                       "channel_names": g["names"], "channel_units": ["V"] * len(g["names"]),
                       "sample_rate_hz": rate, "sweeps": sweeps})
    tables = []
    if ns or spk:
        vpc = {(src, ch): v for _, src, ch, _, v in heads["spike"]}
        wmax = max((w.shape[1] for *_, w in spk), default=0)
        ts = np.concatenate([x[2] for x in spk]) if spk else np.zeros(0, "<u8")
        cols = {"time_s": ts.astype(np.float64) / clock, "timestamp": ts.astype(np.float64),
                "source": np.concatenate([np.full(len(x[2]), x[0], np.float64) for x in spk]) if spk else [],
                "channel": np.concatenate([np.full(len(x[2]), x[1], np.float64) for x in spk]) if spk else [],
                "unit": np.concatenate([x[3].astype(np.float64) for x in spk]) if spk else []}
        for k in range(wmax):
            cols[f"w{k}"] = np.concatenate([x[4][:, k].astype(np.float64) * vpc.get((x[0], x[1]), 1.0) for x in spk])
        tables.append({"index": len(tables), "event_count": int(len(ts)), "parameter_names": [], "dtypes": [], "xxh3": None,
                       "column_hashes": {k: _col_hash(v) for k, v in cols.items()}})
    if nd or evt:
        ts = np.concatenate([x[2] for x in evt]) if evt else np.zeros(0, "<u8")
        cols = {"time_s": ts.astype(np.float64) / clock, "timestamp": ts.astype(np.float64),
                "source": np.concatenate([np.full(len(x[2]), x[0], np.float64) for x in evt]) if evt else [],
                "channel": np.concatenate([np.full(len(x[2]), x[1], np.float64) for x in evt]) if evt else [],
                "value": np.concatenate([x[3].astype(np.float64) for x in evt]) if evt else []}
        tables.append({"index": len(tables), "event_count": int(len(ts)), "parameter_names": [], "dtypes": [], "xxh3": None,
                       "column_hashes": {k: _col_hash(v) for k, v in cols.items()}})
    out = {"reader": "second implementation of our own PL2 notes (NumPy); not an independent oracle", "independent": False,
           "traces": traces}
    if tables:
        out["tables"] = tables
    return out

def _sorted_rows(cols) -> dict:
    """{columns, count, xxh3} of rows sorted lexicographically (row-major float64 LE), the
    `sorted_row_hashes` comparison of the corpus test."""
    names = [c for c, _ in cols]
    m = np.column_stack([np.asarray(v, dtype=np.float64) for _, v in cols]) if cols and len(cols[0][1]) else np.zeros((0, len(cols)))
    order = np.lexsort(tuple(m[:, k] for k in reversed(range(m.shape[1])))) if len(m) else np.zeros(0, dtype=int)
    m = np.ascontiguousarray(m[order])
    return {"columns": names, "count": int(m.shape[0]), "xxh3": xxhash.xxh3_128_hexdigest(m.astype("<f8").tobytes())}


def pl2_plx(p: Path, plx: Path) -> dict:
    """Plexon PL2 checked against a PLX file of the same recording read by Neo's PlexonRawIO
    (BSD-3): an independent oracle for PL2 events and spike waveforms. Used where a depositor
    shares both (Zenodo 11586428: the merged `.pl2` next to the `.plx` it was sorted from).

    Events: every (timestamp, channel, value) of the PLX event channels the PL2 has records for
    (Neo's parsed event blocks), compared as a sorted row set. The PLX's start/stop channels (258,
    259) have no records in the PL2 and are left out. Spikes: the PL2 holds the sorted subset of
    the PLX's threshold crossings, so the rows compared are the PLX spikes whose (channel,
    timestamp) the PL2 lists (selected with this file's NumPy walker, `pl2()`); their timestamps
    and waveforms come from Neo's parse of the PLX, scaled with Neo's waveform gain formula
    (mV per count / 1000, asserted equal to the PL2 header's volts per count). Neo 0.14.5's
    parse_header refuses this PLX (continuous channels of unequal length in one stream), so the
    oracle uses the data blocks Neo has parsed by then (`_data_blocks`, `_memmap`) and Neo's own
    header layouts (`GlobalHeader`, `DspChannelHeader`)."""
    import struct
    import neo
    from neo.rawio import plexonrawio as prx
    r = prx.PlexonRawIO(filename=str(plx), progress_bar=False)
    note = []
    try:
        r.parse_header()
    except Exception as e:  # noqa: BLE001 - recorded in the note
        note.append(f"Neo parse_header refused the PLX ({e}); its parsed data blocks are used")
    blocks = r._data_blocks
    with open(plx, "rb") as fid:
        gh = prx.read_as_dict(fid, prx.GlobalHeader)
    dsp = np.memmap(str(plx), dtype=prx.DspChannelHeader, mode="r", offset=np.dtype(prx.GlobalHeader).itemsize,
                    shape=(int(gh["NumDSPChannels"]),))
    if gh["Version"] < 105:
        raise RuntimeError(f"PLX version {gh['Version']}: only the >= 105 gain formula is used here")
    mv_per_count = {int(d["Channel"]): gh["SpikeMaxMagnitudeMV"] / (0.5 * 2.0 ** gh["BitsPerSpikeSample"] * d["Gain"] * gh["SpikePreAmpGain"])
                    for d in dsp}
    # the PL2 side: which channels have event records, which (channel, timestamp) spikes, the header scale
    b = p.read_bytes()
    u64 = lambda o: struct.unpack_from("<Q", b, o)[0]
    ns = struct.unpack_from("<I", b, 0x264)[0]
    upc = {}
    off = 0x480
    for _ in range(ns):
        ch = struct.unpack_from("<I", b, off + 0x54)[0]
        upc[ch] = struct.unpack_from("<d", b, off + 0x78)[0]
        off += 2592
    r16 = lambda x: (x + 15) // 16 * 16
    pos, stop = u64(0x28), u64(0x30)
    if stop <= pos:
        stop = len(b)
    spk_keys, ev_chans = [], set()
    while pos < stop:
        t, words, ch = b[pos], struct.unpack_from("<H", b, pos + 2)[0], struct.unpack_from("<H", b, pos + 4)[0]
        if t == 0x31:
            wf, n = struct.unpack_from("<HI", b, pos + 6)
            ts = np.frombuffer(b, "<u8", n, pos + 16).astype(np.int64)
            spk_keys.append((ch, ts))
            size = 16 + r16(n * (10 + 2 * wf))
        elif t == 0x5A:
            n = struct.unpack_from("<H", b, pos + 6)[0]
            ev_chans.add(ch)
            size = 16 + r16(10 * n)
        else:
            size = 16 + r16(2 * words)
        pos += size
    # events from the PLX
    et, ec, ev = [], [], []
    for ch in sorted(ev_chans):
        blk = blocks[4].get(ch)
        if blk is None:
            raise RuntimeError(f"PL2 event channel {ch} is not in the PLX")
        et += list(blk["timestamp"].astype(np.float64))
        ec += [float(ch)] * len(blk)
        ev += list(blk["label"].astype(np.float64))
    left_out = {int(ch): int(len(v)) for ch, v in blocks[4].items() if ch not in ev_chans and len(v)}
    if left_out:
        note.append(f"PLX event channels without PL2 records, not compared: {left_out}")
    # spikes from the PLX, restricted to the PL2's (channel, timestamp) pairs
    rows_t, rows_c, waves = [], [], []
    wlen = None
    mm = r._memmap
    for ch, ts in spk_keys:
        blk = blocks[1].get(ch)
        if blk is None:
            raise RuntimeError(f"PL2 spike channel {ch} is not in the PLX")
        scale = mv_per_count[ch] / 1000.0
        if abs(scale - upc[ch]) > 1e-12 * abs(upc[ch]):
            raise RuntimeError(f"channel {ch}: PLX gain {scale} V/count != PL2 header {upc[ch]}")
        pts = blk["timestamp"].astype(np.int64)
        order = np.argsort(pts, kind="stable")
        k = np.searchsorted(pts[order], ts)
        if np.any(k >= len(pts)) or np.any(pts[order][np.minimum(k, len(pts) - 1)] != ts):
            raise RuntimeError(f"channel {ch}: PL2 spike timestamps missing from the PLX")
        for j in order[k]:
            bl = blk[j]
            raw = mm[bl["pos"]:bl["pos"] + bl["size"]].view("<i2").astype(np.float64)
            wlen = wlen or len(raw)
            if len(raw) != wlen:
                raise RuntimeError("waveform lengths differ")
            waves.append(raw * upc[ch])
        rows_t += list(ts.astype(np.float64))
        rows_c += [float(ch)] * len(ts)
    w = np.array(waves) if waves else np.zeros((0, wlen or 0))
    spike_cols = [("timestamp", rows_t), ("channel", rows_c)] + [(f"w{k}", w[:, k]) for k in range(w.shape[1])]
    tables = [
        {"index": 0, "event_count": len(rows_t), "parameter_names": [], "dtypes": [], "xxh3": None,
         "sorted_row_hashes": [_sorted_rows(spike_cols)]},
        {"index": 1, "event_count": len(et), "parameter_names": [], "dtypes": [], "xxh3": None,
         "sorted_row_hashes": [_sorted_rows([("timestamp", et), ("channel", ec), ("value", ev)])]},
    ]
    return {"reader": f"neo {neo.__version__} PlexonRawIO on the paired PLX {plx.name}", "independent": True,
            "oracle_note": "; ".join(note) or None, "tables": tables}


def _heka_parts(label, sweeps, sig_of, emit):
    """Split one series into traces the way docs/formats/heka-patchmaster.md describes: channels
    (trace records by index) with the same signature — (sample interval, per-sweep lengths) — share
    a trace; several traces are named `<series> (part k)`. `sig_of(sweep, c)` gives a channel's
    signature in one sweep, `emit(name, channels)` records one trace."""
    width = {len(w) for w in sweeps}
    if len(width) != 1:
        raise RuntimeError(f"series {label!r}: sweeps hold {sorted(width)} traces")
    nch = width.pop()
    groups = []
    for c in range(nch):
        sig = tuple(sig_of(w, c) for w in sweeps)
        g = next((g for g in groups if g[0] == sig), None)
        if g is None:
            groups.append((sig, [c]))
        else:
            g[1].append(c)
    for k, (_, chans) in enumerate(groups):
        emit(f"{label} (part {k + 1})" if len(groups) > 1 else label, chans)


def heka(p: Path) -> dict:
    """HEKA PatchMaster bundle via load-heka-python (MIT, pinned commit 185f6ed), run as a black
    box through its public API: the pulsed tree (`LoadHeka.pul`) and its data loader
    (`fill_pul_with_data`, without zero subtraction). One trace per series in group/series order
    (split into parts by `_heka_parts`); values raw x data scaler as float64."""
    from load_heka_python.load_heka import LoadHeka
    from load_heka_python.readers import data_reader
    try:
        h = LoadHeka(str(p))
    except BaseException as e:  # it raises BaseException for versions it does not know
        if isinstance(e, (KeyboardInterrupt, SystemExit)):
            raise
        return _heka_pyheka(p, f"load-heka-python refused the file ({e})")
    traces = []
    try:
        for gi, g in enumerate(h.pul["ch"]):
            for si, s in enumerate(g["ch"]):
                data_reader.fill_pul_with_data(h.pul, h.fh, gi, si, add_zero_offset=False)
                sweeps = [w["ch"] for w in s["ch"]]

                def emit(name, chans, sweeps=sweeps):
                    first = sweeps[0]
                    dx = {float(w[c]["hd"]["TrXInterval"]) for w in sweeps for c in chans}
                    out = []
                    for k, w in enumerate(sweeps[:MAX_SWEEPS]):
                        n = {len(w[c]["data"]) for c in chans}
                        out.append({"sweep": k, "sample_count": n.pop(),
                                    "channels": [{"xxh3": _trace_hash(w[c]["data"]), "first": _first(w[c]["data"])} for c in chans]})
                    traces.append({"index": len(traces), "sweep_count": len(sweeps), "channel_count": len(chans),
                                   "channel_names": [first[c]["hd"]["TrLabel"] for c in chans],
                                   "channel_units": [first[c]["hd"]["TrYUnit"] for c in chans],
                                   "sample_rate_hz": 1.0 / dx.pop(), "name": name, "sweeps": out})

                _heka_parts(s["hd"]["SeLabel"], sweeps,
                            lambda w, c: (float(w[c]["hd"]["TrXInterval"]), len(w[c]["data"])), emit)
                for w in sweeps:
                    for r in w:
                        r["data"] = None  # free the series before the next one
    finally:
        h.close()
    return {"reader": "load-heka-python 1.3.0 (commit 185f6ed)", "traces": traces}


def _heka_pyheka(p: Path, why: str) -> dict:
    """Fallback for bundles load-heka-python does not know: pyHEKA (AGPL-3.0, run as a black box
    only, through the calls its README documents: `Bundle`, `pul.children`, `.Label`,
    `data[group, series, sweep, trace]`). It reports no units or intervals through those calls,
    so only names, counts and sample values are recorded, and series are split into parts by
    per-sweep lengths only."""
    import pyheka
    traces = []
    with pyheka.Bundle(str(p)) as b:
        for gi, g in enumerate(b.pul.children):
            for si, s in enumerate(g.children):
                recs = [list(w.children) for w in s.children]
                data = [[np.asarray(b.data[gi, si, k, c], dtype=np.float64) for c in range(len(w))] for k, w in enumerate(recs)]

                def emit(name, chans, recs=recs, data=data):
                    out = []
                    for k in range(min(len(data), MAX_SWEEPS)):
                        out.append({"sweep": k, "sample_count": len(data[k][chans[0]]),
                                    "channels": [{"xxh3": _trace_hash(data[k][c]), "first": _first(data[k][c])} for c in chans]})
                    traces.append({"index": len(traces), "sweep_count": len(recs), "channel_count": len(chans),
                                   "channel_names": [str(recs[0][c].Label) for c in chans], "name": name, "sweeps": out})

                _heka_parts(str(s.Label), data, lambda w, c: len(w[c]), emit)
    return {"reader": "pyheka 1.0.1 (AGPL-3.0, black box)", "oracle_note": why, "traces": traces}


def _is_heka(p: Path) -> bool:
    with open(p, "rb") as f:
        return f.read(4) in (b"DAT1", b"DAT2")


def spike2(p: Path) -> dict:
    """CED Spike2 .smr via Neo's Spike2RawIO (BSD-3), with try_signal_grouping=False (one stream
    per waveform channel, which is our one trace per channel). Sweeps are Neo's segments. Values
    are recomputed in float64 from Neo's raw int16 chunks and Neo's parsed channel header
    (`raw * (scale / 6553.6) + offset`): Neo computes the gain from the float32 scale in float32
    under NumPy 2, which rounds it. Events and spike tables come from Neo's parsed block lists
    (`_all_data_blocks`, `_memmap`, `get_channel_dtype`), every record in channel order and file
    order (Neo's event API keeps only records inside signal segments): time_s = tick x Neo's
    time factor, tick, channel (index + 1), the four marker bytes (NaN for plain events), the
    text-mark text as an index into the texts in order of first appearance, and for AdcMark /
    RealMark the waveform (int16 scaled as above, float32 as stored), NaN-padded."""
    import neo
    from neo.rawio import spike2rawio as s2
    r = s2.Spike2RawIO(filename=str(p), try_signal_grouping=False)
    r.parse_header()
    h = r.header
    infos = r._channel_infos
    tf = r._time_factor
    traces = []
    for si, st in enumerate(h["signal_streams"]):
        ch = h["signal_channels"][h["signal_channels"]["stream_id"] == st["id"]]
        cid = int(ch["id"][0])
        info = infos[cid]
        sweeps = []
        nseg = int(h["nb_segment"][0])
        for seg in range(min(nseg, MAX_SWEEPS)):
            raw = r.get_analogsignal_chunk(0, seg, None, None, si)[:, 0]
            if info["kind"] == 1:
                y = raw.astype(np.float64) * (float(info["scale"]) / 6553.6) + float(info["offset"])
            else:
                y = raw.astype(np.float64)
            sweeps.append({"sweep": seg, "sample_count": int(len(y)), "channels": [{"xxh3": _trace_hash(y), "first": _first(y)}]})
        name = str(ch["name"][0]) or f"ch{cid + 1}"
        traces.append({"index": len(traces), "sweep_count": nseg, "channel_count": 1, "channel_names": [name],
                       "channel_units": [str(ch["units"][0]).strip()], "sample_rate_hz": float(ch["sampling_rate"][0]),
                       "sweeps": sweeps})
    mm = r._memmap
    texts = []
    ev = {k: [] for k in ("time_s", "tick", "channel", "code0", "code1", "code2", "code3", "text")}
    sp_rows = []
    for cid, info in enumerate(infos):
        kind = int(info["kind"])
        if kind not in (2, 3, 4, 5, 6, 7, 8):
            continue
        dt = s2.get_channel_dtype(info)
        for bl in r._all_data_blocks[cid]:
            rec = mm[bl["pos"]:bl["pos"] + bl["size"] * dt.itemsize].view(dt)
            for x in rec:
                tick = int(x["tick"])
                codes = [float("nan")] * 4
                if kind in (5, 6, 7, 8):
                    m = int(x["marker"]) & 0xFFFFFFFF
                    codes = [float((m >> (8 * j)) & 0xFF) for j in range(4)]
                if kind in (6, 7):
                    w = np.asarray(x["waveform"], dtype=np.float64)
                    if kind == 6:
                        w = w * (float(info["scale"]) / 6553.6) + float(info["offset"])
                    sp_rows.append((tick * tf, float(tick), float(cid + 1), codes, w))
                    continue
                text = float("nan")
                if kind == 8:
                    t = bytes(x["label"]).split(b"\0")[0].decode("latin-1")
                    if t not in texts:
                        texts.append(t)
                    text = float(texts.index(t))
                for k, v in zip(ev, [tick * tf, float(tick), float(cid + 1), *codes, text]):
                    ev[k].append(v)
    tables = []
    if any(int(i["kind"]) in (2, 3, 4, 5, 8) for i in infos):
        tables.append({"index": len(tables), "event_count": len(ev["tick"]), "parameter_names": [], "dtypes": [], "xxh3": None,
                       "column_hashes": {k: _col_hash(v) for k, v in ev.items()}})
    if any(int(i["kind"]) in (6, 7) for i in infos):
        wmax = max((len(x[4]) for x in sp_rows), default=0)
        cols = {"time_s": [x[0] for x in sp_rows], "tick": [x[1] for x in sp_rows], "channel": [x[2] for x in sp_rows],
                "unit": [x[3][0] for x in sp_rows], "code1": [x[3][1] for x in sp_rows], "code2": [x[3][2] for x in sp_rows],
                "code3": [x[3][3] for x in sp_rows]}
        for k in range(wmax):
            cols[f"w{k}"] = [x[4][k] if k < len(x[4]) else float("nan") for x in sp_rows]
        tables.append({"index": len(tables), "event_count": len(sp_rows), "parameter_names": [], "dtypes": [], "xxh3": None,
                       "column_hashes": {k: _col_hash(v) for k, v in cols.items()}})
    out = {"reader": f"neo {neo.__version__} Spike2RawIO (try_signal_grouping=False)", "traces": traces}
    if tables:
        out["tables"] = tables
    return out


def neuralynx_nvt(p: Path) -> dict:
    """Neuralynx video-tracker (.nvt) via nept's `load_nvt` (vandermeerlab, MIT; Neo does not
    read .nvt), run as a black box with remove_empty=False: one table; `x`, `y` compared column by
    column and `timestamp_us` = round(nept time s x 1e6) (nept returns seconds)."""
    import nept
    from nept.loaders_neuralynx import load_nvt
    d = load_nvt(str(p), remove_empty=False)
    n = len(d["time"])
    cols = {"timestamp_us": np.round(np.asarray(d["time"], dtype=np.float64) * 1e6),
            "x": np.asarray(d["x"], dtype=np.float64), "y": np.asarray(d["y"], dtype=np.float64)}
    return {"reader": f"nept {getattr(nept, '__version__', '0.1.0')} load_nvt",
            "tables": [{"index": 0, "event_count": n,
                        "parameter_names": ["timestamp_us", "x", "y", "angle", "target_count"],
                        "dtypes": [], "xxh3": None, "column_hashes": {k: _col_hash(v) for k, v in cols.items()}}]}


def winwcp(p: Path) -> dict:
    """WinWCP .wcp via Neo's WinWcpRawIO (BSD-3): one trace whose channels are Neo's signal
    channels and whose sweeps are Neo's segments (the records). Values are recomputed in float64
    from Neo's raw int16 chunks and Neo's gain (VMax / ADCMAX / YG; Neo takes VMax from the last
    record, constant in the corpus files); the sample rate is Neo's (1 / median interval)."""
    from neo.rawio import winwcprawio as ww
    r = ww.WinWcpRawIO(filename=str(p))
    r.parse_header()
    h = r.header
    ch = h["signal_channels"]
    nseg = int(h["nb_segment"][0])
    sweeps = []
    for seg in range(min(nseg, MAX_SWEEPS)):
        cols = []
        for si in range(len(h["signal_streams"])):
            raw = r.get_analogsignal_chunk(0, seg, None, None, si)
            idx = np.flatnonzero(ch["stream_id"] == h["signal_streams"][si]["id"])
            for k, c in enumerate(idx):
                cols.append((c, raw[:, k].astype(np.float64) * float(ch["gain"][c])))
        cols = [v for _, v in sorted(cols, key=lambda t: t[0])]
        sweeps.append({"sweep": seg, "sample_count": int(len(cols[0])),
                       "channels": [{"xxh3": _trace_hash(v), "first": _first(v)} for v in cols]})
    trace = {"index": 0, "name": "record", "sweep_count": nseg, "channel_count": len(ch),
             "channel_names": [str(x) for x in ch["name"]], "channel_units": [str(x) for x in ch["units"]],
             "sample_rate_hz": float(ch["sampling_rate"][0]), "sweeps": sweeps,
             "reader": f"neo {__import__('neo').__version__} WinWcpRawIO"}
    return {"reader": "neo WinWcpRawIO", "traces": [trace]}

EPHYS = {"wcp": winwcp, "ncs": neuralynx, "nvt": neuralynx_nvt, "bin": spikeglx, "rhd": intan, "rhs": intan, "nev": lambda p: blackrock(p) if _is_blackrock(p) else neuralynx(p),
         "nse": neuralynx, "nst": neuralynx, "ntt": neuralynx, "plx": plexon, "pl2": pl2, "smr": spike2,
         **{f"ns{i}": blackrock for i in range(1, 7)}}

# ---------------------------------------------------------------- chromatography
# ChemStation `.D` directories and `.ch`/`.uv`/`.ms` files, AIA/ANDI netCDF, Waters `.raw`
# directories. Oracles: rainbow-api (LGPL-3.0, run as a black box only) for ChemStation versions
# 30/130/131/179/181, `.ms` and Waters; Aston (BSD-3) for ChemStation version 81; scipy
# `netcdf_file` (BSD-3) for ANDI. Trace records follow the electrophysiology shape
# (`traces[].sweeps[].channels[]` = xxh3 of float64 LE + first 8 values) plus `x_first_min` /
# `x_last_min`; spectra follow the Thermo shape (`spectra.scans[]`). Adjustments to oracle output
# are listed in each record's `oracle_note`.

MAX_CHROM_SCANS = int(os.environ.get("ORACLE_MAX_SCANS", "200"))  # spectra hashed per run (evenly spaced)


def _rainbow():
    import warnings
    warnings.filterwarnings("ignore")
    import rainbow as rb
    return rb


def _chrom_trace(index, names, units, x_min, cols, rate=None, rate_tol=None, note=None, x_check=True):
    """One trace record: cols = list of per-channel value arrays (scaled, float64)."""
    x = np.asarray(x_min, dtype=np.float64)
    n = len(cols[0]) if cols else 0
    if rate is None and n > 1 and x[n - 1] > x[0]:
        rate = (n - 1) / ((x[n - 1] - x[0]) * 60.0)
    t = {"index": index, "sweep_count": 1, "channel_count": len(cols), "channel_names": names,
         "channel_units": units, "sample_rate_hz": rate}
    if rate_tol is not None:
        t["sample_rate_tolerance_hz"] = rate_tol
    if x_check and n:
        t["x_first_min"] = float(x[0])
        t["x_last_min"] = float(x[n - 1])
    t["sweeps"] = [{"sweep": 0, "sample_count": n,
                    "channels": [{"xxh3": _trace_hash(c), "first": _first(c)} for c in cols]}]
    if note:
        t["oracle_note"] = note
    return t


def _spectrum_record(i, rt_s, mz, inten, polarity="unknown", centroided=False, tic=None):
    mz = np.asarray(mz, dtype=np.float64)
    inten = np.asarray(inten, dtype=np.float32)
    return {"index": i, "scan_number": i + 1, "ms_level": 1, "rt_s": float(rt_s), "polarity": polarity,
            "centroided": centroided, "filter": None, "precursor_mz": None, "precursor_charge": None,
            "n_peaks": int(mz.size), "mz_bits": None,
            "xxh3_mz": xxhash.xxh3_128_hexdigest(np.ascontiguousarray(mz.astype("<f8")).tobytes()),
            "xxh3_intensity": xxhash.xxh3_128_hexdigest(np.ascontiguousarray(inten.astype("<f4")).tobytes()),
            "sum_intensity": float(inten.astype(np.float64).sum()),
            "mz_min": float(mz[0]) if mz.size else None, "mz_max": float(mz[-1]) if mz.size else None,
            "first_peaks": [[float(a), float(b)] for a, b in zip(mz[:5], inten[:5])]}


def _pick(n):
    """Up to MAX_CHROM_SCANS evenly spaced scan indices (first and last included)."""
    if n <= MAX_CHROM_SCANS:
        return list(range(n))
    return sorted(set(int(round(k)) for k in np.linspace(0, n - 1, MAX_CHROM_SCANS)))


def _cs_version(p: Path) -> str:
    b = p.read_bytes()[:4]
    return b[1:1 + b[0]].decode("ascii", "replace")


def _cs_81(p: Path, index: int):
    """Version 81 through Aston's AgilentFID.total_trace (raw values). Aston multiplies the
    escape's high word by 65,534; the oracle runs Aston's own function with that one constant
    changed to 65,536 (rainbow, run on version 181 files of the same encoding, uses 65,536; see
    docs/provenance/chemstation.md). Scale: big-endian f64 at 0x284 (the offset Aston's MWD reader
    uses for its `del_ab` factor)."""
    import inspect, textwrap, struct, scipy.io, scipy.io.netcdf
    scipy.io.netcdf.NetCDFFile = scipy.io.netcdf_file  # Aston imports a name newer scipy dropped
    import aston.tracefile.agilent_fid as af
    src = textwrap.dedent(inspect.getsource(af.AgilentFID.total_trace)).replace("65534", "65536")
    ns = {}
    exec(src, af.__dict__, ns)
    tf = af.AgilentFID(str(p))
    tr = ns["total_trace"](tf)
    raw = np.asarray(tr.values, dtype=np.float64).ravel()
    times = np.asarray(tr.index, dtype=np.float64)
    b = p.read_bytes()
    scale = struct.unpack(">d", b[0x284:0x28C])[0]
    offset = struct.unpack(">d", b[0x27C:0x284])[0]
    unit = b[0x245:0x245 + b[0x244]].decode("latin-1")
    y = raw * scale + offset
    t = _chrom_trace(index, [p.stem], [unit], times, [y], x_check=False,
                     note="Aston AgilentFID.total_trace with the escape multiplier 65536 (Aston: 65534); Aston's time axis is a fixed 0.2 s step, so rate and x range are not compared")
    t["sample_rate_hz"] = None
    return t


def _cs_rainbow_file(p: Path, index: int):
    """One ChemStation .ch or .uv file through rainbow's parser, as a trace record."""
    rb = _rainbow()
    from rainbow.agilent import chemstation as cs
    df = cs.parse_file(str(p))
    if df is None:
        raise RuntimeError(f"rainbow does not read {p.name} (version {_cs_version(p)})")
    x = np.asarray(df.xlabels, dtype=np.float64)
    data = np.asarray(df.data, dtype=np.float64)
    unit = (df.metadata or {}).get("unit", "")
    if p.suffix.lower() == ".uv":
        names = [f"{w:g} nm" for w in np.asarray(df.ylabels, dtype=np.float64)]
        cols = [data[:, k] for k in range(data.shape[1])]
        return _chrom_trace(index, names, [unit] * len(names), x, cols)
    note = None
    x_check = True
    y = data[:, 0]
    rate_tol = None
    if _cs_version(p) == "181" and p.read_bytes()[-2:] == b"\x00\x00" and len(y) == len(x) + 1:
        # rainbow returns one value more than time labels: the final 0x0000 word, which we read
        # as the body's terminator
        note = "rainbow returns n+1 values for n time labels (the final 0x0000 word as a sample); the oracle keeps the first n"
        y = y[:-1]
    t = _chrom_trace(index, [p.stem], [unit], x, [y], rate_tol=rate_tol, note=note, x_check=x_check)
    return t


def _cs_ms_spectra(p: Path) -> dict:
    """A ChemStation .ms file through rainbow at its finest bin (0.05 m/z = the stored m/z × 20
    grid): every non-zero bin of a scan is one stored pair."""
    rb = _rainbow()
    from rainbow.agilent import chemstation as cs
    df = cs.parse_ms(str(p), bin_width=0.05)
    x = np.asarray(df.xlabels, dtype=np.float64)
    mzs = np.asarray(df.ylabels, dtype=np.float64)
    scans = []
    for i in _pick(len(x)):
        row = np.asarray(df.data[i])
        nz = np.nonzero(row)[0]
        mz = np.round(mzs[nz] * 20.0) / 20.0
        scans.append(_spectrum_record(i, x[i] * 60.0, mz, row[nz]))
    return {"scan_count": int(len(x)),
            "oracle_note": "rainbow parse_ms(bin_width=0.05); m/z rounded to the 0.05 grid, zero-intensity pairs are invisible to the oracle",
            "scans": scans}


def chemstation(p: Path) -> dict:
    files = [p] if p.is_file() else sorted(q for q in p.iterdir() if q.is_file() and q.suffix.lower() in (".ch", ".uv", ".ms"))
    traces, spectra, notes = [], None, []
    for q in files:
        v = _cs_version(q)
        if q.suffix.lower() == ".ms":
            if spectra is not None:
                notes.append(f"{q.name}: only the first .ms file is compared")
                continue
            spectra = _cs_ms_spectra(q)
            continue
        if v == "81":
            traces.append(_cs_81(q, len(traces)))
        else:
            traces.append(_cs_rainbow_file(q, len(traces)))
    out = {"reader": "rainbow-api (black box) / Aston (BSD-3) for version 81"}
    if notes:
        out["notes"] = notes
    if traces:
        out["traces"] = traces
    if spectra is not None:
        out["spectra"] = spectra  # last: main() writes the scans one per line
    return out


def andi(p: Path) -> dict:
    """AIA/ANDI netCDF through scipy.io.netcdf_file (BSD-3)."""
    import scipy.io
    f = scipy.io.netcdf_file(str(p), "r", mmap=False)
    A = {k: (v.decode("latin-1") if isinstance(v, bytes) else v) for k, v in f._attributes.items()}
    V = f.variables

    def scaled(name):
        v = V[name]
        s = float(getattr(v, "scale_factor", 1.0) or 1.0)
        o = float(getattr(v, "add_offset", 0.0) or 0.0)
        return np.asarray(v.data, dtype=np.float64) * s + o

    def scalar(name):
        if name in V:
            return float(np.asarray(V[name].data).ravel()[0])
        return float(A[name]) if name in A else None

    out = {"reader": f"scipy.io.netcdf_file (scipy {__import__('scipy').__version__})"}
    if "ordinate_values" in V:
        y = scaled("ordinate_values")
        to_s = 60.0 if str(A.get("retention_unit", "")).strip().lower().startswith("min") else 1.0
        dt = scalar("actual_sampling_interval") * to_s
        t0 = (scalar("actual_delay_time") or 0.0) * to_s
        x = (t0 + dt * np.arange(len(y))) / 60.0
        name = str(A.get("detector_name", "")).strip() or "ordinate_values"
        out["traces"] = [_chrom_trace(0, [name], [str(A.get("detector_unit", "")).strip()], x, [y], rate=1.0 / dt, rate_tol=1e-6)]
        if "peak_number" in f.dimensions and f.dimensions["peak_number"]:
            cols = [k for k, v in V.items() if v.dimensions == ("peak_number",)]
            if cols:
                arr = np.stack([np.asarray(V[k].data, dtype=np.float64) for k in cols])
                out["tables"] = [{"index": 0, "event_count": int(arr.shape[1]), "parameter_names": cols,
                                  "dtypes": [np.dtype(V[k].data.dtype).name for k in cols],
                                  "xxh3": xxhash.xxh3_128_hexdigest(np.ascontiguousarray(arr.astype("<f8")).tobytes())}]
    if "mass_values" in V:
        t = np.asarray(V["scan_acquisition_time"].data, dtype=np.float64)
        idx = np.asarray(V["scan_index"].data, dtype=np.int64)
        cnt = np.asarray(V["point_count"].data, dtype=np.int64)
        mz = scaled("mass_values")
        it = scaled("intensity_values")
        tic = np.asarray(V["total_intensity"].data, dtype=np.float64)
        pol = str(A.get("test_ionization_polarity", "")).lower()
        pol = "positive" if pol.startswith("pos") else "negative" if pol.startswith("neg") else "unknown"
        cen = str(A.get("experiment_type", "")).lower().startswith("centroid")
        out["spectra"] = {"scan_count": int(len(t)),
                          "scans": [_spectrum_record(i, t[i], mz[idx[i]:idx[i] + cnt[i]], it[idx[i]:idx[i] + cnt[i]], pol, cen, tic[i]) for i in _pick(len(t))]}
        step = (t[-1] - t[0]) / (len(t) - 1)
        if len(t) > 1 and np.all(np.abs(np.diff(t) - step) <= 0.01 * step):
            out["traces"] = [_chrom_trace(0, ["TIC"], [str(getattr(V["total_intensity"], "units", b"").decode("latin-1") if isinstance(getattr(V["total_intensity"], "units", b""), bytes) else getattr(V["total_intensity"], "units", ""))],
                                          t / 60.0, [tic], rate=1.0 / step, rate_tol=1e-9)]
    if "spectra" in out:
        out["spectra"] = out.pop("spectra")  # last: main() writes the scans one per line
    return out


def waters(p: Path) -> dict:
    """Waters .raw through rainbow (black box). rainbow's MRM values are exactly twice the values
    whose per-scan sum equals the TIC stored in _FUNCnnn.IDX; the oracle halves them (exact in
    binary floating point). Column names are built from _FUNCTNS.INF at the offsets documented in
    docs/formats/waters-raw.md (rainbow reports only the set masses)."""
    import struct
    rb = _rainbow()
    d = rb.read(str(p))
    fn = (p / "_FUNCTNS.INF").read_bytes() if (p / "_FUNCTNS.INF").exists() else b""
    tables = []
    for f in sorted(d.datafiles, key=lambda f: f.name):
        if not f.name.upper().startswith("_FUNC"):
            continue
        num = int(f.name[5:8])
        blk = fn[(num - 1) * 416: num * 416]
        a = [x for x in struct.unpack_from("<32f", blk, 0xA0)] if len(blk) == 416 else []
        b = [x for x in struct.unpack_from("<32f", blk, 0x120)] if len(blk) == 416 else []
        a = a[:next((i for i, v in enumerate(a) if v == 0), len(a))]
        b = b[:next((i for i, v in enumerate(b) if v == 0), len(b))]
        data = np.asarray(f.data, dtype=np.float64) / 2.0
        fm = lambda v: str(np.float32(v)).removesuffix(".0")  # shortest f32 text, as Rust prints it
        names = [f"{fm(a[i])} > {fm(b[i])}" if i < len(b) else fm(a[i]) for i in range(data.shape[1])]
        hashes = {"rt_min": _col_hash(np.asarray(f.xlabels, dtype=np.float32).astype(np.float64))}
        for k, nme in enumerate(names):
            hashes[nme] = _col_hash(data[:, k])
        tables.append({"index": num - 1, "event_count": int(data.shape[0]), "parameter_names": [],
                       "column_hashes": hashes, "polarity": (f.metadata or {}).get("polarity")})
    return {"reader": "rainbow-api (black box)", "tables": tables,
            "oracle_note": "values = rainbow / 2 (see waters() in oracle/gen.py)"}
PLATE_SUFFIXES = {".txt", ".csv", ".xlsx", ".xls"}


def plate_oracle(p: Path) -> dict:
    """Plate-reader exports: allotropy (MIT) ASM output summarized per detection mode (oracle/plate.py)."""
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import plate
    return plate.plate(p)


def _is_zarr(p: Path) -> bool:
    """An OME-Zarr store: a `.zarr` directory (or one holding zarr.json/.zgroup), or a zip whose
    entries (at the root or under one top directory) include zarr.json/.zgroup."""
    import zipfile
    if p.is_dir():
        return p.name.lower().endswith(".zarr") or (p / "zarr.json").exists() or (p / ".zgroup").exists()
    if p.suffix.lower() == ".zip" and zipfile.is_zipfile(p):
        names = zipfile.ZipFile(p).namelist()
        return any(n.split("/")[-1] in ("zarr.json", ".zgroup") for n in names[:200])
    return False


def main():
    """gen.py [--id ID] PATH [PATH ...]; --id applies to the single PATH that follows it."""
    import hcs as _hcs  # high-content screening plates (oracle/hcs.py)
    OUT.mkdir(parents=True, exist_ok=True)
    args = sys.argv[1:]
    todo = []
    export = None
    while args:
        a = args.pop(0)
        if a == "--id":
            todo.append((args.pop(0), Path(args.pop(0)), export))
            export = None
        elif a == "--export":
            export = Path(args.pop(0))
        else:
            todo.append((None, Path(a), export))
            export = None
    for given_id, p, export in todo:
        ext = p.suffix.lower()
        is_tiff = ext in TIFF_SUFFIXES or p.name.lower().endswith(".companion.ome")
        # A .lifext sidecar shares its stem with its .lif; its oracle id gets a -lifext suffix.
        fid = given_id or re.sub(r"\.(lif|lof|xlif|xlef|xlcf|czi|nd2|fcs|lmd|abf|atf|jdx|dx|jcamp|jcm|raw|wiff|mzml|mzxml|imzml|mrc|mrcs|map|rec|st|ali|dm3|dm4|dm5|ser|emd|oir|vsi|zip|zarr|ims|nwb|zvi|oib|oif|jdf|dat|spc|mrxs)$", "",
                                 re.sub(r"\.lifext$", "-lifext", p.name, flags=re.I), flags=re.I)
        if (is_tiff or ext in (".nd", ".dcimg")) and not given_id:
            fid = _corpus_id(p)
        elif ext[1:] in EPHYS and not given_id:
            fid = p.name.replace(".", "-")  # multi-file formats share stems: keep the extension
        elif ext in PLATE_SUFFIXES and not given_id:
            fid = p.stem  # plate corpus ids are the file stems
        em = {e: mrc for e in ("mrc", "mrcs", "map", "rec", "st", "ali", "ccp4")} | {"dm3": dm, "dm4": dm, "dm5": dm, "ser": ser, "emd": emd}
        try:
            dispatch = {"nd": metamorph_nd, "dcimg": dcimg_, "czi": czi, "nd2": nd2_, "fcs": fcs, "lmd": fcs, "abf": abf, "atf": atf, "raw": thermo, "mzml": mzml_, "mzxml": mzxml_, "imzml": imzml_, "oir": oir, "vsi": vsi, "ims": ims, "nwb": nwb, "zvi": zvi, "oib": oif_, "oif": oif_, "mrxs": mirax, **EPHYS, **em}
            import spectro

            if _hcs.is_hcs(p):
                data = _hcs.hcs_plate(p)
            elif p.is_file() and (sk := spectro.kind(p)):
                data = getattr(spectro, sk)(p, MAX_SWEEPS)
            elif p.is_dir() and (p / "AcqData" / "MSScan.bin").exists():
                if export is None:
                    raise ValueError("an Agilent MassHunter .d needs --export <depositor mzML> before its path")
                data = agilent_ms(p, export)
            elif p.is_dir() and ext == ".d" and not any((p / f).exists() for f in ("analysis.tdf", "analysis.tsf")):
                data = chemstation(p)
            elif p.is_dir() and ext == ".raw" and export is not None:
                data = waters_ms(p, export)
            elif p.is_dir() and ext == ".raw":
                data = waters(p)
            elif ext == ".dat" and p.is_file() and _is_heka(p):
                data = heka(p)
            elif ext == ".raw" and p.is_file() and export is not None:
                data = thermo(p, export)  # the given export (mzML, mzXML or ANDI-MS)
            elif ext == ".pl2" and export is not None:
                data = pl2_plx(p, export)
            elif ext == ".scn" and _is_image_lab(p):
                data = biorad_scn(p)
            elif ext == ".wiff":
                if export is None:
                    raise ValueError("a Sciex .wiff needs --export <depositor mzML/mzXML> before its path")
                data = sciex_wiff(p, export)
            elif ext == ".gz" and p.name.lower().endswith((".mzml.gz", ".mzxml.gz", ".mrc.gz", ".map.gz", ".mrcs.gz", ".rec.gz", ".st.gz", ".ccp4.gz")):
                data = gz_(p)
            elif ext == ".mzmlb":
                data = mzmlb_(p)
            elif ext in (".ch", ".uv", ".ms"):
                data = chemstation(p)
            elif ext == ".cdf":
                data = andi(p)
            elif _is_zarr(p):
                data = ome_zarr_(p)
            elif p.is_dir() and _ephys_dir(p):
                data = _ephys_dir(p)(p)
            elif _is_varian_dir(p):
                data = varian(p)
            elif p.is_dir():
                data = bruker(p)
            elif ext == ".jdf":
                data = jeol(p)
            elif ext in PLATE_SUFFIXES:
                data = plate_oracle(p)
            elif ext in (".pda", ".sda", ".xpt"):
                if export is None:
                    raise ValueError("a plate-reader binary document needs --export <depositor text/XLSX export> before its path")
                sys.path.insert(0, str(Path(__file__).resolve().parent))
                import plate
                data = plate.plate_export(p, export)
            elif ext == ".dx" and zipfile.is_zipfile(p):
                import openlab_cds  # Agilent OpenLab CDS injection (a zip; JCAMP-DX .dx is text)

                data = openlab_cds.dx(p)
            elif ext in (".jdx", ".dx", ".jcamp", ".jcm"):
                data = jcampdx(p)
            else:
                data = tiff(p) if is_tiff else dispatch.get(ext[1:], lif)(p)
        except Exception as e:  # record failures too: they are information
            data = {"error": f"{type(e).__name__}: {e}"}
        if p.is_dir():
            head = {"id": fid, "file": p.name, "size": _dir_size(p)}
        else:
            head = {"id": fid, "file": p.name, "size": p.stat().st_size, "sha256": sha256(p)}
        data = {**head, "max_planes_hashed": MAX_PLANES, **data}
        out = OUT / f"{fid}.json"
        # Spectra and chromatogram oracles hold one record per scan / trace; keep them compact
        # (one record per line).
        if "spectra" in data:
            scans = data["spectra"].pop("scans")
            # ORACLE_SPECTRA_STRIDE=N keeps every N-th scan (and the last) of a very large export;
            # the harness compares the scans the oracle holds, by index.
            stride = int(os.environ.get("ORACLE_SPECTRA_STRIDE", "1"))
            if stride > 1:
                scans = [s_ for k, s_ in enumerate(scans) if k % stride == 0 or k == len(scans) - 1]
                data["spectra"]["stride"] = stride
            text = json.dumps(data, indent=1, default=str)
            body = ",\n".join(json.dumps(s_, separators=(",", ":"), default=str) for s_ in scans)
            text = text.rstrip()[:-1].rstrip()[:-1] + ',\n  "scans": [\n' + body + "\n ]\n }\n}\n"
            data["spectra"]["scans"] = scans
            json.loads(text)  # must stay valid JSON
        elif "chromatograms" in data:
            traces = data.pop("chromatograms")
            text = json.dumps(data, indent=1, default=str).rstrip()[:-1].rstrip()
            body = ",\n".join(json.dumps(t, separators=(",", ":"), default=str) for t in traces)
            text += ',\n "chromatograms": [\n' + body + "\n ]\n}\n"
            data["chromatograms"] = traces
            json.loads(text)
        else:
            text = json.dumps(data, indent=1, default=str)
        # reader error messages can quote the local path; keep committed ground truth machine-independent
        text = json.dumps(data, indent=1, default=str)
        for d in {str(p.parent), str(p.resolve().parent), str(p), str(p.resolve())}:
            text = text.replace(d + "/", "")
        out = oracle_json.write_text(out, text)  # committed ground truth over 1 MiB: <id>.json.gz
        skipped = sum(1 for i in data.get("images", []) if i.get("skip"))
        note = f" ({skipped} SKIPPED: oracle could not read them)" if skipped else ""
        if "spectra" in data:
            what = f"{data['spectra']['scan_count']} spectra"
        elif "chromatograms" in data:
            what = f"{len(data['chromatograms'])} chromatograms"
        else:
            kind = "images" if "images" in data else "tables" if "tables" in data else "traces"
            what = f"{len(data.get(kind, []))} {kind}{note}"
        print("wrote", out.name, "error: " + data["error"] if "error" in data else what)

if __name__ == "__main__":
    main()
