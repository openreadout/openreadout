"""Facts and questions for the fixes of the second held-out generalization report (2026-09-24).

Every question is about a development file added for a finding. None is about a held-out file, and
the answers come from independent readers and vendor exports only:

- qPCR "Undetermined" wells (N-H1): the vendor's own Results exports (`.xls`, read with xlrd),
  cross-checked against the vendor result files inside the `.eds` (Python standard library);
- Harmony 6 `Index.xml` (N-M2): the index parsed with the Python standard library (its `Maps`),
  cross-checked against Bio-Formats (the committed oracle JSON);
- Gen5 exports without their file header (N-M2): Gen5's own `[Concentration]` row in the export;
- Shimadzu `.lcd` (N-L1): the LabSolutions ASCII export of the same run;
- Renishaw WiRE LiveTrack map (N-L2): renishawWiRE (MIT), cross-checked against the ORGN list read
  with the standard library.

Imported by `analysis.py` (facts → `evals/facts/analysis.json`, questions → the analysis tier).
Only the standard library is imported at the top.
"""

from __future__ import annotations

import re
import statistics
import struct
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import Any, Callable

NPFF = "eds-viia7-npff-ddct"
QS35 = "eds-qs35-npff-mtrna-ampstatus"
QS12K = "eds-qs12k-tdmmc-stdcurve"
HARMONY6 = "hcs-harmony6-cpg0048-41005680-index"
GEN5_HL = "synthetic-gen5-headerless-stdcurve-linear"
LCD = "lcd-streamfind-adc-uv"
WDF_LT = "wdf-zenodo10694741-streamline-livetrack"

ROOT = Path(__file__).resolve().parent.parent
sys.path.append(str(ROOT / "oracle"))
import oracle_json  # noqa: E402  (oracle/oracle_json.py: <id>.json, or <id>.json.gz over 1 MiB)

# ---------------------------------------------------------------- qPCR: vendor Results exports


def _export_rows(p: Path) -> list[dict[str, Any]]:
    import xlrd

    sh = xlrd.open_workbook(str(p)).sheet_by_name("Results")
    rows = [sh.row_values(i) for i in range(sh.nrows)]
    hi = next(i for i, r in enumerate(rows) if r and str(r[0]).strip() == "Well")
    head = [str(c).strip() for c in rows[hi]]
    return [dict(zip(head, r, strict=False)) for r in rows[hi + 1 :] if r and isinstance(r[0], float)]


def _xlrd() -> str:
    from importlib.metadata import version

    return f"xlrd {version('xlrd')} on the vendor's Results export (.xls)"


def export_undetermined(p: Path, target: str | None = None) -> tuple[int, str]:
    rows = _export_rows(p)
    n = sum(
        1 for r in rows if str(r["CT"]).strip() == "Undetermined" and (target is None or r["Target Name"] == target)
    )
    return n, _xlrd()


def export_mean_ct(p: Path, target: str) -> tuple[float, str]:
    cts = [r["CT"] for r in _export_rows(p) if r["Target Name"] == target and isinstance(r["CT"], float)]
    return statistics.fmean(cts), _xlrd() + f" (mean of the {len(cts)} numeric CT values)"


def export_amp_status(p: Path, status: str) -> tuple[int, str]:
    return sum(1 for r in _export_rows(p) if str(r.get("Amp Status", "")).strip() == status), _xlrd()


def export_undetermined_wells(p: Path) -> tuple[list[str], str]:
    return sorted(str(r["Well Position"]) for r in _export_rows(p) if str(r["CT"]).strip() == "Undetermined"), _xlrd()


def _analysis_rows(eds: Path) -> tuple[list[dict[str, str]], int]:
    """analysis_result.txt result lines and the cycling stage's repeat count (tcprotocol.xml)."""
    z = zipfile.ZipFile(eds)
    text = z.read("apldbio/sds/analysis_result.txt").decode("utf-8", "replace")
    root = ET.fromstring(z.read("apldbio/sds/tcprotocol.xml"))
    cycles = next(
        int(st.findtext("NumOfRepetitions")) for st in root.findall("TCStage") if st.findtext("StageFlag") == "CYCLING"
    )
    header, out = None, []
    for line in text.splitlines():
        cells = line.split("\t")
        if header is None:
            if cells and cells[0].strip().lower() == "well":
                header = [c.strip().lower() for c in cells]
            continue
        if cells and cells[0].strip().isdigit():
            out.append(dict(zip(header, (c.strip() for c in cells), strict=False)))
    return out, cycles


def eds_ct_at_cycles(eds: Path, target: str | None = None) -> tuple[int, str]:
    rows, cycles = _analysis_rows(eds)
    n = sum(1 for r in rows if float(r["ct"]) >= cycles and (target is None or r["detector"] == target))
    return n, f"zipfile + analysis_result.txt (Ct = the {cycles} cycles of tcprotocol.xml)"


def eds_mean_ct(eds: Path, target: str) -> tuple[float, str]:
    rows, cycles = _analysis_rows(eds)
    cts = [float(r["ct"]) for r in rows if r["detector"] == target and float(r["ct"]) < cycles]
    return statistics.fmean(cts), "zipfile + analysis_result.txt (Ct below the cycle count)"


def eds_amp_status(eds: Path, code: str) -> tuple[int, str]:
    rows, _ = _analysis_rows(eds)
    return sum(1 for r in rows if r.get("amp status") == code), "zipfile + analysis_result.txt `Amp Status`"


def eds_undetermined_wells(eds: Path, columns: int = 24) -> tuple[list[str], str]:
    rows, cycles = _analysis_rows(eds)
    out = []
    for r in rows:
        if float(r["ct"]) >= cycles:
            w = int(r["well"])
            out.append(f"{chr(65 + w // columns)}{w % columns + 1}")
    return sorted(out), "zipfile + analysis_result.txt (Ct = cycle count), 0-based wells on 24 columns"


# ---------------------------------------------------------------- Harmony 6 index (Maps)


def harmony_maps(p: Path) -> dict[int, dict[str, str]]:
    out: dict[int, dict[str, str]] = {}
    for _, el in ET.iterparse(p, events=("end",)):
        tag = el.tag.rsplit("}", 1)[-1]
        if tag == "Entry" and el.get("ChannelID") is not None:
            e = out.setdefault(int(el.get("ChannelID")), {})
            for c in el:
                e.setdefault(c.tag.rsplit("}", 1)[-1], (c.text or "").strip())
            el.clear()
        elif tag == "Image":
            el.clear()
    return out


def harmony_pixel_um(p: Path) -> tuple[float, str]:
    m = harmony_maps(p)
    return float(m[1]["ImageResolutionX"]) * 1e6, "ElementTree: Maps/Map/Entry[@ChannelID=1]/ImageResolutionX (m → µm)"


def harmony_channel_name(p: Path, channel: int) -> tuple[str, str]:
    return harmony_maps(p)[channel]["ChannelName"], f"ElementTree: Maps/Map/Entry[@ChannelID={channel}]/ChannelName"


def _hcs_oracle(cid: str) -> dict:
    return oracle_json.load(ROOT / "corpus" / "oracle" / f"{cid}.json")["hcs"]


def bf_pixel_um(cid: str) -> tuple[float, str]:
    return _hcs_oracle(cid)["bioformats"]["physical_size_um"][0], "Bio-Formats 8.5.0 showinf -omexml (committed oracle)"


def bf_channel_name(cid: str, channel: int) -> tuple[str, str]:
    return _hcs_oracle(cid)["bioformats"]["channels"][
        channel - 1
    ], "Bio-Formats 8.5.0 showinf -omexml (committed oracle)"


# ---------------------------------------------------------------- Gen5 without its header


def gen5_concentration(p: Path, well: str) -> tuple[float, str]:
    """Gen5's own [Concentration] row for `well` (a row letter, then a column)."""
    lines = p.read_bytes().decode("latin-1").splitlines()
    i = next(k for k, ln in enumerate(lines) if ln.strip() == "Results")
    row = None
    for ln in lines[i + 1 :]:
        cells = ln.split("\t")
        if cells[0] and re.fullmatch(r"[A-H]", cells[0]):
            row = cells[0]
        if row == well[0] and cells[-1].strip() == "[Concentration]":
            return float(cells[int(well[1:])]), "the export's own [Concentration] row (Gen5-computed)"
    raise KeyError(well)


def gen5_concentration_refit(p: Path, well: str) -> tuple[float, str]:
    """The same value refitted with the standard library: blank-subtracted standards, least squares."""
    lines = p.read_bytes().decode("latin-1").splitlines()
    grids: dict[str, dict[str, str]] = {}
    section, row = None, None
    for ln in lines:
        cells = ln.split("\t")
        if ln.strip() in ("Layout", "Results"):
            section, row = ln.strip(), None
            continue
        if section and len(cells) >= 13 and cells[-1].strip():
            if cells[0] and re.fullmatch(r"[A-H]", cells[0]):
                row = cells[0]
            if row:
                g = grids.setdefault(f"{section}:{cells[-1].strip()}", {})
                for c in range(1, 13):
                    g[f"{row}{c}"] = cells[c].strip()
    ids, conc = grids["Layout:Well ID"], grids["Layout:Conc/Dil"]
    raw_key = next(k for k in grids if k.startswith("Results:") and ":" in k[len("Results:") :] and "Blank" not in k)
    raw = {w: float(v) for w, v in grids[raw_key].items()}
    blank = statistics.fmean(raw[w] for w, i in ids.items() if i == "BLK")
    std = [w for w, i in ids.items() if i.startswith("STD")]
    xs = [float(conc[w]) for w in std]
    ys = [raw[w] - blank for w in std]
    fit = statistics.linear_regression(xs, ys)
    return (
        raw[well] - blank - fit.intercept
    ) / fit.slope, "statistics.linear_regression on the blank-subtracted standards of the embedded layout"


# ---------------------------------------------------------------- Shimadzu ASCII export


def _lcd_section(txt: Path, name: str) -> tuple[dict[str, str], list[tuple[float, float]]]:
    lines = txt.read_text(encoding="latin-1").splitlines()
    i = lines.index(f"[{name}]")
    kv, data = {}, []
    for ln in lines[i + 1 :]:
        if not ln.strip():
            break
        c = ln.split("\t")
        if c[0][:1].isdigit() and len(c) >= 2:
            data.append((float(c[0]), float(c[1])))
        elif len(c) > 1:
            kv[c[0]] = c[1]
    return kv, data


def lcd_points(txt: Path, name: str) -> tuple[int, str]:
    kv, _ = _lcd_section(txt, name)
    return int(kv["# of Points"]), f"LabSolutions ASCII export [{name}] `# of Points`"


def lcd_rows(txt: Path, name: str) -> tuple[int, str]:
    _, data = _lcd_section(txt, name)
    return len(data), f"LabSolutions ASCII export [{name}]: data rows counted"


def lcd_wavelength(txt: Path, name: str) -> tuple[float, str]:
    kv, _ = _lcd_section(txt, name)
    return float(kv["Wavelength(nm)"]), f"LabSolutions ASCII export [{name}] `Wavelength(nm)`"


def lcd_value_mv(txt: Path, name: str, t_min: float) -> tuple[float, str]:
    kv, data = _lcd_section(txt, name)
    (v,) = [v for t, v in data if abs(t - t_min) < 1e-6]
    return v * float(
        kv["Intensity Multiplier"]
    ), f"LabSolutions ASCII export [{name}]: Intensity x Intensity Multiplier ({kv['Intensity Units']})"


# ---------------------------------------------------------------- Renishaw WiRE LiveTrack Z


def wdf_z_range(p: Path) -> tuple[float, str]:
    import numpy as np
    from renishawWiRE import WDFReader

    r = WDFReader(str(p))
    z = np.asarray(r.zpos, dtype=float)[: int(r.count)]
    from importlib.metadata import version

    return float(z.max() - z.min()), f"renishawWiRE {version('renishawWiRE')} zpos"


def wdf_z_range_stdlib(p: Path) -> tuple[float, str]:
    b = p.read_bytes()
    pos, blocks = 0, {}
    while pos + 16 <= len(b):
        name, size = b[pos : pos + 4].decode("latin-1"), struct.unpack_from("<Q", b, pos + 8)[0]
        blocks.setdefault(name, pos)
        if size == 0:
            break
        pos += size
    capacity, count = struct.unpack_from("<QQ", b, blocks["WDF1"] + 0x40)
    o = blocks["ORGN"]
    at = o + 20
    for _ in range(struct.unpack_from("<I", b, o + 16)[0]):
        ty = struct.unpack_from("<I", b, at)[0] & 0x7FFFFFFF
        if ty == 5:
            vals = struct.unpack_from(f"<{count}d", b, at + 24)
            return max(vals) - min(vals), "struct: ORGN list of data type 5 (Spatial Z)"
        at += 24 + capacity * 8
    raise KeyError("no Z list")


# ---------------------------------------------------------------- facts and questions


def facts(Fact: type, path: Callable[..., Path]) -> list:
    xls = lambda cid: path(f"{cid}-xls", "oracle-export")  # noqa: E731
    return [
        Fact(
            NPFF,
            "undetermined_reactions",
            "reactions (well x target) the vendor export marks Undetermined",
            lambda: export_undetermined(xls(NPFF)),
            cross=lambda: eds_ct_at_cycles(path(NPFF)),
        ),
        Fact(
            NPFF,
            "cidea_undetermined",
            "Cidea reactions the vendor export marks Undetermined",
            lambda: export_undetermined(xls(NPFF), "Cidea"),
            cross=lambda: eds_ct_at_cycles(path(NPFF), "Cidea"),
        ),
        Fact(
            NPFF,
            "cidea_mean_ct",
            "mean CT of target Cidea over its wells with a numeric CT (vendor export)",
            lambda: export_mean_ct(xls(NPFF), "Cidea"),
            cross=lambda: eds_mean_ct(path(NPFF), "Cidea"),
            cross_rel=1e-6,
        ),
        Fact(
            QS35,
            "inconclusive_reactions",
            "reactions with Amp Status `Inconclusive` in the vendor export",
            lambda: export_amp_status(xls(QS35), "Inconclusive"),
            cross=lambda: eds_amp_status(path(QS35), "0"),
        ),
        Fact(
            QS12K,
            "undetermined_wells",
            "well positions the vendor export marks Undetermined",
            lambda: export_undetermined_wells(xls(QS12K)),
            cross=lambda: eds_undetermined_wells(path(QS12K)),
        ),
        Fact(
            HARMONY6,
            "pixel_size_um",
            "image pixel size (Maps/Map/Entry ImageResolutionX, m → µm)",
            lambda: harmony_pixel_um(path(HARMONY6)),
            cross=lambda: bf_pixel_um(HARMONY6),
        ),
        Fact(
            HARMONY6,
            "channel_4_name",
            "name of ChannelID 4 (Maps/Map/Entry ChannelName)",
            lambda: harmony_channel_name(path(HARMONY6), 4),
            cross=lambda: bf_channel_name(HARMONY6, 4),
        ),
        Fact(
            GEN5_HL,
            "d4_concentration",
            "Gen5's [Concentration] of well D4",
            lambda: gen5_concentration(path(GEN5_HL), "D4"),
            cross=lambda: gen5_concentration_refit(path(GEN5_HL), "D4"),
            cross_rel=5e-3,
        ),
        Fact(
            LCD,
            "uv_points",
            "points of [LC Chromatogram(Detector A-Ch1)] in the ASCII export",
            lambda: lcd_points(path(f"{LCD}-txt", "oracle-export"), "LC Chromatogram(Detector A-Ch1)"),
            cross=lambda: lcd_rows(path(f"{LCD}-txt", "oracle-export"), "LC Chromatogram(Detector A-Ch1)"),
        ),
        Fact(
            LCD,
            "uv_wavelength_nm",
            "detection wavelength of Detector A-Ch1 in the ASCII export",
            lambda: lcd_wavelength(path(f"{LCD}-txt", "oracle-export"), "LC Chromatogram(Detector A-Ch1)"),
        ),
        Fact(
            LCD,
            "uv_mv_at_30_min",
            "Detector A-Ch1 signal at 30.000 min, mV (ASCII export)",
            lambda: lcd_value_mv(path(f"{LCD}-txt", "oracle-export"), "LC Chromatogram(Detector A-Ch1)", 30.0),
        ),
    ]


def specs(g: Any) -> list:
    src = "facts-analysis: {} ({})"
    return [
        g.Spec(
            "ana-qpcr-eds-undetermined-count",
            NPFF,
            "analysis",
            "In this qPCR run, how many reactions (well × target) gave no Ct at all, i.e. the instrument "
            "software reports them as Undetermined (no amplification)?",
            lambda f, m: g.integer(f["undetermined_reactions"]),
            src.format("undetermined_reactions", "vendor Results export; Ct = cycle count in the .eds agrees"),
            answer_hint="the number of reactions",
        ),
        g.Spec(
            "ana-qpcr-eds-undetermined-target",
            NPFF,
            "analysis",
            "How many of the Cidea reactions on this plate are Undetermined (no Ct)?",
            lambda f, m: g.integer(f["cidea_undetermined"]),
            src.format("cidea_undetermined", "vendor Results export; the .eds agrees"),
            answer_hint="the number of reactions",
        ),
        g.Spec(
            "ana-qpcr-eds-mean-ct-excl-undetermined",
            NPFF,
            "analysis",
            "What is the mean Ct of the Cidea assay over all of its wells on this plate, leaving out the wells "
            "where it did not amplify (Undetermined)?",
            lambda f, m: g.number(round(f["cidea_mean_ct"], 4), None, abs_=0.01),
            src.format("cidea_mean_ct", "vendor Results export; the .eds agrees"),
            answer_hint="a number (cycles)",
        ),
        g.Spec(
            "ana-qpcr-eds-inconclusive-count",
            QS35,
            "analysis",
            "The instrument software grades each amplification curve (amplified, inconclusive, no "
            "amplification). How many reactions on this plate are graded inconclusive?",
            lambda f, m: g.integer(f["inconclusive_reactions"]),
            src.format("inconclusive_reactions", "vendor Results export `Amp Status`; the .eds agrees"),
            answer_hint="the number of reactions",
        ),
        g.Spec(
            "ana-qpcr-eds-undetermined-well",
            QS12K,
            "analysis",
            "Which well of this QuantStudio 12K Flex run has no Ct (reported as Undetermined)?",
            lambda f, m: g.string(f["undetermined_wells"][0]),
            src.format("undetermined_wells", "vendor Results export; the .eds agrees"),
            answer_hint="the well, e.g. B7",
        ),
        g.Spec(
            "ana-hcs-harmony6-pixel-size",
            HARMONY6,
            "analysis",
            "This is the plate index of an Operetta CLS screening plate exported by Harmony. What is the pixel "
            "size of its images in micrometres?",
            lambda f, m: g.number(round(f["pixel_size_um"], 5), "µm", rel=0.001),
            src.format("pixel_size_um", "the index's Maps parsed with ElementTree; Bio-Formats agrees"),
            answer_hint="a length in µm",
        ),
        g.Spec(
            "ana-hcs-harmony6-channel-name",
            HARMONY6,
            "analysis",
            "What is the name of the fourth channel (channel ID 4) of this screening plate?",
            lambda f, m: g.string(f["channel_4_name"]),
            src.format("channel_4_name", "the index's Maps parsed with ElementTree; Bio-Formats agrees"),
            answer_hint="the channel name",
        ),
        g.Spec(
            "ana-plate-gen5-headerless-concentration",
            GEN5_HL,
            "analysis",
            "This Gen5 plate-reader export has no file header, but its plate layout (standards with "
            "concentrations, blanks, samples) is embedded. Using a linear standard curve on the blank-corrected "
            "standards, what concentration does well D4 have (read off the curve, before any dilution "
            "correction)?",
            lambda f, m: g.number(round(f["d4_concentration"], 3), None, rel=0.01),
            src.format("d4_concentration", "Gen5's own [Concentration] row; a stdlib refit agrees within 0.5 %"),
            answer_hint="a concentration in the layout's units",
        ),
        g.Spec(
            "ana-chrom-shimadzu-uv-points",
            LCD,
            "analysis",
            "How many data points does the UV detector's chromatogram of this Shimadzu LabSolutions run have, "
            "as LabSolutions itself lists them?",
            lambda f, m: g.integer(f["uv_points"]),
            src.format("uv_points", "the LabSolutions ASCII export; its data rows agree"),
            answer_hint="the number of points",
        ),
        g.Spec(
            "ana-chrom-shimadzu-uv-wavelength",
            LCD,
            "analysis",
            "At what wavelength did the UV detector of this Shimadzu HPLC run record its chromatogram?",
            lambda f, m: g.number(f["uv_wavelength_nm"], "nm", abs_=0.5),
            src.format("uv_wavelength_nm", "the LabSolutions ASCII export"),
            answer_hint="a wavelength in nm",
        ),
        g.Spec(
            "ana-chrom-shimadzu-uv-signal-mv",
            LCD,
            "analysis",
            "What was the UV detector's signal, in mV, at retention time 30.000 min in this Shimadzu run?",
            lambda f, m: g.number(round(f["uv_mv_at_30_min"], 4), "mV", abs_=0.002),
            src.format("uv_mv_at_30_min", "the LabSolutions ASCII export, Intensity x Intensity Multiplier"),
            answer_hint="a signal in mV",
        ),
    ]


def spectro_facts(Fact: type, path: Callable[..., Path]) -> list:
    """The WiRE LiveTrack fact, in `evals/facts/spectroscopy.json` (evals/spectroscopy.py)."""
    return [
        Fact(
            WDF_LT,
            "z_range_um",
            "range of the LiveTrack Z stage position over the map, µm",
            lambda: wdf_z_range(path(WDF_LT)),
            cross=lambda: wdf_z_range_stdlib(path(WDF_LT)),
        ),
    ]


def spectro_specs(g: Any) -> list:
    """The WiRE LiveTrack question (spectroscopy family, `facts-spectroscopy:` source)."""
    return [
        g.Spec(
            "ana-spec-wdf-livetrack-z-range",
            WDF_LT,
            "analysis",
            "This Raman map was acquired with LiveTrack focus tracking, so the stage Z position changes from "
            "spectrum to spectrum. What is the range (maximum minus minimum) of the Z position over the map, in "
            "micrometres?",
            lambda f, m: g.number(round(f["z_range_um"], 3), "µm", rel=0.001),
            "facts-spectroscopy: z_range_um (renishawWiRE zpos; the ORGN list read with struct agrees)",
            answer_hint="a length in µm",
        ),
    ]
