"""Units in the number scorer: concentrations (molar and mass), case-sensitive symbols (nM vs nm,
mM vs mm, MΩ vs mΩ), ratios and percentages, and every question's reference answer."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import generate
import score


def num(value, unit, rel=None, abs_=None):
    return generate.number(value, unit, rel=rel, abs_=abs_)


def correct(spec: dict, answer: str) -> bool:
    return score.score_answer(spec, f"ANSWER: {answer}")["correct"]


# ---------------------------------------------------------------- molar concentrations


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("25 nM", True),
        ("25.4 nM", True),
        ("0.025 µM", True),
        ("0.025 μM", True),  # Greek mu
        ("0.025 uM", True),
        ("2.5e-5 mM", True),
        ("2.5 × 10^-8 M", True),
        ("25 nmol/L", True),
        ("25 nanomolar", True),
        ("IC50 = 25 nM (95 % CI 18-33 nM)", True),
        ("25 NM", True),  # all caps: read as the expected dimension
        ("25", True),  # bare number read in the expected unit
        ("25 µM", False),
        ("25 mM", False),
        ("25 pM", False),
        ("25 nm", False),  # nanometres are no concentration
        ("450 nm read; IC50 = 25 nM", True),  # the wavelength is skipped, not read as nanomolar
        ("450 nm read; IC50 = 0.025 µM", True),
    ],
)
def test_molar(answer, ok):
    assert correct(num(25.0, "nM", rel=0.05), answer) is ok, answer


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("3.2 mM", True),
        ("3200 µM", True),
        ("3.2 mmol/L", True),
        ("0.0032 M", True),
        ("3.2 mm", False),  # millimetres
        ("3.2 MM", True),  # ambiguous spelling, read as the expected dimension
        ("3.2 µM", False),
    ],
)
def test_millimolar_vs_millimetre(answer, ok):
    assert correct(num(3.2, "mM", rel=0.02), answer) is ok, answer


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("254 nm", True),
        ("0.254 µm", True),
        ("254 nM", False),  # nanomolar is no length
        ("254 NM", True),
        ("2.54 mm", False),
    ],
)
def test_lengths_keep_their_case(answer, ok):
    assert correct(num(254, "nm", abs_=0.5), answer) is ok, answer


def test_micrometre_vs_micromolar():
    assert correct(num(1.08, "µm", rel=0.01), "1.08 µm")
    assert not correct(num(1.08, "µm", rel=0.01), "1.08 µM")
    assert correct(num(1.08, "µM", rel=0.01), "1.08 µM")
    assert correct(num(1.08, "µM", rel=0.01), "1080 nM")
    assert not correct(num(1.08, "µM", rel=0.01), "1.08 µm")
    assert correct(num(1.08, "µM", rel=0.01), "1.08 UM")


def test_expected_units_have_the_right_dimension():
    assert score.unit_dimension("nM") == "molar"
    assert score.unit_dimension("nm") == "length"
    assert score.unit_dimension("mM") == "molar"
    assert score.unit_dimension("mm") == "length"
    assert score.unit_dimension("M") == "molar"
    assert score.unit_dimension("m") == "length"
    assert score.unit_dimension("µM") == "molar"
    assert score.unit_dimension("MOhm") == "resistance"
    assert score.unit_dimension("ng/mL") == "mass_concentration"
    assert score.unit_dimension("%") == "ratio"
    assert score.unit_dimension("fold") == "ratio"
    with pytest.raises(KeyError):
        score.unit_dimension("NM")  # ambiguous: a question must say which
    with pytest.raises(KeyError):
        score.unit_dimension("furlong")


# ---------------------------------------------------------------- mass concentrations


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("12.5 ng/mL", True),
        ("12.5 ng/ml", True),
        ("0.0125 µg/mL", True),
        ("0.0125 ug/mL", True),
        ("12500 pg/mL", True),
        ("12.5 µg/L", True),
        ("12.5 ng/µL", False),  # a thousand times more
        ("12.5 nM", False),  # molar is not mass concentration
        ("12.5", True),
    ],
)
def test_mass_concentration(answer, ok):
    assert correct(num(12.5, "ng/mL", rel=0.02), answer) is ok, answer


def test_percent_weight_per_volume():
    spec = num(0.9, "%w/v", rel=0.02)
    assert correct(spec, "0.9 % w/v")
    assert correct(spec, "0.9% (w/v)")
    assert correct(spec, "9 g/L")
    assert correct(spec, "9 mg/mL")
    assert correct(spec, "0.9 g/100 mL")
    assert not correct(spec, "0.9 g/L")


# ---------------------------------------------------------------- resistance, RLU


def test_resistance_case():
    spec = num(104.14, "MOhm", rel=0.01)
    assert correct(spec, "104.1 MΩ")
    assert correct(spec, "104.1 MOhm")
    assert correct(spec, "104.1 megaohms")
    assert correct(spec, "0.1041 GΩ")
    assert correct(spec, "104140 kΩ")
    assert not correct(spec, "104.1 mΩ")  # milliohm
    assert correct(spec, "104.1")


def test_rlu_only_matches_itself():
    spec = num(4442.25, "RLU", rel=0.005)
    assert correct(spec, "4442 RLU")
    assert correct(spec, "4,442.3 RLU")
    assert correct(spec, "4442")
    assert not correct(spec, "4000 RLU")


# ---------------------------------------------------------------- ratios, fold changes, percent


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("2.46", True),
        ("2.46-fold", True),
        ("2.46 fold", True),
        ("2.46x", True),
        ("2.46 ×", True),
        ("2.46 times higher", True),
        ("246 %", True),
        ("log2 fold change 1.30 (2.46-fold)", True),  # the tagged fold wins
        ("ΔΔCq = -1.30, fold change 2.46", True),  # the labelled number wins
        ("ΔΔCq = -1.30", False),
        ("1.30-fold", False),
    ],
)
def test_fold_change(answer, ok):
    assert correct(num(2.46, "fold", rel=0.03), answer) is ok, answer


def test_percent_unit():
    spec = num(97.9, "%", abs_=0.5)
    assert correct(spec, "97.9 %")
    assert correct(spec, "97.9% (area normalisation)")
    assert correct(spec, "97.9 percent")
    assert correct(spec, "97.9")
    assert not correct(spec, "0.979")  # a fraction is not a percentage
    ratio = num(0.85, "ratio", abs_=0.01)
    assert correct(ratio, "0.85")
    assert correct(ratio, "85 %")


def test_plain_numbers_ignore_percent_and_fold_tags():
    """Questions without a unit read the first plain number, and "12.5 %" or "3x" still count as
    plain numbers there (as before the ratio units existed)."""
    spec = generate.number(12.5, None, abs_=0.1)
    assert correct(spec, "12.5% of 40 wells")
    spec = generate.integer(3)
    assert correct(spec, "3x")
    assert correct(spec, "3 wells (7.5 %)")


# ---------------------------------------------------------------- mass and angles (bench instruments)


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("2.7 mg", True),
        ("2.70 milligrams", True),
        ("0.0027 g", True),
        ("2700 µg", True),
        ("2700 ug", True),
        ("2.7", True),  # bare: read in mg
        ("2.7 g", False),
        ("2.7 µg", False),
        ("2.7 mg/mL", False),  # a mass concentration is no mass
    ],
)
def test_mass(answer, ok):
    assert correct(num(2.7, "mg", rel=0.001), answer) is ok, answer


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("43.06°", True),
        ("43.06 °", True),
        ("43.06 °2θ", True),
        ("43.06° 2θ", True),
        ("2θ = 43.06°", True),  # the 2 of 2θ is not a number
        ("2θ ≈ 43.06", True),
        ("2-theta 43.07 degrees", True),
        ("43.06 deg", True),
        ("5.02° 2θ", False),
        ("43.06 °C", False),  # a temperature is no angle
    ],
)
def test_angle_degrees(answer, ok):
    assert correct(num(43.0631, "°", abs_=0.02), answer) is ok, answer


def test_bare_degree_asked_as_temperature_is_celsius():
    spec = num(298.15, "K", abs_=0.5)
    assert correct(spec, "25°")
    assert correct(spec, "25 degrees")
    assert correct(num(25.0, "°C", abs_=0.1), "25°")


# ---------------------------------------------------------------- existing units unchanged


@pytest.mark.parametrize(
    ("spec", "answer", "ok"),
    [
        (num(400.152, "MHz", rel=0.002), "400.15 MHz", True),
        (num(400.152, "MHz", rel=0.002), "400150 kHz", True),
        (num(-119.14, "pA", abs_=0.05), "-0.11914 nA", True),
        (num(54.4, "µV", rel=0.01), "0.0544 mV", True),
        (num(298, "K", abs_=0.5), "25 °C", True),
        (num(298, "K", abs_=0.5), "25 ° C", True),
        (num(298, "K", abs_=0.5), "25 deg C", True),
        (num(11.4, "Å", rel=0.01), "11.4 A", True),
        (num(1030.0, "cm-1", abs_=2), "1030 cm⁻¹", True),
        (num(10.0, "min", abs_=0.05), "600 s", True),
        (num(1.0833, "µm", rel=0.01), "1.08 um/pixel", True),
    ],
)
def test_existing_units(spec, answer, ok):
    assert correct(spec, answer) is ok, answer


# ---------------------------------------------------------------- every question


def _reference_answer(a: dict) -> str:
    t = a["type"]
    if t == "number":
        return f"{a['value']} {a['unit']}" if a.get("unit") else f"{a['value']}"
    if t == "string":
        return a["accept"][0]
    if t == "list":
        return ", ".join(v[0] for v in a["accept"])
    if t == "boolean":
        return "yes" if a["value"] else "no"
    if t == "bbox":
        return "x={}, y={}, width={}, height={}".format(*a["value"])
    if t == "point":
        return "x={}, y={}".format(*a["value"])
    if t == "choice":
        return ", ".join(a["value"]) if a["multiple"] else a["value"][0]
    return str(a["value"])


def _phrasings(a: dict) -> list[str]:
    """The reference answer plus the ways an agent typically writes a number."""
    ref = _reference_answer(a)
    out = [ref]
    if a["type"] == "number":
        v, u = a["value"], a.get("unit")
        out.append(f"approximately {v} {u or ''}".strip())
        out.append(f"**{v}{' ' + u if u else ''}** (computed from the file)")
        if u:
            out.append(f"{v}")  # bare: read in the expected unit
    return out


def test_every_reference_answer_scores():
    questions = score.load_questions()
    assert questions
    failures = []
    for qid, q in sorted(questions.items()):
        for text in _phrasings(q["answer"]):
            got = score.score_answer(q["answer"], f"ANSWER: {text}")
            if not got["correct"]:
                failures.append((qid, text, got))
    assert not failures, failures[:10]


def test_every_expected_unit_is_known():
    for qid, q in score.load_questions().items():
        a = q["answer"]
        if a["type"] == "number" and a.get("unit"):
            assert score.unit_dimension(a["unit"]), qid
