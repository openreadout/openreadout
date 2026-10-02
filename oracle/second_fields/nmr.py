"""NMR: the acquisition and processing parameters, read by a second reader and set against the
normalized fields and `extra` OpenReadout reports. The primary oracles (nmrglue's data readers,
jcamp for JCAMP-DX values) compare the values; these checks cover the metadata.

- Bruker TopSpin: `acqus` and `pdata/*/procs` parsed with nmrglue's JCAMP parameter reader
  (bruker.read_jcamp): frequencies, widths, sizes, scans, pulse program, nucleus, solvent,
  temperature, date, digital-filter parameters, and the processing parameters.
- Varian/Agilent: `procpar` parsed with nmrglue (varian.read_procpar).
- JCAMP-DX: the labelled data records read by the `jcamp` package (MIT).
- JEOL Delta: the same measurement's JCAMP-DX export where the depositor gave one (nmrXiv S200
  limonene), read with `jcamp`: a second, vendor-written view of the experiment.
"""
from __future__ import annotations

import re
from datetime import datetime, timezone
from importlib.metadata import version
from pathlib import Path

from . import Rec

FAMILY = "nmr"
FORMATS = {"bruker-nmr", "varian-nmr", "jcamp-dx", "jeol-jdf"}

NUC = {"H1": "1H", "C13": "13C", "N15": "15N", "P31": "31P", "F19": "19F", "Si29": "29Si", "H2": "2H"}


def bruker(rec: Rec, path: Path) -> None:
    import nmrglue as ng
    r = rec.reader(f"nmrglue {version('nmrglue')} (BSD-3) bruker.read_jcamp on acqus and procs")
    acqus = path / "acqus"
    if not acqus.exists():
        return
    a = ng.bruker.read_jcamp(str(acqus))
    ex = "/traces/0/extra"
    num = dict(cmp="num", rel=1e-12, reader=r)
    for key, field in (("SFO1", "spectrometer_frequency_mhz"), ("BF1", "base_frequency_mhz"),
                       ("SW_h", "spectral_width_hz"), ("SW", "spectral_width_ppm"), ("O1", "carrier_offset_hz"),
                       ("TE", "temperature_k")):
        if key in a:
            rec.check(f"acqus {key}", f"{ex}/{field}", float(a[key]), **num)
    for key, field in (("TD", "time_domain_size"), ("NS", "scans"), ("DS", "dummy_scans"), ("DECIM", "decimation"),
                       ("DSPFVS", "dsp_firmware")):
        if key in a:
            rec.check(f"acqus {key}", f"{ex}/{field}", int(a[key]), reader=r)
    if "RG" in a:
        rec.check("acqus RG", f"{ex}/receiver_gain", float(a["RG"]), **num)
    if float(a.get("GRPDLY", -1)) >= 0:
        rec.check("acqus GRPDLY", f"{ex}/group_delay_points", float(a["GRPDLY"]), **num)
    for key, field in (("PULPROG", "pulse_program"), ("SOLVENT", "solvent"), ("INSTRUM", "instrument")):
        rec.check(f"acqus {key}", f"{ex}/{field}", a.get(key), cmp="text", reader=r)
    nuc = a.get("NUC1")
    if nuc and nuc != "off":
        rec.check("acqus NUC1", f"{ex}/nucleus", nuc, cmp="text", reader=r)
    if a.get("DATE"):
        t = datetime.fromtimestamp(int(a["DATE"]), timezone.utc)
        rec.check("acqus DATE (seconds since 1970)", "/experiment/acquisition/started_at",
                  t.isoformat().replace("+00:00", "Z"), cmp="time", abs=1.0, reader=r)
    for pdir in sorted((path / "pdata").glob("*")) if (path / "pdata").is_dir() else []:
        procs = pdir / "procs"
        if not procs.exists():
            continue
        p = ng.bruker.read_jcamp(str(procs))
        where = f"/traces/[name=pdata~1{pdir.name}]/extra"  # (~1: a "/" inside a path segment)
        for key, field in (("SF", "spectrometer_frequency_mhz"), ("SW_p", "spectral_width_hz"),
                           ("LB", "line_broadening_hz"), ("PHC0", "phase0_deg"), ("PHC1", "phase1_deg")):
            if key in p:
                rec.check(f"pdata/{pdir.name} procs {key}", f"{where}/{field}", float(p[key]), **num)
        if "OFFSET" in p:
            rec.check(f"pdata/{pdir.name} procs OFFSET (first ppm)", f"{where}/axis/first", float(p["OFFSET"]), **num)
        if "NC_proc" in p:
            rec.check(f"pdata/{pdir.name} procs NC_proc", f"{where}/normalization_exponent", int(p["NC_proc"]), reader=r)
        if "SI" in p and (pdir / "1r").exists() and not (path / "acqu2s").exists():
            rec.check(f"pdata/{pdir.name} procs SI", f"/traces/[name=pdata~1{pdir.name}]/sample_count", int(p["SI"]), reader=r)


def varian(rec: Rec, path: Path) -> None:
    import nmrglue as ng
    pp = path / "procpar" if path.is_dir() else path.parent / "procpar"
    if not pp.exists():
        return
    r = rec.reader(f"nmrglue {version('nmrglue')} (BSD-3) varian.read_procpar")
    v = ng.varian.read_procpar(str(pp))

    def val(k):
        x = v.get(k, {}).get("values")
        return x[0] if x else None
    ex = "/traces/0/extra"
    num = dict(cmp="num", rel=1e-12, reader=r)
    if val("sfrq"):
        rec.check("procpar sfrq", f"{ex}/spectrometer_frequency_mhz", float(val("sfrq")), **num)
    if val("sw"):
        rec.check("procpar sw", f"{ex}/spectral_width_hz", float(val("sw")), **num)
    if val("np"):
        rec.check("procpar np", f"{ex}/time_domain_size", int(float(val("np"))), reader=r)
    if val("nt"):
        rec.check("procpar nt", f"{ex}/scans", int(float(val("nt"))), reader=r)
    if val("tn"):
        rec.check("procpar tn (nucleus)", f"{ex}/nucleus", NUC.get(val("tn"), val("tn")), cmp="text", reader=r)
    for k, field in (("solvent", "solvent"), ("seqfil", "pulse_program"), ("console", "console"),
                     ("probe_", "probe"), ("operator", "operator")):
        rec.check(f"procpar {k}", f"{ex}/{field}", val(k), cmp="text", reader=r)
    rec.check("procpar pslabel", "/experiment/method/name", val("pslabel"), cmp="text", reader=r)
    rec.check("procpar samplename", "/experiment/sample/id", val("samplename"), cmp="text", reader=r)
    if val("temp") and v.get("vtc") is not None or val("temp"):
        try:
            rec.check("procpar temp (°C)", f"{ex}/temperature_c", float(val("temp")), **num)
        except (TypeError, ValueError):
            pass
    tr = val("time_run")
    if tr and re.fullmatch(r"\d{8}T\d{6}", tr):
        t = datetime.strptime(tr, "%Y%m%dT%H%M%S").isoformat()
        rec.check("procpar time_run", "/experiment/acquisition/started_at", t, cmp="time", reader=r)


def jcamp_file(rec: Rec, path: Path, reader_name: str | None = None, prefix: str = "") -> dict | None:
    import jcamp
    import contextlib
    import io
    import warnings
    with warnings.catch_warnings(), contextlib.redirect_stdout(io.StringIO()):
        warnings.simplefilter("ignore")
        d = jcamp.jcamp_readfile(str(path)) if hasattr(jcamp, "jcamp_readfile") else jcamp.readfile(str(path))
    return d


def jcampdx(rec: Rec, path: Path) -> None:
    d = jcamp_file(rec, path)
    if not d or d.get("children"):
        # compound files (several blocks): the first block's records only
        d = (d or {}).get("children", [{}])[0] if d else None
    if not d:
        return
    r = rec.reader(f"jcamp {version('jcamp')} (MIT)")
    ex = "/traces/0/extra"
    rec.check("##TITLE", f"{ex}/title", d.get("title"), cmp="text", reader=r)
    rec.check("##DATA TYPE", f"{ex}/data_type", d.get("data type"), cmp="text", reader=r)
    rec.check("##ORIGIN", f"{ex}/origin", d.get("origin"), cmp="text", reader=r)
    rec.check("##OWNER", f"{ex}/owner", d.get("owner"), cmp="text", reader=r)
    rec.check("##JCAMP-DX version", f"{ex}/jcamp_version", str(d.get("jcamp-dx", "")).split()[0] if d.get("jcamp-dx") else None,
              cmp="text", reader=r)
    of = d.get(".observe frequency")
    if isinstance(of, (int, float)):
        rec.check("##.OBSERVE FREQUENCY", f"{ex}/observe_frequency_mhz", float(of), cmp="num", rel=1e-12, reader=r)
    rec.check("##.OBSERVE NUCLEUS", f"{ex}/nucleus", d.get(".observe nucleus"), cmp="text", reader=r)
    rec.check("##.SOLVENT NAME", f"{ex}/solvent", d.get(".solvent name"), cmp="text", reader=r)
    rec.check("##.PULSE SEQUENCE", f"{ex}/pulse_sequence", d.get(".pulse sequence"), cmp="text", reader=r)
    if isinstance(d.get("npoints"), (int, float)) and "x" in d and len(d["x"]) == int(d["npoints"]):
        rec.check("##NPOINTS", "/traces/0/sample_count", int(d["npoints"]), reader=r)
    for key, field in (("firstx", "axis/first"), ("lastx", "axis/last")):
        v = d.get(key)
        if isinstance(v, (int, float)) and "axis" in str(field):
            rec.check(f"##{key.upper()}", f"{ex}/{field}", float(v), cmp="num", rel=1e-9, abs=1e-9, reader=r)
    rec.check("##XUNITS", f"{ex}/axis/unit", d.get("xunits"), cmp="text", reader=r)
    sds = d.get("spectrometer/data system") or d.get("spectrometer")
    if isinstance(sds, str) and sds.strip():
        rec.check("##SPECTROMETER/DATA SYSTEM", "/experiment/instrument/model", sds.strip(), cmp="loose", reader=r)
    ld = d.get("long date") or d.get("longdate")
    if isinstance(ld, str) and re.match(r"\d{4}/\d{2}/\d{2}", ld.strip()):
        t = datetime.strptime(ld.strip()[:19], "%Y/%m/%d %H:%M:%S").isoformat()
        rec.check("##LONG DATE", "/experiment/acquisition/started_at", t, cmp="time", reader=r)


def jeol_params(rec: Rec, path: Path) -> None:
    """nmrglue's JEOL header and parameter block (jeol.read), not its data."""
    import nmrglue as ng
    import contextlib
    import io
    with contextlib.redirect_stdout(io.StringIO()):
        dic, _ = ng.jeol.read(str(path))
    h, p = dic.get("header", {}), dic.get("parameters", {})
    r = rec.reader(f"nmrglue {version('nmrglue')} (BSD-3) jeol.read header and parameters")
    ex = "/traces/0/extra"
    num = dict(cmp="num", rel=1e-12, reader=r)
    rec.check("header title", f"{ex}/title", h.get("title"), cmp="text", reader=r)
    rec.check("header site", f"{ex}/site", h.get("site"), cmp="text", reader=r)
    rec.check("header comment", f"{ex}/comment", h.get("comment"), cmp="text", reader=r)
    if isinstance(p.get("x_freq"), (int, float)):
        rec.check("x_freq (MHz)", f"{ex}/spectrometer_frequency_mhz", p["x_freq"] / 1e6, **num)
    if isinstance(p.get("x_sweep"), (int, float)):
        rec.check("x_sweep (Hz)", f"{ex}/spectral_width_hz", float(p["x_sweep"]), **num)
    if isinstance(p.get("x_offset"), (int, float)):
        rec.check("x_offset (ppm)", f"{ex}/carrier_offset_ppm", float(p["x_offset"]), **num)
    for k, field in (("scans", "scans"), ("total_scans", "total_scans")):
        if isinstance(p.get(k), (int, float)):
            rec.check(k, f"{ex}/{field}", int(p[k]), reader=r)
    for k, field in (("solvent", "solvent"), ("experiment", "pulse_program"), ("x_domain", "domain")):
        rec.check(k, f"{ex}/{field}", p.get(k), cmp="text", reader=r)
    # the sample_id parameter is a 16-byte text field (nmrglue drops its spaces): ours is the whole
    # id, which must contain it
    rec.check("sample_id parameter (16 bytes)", f"{ex}/sample_id", p.get("sample_id"), cmp="contains", reader=r)
    jeol_context_sample_id(rec, path, h)
    if isinstance(p.get("temp_get"), (int, float)):
        rec.check("temp_get (°C)", f"{ex}/temperature_c", float(p["temp_get"]), **num)
    if isinstance(p.get("field_strength"), (int, float)):
        rec.check("field_strength (T)", f"{ex}/field_strength_t", float(p["field_strength"]), **num)
    t = p.get("actual_start_time")
    if isinstance(t, int) and t > 0:
        # seconds since 1990-01-01 UTC (the value that puts every corpus file on its creation date)
        iso = datetime.fromtimestamp(t + 631_152_000, timezone.utc).isoformat().replace("+00:00", "Z")
        rec.check("actual_start_time (s since 1990, UTC)", "/experiment/acquisition/started_at", iso, cmp="time",
                  abs=1.0, reader=r)


def jeol_context_sample_id(rec: Rec, path: Path, h: dict) -> None:
    """The context section (nmrglue's header `context_start`/`context_length`) is the experiment
    text the spectrometer ran; its `sample_id => "...";` line holds the id uncut. Read as text
    with a regular expression."""
    start, length = h.get("context_start"), h.get("context_length")
    if not isinstance(start, int) or not isinstance(length, int) or length <= 0:
        return
    with open(path, "rb") as f:
        f.seek(start)
        text = f.read(length).decode("latin-1")
    m = re.search(r'^\s*sample_id\s*=>?\s*"([^"]*)"', text, re.M)
    if not m or not m.group(1).strip():
        return
    r = rec.reader("the .jdf context section (at nmrglue's context_start), read as text with a regular expression")
    rec.check("sample_id (context text)", "/traces/0/extra/sample_id", m.group(1).strip(), reader=r)
    rec.check("sample id (context text)", "/experiment/sample/id", m.group(1).strip(), reader=r)


def jeol(rec: Rec, path: Path, ctx) -> None:
    """nmrXiv S200 deposits the limonene measurements as JEOL .jdf and as JCAMP-DX: the 1H
    measurement's JCAMP-DX file is a second, vendor-written view of the same experiment."""
    jeol_params(rec, path)
    m = re.match(r"(.*)_(qHNMR)_400MHz_Jeol\.jdf$", path.name)
    if not m:
        return
    twin = path.with_name(f"{m.group(1)}_{m.group(2)}_400MHz_JDX.jdx")
    if not twin.exists():
        return
    d = jcamp_file(rec, twin)
    r = rec.reader(f"jcamp {version('jcamp')} (MIT) on the depositor's JCAMP-DX of the same measurement ({twin.name})")
    ex = "/traces/0/extra"
    of = d.get(".observe frequency")
    if isinstance(of, (int, float)):
        rec.check("observe frequency (JCAMP-DX twin)", f"{ex}/spectrometer_frequency_mhz", float(of), cmp="num",
                  rel=1e-12, reader=r)
    nuc = str(d.get(".observe nucleus", "")).lstrip("^")
    rec.check("nucleus (JCAMP-DX twin)", f"{ex}/nucleus", nuc, cmp="text", reader=r)
    rec.check("solvent (JCAMP-DX twin)", f"{ex}/solvent", d.get(".solvent name"), cmp="text", reader=r)
    rec.check("pulse sequence (JCAMP-DX twin)", f"{ex}/pulse_program", d.get(".pulse sequence"), cmp="text", reader=r)
    rec.check("title (JCAMP-DX twin)", f"{ex}/title", d.get("title"), cmp="text", reader=r)
    ld = d.get("long date")
    if isinstance(ld, str) and re.match(r"\d{4}/\d{2}/\d{2}", ld.strip()):
        # the JCAMP-DX writes a local clock; the .jdf a UTC time: the same reading up to a zone
        t = datetime.strptime(ld.strip()[:19], "%Y/%m/%d %H:%M:%S").isoformat()
        rec.check("acquisition start (JCAMP-DX twin, local clock)", "/experiment/acquisition/started_at", t,
                  cmp="time_mod_zone", abs=5.0, reader=r)


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    fmt = entry["format"]
    if fmt == "bruker-nmr":
        bruker(rec, path)
    elif fmt == "varian-nmr":
        varian(rec, path)
    elif fmt == "jcamp-dx":
        jcampdx(rec, path)
    elif fmt == "jeol-jdf":
        jeol(rec, path, ctx)
