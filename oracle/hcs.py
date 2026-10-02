"""Ground truth for high-content screening plates (openreadout-hcs): Harmony `Index.idx.xml`,
ImageXpress `.HTD` (or a plate folder without one) and CellVoyager `MeasurementData.mlf`.

Three independent sources, none of them openreadout:

1. The plate index itself, parsed with the Python standard library (xml.etree / text): which
   wells, fields, channels, Z planes and time points the plate records, each plane's file name,
   channel names and pixel size.
2. tifffile (BSD-3) on every plane file present on disk: the plane's pixels (xxh3-128 of the
   little-endian samples, the hash `openreadout check --planes` computes) and, for MetaXpress files,
   the MetaSeries/STK calibration and illumination names.
3. Bio-Formats 8.5.0 (GPL; run as a black box from oracle/bftools):
   `showinf -nopix -omexml` for the series geometry, the OME Plate/Well/WellSample layout,
   physical sizes and channel names, and `bfconvert` of every series whose files are present,
   hashed per plane. Bio-Formats reads missing and never-acquired planes as blank.

Conventions shared with docs/formats/hcs.md (our documented model, applied here to the index
the oracle parsed itself): one image per (well, field) ordered by row, column, field number;
channels ordered by their channel number (Harmony ChannelID, CellVoyager Ch, ImageXpress wave),
Z and T by their recorded index; a plane the index lists without a file name is `not_recorded`,
one with no record is `not_acquired`, one whose named file is absent is `missing`.
"""
from __future__ import annotations

import os
import re
import subprocess
import tempfile
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path

import numpy as np

import gen

MAX_BF_SERIES = int(os.environ.get("ORACLE_HCS_MAX_SERIES", "16"))


def local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1].rsplit(":", 1)[-1]


def is_hcs(p: Path) -> bool:
    n = p.name.lower()
    if n.endswith(".idx.xml") or n == "index.xml" or n == "imageindex.columbusidx.xml":
        return True
    if n.endswith(".flex"):
        return True
    if p.is_dir() and _folder_index(p) is None and any(q.suffix.lower() == ".flex" for q in p.iterdir()):
        return True
    if n.endswith(".htd") or n == "measurementdata.mlf":
        return True
    if p.is_dir():
        if _folder_index(p) is not None:
            return True
        names = [q.name for q in p.iterdir()]
        return sum(1 for q in names if _IX_NAME.match(q)) >= 2
    return False


def _folder_index(p: Path):
    """The index file of a plate folder (Harmony measurement folder, CellVoyager measurement
    folder, ImageXpress folder with its HTD), or None."""
    for q in (p / "Images" / "Index.idx.xml", p / "Index.idx.xml", p / "Images" / "Index.xml", p / "Index.xml",  # Index.xml: Harmony V6+
              p / "ImageIndex.ColumbusIDX.xml", p / "MeasurementData.mlf"):
        if q.is_file():
            return q
    htd = sorted(q for q in p.iterdir() if q.suffix.lower() == ".htd")
    return htd[0] if htd else None


def row_name(r: int) -> str:
    s = ""
    n = r
    while True:
        s = chr(ord("A") + n % 26) + s
        if n < 26:
            return s
        n = n // 26 - 1


def well_name(r: int, c: int) -> str:
    return f"{row_name(r)}{c + 1:02d}"


# --------------------------------------------------------------------------------------------
# index parsers: each returns (plate dict, records) where a record is
#   {"row": r0, "col": c0, "field": f, "channel": key, "z": zkey, "t": tkey, "file": name or ""}
# and plate has "channels": [(key, name)], "pixel_size_um": [x, y], "id", "rows", "columns".
# --------------------------------------------------------------------------------------------

def harmony_index(p: Path):
    """Harmony / Columbus index. Harmony 6 and 7 write the channel description (ChannelName,
    ImageResolutionX/Y, ImageSizeX/Y, ...) once per channel in `Maps/Map/Entry[@ChannelID]`
    instead of in every `Image`; an `Image`'s own value wins, the Map entry fills the rest."""
    recs = []
    plate = {"channels": {}, "pixel_size_um": None}
    maps: dict[int, dict] = {}
    for ev, el in ET.iterparse(p, events=("end",)):
        t = local(el.tag)
        if t == "Entry" and el.get("ChannelID") is not None:
            e = maps.setdefault(int(el.get("ChannelID")), {})
            for c in el:
                e.setdefault(local(c.tag), (c.text or "").strip())
            el.clear()
            continue
        if t == "Image" and el.find("./*") is not None and any(local(c.tag) == "URL" for c in el):
            d = {local(c.tag): (c.text or "").strip() for c in el}
            if not d.get("Row"):
                el.clear()
                continue
            ch = int(d.get("ChannelID") or 0)
            if ch not in plate["channels"]:
                plate["channels"][ch] = d.get("ChannelName") or (maps.get(ch) or {}).get("ChannelName")
            m = {**(maps.get(ch) or {}), **{k: v for k, v in d.items() if v}}
            if plate["pixel_size_um"] is None and m.get("ImageResolutionX"):
                plate["pixel_size_um"] = [float(m["ImageResolutionX"]) * 1e6, float(m["ImageResolutionY"]) * 1e6]
                plate["size"] = [int(m["ImageSizeX"]), int(m["ImageSizeY"])]
            url = d.get("URL", "")
            url_el = next(c for c in el if local(c.tag) == "URL")
            page = int(url_el.get("BufferNo") or 0)  # Columbus: the page of a multi-page file
            recs.append({"page": page, "row": int(d["Row"]) - 1, "col": int(d["Col"]) - 1, "field": int(d.get("FieldID") or 1),
                         "channel": ch, "z": int(d.get("PlaneID") or 1), "t": int(d.get("TimepointID") or 0),
                         "file": url.rsplit("/", 1)[-1].rsplit("\\", 1)[-1] if url else ""})
            el.clear()
        elif t == "PlateID":
            plate["id"] = (el.text or "").strip()
        elif t == "PlateRows":
            plate["rows"] = int(el.text)
        elif t == "PlateColumns":
            plate["columns"] = int(el.text)
        elif t == "PlateTypeName":
            plate["plate_type"] = (el.text or "").strip()
    plate["channels"] = sorted(plate["channels"].items())
    return plate, recs


def flex_index(p: Path):
    """Standalone Opera .flex file(s): the XML document in TIFF tag 65200 of each file, read with
    tifffile (BSD-3) and parsed with xml.etree. One record per `Well/Images/Image`: page =
    `@BufferNo`, channel = `ExposureNo` (named after the page's `Arrays/Array@Name`), z = `Stack`,
    field = `Sublayout`; well from `WellCoordinate@Row/@Col` (1-based). A folder is the plate of
    all its .flex files."""
    import tifffile
    files = sorted(q for q in p.iterdir() if q.suffix.lower() == ".flex") if p.is_dir() else [p]
    recs = []
    plate = {"channels": {}, "pixel_size_um": None}
    for f in files:
        with tifffile.TiffFile(f) as tf:
            xml = tf.pages[0].tags[65200].value
        if isinstance(xml, bytes):
            xml = xml.decode("utf-8", "replace")
        root = ET.fromstring(xml.strip("\0"))
        arrays = [a.get("Name") for a in root.findall("./Arrays/Array")]
        flex = root.find("./FLEX")
        pl = flex.find("./Plate")
        if pl is not None:
            plate["id"] = (pl.findtext("Barcode") or "").strip() or None
            plate["rows"] = int(pl.findtext("XSize")) if pl.findtext("XSize") else None
            plate["columns"] = int(pl.findtext("YSize")) if pl.findtext("YSize") else None
        wc = flex.find("./Well/WellCoordinate")
        row, col = int(wc.get("Row")), int(wc.get("Col"))
        for im in flex.findall("./Well/Images/Image"):
            page = int(im.get("BufferNo"))
            ch = int(im.findtext("ExposureNo") or 1)
            plate["channels"].setdefault(ch, arrays[page] if page < len(arrays) else None)
            if plate["pixel_size_um"] is None and im.findtext("ImageResolutionX"):
                plate["pixel_size_um"] = [float(im.findtext("ImageResolutionX")) * 1e6, float(im.findtext("ImageResolutionY")) * 1e6]
                plate["size"] = [int(im.findtext("ImageWidth")), int(im.findtext("ImageHeight"))]
            recs.append({"page": page, "row": row - 1, "col": col - 1, "field": int(im.findtext("Sublayout") or 1),
                         "channel": ch, "z": int(im.findtext("Stack") or 1), "t": 0, "file": f.name})
    plate["channels"] = sorted(plate["channels"].items())
    return plate, recs


def cellvoyager_index(p: Path):
    d = p.parent
    mrf = ET.parse(d / "MeasurementDetail.mrf").getroot()
    attrs = {local(k): v for k, v in mrf.attrib.items()}
    chans = []
    size = None
    px = None
    for c in mrf:
        if local(c.tag) == "MeasurementChannel":
            a = {local(k): v for k, v in c.attrib.items()}
            chans.append(int(a["Ch"]))
            if size is None:
                size = [int(a["HorizontalPixels"]), int(a["VerticalPixels"])]
                px = [float(a["HorizontalPixelDimension"]), float(a["VerticalPixelDimension"])]
        elif local(c.tag) == "MeasurementSamplePlate":
            plate_name = {local(k): v for k, v in c.attrib.items()}.get("Name")
    # channel names: the .mes Target when every channel's target differs, else the emission filter
    names = {}
    mes = d / attrs.get("MeasurementSettingFileName", "")
    if mes.is_file():
        for c in ET.parse(mes).getroot().iter():
            if local(c.tag) == "Channel" and any(local(k) == "Target" for k in c.attrib):
                a = {local(k): v for k, v in c.attrib.items()}
                names[int(a["Ch"])] = (a.get("Target") or "", a.get("Acquisition") or "")
    targets = [names.get(c, ("", ""))[0] for c in chans]
    distinct = len(set(targets)) == len(chans) and all(targets)
    chan_names = [(c, (names.get(c, ("", ""))[0] if distinct else (names.get(c, ("", ""))[1] or names.get(c, ("", ""))[0])) or f"Ch{c}") for c in chans]
    recs = []
    for ev, el in ET.iterparse(p, events=("end",)):
        if local(el.tag) == "MeasurementRecord":
            a = {local(k): v for k, v in el.attrib.items()}
            if a.get("Type", "IMG") == "IMG" and a.get("Row"):
                recs.append({"row": int(a["Row"]) - 1, "col": int(a["Column"]) - 1, "field": int(a.get("FieldIndex") or 1),
                             "channel": int(a["Ch"]), "z": int(a.get("ZIndex") or 1), "t": int(a.get("TimePoint") or 1),
                             "file": (el.text or "").strip()})
            el.clear()
    plate = {"id": plate_name, "rows": int(attrs["RowCount"]), "columns": int(attrs["ColumnCount"]),
             "channels": chan_names, "pixel_size_um": px, "size": size}
    return plate, recs


_IX_NAME = re.compile(r"^(?P<prefix>.+?)_(?P<well>[A-Z]{1,2}\d{2,3})(?:_s(?P<site>\d+))?(?:_w(?P<wave>\d+))?(?P<thumb>_thumb)?"
                      r"(?:_?\[?[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\]?)?\.tiff?$", re.I)


def _parse_well(w: str):
    m = re.match(r"([A-Z]{1,2})(\d+)$", w)
    letters, col = m.group(1), int(m.group(2))
    row = 0
    for i, ch in enumerate(letters):
        v = ord(ch) - ord("A")
        row = v if i == 0 else (row + 1) * 26 + v
    return row, col - 1


def _metamorph(path: Path):
    """(illumination/name, pixel size x, y) of one MetaXpress plane file via tifffile."""
    import tifffile
    with tifffile.TiffFile(path) as tf:
        if tf.is_metaseries:
            md = tf.metaseries_metadata
            pl = md.get("PlaneInfo", {})
            name = pl.get("_IllumSetting_") or pl.get("image-name")
            if pl.get("spatial-calibration-state"):
                return name, pl.get("spatial-calibration-x"), pl.get("spatial-calibration-y")
            return name, None, None
        if tf.is_stk:
            md = tf.stk_metadata
            if md.get("SpatialCalibration") and md.get("CalibrationUnits") in ("um", "µm"):
                return md.get("Name"), md.get("XCalibration"), md.get("YCalibration")
            return md.get("Name"), None, None
    return None, None, None


def imagexpress_index(p: Path):
    htd = None
    base = p if p.is_dir() else p.parent
    if not p.is_dir():
        htd = {}
        for line in p.read_bytes().decode("latin-1").splitlines():
            parts = [x.strip().strip('"') for x in re.split(r',(?=(?:[^"]*"[^"]*")*[^"]*$)', line.strip())]
            if parts and parts[0]:
                if parts[0] == "EndFile":
                    break
                htd[parts[0]] = parts[1:]
    files = {}
    for q in sorted(base.iterdir()):
        m = _IX_NAME.match(q.name)
        if not m or m.group("thumb"):
            continue
        r, c = _parse_well(m.group("well"))
        files[(m.group("prefix"), r, c, int(m.group("site") or 1), int(m.group("wave") or 1))] = q.name
    prefix = p.stem if htd is not None else max({k[0] for k in files}, key=lambda x: sum(1 for k in files if k[0] == x))
    files = {k[1:]: v for k, v in files.items() if k[0] == prefix}
    recs = []
    if htd is not None:
        rows, cols = int(htd["YWells"][0]), int(htd["XWells"][0])
        nw = int(htd.get("NWavelengths", ["1"])[0]) if htd.get("Waves", ["TRUE"])[0] == "TRUE" else 1
        waves = [w for w in range(1, nw + 1) if htd.get(f"WaveCollect{w}", ["1"])[0] != "0"]
        names = [(w, htd.get(f"WaveName{w}", [None])[0]) for w in waves]
        if htd.get("Sites", ["FALSE"])[0] == "TRUE":
            xs, ys = int(htd["XSites"][0]), int(htd["YSites"][0])
            sel = [v for r in range(ys) for v in htd.get(f"SiteSelection{r + 1}", ["TRUE"] * xs)]
            sites = list(range(1, sum(1 for v in sel if v == "TRUE") + 1))
        else:
            sites = [1]
        for r in range(rows):
            for c, v in enumerate(htd.get(f"WellsSelection{r + 1}", [])):
                if v != "TRUE":
                    continue
                for s in sites:
                    for w in waves:
                        recs.append({"row": r, "col": c, "field": s, "channel": w, "z": 1, "t": 1,
                                     "file": files.get((r, c, s, w), "?missing")})
        plate = {"id": p.stem, "rows": rows, "columns": cols}
    else:
        for (r, c, s, w), n in files.items():
            recs.append({"row": r, "col": c, "field": s, "channel": w, "z": 1, "t": 1, "file": n})
        waves = sorted({k[3] for k in files})
        names = [(w, None) for w in waves]
        plate = {"id": None}
    # names and calibration from one plane file per wavelength (tifffile)
    chans = []
    px = None
    for w, n in names:
        f = next((v for k, v in files.items() if k[3] == w), None)
        meta = _metamorph(base / f) if f else (None, None, None)
        chans.append((w, n or meta[0] or f"w{w}"))
        if px is None and meta[1]:
            px = [meta[1], meta[2]]
    plate["channels"] = chans
    plate["pixel_size_um"] = px
    return plate, recs


# --------------------------------------------------------------------------------------------

def _bf_ome(p: Path):
    text = gen._showinf(p, "-omexml")
    i, j = text.find("<OME"), text.rfind("</OME>")
    if i < 0:
        return None
    root = ET.fromstring(text[i:j + 6])
    ns = {"o": root.tag.split("}")[0].strip("{")}
    images = []
    for im in root.findall("o:Image", ns):
        px = im.find("o:Pixels", ns)
        images.append({
            "id": im.get("ID"), "name": im.get("Name"),
            "size": [int(px.get("SizeX")), int(px.get("SizeY"))], "size_c": int(px.get("SizeC")),
            "size_z": int(px.get("SizeZ")), "size_t": int(px.get("SizeT")), "type": px.get("Type"),
            "order": px.get("DimensionOrder"),
            "physical_size_um": [float(px.get("PhysicalSizeX")) if px.get("PhysicalSizeX") else None,
                                 float(px.get("PhysicalSizeY")) if px.get("PhysicalSizeY") else None],
            "channels": [c.get("Name") for c in px.findall("o:Channel", ns)],
        })
    wells = []  # (row, col, [(sample index, image id)])
    for pl in root.findall("o:Plate", ns):
        for w in pl.findall("o:Well", ns):
            samples = sorted((int(s.get("Index")), s.find("o:ImageRef", ns).get("ID"))
                             for s in w.findall("o:WellSample", ns) if s.find("o:ImageRef", ns) is not None)
            wells.append((int(w.get("Row")), int(w.get("Column")), samples))
    return images, wells


def _bf_series_planes(p: Path, series: int, order: str, C: int, Z: int, T: int):
    import tifffile
    with tempfile.TemporaryDirectory() as td:
        out = Path(td) / "s.ome.tif"
        subprocess.run([gen._bftools("bfconvert"), "-no-upgrade", "-overwrite", "-series", str(series), str(p), str(out)],
                       capture_output=True, text=True, timeout=7200, check=True)
        res = {}
        with tifffile.TiffFile(out) as tf:
            pages = tf.pages
            size = {"C": C, "Z": Z, "T": T}
            for c in range(C):
                for z in range(Z):
                    for t in range(T):
                        idx = {"C": c, "Z": z, "T": t}
                        page, stride = 0, 1
                        for ax in order[2:]:
                            page += idx[ax] * stride
                            stride *= size[ax]
                        a = pages[page].asarray()
                        res[(c, z, t)] = (gen.h(a), bool(np.all(a == 0)))
        return res


def hcs_plate(p: Path) -> dict:
    import tifffile
    if p.is_dir() and _folder_index(p) is not None:
        p = _folder_index(p)
    n = p.name.lower()
    flex = n.endswith(".flex") or (p.is_dir() and _folder_index(p) is None and any(q.suffix.lower() == ".flex" for q in p.iterdir()))
    if flex:
        kind, (plate, recs) = "opera-harmony", flex_index(p)
        root = p if p.is_dir() else p.parent
    elif n.endswith(".xml"):
        kind, (plate, recs) = "opera-harmony", harmony_index(p)
        root = p.parent
    elif n.endswith(".mlf"):
        kind, (plate, recs) = "cellvoyager", cellvoyager_index(p)
        root = p.parent
    else:
        kind, (plate, recs) = "imagexpress", imagexpress_index(p)
        root = p if p.is_dir() else p.parent
    ch_keys = [k for k, _ in plate["channels"]]
    ch_index = {k: i for i, k in enumerate(ch_keys)}
    zs = sorted({r["z"] for r in recs})
    ts = sorted({r["t"] for r in recs})
    zi = {z: i for i, z in enumerate(zs)}
    ti = {t: i for i, t in enumerate(ts)}
    fields = sorted({(r["row"], r["col"], r["field"]) for r in recs})
    findex = {f: i for i, f in enumerate(fields)}
    slots = defaultdict(dict)
    for r in recs:
        if r["channel"] not in ch_index:
            continue
        key = (ch_index[r["channel"]], zi[r["z"]], ti[r["t"]])
        slots[findex[(r["row"], r["col"], r["field"])]].setdefault(key, (r["file"], r.get("page", 0)))
    C, Z, T = len(ch_keys), len(zs), len(ts)
    present = {q.name for q in root.iterdir()} if root.is_dir() else set()
    counts = {"present": 0, "missing": 0, "not_recorded": 0, "not_acquired": 0}
    images = []
    by_well = defaultdict(list)
    for i, (r, c, f) in enumerate(fields):
        by_well[(r, c)].append(i)
        planes = []
        for cc in range(C):
            for z in range(Z):
                for t in range(T):
                    name, page = slots[i].get((cc, z, t), (None, 0))
                    if name is None:
                        state = "not_acquired"
                    elif name == "":
                        state = "not_recorded"
                    elif name in present:
                        state = "present"
                    else:
                        state = "missing"
                    counts[state] += 1
                    planes.append({"c": cc, "z": z, "t": t, "state": state, "file": name if name and name != "?missing" else None, "page": page})
        if any(pl["state"] == "present" for pl in planes):
            for pl in planes:
                if pl["state"] == "present":
                    with tifffile.TiffFile(root / pl["file"]) as tf:
                        a = tf.pages[pl["page"]].asarray()
                    pl["xxh3"] = gen.h(a)
                    pl["dtype"] = gen.dtype_name(a.dtype)
                    pl["shape"] = list(a.shape)
            images.append({"index": i, "well": well_name(r, c), "field": f, "planes": planes})
        elif sum(1 for im in images if not any(pl.get("xxh3") for pl in im["planes"])) < 3 and any(pl["state"] == "missing" for pl in planes):
            # a few fields with no file on disk: reading them must report the missing files
            images.append({"index": i, "well": well_name(r, c), "field": f, "planes": planes})
        elif sum(1 for im in images if all(pl["state"] == "not_recorded" for pl in im["planes"])) < 2 and all(pl["state"] == "not_recorded" for pl in planes):
            # fields the index lists without any image: they read as blank
            images.append({"index": i, "well": well_name(r, c), "field": f, "planes": planes})
    first = next((pl for im in images for pl in im["planes"] if pl.get("xxh3")), None)
    images.sort(key=lambda im: im["index"])
    out = {
        "reader": "Python stdlib index parse + tifffile %s; Bio-Formats 8.5.0 showinf/bfconvert (GPL, black box)" % tifffile.__version__,
        "hcs": {
            "format": kind,
            "plate": {"id": plate.get("id"), "rows": plate.get("rows"), "columns": plate.get("columns"),
                      "wells_imaged": len(by_well), "fields": len(fields), "planes_expected": len(fields) * C * Z * T,
                      "planes_missing": counts["missing"], "planes_absent": counts["not_recorded"] + counts["not_acquired"],
                      "planes_not_recorded": counts["not_recorded"], "planes_present": counts["present"]},
            "channels": [nm for _, nm in plate["channels"]],
            "size_c": C, "size_z": Z, "size_t": T,
            "size": plate.get("size") or (first["shape"][::-1] if first else None),
            "pixel_type": first["dtype"] if first else None,
            "pixel_size_um": plate.get("pixel_size_um"),
            "wells": [{"well": well_name(r, c), "images": v} for (r, c), v in sorted(by_well.items())],
            "images": images,
        },
    }
    # Bio-Formats second opinion (not for folders without an index file)
    bf_target = p
    if kind == "cellvoyager":
        wpi = sorted(root.glob("*.wpi"))
        bf_target = wpi[0] if wpi else None
    if kind == "imagexpress" and p.is_dir():
        bf_target = None
    if flex and p.is_dir():
        bf_target = sorted(q for q in p.iterdir() if q.suffix.lower() == ".flex")[0]  # Bio-Formats reads the folder from one file
    if bf_target is not None:
        try:
            out["hcs"]["bioformats"] = _bioformats(bf_target, fields, images, C, Z, T, ch_keys, kind)
        except Exception as e:  # recorded, never fatal
            out["hcs"]["bioformats"] = {"error": f"{type(e).__name__}: {e}"}
    return out


def _bioformats(p: Path, fields, images, C, Z, T, ch_keys, kind) -> dict:
    parsed = _bf_ome(p)
    if parsed is None:
        return {"error": "showinf printed no OME-XML"}
    bf_images, bf_wells = parsed
    id_to_series = {im["id"]: s for s, im in enumerate(bf_images)}
    # (row, col, k-th sample) -> series
    series_of = {}
    for r, c, samples in bf_wells:
        for k, (_, iid) in enumerate(samples):
            if iid in id_to_series:
                series_of[(r, c, k)] = id_to_series[iid]
    # our (row, col, k-th field of the well) for every field
    kth = {}
    seen = defaultdict(int)
    for i, (r, c, f) in enumerate(fields):
        kth[i] = (r, c, seen[(r, c)])
        seen[(r, c)] += 1
    res = {"series": len(bf_images), "size_c": sorted({im["size_c"] for im in bf_images}),
           "size_z": sorted({im["size_z"] for im in bf_images}), "size_t": sorted({im["size_t"] for im in bf_images}),
           "sizes": sorted({tuple(im["size"]) for im in bf_images}), "types": sorted({im["type"] for im in bf_images}),
           "physical_size_um": bf_images[0]["physical_size_um"] if bf_images else None,
           "channels": bf_images[0]["channels"] if bf_images else [],
           "fields_mapped": sum(1 for i in kth if kth[i] in series_of)}
    # Bio-Formats orders CellVoyager channels by acquisition action ("Action #1, Channel #3, ..."):
    # map its channel index to ours through the channel number in the name.
    cmap = {}
    if kind == "cellvoyager" and bf_images:
        for j, nm in enumerate(bf_images[0]["channels"]):
            m = re.search(r"Channel #(\d+)", nm or "")
            if m and int(m.group(1)) in ch_keys:
                cmap[ch_keys.index(int(m.group(1)))] = j
    agree = disagree = blank_absent = repeat_absent = compared = 0
    problems = []
    planes_out = []
    for im in [x for x in images if any(pl.get("xxh3") for pl in x["planes"])][:MAX_BF_SERIES]:
        key = kth[im["index"]]
        s = series_of.get(key)
        if s is None:
            problems.append(f"no Bio-Formats series for image {im['index']} ({im['well']} field {im['field']})")
            continue
        b = bf_images[s]
        got = _bf_series_planes(p, s, b["order"], b["size_c"], b["size_z"], b["size_t"])
        for pl in im["planes"]:
            h = got.get((cmap.get(pl["c"], pl["c"]), pl["z"], pl["t"]))
            if h is None:
                continue
            if pl["state"] == "present":
                compared += 1
                if h[0] == pl["xxh3"]:
                    agree += 1
                else:
                    disagree += 1
                    problems.append(f"image {im['index']} c={pl['c']} z={pl['z']} t={pl['t']}: Bio-Formats {h[0]} != tifffile {pl['xxh3']}")
            elif pl["state"] in ("not_acquired", "not_recorded"):
                if h[1]:
                    blank_absent += 1
                elif any(h[0] == q.get("xxh3") for q in im["planes"] if q["c"] == pl["c"]):
                    repeat_absent += 1  # Bio-Formats repeats an acquired plane of the channel
            planes_out.append({"image": im["index"], "series": s, "c": pl["c"], "z": pl["z"], "t": pl["t"], "xxh3": h[0], "blank": h[1]})
    res.update({"planes_compared": compared, "planes_agree": agree, "planes_disagree": disagree,
                "absent_planes_blank": blank_absent,
                "absent_planes_repeated": repeat_absent, "problems": problems[:20]})
    return res
