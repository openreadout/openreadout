"""Held-out draw D (2026-10-06): electrophysiology, NMR, optical spectroscopy and flow cytometry.

Part of evals/heldout_draw_d.py; the questions were written from the held-out oracles, the
depositors' exports and records, and facts computed with third-party readers, before OpenReadout
was run on any of these files.

The JASCO fact runs jws2txt (MIT): JWS2TXT_DIR names the directory that holds its `helpers.py`,
as for oracle/gen_heldout.py.
"""

from __future__ import annotations

import os
import sys
from typing import Any

C: dict[str, str] = {
    # electrophysiology
    "d_signals_ns5": "ho-figshare10280939-ns5-dorsalroot",
    "d_signals_smr": "ho-zenodo10579531-smr-fg7142",
    "d_signals_wcp": "ho-zenodo5338927-wcp-charaxes",
    "d_signals_abf_squid": "ho-figshare11717355-abf-squid",
    "d_signals_abf_wichr": "ho-figshare30052543-abf-wichr",
    "d_signals_atf": "ho-zenodo7995719-atf-ina",
    "d_signals_nwb": "ho-figshare19298747-nwb-ash",
    "d_signals_heka": "ho-zenodo5794197-heka-locus",
    "d_signals_rhd": "ho-figshare28446950-rhd-amph",
    "d_signals_sglx": "ho-gh-cullenlab-npxl-nosync",
    "d_signals_sglx_meta": "ho-gh-cullenlab-npxl-nosync-meta",
    "d_signals_oe_nidaq": "ho-gh-brainbremen-oecon-nidaq",
    "d_signals_oe_rhythm": "ho-gh-elgarulli-neurokin-rhythm",
    # NMR
    "d_signals_jdf": "ho-zenodo3722793-jdf-malonate",
    "d_signals_jdx": "ho-zenodo3722793-jdx-malonate",
    "d_signals_honey": "ho-zenodo10969320-bruker-honey",
    "d_signals_his": "ho-zenodo15163468-bruker-his",
    "d_signals_cosy": "ho-zenodo4694912-varian-cosy",
    "d_signals_spinsolve": "ho-zenodo15163468-spinsolve-t2",
    # spectroscopy
    "d_signals_opus_ice": "ho-zenodo14095495-opus-ice",
    "d_signals_opus_dom": "ho-zenodo17238934-opus-dom",
    "d_signals_spa": "ho-figshare32287374-spa-rebar",
    "d_signals_sp": "ho-figshare21354474-sp-uvvis",
    "d_signals_spc": "ho-figshare30060709-spc-mars",
    "d_signals_wip_zncds": "ho-figshare26206907-wip-zncds",
    "d_signals_wip_bone": "ho-figshare22060427-wip-bone",
    "d_signals_jws": "ho-figshare29163440-jws-bradford",
}


# ---------------------------------------------------------------- facts (oracle venv only)


def _h():
    import heldout

    return heldout


def abf_extreme_sweep(cid: str, channel: int, kind: str, neo: bool = False):
    """Sweep (counting from 1) whose `kind` ("min" or "max") value on `channel` is the most extreme;
    stops unless it beats the runner-up by more than 1 %."""
    import analysis as a

    sw, _ = (a.abf_sweeps_neo if neo else a.abf_sweeps)(cid, channel)
    vals = [float(y.min()) if kind == "min" else float(y.max()) for y in sw]
    order = sorted(range(len(vals)), key=lambda i: vals[i], reverse=kind == "max")
    best, second = vals[order[0]], vals[order[1]]
    assert abs(best - second) > 0.01 * abs(best), vals
    return order[0] + 1, _h().rd("neo") + " AxonRawIO" if neo else _h().rd("pyabf")


def atf_most_negative_sweep(cid: str, numpy: bool = False):
    """Sweep (counting from 1) of an Axon Text File with the most negative current: pyABF's ATF
    reader, or (`numpy`) the text itself (header line 2 gives the header record count; then a column
    of times and one column per sweep)."""
    import numpy as np

    if numpy:
        lines = _h().hpath(cid).read_text(errors="replace").splitlines()
        n_header = int(lines[1].split()[0])
        rows = np.asarray([[float(v) for v in line.split("\t")] for line in lines[3 + n_header :] if line.strip()])
        mins = rows[:, 1:].min(axis=0)
        reader = "NumPy on the ATF text"
    else:
        import pyabf

        a = pyabf.ATF(str(_h().hpath(cid)))
        mins = []
        for s in a.sweepList:
            a.setSweep(s)
            mins.append(float(a.sweepY.min()))
        mins = np.asarray(mins)
        reader = _h().rd("pyabf") + " ATF"
    order = np.argsort(mins)
    assert mins[order[1]] - mins[order[0]] > 0.01 * abs(mins[order[0]]), mins
    return int(order[0]) + 1, reader


def neo_noisiest_channel(cid: str, kind: str):
    """Name of the channel with the largest standard deviation of its scaled samples (Neo raw IO:
    Spike2 with one stream per waveform channel, or the Intan amplifier stream); stops unless it beats
    the runner-up by more than 5 %."""
    import numpy as np
    from neo.rawio import IntanRawIO, Spike2RawIO

    p = str(_h().hpath(cid))
    if kind == "spike2":
        r = Spike2RawIO(filename=p, try_signal_grouping=False)
        r.parse_header()
        streams = range(r.signal_streams_count())
    else:
        r = IntanRawIO(filename=p)
        r.parse_header()
        streams = [0]
    names, sds = [], []
    for st in streams:
        n = r.get_signal_size(0, 0, st)
        s1 = s2 = 0.0
        for i0 in range(0, n, 2_000_000):
            raw = r.get_analogsignal_chunk(0, 0, i0, min(n, i0 + 2_000_000), st)
            y = r.rescale_signal_raw_to_float(raw, dtype="float64", stream_index=st)
            s1, s2 = s1 + y.sum(axis=0), s2 + (y * y).sum(axis=0)
        sd = np.sqrt(s2 / n - (s1 / n) ** 2)
        chans = r.header["signal_channels"]
        names += [str(c["name"]) for c in chans if c["stream_id"] == r.header["signal_streams"][st]["id"]]
        sds += [float(v) for v in np.atleast_1d(sd)]
    order = sorted(range(len(sds)), key=sds.__getitem__, reverse=True)
    assert sds[order[0]] > 1.05 * sds[order[1]], list(zip(names, sds, strict=True))
    return names[order[0]], _h().rd("neo") + (" Spike2RawIO" if kind == "spike2" else " IntanRawIO")


def nwb_current_clamp_count(cid: str, pynwb: bool = False):
    """Current-clamp recordings (CurrentClampSeries) in the file's acquisition group."""
    if pynwb:
        from pynwb import NWBHDF5IO
        from pynwb.icephys import CurrentClampSeries

        with _h()._quiet(), NWBHDF5IO(str(_h().hpath(cid)), "r", load_namespaces=True) as io:
            f = io.read()
            n = sum(1 for s in f.acquisition.values() if isinstance(s, CurrentClampSeries))
        return n, _h().rd("pynwb")
    import h5py

    with h5py.File(_h().hpath(cid), "r") as f:
        g = f["acquisition"]
        n = sum(1 for k in g if g[k].attrs.get("neurodata_type") == "CurrentClampSeries")
    return n, _h().rd("h5py")


def nwb_lab_institution(cid: str, pynwb: bool = False):
    if pynwb:
        from pynwb import NWBHDF5IO

        with _h()._quiet(), NWBHDF5IO(str(_h().hpath(cid)), "r", load_namespaces=True) as io:
            f = io.read()
            return [str(f.lab), str(f.institution)], _h().rd("pynwb")
    import h5py

    with h5py.File(_h().hpath(cid), "r") as f:
        return [f["general/lab"][()].decode(), f["general/institution"][()].decode()], _h().rd("h5py")


def bruker_tallest_window(cid: str, lo: float, hi: float, raw: bool = False):
    import analysis as a

    return a.pdata_ppm(cid, lo, hi, raw=raw)


def jdx_tallest_ppm(cid: str, second: bool = False):
    """Chemical shift (ppm) of the highest point of a JCAMP-DX NMR spectrum whose x axis is in Hz:
    x / observe frequency (MHz). nmrglue's jcampdx reader, or (`second`) the jcamp package."""
    import numpy as np

    p = _h().hpath(cid)
    if second:
        import jcamp

        blocks = jcamp.jcamp_readfile(str(p)) if hasattr(jcamp, "jcamp_readfile") else jcamp.readfile(str(p))
        d = next(b for b in blocks.get("children", [blocks]) if "y" in b and len(b["y"]) > 100)
        sf = float(d[".observe frequency"])
        x, y = np.asarray(d["x"], dtype="float64"), np.asarray(d["y"], dtype="float64")
        reader = _h().rd("jcamp")
    else:
        import nmrglue as ng

        dic, data = ng.jcampdx.read(str(p))
        y = np.asarray(data, dtype="float64")
        sf = float(dic[".OBSERVEFREQUENCY"][0])
        x = np.linspace(float(dic["FIRSTX"][0]), float(dic["LASTX"][0]), y.size)
        reader = _h().rd("nmrglue") + " jcampdx"
    s = np.sort(y)
    assert s[-1] > 1.5 * y[np.abs(x / sf - x[int(y.argmax())] / sf) > 0.2].max()
    return round(float(x[int(y.argmax())] / sf), 3), reader


def spc_band(cid: str, lo: float, hi: float, spcio: bool = False):
    import numpy as np

    H = _h()
    if spcio:
        from spc_io import SPC

        with open(H.hpath(cid), "rb") as f:
            s = SPC.from_bytes_io(f)
        x = np.asarray(s.xarray, dtype="float64")
        y = np.asarray(next(iter(s)).yarray, dtype="float64")
        return H.band(x, y, lo, hi, "max"), H.rd("spc-io")
    import spectrochempy as scp

    with H._quiet():
        d = scp.read_spc(str(H.hpath(cid)))
    return H.band(np.asarray(d.x.data, dtype="float64"), np.asarray(d.data, dtype="float64")[0], lo, hi, "max"), (
        H.rd("spectrochempy")
    )


def pesp_value_at(cid: str, at: float):
    x, y = _h().pesp_xy(cid)
    return _h().value_at(x, y, at), _h().rd("specio")


def jws_peak_nm(cid: str, lo: float, hi: float):
    """Wavelength (nm) of the highest absorbance within [lo, hi] (jws2txt, MIT, run as a black box)."""
    import numpy as np

    lib = os.environ.get("JWS2TXT_DIR")
    if not lib:
        raise SystemExit("JWS2TXT_DIR must name the directory holding jws2txt's helpers.py")
    sys.path.insert(0, lib)
    from helpers import JWSFile

    d = JWSFile(str(_h().hpath(cid))).unpacked_data
    x, y = np.asarray(d[0], dtype="float64"), np.asarray(d[1], dtype="float64")
    m = (x >= lo) & (x <= hi)
    return float(x[m][int(y[m].argmax())]), "jws2txt 1.0 (MIT)"


def _bundle_dir(cid: str):
    return _h().hpath(cid)


def vdlist_longest_s(cid: str):
    """Longest relaxation delay (s) in the experiment's vdlist (the depositor's TopSpin file)."""
    vals = [float(v.rstrip("s")) for v in (_bundle_dir(cid) / "vdlist").read_text().split()]
    return max(vals), "the experiment's vdlist (TopSpin text file)"


def sglx_meta_value(cid: str, key: str):
    """A key of the SpikeGLX .meta (the depositor's text file next to the .bin)."""
    p = _h().hpath(cid).with_suffix(".meta")
    for line in p.read_text(errors="replace").splitlines():
        k, _, v = line.partition("=")
        if k == key:
            return v, "the SpikeGLX .meta text"
    raise KeyError(key)


def facts(Fact, H) -> list:
    """The analysis-tier and file-text facts of the signals area (computed in the oracle venv)."""
    return [
        Fact(
            C["d_signals_abf_squid"],
            "most_negative_current_sweep",
            "sweep (counting from 1) whose current channel (IN 1, pA) reaches the most negative value",
            lambda: abf_extreme_sweep(C["d_signals_abf_squid"], 1, "min"),
            lambda: abf_extreme_sweep(C["d_signals_abf_squid"], 1, "min", neo=True),
        ),
        Fact(
            C["d_signals_abf_wichr"],
            "largest_current_sweep",
            "sweep (counting from 1) whose Im_scaled channel reaches the highest value",
            lambda: abf_extreme_sweep(C["d_signals_abf_wichr"], 0, "max"),
            lambda: abf_extreme_sweep(C["d_signals_abf_wichr"], 0, "max", neo=True),
        ),
        Fact(
            C["d_signals_atf"],
            "most_negative_sweep",
            "sweep (counting from 1) with the most negative current",
            lambda: atf_most_negative_sweep(C["d_signals_atf"]),
            lambda: atf_most_negative_sweep(C["d_signals_atf"], numpy=True),
        ),
        Fact(
            C["d_signals_smr"],
            "noisiest_lfp_channel",
            "waveform channel with the largest standard deviation over the whole recording",
            lambda: neo_noisiest_channel(C["d_signals_smr"], "spike2"),
        ),
        Fact(
            C["d_signals_rhd"],
            "noisiest_amplifier_channel",
            "amplifier channel with the largest standard deviation over the whole recording (µV)",
            lambda: neo_noisiest_channel(C["d_signals_rhd"], "intan"),
        ),
        Fact(
            C["d_signals_nwb"],
            "current_clamp_series",
            "number of CurrentClampSeries in the acquisition group",
            lambda: nwb_current_clamp_count(C["d_signals_nwb"]),
            lambda: nwb_current_clamp_count(C["d_signals_nwb"], pynwb=True),
        ),
        Fact(
            C["d_signals_nwb"],
            "lab_institution",
            "general/lab and general/institution of the file",
            lambda: nwb_lab_institution(C["d_signals_nwb"]),
            lambda: nwb_lab_institution(C["d_signals_nwb"], pynwb=True),
        ),
        Fact(
            C["d_signals_honey"],
            "tallest_ppm_05_45",
            "chemical shift of the highest point of TopSpin's pdata/1/1r between 0.5 and 4.5 ppm",
            lambda: bruker_tallest_window(C["d_signals_honey"], 0.5, 4.5),
            lambda: bruker_tallest_window(C["d_signals_honey"], 0.5, 4.5, raw=True),
        ),
        Fact(
            C["d_signals_jdx"],
            "tallest_ppm",
            "chemical shift of the highest point of the spectrum (x in Hz / observe frequency)",
            lambda: jdx_tallest_ppm(C["d_signals_jdx"]),
            lambda: jdx_tallest_ppm(C["d_signals_jdx"], second=True),
            rel=1e-3,
        ),
        Fact(
            C["d_signals_his"],
            "longest_delay_s",
            "longest relaxation delay in vdlist, in seconds",
            lambda: vdlist_longest_s(C["d_signals_his"]),
        ),
        Fact(
            C["d_signals_opus_ice"],
            "ch_stretch_band",
            "wavenumber of the highest absorbance between 2700 and 3100 cm-1 (block AB)",
            lambda: H.opus_extreme(C["d_signals_opus_ice"], "a", 2700, 3100, "max"),
        ),
        Fact(
            C["d_signals_opus_dom"],
            "band_1500_1800",
            "wavenumber of the highest absorbance between 1500 and 1800 cm-1 (block AB)",
            lambda: H.opus_extreme(C["d_signals_opus_dom"], "a", 1500, 1800, "max"),
        ),
        Fact(
            C["d_signals_spc"],
            "band_900_1200",
            "wavenumber of the highest absorbance between 900 and 1200 cm-1",
            lambda: spc_band(C["d_signals_spc"], 900, 1200),
            lambda: spc_band(C["d_signals_spc"], 900, 1200, spcio=True),
            rel=1e-4,
        ),
        Fact(
            C["d_signals_sp"],
            "absorbance_562",
            "absorbance at 562 nm (the depositor's CSV export, linear interpolation)",
            lambda: H.export_value_at(C["d_signals_sp"], 562.0),
            lambda: pesp_value_at(C["d_signals_sp"], 562.0),
            rel=1e-4,
        ),
        Fact(
            C["d_signals_jws"],
            "peak_nm_500_700",
            "wavelength of the highest absorbance between 500 and 700 nm",
            lambda: jws_peak_nm(C["d_signals_jws"], 500, 700),
        ),
        Fact(
            C["d_signals_sglx"],
            "probe_part_number",
            "imDatPrb_pn of the .meta",
            lambda: sglx_meta_value(C["d_signals_sglx"], "imDatPrb_pn"),
        ),
    ]


# ---------------------------------------------------------------- questions


def specs(spec, fact, g, H) -> list[Any]:
    unit_hint = "a number with its unit"

    def tr(o: dict, k: int = 0) -> dict:
        return o["traces"][k]

    def extra(o: dict, key: str, k: int = 0):
        return o["traces"][k]["parameters"]["extra"][key]

    def acq(o: dict, key: str, k: int = 0):
        return o["traces"][k]["parameters"]["acqus"][key]

    def seconds(o: dict, k: int = 0) -> float:
        t = o["traces"][k]
        return t["sweeps"][0]["sample_count"] / t["sample_rate_hz"]

    return [
        # ------------------------------------------------ electrophysiology: lookups
        spec(
            "ho-d-ephys-blackrock-channels",
            "d_signals_ns5",
            "channels",
            "How many channels does this Blackrock continuous file hold?",
            lambda o, m: g.integer(tr(o)["channel_count"]),
            "oracle: /traces/0/channel_count (neo BlackrockRawIO)",
        ),
        spec(
            "ho-d-ephys-spike2-rate",
            "d_signals_smr",
            "sample-rate",
            "At what rate were the local field potential channels of this Spike2 file sampled?",
            lambda o, m: g.number(tr(o)["sample_rate_hz"], "Hz", rel=0.001),
            "oracle: /traces/0/sample_rate_hz (neo Spike2RawIO; the three waveform channels share it)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ephys-spike2-duration",
            "d_signals_smr",
            "dimensions",
            "How long is this recording, in minutes?",
            lambda o, m: g.number(seconds(o) / 60.0, "min", rel=0.002),
            "oracle: /traces/0/sweeps/0/sample_count / sample_rate_hz (neo Spike2RawIO)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ephys-winwcp-records",
            "d_signals_wcp",
            "counts",
            "How many records (sweeps) does this WinWCP file contain?",
            lambda o, m: g.integer(tr(o)["sweep_count"]),
            "oracle: /traces/0/sweep_count (neo WinWcpRawIO)",
        ),
        spec(
            "ho-d-ephys-abf-squid-rate",
            "d_signals_abf_squid",
            "sample-rate",
            "At what sampling rate was this squid muscle recording acquired?",
            lambda o, m: g.number(tr(o)["sample_rate_hz"], "Hz", rel=0.001),
            "oracle: /traces/0/sample_rate_hz (pyabf)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ephys-abf-wichr-channels",
            "d_signals_abf_wichr",
            "channels",
            "How many input channels were recorded in this Axon file?",
            lambda o, m: g.integer(tr(o)["channel_count"]),
            "oracle: /traces/0/channel_count (pyabf)",
        ),
        spec(
            "ho-d-ephys-atf-sweeps",
            "d_signals_atf",
            "counts",
            "How many sweeps does this Axon text file contain?",
            lambda o, m: g.integer(tr(o)["sweep_count"]),
            "oracle: /traces/0/sweep_count (the ATF text; pyabf agrees)",
        ),
        spec(
            "ho-d-ephys-nwb-lab",
            "d_signals_nwb",
            "operator",
            "In which lab, at which institution, were these recordings made?",
            fact(
                "lab_institution",
                lambda v: g.items(
                    v,
                    {v[0]: [v[0].replace(" Lab", ""), v[0].replace("Lab", "laboratory")], v[1]: [v[1].split()[0]]},
                ),
            ),
            "facts-heldout: lab_institution (h5py: general/lab and general/institution; pynwb agrees)",
            answer_hint="the lab and the institution",
        ),
        spec(
            "ho-d-ephys-heka-sweeps",
            "d_signals_heka",
            "counts",
            "How many sweeps does the series in this PatchMaster file have, and how many traces (channels) does "
            "each sweep hold?",
            lambda o, m: g.items([str(tr(o)["sweep_count"]), str(tr(o)["channel_count"])], ordered=True),
            "oracle: /traces/0/sweep_count and channel_count (pyHEKA, black box)",
            answer_hint="the number of sweeps, then the number of traces per sweep",
        ),
        spec(
            "ho-d-ephys-intan-channels",
            "d_signals_rhd",
            "channels",
            "How many amplifier channels were recorded in this Intan file?",
            lambda o, m: g.integer(tr(o)["channel_count"]),
            "oracle: /traces/0/channel_count (neo IntanRawIO, amplifier stream)",
        ),
        spec(
            "ho-d-ephys-intan-duration",
            "d_signals_rhd",
            "dimensions",
            "How long is this recording, in seconds?",
            lambda o, m: g.number(seconds(o), "s", rel=0.001),
            "oracle: /traces/0/sweeps/0/sample_count / sample_rate_hz (neo IntanRawIO)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ephys-spikeglx-channels",
            "d_signals_sglx",
            "channels",
            "How many channels were saved in this Neuropixels AP-band file (including the sync channel)?",
            lambda o, m: g.integer(tr(o)["channel_count"]),
            "oracle: /traces/0/channel_count (neo SpikeGLXRawIO: main and SYNC streams)",
            stage_as="sample.imec0.ap.bin",
            extra=[(C["d_signals_sglx_meta"], "sample.imec0.ap.meta")],
        ),
        spec(
            "ho-d-ephys-spikeglx-probe",
            "d_signals_sglx",
            "instrument",
            "Which Neuropixels probe model (part number) recorded this file?",
            fact("probe_part_number", lambda v: g.string(v, accept=[f"Neuropixels {v}", f"NP {v[2:]}"])),
            "facts-heldout: probe_part_number (the .meta's imDatPrb_pn)",
            stage_as="sample.imec0.ap.bin",
            extra=[(C["d_signals_sglx_meta"], "sample.imec0.ap.meta")],
            answer_hint="the probe part number",
        ),
        spec(
            "ho-d-ephys-openephys-nidaq-rate",
            "d_signals_oe_nidaq",
            "sample-rate",
            "This Open Ephys recording comes from an NI-DAQ board. At what rate were its analog channels sampled?",
            lambda o, m: g.number(tr(o)["sample_rate_hz"], "Hz", rel=0.001),
            "oracle: /traces/0/sample_rate_hz (neo OpenEphysBinaryRawIO)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ephys-openephys-nidaq-duration",
            "d_signals_oe_nidaq",
            "dimensions",
            "How long is this recording, in seconds?",
            lambda o, m: g.number(seconds(o), "s", rel=0.001),
            "oracle: /traces/0/sweeps/0/sample_count / sample_rate_hz (neo OpenEphysBinaryRawIO)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ephys-openephys-rhythm-channels",
            "d_signals_oe_rhythm",
            "channels",
            "How many continuous channels does the acquisition-board stream of this Open Ephys recording have?",
            lambda o, m: g.integer(tr(o)["channel_count"]),
            "oracle: /traces/0/channel_count (neo OpenEphysBinaryRawIO, Rhythm_FPGA-100.0)",
        ),
        # ------------------------------------------------ electrophysiology: analysis
        spec(
            "ho-d-ana-ephys-abf-squid-current",
            "d_signals_abf_squid",
            "analysis",
            "This is a voltage step protocol on a squid muscle fibre. In which sweep (counting from 1) does the "
            "current reach its most negative value?",
            fact("most_negative_current_sweep", g.integer),
            "facts-heldout: most_negative_current_sweep (pyABF; neo AxonRawIO agrees)",
            answer_hint="the sweep number",
        ),
        spec(
            "ho-d-ana-ephys-abf-wichr-current",
            "d_signals_abf_wichr",
            "analysis",
            "This is an optogenetics recording with light pulses. Which sweep (counting from 1) reaches the "
            "largest membrane current (Im_scaled)?",
            fact("largest_current_sweep", g.integer),
            "facts-heldout: largest_current_sweep (pyABF; neo AxonRawIO agrees)",
            answer_hint="the sweep number",
        ),
        spec(
            "ho-d-ana-ephys-atf-peak-sweep",
            "d_signals_atf",
            "analysis",
            "These are sodium-current activation traces. Which sweep (counting from 1) has the most negative current?",
            fact("most_negative_sweep", g.integer),
            "facts-heldout: most_negative_sweep (pyABF ATF reader; NumPy on the text agrees)",
            answer_hint="the sweep number",
        ),
        spec(
            "ho-d-ana-ephys-spike2-noisiest",
            "d_signals_smr",
            "analysis",
            "This file holds local field potentials from three brain regions. Which channel has the largest "
            "standard deviation over the whole recording?",
            fact("noisiest_lfp_channel", lambda v: g.string(v)),
            "facts-heldout: noisiest_lfp_channel (neo Spike2RawIO)",
            answer_hint="the channel name",
        ),
        spec(
            "ho-d-ana-ephys-intan-noisiest",
            "d_signals_rhd",
            "analysis",
            "Which amplifier channel of this Intan recording has the largest standard deviation over the whole "
            "recording?",
            fact("noisiest_amplifier_channel", lambda v: g.string(v)),
            "facts-heldout: noisiest_amplifier_channel (neo IntanRawIO)",
            answer_hint="the channel name",
        ),
        spec(
            "ho-d-ana-ephys-nwb-current-clamp",
            "d_signals_nwb",
            "analysis",
            "How many current-clamp recordings (as opposed to voltage-clamp ones) does this NWB file contain?",
            fact("current_clamp_series", g.integer),
            "facts-heldout: current_clamp_series (h5py; pynwb agrees)",
        ),
        # ------------------------------------------------ NMR: lookups
        spec(
            "ho-d-nmr-jdf-frequency",
            "d_signals_jdf",
            "method",
            "At what spectrometer frequency was this 1H spectrum recorded, in MHz?",
            lambda o, m: g.number(extra(o, "spectrometer_frequency_mhz"), "MHz", rel=0.001),
            "oracle: /traces/0/parameters/extra/spectrometer_frequency_mhz (nmrglue)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-nmr-jdx-points",
            "d_signals_jdx",
            "counts",
            "How many data points does the NMR spectrum in this JCAMP-DX file have?",
            lambda o, m: g.integer(tr(o)["sample_count"]),
            "oracle: /traces/0/sample_count (nmrglue jcampdx; jcamp agrees)",
        ),
        spec(
            "ho-d-nmr-bruker-honey-method",
            "d_signals_honey",
            "method",
            "Which pulse program was used for this 1H experiment?",
            lambda o, m: g.string(acq(o, "PULPROG"), accept=["zgpr (presaturation)", "Bruker zgpr"]),
            "oracle: /traces/0/parameters/acqus/PULPROG (nmrglue)",
            answer_hint="the pulse program",
        ),
        spec(
            "ho-d-nmr-bruker-his-delays",
            "d_signals_his",
            "counts",
            "This is a series of relaxation experiments stored as one pseudo-2D data set. How many relaxation "
            "delays (rows) does it hold, and which nucleus was observed?",
            lambda o, m: g.items(
                [str(tr(o)["sweep_count"]), acq(o, "NUC1")],
                {acq(o, "NUC1"): ["nitrogen-15", "N15", "15-N", "N-15"]},
                ordered=True,
            ),
            "oracle: /traces/0/sweep_count and parameters/acqus/NUC1 (nmrglue)",
            answer_hint="the number of delays, then the nucleus",
        ),
        spec(
            "ho-d-nmr-bruker-his-longest-delay",
            "d_signals_his",
            "method",
            "What is the longest relaxation delay used in this series, in seconds?",
            fact("longest_delay_s", lambda v: g.number(v, "s", rel=0.001)),
            "facts-heldout: longest_delay_s (the experiment's vdlist)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-nmr-varian-cosy",
            "d_signals_cosy",
            "method",
            "Which pulse sequence was run in this experiment, and how many increments were recorded in the "
            "indirect dimension?",
            lambda o, m: g.items(
                [extra(o, "pulse_program"), str(tr(o)["sweep_count"])],
                {extra(o, "pulse_program"): ["COSY", "gradient COSY", "gCOSY (gradient COSY)"]},
                ordered=True,
            ),
            "oracle: /traces/0/parameters/extra/pulse_program and sweep_count (nmrglue varian)",
            answer_hint="the pulse sequence, then the number of increments",
        ),
        spec(
            "ho-d-nmr-spinsolve-steps",
            "d_signals_spinsolve",
            "counts",
            "This is a T2 (CPMG) measurement on a benchtop NMR spectrometer. How many steps does the 2D data set hold?",
            lambda o, m: g.integer(tr(o, 1)["sweep_count"]),
            "oracle: /traces/1/sweep_count (spinsolve.py: the documented data.2d layout; nmrglue for the header)",
        ),
        spec(
            "ho-d-nmr-spinsolve-frequency",
            "d_signals_spinsolve",
            "method",
            "At what 1H frequency does this benchtop spectrometer work, in MHz?",
            lambda o, m: g.number(extra(o, "reference_frequency_mhz"), "MHz", rel=0.001),
            "oracle: /traces/0/parameters/extra/reference_frequency_mhz (nmrglue spinsolve: acqu.par b1Freq)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ NMR: analysis
        spec(
            "ho-d-ana-nmr-bruker-honey-tallest",
            "d_signals_honey",
            "analysis",
            "In the processed 1H spectrum of this honey extract, at what chemical shift is the tallest peak "
            "between 0.5 and 4.5 ppm?",
            fact("tallest_ppm_05_45", lambda v: g.number(v, None, abs_=0.01)),
            "facts-heldout: tallest_ppm_05_45 (nmrglue on pdata/1; NumPy agrees)",
            answer_hint="the chemical shift in ppm",
        ),
        spec(
            "ho-d-ana-nmr-jdx-tallest",
            "d_signals_jdx",
            "analysis",
            "At what chemical shift (ppm) is the tallest peak of this 1H spectrum?",
            fact("tallest_ppm", lambda v: g.number(v, None, abs_=0.02)),
            "facts-heldout: tallest_ppm (nmrglue jcampdx; jcamp agrees)",
            answer_hint="the chemical shift in ppm",
        ),
        # ------------------------------------------------ spectroscopy: lookups
        spec(
            "ho-d-spec-opus-ice-instrument",
            "d_signals_opus_ice",
            "instrument",
            "Which spectrometer model recorded this infrared spectrum?",
            lambda o, m: g.string(extra(o, "instrument"), accept=["Bruker INVENIO-R", "INVENIO R", "Bruker INVENIO R"]),
            "oracle: /traces/0/parameters/extra/instrument (brukeropus)",
        ),
        spec(
            "ho-d-spec-opus-dom-points",
            "d_signals_opus_dom",
            "counts",
            "How many points does the absorbance spectrum in this OPUS file have?",
            lambda o, m: g.integer(tr(o)["sample_count"]),
            "oracle: /traces/0/sample_count (brukeropus block AB)",
        ),
        spec(
            "ho-d-spec-omnic-points",
            "d_signals_spa",
            "counts",
            "How many data points does this Raman spectrum have?",
            lambda o, m: g.integer(tr(o)["sample_count"]),
            "oracle: /traces/0/sample_count (SpectroChemPy)",
        ),
        spec(
            "ho-d-spec-sp-instrument",
            "d_signals_sp",
            "instrument",
            "Which instrument model recorded this UV-Vis spectrum?",
            lambda o, m: g.string(extra(o, "instrument"), accept=["PerkinElmer Lambda 25", "Lambda25"]),
            "oracle: /traces/0/parameters/extra/instrument (specio)",
        ),
        spec(
            "ho-d-spec-sp-absorbance",
            "d_signals_sp",
            "values",
            "What is the absorbance of this sample at 562 nm?",
            fact("absorbance_562", lambda v: g.number(v, None, abs_=0.002)),
            "facts-heldout: absorbance_562 (the depositor's CSV export; specio agrees)",
            answer_hint="a number (absorbance)",
        ),
        spec(
            "ho-d-spec-spc-points",
            "d_signals_spc",
            "counts",
            "How many data points does this infrared spectrum have?",
            lambda o, m: g.integer(tr(o)["sample_count"]),
            "oracle: /traces/0/sample_count (SpectroChemPy; spc-io agrees)",
        ),
        spec(
            "ho-d-spec-witec-maps",
            "d_signals_wip_zncds",
            "counts",
            "This WITec project holds several Raman maps. How many maps does it hold, and how many spectra does "
            "each map contain?",
            lambda o, m: g.items([str(o["trace_count"]), str(o["traces"][0]["sweeps"])], ordered=True),
            "oracle: /trace_count and /traces/0/sweeps (witio)",
            answer_hint="the number of maps, then the spectra per map",
        ),
        spec(
            "ho-d-spec-witec-spectra",
            "d_signals_wip_bone",
            "counts",
            "How many Raman spectra does this WITec project file contain?",
            lambda o, m: g.integer(o["trace_count"]),
            "oracle: /trace_count (witio: one single spectrum per graph)",
        ),
        spec(
            "ho-d-spec-jws-points",
            "d_signals_jws",
            "counts",
            "How many data points does this UV-Vis spectrum have?",
            lambda o, m: g.integer(o["jws2txt"]["x_count"]),
            "oracle: /jws2txt/x_count (jws2txt, MIT)",
        ),
        # ------------------------------------------------ spectroscopy: analysis
        spec(
            "ho-d-ana-spec-opus-ice-ch",
            "d_signals_opus_ice",
            "analysis",
            "In this infrared spectrum of a methanol-methylamine ice, at what wavenumber is the strongest "
            "absorbance between 2700 and 3100 cm-1?",
            fact("ch_stretch_band", lambda v: g.number(v, "cm-1", abs_=4.0)),
            "facts-heldout: ch_stretch_band (brukeropus)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-spec-opus-dom-band",
            "d_signals_opus_dom",
            "analysis",
            "In this FTIR spectrum of dissolved organic matter, at what wavenumber is the strongest absorbance "
            "between 1500 and 1800 cm-1?",
            fact("band_1500_1800", lambda v: g.number(v, "cm-1", abs_=4.0)),
            "facts-heldout: band_1500_1800 (brukeropus)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-spec-spc-band",
            "d_signals_spc",
            "analysis",
            "At what wavenumber does this infrared spectrum of gypsum show its strongest absorbance between 900 "
            "and 1200 cm-1?",
            fact("band_900_1200", lambda v: g.number(v, "cm-1", abs_=4.0)),
            "facts-heldout: band_900_1200 (SpectroChemPy; spc-io agrees)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-spec-jws-peak",
            "d_signals_jws",
            "analysis",
            "This is the absorbance spectrum of a protein-assay standard (0.80 mg/ml BSA). At what wavelength "
            "between 500 and 700 nm is the absorbance highest?",
            fact("peak_nm_500_700", lambda v: g.number(v, "nm", abs_=5.0)),
            "facts-heldout: peak_nm_500_700 (jws2txt, MIT)",
            answer_hint=unit_hint,
        ),
    ]
