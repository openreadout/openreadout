"""Electrophysiology: a second reading of each recording's structure, metadata and samples.

- Axon ABF: Neo's AxonRawIO (BSD-3; the primary oracle is pyABF): sweeps, sample rate, channel
  names, units and scaling, the recording start, and every channel's samples in up to three
  sweeps (whole-sweep statistics and the first samples).
- Axon ATF: the text table read with Python's csv module (the primary oracle is pyABF).
- NWB: pynwb (BSD-3; the primary oracle is h5py): session start (with its zone), identifier,
  description, experimenter, institution and lab, and each acquisition TimeSeries' rate, unit,
  length and samples.
- Neuralynx .ncs: nept (MIT; the primary oracle is Neo): header (sampling rate, ADBitVolts,
  channel name) and the continuous samples.
- SpikeGLX .bin + .meta: the .meta text and the int16 samples read with NumPy, scaled to volts
  from the meta's ranges and gains as SpikeGLX's published metadata documentation describes
  (the primary oracle is Neo).
"""
from __future__ import annotations

import csv
import re
from importlib.metadata import version
from pathlib import Path

import numpy as np

from . import Rec, trace_values

FAMILY = "electrophysiology"
FORMATS = {"abf", "atf", "nwb", "neuralynx", "spikeglx", "blackrock", "plexon"}


def sweeps_to_check(n: int) -> list[int]:
    return sorted({0, n // 2, n - 1}) if n > 0 else []


def abf(rec: Rec, path: Path) -> None:
    import neo
    r = rec.reader(f"Neo {version('neo')} AxonRawIO (BSD-3)")
    io = neo.rawio.AxonRawIO(str(path))
    io.parse_header()
    sig = io.header["signal_channels"]
    nseg = int(io.header["nb_segment"][0])
    rate = float(io.get_signal_sampling_rate(0))
    lengths = [int(io.get_signal_size(0, s, 0)) for s in range(nseg)]
    rec.check("sweep count", "/traces/0/sweep_count", nseg, reader=r, required=True)
    rec.check("sample rate", "/traces/0/sample_rate_hz", rate, cmp="num", rel=1e-9, reader=r)
    if len(set(lengths)) == 1:
        rec.check("samples per sweep", "/traces/0/sample_count", lengths[0], reader=r)
    else:  # variable-length (event-driven) sweeps
        rec.check("samples of each sweep", "/traces/0/extra/sweep_sample_counts", lengths, reader=r)
    # Neo re-spells names and units (drops spaces, `uA` for `µA`): compared letters and digits only
    names = {str(i): str(s["name"]) for i, s in enumerate(sig) if str(s["name"]).strip()}
    kw = dict(by="/index", reader=r)
    rec.check("channel names", "/traces/0/channels", names, each="/name", cmp="loose", **kw)
    rec.check("channel units", "/traces/0/channels", {str(i): str(s["units"]) for i, s in enumerate(sig)},
              each="/unit", cmp="loose", **kw)
    rec.check("channel scale", "/traces/0/channels/*/scale", [float(s["gain"]) for s in sig], cmp="num", rel=1e-6, reader=r)
    rec.check("channel offset", "/traces/0/channels/*/offset", [float(s["offset"]) for s in sig], cmp="num", rel=1e-6, abs=1e-9, reader=r)
    dt = io.raw_annotations["blocks"][0].get("rec_datetime") or getattr(io, "_axon_info", {}).get("rec_datetime")
    if dt is not None and dt.year > 1900:  # Neo gives 1900-01-01 when it reads no date (ABF 1)
        rec.check("recording start", "/experiment/acquisition/started_at", dt.isoformat(), cmp="time", abs=0.002, reader=r)
    else:
        import pyabf
        try:
            adt = getattr(pyabf.ABF(str(path), loadData=False), "abfDateTime", None)
        except ValueError:  # pyABF refuses an impossible stored time (the file's own)
            adt = None
        if adt is not None and adt.year > 1980:
            rp = rec.reader(f"pyABF {version('pyabf')} (MIT) abfDateTime")
            rec.check("recording start (pyABF)", "/experiment/acquisition/started_at", adt.isoformat(), cmp="time",
                      abs=0.002, reader=rp)
    for s in sweeps_to_check(nseg):
        raw = io.get_analogsignal_chunk(block_index=0, seg_index=s, stream_index=0)
        vals = io.rescale_signal_raw_to_float(raw, dtype="float64", stream_index=0)
        for c in range(vals.shape[1]):
            trace_values(rec, 0, s, c, vals[:, c], r)


def atf(rec: Rec, path: Path) -> None:
    """ATF 1.0: line 1 'ATF\t1.0', line 2 '<header lines>\t<columns>', header records, a column
    title row, then tab-separated numbers. Columns are 'Time (s)' then one per signal and sweep
    ('Trace #1 (pA)', ...)."""
    r = rec.reader("Python csv module on the ATF text (Axon Text File layout)")
    lines = path.read_text(encoding="latin-1").splitlines()
    nh = int(lines[1].split()[0])
    header = {}
    for l in lines[2:2 + nh]:
        m = re.match(r'"([^=]+)=(.*)"', l.strip())
        if m:
            header[m.group(1).strip()] = m.group(2).strip()
    titles = next(csv.reader([lines[2 + nh]], delimiter="\t"))
    rows = [[float(x) for x in row] for row in csv.reader(lines[3 + nh:], delimiter="\t") if row and row[0].strip()]
    data = np.array(rows)
    t = data[:, 0]
    rate = 1.0 / float(np.median(np.diff(t))) if len(t) > 1 else None
    rec.check("samples per sweep", "/traces/0/sample_count", data.shape[0], reader=r)
    if rate:
        rec.check("sample rate", "/traces/0/sample_rate_hz", rate, cmp="num", rel=1e-6, reader=r)
    signals = header.get("SignalsExported", "")
    nsig = len([s for s in signals.split(",") if s]) or 1
    sweeps = (data.shape[1] - 1) // nsig
    rec.check("sweep count", "/traces/0/sweep_count", sweeps, reader=r)
    units = []
    for ti in titles[1:1 + nsig]:
        m = re.search(r"\(([^)]*)\)\s*$", ti)
        units.append(m.group(1) if m else None)
    if all(units):
        rec.check("channel units", "/traces/0/channels/*/unit", units, cmp="text", reader=r)
    for s in sweeps_to_check(sweeps):
        for c in range(nsig):
            trace_values(rec, 0, s, c, data[:, 1 + s * nsig + c], r)


def nwb(rec: Rec, path: Path) -> None:
    import pynwb
    r = rec.reader(f"pynwb {version('pynwb')} (BSD-3)")
    with pynwb.NWBHDF5IO(str(path), "r", load_namespaces=True) as io:
        f = io.read()
        rec.check("session start", "/experiment/acquisition/started_at", f.session_start_time.isoformat(),
                  cmp="time", zone=True, reader=r)
        rec.check("session description", "/experiment/acquisition/comment", f.session_description, cmp="text", reader=r)
        exp = list(f.experimenter or [])
        if exp:
            rec.check("experimenter (all, joined)", "/experiment/acquisition/operator", ", ".join(x.decode() if isinstance(x, bytes) else str(x) for x in exp), cmp="text", reader=r)
        if f.subject is not None:
            rec.check("subject id", "/experiment/sample/id", f.subject.subject_id, cmp="text", reader=r)
        series = []
        for name, obj in f.acquisition.items():
            if isinstance(obj, pynwb.base.TimeSeries):
                series.append(obj)
            elif hasattr(obj, "time_series"):
                series.extend(obj.time_series.values())
        for ts in series:
            where = f"/traces/[name={ts.name}]"
            data = ts.data
            n = int(data.shape[0])
            rec.check(f"{ts.name}: samples", f"{where}/sample_count", n, reader=r)
            if ts.rate is not None:
                rec.check(f"{ts.name}: rate", f"{where}/sample_rate_hz", float(ts.rate), cmp="num", rel=1e-9, reader=r)
            rec.check(f"{ts.name}: unit", f"{where}/channels/[name={ts.name}]/unit", ts.unit, cmp="text", reader=r)


NLX_SUFFIXES = {".ncs", ".nev", ".nse", ".ntt", ".nst", ".nvt"}


def nlx_opened(rec: Rec, path: Path) -> None:
    """The header's `## Time Opened` (or `-TimeCreated`), read by Neo's Neuralynx header parser;
    a session directory's files are compared only when they share one time."""
    from neo.rawio.neuralynxrawio.nlxheader import NlxHeader
    files = sorted(f for f in path.iterdir() if f.suffix.lower() in NLX_SUFFIXES) if path.is_dir() else [path]
    opened = set()
    for f in files:
        try:
            t = NlxHeader(str(f)).get("recording_opened")
        except Exception:  # Neo refuses some headers; the file is then not compared
            return
        if t is not None:
            opened.add(t)
    if len(opened) == 1:
        r = rec.reader(f"Neo {version('neo')} (BSD-3) NlxHeader: `## Time Opened` / `-TimeCreated`")
        rec.check("header time opened", "/experiment/acquisition/started_at", next(iter(opened)).isoformat(),
                  cmp="time_or_utc", abs=0.002, reader=r)


def neo_rec_datetime(rec: Rec, path: Path, cls: str, **kw) -> None:
    """Neo's block `rec_datetime` (the recorder's header start time) for one file."""
    import contextlib
    import io as _io
    import warnings
    import neo
    warnings.filterwarnings("ignore")
    with contextlib.redirect_stderr(_io.StringIO()):
        io = getattr(neo.rawio, cls)(**kw)
        io.parse_header()
    dt = io.raw_annotations["blocks"][0].get("rec_datetime")
    if dt is not None:
        r = rec.reader(f"Neo {version('neo')} (BSD-3) {cls}: the block's rec_datetime")
        rec.check("recording start (rec_datetime)", "/experiment/acquisition/started_at", dt.isoformat(),
                  cmp="time_or_utc", abs=0.002, reader=r)


def blackrock(rec: Rec, path: Path) -> None:
    if path.is_dir():
        return
    neo_rec_datetime(rec, path, "BlackrockRawIO", filename=str(path.with_suffix("")))


def plexon(rec: Rec, path: Path) -> None:
    if path.suffix.lower() == ".plx":
        neo_rec_datetime(rec, path, "PlexonRawIO", filename=str(path))


def neuralynx(rec: Rec, path: Path) -> None:
    nlx_opened(rec, path)
    if path.suffix.lower() != ".ncs":
        return
    try:
        ncs_values(rec, path)
    except IndexError as e:  # nept cannot read some truncated files; the header check stands
        rec.note(f"nept: {type(e).__name__}: {e}")


def ncs_values(rec: Rec, path: Path) -> None:
    import nept
    r = rec.reader(f"nept {version('nept')} (MIT) load_lfp / load_neuralynx_header")
    with path.open("rb") as f:
        hdr = f.read(16384)
    text = hdr.decode("latin-1", "replace").rstrip("\x00")
    kv = {}
    for line in text.splitlines():
        m = re.match(r"\s*-(\S+)\s+(.*)$", line)
        if m:
            kv[m.group(1)] = m.group(2).strip()
    lfp = nept.load_lfp(str(path))
    vals = np.asarray(lfp.data).reshape(-1) * 1e6  # nept: volts; compared in microvolts
    t = np.asarray(lfp.time).reshape(-1)
    fs = float(kv.get("SamplingFrequency", 0) or 0)
    if fs:
        rec.check("sample rate (-SamplingFrequency)", "/traces/0/sample_rate_hz", fs, cmp="num", rel=1e-9, reader=r)
    if "AcqEntName" in kv:
        rec.check("channel name (-AcqEntName)", "/traces/0/channels/0/name", kv["AcqEntName"], cmp="text", reader=r)
    rec.check("channel unit", "/traces/0/channels/0/unit", "µV", cmp="loose", reader=r)
    # a gap in the record time stamps starts a new segment (sweep), as in Neo and OpenReadout
    cuts = [0]
    if fs and t.size > 1:
        cuts += [int(i) + 1 for i in np.nonzero(np.diff(t) > 1.5 / fs)[0]]
    cuts.append(vals.size)
    rec.check("segments (time-stamp gaps)", "/traces/0/sweep_count", len(cuts) - 1, reader=r)
    trace_values(rec, 0, 0, 0, vals[cuts[0]:cuts[1]], r, rel=1e-9)


def spikeglx(rec: Rec, path: Path) -> None:
    """Scaling per SpikeGLX's metadata documentation: imec probes int16 × imAiRangeMax /
    imMaxInt / gain (gain from imroTbl: AP or LF gain per channel for NP1.0, 80 for NP2.0);
    NI-DAQ int16 × niAiRangeMax / 32768 / gain (MN, MA, XA, DW)."""
    if path.is_dir() or path.suffix != ".bin":
        return
    meta_p = path.with_suffix(".meta")
    meta = {}
    for line in meta_p.read_text(encoding="latin-1").splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            meta[k.lstrip("~")] = v.strip()
    r = rec.reader("the .meta text and NumPy int16 samples (SpikeGLX metadata documentation)")
    nchan = int(meta["nSavedChans"])
    rate = float(meta.get("imSampRate") or meta.get("niSampRate"))
    size = path.stat().st_size
    n = size // (2 * nchan)
    rec.check("saved channels (nSavedChans)", "/traces/0/channels#len", nchan, reader=r)
    rec.check("sample rate", "/traces/0/sample_rate_hz", rate, cmp="num", rel=1e-9, reader=r)
    rec.check("samples", "/traces/0/sample_count", n, reader=r)
    if meta.get("fileCreateTime"):
        rec.check("file created (fileCreateTime)", "/experiment/acquisition/started_at", meta["fileCreateTime"],
                  cmp="time", reader=r)
    scales = sglx_scales(meta, path.name, nchan)
    if scales is not None and meta.get("typeThis") == "imec":
        # neural (AP/LF) channels are compared in microvolts, NI-DAQ channels in volts
        scales = [None if s is None else s * 1e6 for s in scales]
        rec.check("neural channel unit", "/traces/0/channels/0/unit", "µV", cmp="loose", reader=r)
    elif scales is not None:
        rec.check("analog channel unit", "/traces/0/channels/0/unit", "V", cmp="loose", reader=r)
    if scales is not None:
        pick = [c for c in range(nchan) if scales[c] is not None]
        rec.check("channel scale (per count)", "/traces/0/channels", {str(c): scales[c] for c in pick},
                  by="/index", each="/scale", cmp="num", rel=1e-9, reader=r)
    raw = np.memmap(path, dtype="<i2", mode="r", shape=(n, nchan))
    k = min(n, 4096)
    first = np.asarray(raw[:k], dtype=np.float64)
    for c in spread_channels(nchan):
        s = scales[c] if scales is not None and scales[c] is not None else None
        if s is None:
            continue
        trace_values(rec, 0, 0, c, first[:, c] * s, r, n_samples=64, stats=False, count=False)


def spread_channels(n: int, k: int = 6) -> list[int]:
    return sorted({round(i * (n - 1) / (k - 1)) for i in range(k)}) if n > k else list(range(n))


def saved_channels(meta: dict, nchan: int) -> list[int]:
    """Acquired channel index of each saved channel (snsSaveChanSubset: 'all' or 'a:b,c,...')."""
    sub = meta.get("snsSaveChanSubset", "all")
    if sub == "all":
        return list(range(nchan))
    out = []
    for part in sub.split(","):
        if ":" in part:
            a, b = part.split(":")
            out.extend(range(int(a), int(b) + 1))
        elif part.strip():
            out.append(int(part))
    return out


def sglx_scales(meta: dict, name: str, nchan: int) -> list | None:
    """Volts per count of each saved channel; None for digital/sync channels."""
    acq = saved_channels(meta, nchan)
    if len(acq) != nchan:
        return None
    if meta.get("typeThis") == "nidq":
        mn, ma, xa, dw = (int(x) for x in meta["acqMnMaXaDw"].split(","))
        rng = float(meta["niAiRangeMax"])
        maxint = float(meta.get("niMaxInt", 32768))
        out = []
        for a in acq:
            if a < mn:
                out.append(rng / maxint / float(meta["niMNGain"]))
            elif a < mn + ma:
                out.append(rng / maxint / float(meta["niMAGain"]))
            elif a < mn + ma + xa:
                out.append(rng / maxint)
            else:
                out.append(None)
        return out
    ap, lf, sy = (int(x) for x in meta["acqApLfSy"].split(","))
    rng = float(meta.get("imAiRangeMax", 0.6))
    ptype = int(meta.get("imDatPrb_type", 0))
    maxint = float(meta.get("imMaxInt", 8192 if ptype in (21, 24, 2003, 2004, 2013, 2014) else 512))
    entries = re.findall(r"\(([^()]*)\)", meta.get("imroTbl", ""))[1:]
    is_lf = ".lf." in name
    out = []
    for a in acq:
        if a >= ap + lf:
            out.append(None)  # SY: the sync word
            continue
        ch = a - ap if a >= ap else a
        lf_chan = is_lf or a >= ap
        if meta.get("imChan0lfGain" if lf_chan else "imChan0apGain"):
            # newer metadata states the (fixed) gain of the probe's channels
            gain = float(meta["imChan0lfGain" if lf_chan else "imChan0apGain"])
        elif ptype in (21, 24, 2003, 2004, 2013, 2014):
            gain = 80.0
        else:
            try:
                f = entries[ch].split()
                gain = float(f[4] if (is_lf or a >= ap) else f[3])
            except (IndexError, ValueError):
                return None
        out.append(rng / maxint / gain)
    return out


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    fmt = entry["format"]
    {"abf": abf, "atf": atf, "nwb": nwb, "neuralynx": neuralynx, "spikeglx": spikeglx, "blackrock": blackrock,
     "plexon": plexon}[fmt](rec, path)
