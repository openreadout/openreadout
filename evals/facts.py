"""Extract experiment-level facts for the eval set from independent sources.

The experiment questions (sample identity, method, operator, instrument) need answers that do not
come from OpenReadout. This script reads them with plain Python from sources a third party wrote:

- the depositor's own mzML export of a Thermo `.raw` run (`role = "oracle-export"` in
  `corpus/manifest.toml`): the instrument model term, the software list and the run's start time
  stamp;
- the vendor's own text files inside directory datasets: Waters `_HEADER.TXT`, Agilent ChemStation
  `result.ini` and `acqmeth.txt`, Bruker TopSpin `acqus` and `pdata/1/title`;
- the text part of files that are text by specification: the FCS TEXT segment (keyword/value
  pairs), plate-reader text exports (BMG MARS CSV, Tecan i-control TXT);
- plain text and XML stored verbatim inside binary files, found by a byte search or a header
  offset and parsed here: the CZI metadata XML (at the file header's metadata position), the LIF
  XML header (UTF-16 after the 0x70/0x2A block header), the Shimadzu `File Property` XML records
  (`@StoX@` + hex values), and the per-device method text a Thermo `.raw` embeds (UTF-16).

The facts are written to `evals/facts/experiment.json`, which is committed: `generate.py` reads it
like the oracle JSON, so questions can be regenerated without the corpus. Run this after the corpus
changes (it needs the corpus files; `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`):

    uv run python facts.py          # rewrite evals/facts/experiment.json
    uv run python facts.py --check  # exit 1 if the committed facts differ from the files
"""

from __future__ import annotations

import argparse
import json
import os
import re
import struct
import sys
import tomllib
import xml.etree.ElementTree as ET
from collections.abc import Callable
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "corpus" / "manifest.toml"
OUT = Path(__file__).resolve().parent / "facts" / "experiment.json"


def corpus_dir() -> Path:
    env = os.environ.get("OPENREADOUT_CORPUS_DIR")
    return Path(env) if env else ROOT / "corpus" / "files"


def manifest_file(corpus_id: str, role: str = "input") -> str:
    with MANIFEST.open("rb") as fh:
        data = tomllib.load(fh)
    for e in data["file"]:
        if e["id"] == corpus_id and e.get("role", "input") == role:
            return e["filename"]
    raise KeyError(f"{corpus_id}: no {role} entry in corpus/manifest.toml")


def fact(value: object, where: str) -> dict:
    return {"value": value, "where": where}


# ---------------------------------------------------------------- extractors


def mzml_header(path: Path) -> dict:
    """Instrument model (the `referenceableParamGroup`/`instrumentConfiguration` cvParam that is not
    the serial number), the first `softwareList/software` (the acquisition software) and
    `run/@startTimeStamp`, reading only up to the first `<spectrum>`."""
    ns = "{http://psi.hupo.org/ms/mzml}"
    out: dict = {}
    for event, el in ET.iterparse(path, events=("start", "end")):
        tag = el.tag.removeprefix(ns)
        if event == "end" and tag == "software" and "software" not in out:
            names = [p.get("name") for p in el.iter(f"{ns}cvParam")]
            if names:
                out["software"] = fact(names[0], "mzML softwareList/software[0]/cvParam/@name")
                out["software_version"] = fact(el.get("version"), "mzML softwareList/software[0]/@version")
        if event == "start" and tag == "run":
            if el.get("startTimeStamp"):
                out["run_start"] = fact(el.get("startTimeStamp"), "mzML run/@startTimeStamp")
        if event == "start" and tag == "spectrum":
            break
        if event == "end" and tag == "referenceableParamGroup":
            for p in el.iter(f"{ns}cvParam"):
                if p.get("accession") == "MS:1000529":
                    out["instrument_serial"] = fact(
                        p.get("value"), "mzML referenceableParamGroup/cvParam[MS:1000529]/@value"
                    )
                elif "instrument_model" not in out:
                    out["instrument_model"] = fact(
                        p.get("name"),
                        f"mzML referenceableParamGroup/cvParam[{p.get('accession')}]/@name",
                    )
    return out


def waters_header(path: Path) -> dict:
    """`$$ Key: value` lines of a Waters `_HEADER.TXT`."""
    wanted = {
        "Acquired Name": "acquired_name",
        "Bottle Number": "bottle_number",
        "MS Method": "ms_method",
        "Job Code": "job_code",
        "Instrument": "instrument",
    }
    out: dict = {}
    for line in (path / "_HEADER.TXT").read_text(encoding="latin-1").splitlines():
        m = re.match(r"\$\$ ([^:]+):\s*(.*)$", line)
        if m and m.group(1) in wanted and m.group(2).strip():
            out[wanted[m.group(1)]] = fact(m.group(2).strip(), f"_HEADER.TXT `$$ {m.group(1)}:`")
    return out


def chemstation_result_ini(path: Path) -> dict:
    """`SampleLocation=Vial N` in the ChemStation data directory's `result.ini`."""
    text = (path / "result.ini").read_bytes().replace(b"\x00", b"").decode("latin-1")
    m = re.search(r"^SampleLocation=Vial\s+(\d+)", text, re.M)
    return {"vial": fact(int(m.group(1)), "result.ini [Header] SampleLocation")} if m else {}


def bruker_nmr(path: Path) -> dict:
    """`##OWNER=` and `##$PULPROG=` of `acqus`, and the first line of `pdata/1/title`."""
    out: dict = {}
    acqus = (path / "acqus").read_text(encoding="latin-1")
    m = re.search(r"^##OWNER=\s*(.*)$", acqus, re.M)
    if m:
        out["owner"] = fact(m.group(1).strip(), "acqus ##OWNER=")
    m = re.search(r"^##\$PULPROG=\s*<(.*)>", acqus, re.M)
    if m:
        out["pulse_program"] = fact(m.group(1).strip(), "acqus ##$PULPROG=")
    title = path / "pdata" / "1" / "title"
    if title.is_file():
        lines = [ln.strip() for ln in title.read_text(encoding="latin-1").splitlines() if ln.strip()]
        if lines:
            out["title"] = fact(lines[0], "pdata/1/title (first line)")
    return out


def fcs_text(path: Path) -> dict:
    """Keyword/value pairs of the primary FCS TEXT segment (FCS 3.1 § 3.2: offsets in the HEADER,
    the first byte is the delimiter; a doubled delimiter is an escaped one)."""
    raw = path.read_bytes()
    start, end = int(raw[10:18]), int(raw[18:26])
    text = raw[start : end + 1].decode("latin-1")
    delim = text[0]
    parts: list[str] = []
    cur = ""
    i = 1
    while i < len(text):
        ch = text[i]
        if ch == delim:
            if i + 1 < len(text) and text[i + 1] == delim:
                cur += delim
                i += 2
                continue
            parts.append(cur)
            cur = ""
        else:
            cur += ch
        i += 1
    kv = {parts[j].upper(): parts[j + 1] for j in range(0, len(parts) - 1, 2)}
    out: dict = {}
    for key, name in [
        ("$WELLID", "well_id"),
        ("TUBE NAME", "tube_name"),
        ("$OP", "operator"),
        ("$SMNO", "specimen"),
        ("$COM", "comment"),
    ]:
        if kv.get(key, "").strip():
            out[name] = fact(kv[key].strip(), f"FCS TEXT keyword {key}")
    return out


def bmg_csv(path: Path) -> dict:
    """`ID1: …` and `Test name: …` cells of a BMG MARS table export."""
    out: dict = {}
    for line in path.read_text(encoding="latin-1").splitlines()[:12]:
        for cell in line.split(","):
            if cell.startswith("ID1: "):
                out["id1"] = fact(cell[5:].strip(), "CSV header cell `ID1:`")
            if cell.startswith("Test name: "):
                out["test_name"] = fact(cell[11:].strip(), "CSV header cell `Test name:`")
    return out


def tecan_txt(path: Path) -> dict:
    """The `User` row of a Tecan i-control text export."""
    for line in path.read_text(encoding="utf-8-sig").splitlines()[:40]:
        cells = [c for c in line.split("\t") if c.strip()]
        if cells and cells[0] == "User" and len(cells) > 1:
            return {"user": fact(cells[1].strip(), "TXT header row `User`")}
    return {}


def czi_xml(path: Path) -> dict:
    """The metadata XML of a CZI: the u64 metadata-segment position at byte 92 of the file header
    (`docs/formats/czi.md`), the XML size at the segment payload's start (segment header 32 bytes),
    the UTF-8 XML 256 bytes further on; `Information/Document/UserName` and the microscope's
    `System`."""
    with path.open("rb") as fh:
        (meta,) = struct.unpack("<Q", fh.read(100)[92:100])
        fh.seek(meta + 32)
        (size,) = struct.unpack("<I", fh.read(4))
        fh.seek(meta + 32 + 256)
        root = ET.fromstring(fh.read(size).decode("utf-8"))
    info = root.find("Metadata/Information")
    out: dict = {}
    user = (info.findtext("Document/UserName") or "").strip()
    if user:
        out["user_name"] = fact(user, "CZI metadata XML Information/Document/UserName")
    system = (info.findtext("Instrument/Microscopes/Microscope/System") or "").strip()
    if system:
        out["microscope_system"] = fact(system, "CZI metadata XML Information/Instrument/Microscopes/Microscope/System")
    return out


def lif_xml(path: Path) -> dict:
    """The LIF XML header (u32 0x70, u32 length, byte 0x2A, u32 character count, UTF-16LE XML):
    the LAS AF scanner record `SystemType` and the filter records `Objective` and
    `NumericalAperture`."""
    with path.open("rb") as fh:
        test, _, mark = struct.unpack("<iib", fh.read(9))
        if test != 0x70 or mark != 0x2A:
            raise SystemExit(f"{path}: not a LIF header")
        (n,) = struct.unpack("<i", fh.read(4))
        root = ET.fromstring(fh.read(2 * n).decode("utf-16-le"))
    out: dict = {}
    for r in root.iter("ScannerSettingRecord"):
        if r.get("Identifier") == "SystemType" and "system_type" not in out:
            out["system_type"] = fact(r.get("Variant"), "LIF XML ScannerSettingRecord[@Identifier=SystemType]/@Variant")
    for r in root.iter("FilterSettingRecord"):
        attr = r.get("Attribute")
        if attr == "Objective" and "objective" not in out:
            out["objective"] = fact(
                " ".join(r.get("Variant", "").split()), "LIF XML FilterSettingRecord[@Attribute=Objective]/@Variant"
            )
        if attr == "NumericalAperture" and "numerical_aperture" not in out:
            out["numerical_aperture"] = fact(
                float(r.get("Variant")), "LIF XML FilterSettingRecord[@Attribute=NumericalAperture]/@Variant"
            )
    return out


def chemstation_acqmeth(path: Path) -> dict:
    """The GC oven program, injection volume and the MSD serial number of a ChemStation `.D`
    directory's `acqmeth.txt` (Latin-1 text)."""
    text = (path / "acqmeth.txt").read_text(encoding="latin-1")
    out: dict = {}
    m = re.search(r"Oven Program\s+On\s*\n\s*([\d.]+) \xb0C for ([\d.]+) min", text)
    if m:
        out["oven_start_c"] = fact(float(m.group(1)), "acqmeth.txt `Oven Program On`, first step")
    ramps = re.findall(r"then ([\d.]+) \xb0C/min to ([\d.]+) \xb0C for ([\d.]+) min", text)
    if ramps:
        out["oven_ramp_c_per_min"] = fact(float(ramps[0][0]), "acqmeth.txt first `then R \xb0C/min` step")
        out["oven_final_c"] = fact(float(ramps[-1][1]), "acqmeth.txt last `to T \xb0C` step")
    m = re.search(r"Injection Volume\s+([\d.]+) \xb5L", text)
    if m:
        out["injection_volume_ul"] = fact(float(m.group(1)), "acqmeth.txt `Injection Volume`")
    m = re.search(r"TUNE PARAMETERS for SN:\s*(\S+)", text)
    if m:
        out["ms_serial"] = fact(m.group(1), "acqmeth.txt `TUNE PARAMETERS for SN:`")
    return out


def shimadzu_property(path: Path) -> dict:
    """`<name>@StoX@HEX</name>` records of a LabSolutions file's `File Property` XML, found by a
    byte search (the values are the hex of the text)."""
    raw = path.read_bytes()
    out: dict = {}
    for tag, name in [("smpl_name", "sample_name"), ("methodfile", "method_file"), ("szVialNum", "vial")]:
        m = re.search(rb"<" + tag.encode() + rb">@StoX@([0-9A-Fa-f]*)</" + tag.encode() + rb">", raw)
        if m and m.group(1):
            out[name] = fact(
                bytes.fromhex(m.group(1).decode()).decode("latin-1"), f"File Property XML <{tag}> (@StoX@ hex)"
            )
    return out


def utf16_block(raw: bytes, marker: str, before: int = 0, after: int = 4000) -> tuple[int, str] | None:
    """The UTF-16LE text around the first occurrence of `marker`, with its byte offset."""
    i = raw.find(marker.encode("utf-16-le"))
    if i < 0:
        return None
    start = max(0, i - before)
    start += (i - start) % 2
    return i, raw[start : i + after].decode("utf-16-le", errors="replace")


def thermo_method_text(path: Path) -> dict:
    """The LC pump program in the per-device method text a Thermo `.raw` embeds (UTF-16LE), found
    by a byte search: the Accela `Pump 1 gradient table:` (with the `Solvent X:` lines above it),
    the EASY-nLC `Gradient:` table, or the Vanquish script's `PumpModule.Pump` settings. Steps are
    `[time_min, {channel: percent}, flow]` as written (flow unit in `flow_unit`)."""
    raw = path.read_bytes()
    out: dict = {}
    hit = utf16_block(raw, "Pump 1 gradient table:", before=1600)
    if hit:
        at, text = hit
        solvents = dict(re.findall(r"^Solvent ([A-D]):[ \t]+(\S[^\r\n]*?)\s*$", text, re.M))
        head = re.search(r"No\. Time\s+((?:[A-D]%\s+)+)(\S+/min)", text)
        chans = [c.rstrip("%") for c in head.group(1).split()]
        steps = []
        for row in re.findall(r"^\d+\s+([\d. ]+?)\s*$", text[head.end() :], re.M):
            cells = [float(x) for x in row.split()]
            if len(cells) != len(chans) + 2:
                break
            steps.append([cells[0], dict(zip(chans, cells[1:-1], strict=True)), cells[-1]])
        out["gradient_steps"] = fact(steps, f"UTF-16 method text `Pump 1 gradient table:` at byte {at}")
        out["gradient_flow_unit"] = fact(head.group(2), "gradient table header")
        out["solvents"] = fact(solvents, "UTF-16 method text `Solvent A:` … `Solvent D:` lines")
        return out
    hit = utf16_block(raw, "Mixture [%B]", before=200, after=1200)
    if hit:
        at, text = hit
        steps = []
        for mm, ss, flow, b in re.findall(r"^\s+(\d+):(\d\d)\s+\d+:\d\d\s+(\d+)\s+(\d+)\s*$", text, re.M):
            steps.append([int(mm) + int(ss) / 60, {"B": float(b)}, float(flow)])
        out["gradient_steps"] = fact(steps, f"UTF-16 method text `Gradient:` table (`Mixture [%B]` at byte {at})")
        out["gradient_flow_unit"] = fact("nl/min", "gradient table header `Flow [nl/min]`")
        return out
    hit = utf16_block(raw, "PumpModule.Pump.%B.Value", before=6000, after=4000)
    if hit:
        at, text = hit
        steps = []
        for t, body in re.findall(r"^([\d.]+) \[min\][^\n]*\n((?:[ \t]+[^\n]*\n)*)", text, re.M):
            b = re.search(r"PumpModule\.Pump\.%B\.Value: ([\d.]+) \[%\]", body)
            f = re.search(r"PumpModule\.Pump\.Flow\.Nominal: ([\d.]+) \[ml/min\]", body)
            if b:
                steps.append([float(t), {"B": float(b.group(1))}, float(f.group(1)) if f else None])
        out["gradient_steps"] = fact(steps, f"UTF-16 Vanquish script `PumpModule.Pump.%B.Value` (first at byte {at})")
        out["gradient_flow_unit"] = fact("ml/min", "script `Flow.Nominal` unit")
    return out


# corpus id -> (role of the file read, extractor, description)
SOURCES: dict[str, tuple[str, Callable[[Path], dict], str]] = {
    "mtbls755-hilic-dpoly": ("oracle-export", mzml_header, "xml.etree on the depositor's mzML header"),
    "mtbls1822-tsq-74": ("oracle-export", mzml_header, "xml.etree on the depositor's mzML header"),
    "mtbls3555-bv-ix-alpha-raw": ("input", waters_header, "Waters _HEADER.TXT inside the .raw directory"),
    "mtbls15166-brain-b1-raw": ("input", waters_header, "Waters _HEADER.TXT inside the .raw directory"),
    "entab-chemstation-mwd-d": ("input", chemstation_result_ini, "ChemStation result.ini inside the .D directory"),
    "nmrxiv-s846-50": ("input", bruker_nmr, "TopSpin acqus and pdata/1/title inside the experiment directory"),
    "nmrxiv-s837-21": ("input", bruker_nmr, "TopSpin acqus and pdata/1/title inside the experiment directory"),
    "flowio-g11": ("input", fcs_text, "FCS TEXT segment parsed per FCS 3.1"),
    "flowio-100715": ("input", fcs_text, "FCS TEXT segment parsed per FCS 3.1"),
    "fcsparser-fortessa-a01": ("input", fcs_text, "FCS TEXT segment parsed per FCS 3.1"),
    "bmg-mars-abs-384-qc": ("input", bmg_csv, "BMG MARS CSV export header"),
    "tecan-icontrol-f200-txt": ("input", tecan_txt, "Tecan i-control TXT export header"),
    "fcsparser-cyflow-cube-8": ("input", fcs_text, "FCS TEXT segment parsed per FCS 3.1"),
    "zenodo10577621-LineScan-T3500": ("input", czi_xml, "CZI metadata XML read at the header's metadata position"),
    "bsst749-2a-ishi-hf-fshr-dmso": ("input", lif_xml, "LIF XML header"),
    "mtbls75-x-fsfa-hl-gc-o7c-1-d": ("input", chemstation_acqmeth, "ChemStation acqmeth.txt inside the .D directory"),
    "zenodo13987390-chiral-11a-ad": ("input", shimadzu_property, "byte search for the File Property XML records"),
    "zenodo13950130-std-501": ("input", shimadzu_property, "byte search for the File Property XML records"),
    "mtbls404-QC1_001": ("input", thermo_method_text, "byte search for the UTF-16 instrument-method text"),
    "pxd000001-tmt-erwinia-01": ("input", thermo_method_text, "byte search for the UTF-16 instrument-method text"),
    "mtbls1820-lumos-uplc-31": ("input", thermo_method_text, "byte search for the UTF-16 instrument-method text"),
}


def extract() -> dict:
    out: dict = {}
    for cid, (role, fn, how) in SOURCES.items():
        rel = manifest_file(cid, role)
        path = corpus_dir() / rel
        if not path.exists():
            raise SystemExit(f"{cid}: {path} is missing; fetch the corpus first (cargo xtask corpus fetch)")
        out[cid] = {"file": rel, "extractor": f"evals/facts.py: {how}", "facts": fn(path)}
    return out


def render(facts: dict) -> str:
    return json.dumps(facts, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/experiment.json is out of date")
    args = ap.parse_args()
    text = render(extract())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/experiment.json is out of date; run evals/facts.py", file=sys.stderr)
            return 1
        print("evals/facts/experiment.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {sum(len(v['facts']) for v in json.loads(text).values())} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
