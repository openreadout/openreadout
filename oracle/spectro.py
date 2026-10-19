"""Vibrational-spectroscopy oracles (imported by gen.py): Bruker OPUS, Thermo OMNIC .spa/.spg,
Renishaw WiRE .wdf, PerkinElmer .sp. Every reader is run as a black box and its values are
written in the corpus harness's trace layout (`traces[]`, one per openreadout trace, `sweeps`
with xxh3-128 over the little-endian float64 values of each channel and the first values), plus
`x_first`/`x_last`/`x_quantity` for the axis and `parameters.extra` for metadata the reader
decodes (compared with openreadout's `traces[].extra` by the harness).

Readers:
  OPUS  -> brukeropus (MIT): data blocks of `OPUSFile(debug=True)` decoded with brukeropus's own
           block parser, times CSF in float64, cut to NPT (the last NPT values of compact blocks);
           brukeropusreader (GPL-3.0, black box) as a second opinion on the result spectrum.
  OMNIC -> SpectroChemPy (CeCILL-B): read_omnic (spa, spg).
  WDF   -> renishawWiRE (MIT): spectra, x list, origin lists, map shape, laser wavelength.
  .sp   -> specio (BSD-3) when installed, else SpectroChemPy is not used (it has no .sp reader).

Trace order follows openreadout's documented order (docs/formats/*.md): for OPUS result
spectra, then sample, then reference blocks; plain data before interferograms and phases; then
the extended type, derivative, part, and the later block (higher offset) first.
"""
import io, contextlib, re, warnings
from pathlib import Path

import numpy as np


def _hash(values) -> str:
    import xxhash
    return xxhash.xxh3_128_hexdigest(np.ascontiguousarray(np.asarray(values, dtype="<f8")).tobytes())


def _first(values, n=8):
    out = []
    for v in np.asarray(values, dtype=np.float64)[:n]:
        if not np.isfinite(v):
            break
        out.append(float(v))
    return out


def _sweeps(rows, max_sweeps):
    return [{"sweep": s, "sample_count": int(len(chans[0])),
             "channels": [{"xxh3": _hash(c), "first": _first(c)} for c in chans]}
            for s, chans in enumerate(rows[:max_sweeps])]


def kind(p: Path):
    """The oracle function name for a spectroscopy file (by its first bytes), else None."""
    try:
        with open(p, "rb") as f:
            head = f.read(18)
    except OSError:
        return None
    if head[:4] == b"\n\n\xfe\xfe":
        return "opus"
    if head in (b"Spectral Data File", b"Spectral Exte File"):
        return "omnic"
    if head[:4] == b"WDF1":
        return "wdf"
    if head[:4] == b"PEPE":
        return "pesp"
    if head[:3] == b"PE " and p.name.lower().endswith(".sp"):
        return "pesp_ascii"
    if _is_galactic_spc(head, p.name):
        return "spc"
    return None


def _is_galactic_spc(head: bytes, name: str) -> bool:
    return name.lower().endswith((".spc", ".spe")) and len(head) >= 2 and head[1] in (0x4B, 0x4D)


def _spc_io_primary(p: Path, head: bytes, max_sweeps, why: str) -> dict:
    """spc-io (MIT) as the SPC oracle where SpectroChemPy fails: y values of every subfile and
    the common x array (or the evenly spaced axis from the header)."""
    from spc_io import SPC
    with open(p, "rb") as f:
        s = SPC.from_bytes_io(f)
    rows = np.asarray([np.asarray(sub.yarray, dtype=np.float64) for sub in s], dtype=np.float64)
    xvals = head[0] & 0x80
    x = np.asarray(s.xarray, dtype=np.float64)
    chans = (lambda r: [x, r]) if xvals else (lambda r: [r])
    t = {"index": 0, "reader": "spc-io 0.2.1", "sweep_count": int(rows.shape[0]),
         "channel_count": 2 if xvals else 1, "sample_count": int(rows.shape[1]),
         "sweeps": _sweeps([chans(r) for r in rows], max_sweeps)}
    if not xvals:
        t["x_first"], t["x_last"] = float(x[0]), float(x[-1])
    return {"reader": "spc-io 0.2.1 (MIT)", "oracle_note": why, "traces": [t]}


def spc(p: Path, max_sweeps=1000) -> dict:
    """Galactic SPC via SpectroChemPy read_spc (CeCILL-B): one trace, one sweep per subfile, the
    x axis from its coordinate (a channel of its own when the file stores an x array); spc-io
    (MIT) is run as a recorded second opinion on the y values. SpectroChemPy scales single
    files by the main exponent, spc-io by the subfile exponent; where they differ the
    disagreement is recorded (and adjudicated)."""
    warnings.simplefilter("ignore")
    import spectrochempy as scp
    from spectrochempy.core.readers.read_spc import _SpcFile
    with open(p, "rb") as f:
        head = f.read(512)
    try:
        # SpectroChemPy's parser object (its public read_spc returns None when a text field
        # is not UTF-8, so the parser is called directly to get the error)
        sf = _SpcFile(p.read_bytes())
    except UnicodeDecodeError as e:
        return _spc_io_primary(p, head, max_sweeps,
                               f"SpectroChemPy {scp.__version__} cannot decode a text field ({e}); spc-io read the values")
    if len(sf.nds) != 1 and sf.format == "MXY":
        raise RuntimeError("per-subfile x arrays")
    rows = np.asarray([nd[1] for nd in sf.nds], dtype=np.float64)
    x = np.asarray(sf.nds[0][0], dtype=np.float64)
    xvals = head[1] == 0x4B and head[0] & 0x80
    chans = (lambda r: [x, r]) if xvals else (lambda r: [r])
    t = {"index": 0, "reader": f"spectrochempy {scp.__version__} read_spc", "sweep_count": int(rows.shape[0]),
         "channel_count": 2 if xvals else 1, "sample_count": int(rows.shape[1]),
         "sweeps": _sweeps([chans(r) for r in rows], max_sweeps)}
    if not xvals:
        t["x_first"], t["x_last"] = float(x[0]), float(x[-1])
    out = {"reader": f"spectrochempy {scp.__version__} (CeCILL-B)", "traces": [t]}
    try:
        from spc_io import SPC
        with open(p, "rb") as f:
            s = SPC.from_bytes_io(f)
        ys = np.asarray([np.asarray(sub.yarray, dtype=np.float64) for sub in s], dtype=np.float64)
        agree = ys.shape == rows.shape and bool(np.array_equal(ys, rows))
        out["second_opinion"] = {"reader": "spc-io 0.2.1 (MIT)", "values_agree": agree}
        if not agree and ys.shape == rows.shape:
            nz = rows != 0
            ratio = np.unique(np.round(ys[nz] / rows[nz], 12))
            out["second_opinion"]["ratio"] = [float(v) for v in ratio[:5]]
    except Exception as e:  # noqa: BLE001 - recorded
        out["second_opinion"] = {"reader": "spc-io 0.2.1 (MIT)", "error": f"{type(e).__name__}: {e}"}
    return out


def is_opus(p: Path) -> bool:
    try:
        with open(p, "rb") as f:
            return f.read(4) == b"\n\n\xfe\xfe"
    except OSError:
        return False


def _opus_sort_key(t, start):
    role = {3: 0, 1: 1, 2: 2}.get(t[1], 3)
    kind = {2: 2, 8: 2, 3: 3}.get(t[3] % 32, 1)
    return (role, kind, t[5], t[4], t[0], -start)


def opus(p: Path, max_sweeps=1000) -> dict:
    warnings.simplefilter("ignore")
    from brukeropus import OPUSFile
    from brukeropus.file.parse import parse_data
    f = OPUSFile(str(p), debug=True)
    items = []
    for key in f.all_data_keys:
        d = getattr(f, key)
        items.append((_opus_sort_key(tuple(d.block.type), d.block.start), key, d))
    items.sort(key=lambda x: x[0])
    traces = []
    for i, (_, key, d) in enumerate(items):
        ps = d.params
        npt = int(ps.npt)
        csf = float(getattr(ps, "csf", 1.0) or 1.0)
        t = {"index": i, "reader": "brukeropus", "brukeropus_key": key, "sweep_count": 1, "channel_count": 1,
             "sample_count": npt}
        if hasattr(d, "num_spectra"):  # series
            ys = np.asarray(d.y, dtype=np.float64)
            t.update(sweep_count=int(len(ys)), sweeps=_sweeps([[y] for y in ys], max_sweeps))
        else:
            raw = np.asarray(parse_data(d.block.bytes), dtype=np.float64)
            y = raw[-npt:] if d.block.is_compact_data() else raw[:npt]
            t["sweeps"] = _sweeps([[y * csf]], max_sweeps)
            # brukeropus's own y (float32 arithmetic) must agree to float32 rounding
            if not np.allclose(np.asarray(d.y, dtype=np.float64), y * csf, rtol=2e-7, atol=0):
                t["note"] = "brukeropus y differs from block values x CSF beyond float32 rounding"
        x = np.asarray(d.x, dtype=np.float64)
        t["x_first"], t["x_last"] = float(x[0]), float(x[-1])
        extra = {}
        for ours, attr in (("resolution_cm1", "res"), ("laser_wavenumber_cm1", "lwn"), ("apodization", "apf"),
                           ("beamsplitter", "bms"), ("source", "src"), ("detector", "dtc"), ("instrument", "ins"),
                           ("instrument_serial", "srn"), ("sample_name", "snm"), ("operator", "cnm"),
                           ("aperture", "apt"), ("accessory", "acc")):
            params = f.rf_params if d.block.type[1] == 2 else f.params
            v = params._params.get(attr) if hasattr(params, "_params") else None
            if v is None and d.block.type[1] == 2:
                v = f.params._params.get(attr)
            if isinstance(v, str):
                v = v.strip()
                if not v:
                    continue
            if v is not None:
                extra[ours] = v
        scans = (f.rf_params._params.get("nsr") or f.params._params.get("nsr")) if d.block.type[1] == 2 else f.params._params.get("nss")
        if scans is not None:
            extra["scans"] = int(scans)
        t["parameters"] = {"extra": extra}
        traces.append(t)
    out = {"reader": "brukeropus (MIT)", "traces": traces}
    # second opinion: brukeropusreader (GPL-3.0, black box) on the absorbance spectrum
    try:
        from brukeropusreader import read_file
        with contextlib.redirect_stdout(io.StringIO()):
            od = read_file(str(p))
        if "AB" in od:
            ab = np.asarray(od["AB"], dtype=np.float64)
            mine = next((t for t in traces if t["brukeropus_key"] == "a"), None)
            if mine is not None:
                n = mine["sample_count"]
                agree = len(ab) >= n and _hash(ab[:n]) == mine["sweeps"][0]["channels"][0]["xxh3"]
                out["second_opinion"] = {"reader": "brukeropusreader (GPL-3.0, black box)", "absorbance_agrees": bool(agree)}
    except Exception as e:  # recorded, not fatal
        out["second_opinion"] = {"reader": "brukeropusreader", "error": f"{type(e).__name__}: {e}"}
    return out


def is_omnic(p: Path) -> bool:
    try:
        with open(p, "rb") as f:
            return f.read(18) in (b"Spectral Data File", b"Spectral Exte File")
    except OSError:
        return False


def omnic(p: Path, max_sweeps=1000) -> dict:
    """SpectroChemPy read_omnic (CeCILL-B, black box). `.spa`: the spectrum, then the sample and
    background interferograms (read_spa(return_ifg=...)) when the file has them. `.spg`: one
    trace, one sweep per spectrum in file order (sortbydate=False)."""
    warnings.simplefilter("ignore")
    import spectrochempy as scp
    if p.suffix.lower() == ".srs":
        return omnic_series(p, max_sweeps)
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        d = scp.read_omnic(str(p), sortbydate=False)
    rows = np.asarray(d.data, dtype=np.float64)
    x = np.asarray(d.x.data, dtype=np.float64)
    meta = d.meta
    extra = {}
    for ours, key in (("scans", "sample_scans"), ("background_scans", "background_scans")):
        v = meta.get(key) if hasattr(meta, "get") else None
        if v is not None and int(v) > 0:
            extra[ours] = int(v)
    lf = meta.get("reference_frequency") if hasattr(meta, "get") else None
    if lf is None and hasattr(meta, "get"):
        lf = meta.get("laser_frequency")
    if lf is not None:
        extra["laser_wavenumber_cm1"] = float(getattr(lf, "magnitude", lf))
    t0 = {"index": 0, "reader": f"spectrochempy {scp.__version__} read_omnic", "sweep_count": int(rows.shape[0]),
          "channel_count": 1, "sample_count": int(rows.shape[1]), "x_first": float(x[0]), "x_last": float(x[-1]),
          "sweeps": _sweeps([[r] for r in rows], max_sweeps), "parameters": {"extra": extra},
          # SpectroChemPy rounds its linspace axis to 3 decimals, and some files' last x (a float32
          # in the header) comes out one rounding step away from SpectroChemPy's rebuilt axis
          "x_tolerance": 2e-3}
    if getattr(meta, "get", lambda k: None)("interferogram"):
        # SpectroChemPy gives interferograms an optical-path axis; openreadout keeps points
        del t0["x_first"], t0["x_last"]
    traces = [t0]
    if p.suffix.lower() == ".spa":
        for who in ("sample", "background"):
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                i = scp.read_spa(str(p), return_ifg=who)
            if i is None:
                continue
            y = np.asarray(i.data, dtype=np.float64)[0]
            traces.append({"index": len(traces), "reader": "spectrochempy read_spa(return_ifg)", "sweep_count": 1,
                           "channel_count": 1, "sample_count": int(len(y)), "x_first": 0.0, "x_last": float(len(y) - 1),
                           "sweeps": _sweeps([[y]], max_sweeps)})
    out = {"reader": f"spectrochempy {scp.__version__} (CeCILL-B)", "traces": traces}
    if d.acquisition_date is not None:
        out["acquisition_date"] = str(d.acquisition_date)
    return out


def omnic_series(p: Path, max_sweeps=1000) -> dict:
    """SpectroChemPy read_srs (CeCILL-B, black box): trace 0 the series (one sweep per spectrum),
    trace 1 the background (return_bg=True). SpectroChemPy presents spectra in descending
    wavenumber; they are put back in the ascending order they are stored in (and OpenReadout
    returns) before hashing, so each value keeps its x. Interferograms keep their order."""
    import spectrochempy as scp

    def ascending(ds):
        rows = np.asarray(ds.data, dtype=np.float64)
        if rows.ndim == 1:
            rows = rows[None, :]
        x = np.asarray(ds.x.data, dtype=np.float64)
        if len(x) > 1 and x[0] > x[-1]:
            x, rows = x[::-1], rows[:, ::-1]
        return rows, x

    def trace(k, rows, x, who, ifg):
        t = {"index": k, "reader": who, "sweep_count": int(rows.shape[0]), "channel_count": 1,
             "sample_count": int(rows.shape[1]), "sweeps": _sweeps([[r] for r in rows], max_sweeps)}
        if not ifg:
            t["x_first"], t["x_last"], t["x_tolerance"] = float(x[0]), float(x[-1]), 2e-3
        return t

    try:
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            d = scp.read_srs(str(p))
    except Exception as e:  # noqa: BLE001 - recorded as an oracle error, not hidden
        return {"reader": f"spectrochempy {scp.__version__} (CeCILL-B)",
                "traces": [{"index": 0, "error": f"read_srs: {e!r}"}]}
    rows, x = ascending(d)
    ifg = bool(getattr(d.meta, "get", lambda k: None)("interferogram"))
    traces = [trace(0, rows, x, f"spectrochempy {scp.__version__} read_srs", ifg)]
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        bg = scp.read_srs(str(p), return_bg=True)
    if bg is not None:
        rows, x = ascending(bg)
        bifg = bool(getattr(bg.meta, "get", lambda k: None)("interferogram"))
        traces.append(trace(1, rows, x, "spectrochempy read_srs(return_bg=True)", bifg))
        # SpectroChemPy returns the first background only; further ones (set-0 key-2 records of
        # the 22-byte key table at 304) are listed as traces it cannot compare
        import struct
        b = p.read_bytes()
        n = struct.unpack_from("<H", b, 294)[0]
        backgrounds = sum(1 for i in range(n) if 304 + 22 * i + 22 <= len(b)
                          and struct.unpack_from("<H", b, 304 + 22 * i)[0] == 2
                          and struct.unpack_from("<I", b, 304 + 22 * i + 14)[0] == 0)
        for k in range(1, backgrounds):
            traces.append({"index": len(traces), "reader": "not read by SpectroChemPy (a further background)"})
    out = {"reader": f"spectrochempy {scp.__version__} (CeCILL-B)", "traces": traces}
    if d.acquisition_date is not None:
        out["acquisition_date"] = str(d.acquisition_date)
    return out


def is_pesp(p: Path) -> bool:
    try:
        with open(p, "rb") as f:
            return f.read(4) == b"PEPE"
    except OSError:
        return False


def pesp(p: Path, max_sweeps=1000) -> dict:
    """specio (BSD-3, black box): the data member (float64) and the x range
    (linspace(first, last, n)). specio 0.1 imports `collections.Iterable`, removed in Python
    3.10: the ABCs are put back on `collections` before the import."""
    import collections, collections.abc
    for n in ("Iterable", "Mapping", "Sequence", "MutableMapping"):
        if not hasattr(collections, n):
            setattr(collections, n, getattr(collections.abc, n))
    from specio import specread
    import specio.plugins.sp as sp_plugin
    if not getattr(sp_plugin, "_lenient", False):
        # specio decodes the instrument texts as UTF-8 and fails on Latin-1 bytes (µ); only
        # that metadata step is made lenient, the data path is untouched
        strict = sp_plugin._decode_5104

        def lenient(data):
            try:
                return strict(data)
            except (UnicodeDecodeError, IndexError):
                return {}
        sp_plugin._decode_5104 = lenient
        sp_plugin._lenient = True
    try:
        s = specread(str(p))
    except UnicodeDecodeError:
        # specio decodes the fixed 40-byte description after `PEPE` as UTF-8; some files
        # (Spectrum 10) carry non-ASCII bytes after its NUL. Only those 40 description bytes are
        # made ASCII (the blocks start at byte 44 and are passed through unchanged).
        raw = p.read_bytes()
        fixed = raw[:4] + bytes(c if c < 0x80 else 0x20 for c in raw[4:44]) + raw[44:]
        f = io.BytesIO(fixed)
        f.name = p.name
        s = sp_plugin.SP.Reader._read_sp(f)
    y = np.asarray(s.amplitudes, dtype=np.float64).reshape(-1)
    x = np.asarray(s.wavelength, dtype=np.float64)
    t = {"index": 0, "reader": "specio 0.1.0", "sweep_count": 1, "channel_count": 1, "sample_count": int(len(y)),
         "x_first": float(x[0]), "x_last": float(x[-1]), "sweeps": _sweeps([[y]], max_sweeps)}
    # the instrument texts specio reads by position (absent when its UTF-8 decoding failed). The
    # infrared settings are taken only when the file holds their blocks (35840 scans, 35841
    # detector, 35842 source, 35843 beamsplitter, 35845 apodization: a u16 id, an i32 length, then
    # a 0x75xx member type): a UV-Vis file has none of them, and specio's positions then land on
    # other settings.
    raw = p.read_bytes()
    ir_settings = all(re.search(re.escape(i.to_bytes(2, "little")) + rb"[\x00-\xff]{4}[\x00-\xff]\x75", raw, re.S)
                      for i in (35840, 35841, 35842, 35843, 35845))
    extra = {}
    for ours, theirs in (("instrument", "instrument_model"), ("instrument_serial", "instrument_serial_number"),
                         ("scans", "accumulations"), ("detector", "detector"), ("source", "source"),
                         ("beamsplitter", "beam_splitter"), ("apodization", "apodization")):
        if ours in ("scans", "detector", "source", "beamsplitter", "apodization") and not ir_settings:
            continue
        v = s.meta.get(theirs)
        if isinstance(v, str):
            v = v.strip().lstrip("/").strip()
        if v not in (None, ""):
            extra[ours] = v
    if extra:
        t["parameters"] = {"extra": extra}
    return {"reader": "specio (BSD-3)", "traces": [t]}


def pesp_ascii(p: Path, max_sweeps=1000) -> dict:
    """A PerkinElmer `.sp` saved as text (`PE … ASCII PEDS`): the x and y pairs after `#DATA`,
    read with the standard library. No independent reader of this form exists, so this is a
    second implementation of the format note (`independent: false`)."""
    lines = p.read_bytes().decode("latin-1").splitlines()
    at = [l.strip() for l in lines].index("#DATA")
    pairs = [l.split() for l in lines[at + 1:] if l.strip() and not l.startswith("#")]
    x = np.asarray([float(a) for a, _ in pairs], dtype=np.float64)
    y = np.asarray([float(b) for _, b in pairs], dtype=np.float64)
    t = {"index": 0, "reader": "standard library (#DATA pairs)", "sweep_count": 1, "channel_count": 1,
         "sample_count": int(len(y)), "x_first": float(x[0]), "x_last": float(x[-1]),
         "sweeps": _sweeps([[y]], max_sweeps)}
    return {"reader": "second implementation (#DATA pairs read with the standard library); not an independent oracle",
            "independent": False, "traces": [t]}


def is_wdf(p: Path) -> bool:
    try:
        with open(p, "rb") as f:
            return f.read(4) == b"WDF1"
    except OSError:
        return False


# planes of a map image hashed: these band indices (clipped to the band count)
WDF_PLANES = (0, 1, 100, 500, 1000)


def wdf(p: Path, max_sweeps=1000) -> dict:
    """renishawWiRE (MIT, black box). One trace: sweeps are the spectra in storage order, two
    channels (Raman shift from the x list, intensity). Table 0: `spectrum` and the stage
    coordinates / times renishawWiRE decodes (x/y/z in µm; time in s from the first spectrum).
    Maps (WMAP with more than one row and column): the image is laid out from renishawWiRE's
    stage coordinates on its map grid (x_start, y_start, x_pad, y_pad): column = round((x - x0)
    / dx), row = round((y - y0) / dy) — openreadout's documented rule — and planes (bands) of
    float32 samples are hashed."""
    warnings.simplefilter("ignore")
    import xxhash
    from renishawWiRE import WDFReader
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        r = WDFReader(str(p))
    x = np.asarray(r.xdata, dtype=np.float64)
    n = int(r.point_per_spectrum)
    spectra = np.asarray(r.spectra, dtype=np.float64).reshape(-1, n)
    count = int(r.count)
    spectra = spectra[:count]
    extra = {"accumulations": int(r.accumulation_count)}
    if r.laser_length is not None and np.isfinite(r.laser_length):
        extra["laser_wavelength_nm"] = float(r.laser_length)
    t = {"index": 0, "reader": "renishawWiRE", "sweep_count": count, "channel_count": 2, "sample_count": n,
         "x_first": float(x[0]), "x_last": float(x[-1]),
         "sweeps": _sweeps([[x, s] for s in spectra], min(max_sweeps, 64)), "parameters": {"extra": extra}}
    out = {"reader": "renishawWiRE (MIT)", "traces": [t]}
    cols = {}
    names = ["spectrum"]
    for axis in ("x", "y", "z"):
        pos = getattr(r, f"{axis}pos", None)
        if pos is not None and hasattr(r, f"{axis}pos_unit"):
            cols[f"{axis.upper()}_um".lower()] = np.asarray(pos, dtype=np.float64)[:count]
    approx = {}
    for h in getattr(r, "origin_list_header", []) or []:
        if str(h[1]) == "Time":
            # renishawWiRE divides the 100 ns ticks by 1e7 before subtracting the first: compared
            # within 1e-5 s, not hashed
            approx["time_s"] = [float(v) for v in np.asarray(h[4], dtype=np.float64)[:count][:64]]
    if cols or approx:
        out["tables"] = [{"index": 0, "event_count": count, "parameter_names": [],
                          "column_hashes": {k: _hash(v) for k, v in cols.items()},
                          "approx_columns": approx}]
    mi = getattr(r, "map_info", None)
    if mi is not None:
        w, h = (int(v) for v in r.map_shape)
        if w > 1 and h > 1:
            xs, ys = np.asarray(r.xpos)[:count], np.asarray(r.ypos)[:count]
            col = np.rint((xs - mi["x_start"]) / mi["x_pad"]).astype(int)
            row = np.rint((ys - mi["y_start"]) / mi["y_pad"]).astype(int)
            planes = []
            for c in sorted({min(b, n - 1) for b in WDF_PLANES}):
                img = np.full((h, w), np.nan, dtype="<f4")
                img[row, col] = spectra[:, c].astype("<f4")
                planes.append({"c": int(c), "z": 0, "t": 0, "xxh3": xxhash.xxh3_128_hexdigest(img.tobytes())})
            out["images"] = [{"index": 0, "size_x": w, "size_y": h, "size_z": 1, "size_c": n, "size_t": 1,
                              "pixel_type": "float", "planes": planes,
                              "physical_size_um": {"x": abs(float(mi["x_pad"])), "y": abs(float(mi["y_pad"]))}}]
    # White-light image (WHTL JPEG): decoded by Pillow; JPEG decoders may differ by a level on
    # some pixels, so the plane carries its mean (the manifest marks such files `lossy`).
    if getattr(r, "img", None) is not None:
        from PIL import Image
        r.img.seek(0)
        pil = Image.open(r.img)
        pix = np.asarray(pil)
        size = {}
        dims = getattr(r, "img_dimensions", None)
        if dims is not None and str(getattr(r, "img_dimension_unit", "")) in ("um", "UnitType.Micron"):
            size = {"x": float(dims[0]) / pil.width, "y": float(dims[1]) / pil.height}
        images = out.setdefault("images", [])
        images.append({"index": len(images), "size_x": pil.width, "size_y": pil.height, "size_z": 1,
                       "size_c": 1, "size_t": 1, "pixel_type": "uint8", "physical_size_um": size,
                       "planes": [{"c": 0, "z": 0, "t": 0, "xxh3": xxhash.xxh3_128_hexdigest(pix.tobytes()),
                                   "mean": float(pix.mean())}]})
    return out
