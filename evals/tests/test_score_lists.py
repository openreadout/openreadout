"""Lists of numbers in the scorer: dimensions joined by a multiplication sign, and decimals that
round to the expected value."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import run
import score


def number_list(*values: str) -> dict:
    return {"type": "list", "value": list(values), "accept": [[v] for v in values], "ordered": False}


def correct(spec: dict, answer: str) -> bool:
    return score.score_answer(spec, f"ANSWER: {answer}")["correct"]


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("25088 × 37632", True),
        ("25088 x 37632", True),
        ("25088 X 37632", True),
        ("25088x37632", True),
        ("25088 by 37632", True),
        ("25088 * 37632", True),
        ("37632 × 25088", True),
        ("25088, 37632", True),
        ("width 25088, height 37632 pixels", True),
        ("25088 × 3763", False),
        ("2508 × 37632", False),
        ("25088", False),
        ("250880 × 37632", False),
        ("25088 × 376320", False),
    ],
)
def test_dimensions_joined_by_a_multiplication_sign(answer, ok):
    assert correct(number_list("25088", "37632"), answer) is ok


def test_three_dimensions_joined_by_signs():
    spec = number_list("512", "720", "160")
    assert correct(spec, "512 × 720 × 160")
    assert not correct(spec, "512 × 720 × 16")


def test_a_magnification_keeps_its_x():
    spec = {"type": "string", "accept": ["100x"], "reject": []}
    assert correct(spec, "a 100 x objective")
    assert correct(spec, "100x")


@pytest.mark.parametrize(
    ("answer", "ok"),
    [
        ("1004.46, 385.05", True),
        ("1004.462, 385.053", True),
        ("1004.4649, 385.0451", True),
        ("385.053 -> 1004.462", True),
        ("m/z 1004.462/385.053", True),
        ("1004.466, 385.053", False),  # rounds to 1004.47
        ("1004.4, 385.05", False),  # fewer decimals must match exactly
        ("1004.46", False),
        ("1004.462, 385.063", False),
        ("1004.452, 385.053", False),
        ("1004.462, 386.053", False),
    ],
)
def test_longer_decimals_match_when_they_round_to_the_expected_value(answer, ok):
    assert correct(number_list("1004.46", "385.05"), answer) is ok


def test_an_integer_count_is_not_matched_by_rounding():
    spec = number_list("12")
    assert correct(spec, "12")
    assert not correct(spec, "12.4")


@pytest.mark.parametrize(
    ("calls", "used"),
    [
        (
            [
                {
                    "name": "Bash",
                    "input": {"command": "cd /tmp/openreadout-eval-ab12/q-x-0 && python read.py sample.czi"},
                }
            ],
            False,
        ),
        ([{"name": "Bash", "input": {"command": "ls /var/folders/xy/openreadout-eval-ab_12/q-ho-1-9/"}}], False),
        ([{"name": "Bash", "input": {"command": "/tmp/openreadout-eval-ab12/bin/openreadout info sample.czi"}}], True),
        ([{"name": "Bash", "input": {"command": "openreadout info sample.czi"}}], True),
        ([{"name": "Skill", "input": {"skill": "openreadout:openreadout"}}], True),
        ([{"name": "mcp__openreadout__openreadout_info", "input": {}}], True),
        ([{"name": "mcp__plugin_openreadout_openreadout__openreadout_info", "input": {}}], True),
        ([{"name": "Read", "input": {"file_path": "/tmp/openreadout-eval-ab12/q/sample.txt"}}], False),
        ([], False),
    ],
)
def test_the_work_directory_name_is_not_a_use_of_openreadout(calls, used):
    assert run.used_openreadout(calls) is used
