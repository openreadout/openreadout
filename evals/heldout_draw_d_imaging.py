"""Held-out draw D (2026-10-06): light microscopy, electron microscopy, screening plates and gel imaging.

Part of evals/heldout_draw_d.py; the questions were written from the held-out oracles, the
depositors' exports and records, and facts computed with third-party readers, before OpenReadout
was run on any of these files.
"""

from __future__ import annotations

import re
from typing import Any

C: dict[str, str] = {
    # light microscopy
    "d_imaging_oxa1": "ho-zenodo18006455-czi-oxa1",
    "d_imaging_apotome": "ho-zenodo15042009-czi-apotome",
    "d_imaging_lsm710": "ho-zenodo13835874-czi-lsm710",
    "d_imaging_lysoip": "ho-zenodo14827155-czi-lysoip",
    "d_imaging_fishscale": "ho-zenodo18360756-nd2-fishscale",
    "d_imaging_txtl": "ho-zenodo20503086-nd2-txtl",
    "d_imaging_isim": "ho-zenodo6498340-nd2-isim",
    "d_imaging_opsin": "ho-zenodo7688944-lif-opsin",
    "d_imaging_pmpcb": "ho-zenodo14274136-lif-pmpcb",
    "d_imaging_raman": "ho-zenodo18692967-lif-raman",
    "d_imaging_oir": "ho-zenodo1490851-oir-hek293",
    "d_imaging_oib": "ho-zenodo4563053-oib-osn",
    "d_imaging_ims_owenia": "ho-zenodo4944369-ims-owenia",
    "d_imaging_ims_zebrafish": "ho-zenodo10937428-ims-zebrafish",
    "d_imaging_zvi": "ho-bsst1202-zvi-melanoma",
    "d_imaging_lsm": "ho-zenodo15500562-lsm-spectral",
    "d_imaging_imagej": "ho-omesample-condensation-imagej",
    "d_imaging_vsi": "ho-biad1129-vsi-feline",
    "d_imaging_vsi_ets1": "ho-biad1129-vsi-feline-ets-stack1",
    "d_imaging_vsi_ets2": "ho-biad1129-vsi-feline-ets-stack10001",
    "d_imaging_mirax": "ho-zenodo10966338-mrxs-fluo",
    "d_imaging_zarr": "ho-zenodo22078388-zarr-organoid",
    # gel imaging
    "d_imaging_scn_sio2": "ho-zenodo6465112-scn-sio2",
    "d_imaging_scn_dna": "ho-zenodo5898370-scn-dna",
    # electron microscopy
    "d_imaging_dm4_tfs": "ho-zenodo14827898-dm4-tfs",
    "d_imaging_dm3_profile": "ho-zenodo5716144-dm3-oam",
    "d_imaging_dm4_eds": "ho-zenodo10609594-dm4-eds",
    "d_imaging_ser": "ho-zenodo14007670-ser-coreshell",
    "d_imaging_emi": "ho-zenodo14007670-emi-coreshell",
    "d_imaging_emd_ceta": "ho-zenodo5716148-emd-ceta",
    "d_imaging_mrc_template": "ho-zenodo20184093-mrc-nucleosome",
    "d_imaging_mrc_tomo": "ho-zenodo22960491-mrc-ankyring",
    # high-content screening plates
    "d_imaging_harmony7": "ho-cpg0045-ncats-harmony7",
    "d_imaging_cv8000": "ho-cpg0016s5-eisai-cv8000",
    "d_imaging_ixm": "ho-cpg0030-bbbc022-ixm",
}

MIRAX_DATA = [f"Data{i:04d}.dat" for i in range(24)] + [
    "Index.dat",
    "Slidedat.ini",
]  # the slide's data folder (companion entries ho-zenodo10966338-mrxs-fluo-<file>)


# ---------------------------------------------------------------- facts (oracle venv only)


def _h():
    import heldout

    return heldout


def _margin(means: dict, factor: float = 1.02):
    s = sorted(means.values())
    assert s[-1] > factor * s[-2], means
    return max(means, key=means.get)


def czi_channel_names(cid: str) -> list[str]:
    """Channel names in channel order from the CZI metadata (czifile), one per Channel Id."""
    import czifile

    with czifile.CziFile(_h().hpath(cid)) as f:
        meta = f.metadata() if callable(getattr(f, "metadata", None)) else f.metadata
    names: dict[str, str] = {}
    for cid_, name in re.findall(r'<Channel Id="(Channel:\d+)" Name="([^"]*)"', str(meta)):
        names.setdefault(cid_, name)
    return [names[k] for k in sorted(names, key=lambda k: int(k.split(":")[1]))]


def czi_brightest_channel(cid: str, cross: bool = False):
    """Name of the channel with the highest mean over its plane: czifile, or pylibCZIrw (black box)."""
    import analysis as a

    arr, dims = a.czi_scene(_h().hpath(cid))
    assert dims == ("C", "Y", "X"), dims
    names = czi_channel_names(cid)
    if cross:
        st = a.czi_planes(_h().hpath(cid), [{"C": c, "Z": 0, "T": 0} for c in range(arr.shape[0])])
        means = {names[c]: float(st[c].mean(dtype="float64")) for c in range(arr.shape[0])}
        return _margin(means), _h().rd("pylibCZIrw") + " (black box)"
    means = {names[c]: float(arr[c].mean(dtype="float64")) for c in range(arr.shape[0])}
    return _margin(means), _h().rd("czifile")


def czi_brightest_z(cid: str, channel: int, cross: bool = False):
    """Optical section (counting from 1) with the highest mean in one channel."""
    import analysis as a

    arr, dims = a.czi_scene(_h().hpath(cid))
    assert dims == ("C", "Z", "Y", "X"), dims
    nz = arr.shape[1]
    if cross:
        st = a.czi_planes(_h().hpath(cid), [{"C": channel, "Z": z, "T": 0} for z in range(nz)])
        means = {z + 1: float(st[z].mean(dtype="float64")) for z in range(nz)}
        return _margin(means), _h().rd("pylibCZIrw") + " (black box)"
    means = {z + 1: float(arr[channel, z].mean(dtype="float64")) for z in range(nz)}
    return _margin(means), _h().rd("czifile")


def ims_channel_mean(cid: str, channel: int):
    """Mean of one channel at full resolution (h5py), cropped to the image size Imaris records."""
    import h5py

    with h5py.File(_h().hpath(cid), "r") as f:
        info = f["DataSetInfo/Image"].attrs

        def num(k):
            return int(b"".join(info[k]).decode())

        sx, sy, sz = num("X"), num("Y"), num("Z")
        d = f[f"DataSet/ResolutionLevel 0/TimePoint 0/Channel {channel}/Data"][:sz, :sy, :sx]
    return float(d.mean(dtype="float64")), _h().rd("h5py")


def dm_total_counts(cid: str, export: bool = False):
    """Sum of every value of the EDS spectrum image: dm3_lib on the .dm4, or h5py on the
    depositor's HyperSpy export."""
    import numpy as np

    if export:
        import h5py

        with h5py.File(_h().hpath(cid, "oracle-export"), "r") as f:
            d = f["Experiments/EDS Spectrum Image/data"][()]
        return int(d.sum(dtype=np.int64)), _h().rd("h5py") + " on the depositor's HyperSpy export"
    import dm3_lib

    d = dm3_lib.DM3(str(_h().hpath(cid))).imagedata
    assert d.shape == (2048, 104, 46), d.shape
    return int(np.asarray(d).sum(dtype=np.int64)), _h().rd("dm3_lib")


def emd_mean(cid: str):
    import analysis as a

    return a.emd_image_mean(cid)


def mrc_std(cid: str, numpy: bool = False):
    import analysis as a

    if numpy:
        return a.mrc_std(cid, a.mrc_numpy, a.NUMPY_MRC)
    return a.mrc_std(cid)


def harmony_root_brighter_well(cid: str, channel: int, cross: bool = False):
    """Of wells A01 and A02 (fields 1-2 in the copy), the one with the higher pooled mean of one channel:
    plane files found by the Harmony file-name convention next to Index.xml, or (`cross`) the plane
    file names Index.xml lists for that row, column and channel."""
    import analysis as a

    folder = _h().hpath(cid)
    means = {}
    if not cross:
        for col in (1, 2):
            files = sorted(folder.glob(f"r01c{col:02d}f*p01-ch{channel}sk1fk1fl1.tiff"))
            assert len(files) == 2, files
            means[f"A{col:02d}"] = a._tif_mean(files)
        return _margin(means), _h().rd("tifffile") + " (Harmony file names)"
    import xml.etree.ElementTree as ET

    urls: dict[str, list] = {"A01": [], "A02": []}
    for _, el in ET.iterparse(folder / "Index.xml", events=("end",)):
        if a._local(el.tag) == "Image":
            d = {a._local(c.tag): (c.text or "").strip() for c in el}
            if d.get("Row") == "1" and d.get("Col") in ("1", "2") and d.get("ChannelID") == str(channel):
                f = folder / d.get("URL", "")
                if f.is_file():
                    urls[f"A{int(d['Col']):02d}"].append(f)
            el.clear()
    means = {w: a._tif_mean(sorted(fs)) for w, fs in urls.items()}
    return _margin(means), _h().rd("tifffile") + " (planes Index.xml names)"


def cv_brighter_well(cid: str, channel: int, cross: bool = False):
    """Of wells A01 and A02 (fields 1-2 in the copy), the one with the higher pooled mean of one channel:
    CellVoyager file names (..._<well>_T0001F<field>L01A..Z..C<ch>.tif), or (`cross`) the plane files
    MeasurementData.mlf records for that row, column and channel."""
    import analysis as a

    folder = _h().hpath(cid)
    means = {}
    if not cross:
        for well in ("A01", "A02"):
            files = sorted(q for q in folder.glob(f"*_{well}_T0001F*L01A*Z01C{channel:02d}.tif"))
            assert len(files) == 2, files
            means[well] = a._tif_mean(files)
        return _margin(means), _h().rd("tifffile") + " (CellVoyager file names)"
    import xml.etree.ElementTree as ET

    files: dict[str, list] = {"A01": [], "A02": []}
    for _, el in ET.iterparse(folder / "MeasurementData.mlf", events=("end",)):
        if a._local(el.tag) == "MeasurementRecord":
            at = {a._local(k): v for k, v in el.attrib.items()}
            wanted = at.get("Row") == "1" and at.get("Column") in ("1", "2") and at.get("Ch") == str(channel)
            if wanted and at.get("ZIndex", "1") == "1":
                f = folder / (el.text or "").strip()
                if f.is_file():
                    files[f"A{int(at['Column']):02d}"].append(f)
            el.clear()
    means = {w: a._tif_mean(sorted(fs)) for w, fs in files.items()}
    return _margin(means), _h().rd("tifffile") + " (planes MeasurementData.mlf lists)"


def ixm_brightest_site(cid: str, well: str, cross: bool = False):
    """Site of `well` with the highest mean DAPI: the wavelength-1 files by name
    (IXMtest_<well>_s<site>_w1<GUID>.tif), or (`cross`) every file whose MetaMorph tags say this well,
    site and illumination 'DAPI'."""
    import tifffile

    import analysis as a

    folder = _h().hpath(cid)
    means = {}
    for q in sorted(folder.glob("*.tif")):
        if not cross:
            m = re.fullmatch(rf"IXMtest_{well}_s(\d+)_w1[0-9A-F]{{8}}-[0-9A-F-]+\.tif", q.name)
            if m:
                means[int(m.group(1))] = a._tif_mean([q])
            continue
        with tifffile.TiffFile(q) as t:
            md = t.stk_metadata
        label = md["StageLabel"][0] if isinstance(md["StageLabel"], list) else md["StageLabel"]
        m = re.fullmatch(rf"{well} : Site (\d+)", label)
        if m and md["Name"] == "DAPI":
            means[int(m.group(1))] = a._tif_mean([q])
    assert len(means) == 9, means
    how = " (file names)" if not cross else " (MetaMorph stage label and illumination name)"
    return _margin(means), _h().rd("tifffile") + how


def zarr_brightest_t(cid: str, channel: int):
    """Time point (counting from 1) whose full-resolution volume has the highest mean in one channel."""
    import zarr

    g = zarr.open_group(str(_h().hpath(cid)), mode="r")
    arr = g["0"]
    assert arr.ndim == 5, arr.shape
    means = {t + 1: float(arr[t, channel].mean(dtype="float64")) for t in range(arr.shape[0])}
    return _margin(means, 1.005), _h().rd("zarr")


def tiff_brightest(cid: str, axis: str, channel: int | None = None):
    """Index (counting from 1) along `axis` with the highest mean, tifffile; for TCYX, within one channel."""
    import tifffile

    with tifffile.TiffFile(_h().hpath(cid)) as t:
        s = t.series[0]
        arr, axes = s.asarray(), s.axes
    if axes == "CYX" and axis == "C":
        means = {c + 1: float(arr[c].mean(dtype="float64")) for c in range(arr.shape[0])}
    elif axes == "TCYX" and axis == "T":
        means = {i + 1: float(arr[i, channel].mean(dtype="float64")) for i in range(arr.shape[0])}
    else:
        raise AssertionError(axes)
    return _margin(means), _h().rd("tifffile")


def tiff_channel_mean(cid: str, channel: int, pages: bool = False):
    """Mean of one channel of an ImageJ TCYX hyperstack: the tifffile series, or (`pages`) the pages
    themselves in ImageJ's C-fastest order (page t * channels + c)."""
    import numpy as np
    import tifffile

    with tifffile.TiffFile(_h().hpath(cid)) as t:
        nc = int(t.imagej_metadata["channels"])
        if pages:
            planes = [t.pages[i].asarray() for i in range(channel, len(t.pages), nc)]
            return float(np.mean([p.mean(dtype="float64") for p in planes])), _h().rd("tifffile") + " (pages)"
        s = t.series[0]
        assert s.axes == "TCYX", s.axes
        return float(s.asarray()[:, channel].mean(dtype="float64")), _h().rd("tifffile")


def facts(Fact, H) -> list:
    """The analysis-tier facts of the imaging area (computed in the oracle venv)."""
    return [
        Fact(
            C["d_imaging_apotome"],
            "brightest_channel",
            "channel with the highest mean intensity",
            lambda: czi_brightest_channel(C["d_imaging_apotome"]),
            lambda: czi_brightest_channel(C["d_imaging_apotome"], cross=True),
        ),
        Fact(
            C["d_imaging_lsm710"],
            "brightest_z_ch2",
            "optical section (counting from 1) with the highest mean in the second channel",
            lambda: czi_brightest_z(C["d_imaging_lsm710"], 1),
            lambda: czi_brightest_z(C["d_imaging_lsm710"], 1, cross=True),
        ),
        Fact(
            C["d_imaging_isim"],
            "brightest_channel",
            "channel with the highest mean intensity",
            lambda: H.nd2_brightest_channel(C["d_imaging_isim"]),
        ),
        Fact(
            C["d_imaging_pmpcb"],
            "brightest_image_ch1",
            "image (by name) with the highest mean in the first channel",
            lambda: H.lif_brightest_series(C["d_imaging_pmpcb"], 0),
            lambda: H.lif_brightest_series(C["d_imaging_pmpcb"], 0, cross=True),
        ),
        Fact(
            C["d_imaging_ims_zebrafish"],
            "mean_ch2",
            "mean of the second channel over the full-resolution volume",
            lambda: ims_channel_mean(C["d_imaging_ims_zebrafish"], 1),
        ),
        Fact(
            C["d_imaging_lsm"],
            "brightest_channel",
            "spectral channel (counting from 1) with the highest mean",
            lambda: tiff_brightest(C["d_imaging_lsm"], "C"),
        ),
        Fact(
            C["d_imaging_imagej"],
            "mean_ch2",
            "mean of the second channel over every time point",
            lambda: tiff_channel_mean(C["d_imaging_imagej"], 1),
            lambda: tiff_channel_mean(C["d_imaging_imagej"], 1, pages=True),
            rel=1e-9,
        ),
        Fact(
            C["d_imaging_zarr"],
            "brightest_t_h2b",
            "time point (counting from 1) with the highest mean H2B (channel 2) over the full-resolution volume",
            lambda: zarr_brightest_t(C["d_imaging_zarr"], 1),
        ),
        Fact(
            C["d_imaging_dm4_eds"],
            "total_counts",
            "sum of every value of the EDS spectrum image",
            lambda: dm_total_counts(C["d_imaging_dm4_eds"]),
            lambda: dm_total_counts(C["d_imaging_dm4_eds"], export=True),
            rel=0,
        ),
        Fact(
            C["d_imaging_emd_ceta"],
            "image_mean",
            "mean of every pixel of the image",
            lambda: emd_mean(C["d_imaging_emd_ceta"]),
        ),
        Fact(
            C["d_imaging_mrc_template"],
            "map_std",
            "standard deviation of every voxel value of the map",
            lambda: mrc_std(C["d_imaging_mrc_template"]),
            lambda: mrc_std(C["d_imaging_mrc_template"], numpy=True),
            rel=1e-6,
        ),
        Fact(
            C["d_imaging_mrc_tomo"],
            "mean_voxel",
            "mean of every voxel value of the tomogram",
            lambda: H.mrc_mean(C["d_imaging_mrc_tomo"]),
            lambda: H.mrc_mean(C["d_imaging_mrc_tomo"], numpy=True),
            rel=1e-9,
        ),
        Fact(
            C["d_imaging_harmony7"],
            "brighter_well_hoechst",
            "of wells A01 and A02 (fields 1-2 in the copy), the one with the higher pooled mean of channel 1 "
            "(HOECHST 33342)",
            lambda: harmony_root_brighter_well(C["d_imaging_harmony7"], 1),
            lambda: harmony_root_brighter_well(C["d_imaging_harmony7"], 1, cross=True),
        ),
        Fact(
            C["d_imaging_cv8000"],
            "brighter_well_ch1",
            "of wells A01 and A02 (fields 1-2 in the copy), the one with the higher pooled mean of channel 1 "
            "(BP445/45)",
            lambda: cv_brighter_well(C["d_imaging_cv8000"], 1),
            lambda: cv_brighter_well(C["d_imaging_cv8000"], 1, cross=True),
        ),
        Fact(
            C["d_imaging_ixm"],
            "brightest_site_dapi_a01",
            "site (counting from 1) of well A01 with the highest mean DAPI intensity",
            lambda: ixm_brightest_site(C["d_imaging_ixm"], "A01"),
            lambda: ixm_brightest_site(C["d_imaging_ixm"], "A01", cross=True),
        ),
    ]


# ---------------------------------------------------------------- questions


def specs(spec, fact, g, H) -> list[Any]:
    o_img, o_um = H.o_img, H.o_um
    raw = H.HINT_RAW
    unit_hint = "a number with its unit"
    size_hint = "width × height in pixels"
    plate = "sample_plate"

    def wh(o: dict, image: int = 0) -> dict:
        return g.items([str(o_img(o, "size_x", image)), str(o_img(o, "size_y", image))])

    def other_well(v: str) -> str:
        return "A01" if v == "A02" else "A02"

    vsi_extra = [
        (C["d_imaging_vsi_ets1"], "_sample_/stack1/frame_t.ets"),
        (C["d_imaging_vsi_ets2"], "_sample_/stack10001/frame_t.ets"),
    ]
    mirax_extra = [(f"{C['d_imaging_mirax']}-{n.lower().replace('.', '-')}", f"sample/{n}") for n in MIRAX_DATA]

    return [
        # ------------------------------------------------ light microscopy: CZI
        spec(
            "ho-d-mic-czi-oxa1-pixel",
            "d_imaging_oxa1",
            "pixel-size",
            "This is a brightfield overview scan of a tissue slide. What is its pixel size at full resolution?",
            lambda o, m: g.number(o_um(o), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (czifile)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-mic-czi-apotome-channels",
            "d_imaging_apotome",
            "channels",
            "This is a maximum-intensity projection from a widefield microscope with structured illumination. "
            "How many fluorescence channels does it have?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (czifile)",
        ),
        spec(
            "ho-d-ana-mic-czi-apotome-brightest",
            "d_imaging_apotome",
            "analysis",
            "Which channel of this image has the highest mean intensity? Give the channel name.",
            fact("brightest_channel", lambda v: g.string(v)),
            "facts-heldout: brightest_channel (czifile; pylibCZIrw agrees)",
            answer_hint="the channel name",
        ),
        spec(
            "ho-d-mic-czi-lsm710-zstep",
            "d_imaging_lsm710",
            "pixel-size",
            "This confocal stack shows a spider embryo. What is the spacing between its optical sections?",
            lambda o, m: g.number(o_um(o, "z"), "µm", rel=0.02),
            "oracle: /images/0/physical_size_um/z (czifile)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-mic-czi-lsm710-brightest-z",
            "d_imaging_lsm710",
            "analysis",
            "In the second channel of this confocal stack, which optical section (counting from 1) has the highest "
            "mean intensity?",
            fact("brightest_z_ch2", g.integer),
            "facts-heldout: brightest_z_ch2 (czifile; pylibCZIrw agrees)",
            answer_hint="the section number",
        ),
        spec(
            "ho-d-mic-czi-lysoip-pixel",
            "d_imaging_lysoip",
            "pixel-size",
            "What is the pixel size of this confocal immunofluorescence image, in nanometres?",
            lambda o, m: g.number(o_um(o) * 1000.0, "nm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (czifile), in nm",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ ND2
        spec(
            "ho-d-mic-nd2-fishscale-pixel",
            "d_imaging_fishscale",
            "pixel-size",
            "What is the pixel size of this brightfield image?",
            lambda o, m: g.number(o_um(o), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (nd2)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-mic-nd2-txtl-pixel",
            "d_imaging_txtl",
            "pixel-size",
            "What is the pixel size of this confocal image, in nanometres?",
            lambda o, m: g.number(o_um(o) * 1000.0, "nm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (nd2), in nm",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-mic-nd2-isim-channels",
            "d_imaging_isim",
            "channels",
            "How many channels were recorded in this super-resolution (iSIM) image of a reference slide?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (nd2)",
        ),
        spec(
            "ho-d-ana-mic-nd2-isim-brightest",
            "d_imaging_isim",
            "analysis",
            "Which channel of this image has the highest mean intensity? Give the channel name.",
            fact("brightest_channel", lambda v: g.string(v)),
            "facts-heldout: brightest_channel (nd2)",
            answer_hint="the channel name",
        ),
        # ------------------------------------------------ LIF
        spec(
            "ho-d-mic-lif-opsin-images",
            "d_imaging_opsin",
            "counts",
            "How many images (series) does this Leica file contain?",
            lambda o, m: g.integer(len(o["images"])),
            "oracle: len(/images) (liffile)",
        ),
        spec(
            "ho-d-mic-lif-pmpcb-images",
            "d_imaging_pmpcb",
            "counts",
            "How many images (series) are stored in this Leica file?",
            lambda o, m: g.integer(len(o["images"])),
            "oracle: len(/images) (liffile)",
        ),
        spec(
            "ho-d-ana-mic-lif-pmpcb-brightest",
            "d_imaging_pmpcb",
            "analysis",
            "The images in this Leica file are named after the cells they show. Which image has the highest mean "
            "intensity in its first channel? Give the image name.",
            fact("brightest_image_ch1", lambda v: g.string(v)),
            "facts-heldout: brightest_image_ch1 (liffile; readlif agrees)",
            answer_hint="the image name",
        ),
        spec(
            "ho-d-mic-lif-raman-pixel",
            "d_imaging_raman",
            "pixel-size",
            "These are coherent Raman (SRS/CARS) images of live yeast. What is the pixel size of the first image, in "
            "nanometres?",
            lambda o, m: g.number(o_um(o) * 1000.0, "nm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (liffile), in nm",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ Olympus OIR / OIB
        spec(
            "ho-d-mic-oir-hek293-slices",
            "d_imaging_oir",
            "dimensions",
            "How many z-slices does the first image of this Olympus/Evident file have?",
            lambda o, m: g.integer(o_img(o, "size_z")),
            "oracle: /images/0/size_z (oirfile)",
        ),
        spec(
            "ho-d-mic-oib-osn-slices",
            "d_imaging_oib",
            "dimensions",
            "How many optical sections (z-slices) does this FluoView stack contain?",
            lambda o, m: g.integer(o_img(o, "size_z")),
            "oracle: /images/0/size_z (Bio-Formats, black box; oiffile agrees)",
        ),
        # ------------------------------------------------ Imaris
        spec(
            "ho-d-mic-ims-owenia-zstep",
            "d_imaging_ims_owenia",
            "pixel-size",
            "This Imaris file shows an annelid larva. What is the spacing between its z-slices?",
            lambda o, m: g.number(o_um(o, "z"), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/z (h5py)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-mic-ims-zebrafish-slices",
            "d_imaging_ims_zebrafish",
            "dimensions",
            "How many z-slices does this Imaris volume have at full resolution?",
            lambda o, m: g.integer(o_img(o, "size_z")),
            "oracle: /images/0/size_z (h5py)",
        ),
        spec(
            "ho-d-ana-mic-ims-zebrafish-mean",
            "d_imaging_ims_zebrafish",
            "analysis",
            "What is the mean intensity of the second channel over the whole full-resolution volume?",
            fact("mean_ch2", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: mean_ch2 (h5py)",
            answer_hint=raw,
        ),
        # ------------------------------------------------ ZVI, TIFF variants
        spec(
            "ho-d-mic-zvi-channels",
            "d_imaging_zvi",
            "channels",
            "This AxioVision image shows a mouse melanoma section. Which channels does it have? "
            "Give the channel names.",
            lambda o, m: g.items(o_img(o, "channel_names")),
            "oracle: /images/0/channel_names (Bio-Formats, black box)",
            answer_hint="the channel names, comma-separated",
        ),
        spec(
            "ho-d-mic-lsm-spectral-channels",
            "d_imaging_lsm",
            "channels",
            "This Zeiss LSM file is a spectral (lambda) scan. How many spectral channels does it have?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (tifffile)",
        ),
        spec(
            "ho-d-ana-mic-lsm-spectral-brightest",
            "d_imaging_lsm",
            "analysis",
            "Which spectral channel of this lambda scan (counting from 1) has the highest mean intensity?",
            fact("brightest_channel", g.integer),
            "facts-heldout: brightest_channel (tifffile)",
            answer_hint="the channel number",
        ),
        spec(
            "ho-d-mic-tiff-imagej-timepoints",
            "d_imaging_imagej",
            "dimensions",
            "This TIFF is an ImageJ hyperstack from a time-lapse screen. How many time points does it hold?",
            lambda o, m: g.integer(o_img(o, "size_t")),
            "oracle: /images/0/size_t (tifffile)",
        ),
        spec(
            "ho-d-ana-mic-tiff-imagej-mean",
            "d_imaging_imagej",
            "analysis",
            "What is the mean intensity of the second channel over all time points (raw values as stored)?",
            fact("mean_ch2", lambda v: g.number(v, None, rel=2e-4)),
            "facts-heldout: mean_ch2 (tifffile hyperstack series; the ImageJ page order read page by page agrees)",
            answer_hint=raw,
        ),
        # ------------------------------------------------ slides: VSI, MIRAX; OME-Zarr
        spec(
            "ho-d-mic-vsi-size",
            "d_imaging_vsi",
            "dimensions",
            "This is an Olympus whole-slide scan of an intestinal biopsy, with a low-magnification overview and a "
            "40x scan. What are the width and height of the 40x scan at full resolution, in pixels?",
            lambda o, m: wh(o, next(i for i, im in enumerate(o["images"]) if im.get("name") == "40x")),
            "oracle: /images[name=40x]/size_x and size_y (Bio-Formats, black box)",
            stage_as="sample.vsi",
            extra=vsi_extra,
            answer_hint=size_hint,
        ),
        spec(
            "ho-d-mic-mirax-size",
            "d_imaging_mirax",
            "dimensions",
            "This is a fluorescence whole-slide scan (MIRAX). What are the width and height of the full-resolution "
            "slide image, in pixels?",
            lambda o, m: wh(o),
            "oracle: /images/0/size_x and size_y (OpenSlide, black box)",
            stage_as="sample.mrxs",
            extra=mirax_extra,
            answer_hint=size_hint,
        ),
        spec(
            "ho-d-mic-mirax-pixel",
            "d_imaging_mirax",
            "pixel-size",
            "What is the pixel size of this slide scan at full resolution, in micrometres?",
            lambda o, m: g.number(o_um(o), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (OpenSlide mpp-x, black box)",
            stage_as="sample.mrxs",
            extra=mirax_extra,
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-mic-zarr-timepoints",
            "d_imaging_zarr",
            "dimensions",
            "This OME-Zarr image is a light-sheet time lapse of an organoid. How many time points does it have?",
            lambda o, m: g.integer(o_img(o, "size_t")),
            "oracle: /images/0/size_t (zarr-python)",
        ),
        spec(
            "ho-d-mic-zarr-zstep",
            "d_imaging_zarr",
            "pixel-size",
            "What is the z-step of the full-resolution volume?",
            lambda o, m: g.number(o_um(o, "z"), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/z (zarr-python; NGFF coordinate transformations)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-mic-zarr-brightest-t",
            "d_imaging_zarr",
            "analysis",
            "In the H2B channel, which time point (counting from 1) has the highest mean intensity over the "
            "full-resolution volume?",
            fact("brightest_t_h2b", g.integer),
            "facts-heldout: brightest_t_h2b (zarr-python)",
            answer_hint="the time point",
        ),
        # ------------------------------------------------ gel imaging
        spec(
            "ho-d-scn-sio2-size",
            "d_imaging_scn_sio2",
            "dimensions",
            "What are the width and height of this gel/blot image, in pixels?",
            lambda o, m: wh(o),
            "oracle: /images/0/size_x and size_y (Bio-Formats, black box)",
            answer_hint=size_hint,
        ),
        spec(
            "ho-d-scn-dna-width",
            "d_imaging_scn_dna",
            "dimensions",
            "How wide is this agarose gel image, in pixels?",
            lambda o, m: g.integer(o_img(o, "size_x")),
            "oracle: /images/0/size_x (Bio-Formats, black box)",
        ),
        # ------------------------------------------------ electron microscopy
        spec(
            "ho-d-em-dm4-tfs-pixel",
            "d_imaging_dm4_tfs",
            "pixel-size",
            "This is a small HAADF STEM image. What is its pixel size, in picometres?",
            lambda o, m: g.number(o_um(o) * 1e6, "pm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (dm3_lib), in pm",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-em-dm4-tfs-size",
            "d_imaging_dm4_tfs",
            "dimensions",
            "What are the width and height of this image, in pixels?",
            lambda o, m: wh(o),
            "oracle: /images/0/size_x and size_y (dm3_lib)",
            answer_hint=size_hint,
        ),
        spec(
            "ho-d-em-dm3-profile-points",
            "d_imaging_dm3_profile",
            "counts",
            "This DigitalMicrograph file holds a line profile, not an image. How many values does the profile have?",
            lambda o, m: g.integer(o_img(o, "size_x")),
            "oracle: /images/0/size_x (dm3_lib tag dictionary)",
        ),
        spec(
            "ho-d-em-dm4-eds-channels",
            "d_imaging_dm4_eds",
            "dimensions",
            "This DigitalMicrograph file holds an EDS spectrum image. How many energy channels does each "
            "spectrum have?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (dm3_lib)",
        ),
        spec(
            "ho-d-ana-em-dm4-eds-counts",
            "d_imaging_dm4_eds",
            "analysis",
            "How many X-ray counts does this EDS spectrum image hold in total (summed over every pixel and energy "
            "channel)?",
            fact("total_counts", g.integer),
            "facts-heldout: total_counts (dm3_lib; the depositor's HyperSpy export agrees)",
            answer_hint="the total number of counts",
        ),
        spec(
            "ho-d-em-ser-pixel",
            "d_imaging_ser",
            "pixel-size",
            "What is the pixel size of this TEM image, in picometres?",
            lambda o, m: g.number(o_um(o) * 1e6, "pm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (ncempy, black box), in pm",
            stage_as="sample_1.ser",
            extra=[(C["d_imaging_emi"], "sample.emi")],
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-em-emd-ceta-detector",
            "d_imaging_emd_ceta",
            "instrument",
            "Which detector (camera) recorded this Velox image?",
            lambda o, m: g.string(o_img(o, "detector"), ["Ceta", "BM Ceta", "Ceta camera"]),
            "oracle: /images/0/detector (h5py, Velox metadata)",
            answer_hint="the detector name",
        ),
        spec(
            "ho-d-ana-em-emd-ceta-mean",
            "d_imaging_emd_ceta",
            "analysis",
            "What is the mean pixel value of this image (raw values as stored)?",
            fact("image_mean", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: image_mean (h5py)",
            answer_hint=raw,
        ),
        spec(
            "ho-d-em-mrc-template-voxel",
            "d_imaging_mrc_template",
            "pixel-size",
            "This MRC file is a template volume for cryo-ET template matching. What is its voxel size, in ångström?",
            lambda o, m: g.number(o_um(o) * 1e4, "Å", rel=0.01),
            "oracle: /images/0/physical_size_um/x (mrcfile), in Å",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-em-mrc-template-std",
            "d_imaging_mrc_template",
            "analysis",
            "What is the standard deviation of the voxel values of this map?",
            fact("map_std", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: map_std (mrcfile; NumPy on the MRC2014 layout agrees)",
            answer_hint=raw,
        ),
        spec(
            "ho-d-em-mrc-tomo-size",
            "d_imaging_mrc_tomo",
            "dimensions",
            "What are the dimensions of this tomogram in voxels (x, y and z)?",
            lambda o, m: g.items([str(o_img(o, k)) for k in ("size_x", "size_y", "size_z")], ordered=True),
            "oracle: /images/0/size_x, size_y, size_z (mrcfile)",
            answer_hint="x × y × z in voxels",
        ),
        spec(
            "ho-d-ana-em-mrc-tomo-mean",
            "d_imaging_mrc_tomo",
            "analysis",
            "What is the mean voxel value of this 8-bit tomogram (raw values as stored)?",
            fact("mean_voxel", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: mean_voxel (mrcfile; NumPy on the MRC2014 layout agrees)",
            answer_hint=raw,
        ),
        # ------------------------------------------------ high-content screening
        spec(
            "ho-d-hcs-harmony7-wells",
            "d_imaging_harmony7",
            "counts",
            "This folder is a high-content screening plate exported from the imager (only some wells were copied). "
            "How many wells does the plate format have?",
            lambda o, m: g.integer(o["hcs"]["plate"]["rows"] * o["hcs"]["plate"]["columns"]),
            "oracle: /hcs/plate/rows × columns (Index.xml parsed with the Python standard library)",
            stage_as=plate,
            answer_hint="the number of wells",
        ),
        spec(
            "ho-d-ana-hcs-harmony7-brighter-well",
            "d_imaging_harmony7",
            "analysis",
            "This copy of the plate holds fields 1 and 2 of wells A01 and A02. Which of the two wells has the higher "
            "mean intensity in the HOECHST 33342 channel, pooled over its fields?",
            fact("brighter_well_hoechst", lambda v: g.string(v, [v.replace("0", "", 1)], reject=[other_well(v)])),
            "facts-heldout: brighter_well_hoechst (tifffile on the plane files; the planes Index.xml names agree)",
            stage_as=plate,
            answer_hint="the well, e.g. B03",
        ),
        spec(
            "ho-d-hcs-cv8000-fields",
            "d_imaging_cv8000",
            "counts",
            "This folder is a Yokogawa CellVoyager measurement (only some images were copied). How many fields of "
            "view per well does the measurement define?",
            lambda o, m: g.integer(next(len(w["images"]) for w in o["hcs"]["wells"] if w["well"] == "A01")),
            "oracle: /hcs/wells (A01) images (MeasurementData.mlf parsed with the Python standard library)",
            stage_as=plate,
            answer_hint="the number of fields",
        ),
        spec(
            "ho-d-ana-hcs-cv8000-brighter-well",
            "d_imaging_cv8000",
            "analysis",
            "This copy of the plate holds fields 1 and 2 of wells A01 and A02. Which of the two wells has the higher "
            "mean intensity in channel 1 (the BP445/45 nuclear channel), pooled over its fields?",
            fact("brighter_well_ch1", lambda v: g.string(v, [v.replace("0", "", 1)], reject=[other_well(v)])),
            "facts-heldout: brighter_well_ch1 (tifffile on the plane files; "
            "the planes MeasurementData.mlf lists agree)",
            stage_as=plate,
            answer_hint="the well, e.g. B03",
        ),
        spec(
            "ho-d-hcs-ixm-channels",
            "d_imaging_ixm",
            "channels",
            "This folder holds ImageXpress images of two wells of a Cell Painting plate. How many wavelengths "
            "(channels) were imaged per site?",
            lambda o, m: g.integer(o["hcs"]["size_c"]),
            "oracle: /hcs/size_c (file names and MetaMorph tags read with the Python standard library and tifffile)",
            stage_as=plate,
            answer_hint="the number of channels",
        ),
        spec(
            "ho-d-ana-hcs-ixm-site",
            "d_imaging_ixm",
            "analysis",
            "In well A01, which site (counting from 1) has the highest mean intensity in the DAPI channel?",
            fact("brightest_site_dapi_a01", g.integer),
            "facts-heldout: brightest_site_dapi_a01 (tifffile; the MetaMorph stage labels agree)",
            stage_as=plate,
            answer_hint="the site number",
        ),
    ]
