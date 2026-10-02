# Held-out corpus and questions

Every reader in OpenReadout was developed against the files in `corpus/manifest.toml`, and the
benchmark's `dev` and `test` questions (`evals/`) ask about those same files. A score on them shows
that the readers work on the files they were built on; it cannot show that they work on the next
lab's files: another instrument model, another acquisition-software version, another depositor's
habits. The held-out set measures that: public files from sources no reader was developed on, with
ground truth from third-party readers and depositor exports, and questions about them that are
kept apart from the rest of the benchmark.

## What is in it

- **Files**: entries on the `heldout` tier of `corpus/manifest.toml`. Inputs have
  `role = "heldout"`; a depositor's open-format export paired with an input (the mzML of a Thermo
  run) keeps `role = "oracle-export"`, and a zip holding a directory data set keeps
  `role = "bundle"`, both on the `heldout` tier; the files of a directory data set copied file by
  file (a partial copy of a screening plate) are `role = "part"` entries under the directory's own
  `role = "heldout"` entry. Every held-out file comes from a source record
  (Zenodo record, MetaboLights study, PRIDE project, EMPIAR entry, OME sample directory, GitHub
  repository, ...) that the development corpus does not use (a GitHub repository is one record; ProteoWizard's collects test data from unrelated contributors, so its top-level trees — `pwiz/…`, the vendor-reader test data, and `pwiz_tools/…`, Skyline's — are separate records), and each entry records its licence,
  where the licence was checked (`license_checked`) and what ground truth exists. Each input names
  the generalization draw that added it (`draw = "2026-09-24"`, `"2026-09-24b"`, `"2026-09-26"`,
  `"2026-09-26c"`, ...), so a run can be reported per draw. Files are
  downloaded into `<corpus dir>/heldout/` (bundles into `<corpus dir>/<bundle id>/`).
- **Ground truth**: `corpus/oracle/heldout/<id>.json`, written by `oracle/gen.py` with the same
  third-party readers as the rest of the corpus (`oracle/gen_heldout.py` runs it with
  `ORACLE_OUT=corpus/oracle/heldout`; a plate export's dialect comes from its `plate_vendor` field), and
  `evals/facts/heldout.json`, computed by `evals/heldout.py` in the oracle venv (analysis-tier values
  from the data, depositor-stated values from the repository record).
- **Questions**: `evals/questions/heldout.jsonl` (ids `ho-*`, `split = "heldout"`), lookups like the
  `dev`/`test` ones plus an analysis tier. Answers come from the held-out oracles, depositor exports
  and depositor metadata only, never from OpenReadout.

```bash
cargo xtask corpus fetch --tier heldout                         # only the held-out files; never part of smoke/standard/full
cd oracle && uv run python gen_heldout.py [ID-SUBSTRING ...]      # gen.py per input into corpus/oracle/heldout
                                                                  # (--group chrom for ChemStation, --group plate for plates;
                                                                  #  JWS2TXT_DIR for JASCO, CHROMCONVERTER_LIB for Shimadzu)
oracle/.venv/bin/python evals/heldout.py [--check]              # evals/facts/heldout.json
cd evals && uv run python generate.py                           # writes questions/heldout.jsonl too
uv run python run.py preflight --split heldout                  # every held-out question stages; no model calls
uv run python run.py --split heldout ...                        # a benchmark run over the held-out questions
uv run python stats.py --run-name ... --split heldout           # its own report, with the gap to the test split
cargo test -p openreadout-corpus-tests --features corpus --profile corpus heldout -- --nocapture   # records oracle agreement
uv run python heldout_probe.py --binary ... --corpus-test ...   # detect/info/check/dump per input, with its assurance level
uv run python heldout_summary.py <probe.json> --classes <adjudication.json> [--draw D]   # per draw, family, assurance level
cargo xtask heldout-check                                       # the rules below
```

## Rules

1. **Held-out files are for measuring, never for developing.** Nobody opens a held-out file to
   develop or debug a reader: no hex dumps, no differential comparisons, no reading its oracle JSON
   while writing a parser. No provenance log (`docs/provenance/*.md`), format note
   (`docs/formats/*.md`) or reader source (`crates/*/src`) cites a held-out corpus id, download URL
   or file name. `cargo xtask heldout-check` (also a test in `cargo test -p xtask` and a CI step)
   fails if one does.
2. **A held-out failure is fixed on a new file.** The generalization report
   (`docs/benchmark/heldout-<date>.md`) lists what fails, with severity, so the failures can be
   assigned. The fix is developed on a NEW public file that shows the same problem, added to the
   development corpus with its own provenance entry; the held-out file only confirms afterwards that
   the fix generalizes. If no other file shows the problem, the held-out file is moved to the
   development corpus (and out of the held-out set, questions and all) and a replacement held-out
   file is found: a file that has been developed on is never held out again.
3. **Sources stay disjoint.** A source record is either held out entirely or not at all: the
   development corpus never takes a file from a record the held-out set uses (`heldout-check`
   compares Zenodo records, MetaboLights studies, PRIDE projects, EMPIAR entries, OME sample
   directories, GitHub repositories, G-Node GIN repositories, MassIVE datasets, DANDI dandisets,
   Cell Painting Gallery dataset sites and BioImage Archive studies found in each entry's URL, and
   the records its `source` text names where the URL carries none: figshare items ("Figshare
   28050092"), DANDI dandisets, nmrXiv projects ("project P52"), Dataverse DOIs, and Zenodo, PRIDE,
   MetaboLights or MassIVE ids of entries that come out of a bundle and have no URL). A depositor who
   put each sample in its own figshare item is one depositor: the report that adds such a file names
   the sibling items to avoid.

   **Records are the unit, not labs.** A different record (another Zenodo record, another figshare
   article) by the same lab or authors is a different source and may be development data, even when
   a sibling record is held out. Such a pair is legitimate but not independent in spirit (same
   instruments, same habits), so it is noted where it happens: in the development file's provenance
   entry and in the draw report that reserves the sibling. Example: the development Neuralynx files
   of figshare 25325560 come from the lab whose figshare 26337268 draw C holds out
   (`docs/provenance/neuralynx.md`).
4. **Corpus tests only record.** `heldout_agreement_is_recorded` (in
   `crates/openreadout-corpus-tests/tests/corpus/`) runs the same oracle comparison as
   `corpus_matches_oracle` on every held-out input and prints pass / FAIL / PANIC per file
   (`HELDOUT_REPORT=path` writes it as JSON lines); it never fails the build. `corpus_matches_oracle`
   and the other corpus tests skip `role = "heldout"`, and the share questions (`evals/share.py`) only
   take `smoke`/`standard` files, so held-out files never enter the `dev`/`test` questions.
5. **Held-out questions are sealed.** Like the `test` split, only the aggregate score of `heldout`
   is read; `triage.py` refuses it without `--unseal`, and `run.py --split all` means `dev` + `test`
   (the held-out questions run only with `--split heldout`). Quote held-out accuracy next to the
   `test` split's: the difference is the generalization gap.
6. **Adding held-out files** follows the corpus rules (a recorded, redistribution-compatible licence;
   nothing medical) plus: a source no development file uses, and preferably a different instrument
   model, software version, depositor and year from anything in the corpus. Questions about a new
   held-out file are written from its oracle and depositor metadata before OpenReadout is run on it.
7. **A record reserved after development exposure keeps its files, but they do not count.** When a
   draw reserves a source record that development work had already used (files from it were in the
   development corpus before the draw), those development files leave the development corpus (rule 3)
   and the record's held-out inputs stay in the held-out set, but each carries
   `exposed = "<YYYY-MM-DD>: <who> developed on <which files> of this record before the draw; <what was
   inferred from them, or: read, passed unchanged, nothing inferred>"` in corpus/manifest.toml. Exposed
   inputs are still probed and reported, but they are not generalization evidence:
   `evals/heldout_summary.py` leaves them out of every agreement rate and precision/recall figure and
   lists them in a table of their own, `evals/heldout_probe.py` marks their rows, the held-out
   questions about them carry `exposed` and `evals/stats.py` leaves them out of held-out accuracy.
   `cargo xtask heldout-check` prints every exposed entry and refuses an `exposed` value that is not
   a dated statement on a held-out input. An exposed input never becomes unexposed.

## Measurements

| draw | report | inputs | agreement with an independent oracle when first measured |
| --- | --- | --- | --- |
| 2026-09-24 (A) | [heldout-2026-09-24.md](heldout-2026-09-24.md) | 54 | 46 of 53 (87 %) |
| 2026-09-24b (B) | [heldout-2026-09-24b.md](heldout-2026-09-24b.md) | 22 | 17 of 21 (81 %) |
| 2026-09-26 (bench) | below | 5 | 1 of 3 (the ITC "pass" compared nothing: [draw C](heldout-2026-09-26c.md), C-O3) |
| 2026-09-26c (C) | [heldout-2026-09-26c.md](heldout-2026-09-26c.md) | 90 (82 counted, 8 exposed) | 67 of 71 (94.4 %, 95 % CI 86.4–97.8); 69 of 71 after adjudication; 4 variants refused cleanly (as first reported, with the 8 exposed inputs: 74 of 78) |

A draw's first measurement is the generalization number: later runs over the same files follow
fixes developed on other files, so they are no longer a fresh test. The latest run over all 171
inputs (2026-09-26, `main` at `7c0ad6bd`): 143 of 155 agree (92.3 %), 150 of 155 after adjudication
(without draw C's 8 exposed inputs, all of which agree or are self-consistency only: 136 of 148,
91.9 %, and 143 of 148);
the adjudication of every disagreement is in
[heldout-2026-09-26c-classes.json](heldout-2026-09-26c-classes.json).

## Incidents

Exposures of held-out files to development work, recorded so the reader of a held-out score can judge it.

- **2026-09-26, draw C reserved eight records development work had used.** Before draw C, the
  electrophysiology and spectroscopy work had added development files from eight source
  records that draw C then held out; for five of them the development file *is* the held-out file.
  The development entries and their oracles were removed when the draw was merged, and the eight
  held-out inputs carry `exposed` (rule 7): `ho-zenodo11586428-pl2-v4` (the offline-written PL2
  rules — footer offset 0, the 10-word end record — were inferred from files of this record, this
  one among them), `ho-figshare26337268-ncs-hpc` (the `.nvt` reader was first run on this record's
  video-tracker file; the held-out `.ncs` was not opened), and six read-and-passed-unchanged files
  with nothing inferred: `ho-zenodo5139650-abf-stns`, `ho-zenodo17294324-opus-silica`,
  `ho-zenodo21396092-spa-dust`, `ho-zenodo10885924-spa-raman`, `ho-zenodo21456008-sp-aeb`,
  `ho-zenodo3898505-sp-microplastic`. Draw C's headline is restated without them in its report
  (addendum).

- **2026-09-24, detection sweep.** `corpus/files/files` was a symlink back to `corpus/files`. A development script passed it to `openreadout info --view format -r`, which followed it into the held-out `ho-*` folders and ran header-only format detection on them. The engineer saw only the detected format names in a listing; no header, value or oracle was read, and nothing was used for or cited in any change. The symlink was removed, and the committed detection test (`crates/openreadout-corpus-tests/tests/detect.rs`) starts only from folders the development manifest names. Impact on held-out scores: none expected (every held-out file's format was already detected correctly in the first generalization report).
- **2026-09-26, bench-instrument held-out files.** The held-out ÄKTA/UNICORN, Image Lab, MicroCal ITC, JASCO and Seahorse files were downloaded by checksum-verifying scripts that did not print their content, and their ground truth was written by `oracle/gen_heldout.py`. The engineer saw the oracle scripts' one-line summaries (for the Seahorse file: well, reading and measurement counts and the background-well ratio) and listed the top-level key names of four held-out oracle JSONs to write the held-out questions (`evals/heldout.py`, `ho-bench-*`); no value was used for or cited in a reader change. Separately, a development Image Lab file turned out to come from a Zenodo record that also holds a held-out Rotor-Gene file (held out 2026-09-24); the `.scn` file was removed from the development corpus (`docs/provenance/biorad-scn.md`). It is unrelated to the qPCR reader.

## First measurement of the bench-instrument held-out files (2026-09-26)

Recorded by `heldout_agreement_is_recorded`; nothing was changed in a reader because of it.

| file | format | result |
| --- | --- | --- |
| `ho-zenodo16611302-scn-biochimlab` | `biorad-scn` | pass: the plane equals Bio-Formats' |
| `ho-zenodo21529576-itc-igp1-serk2` | `microcal-itc` | pass (structure only: 13 injections) |
| `ho-zenodo8277227-seahorse-taz` | `agilent-seahorse-asyr` | pass (self-consistency; background wells flattest) |
| `ho-gh-artiums-unicorn-histrap` | `cytiva-unicorn-zip` | FAIL: UNICORN's peak-table heights are 3-6 % below the curve value at the peak (the development files agreed within 2 %); whether UNICORN reports heights above a baseline is to be checked on a new development file |
| `ho-zenodo22832447-jws-ftir-atr` | `jasco-jws` | FAIL: the measurement time differs from the export's by far more than a time zone; the time field this file uses is to be identified on a new development file |
