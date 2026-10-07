"""Held-out draw C (2026-09-26): questions about the third generalization draw.

The third draw adds 90 held-out inputs from 89 source
records no development or earlier held-out file uses. These questions were written from the
held-out oracles (`corpus/oracle/heldout/<id>.json`), the depositors' exports and records, and
facts computed here with third-party readers, before OpenReadout was run on any of these files.

`evals/heldout.py` imports this module and extends its `C`, `FACTS` and `SPECS`; the readers are
imported inside the functions that use them, so generate.py runs without them.
"""

from __future__ import annotations

import re
from typing import Any

C = {
    # light microscopy
    "c_lattice": "ho-zenodo14903188-czi-lattice",
    "c_spectral": "ho-zenodo3455774-czi-spectral",
    "c_organoid": "ho-zenodo17587777-czi-organoid",
    "c_rotifer": "ho-zenodo18328675-czi-rotifer",
    "c_centro": "ho-zenodo13773035-czi-centrosome",
    "c_bee": "ho-zenodo18026691-nd2-bee",
    "c_septin": "ho-zenodo4903631-nd2-septin",
    "c_shear": "ho-zenodo3468627-nd2-shear",
    "c_coacervate": "ho-zenodo20648254-nd2-coacervate",
    "c_counting": "ho-zenodo5569195-lif-counting",
    "c_epoxy": "ho-zenodo5702329-lif-epoxy",
    "c_hcar1": "ho-zenodo3896128-lif-hcar1",
    "c_aphid": "ho-zenodo15105505-lif-aphid",
    "c_oir": "ho-zenodo20703055-oir-trpv1",
    "c_oib_mcu": "ho-figshare19683225-oib-mcu",
    "c_oib_spleen": "ho-figshare12957947-oib-spleen",
    "c_ims_tracked": "ho-zenodo18473314-ims-tracked",
    "c_ims_retina": "ho-zenodo17975750-ims-retina",
    "c_ndpi": "ho-zenodo22686265-ndpi-cfos",
    "c_svs": "ho-zenodo15752981-svs-ihc",
    # electron microscopy
    "c_dm4": "ho-zenodo22115991-dm4-haadf",
    "c_dm4_si": "ho-zenodo19963879-dm4-stem-si",
    "c_dm3": "ho-zenodo4496929-dm3-tomo",
    "c_ser_tem": "ho-zenodo4321274-ser-tem",
    "c_ser_stem": "ho-zenodo14050919-ser-stem",
    "c_velox": "ho-zenodo19963421-emd-velox",
    "c_beacon": "ho-zenodo13891904-emd-beacon",
    "c_mrc_bi": "ho-zenodo19044980-mrc-bismuth",
    "c_mrc_vault": "ho-zenodo18414818-mrc-vault",
    # flow cytometry
    "c_fcs_mp": "ho-zenodo20559117-fcs-microparticles",
    "c_fcs_spectral": "ho-zenodo21643327-fcs-spectral",
    "c_fcs_cytof": "ho-zenodo4465011-fcs-exported",
    "c_fcs_angle": "ho-zenodo5235706-fcs-multiangle",
    "c_fcs_histone": "ho-zenodo377035-fcs-histone",
    "c_fcs_uv": "ho-zenodo22808976-fcs-uvcomp",
    # electrophysiology
    "c_ncs": "ho-figshare26337268-ncs-hpc",
    "c_ns5": "ho-figshare25036019-ns5-tfus",
    "c_rhs": "ho-figshare27687309-rhs-nerve",
    "c_nwb": "ho-zenodo8379272-nwb-acc",
    "c_abf_pore": "ho-zenodo8123395-abf-nanopore",
    "c_abf_stns": "ho-zenodo5139650-abf-stns",
    "c_abf_ca1": "ho-zenodo8747-abf-ca1",
    # NMR
    "c_jdf_1h": "ho-zenodo5115431-jdf-isoxazole",
    "c_jdf_ft": "ho-zenodo22940442-jdf-ft",
    "c_cari_1d": "ho-nmrxiv-p7-caripyrin-1d",
    "c_cari_2d": "ho-nmrxiv-p7-caripyrin-2d",
    "c_19f": "ho-nmrxiv-p170-spirocyclobutane",
    # spectroscopy
    "c_dx_ftir": "ho-zenodo18009054-dx-ftir",
    "c_opus_si": "ho-zenodo17294324-opus-silica",
    "c_opus_caco2": "ho-zenodo17950195-opus-caco2",
    "c_spa_dust": "ho-zenodo21396092-spa-dust",
    "c_spa_raman": "ho-zenodo10885924-spa-raman",
    "c_spg": "ho-figshare27175326-spg-raman",
    "c_wdf": "ho-zenodo18849615-wdf-otolith",
    "c_sp_aeb": "ho-zenodo21456008-sp-aeb",
    "c_sp_mp": "ho-zenodo3898505-sp-microplastic",
    "c_jws_cd": "ho-figshare29371550-jws-cd-protein",
    # mass spectrometry
    "c_raw_germ": "ho-zenodo6948449-raw-germicidin",
    "c_raw_aqc": "ho-zenodo10043877-raw-aqc",
    "c_dmrm": "ho-zenodo18502866-mzml-dmrm",
    "c_fticr": "ho-zenodo18494294-mzml-fticr",
    "c_vion": "ho-zenodo20591730-mzml-vion",
    "c_cems": "ho-zenodo18481720-mzml-cems",
    "c_mzxml": "ho-zenodo20345272-mzxml-microbiome",
    # chromatography
    "c_gcms": "ho-figshare28560599-gcms-abronia",
    "c_cdf_sam": "ho-figshare27987005-cdf-sam",
    "c_cdf_serum": "ho-figshare21348210-cdf-serum",
    "c_lcd_kat": "ho-figshare5831265-lcd-kat",
    # plates, cell metabolism, qPCR, bench
    "c_gen5": "ho-gh-lennonlab-gen5-ecoplate",
    "c_bmg": "ho-gh-maxlindberg-clariostar-scan",
    "c_envision": "ho-gh-cellhts2-envision",
    "c_xf_perm": "ho-figshare22557340-seahorse-perm",
    "c_xf_glyco": "ho-figshare13490271-seahorse-glyco",
    "c_lamp": "ho-figshare34002258-eds-lamp",
    "c_tsa": "ho-figshare20004872-eds-tsa",
    "c_vero": "ho-figshare23574411-eds-vero",
    "c_gpr_cu": "ho-figshare21681689-gpr-copper",
    "c_gpr_prairie": "ho-figshare5627425-gpr-prairie",
    "c_scn_tgev": "ho-figshare32604195-scn-tgev",
    "c_scn_coq8": "ho-figshare30902465-scn-coq8",
}


# ---------------------------------------------------------------- facts (oracle venv only)


def _h():
    import heldout

    return heldout


def nd2_brightest_frame(cid: str):
    import analysis as a

    means, reader = a.nd2_frame_means(cid, "T")
    s = sorted(means)
    assert s[-1] > 1.001 * s[-2], means
    return means.index(max(means)) + 1, reader


def mrc_std_fact(cid: str, numpy: bool = False):
    import analysis as a

    if numpy:
        return float(a.mrc_numpy(cid).std(dtype="float64")), a.NUMPY_MRC
    return float(a.mrc_data(cid).std(dtype="float64")), _h().rd("mrcfile")


def fcs_median(cid: str, param: str, fcsparser: bool = False):
    return _h().fcs_raw_median(cid, param, fcsparser)


def abf_argmax(cid: str, channel: int, neo: bool = False):
    import analysis as a

    return a.abf_argmax_sweep(cid, channel, neo)


def abf_channel_min(cid: str, channel: int, neo: bool = False):
    import numpy as np

    import analysis as a

    sw, _ = (a.abf_sweeps_neo if neo else a.abf_sweeps)(cid, channel)
    return float(np.concatenate(sw).min()), _h().rd("neo") + " AxonRawIO" if neo else _h().rd("pyabf")


def ncs_std(cid: str, numpy: bool = False):
    import analysis as a

    return a.ncs_std_numpy(cid) if numpy else a.ncs_std_neo(cid)


def emd_stack_count(cid: str):
    """Images in the Berkeley EMD group (`emd_group_type` 1) of the file: the first axis of its `data`."""
    import h5py

    with h5py.File(_h().hpath(cid), "r") as f:
        groups = []
        f.visititems(
            lambda n, o: groups.append(o) if isinstance(o, h5py.Group) and o.attrs.get("emd_group_type") == 1 else None
        )
        (g,) = groups
        return int(g["data"].shape[0]), _h().rd("h5py")


def wdf_mean_window(cid: str, lo: float, hi: float, cross: bool = False):
    """Raman shift of the tallest point within [lo, hi] of the mean of every spectrum of a WiRE map."""
    import numpy as np

    H = _h()
    if cross:
        import spectrochempy as scp

        with H._quiet():
            d = scp.read_wdf(str(H.hpath(cid)))
        x = np.asarray(d.x.data, dtype="float64")
        y = np.asarray(d.data, dtype="float64").reshape(-1, x.size).mean(axis=0)
        return H.band(x, y, lo, hi, "max"), H.rd("spectrochempy") + " read_wdf"
    x, s = H.wdf_spectra(cid)
    return H.band(x, s.mean(axis=0), lo, hi, "max"), H.rd("renishawWiRE")


def ms_tic_apex(cid: str, role: str = "heldout", from_cvparam: bool = False):
    return _h().ms_tic_apex(cid, role, from_cvparam)


def mzxml_tic_attr_apex(cid: str):
    """Retention time (min) of the scan with the largest `totIonCurrent` attribute (pyteomics mzxml)."""
    from pyteomics import mzxml

    with mzxml.MzXML(str(_h().hpath(cid))) as r:
        best = max(r, key=lambda s: float(s["totIonCurrent"]))
    return round(float(best["retentionTime"]), 4), _h().rd("pyteomics") + " (`totIonCurrent` attributes)"


def andi_tic_apex_min(cid: str, summed: bool = False):
    """Scan time (min) of the largest total ion current of an ANDI-MS file (scipy netcdf):
    `total_intensity`, or (`summed`) the sum of each scan's `intensity_values`."""
    import numpy as np
    from scipy.io import netcdf_file

    f = netcdf_file(str(_h().hpath(cid)), "r", mmap=False)
    t = np.asarray(f.variables["scan_acquisition_time"].data, dtype="float64")
    if summed:
        idx = np.asarray(f.variables["scan_index"].data, dtype="int64")
        cnt = np.asarray(f.variables["point_count"].data, dtype="int64")
        it = np.asarray(f.variables["intensity_values"].data, dtype="float64")
        tic = np.array([it[i : i + n].sum() for i, n in zip(idx, cnt, strict=True)])
        how = "sum of intensity_values per scan"
    else:
        tic = np.asarray(f.variables["total_intensity"].data, dtype="float64")
        how = "total_intensity"
    return round(float(t[int(tic.argmax())]) / 60.0, 3), _h().rd("scipy") + f" netcdf_file ({how})"


def gcms_export_tic_apex(cid: str):
    """Time (min) of the largest value of the depositor's TIC CSV export (first two numeric columns)."""
    import numpy as np

    x, y = _h().text_xy(_h().hpath(cid, "oracle-export"))
    return round(float(np.asarray(x)[int(np.asarray(y).argmax())]), 3), "the depositor's TIC export " + _h().hpath(
        cid, "oracle-export"
    ).name


def gcms_rainbow_tic_apex(cid: str):
    v, r = _h().gcms_tic_apex_rainbow(cid)
    return round(v, 3), r


def gen5_grid(cid: str) -> dict[str, float]:
    """Well -> value of a Gen5 text export's 8 x 12 result grid (pandas, tab-separated)."""
    import io

    import pandas as pd

    text = _h().hpath(cid).read_text(errors="replace")
    lines = text.splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith("\t\t1\t2\t3"))
    df = pd.read_csv(io.StringIO("\n".join(lines[start : start + 9])), sep="\t", header=0, index_col=1)
    out = {}
    for row in "ABCDEFGH":
        for col in range(1, 13):
            out[f"{row}{col}"] = float(df.loc[row, str(col)])
    return out


def gen5_lowest_well(cid: str):
    vals = gen5_grid(cid)
    s = sorted(vals, key=vals.get)
    assert vals[s[0]] < vals[s[1]], (s[0], s[1])
    return s[0], _h().rd("pandas")


def bmg_spectrum_peak_nm(cid: str, well: str):
    """Wavelength (nm) with the highest raw signal in one well of a BMG spectrum CSV (standard csv)."""
    import csv

    rows = list(csv.reader(_h().hpath(cid).read_text(errors="replace").splitlines(), delimiter=";"))
    wl = next(r for r in rows if len(r) > 2 and r[1].startswith("Wavelength"))
    wls = [float(c) for c in wl[2:] if c.strip()]
    (r,) = [r for r in rows if r and r[0] == well]
    vals = [float(c) for c in r[2 : 2 + len(wls)] if c.strip()]
    i = max(range(len(vals)), key=vals.__getitem__)
    assert sorted(vals)[-1] > sorted(vals)[-2], vals
    return wls[i], "Python csv on the export text"


def envision_top_well(cid: str):
    """Well with the highest raw (`Results for …`, not crosstalk-corrected) luminescence."""
    lines = _h().hpath(cid).read_text(errors="replace").splitlines()
    k = next(i for i, line in enumerate(lines) if line.startswith("Results for "))
    cols = [c for c in lines[k + 1].split(",")[1:] if c.strip()]
    vals = {}
    for line in lines[k + 2 :]:
        parts = line.split(",")
        if not parts or not re.fullmatch(r"[A-P]", parts[0].strip()):
            break
        for c, v in zip(cols, parts[1:], strict=False):
            if v.strip():
                vals[f"{parts[0].strip()}{int(c)}"] = float(v)
    s = sorted(vals, key=vals.get)
    assert vals[s[-1]] > vals[s[-2]]
    return s[-1], "Python on the export text"


def gpr_frame(cid: str):
    import io

    import pandas as pd

    text = _h().hpath(cid).read_text(errors="replace")
    lines = text.splitlines()
    n = int(lines[1].split("\t")[0])  # ATF: header record count on line 2
    return pd.read_csv(io.StringIO("\n".join(lines[2 + n :])), sep="\t")


def gpr_median_of(cid: str, column: str):
    df = gpr_frame(cid)
    return float(df[column].median()), _h().rd("pandas")


def gpr_count_name(cid: str, name: str):
    df = gpr_frame(cid)
    return int((df["Name"].astype(str) == name).sum()), _h().rd("pandas")


def lamp_fastest_target(o: dict) -> str:
    """Target with the lowest mean vendor Cq (called wells only)."""
    by: dict[str, list[float]] = {}
    for r in o["records"]:
        if r.get("cq") is not None and not r.get("cq_undetermined"):
            by.setdefault(r["target"], []).append(r["cq"])
    means = {t: sum(v) / len(v) for t, v in by.items()}
    s = sorted(means, key=means.get)
    assert means[s[1]] - means[s[0]] > 1.0, means
    return s[0]


def facts(Fact, H) -> list:
    """The analysis-tier facts of draw C (computed in the oracle venv)."""
    return [
        Fact(
            C["c_septin"],
            "brightest_frame",
            "time point (counting from 1) with the highest mean intensity",
            lambda: nd2_brightest_frame(C["c_septin"]),
        ),
        Fact(
            C["c_mrc_vault"],
            "map_std",
            "standard deviation of every voxel value of the map",
            lambda: mrc_std_fact(C["c_mrc_vault"]),
            lambda: mrc_std_fact(C["c_mrc_vault"], numpy=True),
            rel=1e-6,
        ),
        Fact(
            C["c_beacon"],
            "stack_images",
            "number of images in the file's aberration stack (Berkeley EMD data group)",
            lambda: emd_stack_count(C["c_beacon"]),
        ),
        Fact(
            C["c_fcs_histone"],
            "median_fitc_a",
            "median of the stored FITC-A values over all events",
            lambda: fcs_median(C["c_fcs_histone"], "FITC-A"),
            lambda: fcs_median(C["c_fcs_histone"], "FITC-A", True),
        ),
        Fact(
            C["c_fcs_uv"],
            "median_u450",
            "median of the stored U 450/50-A values over all events",
            lambda: fcs_median(C["c_fcs_uv"], "U 450/50-A"),
            lambda: fcs_median(C["c_fcs_uv"], "U 450/50-A", True),
        ),
        Fact(
            C["c_fcs_angle"],
            "median_fsc_a",
            "median of the stored FSC-A values over all events",
            lambda: fcs_median(C["c_fcs_angle"], "FSC-A"),
            lambda: fcs_median(C["c_fcs_angle"], "FSC-A", True),
        ),
        Fact(
            C["c_abf_ca1"],
            "vm_peak_sweep",
            "sweep (counting from 1) whose Vm channel reaches the highest value",
            lambda: abf_argmax(C["c_abf_ca1"], 1),
            lambda: abf_argmax(C["c_abf_ca1"], 1, neo=True),
        ),
        Fact(
            C["c_abf_pore"],
            "peak_current_sweep",
            "sweep (counting from 1) with the largest current",
            lambda: abf_argmax(C["c_abf_pore"], 0),
            lambda: abf_argmax(C["c_abf_pore"], 0, neo=True),
        ),
        Fact(
            C["c_abf_stns"],
            "ch0_min",
            "minimum of the first channel (I_MTest 2, mV) over the recording",
            lambda: abf_channel_min(C["c_abf_stns"], 0),
            lambda: abf_channel_min(C["c_abf_stns"], 0, neo=True),
            rel=1e-4,
        ),
        Fact(
            C["c_ncs"],
            "signal_std_uv",
            "standard deviation of every sample of the channel, in µV",
            lambda: ncs_std(C["c_ncs"]),
            lambda: ncs_std(C["c_ncs"], numpy=True),
            rel=1e-6,
        ),
        Fact(
            C["c_cari_1d"],
            "tallest_ppm_0_6",
            "chemical shift of the highest point of TopSpin's pdata/1/1r between 0 and 6 ppm",
            lambda: __import__("analysis").pdata_ppm(C["c_cari_1d"], 0.0, 6.0),
            lambda: __import__("analysis").pdata_ppm(C["c_cari_1d"], 0.0, 6.0, raw=True),
        ),
        Fact(
            C["c_opus_caco2"],
            "amide_band",
            "wavenumber of the highest absorbance between 1500 and 1800 cm-1 (block AB)",
            lambda: H.opus_extreme(C["c_opus_caco2"], "a", 1500, 1800, "max"),
        ),
        Fact(
            C["c_spa_dust"],
            "strongest_absorption_600_4000",
            "wavenumber of the lowest transmittance between 600 and 4000 cm-1",
            lambda: H.omnic_extreme(C["c_spa_dust"], 600, 4000, "min"),
        ),
        Fact(
            C["c_wdf"],
            "carbonate_band",
            "Raman shift of the tallest point between 1000 and 1200 cm-1 of the mean of every spectrum of the map",
            lambda: wdf_mean_window(C["c_wdf"], 1000, 1200),
            lambda: wdf_mean_window(C["c_wdf"], 1000, 1200, cross=True),
            rel=1e-4,
        ),
        Fact(
            C["c_raw_germ"],
            "ms1_tic_apex_min",
            "retention time (min) of the MS1 spectrum with the largest summed intensity (depositor's mzML)",
            lambda: ms_tic_apex(C["c_raw_germ"], "oracle-export"),
            lambda: ms_tic_apex(C["c_raw_germ"], "oracle-export", True),
            role="oracle-export",
        ),
        Fact(
            C["c_cems"],
            "tic_apex_min",
            "migration time (min) of the spectrum with the largest summed intensity",
            lambda: ms_tic_apex(C["c_cems"]),
            lambda: ms_tic_apex(C["c_cems"], from_cvparam=True),
        ),
        Fact(
            C["c_mzxml"],
            "tic_apex_min",
            "retention time (min) of the spectrum with the largest summed intensity",
            lambda: ms_tic_apex(C["c_mzxml"]),
            lambda: mzxml_tic_attr_apex(C["c_mzxml"]),
        ),
        Fact(
            C["c_fticr"],
            "base_peak_mz",
            "m/z of the most intense point of the file's one spectrum",
            lambda: H.ms_base_peak_mz(C["c_fticr"]),
        ),
        Fact(
            C["c_gcms"],
            "tic_apex_min",
            "retention time (min) of the largest value of the total ion chromatogram",
            lambda: gcms_export_tic_apex(C["c_gcms"]),
            lambda: gcms_rainbow_tic_apex(C["c_gcms"]),
            rel=2e-3,
        ),
        Fact(
            C["c_cdf_serum"],
            "tic_apex_min",
            "scan time (min) of the largest total ion current",
            lambda: andi_tic_apex_min(C["c_cdf_serum"]),
            lambda: andi_tic_apex_min(C["c_cdf_serum"], summed=True),
        ),
        Fact(
            C["c_gen5"],
            "lowest_well_590",
            "well with the lowest absorbance at 590 nm",
            lambda: gen5_lowest_well(C["c_gen5"]),
        ),
        Fact(
            C["c_bmg"],
            "a01_peak_excitation_nm",
            "excitation wavelength (nm) with the highest raw signal in well A01",
            lambda: bmg_spectrum_peak_nm(C["c_bmg"], "A01"),
        ),
        Fact(
            C["c_envision"],
            "top_well_raw",
            "well with the highest raw luminescence (the measured block, not the crosstalk-corrected one)",
            lambda: envision_top_well(C["c_envision"]),
        ),
        Fact(
            C["c_gpr_prairie"],
            "median_f532_median",
            "median over all features of the F532 Median column",
            lambda: gpr_median_of(C["c_gpr_prairie"], "F532 Median"),
        ),
        Fact(
            C["c_gpr_cu"],
            "empty_nctrl_count",
            "number of features named Empty_NCTRL",
            lambda: gpr_count_name(C["c_gpr_cu"], "Empty_NCTRL"),
        ),
    ]


# ---------------------------------------------------------------- questions


def specs(spec, fact, g, H) -> list:
    o_img, o_um = H.o_img, H.o_um
    raw = H.HINT_RAW
    unit_hint = "a number with its unit"

    def tr(o: dict, k: int = 0) -> dict:
        return o["traces"][k]

    def extra(o: dict, key: str, k: int = 0):
        return o["traces"][k]["parameters"]["extra"][key]

    def acq(o: dict, key: str, k: int = 0):
        return o["traces"][k]["parameters"]["acqus"][key]

    def table(o: dict, k: int = 0) -> dict:
        return o["tables"][k]

    def ms2(o: dict) -> int:
        return sum(1 for s in o["spectra"]["scans"] if s["ms_level"] == 2)

    out: list[Any] = [
        # ------------------------------------------------ light microscopy: lookups
        spec(
            "ho-c-mic-czi-lattice-slices",
            "c_lattice",
            "dimensions",
            "This is a raw lattice light-sheet acquisition of red blood cells. How many z-slices (sample-scan planes) does the stack have?",
            lambda o, m: g.integer(o_img(o, "size_z")),
            "oracle: /images/0/size_z (czifile)",
        ),
        spec(
            "ho-c-mic-czi-spectral-channels",
            "c_spectral",
            "channels",
            "How many channels were recorded in this confocal image of a barcoded FISH sample?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (czifile)",
        ),
        spec(
            "ho-c-mic-czi-rotifer-zstep",
            "c_rotifer",
            "pixel-size",
            "What is the z-step between the optical sections of this confocal stack of a rotifer egg?",
            lambda o, m: g.number(o_um(o, "z"), "µm", rel=0.02),
            "oracle: /images/0/physical_size_um/z (czifile)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-mic-czi-organoid-pixel",
            "c_organoid",
            "pixel-size",
            "What is the pixel size of this organoid immunofluorescence image?",
            lambda o, m: g.number(o_um(o), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (czifile)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-mic-nd2-bee-zstep",
            "c_bee",
            "pixel-size",
            "This stack of a bee specimen was taken at many focal planes. What is the spacing between focal planes?",
            lambda o, m: g.number(o_um(o, "z"), "µm", rel=0.02),
            "oracle: /images/0/physical_size_um/z (nd2)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-mic-nd2-shear-channel",
            "c_shear",
            "channels",
            "What is the name of the imaging channel (optical configuration) used for this image?",
            lambda o, m: g.string(o_img(o, "channel_names")[0]),
            "oracle: /images/0/channel_names (nd2)",
        ),
        spec(
            "ho-c-mic-nd2-coacervate-pixel",
            "c_coacervate",
            "pixel-size",
            "What is the pixel size of this DIC image?",
            lambda o, m: g.number(o_um(o), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (nd2)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-mic-lif-counting-images",
            "c_counting",
            "counts",
            "How many images (series) does this Leica project file contain?",
            lambda o, m: g.integer(len(o["images"])),
            "oracle: len(/images) (liffile)",
        ),
        spec(
            "ho-c-mic-lif-aphid-images",
            "c_aphid",
            "counts",
            "How many images (series) are stored in this Leica file?",
            lambda o, m: g.integer(len(o["images"])),
            "oracle: len(/images) (liffile)",
        ),
        spec(
            "ho-c-mic-lif-epoxy-pixel",
            "c_epoxy",
            "pixel-size",
            "What is the pixel size of this confocal image, in nanometres?",
            lambda o, m: g.number(o_um(o) * 1000.0, "nm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (liffile), in nm",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-mic-lif-hcar1-pixel",
            "c_hcar1",
            "pixel-size",
            "What is the pixel size of the first image in this file?",
            lambda o, m: g.number(o_um(o), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (liffile)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-mic-oir-channels",
            "c_oir",
            "channels",
            "How many channels does the first image of this Olympus/Evident file have?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (oirfile)",
        ),
        spec(
            "ho-c-mic-oib-spleen-channels",
            "c_oib_spleen",
            "channels",
            "How many channels were recorded in this FluoView image?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (Bio-Formats, black box)",
        ),
        spec(
            "ho-c-mic-oib-mcu-timepoints",
            "c_oib_mcu",
            "dimensions",
            "How many time points does this FluoView recording contain?",
            lambda o, m: g.integer(o_img(o, "size_t")),
            "oracle: /images/0/size_t (Bio-Formats, black box)",
        ),
        spec(
            "ho-c-mic-ims-tracked-timepoints",
            "c_ims_tracked",
            "dimensions",
            "How many time points are in this Imaris file?",
            lambda o, m: g.integer(o_img(o, "size_t")),
            "oracle: /images/0/size_t (h5py)",
        ),
        spec(
            "ho-c-mic-ims-retina-channels",
            "c_ims_retina",
            "channels",
            "How many channels does this multiplexed retina image have?",
            lambda o, m: g.integer(o_img(o, "size_c")),
            "oracle: /images/0/size_c (h5py)",
        ),
        spec(
            "ho-c-mic-ndpi-size",
            "c_ndpi",
            "dimensions",
            "What are the width and height of the full-resolution image of this slide scan, in pixels?",
            lambda o, m: g.items([str(o_img(o, "size_x")), str(o_img(o, "size_y"))]),
            "oracle: /images/0/size_x and size_y (tifffile)",
            answer_hint="width × height in pixels",
        ),
        spec(
            "ho-c-mic-svs-mpp",
            "c_svs",
            "pixel-size",
            "What is the resolution of this immunohistochemistry slide at full resolution, in microns per pixel?",
            lambda o, m: g.number(o_um(o), "µm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (tifffile)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ light microscopy: analysis
        spec(
            "ho-c-mic-czi-centrosome-pixel",
            "c_centro",
            "pixel-size",
            "What is the lateral pixel size of this confocal stack of centrosomes, in nanometres?",
            lambda o, m: g.number(o_um(o) * 1000.0, "nm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (czifile), in nm",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-mic-nd2-septin-brightest-t",
            "c_septin",
            "analysis",
            "This is a TIRF time series. Which time point (counting from 1) has the highest mean intensity?",
            fact("brightest_frame", g.integer),
            "facts-heldout: brightest_frame (nd2)",
            answer_hint="the time point",
        ),
        # ------------------------------------------------ electron microscopy
        spec(
            "ho-c-em-dm4-pixel",
            "c_dm4",
            "pixel-size",
            "This is an atomic-resolution HAADF STEM image. What is its pixel size, in picometres?",
            lambda o, m: g.number(o_um(o) * 1e6, "pm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (dm3_lib), in pm",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-em-dm4-si-channels",
            "c_dm4_si",
            "dimensions",
            "This DigitalMicrograph file holds a STEM spectrum image. How many energy channels does each spectrum have?",
            lambda o, m: g.integer(next(i["size_t"] for i in o["images"] if i["size_t"] > 1)),
            "oracle: /images[*]/size_t of the spectrum image (dm3_lib)",
        ),
        spec(
            "ho-c-em-dm3-size",
            "c_dm3",
            "dimensions",
            "What are the width and height of this image, in pixels?",
            lambda o, m: g.items([str(o_img(o, "size_x")), str(o_img(o, "size_y"))]),
            "oracle: /images/0/size_x and size_y (dm3_lib)",
            answer_hint="width × height in pixels",
        ),
        spec(
            "ho-c-em-ser-tem-pixel",
            "c_ser_tem",
            "pixel-size",
            "What is the pixel size of this TEM image, in nanometres?",
            lambda o, m: g.number(o_um(o) * 1000.0, "nm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (ncempy, black box), in nm",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-em-velox-pixel",
            "c_velox",
            "pixel-size",
            "What is the pixel size of this HAADF image, in picometres?",
            lambda o, m: g.number(o_um(o) * 1e6, "pm", rel=0.01),
            "oracle: /images/0/physical_size_um/x (h5py), in pm",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-em-beacon-stack",
            "c_beacon",
            "counts",
            "How many images does the aberration stack in this EMD file contain?",
            fact("stack_images", g.integer),
            "facts-heldout: stack_images (h5py)",
        ),
        spec(
            "ho-c-em-mrc-bismuth-pixel",
            "c_mrc_bi",
            "pixel-size",
            "What is the pixel size of this image, in ångström?",
            lambda o, m: g.number(o_um(o) * 1e4, "Å", rel=0.01),
            "oracle: /images/0/physical_size_um/x (mrcfile), in Å",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-em-mrc-vault-std",
            "c_mrc_vault",
            "analysis",
            "What is the standard deviation of the voxel values of this subtomogram-average map?",
            fact("map_std", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: map_std (mrcfile; NumPy on the layout agrees)",
            answer_hint=raw,
        ),
        # ------------------------------------------------ flow cytometry
        spec(
            "ho-c-flow-microparticles-events",
            "c_fcs_mp",
            "counts",
            "How many events were recorded in this file?",
            lambda o, m: g.integer(table(o)["event_count"]),
            "oracle: /tables/0/event_count (flowio; fcsparser agrees)",
        ),
        spec(
            "ho-c-flow-cytof-parameters",
            "c_fcs_cytof",
            "counts",
            "How many parameters (channels) does this mass-cytometry FCS file record per event?",
            lambda o, m: g.integer(table(o)["parameter_count"]),
            "oracle: /tables/0/parameter_count (flowio)",
        ),
        spec(
            "ho-c-flow-spectral-events",
            "c_fcs_spectral",
            "counts",
            "How many events does this spectral-cytometry file contain?",
            lambda o, m: g.integer(table(o)["event_count"]),
            "oracle: /tables/0/event_count (flowio; fcsparser agrees)",
        ),
        spec(
            "ho-c-flow-multiangle-parameters",
            "c_fcs_angle",
            "counts",
            "How many parameters does this cytometer record per event?",
            lambda o, m: g.integer(table(o)["parameter_count"]),
            "oracle: /tables/0/parameter_count (flowio)",
        ),
        spec(
            "ho-c-ana-flow-histone-fitc",
            "c_fcs_histone",
            "analysis",
            "What is the median FITC-A value over all events (raw values as stored)?",
            fact("median_fitc_a", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: median_fitc_a (flowio; fcsparser agrees)",
            answer_hint=raw,
        ),
        spec(
            "ho-c-ana-flow-uv-median",
            "c_fcs_uv",
            "analysis",
            "This is a single-stain compensation control for the UV 450/50 detector. What is the median of U 450/50-A over all events?",
            fact("median_u450", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: median_u450 (flowio; fcsparser agrees)",
            answer_hint=raw,
        ),
        spec(
            "ho-c-ana-flow-multiangle-fsc",
            "c_fcs_angle",
            "analysis",
            "What is the median forward-scatter area (FSC-A) over all events?",
            fact("median_fsc_a", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: median_fsc_a (flowio; fcsparser agrees)",
            answer_hint=raw,
        ),
        # ------------------------------------------------ electrophysiology
        spec(
            "ho-c-ephys-ncs-rate",
            "c_ncs",
            "sample-rate",
            "At what rate was this Neuralynx channel sampled?",
            lambda o, m: g.number(tr(o)["sample_rate_hz"], "Hz", rel=0.001),
            "oracle: /traces/0/sample_rate_hz (neo NeuralynxRawIO)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ephys-ns5-rate",
            "c_ns5",
            "sample-rate",
            "What is the sampling rate of this Blackrock continuous file?",
            lambda o, m: g.number(tr(o)["sample_rate_hz"], "Hz", rel=0.001),
            "oracle: /traces/0/sample_rate_hz (neo BlackrockRawIO)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ephys-rhs-channels",
            "c_rhs",
            "channels",
            "How many amplifier channels were recorded in this Intan stimulation/recording file?",
            lambda o, m: g.integer(tr(o)["channel_count"]),
            "oracle: /traces/0/channel_count (neo IntanRawIO, amplifier stream)",
        ),
        spec(
            "ho-c-ephys-nwb-units",
            "c_nwb",
            "counts",
            "How many sorted units does this NWB file contain?",
            lambda o, m: g.integer(next(t["event_count"] for t in o["tables"] if t["name"] == "units")),
            "oracle: /tables[name=units]/event_count (h5py)",
        ),
        spec(
            "ho-c-ephys-abf-stns-channels",
            "c_abf_stns",
            "channels",
            "How many input channels were recorded in this Axon file?",
            lambda o, m: g.integer(tr(o)["channel_count"]),
            "oracle: /traces/0/channel_count (pyabf)",
        ),
        spec(
            "ho-c-ana-ephys-abf-ca1-vm-sweep",
            "c_abf_ca1",
            "analysis",
            "This is a current-clamp step protocol. In which sweep (counting from 1) does the membrane potential (Vm) reach its highest value?",
            fact("vm_peak_sweep", g.integer),
            "facts-heldout: vm_peak_sweep (pyABF; neo AxonRawIO agrees)",
            answer_hint="the sweep number",
        ),
        spec(
            "ho-c-ana-ephys-abf-pore-sweep",
            "c_abf_pore",
            "analysis",
            "This is a nanopore current-voltage (I-V) protocol. Which sweep (counting from 1) reaches the largest current?",
            fact("peak_current_sweep", g.integer),
            "facts-heldout: peak_current_sweep (pyABF; neo AxonRawIO agrees)",
            answer_hint="the sweep number",
        ),
        spec(
            "ho-c-ana-ephys-abf-stns-min",
            "c_abf_stns",
            "analysis",
            "What is the most negative value of the first recorded channel (I_MTest 2) over the whole recording, in mV?",
            fact("ch0_min", lambda v: g.number(v, "mV", abs_=0.05)),
            "facts-heldout: ch0_min (pyABF; neo AxonRawIO agrees)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-ephys-ncs-std",
            "c_ncs",
            "analysis",
            "What is the standard deviation of this local-field-potential channel, in microvolts?",
            fact("signal_std_uv", lambda v: g.number(v, "µV", rel=0.001)),
            "facts-heldout: signal_std_uv (neo; NumPy on the record layout agrees)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ NMR
        spec(
            "ho-c-nmr-jdf-frequency",
            "c_jdf_1h",
            "method",
            "At what spectrometer frequency was this 1H spectrum recorded, in MHz?",
            lambda o, m: g.number(extra(o, "spectrometer_frequency_mhz"), "MHz", rel=0.001),
            "oracle: /traces/0/parameters/extra/spectrometer_frequency_mhz (nmrglue)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-nmr-cari-2d-experiment",
            "c_cari_2d",
            "method",
            "What kind of 2D experiment is this (pulse program), and how many increments were recorded in the indirect dimension?",
            lambda o, m: g.items(
                [acq(o, "PULPROG"), str(o["traces"][0]["sweep_count"])],
                {acq(o, "PULPROG"): ["COSY", "cosy", "gradient COSY", "DQF-COSY"]},
            ),
            "oracle: /traces/0/parameters/acqus/PULPROG and sweep_count (nmrglue)",
            answer_hint="the pulse program and the number of increments",
        ),
        spec(
            "ho-c-nmr-19f-nucleus",
            "c_19f",
            "method",
            "Which nucleus was observed in this experiment?",
            lambda o, m: g.string(acq(o, "NUC1"), accept=["fluorine-19", "F19", "19-F", "fluorine", "F-19"]),
            "oracle: /traces/0/parameters/acqus/NUC1 (nmrglue)",
            answer_hint="the nucleus",
        ),
        spec(
            "ho-c-nmr-19f-frequency",
            "c_19f",
            "method",
            "At what frequency (MHz) was the observed nucleus of this experiment excited (the transmitter frequency)?",
            lambda o, m: g.number(acq(o, "SFO1"), "MHz", rel=0.0005),
            "oracle: /traces/0/parameters/acqus/SFO1 (nmrglue)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-nmr-cari-tallest",
            "c_cari_1d",
            "analysis",
            "In the processed 1H spectrum, at what chemical shift is the tallest peak between 0 and 6 ppm?",
            fact("tallest_ppm_0_6", lambda v: g.number(v, None, abs_=0.01)),
            "facts-heldout: tallest_ppm_0_6 (nmrglue on pdata/1; NumPy agrees)",
            answer_hint="the chemical shift in ppm",
        ),
        spec(
            "ho-c-nmr-jdf-ft-frequency",
            "c_jdf_ft",
            "method",
            "This JEOL file holds a processed 1H spectrum. At what spectrometer frequency was it recorded, in MHz?",
            lambda o, m: g.number(extra(o, "spectrometer_frequency_mhz"), "MHz", rel=0.001),
            "oracle: /traces/0/parameters/extra/spectrometer_frequency_mhz (nmrglue)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ vibrational spectroscopy
        spec(
            "ho-c-spec-dx-points",
            "c_dx_ftir",
            "counts",
            "How many data points does this FT-IR spectrum have?",
            lambda o, m: g.integer(tr(o)["sample_count"]),
            "oracle: /traces/0/sample_count (jcamp)",
        ),
        spec(
            "ho-c-spec-spa-raman-points",
            "c_spa_raman",
            "counts",
            "How many points does this Raman spectrum have?",
            lambda o, m: g.integer(tr(o)["sample_count"]),
            "oracle: /traces/0/sample_count (SpectroChemPy)",
        ),
        spec(
            "ho-c-spec-spg-count",
            "c_spg",
            "counts",
            "How many spectra does this OMNIC spectral group file contain?",
            lambda o, m: g.integer(tr(o)["sweep_count"]),
            "oracle: /traces/0/sweep_count (SpectroChemPy)",
        ),
        spec(
            "ho-c-spec-wdf-spectra",
            "c_wdf",
            "counts",
            "How many spectra does this Raman map contain?",
            lambda o, m: g.integer(tr(o)["sweep_count"]),
            "oracle: /traces/0/sweep_count (renishawWiRE)",
        ),
        spec(
            "ho-c-spec-sp-instrument",
            "c_sp_mp",
            "instrument",
            "Which instrument model recorded this spectrum?",
            lambda o, m: g.string(
                extra(o, "instrument"), accept=["Frontier", "PerkinElmer Frontier", "Frontier FT-IR"]
            ),
            "oracle: /traces/0/parameters/extra/instrument (specio)",
        ),
        spec(
            "ho-c-spec-opus-points",
            "c_opus_si",
            "counts",
            "How many points does the absorbance spectrum in this OPUS file have?",
            lambda o, m: g.integer(tr(o)["sample_count"]),
            "oracle: /traces/0/sample_count (brukeropus; brukeropusreader agrees)",
        ),
        spec(
            "ho-c-spec-jws-points",
            "c_jws_cd",
            "counts",
            "How many data points does this circular dichroism spectrum have?",
            lambda o, m: g.integer(o["jws2txt"]["channels"][0]["n"]),
            "oracle: /jws2txt/channels/0/n (jws2txt, MIT)",
        ),
        spec(
            "ho-c-ana-spec-opus-amide",
            "c_opus_caco2",
            "analysis",
            "In this ATR-FTIR spectrum of cells, at what wavenumber is the strongest absorbance between 1500 and 1800 cm-1?",
            fact("amide_band", lambda v: g.number(v, "cm-1", abs_=4.0)),
            "facts-heldout: amide_band (brukeropus)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-spec-spa-strongest",
            "c_spa_dust",
            "analysis",
            "At what wavenumber does this infrared spectrum show its strongest absorption between 600 and 4000 cm-1?",
            fact("strongest_absorption_600_4000", lambda v: g.number(v, "cm-1", abs_=4.0)),
            "facts-heldout: strongest_absorption_600_4000 (SpectroChemPy; the file stores % transmittance)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-spec-wdf-mean-band",
            "c_wdf",
            "analysis",
            "Average all spectra of this Raman map of an otolith. Between 1000 and 1200 cm-1, at what Raman shift is the tallest peak of the mean spectrum?",
            fact("carbonate_band", lambda v: g.number(v, "cm-1", abs_=2.0)),
            "facts-heldout: carbonate_band (renishawWiRE; SpectroChemPy agrees)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ mass spectrometry
        spec(
            "ho-c-ms-raw-germ-ms2",
            "c_raw_germ",
            "counts",
            "How many MS/MS (MS2) scans does this run contain?",
            lambda o, m: g.integer(ms2(o)),
            "oracle: /spectra/scans with ms_level 2 (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-c-ms-raw-aqc-scans",
            "c_raw_aqc",
            "counts",
            "How many scans (MS1 and MS2 together) does this run contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (pyteomics on the depositor's mzML)",
        ),
        spec(
            "ho-c-ms-dmrm-transitions",
            "c_dmrm",
            "counts",
            "How many SRM/MRM transitions (selected-reaction chromatograms) does this file hold?",
            lambda o, m: g.integer(sum(1 for t in o["traces"] if "SRM" in str(t.get("id", "")))),
            "oracle: /traces with an SRM id (pyteomics)",
        ),
        spec(
            "ho-c-ms-vion-ms2",
            "c_vion",
            "counts",
            "How many MS2 spectra does this ion-mobility QTOF file contain?",
            lambda o, m: g.integer(ms2(o)),
            "oracle: /spectra/scans with ms_level 2 (pyteomics)",
        ),
        spec(
            "ho-c-ana-ms-raw-germ-tic",
            "c_raw_germ",
            "analysis",
            "At what retention time (minutes) does the MS1 total ion chromatogram of this run reach its maximum?",
            fact("ms1_tic_apex_min", lambda v: g.number(v, "min", abs_=0.1)),
            "facts-heldout: ms1_tic_apex_min (pyteomics on the depositor's mzML; its TIC cvParams agree)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-ms-cems-tic",
            "c_cems",
            "analysis",
            "This is a capillary-electrophoresis MS run. At what migration time (minutes) is the total ion current largest?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.05)),
            "facts-heldout: tic_apex_min (pyteomics; TIC cvParams agree)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-ms-mzxml-tic",
            "c_mzxml",
            "analysis",
            "At what retention time (minutes) does the total ion chromatogram of this run peak?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.05)),
            "facts-heldout: tic_apex_min (pyteomics)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-ms-fticr-base-peak",
            "c_fticr",
            "analysis",
            "This file holds one FT-ICR mass spectrum. What is the m/z of its most intense peak?",
            fact("base_peak_mz", lambda v: g.number(v, None, abs_=0.01)),
            "facts-heldout: base_peak_mz (pyteomics)",
            answer_hint="the m/z",
        ),
        # ------------------------------------------------ chromatography
        spec(
            "ho-c-chrom-cdf-scans",
            "c_cdf_sam",
            "counts",
            "How many mass spectra (scans) does this ANDI file contain?",
            lambda o, m: g.integer(o["spectra"]["scan_count"]),
            "oracle: /spectra/scan_count (scipy netcdf)",
        ),
        spec(
            "ho-c-chrom-lcd-largest-peak",
            "c_lcd_kat",
            "values",
            "According to the peak table stored with this HPLC run, at what retention time (minutes) is the largest peak by area?",
            lambda o, m: g.number(o["peak_table"][0]["rt_min"], "min", abs_=0.02),
            "oracle: /peak_table (largest area first; chromConverter, black box, reads the vendor's peak table)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-chrom-gcms-tic",
            "c_gcms",
            "analysis",
            "At what retention time (minutes) does the total ion chromatogram of this GC-MS run of floral volatiles reach its maximum?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: tic_apex_min (the depositor's TIC export; rainbow-api agrees)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-chrom-cdf-serum-tic",
            "c_cdf_serum",
            "analysis",
            "At what time (minutes) is the total ion current of this GC-MS run largest?",
            fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
            "facts-heldout: tic_apex_min (scipy netcdf; the summed scans agree)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ plate readers, cell metabolism
        spec(
            "ho-c-ana-plate-gen5-lowest",
            "c_gen5",
            "analysis",
            "This Biolog EcoPlate was read at 590 nm. Which well has the lowest absorbance?",
            fact("lowest_well_590", g.string),
            "facts-heldout: lowest_well_590 (pandas on the Gen5 export)",
            answer_hint="the well, e.g. B7",
        ),
        spec(
            "ho-c-ana-plate-bmg-excitation",
            "c_bmg",
            "analysis",
            "This is a fluorescence excitation scan. At what excitation wavelength (nm) is the signal of well A01 highest?",
            fact("a01_peak_excitation_nm", lambda v: g.number(v, "nm", abs_=1.0)),
            "facts-heldout: a01_peak_excitation_nm (the export text)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-c-ana-plate-envision-top",
            "c_envision",
            "analysis",
            "Which well has the highest raw luminescence (before crosstalk correction)?",
            fact("top_well_raw", g.string),
            "facts-heldout: top_well_raw (the export text)",
            answer_hint="the well, e.g. B7",
        ),
        spec(
            "ho-c-xf-glyco-measurements",
            "c_xf_glyco",
            "counts",
            "How many measurement cycles did this Seahorse glycolysis stress test have?",
            lambda o, m: g.integer(o["measurements"]),
            "oracle: /measurements (seahorse_oracle.py)",
        ),
        spec(
            "ho-c-xf-perm-measurements",
            "c_xf_perm",
            "counts",
            "How many measurement cycles were run in this Seahorse assay?",
            lambda o, m: g.integer(o["measurements"]),
            "oracle: /measurements (seahorse_oracle.py)",
        ),
        # ------------------------------------------------ qPCR
        spec(
            "ho-c-qpcr-lamp-negatives",
            "c_lamp",
            "counts",
            "This is an isothermal amplification (LAMP) run. In how many wells did the software call no amplification (undetermined)?",
            lambda o, m: g.integer(sum(1 for r in o["records"] if r.get("cq_undetermined"))),
            "oracle: /records with cq_undetermined (the vendor's results in the .eds)",
        ),
        spec(
            "ho-c-ana-qpcr-lamp-fastest",
            "c_lamp",
            "analysis",
            "Which assay target amplified fastest on average (lowest mean threshold cycle over the wells where it amplified)?",
            lambda o, m: g.string(lamp_fastest_target(o)),
            "oracle: /records (the vendor's Cq values in the .eds, averaged per target)",
            answer_hint="the target name",
        ),
        spec(
            "ho-c-qpcr-vero-plate",
            "c_vero",
            "counts",
            "How many wells does the plate format of this run have?",
            lambda o, m: g.integer(o["rows"] * o["columns"]),
            "oracle: /rows × /columns (the .eds plate setup)",
        ),
        spec(
            "ho-c-qpcr-tsa-targets",
            "c_tsa",
            "counts",
            "This is a protein thermal-shift run. How many different targets (proteins, including controls) are assigned to wells?",
            lambda o, m: g.integer(len({r["target"] for r in o["records"]})),
            "oracle: /records targets (the .eds plate setup)",
        ),
        # ------------------------------------------------ bench instruments
        spec(
            "ho-c-gpr-cu-features",
            "c_gpr_cu",
            "counts",
            "How many features (spots) does this GenePix results file list?",
            lambda o, m: g.integer(o["rows"]),
            "oracle: /rows (pandas)",
        ),
        spec(
            "ho-c-gpr-cu-scanner",
            "c_gpr_cu",
            "instrument",
            "Which scanner acquired this microarray?",
            lambda o, m: g.string(o["scanner"], accept=["Agilent G2505C", "G2505C scanner"]),
            "oracle: /scanner (the file's Scanner record)",
        ),
        spec(
            "ho-c-ana-gpr-prairie-median",
            "c_gpr_prairie",
            "analysis",
            "What is the median of the F532 Median column over all features?",
            fact("median_f532_median", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: median_f532_median (pandas)",
            answer_hint=raw,
        ),
        spec(
            "ho-c-ana-gpr-cu-empty",
            "c_gpr_cu",
            "analysis",
            "How many features are empty negative-control spots (named Empty_NCTRL)?",
            fact("empty_nctrl_count", g.integer),
            "facts-heldout: empty_nctrl_count (pandas)",
        ),
        spec(
            "ho-c-scn-tgev-width",
            "c_scn_tgev",
            "dimensions",
            "How wide is this blot image, in pixels?",
            lambda o, m: g.integer(o_img(o, "size_x")),
            "oracle: /images/0/size_x (Bio-Formats, black box)",
        ),
        spec(
            "ho-c-scn-coq8-size",
            "c_scn_coq8",
            "dimensions",
            "What are the width and height of this gel image, in pixels?",
            lambda o, m: g.items([str(o_img(o, "size_x")), str(o_img(o, "size_y"))]),
            "oracle: /images/0/size_x and size_y (Bio-Formats, black box)",
            answer_hint="width × height in pixels",
        ),
    ]
    return out
