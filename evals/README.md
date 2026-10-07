# Agent evals

This directory measures whether an AI agent answers questions about raw instrument files more often, faster and more cheaply with OpenReadout than without it. Each question is a corpus file (or a folder) plus a question a scientist would ask, with a typed answer computed by independent readers.

Runs spend model usage, so they are started by hand. CI only checks the harness: `ruff check`, `ruff format --check`, the tests that need no corpus, and `generate.py --check`.

## What is measured

321 questions (`questions/*.jsonl`, not counting the held-out split): lookups about single corpus files, 11 about whole staged shares, 115 in the analysis tier, 13 in the quantitation tier, 10 in the batch tier, 18 in the scenario tier and 15 in the visual tier.

| category | questions | examples |
| --- | --- | --- |
| identify-format | 5 | the format of a file with its extension removed |
| dimensions | 3 | z-slices, stage positions |
| pixel-size | 6 | µm per pixel, z-step, Å per voxel |
| channels | 10 | channel names, dyes, detection wavelengths |
| acquisition-time | 4 | acquisition date |
| instrument | 21 | objective and NA, detector, spectrometer frequency, instrument model |
| counts | 20 | events, sweeps, scans, MS2 scans, wells |
| sample-rate | 6 | sampling rate of a recording or chromatogram |
| values | 12 | first sample of a sweep, a precursor m/z, last retention time |
| integrity | 9 | truncated copies, a damaged file, an intact control |
| conversion | 6 | OME-TIFF and CSV exports, checked on disk |
| sample | 11 | sample name, vial, well, barcode |
| method | 23 | pulse program, LC gradient, GC oven ramp, plate protocol |
| operator | 3 | the user account that acquired the file |
| search | 11 | questions across a staged share of 9 to 63 files |
| analysis | 115 | a projection, a sweep's peak, an XIC apex, an IC50, a ΔΔCq |
| quantitation | 13 | peak area, height and purity, checked against the vendor's integration |
| batch | 10 | a folder plus the lab's sample sheet or plate map |
| scenario | 18 | an end-to-end lab request over a folder |
| visual | 15 | questions a picture answers first: where the fold is, which slice is in focus |

By family: microscopy 62, electron microscopy 10, flow cytometry 23, electrophysiology 25, NMR 17, mass spectrometry 39, chromatography 43, plate readers 32, high-content screening 15, qPCR 17, vibrational spectroscopy 15, bench instruments 21, share 2.

Every question has a `split`: `dev` (185) or `test` (136). The split is a hash of the question's corpus file, so all questions about one file land on the same side. The `heldout` (353 questions, `questions/heldout.jsonl`) split asks about files from sources no reader was developed on.

## Where the answers come from

Never from OpenReadout. `generate.py` reads each answer from:

- the committed ground truth in `corpus/oracle/<id>.json`, written by independent readers (czifile, nd2, tifffile, pyabf, pyteomics, nmrglue and others);
- facts in `corpus/manifest.toml`, such as a file's format or that it is damaged;
- fact files in `facts/`, computed with independent readers in the oracle environment by `facts.py`, `analysis.py`, `batch.py`, `scenario_facts.py`, `visual.py`, `heldout.py` and the `*_facts.py` helpers. Each fact records its reader, method and cross-check.

Every question's `source` field names where its answer came from. `generate.py --check` fails when the committed questions no longer match the facts; each fact script has a `--check` mode that recomputes from the corpus.

## How a question runs

`run.py` drives the Claude Code CLI headless, one process per question and condition. The question's file is copied into a fresh temporary directory under a neutral name, so the path gives nothing away. The repository and the corpus cannot be read from inside a run; a tool call that touches them is recorded as contamination.

There are three conditions:

- `with`: the `openreadout` binary, the skill and the MCP server.
- `baseline`: the open-source readers a well-equipped scientist would install (`baseline/pyproject.toml`), Bio-Formats, and a skill naming the right reader per format.
- `without`: a shell and Python, with nothing preinstalled.

The prompt is the same in every condition and asks for a final line `ANSWER: <answer>`. Runs use your Claude Code login; `run.py` refuses to start while `ANTHROPIC_API_KEY` is set unless you pass `--allow-api-key`. Dollar figures are Claude Code's estimates at API prices.

## Running

You need the corpus (`cargo xtask corpus fetch --tier standard`), a release binary (`cargo build --release -p openreadout`) and a logged-in `claude` CLI.

```bash
cd evals
uv run python -m pytest -q                 # harness tests, no model calls
uv run python generate.py --check          # questions match the facts
uv run python run.py --dry-run             # print prompts and score fixture answers; no model calls
uv run python run.py preflight             # stage every question; no model calls

# a benchmark run; rerun the same command to resume it
uv run python baseline.py setup            # once
uv run python run.py --model claude-sonnet-5 --repeats 3 --jobs 4 --budget-usd 150 --run-name NAME
```

Select questions with `--ids`, `--family`, `--category`, `--split dev|test|heldout` or `--sample N`, and conditions with `--conditions`. Each question has caps on turns, time and estimated cost (60 turns, 15 minutes and $1.00 by default); `--budget-usd` caps the whole run. Exit code 3 means the budget stopped the run and 4 a deadline; both resume with the same command.

To recompute facts, set up the oracle environment once with `sh oracle/setup_full_env.sh`, then run the fact script with `oracle/.venv/bin/python evals/<script>.py [--check]` and regenerate the questions with `uv run python generate.py`.

## Reading the results

Everything goes to `results/`, which is not committed:

- `results/<run>-<condition>.md` and `.json`: accuracy overall, per category and per family, and per question with cost, turns and time.
- `results/<run>-summary.md`: the conditions side by side.
- `results/<run>-benchmark-{dev,test}.md`: accuracy with 95 % bootstrap intervals over questions, paired comparisons between conditions, cost per correct answer, and how often a cap was hit (`stats.py` rebuilds them).
- `results/<run>-triage-dev.md`: one cause per failed run, and for `with` failures whether the answer was in OpenReadout's output (`triage.py`).
- `results/transcripts/`: full transcripts.

`score.py` reads the text after the last `ANSWER:` line. Numbers are parsed with their units and compared within the question's tolerance; strings match whole words, case-insensitively; dates, lists, booleans, boxes and points have their own rules. `uv run python score.py answer ID "ANSWER: ..."` scores one answer.

Rules for honest numbers:

- Optimize against `dev` only. Read only the aggregate score of `test`; `triage.py` refuses it without `--unseal`.
- Run at least three repeats; agents are not deterministic.
- Quote `test`, `with` against `baseline`, with the model, the Claude Code and OpenReadout versions, the number of questions and repeats, and the intervals.
- Report `heldout` on its own, next to `test`. The held-out protocol is in [docs/benchmark/heldout.md](../docs/benchmark/heldout.md).

## Adding a question

1. Pick a small corpus file with ground truth, and a fact a scientist would ask about.
2. Add a `Spec` to `generate.py` (or to the tier's spec list): an id with the tier's prefix (`ana-`, `qnt-`, `batch-`, `scn-`, `vis-`, `share-`, `task-`, `ho-`), the corpus id, the category, the question, and an answer builder that reads the value from the ground truth.
3. Run `uv run python generate.py`, `uv run python run.py preflight --ids ID` and `uv run python -m pytest -q`.
4. If the scorer needs a new unit or spelling, add it to `score.py` with a test.
