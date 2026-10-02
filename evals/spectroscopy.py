"""Vibrational-spectroscopy questions (FT-IR, Raman): band positions, laser, resolution,
apodization, spectrum counts, instrument, and a JCAMP-DX conversion.

Every answer is computed here with third-party readers run in the oracle venv, never with
OpenReadout: brukeropus (MIT) for Bruker OPUS, SpectroChemPy (CeCILL-B) for Thermo OMNIC,
renishawWiRE (MIT) for Renishaw WiRE and specio (BSD-3) for PerkinElmer `.sp`. Each band position
is cross-checked with a second, independent source that must agree or the script stops: the
vendor's own export of the same spectrum where the dataset ships one (OMNIC's CSV, WiRE's text
export) or brukeropusreader (GPL-3.0, run as a black box) for OPUS.

The values go to `evals/facts/spectroscopy.json` (committed); `generate.py` reads them like the
other facts (`facts-spectroscopy:` sources), so the questions regenerate without the corpus.

    oracle/.venv/bin/python evals/spectroscopy.py          # recompute evals/facts/spectroscopy.json
    oracle/.venv/bin/python evals/spectroscopy.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import contextlib
import io
import sys
import warnings
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analysis as a
import facts
import generate as g

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(__file__).resolve().parent / "facts" / "spectroscopy.json"


# ---------------------------------------------------------------- readers


def opus_block(cid: str, key: str = "a"):
    """(x, y) of an OPUS data block, brukeropus (MIT)."""
    warnings.simplefilter("ignore")
    from brukeropus import OPUSFile

    f = OPUSFile(str(a.path(cid)))
    d = getattr(f, key)
    return d.x, d.y, f


def opus_band(cid: str):
    import numpy as np

    x, y, _ = opus_block(cid)
    return float(x[int(np.argmax(y))]), a.rd("brukeropus")


def opus_band_cross(cid: str):
    """brukeropusreader (GPL-3.0; run as a black box)."""
    import numpy as np
    from brukeropusreader import read_file

    with contextlib.redirect_stdout(io.StringIO()):
        d = read_file(str(a.path(cid)))
    x = d.get_range("AB")
    y = np.asarray(d["AB"][: len(x)])
    return float(x[int(np.argmax(y))]), a.rd("brukeropusreader") + " (black box)"


def opus_param(cid: str, name: str):
    _, _, f = opus_block(cid)
    return f.params._params[name], a.rd("brukeropus")


def opus_points(cid: str):
    x, _, _ = opus_block(cid)
    return len(x), a.rd("brukeropus")


def omnic_band(cid: str):
    import numpy as np
    import spectrochempy as scp

    warnings.simplefilter("ignore")
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        d = scp.read_omnic(str(a.path(cid)))
    y = np.asarray(d.data[0], dtype=float)
    x = np.asarray(d.x.data, dtype=float)
    return float(x[int(np.argmax(y))]), a.rd("spectrochempy")


def export_band(cid: str, export_id: str):
    """The maximum of the vendor's own text export of the same spectrum (OMNIC CSV, WiRE TXT)."""
    import numpy as np

    rows = []
    for line in a.path(export_id, "oracle-export").read_text(errors="replace").splitlines():
        if line.lstrip().startswith("#"):
            continue
        parts = [p for p in line.replace(",", " ").replace("\t", " ").split() if p]
        if len(parts) >= 2:
            rows.append((float(parts[0]), float(parts[1])))
    arr = np.asarray(rows)
    return float(arr[int(np.argmax(arr[:, 1])), 0]), "the vendor's export " + facts.manifest_file(
        export_id, "oracle-export"
    )


def wdf(cid: str):
    from renishawWiRE import WDFReader

    warnings.simplefilter("ignore")
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        return WDFReader(str(a.path(cid)))


def wdf_band(cid: str):
    import numpy as np

    r = wdf(cid)
    y = np.asarray(r.spectra, dtype=float).reshape(-1, r.point_per_spectrum)[0]
    return float(np.asarray(r.xdata, dtype=float)[int(np.argmax(y))]), a.rd("renishawWiRE")


def wdf_laser_nm(cid: str):
    return float(wdf(cid).laser_length), a.rd("renishawWiRE")


def wdf_count(cid: str):
    r = wdf(cid)
    return int(r.count), a.rd("renishawWiRE")


def wdf_map_shape(cid: str):
    r = wdf(cid)
    w, h = (int(v) for v in r.map_shape)
    return [w, h], a.rd("renishawWiRE")


def pesp_meta(cid: str, key: str):
    import collections
    import collections.abc

    for n in ("Iterable", "Mapping", "Sequence", "MutableMapping"):
        if not hasattr(collections, n):
            setattr(collections, n, getattr(collections.abc, n))
    from specio import specread

    return specread(str(a.path(cid))).meta[key], a.rd("specio")


# ---------------------------------------------------------------- facts


FACTS: list[a.Fact] = [
    a.Fact(
        "opus-bitumen-unaged-2",
        "strongest_band_cm1",
        "absorbance block (AB): wavenumber of the highest point",
        lambda: opus_band("opus-bitumen-unaged-2"),
        lambda: opus_band_cross("opus-bitumen-unaged-2"),
    ),
    a.Fact(
        "opus-bitumen-unaged-2",
        "apodization",
        "Fourier-transform parameter APF",
        lambda: opus_param("opus-bitumen-unaged-2", "apf"),
    ),
    a.Fact("opus-bitumen-unaged-2", "points", "absorbance block: NPT", lambda: opus_points("opus-bitumen-unaged-2")),
    a.Fact(
        "opus-bitumen-unaged-2",
        "first_x",
        "absorbance block: FXV",
        lambda: (float(opus_block("opus-bitumen-unaged-2")[0][0]), a.rd("brukeropus")),
    ),
    a.Fact(
        "opus-bitumen-unaged-2",
        "last_x",
        "absorbance block: LXV",
        lambda: (float(opus_block("opus-bitumen-unaged-2")[0][-1]), a.rd("brukeropus")),
    ),
    a.Fact(
        "opus-bitumen-unaged-2",
        "max_y",
        "absorbance block: highest value",
        lambda: (float(max(opus_block("opus-bitumen-unaged-2")[1])), a.rd("brukeropus")),
    ),
    a.Fact(
        "opus-or2-mmp-2107-test1",
        "resolution_cm1",
        "acquisition parameter RES",
        lambda: opus_param("opus-or2-mmp-2107-test1", "res"),
    ),
    a.Fact(
        "omnic-toffolo-atr-paraffin",
        "strongest_band_cm1",
        "absorbance: wavenumber of the highest point",
        lambda: omnic_band("omnic-toffolo-atr-paraffin"),
        lambda: export_band("omnic-toffolo-atr-paraffin", "omnic-toffolo-atr-paraffin-csv"),
    ),
    a.Fact(
        "wdf-zenodo8102788-ooid",
        "strongest_band_cm1",
        "Raman spectrum: Raman shift of the highest point",
        lambda: wdf_band("wdf-zenodo8102788-ooid"),
        lambda: export_band("wdf-zenodo8102788-ooid", "wdf-zenodo8102788-ooid-txt"),
    ),
    a.Fact(
        "wdf-zenodo8102788-specimen-a-9",
        "laser_nm",
        "laser wavelength (header wavenumber, 10^7 / cm-1)",
        lambda: wdf_laser_nm("wdf-zenodo8102788-specimen-a-9"),
    ),
    a.Fact("wdf-pywdf-streamline", "spectra", "spectra in the map (count)", lambda: wdf_count("wdf-pywdf-streamline")),
    a.Fact(
        "wdf-pywdf-streamline", "map_shape", "map grid (columns, rows)", lambda: wdf_map_shape("wdf-pywdf-streamline")
    ),
    a.Fact(
        "pesp-zenodo8161216-ts-black-gallus-untreated-01-01",
        "instrument",
        "instrument model text",
        lambda: pesp_meta("pesp-zenodo8161216-ts-black-gallus-untreated-01-01", "instrument_model"),
    ),
    a.Fact(
        "pesp-zenodo8161216-ts-black-gallus-untreated-01-01",
        "scans",
        "accumulations",
        lambda: pesp_meta("pesp-zenodo8161216-ts-black-gallus-untreated-01-01", "accumulations"),
    ),
]


FACTS += __import__("heldout_report_fixes").spectro_facts(a.Fact, a.path)  # LiveTrack Z (2026-09-24)


def compute() -> dict:
    out: dict = {}
    for f in FACTS:
        value, reader = f.compute()
        value = a.tidy(value)
        rec: dict[str, Any] = {"value": value, "reader": reader, "how": f.how}
        if f.cross:
            other, other_reader = f.cross()
            # band positions: the export rounds x; one sampling step apart would be a real disagreement
            if not a.agree(value, a.tidy(other), rel=1e-4):
                raise SystemExit(f"{f.corpus_id} {f.name}: {reader} says {value!r}, {other_reader} says {other!r}")
            rec["cross_check"] = f"{other_reader}: agrees"
        entry = out.setdefault(
            f.corpus_id,
            {"file": facts.manifest_file(f.corpus_id, f.role), "extractor": "evals/spectroscopy.py", "facts": {}},
        )
        entry["facts"][f.name] = rec
        print(f"{f.corpus_id} {f.name} = {value!r}", file=sys.stderr)
    return out


# ---------------------------------------------------------------- questions

BAND_HINT = "the position in cm-1"

SPECTRO_SPECS: list[g.Spec] = [
    g.Spec(
        "ana-spec-opus-strongest-band",
        "opus-bitumen-unaged-2",
        "analysis",
        "This is an FT-IR spectrum from a Bruker instrument. At what wavenumber is the strongest absorbance band?",
        lambda f, m: g.number(f["strongest_band_cm1"], "cm-1", abs_=3.0),
        "facts-spectroscopy: strongest_band_cm1 (brukeropus; brukeropusreader agrees)",
        answer_hint=BAND_HINT,
    ),
    g.Spec(
        "ana-spec-omnic-strongest-band",
        "omnic-toffolo-atr-paraffin",
        "analysis",
        "At what wavenumber is the strongest absorbance band in this ATR-FTIR spectrum?",
        lambda f, m: g.number(f["strongest_band_cm1"], "cm-1", abs_=3.0),
        "facts-spectroscopy: strongest_band_cm1 (SpectroChemPy; OMNIC's own CSV export agrees)",
        answer_hint=BAND_HINT,
    ),
    g.Spec(
        "ana-spec-wdf-strongest-band",
        "wdf-zenodo8102788-ooid",
        "analysis",
        "At what Raman shift is the most intense peak of this Raman spectrum?",
        lambda f, m: g.number(f["strongest_band_cm1"], "cm-1", abs_=3.0),
        "facts-spectroscopy: strongest_band_cm1 (renishawWiRE; WiRE's own text export agrees)",
        answer_hint=BAND_HINT,
    ),
    g.Spec(
        "spec-wdf-laser-wavelength",
        "wdf-zenodo8102788-specimen-a-9",
        "instrument",
        "What laser wavelength was used to record this Raman spectrum?",
        lambda f, m: g.number(f["laser_nm"], "nm", abs_=1.0),
        "facts-spectroscopy: laser_nm (renishawWiRE)",
        answer_hint="a wavelength with its unit",
    ),
    g.Spec(
        "spec-wdf-map-spectra",
        "wdf-pywdf-streamline",
        "counts",
        "How many spectra are in this Raman map?",
        lambda f, m: g.integer(f["spectra"]),
        "facts-spectroscopy: spectra (renishawWiRE)",
    ),
    g.Spec(
        "spec-opus-resolution",
        "opus-or2-mmp-2107-test1",
        "method",
        "What spectral resolution was this spectrum acquired at?",
        lambda f, m: g.number(f["resolution_cm1"], "cm-1", abs_=0.01),
        "facts-spectroscopy: resolution_cm1 (brukeropus)",
        answer_hint="the resolution in cm-1",
    ),
    g.Spec(
        "spec-opus-apodization",
        "opus-bitumen-unaged-2",
        "method",
        "Which apodization function was used to compute this spectrum?",
        lambda f, m: g.string(
            f["apodization"],
            ["Blackman-Harris", "Blackman Harris", "B3"] if f["apodization"] == "B3" else [f["apodization"]],
            reject=["Happ-Genzel", "boxcar", "Norton-Beer", "triangular"],
        ),
        "facts-spectroscopy: apodization (brukeropus; B3 is the 3-term Blackman-Harris function)",
        answer_hint="the apodization function",
    ),
    g.Spec(
        "spec-pesp-instrument",
        "pesp-zenodo8161216-ts-black-gallus-untreated-01-01",
        "instrument",
        "Which spectrometer model recorded this IR spectrum?",
        lambda f, m: g.string(f["instrument"], ["Frontier"] if "Frontier" in f["instrument"] else [f["instrument"]]),
        "facts-spectroscopy: instrument (specio)",
        answer_hint="the instrument model",
    ),
    g.Spec(
        "task-opus-jcamp",
        "opus-bitumen-unaged-2",
        "conversion",
        "Convert the absorbance spectrum in this file to JCAMP-DX, saved as spectrum.jdx in the current "
        "directory, and tell me how many data points it has.",
        lambda f, m: g.integer(f["points"]),
        "facts-spectroscopy: points, first_x, last_x, max_y (brukeropus); output read with the jcamp package (MIT)",
        task={
            "output": "spectrum.jdx",
            "kind": "jcamp",
            "expect": {"points": "/points", "first_x": "/first_x", "last_x": "/last_x", "max_y": "/max_y"},
        },
    ),
]
SPECTRO_SPECS += __import__("heldout_report_fixes").spectro_specs(g)  # LiveTrack Z (2026-09-24)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/spectroscopy.json is out of date")
    args = ap.parse_args()
    text = a.render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/spectroscopy.json is out of date; run evals/spectroscopy.py", file=sys.stderr)
            return 1
        print("evals/facts/spectroscopy.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
