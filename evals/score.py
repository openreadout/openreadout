"""Deterministic scoring for the OpenReadout eval set.

The agent ends its reply with a line `ANSWER: <answer>`. This module parses that line and compares
it with the typed answer of the question (evals/questions/*.jsonl):

- number: unit-aware (µm/nm/Å/mm, s/ms/min/h, Hz/kHz/MHz, K/°C, A/mA/µA/nA/pA, V/mV/µV, Ω/kΩ/MΩ,
  molar M/mM/µM/nM/pM and mol/L, mass concentration g/L/mg/mL/µg/mL/ng/mL/pg/mL/% w/v, mass
  g/mg/µg/ng, angles in degrees (° or °2θ), ratios fold/x/%) with an absolute and/or relative
  tolerance. Case decides between symbols that differ only by case (nM nanomolar vs nm
  nanometre, mM vs mm, MΩ vs mΩ); a spelling that is neither (`NM`) is read in the expected
  unit's dimension. A bare number is read in the expected unit (the report flags it); for a
  ratio a number labelled "fold"/"ratio" wins over an earlier one (a log2 or ΔΔCq value).
- string: case- and punctuation-insensitive match of any accepted variant as a whole word; a
  `reject` variant anywhere makes the answer wrong (hedging "positive or negative" fails).
- boolean: yes/no, true/false, intact/truncated, complete/incomplete, ...
- list: every expected item (or one of its variants) must appear; a `reject` item anywhere makes
  the answer wrong (listing every candidate is not an answer).
- date: ISO or written dates, within `tolerance_days`.

Conversion tasks are also checked on disk: the produced OME-TIFF is opened with tifffile and its
OME-XML compared with the oracle; an OME-Zarr store's NGFF metadata (0.4 or 0.5, Zarr v2 or v3) is
read as JSON and one chunk decoded with imagecodecs; a CSV is read with Python's csv module
(`check_task`; a task may name several `outputs`, all of which must pass).

Usage:
    python evals/score.py answer QUESTION_ID "ANSWER: 0.5 µm"        # score one answer
    python evals/score.py report RECORDS.jsonl [--out PREFIX]         # JSON + Markdown report
    python evals/score.py compare WITH.json WITHOUT.json --out FILE   # side-by-side summary
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import math
import re
import sys
import unicodedata
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path
from typing import Any

EVALS = Path(__file__).resolve().parent
QUESTIONS_DIR = EVALS / "questions"

# ---------------------------------------------------------------- questions


def load_questions(directory: Path = QUESTIONS_DIR) -> dict[str, dict]:
    out: dict[str, dict] = {}
    for path in sorted(directory.glob("*.jsonl")):
        for line in path.read_text().splitlines():
            if line.strip():
                q = json.loads(line)
                out[q["id"]] = q
    return out


# ---------------------------------------------------------------- answer line

ANSWER_RE = re.compile(r"^[\s>*_`#-]*answer[\s*_`]*[:：][\s*_`]*(.*?)[\s*_`]*$", re.IGNORECASE)


def extract_answer(text: str | None) -> str | None:
    """The text after the last `ANSWER:` line, or None when the agent gave no such line."""
    if not text:
        return None
    found = None
    for line in text.splitlines():
        m = ANSWER_RE.match(line)
        if m:
            found = m.group(1).strip()
    if found is None:
        return None
    return found


def normalize(text: str) -> str:
    # Trademark signs go before NFKC, which would turn "™" into "TM".
    text = text.replace("™", "").replace("®", "").replace("×", "x").replace("−", "-")
    text = unicodedata.normalize("NFKC", text)
    text = text.lower()
    text = re.sub(r"(\d)\s+x\b", r"\1x", text)  # "100 x" -> "100x"
    text = re.sub(r"[\"'`*_]", "", text)
    return re.sub(r"\s+", " ", text).strip()


def contains_word(haystack: str, needle: str) -> bool:
    n = normalize(needle)
    if not n:
        return False
    return re.search(r"(?<![a-z0-9])" + re.escape(n) + r"(?![a-z0-9])", haystack) is not None


# ---------------------------------------------------------------- units

# unit -> (dimension, factor to the base unit, offset); value_base = value * factor + offset.
# `UNITS` is matched case-insensitively (keys are lower case); `CASED_UNITS` only in the exact
# case written, for symbols whose case is their meaning: nM (nanomolar) vs nm (nanometre), mM vs
# mm, µM vs µm, M (molar) vs m (metre), MΩ vs mΩ. A spelling that is no exact key (`NM`, `UM`)
# is read as whichever of its case-insensitive matches has the dimension the question expects.
UNITS: dict[str, tuple[str, float, float]] = {}
CASED_UNITS: dict[str, tuple[str, float, float]] = {}


def _add(dimension: str, factor: float, *names: str, offset: float = 0.0) -> None:
    for name in names:
        UNITS[name] = (dimension, factor, offset)


def _add_cased(dimension: str, factor: float, *names: str) -> None:
    for name in names:
        CASED_UNITS[name] = (dimension, factor, 0.0)


_add("length", 1e6, "m", "meter", "meters", "metre", "metres")
_add("length", 1e3, "mm", "millimeter", "millimeters", "millimetre", "millimetres")
_add(
    "length",
    1.0,
    "µm",
    "μm",
    "um",
    "micron",
    "microns",
    "micrometer",
    "micrometers",
    "micrometre",
    "micrometres",
)
_add("length", 1e-3, "nm", "nanometer", "nanometers", "nanometre", "nanometres")
_add(
    "length",
    1e-4,
    "å",
    "Å",
    "a",
    "angstrom",
    "angstroms",
    "ångström",
    "ångströms",
    "ångstrom",
    "ångstroms",
)
_add("length", 1e-6, "pm", "picometer", "picometers", "picometre", "picometres")
_add("time", 1.0, "s", "sec", "secs", "second", "seconds")
_add("time", 1e-3, "ms", "msec", "millisecond", "milliseconds")
_add("time", 1e-6, "µs", "μs", "us", "microsecond", "microseconds")
_add("time", 60.0, "min", "mins", "minute", "minutes")
_add("time", 3600.0, "h", "hr", "hrs", "hour", "hours")
_add("frequency", 1.0, "hz", "hertz", "samples/s", "sample/s", "samples/sec", "sps")
_add("frequency", 1e3, "khz", "kilohertz", "ksps", "ks/s")
_add("frequency", 1e6, "mhz", "megahertz")
_add("frequency", 1e9, "ghz", "gigahertz")
_add("temperature", 1.0, "k", "kelvin")
_add("temperature", 1.0, "°c", "ºc", "c", "degc", "celsius", "degrees celsius", offset=273.15)
_add("current", 1.0, "a", "amp", "amps", "ampere", "amperes")
_add("current", 1e-3, "ma", "milliamp", "milliamps")
_add("current", 1e-6, "µa", "μa", "ua", "microamp", "microamps")
_add("current", 1e-9, "na", "nanoamp", "nanoamps")
_add("current", 1e-12, "pa", "picoamp", "picoamps", "picoampere", "picoamperes")
_add("voltage", 1.0, "v", "volt", "volts")
_add("voltage", 1e-3, "mv", "millivolt", "millivolts")
_add("voltage", 1e-6, "µv", "μv", "uv", "microvolt", "microvolts")
# Wavenumbers and Raman shifts: "2919.9 cm-1", "cm⁻¹", "1/cm" (the unit token stops at the "-1").
_add("wavenumber", 1.0, "cm-1", "cm^-1", "cm⁻¹", "1/cm", "/cm", "cm", "wavenumber", "wavenumbers")
# Resistance (base: ohm). MΩ (mega) and mΩ (milli) differ only by case.
_add("resistance", 1.0, "ohm", "ohms", "ω", "Ω")
_add("resistance", 1e3, "kohm", "kohms", "kω", "kΩ", "kiloohm", "kiloohms", "kilohm", "kilohms")
_add("resistance", 1e6, "megaohm", "megaohms", "megohm", "megohms")
_add("resistance", 1e9, "gohm", "gohms", "gω", "gΩ", "gigaohm", "gigaohms")
_add_cased("resistance", 1e6, "MOhm", "MOhms", "Mohm", "Mohms", "MOHM", "MΩ", "MΩ")
_add_cased("resistance", 1e-3, "mOhm", "mOhms", "mΩ", "mΩ", "milliohm", "milliohms")
# Molar concentration (base: mol/L). The SI symbols are case-sensitive.
_add_cased("molar", 1.0, "M")
_add_cased("molar", 1e-3, "mM")
_add_cased("molar", 1e-6, "µM", "μM", "uM")
_add_cased("molar", 1e-9, "nM")
_add_cased("molar", 1e-12, "pM")
_add_cased("molar", 1e-15, "fM")
_add("molar", 1.0, "mol/l", "mol/dm3", "molar")
_add("molar", 1e-3, "mmol/l", "millimolar")
_add("molar", 1e-6, "µmol/l", "μmol/l", "umol/l", "micromolar")
_add("molar", 1e-9, "nmol/l", "nanomolar")
_add("molar", 1e-12, "pmol/l", "picomolar")
# Mass concentration (base: g/L = mg/mL). 1 % w/v = 1 g per 100 mL.
_add("mass_concentration", 1.0, "g/l", "mg/ml", "µg/µl", "μg/μl", "ug/ul")
_add("mass_concentration", 1e3, "g/ml")
_add("mass_concentration", 1e-3, "mg/l", "µg/ml", "μg/ml", "ug/ml", "ng/µl", "ng/μl", "ng/ul")
_add("mass_concentration", 1e-6, "µg/l", "μg/l", "ug/l", "ng/ml", "pg/µl", "pg/μl", "pg/ul")
_add("mass_concentration", 1e-9, "ng/l", "pg/ml")
_add("mass_concentration", 1e-2, "mg/dl")
_add("mass_concentration", 10.0, "%w/v", "g/100ml", "g/dl")
# Dimensionless ratios: fold changes, "x", percentages (base: a plain ratio).
_add("ratio", 1.0, "fold", "-fold", "x", "×", "times", "ratio")
_add("ratio", 1e-2, "%", "percent", "pct")
# Plate-reader luminescence: arbitrary units that only match themselves.
_add("rlu", 1.0, "rlu", "rlus")
# Mass (base: mg): sample masses of thermal-analysis runs.
_add("mass", 1e3, "g", "gram", "grams")
_add("mass", 1.0, "mg", "milligram", "milligrams")
_add("mass", 1e-3, "µg", "μg", "ug", "microgram", "micrograms")
_add("mass", 1e-6, "ng", "nanogram", "nanograms")
# Angles (base: degree): diffraction angles 2θ. `°C` is a temperature (its own token); a bare
# degree asked as a temperature is read as °C (`to_base`).
DEGREES = ("°", "º", "deg", "degs", "degree", "degrees")
_add("angle", 1.0, *DEGREES)

# Ambiguous one-letter units are only read as the dimension the question expects.
AMBIGUOUS = {"a": ("length", "current"), "c": ("temperature",)}

# Words after the unit that do not change it: "0.5 µm per pixel", "11.4 Å/voxel".
PER_SUFFIX = re.compile(r"^(?:/|per\s+)(?:pixel|px|voxel|pix|sample)s?\b", re.IGNORECASE)

NUMBER_RE = re.compile(
    r"(?<![A-Za-z0-9.])"  # not the 2 in "MS2" or the 1 in "FL1"
    r"(?P<num>[-+−]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?(?:[eE][-+−]?\d+)?|[-+−]?\.\d+)"
    r"(?:\s*(?:x|×|\*)\s*10\s*(?:\^|\*\*)?\s*(?P<exp>[-+−]?\d+|[⁻⁺]?[⁰¹²³⁴⁵⁶⁷⁸⁹]+))?"
)

SUPERSCRIPT = str.maketrans("⁰¹²³⁴⁵⁶⁷⁸⁹⁻⁺", "0123456789-+")

UNIT_TOKEN = re.compile(
    r"\s*(?P<unit>degrees\s+celsius|samples?/s(?:ec)?|ks/s|°\s*c|º\s*c|deg\s*c"
    r"|%\s*\(?\s*w\s*/\s*v\s*\)?|%|-fold|×"
    r"|[a-zµμåÅ°ΩΩ]+(?:/(?:100\s*)?[a-zµμ]+(?:3\b)?)?)",
    re.IGNORECASE,
)


def _candidates(unit: str) -> list[tuple[str, float, float]]:
    """Every unit entry the token can mean: its exact spelling when that is a unit (case decides:
    `nM` is nanomolar, `nm` nanometre), else every case-insensitive match (`NM`: either)."""
    exact = CASED_UNITS.get(unit) or UNITS.get(unit)
    if exact is not None:
        return [exact]
    low = unit.lower()
    out: list[tuple[str, float, float]] = []
    for key, entry in (*CASED_UNITS.items(), *UNITS.items()):
        if key.lower() == low and entry not in out:
            out.append(entry)
    return out


def _unit_key(token: str) -> str:
    """The unit token as written, with spacing variants folded (`° C`, `% (w/v)`, `g/100 mL`)."""
    t = re.sub(r"\s+", " ", token).strip()
    t = t.replace("° c", "°c").replace("° C", "°C").replace("º c", "ºc").replace("º C", "ºC")
    if re.fullmatch(r"deg ?c", t, re.IGNORECASE):
        return "degc"
    if t.startswith("%") and "w" in t.lower():
        return "%w/v"
    return t.replace("/100 ", "/100")


# The "2" of a diffraction angle's name is not a number: "°2θ" is the degree sign, "2θ = 43.1°"
# names the angle, "2-theta" too.
TWO_THETA = re.compile(r"(?:(?<=[°º])\s*|(?<![A-Za-z0-9.]))2\s*(?:[θΘ]|-?\s*theta\b)", re.IGNORECASE)


def parse_quantities(text: str) -> list[tuple[float, str | None]]:
    """Every (value, unit-or-None) pair in the text, in order. Units keep the case they were
    written in (it tells nM from nm)."""
    text = TWO_THETA.sub(" two-theta", text)
    out: list[tuple[float, str | None]] = []
    for m in NUMBER_RE.finditer(text):
        raw = m.group("num").replace(",", "").replace("−", "-")
        try:
            value = float(raw)
        except ValueError:
            continue
        if m.group("exp"):
            value *= 10 ** int(m.group("exp").translate(SUPERSCRIPT))
        rest = text[m.end() :]
        unit = None
        um = UNIT_TOKEN.match(rest)
        if um:
            key = _unit_key(um.group("unit"))
            if not _candidates(key) and "/" in key:
                head = key.split("/", 1)[0]
                if _candidates(head) and PER_SUFFIX.match(key[len(head) :]):
                    key = head
            if _candidates(key):
                unit = key
        out.append((value, unit))
    return out


def resolve_unit(unit: str, dimension: str | None = None) -> tuple[str, float, float] | None:
    """The unit entry a token means, an ambiguous spelling (`NM`, `UM`) read as the expected
    `dimension`; None when it is no unit of that dimension or stays ambiguous."""
    cands = _candidates(unit)
    if dimension is not None:
        cands = [e for e in cands if e[0] == dimension]
    return cands[0] if len(cands) == 1 else None


def to_base(value: float, unit: str, dimension: str) -> float | None:
    key = unit if unit in UNITS or unit in CASED_UNITS else unit.lower()
    if key in AMBIGUOUS and dimension not in AMBIGUOUS[key]:
        return None
    if key == "a":  # "A" is both ampere and a lazily typed ångström
        return value if dimension == "current" else value * 1e-4
    if key in ("c", *DEGREES) and dimension == "temperature":
        return value + 273.15
    entry = resolve_unit(unit, dimension)
    if entry is None:
        return None
    return value * entry[1] + entry[2]


def unit_dimension(unit: str) -> str:
    entry = resolve_unit(unit)
    if entry is None:
        raise KeyError(f"unknown or ambiguous unit {unit!r}; add it to score.UNITS or score.CASED_UNITS")
    return entry[0]


# ---------------------------------------------------------------- scoring


def within(got: float, expected: float, tol: dict) -> bool:
    allowed = max(tol.get("abs", 0.0), tol.get("rel", 0.0) * abs(expected))
    return math.isclose(got, expected, rel_tol=0.0, abs_tol=allowed + 1e-12)


# Words just before a bare number that mark it as the ratio asked for ("fold change 2.46" after a
# log2 or ΔΔCq value).
RATIO_LABEL = re.compile(r"(?:fold|ratio|\brq\b|relative quantity|relative expression)[^0-9]{0,25}$", re.IGNORECASE)


def _labelled_ratio(text: str) -> float | None:
    """The first unit-less number right after a fold/ratio label, if any."""
    for m in NUMBER_RE.finditer(text):
        if not RATIO_LABEL.search(text[max(0, m.start() - 40) : m.start()]):
            continue
        if UNIT_TOKEN.match(text[m.end() :]) and _candidates(_unit_key(UNIT_TOKEN.match(text[m.end() :])["unit"])):
            continue
        value = float(m.group("num").replace(",", "").replace("−", "-"))
        if m.group("exp"):
            value *= 10 ** int(m.group("exp").translate(SUPERSCRIPT))
        return value
    return None


def score_number(spec: dict, text: str) -> dict:
    expected = float(spec["value"])
    unit = spec.get("unit")
    pairs = parse_quantities(text)
    if not pairs:
        return {"correct": False, "reason": "no number in the answer"}
    if unit is None:
        # A plain number: "12.5 %", "2.5x" and "3-fold" count as plain numbers here.
        def plain(u: str | None) -> bool:
            return u is None or resolve_unit(u, "ratio") is not None

        value, got_unit = next(((v, u) for v, u in pairs if plain(u)), pairs[0])
        ok = within(value, expected, spec["tolerance"])
        return {"correct": ok, "parsed": value, "unit": got_unit}
    dimension = unit_dimension(unit)
    expected_entry = resolve_unit(unit)
    assert expected_entry is not None
    for value, got_unit in pairs:
        if got_unit is None:
            continue
        base = to_base(value, got_unit, dimension)
        if base is None:
            continue
        # Tolerances are in the expected unit; compare in it.
        in_expected = (base - expected_entry[2]) / expected_entry[1]
        ok = within(in_expected, expected, spec["tolerance"])
        return {"correct": ok, "parsed": value, "unit": got_unit, "converted": in_expected}
    bare = [v for v, u in pairs if u is None]
    if bare:
        value = bare[0]
        if dimension == "ratio":
            labelled = _labelled_ratio(text)
            if labelled is not None:
                value = labelled
        ok = within(value, expected, spec["tolerance"])
        return {"correct": ok, "parsed": value, "unit": None, "unit_assumed": unit}
    return {
        "correct": False,
        "reason": f"no quantity in a unit of {dimension}",
        "parsed": pairs[0][0],
    }


def score_string(spec: dict, text: str) -> dict:
    norm = normalize(text)
    for bad in spec.get("reject", []):
        if contains_word(norm, bad):
            return {"correct": False, "reason": f"contains rejected {bad!r}"}
    for good in spec["accept"]:
        if contains_word(norm, good):
            return {"correct": True, "matched": good}
    return {"correct": False, "reason": "no accepted variant found"}


NEGATIVE = [
    "no",
    "not",
    "false",
    "truncated",
    "incomplete",
    "corrupt",
    "corrupted",
    "damaged",
    "broken",
    "partial",
    "partially",
    "missing",
    "invalid",
    "cut off",
    "cut short",
]
POSITIVE = ["yes", "true", "intact", "complete", "ok", "okay", "valid", "fine", "undamaged"]


def parse_boolean(text: str) -> bool | None:
    norm = normalize(text)
    first = re.split(r"[^a-z]+", norm.strip(), maxsplit=1)[0] if norm else ""
    if first in ("yes", "true"):
        return True
    if first in ("no", "false"):
        return False
    if any(contains_word(norm, w) for w in NEGATIVE):
        return False
    if any(contains_word(norm, w) for w in POSITIVE):
        return True
    return None


def score_boolean(spec: dict, text: str) -> dict:
    got = parse_boolean(text)
    if got is None:
        return {"correct": False, "reason": "not a yes/no answer"}
    return {"correct": got == bool(spec["value"]), "parsed": got}


def score_list(spec: dict, text: str) -> dict:
    norm = normalize(text)
    for bad in spec.get("reject", []):
        if contains_word(norm, bad):
            return {"correct": False, "reason": f"contains rejected {bad!r}"}
    missing = [variants[0] for variants in spec["accept"] if not any(contains_word(norm, v) for v in variants)]
    return {"correct": not missing, "missing": missing}


MONTHS = {
    m: i
    for i, names in enumerate(
        [
            ("jan", "january"),
            ("feb", "february"),
            ("mar", "march"),
            ("apr", "april"),
            ("may",),
            ("jun", "june"),
            ("jul", "july"),
            ("aug", "august"),
            ("sep", "sept", "september"),
            ("oct", "october"),
            ("nov", "november"),
            ("dec", "december"),
        ],
        start=1,
    )
    for m in names
}


def parse_dates(text: str) -> list[dt.date]:
    out: list[dt.date] = []
    norm = normalize(text)

    def add(y: int, mo: int, d: int) -> None:
        try:
            out.append(dt.date(y, mo, d))
        except ValueError:
            pass

    for m in re.finditer(r"(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})", norm):
        add(int(m.group(1)), int(m.group(2)), int(m.group(3)))
    for m in re.finditer(r"(\d{1,2})(?:st|nd|rd|th)?\s+([a-z]+)\.?,?\s+(\d{4})", norm):
        if m.group(2) in MONTHS:
            add(int(m.group(3)), MONTHS[m.group(2)], int(m.group(1)))
    for m in re.finditer(r"([a-z]+)\.?\s+(\d{1,2})(?:st|nd|rd|th)?,?\s+(\d{4})", norm):
        if m.group(1) in MONTHS:
            add(int(m.group(3)), MONTHS[m.group(1)], int(m.group(2)))
    for m in re.finditer(r"(?<![\d-])(\d{1,2})[/.](\d{1,2})[/.](\d{4})", norm):
        a, b, y = int(m.group(1)), int(m.group(2)), int(m.group(3))
        add(y, a, b)  # month/day/year
        add(y, b, a)  # day/month/year: ambiguous numeric dates accept either reading
    return out


def score_date(spec: dict, text: str) -> dict:
    expected = dt.date.fromisoformat(spec["value"])
    dates = parse_dates(text)
    if not dates:
        return {"correct": False, "reason": "no date in the answer"}
    tol = int(spec.get("tolerance_days", 0))
    best = min(dates, key=lambda d: abs((d - expected).days))
    return {"correct": abs((best - expected).days) <= tol, "parsed": best.isoformat()}


# ---------------------------------------------------------------- visual tier: boxes, points, choices

# A thousands separator is a comma followed by exactly three digits ("1,200"); "1200, 3400" and
# "(1200,3400)" stay two numbers.
_THOUSANDS = re.compile(r"(?<=\d),(?=\d{3}(?!\d))")
_NUM = r"-?\d+(?:\.\d+)?"
COORD_KEYS = {
    "x": ("x", "x0", "xmin", "x_min", "left"),
    "y": ("y", "y0", "ymin", "y_min", "top"),
    "w": ("width", "w"),
    "h": ("height", "h"),
    "x1": ("x1", "x2", "xmax", "x_max", "right"),
    "y1": ("y1", "y2", "ymax", "y_max", "bottom"),
}


def _coord_text(text: str) -> str:
    return _THOUSANDS.sub("", normalize(text).replace("−", "-"))


def _labelled(text: str) -> dict[str, float]:
    """Coordinates written as `x = 1200`, `width: 300`, `x0=…`, `right=…` (the first of each)."""
    out: dict[str, float] = {}
    for key, names in COORD_KEYS.items():
        for name in names:
            m = re.search(rf"(?<![a-z0-9_]){re.escape(name)}\s*(?:=|:|≈|~|is)\s*({_NUM})", text)
            if m:
                out[key] = float(m.group(1))
                break
    return out


def parse_point(text: str) -> tuple[float, float] | None:
    """(x, y) from `x=…, y=…`, a parenthesised pair `(x, y)` or the first two numbers."""
    t = _coord_text(text)
    lab = _labelled(t)
    if "x" in lab and "y" in lab:
        return lab["x"], lab["y"]
    m = re.search(rf"\(\s*({_NUM})\s*[,;]\s*({_NUM})\s*\)", t)
    if m:
        return float(m.group(1)), float(m.group(2))
    nums = [float(n) for n in re.findall(_NUM, t)]
    return (nums[0], nums[1]) if len(nums) >= 2 else None


def parse_boxes(text: str) -> list[tuple[str, list[float]]]:
    """Candidate readings of a box as [x, y, width, height]: labelled width/height or corner
    coordinates, else the first four numbers read both as x, y, width, height and (when valid) as
    the corners x0, y0, x1, y1."""
    t = _coord_text(text)
    lab = _labelled(t)
    if {"x", "y", "w", "h"} <= set(lab):
        return [("xywh", [lab["x"], lab["y"], lab["w"], lab["h"]])]
    if {"x", "y", "x1", "y1"} <= set(lab):
        return [("corners", [lab["x"], lab["y"], lab["x1"] - lab["x"], lab["y1"] - lab["y"]])]
    nums = [float(n) for n in re.findall(_NUM, t)][:4]
    if len(nums) < 4:
        return []
    a, b, c, d = nums
    out = [("xywh", [a, b, c, d])]
    if c > a and d > b:
        out.append(("corners", [a, b, c - a, d - b]))
    return out


def box_iou(a: list[float], b: list[float]) -> float:
    ix = max(0.0, min(a[0] + a[2], b[0] + b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[1] + a[3], b[1] + b[3]) - max(a[1], b[1]))
    inter = ix * iy
    union = max(a[2], 0) * max(a[3], 0) + b[2] * b[3] - inter
    return inter / union if union > 0 else 0.0


def score_bbox(spec: dict, text: str) -> dict:
    """Correct when the box overlaps the ground truth with IoU ≥ `min_iou`; four bare numbers are
    read as x, y, width, height and, when that reading fails, as corners (the better one counts)."""
    cands = parse_boxes(text)
    if not cands:
        return {"correct": False, "reason": "no box (x, y, width, height) in the answer"}
    scored = [(box_iou(b, spec["value"]), how, b) for how, b in cands]
    best = max(scored, key=lambda s: s[0])
    return {"correct": best[0] >= spec["min_iou"], "iou": round(best[0], 3), "read_as": best[1], "parsed": best[2]}


def in_mask(mask: dict, x: float, y: float) -> bool:
    sx, sy = mask["scale"]
    mx, my = math.floor(x / sx), math.floor(y / sy)
    return any(r == my and x0 <= mx < x1 for r, x0, x1 in mask["runs"])


def score_point(spec: dict, text: str) -> dict:
    p = parse_point(text)
    if p is None:
        return {"correct": False, "reason": "no point (x, y) in the answer"}
    x, y = p
    dist = math.hypot(x - spec["value"][0], y - spec["value"][1])
    inside = bool(spec.get("mask")) and in_mask(spec["mask"], x, y)
    near = spec.get("radius") is not None and dist <= spec["radius"]
    return {"correct": inside or near, "parsed": [x, y], "inside_mask": inside, "distance": round(dist, 1)}


def _choice_norm(text: str) -> str:
    return re.sub(r"\s+", " ", re.sub(r"[_\-–]+", " ", normalize(text))).strip()


def named_options(spec: dict, text: str) -> list[str]:
    """The options the answer names: every alias found as a whole word, longest first, a shorter
    alias inside an already matched span not counted (`00000` inside `bead_…_00000_00005`)."""
    norm = _choice_norm(text)
    found: list[tuple[int, int, str]] = []
    aliases = sorted(
        ((_choice_norm(a), o) for o, names in spec["options"].items() for a in names),
        key=lambda t: -len(t[0]),
    )
    for alias, option in aliases:
        if not alias:
            continue
        for m in re.finditer(r"(?<![a-z0-9])" + re.escape(alias) + r"(?![a-z0-9])", norm):
            if not any(s < m.end() and m.start() < e for s, e, _ in found):
                found.append((m.start(), m.end(), option))
    seen: list[str] = []
    for _, _, o in sorted(found):
        if o not in seen:
            seen.append(o)
    return seen


# A clause that sets options aside ("…; only the top-left corner was scanned") names them without
# choosing them.
_CLAUSE_SPLIT = re.compile(r";|\.\s|\s[—–]\s|(?=\b(?:while|whereas|but|except|unlike)\b)")
_CONTRAST = re.compile(r"\b(?:only|not|except|but|while|whereas|unlike)\b", re.I)


def chosen_options(spec: dict, text: str) -> list[str]:
    """The options the answer chooses: those named outside clauses that contrast with the
    choice; every option named when that leaves none."""
    chosen: list[str] = []
    for clause in _CLAUSE_SPLIT.split(text):
        if not _CONTRAST.search(clause):
            chosen += [o for o in named_options(spec, clause) if o not in chosen]
    return chosen or named_options(spec, text)


def score_choice(spec: dict, text: str) -> dict:
    """Single: at least one option chosen and every chosen option accepted. Multiple: the chosen
    options are exactly the expected set."""
    named = chosen_options(spec, text)
    if not named:
        return {"correct": False, "reason": "names none of the options"}
    want = set(spec["value"])
    ok = set(named) == want if spec.get("multiple") else set(named) <= want
    return {"correct": ok, "named": named}


SCORERS = {
    "number": score_number,
    "string": score_string,
    "boolean": score_boolean,
    "list": score_list,
    "date": score_date,
    "bbox": score_bbox,
    "point": score_point,
    "choice": score_choice,
}


def score_answer(answer_spec: dict, final_text: str | None) -> dict:
    """Score the agent's final reply against a typed answer. Never raises on agent text."""
    answer = extract_answer(final_text)
    if answer is None:
        return {"correct": False, "answer": None, "reason": "no ANSWER: line"}
    if normalize(answer) in ("unknown", "n/a", "none", ""):
        return {"correct": False, "answer": answer, "reason": "answered unknown"}
    result = SCORERS[answer_spec["type"]](answer_spec, answer)
    result["answer"] = answer
    return result


# ---------------------------------------------------------------- conversion tasks

OME_NS = re.compile(r"^\{[^}]*\}")


def check_ome_tiff(path: Path, expect: dict) -> dict:
    import tifffile  # the independent reader (BSD-3); imported lazily so scoring text needs no numpy

    details: dict[str, Any] = {}
    try:
        with tifffile.TiffFile(path) as tif:
            xml = tif.ome_metadata
            if not xml:
                return {"ok": False, "reason": "not an OME-TIFF (no OME-XML in the first IFD)"}
            root = ET.fromstring(xml)
            pixels = next((el for el in root.iter() if OME_NS.sub("", el.tag) == "Pixels"), None)
            if pixels is None:
                return {"ok": False, "reason": "OME-XML has no Pixels element"}
            series = tif.series[0]
            # Decode one plane to prove the pixel data is readable, not just the header.
            tif.pages[0].asarray()
            details["series_shape"] = list(series.shape)
    except Exception as exc:
        return {"ok": False, "reason": f"tifffile could not read it: {type(exc).__name__}: {exc}"}
    ok = True
    for key, want in expect.items():
        raw = pixels.get(key)
        if key.startswith("PhysicalSize"):
            if want is None:
                continue
            unit = pixels.get(key + "Unit", "µm")
            try:
                got = float(raw) if raw is not None else None
            except ValueError:
                got = None
            conv = to_base(got, unit, "length") if got is not None else None
            good = conv is not None and within(conv, float(want), {"rel": 0.01})
            details[key] = {"got": raw, "unit": unit, "want_um": want, "ok": good}
        else:
            good = raw is not None and int(raw) == int(want)
            details[key] = {"got": raw, "want": want, "ok": good}
        ok = ok and good
    return {"ok": ok, **details}


def _is_number(field: str) -> bool:
    field = field.strip()
    if field == "":
        return True
    try:
        float(field)
        return True
    except ValueError:
        return field.lower() in ("nan", "inf", "-inf")


def check_csv(path: Path, expect: dict) -> dict:
    try:
        text = path.read_text(errors="replace")
    except OSError as exc:
        return {"ok": False, "reason": f"cannot read: {exc}"}
    sample = text[:20000]
    try:
        dialect = csv.Sniffer().sniff(sample, delimiters=",;\t")
        delimiter = dialect.delimiter
    except csv.Error:
        delimiter = ","
    rows = list(csv.reader(text.splitlines(), delimiter=delimiter))
    rows = [r for r in rows if any(c.strip() for c in r)]
    numeric = [r for r in rows if all(_is_number(c) for c in r)]
    header_rows = len(rows) - len(numeric)
    details: dict[str, Any] = {
        "delimiter": delimiter,
        "header_rows": header_rows,
        "numeric_rows": len(numeric),
    }
    ok = True
    if "rows" in expect:
        good = len(numeric) == int(expect["rows"])
        details["rows"] = {"got": len(numeric), "want": expect["rows"], "ok": good}
        ok = ok and good
    if "columns" in expect and numeric:
        ncols = len(numeric[0])
        # A leading index column (pandas' default) is tolerated.
        good = ncols in (int(expect["columns"]), int(expect["columns"]) + 1)
        details["columns"] = {"got": ncols, "want": expect["columns"], "ok": good}
        ok = ok and good
    if "first_value" in expect and numeric:
        tol = float(expect.get("first_value_tolerance", 0.0))
        firsts = [float(c) for c in numeric[0] if c.strip()]
        good = any(abs(v - float(expect["first_value"])) <= tol for v in firsts)
        details["first_value"] = {
            "got_row": numeric[0][:6],
            "want": expect["first_value"],
            "ok": good,
        }
        ok = ok and good
    if not numeric:
        ok = False
        details["reason"] = "no numeric rows"
    return {"ok": ok, **details}


def check_jcamp(path: Path, expect: dict) -> dict:
    """A JCAMP-DX file read with the jcamp package (MIT), independent of OpenReadout: the number of
    points, the ends of the x axis (either order) and the largest y value."""
    try:
        import jcamp
    except ImportError:
        return {"ok": False, "reason": "the jcamp package is not installed"}
    try:
        d = jcamp.jcamp_readfile(str(path)) if hasattr(jcamp, "jcamp_readfile") else jcamp.readfile(str(path))
    except Exception as exc:
        return {"ok": False, "reason": f"jcamp could not read it: {exc}"}
    x = [float(v) for v in d.get("x", [])]
    y = [float(v) for v in d.get("y", [])]
    details: dict[str, Any] = {"points": len(y)}
    ok = bool(y) and len(x) == len(y)
    if "points" in expect:
        good = len(y) == int(expect["points"])
        details["points_ok"] = good
        ok = ok and good
    if x and "first_x" in expect and "last_x" in expect:
        want = sorted([float(expect["first_x"]), float(expect["last_x"])])
        got = sorted([x[0], x[-1]])
        good = all(abs(a - b) <= 1e-3 * max(1.0, abs(b)) for a, b in zip(got, want, strict=True))
        details["x_range"] = {"got": got, "want": want, "ok": good}
        ok = ok and good
    if y and "max_y" in expect:
        want = float(expect["max_y"])
        good = abs(max(y) - want) <= 1e-4 * max(1e-12, abs(want))
        details["max_y"] = {"got": max(y), "want": want, "ok": good}
        ok = ok and good
    return {"ok": ok, **details}


def _zarr_json(store: Path, rel: str) -> dict | None:
    """Group or array metadata at `rel` inside a Zarr store: v3 `zarr.json` (attributes) or v2
    `.zattrs` / `.zarray` merged into one dict with a `zarr_format` key."""
    base = store / rel if rel else store
    v3 = base / "zarr.json"
    if v3.is_file():
        return json.loads(v3.read_text())
    out: dict[str, Any] = {}
    for name in (".zgroup", ".zarray", ".zattrs"):
        p = base / name
        if p.is_file():
            d = json.loads(p.read_text())
            if name == ".zattrs":
                out["attributes"] = d
            else:
                out.update(d)
    return out or None


def _find_multiscales(store: Path, depth: int = 3) -> tuple[str, dict] | None:
    """The first group (breadth-first, up to `depth` levels: bioformats2raw puts the image at `0/`)
    whose attributes hold NGFF `multiscales` (0.5: under `ome`; 0.4: at the top)."""
    todo = [""]
    for _ in range(depth + 1):
        nxt = []
        for rel in todo:
            meta = _zarr_json(store, rel)
            attrs = (meta or {}).get("attributes") or {}
            ms = (attrs.get("ome") or {}).get("multiscales") or attrs.get("multiscales")
            if ms:
                return rel, ms[0]
            base = store / rel if rel else store
            nxt += [f"{rel}/{p.name}".lstrip("/") for p in sorted(base.iterdir()) if p.is_dir()]
        todo = nxt
    return None


UNIT_TO_UM = {
    "micrometer": 1.0,
    "micron": 1.0,
    "um": 1.0,
    "µm": 1.0,
    "nanometer": 1e-3,
    "nm": 1e-3,
    "millimeter": 1e3,
    "mm": 1e3,
    "meter": 1e6,
    "m": 1e6,
    "angstrom": 1e-4,
}


def _decode_first_chunk(array_dir: Path, meta: dict) -> tuple[bool, str]:
    """Decode one stored chunk of a Zarr array with imagecodecs (v2 compressors, or v3 `bytes` plus
    one compressor without sharding). Stores whose codecs this cannot undo only need a non-empty chunk."""
    import imagecodecs
    import numpy as np

    files = [p for p in array_dir.rglob("*") if p.is_file() and not p.name.startswith(".") and p.name != "zarr.json"]
    files = [p for p in files if p.stat().st_size > 0]
    if not files:
        return False, "no chunk data written"
    raw = sorted(files)[0].read_bytes()
    if meta.get("zarr_format") == 3:
        codecs = [c.get("name") for c in meta.get("codecs", [])]
        comp = [c for c in codecs if c not in ("bytes", "transpose")]
        dtype = np.dtype(meta.get("data_type", "uint8"))
        endian = next(
            (c.get("configuration", {}).get("endian") for c in meta.get("codecs", []) if c.get("name") == "bytes"),
            "little",
        )
        dtype = dtype.newbyteorder("<" if endian == "little" else ">")
    else:
        comp = [(meta.get("compressor") or {}).get("id")] if meta.get("compressor") else []
        dtype = np.dtype(meta.get("dtype", "|u1"))
    decoders = {
        "zstd": imagecodecs.zstd_decode,
        "blosc": imagecodecs.blosc_decode,
        "gzip": imagecodecs.gzip_decode,
        "zlib": imagecodecs.zlib_decode,
        "lz4": imagecodecs.lz4_decode,
    }
    if len(comp) > 1 or (comp and comp[0] not in decoders):
        return True, f"chunk present; codecs {comp} not decoded here"
    data = decoders[comp[0]](raw) if comp else raw
    n = int(
        np.prod(
            meta.get("chunk_grid", {}).get("configuration", {}).get("chunk_shape")
            or meta.get("chunks")
            or [len(data) // dtype.itemsize]
        )
    )
    ok = len(data) == n * dtype.itemsize
    return ok, f"decoded {len(data)} bytes ({'=' if ok else '!='} {n} × {dtype.itemsize})"


def check_ome_zarr(path: Path, expect: dict) -> dict:
    """An OME-Zarr image read without OpenReadout: NGFF multiscales axes and the level-0 scale
    (PhysicalSizeX/Y/Z in µm, 1 % tolerance, axis units converted), the level-0 array's sizes, and
    one decoded chunk."""
    if not path.is_dir():
        return {"ok": False, "reason": "not a directory store"}
    try:
        found = _find_multiscales(path)
    except (OSError, ValueError) as exc:
        return {"ok": False, "reason": f"unreadable metadata: {exc}"}
    if found is None:
        return {"ok": False, "reason": "no NGFF multiscales metadata"}
    rel, ms = found
    axes = [a["name"] if isinstance(a, dict) else a for a in ms.get("axes", [])]
    units = {a["name"]: a.get("unit") for a in ms.get("axes", []) if isinstance(a, dict)}
    ds = ms["datasets"][0]
    scale = next((t["scale"] for t in ds.get("coordinateTransformations", []) if t.get("type") == "scale"), None)
    arr_rel = f"{rel}/{ds['path']}".lstrip("/")
    meta = _zarr_json(path, arr_rel) or {}
    shape = meta.get("shape")
    details: dict[str, Any] = {"group": rel or "/", "axes": axes, "scale": scale, "shape": shape}
    ok = bool(axes) and scale is not None and shape is not None and len(axes) == len(scale) == len(shape)
    if not ok:
        return {"ok": False, "reason": "axes, scale and array shape do not line up", **details}
    for key, want in expect.items():
        ax = key[-1].lower()
        if ax not in axes:
            good = want in (None, 1) and key.startswith("Size")
            details[key] = {"got": None, "want": want, "ok": good}
        elif key.startswith("Size"):
            got = shape[axes.index(ax)]
            good = int(got) == int(want)
            details[key] = {"got": got, "want": want, "ok": good}
        else:
            if want is None:
                continue
            factor = UNIT_TO_UM.get((units.get(ax) or "micrometer").lower())
            got = float(scale[axes.index(ax)]) * factor if factor is not None else None
            good = got is not None and within(got, float(want), {"rel": 0.01})
            details[key] = {"got": scale[axes.index(ax)], "unit": units.get(ax), "want_um": want, "ok": good}
        ok = ok and good
    try:
        decoded, how = _decode_first_chunk(path / arr_rel, meta)
    except Exception as exc:  # an agent's store can be anything; never raise while scoring
        decoded, how = False, f"{type(exc).__name__}: {exc}"
    details["chunk"] = how
    return {"ok": ok and decoded, **details}


def _check_one(workdir: Path, kind: str, output: str, expect: dict) -> dict:
    path = workdir / output
    if not path.exists():
        # Accept the file anywhere under the working directory if the agent picked a subfolder.
        hits = sorted(workdir.rglob(output))
        if not hits:
            return {"ok": False, "reason": f"{output} was not written"}
        path = hits[0]
    if kind == "ome-tiff":
        return check_ome_tiff(path, expect)
    if kind == "ome-zarr":
        return check_ome_zarr(path, expect)
    if kind == "csv":
        return check_csv(path, expect)
    if kind == "jcamp":
        return check_jcamp(path, expect)
    raise ValueError(f"unknown task kind {kind}")


def check_task(question: dict, workdir: Path) -> dict:
    """Inspect the file(s) a conversion task asked for, with readers independent of OpenReadout."""
    task = question["task"]
    if "outputs" in task:
        results = {o["output"]: _check_one(workdir, task["kind"], o["output"], o["expect"]) for o in task["outputs"]}
        return {"ok": all(r["ok"] for r in results.values()), "outputs": results}
    return _check_one(workdir, task["kind"], task["output"], task["expect"])


def score_record(question: dict, record: dict) -> dict:
    """Combine the answer score and (for tasks) the artifact check into one verdict."""
    result = score_answer(question["answer"], record.get("final_text"))
    if "task" in question:
        artifact = record.get("artifact")
        result["artifact"] = artifact
        if artifact is None:
            result["correct"] = False
            result["reason"] = (result.get("reason", "") + "; output file not checked").strip("; ")
        elif not artifact.get("ok"):
            result["correct"] = False
            result["reason"] = (result.get("reason", "") + "; output file failed its check").strip("; ")
    return result


# ---------------------------------------------------------------- reports


def expected_text(answer: dict) -> str:
    t = answer["type"]
    if t == "number":
        tol = answer["tolerance"]
        tol_s = " ± " + (f"{tol['abs']:g}" if "abs" in tol else f"{tol['rel'] * 100:g} %") if any(tol.values()) else ""
        return f"{answer['value']:.6g}{' ' + answer['unit'] if answer.get('unit') else ''}{tol_s}"
    if t == "list":
        return ", ".join(answer["value"])
    if t == "boolean":
        return "yes" if answer["value"] else "no"
    if t == "date":
        tol = answer.get("tolerance_days", 0)
        return answer["value"] + (f" ± {tol} d" if tol else "")
    if t == "bbox":
        x, y, w, h = answer["value"]
        return f"x={x}, y={y}, width={w}, height={h} (IoU ≥ {answer['min_iou']:g})"
    if t == "point":
        x, y = answer["value"]
        return f"x={x}, y={y}" + (" (inside the mask)" if answer.get("mask") else f" (± {answer['radius']:g})")
    if t == "choice":
        v = answer["value"]
        return ", ".join(v) if answer.get("multiple") or len(v) == 1 else "one of " + ", ".join(v)
    return str(answer["value"])


def _bucket() -> dict[str, float]:
    return {"n": 0, "correct": 0}


def looked(rec: dict) -> bool:
    """Did the run look at the data as a picture? A tool result handed the model an image
    (`images_seen`), or it called a preview (MCP tool or `openreadout preview`) or Read an image
    file (`look_calls`, run.py). Records written before those fields: an MCP preview call in
    `tool_calls`."""
    if rec.get("images_seen") or any((rec.get("look_calls") or {}).values()):
        return True
    return any(name.endswith("openreadout_preview") for name in rec.get("tool_calls") or {})


def summarize(records: list[dict], questions: dict[str, dict]) -> dict:
    overall = _bucket()
    by_cat: dict[str, dict] = defaultdict(_bucket)
    by_fam: dict[str, dict] = defaultdict(_bucket)
    cost = 0.0
    tokens = defaultdict(int)
    turns: list[int] = []
    wall: list[float] = []
    used_tool = 0
    by_look: dict[str, dict] = {"looked": _bucket(), "did_not_look": _bucket()}
    visual_look: dict[str, dict] = {"looked": _bucket(), "did_not_look": _bucket()}
    per_q = []
    for rec in records:
        q = questions.get(rec["id"])
        if q is None:
            continue
        verdict = score_record(q, rec)
        ok = bool(verdict["correct"])
        saw = looked(rec)
        key = "looked" if saw else "did_not_look"
        buckets = [overall, by_cat[q["category"]], by_fam[q["family"]], by_look[key]]
        if q["category"] == "visual":
            buckets.append(visual_look[key])
        for bucket in buckets:
            bucket["n"] += 1
            bucket["correct"] += int(ok)
        cost += rec.get("cost_usd") or 0.0
        for k, v in (rec.get("usage") or {}).items():
            if isinstance(v, int):
                tokens[k] += v
        if rec.get("num_turns") is not None:
            turns.append(rec["num_turns"])
        if rec.get("wall_s") is not None:
            wall.append(rec["wall_s"])
        used_tool += int(bool(rec.get("used_openreadout")))
        per_q.append(
            {
                "id": rec["id"],
                "family": q["family"],
                "category": q["category"],
                "correct": ok,
                "answer": verdict.get("answer"),
                "expected": expected_text(q["answer"]),
                "verdict": verdict,
                "cost_usd": rec.get("cost_usd"),
                "num_turns": rec.get("num_turns"),
                "wall_s": rec.get("wall_s"),
                "stop": rec.get("subtype"),
                "used_openreadout": rec.get("used_openreadout"),
                "looked": saw,
                "contamination": rec.get("contamination") or [],
            }
        )

    def rate(b: dict) -> dict:
        return {**b, "score": round(b["correct"] / b["n"], 3) if b["n"] else None}

    n = len(per_q)
    return {
        "overall": rate(overall),
        "by_category": {k: rate(v) for k, v in sorted(by_cat.items())},
        "by_family": {k: rate(v) for k, v in sorted(by_fam.items())},
        "cost": {
            "total_usd": round(cost, 4),
            "mean_usd": round(cost / n, 4) if n else None,
            "tokens": dict(tokens),
            "mean_turns": round(sum(turns) / len(turns), 2) if turns else None,
            "total_wall_s": round(sum(wall), 1),
            "mean_wall_s": round(sum(wall) / len(wall), 1) if wall else None,
        },
        "used_openreadout": used_tool,
        # runs that looked at the data as a picture, and accuracy with and without looking
        "looking": {k: rate(v) for k, v in by_look.items()},
        "looking_visual": {k: rate(v) for k, v in visual_look.items()},
        "questions": per_q,
    }


def _pct(b: dict) -> str:
    return f"{b['correct']}/{b['n']} ({b['score'] * 100:.0f} %)" if b["n"] else "–"


def _looked_line(looking: dict | None) -> str:
    """`k/n; correct when looking a/k, when not b/m`."""
    if not looking:
        return "–"
    yes, no = looking["looked"], looking["did_not_look"]
    n = yes["n"] + no["n"]
    if not n:
        return "–"
    return f"{yes['n']}/{n}; correct when looking {_pct(yes)}, when not {_pct(no)}"


def markdown_report(meta: dict, summary: dict) -> str:
    lines = [
        f"# Eval run: {meta.get('model', '?')}, condition `{meta.get('condition', '?')}`",
        "",
        f"- Date: {meta.get('date', '?')}",
        f"- Questions scored: {summary['overall']['n']}",
        f"- Score: **{_pct(summary['overall'])}**",
        f"- Cost: ${summary['cost']['total_usd']:.2f} total, ${summary['cost']['mean_usd'] or 0:.3f} per question",
        f"- Turns per question (mean): {summary['cost']['mean_turns']}",
        f"- Wall time: {summary['cost']['total_wall_s']} s total, {summary['cost']['mean_wall_s']} s per question",
        f"- Tokens: {', '.join(f'{k} {v:,}' for k, v in sorted(summary['cost']['tokens'].items())) or '–'}",
        f"- Runs that called OpenReadout (CLI or MCP): {summary['used_openreadout']}",
        f"- Runs that looked at the data as a picture: {_looked_line(summary.get('looking'))}",
        f"- Visual tier, runs that looked: {_looked_line(summary.get('looking_visual'))}",
    ]
    for key in (
        "claude_version",
        "openreadout_version",
        "max_turns",
        "max_budget_usd_per_question",
        "timeout_s",
    ):
        if key in meta:
            lines.append(f"- {key.replace('_', ' ')}: {meta[key]}")
    lines += ["", "## By category", "", "| category | score |", "| --- | --- |"]
    lines += [f"| {k} | {_pct(v)} |" for k, v in summary["by_category"].items()]
    lines += ["", "## By family", "", "| family | score |", "| --- | --- |"]
    lines += [f"| {k} | {_pct(v)} |" for k, v in summary["by_family"].items()]
    lines += [
        "",
        "## Questions",
        "",
        "| id | ok | answer | expected | turns | cost $ | s | stop |",
        "| --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for q in summary["questions"]:
        ans = (q["answer"] or "–").replace("|", "\\|")
        if len(ans) > 60:
            ans = ans[:57] + "..."
        cost = f"{q['cost_usd']:.3f}" if q["cost_usd"] is not None else "–"
        wall = f"{q['wall_s']:.0f}" if q["wall_s"] is not None else "–"
        flag = "yes" if q["correct"] else "**no**"
        if q["contamination"]:
            flag += " (leak?)"
        lines.append(
            f"| {q['id']} | {flag} | {ans} | {q['expected'].replace('|', '/')} | {q['num_turns'] or '–'} "
            f"| {cost} | {wall} | {q['stop'] or '–'} |"
        )
    return "\n".join(lines) + "\n"


def load_records(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def write_report(records: list[dict], questions: dict[str, dict], meta: dict, prefix: Path) -> dict:
    summary = summarize(records, questions)
    report = {"meta": meta, **summary}
    prefix.parent.mkdir(parents=True, exist_ok=True)
    prefix.with_suffix(".json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    prefix.with_suffix(".md").write_text(markdown_report(meta, summary))
    return report


def compare_markdown(reports: list[dict]) -> str:
    conds = [r["meta"].get("condition", "?") for r in reports]
    lines = [
        f"# Eval comparison: {reports[0]['meta'].get('model', '?')}",
        "",
        "| | " + " | ".join(f"`{c}`" for c in conds) + " |",
        "| --- |" + " --- |" * len(conds),
        "| score | " + " | ".join(_pct(r["overall"]) for r in reports) + " |",
        "| cost (USD) | " + " | ".join(f"{r['cost']['total_usd']:.2f}" for r in reports) + " |",
        "| mean turns | " + " | ".join(str(r["cost"]["mean_turns"]) for r in reports) + " |",
        "| mean wall time (s) | " + " | ".join(str(r["cost"]["mean_wall_s"]) for r in reports) + " |",
        "| output tokens | " + " | ".join(f"{r['cost']['tokens'].get('output_tokens', 0):,}" for r in reports) + " |",
        "| looked at the data (runs; accuracy looking / not) | "
        + " | ".join(_looked_line(r.get("looking")) for r in reports)
        + " |",
        "| visual tier: looked (runs; accuracy looking / not) | "
        + " | ".join(_looked_line(r.get("looking_visual")) for r in reports)
        + " |",
        "",
        "## By category",
        "",
        "| category | " + " | ".join(conds) + " |",
        "| --- |" + " --- |" * len(conds),
    ]
    cats = sorted({k for r in reports for k in r["by_category"]})
    for c in cats:
        lines.append(f"| {c} | " + " | ".join(_pct(r["by_category"].get(c, _bucket())) for r in reports) + " |")
    lines += [
        "",
        "## By family",
        "",
        "| family | " + " | ".join(conds) + " |",
        "| --- |" + " --- |" * len(conds),
    ]
    fams = sorted({k for r in reports for k in r["by_family"]})
    for f in fams:
        lines.append(f"| {f} | " + " | ".join(_pct(r["by_family"].get(f, _bucket())) for r in reports) + " |")
    lines += [
        "",
        "## Per question",
        "",
        "| id | " + " | ".join(conds) + " |",
        "| --- |" + " --- |" * len(conds),
    ]
    by_id = [{q["id"]: q for q in r["questions"]} for r in reports]
    for qid in sorted({i for m in by_id for i in m}):
        cells = []
        for m in by_id:
            q = m.get(qid)
            cells.append("–" if q is None else ("yes" if q["correct"] else "no"))
        lines.append(f"| {qid} | " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    p_ans = sub.add_parser("answer", help="score one answer text against a question")
    p_ans.add_argument("question_id")
    p_ans.add_argument("text")
    p_rep = sub.add_parser("report", help="write <prefix>.json and <prefix>.md from a records file")
    p_rep.add_argument("records", type=Path)
    p_rep.add_argument("--out", type=Path, help="output prefix (default: the records path without .records)")
    p_cmp = sub.add_parser("compare", help="side-by-side Markdown of several report JSON files")
    p_cmp.add_argument("reports", type=Path, nargs="+")
    p_cmp.add_argument("--out", type=Path)
    args = parser.parse_args(argv)
    questions = load_questions()
    if args.cmd == "answer":
        q = questions[args.question_id]
        print(json.dumps(score_answer(q["answer"], args.text), indent=2, ensure_ascii=False))
        return 0
    if args.cmd == "report":
        records = load_records(args.records)
        meta = records[0].get("meta", {}) if records else {}
        prefix = args.out or Path(str(args.records).removesuffix(".jsonl").removesuffix(".records"))
        report = write_report(records, questions, meta, prefix)
        print(f"{prefix}.md: {_pct(report['overall'])}")
        return 0
    reports = [json.loads(p.read_text()) for p in args.reports]
    text = compare_markdown(reports)
    if args.out:
        args.out.write_text(text)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
