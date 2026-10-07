"""Electrophysiology ground truth for `analyze ephys-features` and `analyze spikes` (corpus/oracle/ephys-analysis/*.json).

- Current clamp (ABF): samples and the command epochs are read with pyABF (MIT); features are
  computed by eFEL (LGPL-3.0, run as a black box only) with its defaults (voltage threshold
  -20 mV, DerivativeThreshold 10 V/s) on every sweep, with stim_start/stim_end = the stimulus
  epoch (the first step epoch whose level changes between sweeps, as pyABF's sweepEpochs lists
  it). Cell values are derived from eFEL's per-sweep numbers: rheobase = smallest positive step
  with spike_count_stimint >= 1; input resistance = least-squares slope of
  (steady_state_voltage_stimend - voltage_base) against the step over spike-free negative steps;
  tau = median eFEL time_constant over those steps; sag = eFEL sag_ratio1 of the most negative.
- Current clamp (NWB, `NWB_CURRENT_CLAMP`): every `CurrentClampSeries` under `/acquisition`, its
  samples read with h5py (data x conversion + offset, in volts, shown in mV), its rate from
  `starting_time@rate`; eFEL's spike_count, peak_time, peak_voltage and AP_begin_voltage over the
  whole sweep (NWB carries no epoch table, so there is no stimulus window).
- Voltage clamp membrane tests (ABF): pyABF's own memtest (pyabf.tools.memtest: Ih, Ra, Rm, Cm).
- Extracellular: samples read with Neo (BSD-3; the first segment only), band-pass filtered by SpikeInterface
  (bandpass_filter, 300-6000 Hz, order 5 Butterworth, forward-backward), noise = median(|x|)/0.6745
  of the whole filtered channel, peaks by SpikeInterface detect_peaks (method by_channel,
  peak_sign neg, detect_threshold 5, exclude_sweep_ms 0.1).

Usage: cd oracle/signal && uv run python ephys.py
"""
import json
import os
import sys
import warnings

import numpy as np

warnings.filterwarnings("ignore")

CORPUS = os.environ.get("OPENREADOUT_CORPUS_DIR", os.path.join(os.path.dirname(__file__), "..", "..", "corpus", "files"))
OUT = os.path.join(os.path.dirname(__file__), "..", "..", "corpus", "oracle", "ephys-analysis")

CURRENT_CLAMP = ["pyabf-171116sh-0018", "pyabf-2019-07-24-0055-fsi", "pyabf-190619b-0003"]
NWB_CURRENT_CLAMP = ["dandi001544-icephys-cc328"]
MEMTEST = ["pyabf-18808025-memtest", "pyabf-171116sh-0011"]
EXTRACELLULAR = [
    # (id, file, neo io, stream/notes)
    ("brk-filespec2-3001", "brk-filespec2-3001.ns5", "blackrock"),
    ("intan-test-tetrode-163225", "intan-test-tetrode-163225.rhd", "intan"),
    ("nlx-cheetah-v5-5-1-tet3a", "nlx-cheetah-v5-5-1-tet3a.ncs", "neuralynx"),
]

EFEL_FEATURES = [
    "spike_count", "spike_count_stimint", "peak_time", "peak_voltage", "AP_begin_voltage",
    "AP_amplitude", "AP_duration_half_width", "AHP_depth_abs", "ISI_values", "time_to_first_spike",
    "adaptation_index2", "voltage_base", "steady_state_voltage_stimend", "minimum_voltage",
    "sag_ratio1", "sag_ratio2", "time_constant", "mean_frequency",
]


def clean(v):
    if v is None:
        return None
    a = np.asarray(v, dtype=float).ravel()
    return [None if not np.isfinite(x) else float(x) for x in a]


def stimulus_epoch(abf):
    """Index into sweepEpochs of the first Step epoch whose level changes between sweeps."""
    levels = []
    for s in abf.sweepList:
        abf.setSweep(s)
        levels.append(list(abf.sweepEpochs.levels))
    for i, t in enumerate(abf.sweepEpochs.types):
        if t != "Step":
            continue
        if len({round(lv[i], 9) for lv in levels}) > 1:
            return i
    return None


def current_clamp(eid):
    import efel
    import pyabf
    abf = pyabf.ABF(os.path.join(CORPUS, eid + ".abf"))
    k = stimulus_epoch(abf)
    sweeps = []
    for s in abf.sweepList:
        abf.setSweep(s, channel=0)
        t = abf.sweepX * 1000.0
        v = abf.sweepY
        p1, p2 = abf.sweepEpochs.p1s[k], abf.sweepEpochs.p2s[k]
        level = abf.sweepEpochs.levels[k]
        start, end = p1 / abf.dataRate * 1000.0, p2 / abf.dataRate * 1000.0
        trace = {"T": t, "V": v, "stim_start": [start], "stim_end": [end]}
        res = efel.get_feature_values([trace], EFEL_FEATURES, raise_warnings=False)[0]
        row = {"sweep": s, "stim_start_ms": start, "stim_end_ms": end, "current_pa": float(level)}
        for f in EFEL_FEATURES:
            row[f] = clean(res.get(f))
        sweeps.append(row)
    one = lambda r, f: (r[f] or [None])[0]
    fire = [r for r in sweeps if r["current_pa"] > 0 and (one(r, "spike_count_stimint") or 0) >= 1]
    rheobase = min((r["current_pa"] for r in fire), default=None)
    neg = [r for r in sweeps if r["current_pa"] < 0 and (one(r, "spike_count_stimint") or 0) == 0
           and one(r, "voltage_base") is not None and one(r, "steady_state_voltage_stimend") is not None]
    rin = None
    if len(neg) >= 2:
        x = np.array([r["current_pa"] for r in neg])
        y = np.array([one(r, "steady_state_voltage_stimend") - one(r, "voltage_base") for r in neg])
        rin = float(np.polyfit(x, y, 1)[0] * 1000.0)
    taus = [one(r, "time_constant") for r in neg if one(r, "time_constant") is not None]
    most = min(neg, key=lambda r: r["current_pa"]) if neg else None
    return {
        "id": eid,
        "tool": "pyABF %s + eFEL %s" % (pyabf.__version__, getattr(efel, "__version__", "?")),
        "stimulus_epoch": k,
        "sweeps": sweeps,
        "cell": {
            "rheobase_pa": rheobase,
            "input_resistance_mohm": rin,
            "tau_ms": float(np.median(taus)) if taus else None,
            "sag_ratio": one(most, "sag_ratio1") if most else None,
        },
    }


def nwb_current_clamp(eid):
    import efel
    import h5py
    series = []
    with h5py.File(os.path.join(CORPUS, eid + ".nwb"), "r") as f:
        for name in sorted(f["acquisition"]):
            g = f["acquisition"][name]
            nt = g.attrs.get("neurodata_type")
            nt = nt.decode() if isinstance(nt, bytes) else nt
            if nt != "CurrentClampSeries":
                continue
            d = g["data"]
            conv = float(d.attrs.get("conversion", 1.0))
            off = float(d.attrs.get("offset", 0.0))
            v = (np.asarray(d[()], dtype=float) * conv + off) * 1000.0
            fs = float(g["starting_time"].attrs["rate"])
            t = np.arange(v.size) / fs * 1000.0
            trace = {"T": t, "V": v, "stim_start": [0.0], "stim_end": [float(t[-1])]}
            feats = ["spike_count", "peak_time", "peak_voltage", "AP_begin_voltage"]
            res = efel.get_feature_values([trace], feats, raise_warnings=False)[0]
            row = {"name": name, "sample_rate_hz": fs, "samples": int(v.size)}
            for k in feats:
                row[k] = clean(res.get(k))
            series.append(row)
    return {"id": eid, "tool": "h5py %s + eFEL %s" % (h5py.__version__, getattr(efel, "__version__", "?")),
            "series": series}


def memtest(eid):
    import pyabf
    import pyabf.tools.memtest
    abf = pyabf.ABF(os.path.join(CORPUS, eid + ".abf"))
    mt = pyabf.tools.memtest.Memtest(abf)
    return {
        "id": eid,
        "tool": "pyABF %s memtest" % pyabf.__version__,
        "Ih_pa": clean(mt.Ih.values), "Ra_mohm": clean(mt.Ra.values),
        "Rm_mohm": clean(mt.Rm.values), "Cm_step_pf": clean(mt.CmStep.values),
    }


def read_neo(path, kind):
    import neo
    if kind == "blackrock":
        r = neo.rawio.BlackrockRawIO(filename=path[:-4], nsx_to_load=5)
    elif kind == "intan":
        r = neo.rawio.IntanRawIO(filename=path)
    else:
        r = neo.rawio.NeuralynxRawIO(dirname=os.path.dirname(path), include_filenames=[os.path.basename(path)])
    r.parse_header()
    streams = r.header["signal_streams"]
    # the broadband stream: highest sampling rate with the most channels
    best = None
    for si in range(len(streams)):
        ch = r.header["signal_channels"][r.header["signal_channels"]["stream_id"] == streams[si]["id"]]
        fs = float(ch["sampling_rate"][0])
        if best is None or (fs, len(ch)) > best[1:]:
            best = (si, fs, len(ch))
    si, fs, _ = best
    raw = r.get_analogsignal_chunk(block_index=0, seg_index=0, stream_index=si)
    x = r.rescale_signal_raw_to_float(raw, dtype="float64", stream_index=si)
    return x, fs, streams[si]["name"]


def extracellular(eid, fname, kind):
    import spikeinterface.core as sc
    import spikeinterface.preprocessing as spre
    from spikeinterface.sortingcomponents.peak_detection import detect_peaks
    x, fs, stream = read_neo(os.path.join(CORPUS, fname), kind)
    rec = sc.NumpyRecording([x.astype("float64")], sampling_frequency=fs)
    rf = spre.bandpass_filter(rec, freq_min=300.0, freq_max=min(6000.0, 0.45 * fs), filter_order=5, dtype="float64")
    y = rf.get_traces()
    noise = np.median(np.abs(y), axis=0) / 0.6745
    peaks = detect_peaks(rf, method="by_channel", method_kwargs=dict(peak_sign="neg", detect_threshold=5, exclude_sweep_ms=0.1, noise_levels=noise), job_kwargs=dict(n_jobs=1, progress_bar=False))
    chans = []
    for c in range(x.shape[1]):
        idx = np.sort(peaks["sample_index"][peaks["channel_index"] == c])
        chans.append({"channel": c, "noise": float(noise[c]), "count": int(len(idx)), "samples": [int(i) for i in idx[:2000]]})
    import spikeinterface
    return {"id": eid, "file": fname, "stream": stream, "sample_rate_hz": fs, "samples": int(x.shape[0]),
            "tool": "Neo + SpikeInterface %s" % spikeinterface.__version__, "channels": chans}


def main():
    os.makedirs(OUT, exist_ok=True)
    only = set(sys.argv[1:])
    for eid in CURRENT_CLAMP:
        if only and eid not in only:
            continue
        o = current_clamp(eid)
        json.dump(o, open(os.path.join(OUT, eid + ".json"), "w"), indent=1)
        print(eid, "stimulus epoch", o["stimulus_epoch"], "cell", o["cell"],
              "counts", [(r["spike_count"] or [None])[0] for r in o["sweeps"]])
    for eid in NWB_CURRENT_CLAMP:
        if only and eid not in only:
            continue
        o = nwb_current_clamp(eid)
        json.dump(o, open(os.path.join(OUT, eid + ".nwb-cc.json"), "w"), indent=1)
        print(eid, len(o["series"]), "series, counts", [(r["spike_count"] or [None])[0] for r in o["series"]])
    for eid in MEMTEST:
        if only and eid not in only:
            continue
        o = memtest(eid)
        json.dump(o, open(os.path.join(OUT, eid + ".memtest.json"), "w"), indent=1)
        print(eid, "Ra", o["Ra_mohm"][:3], "Rm", o["Rm_mohm"][:3], "Cm", o["Cm_step_pf"][:3])
    for eid, fname, kind in EXTRACELLULAR:
        if only and eid not in only:
            continue
        try:
            o = extracellular(eid, fname, kind)
        except Exception as e:  # report and continue; the test skips missing oracles
            print(eid, "FAILED", repr(e))
            continue
        json.dump(o, open(os.path.join(OUT, eid + ".spikes.json"), "w"), indent=1)
        print(eid, o["stream"], o["sample_rate_hz"], "counts", [c["count"] for c in o["channels"]])


if __name__ == "__main__":
    main()
