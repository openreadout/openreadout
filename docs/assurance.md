# Assurance: how the evidence is derived

What the `assurance` block in `info` and `check` output means, and what `--strict` refuses, is described for users at <https://openreadout.github.io/openreadout/reference/assurance.html> (`book/src/reference/assurance.md`). This page is for maintainers: where the validated sets come from, how they are checked, and how they were measured.

## How the validated sets are derived

Each reader crate has a small `src/assurance.rs` with an `AssuranceProfile`. It holds:

- an `observe` function that extracts the file's variant features from the normalized `info`
  (format version, writer and its version, instrument, codec, sample layout, layout, acquisition
  mode, export dialect, record kinds), together with structures left undecoded, values assumed
  and calibrations. A few readers add what only they can see through
  `Dataset::assurance_observations`: the TIFF page codec with its photometric interpretation,
  planar configuration and predictor; the ND2 frame codec; the Zarr codec pipeline; the Imaris
  HDF5 filters; and the plate-export container and delimiter;
- the `basis` of the reader's knowledge: `open_spec`, `vendor_docs`, `prior_art` (permissively
  licensed community readers' documentation) or `reverse_engineered`;
- a generated table of feature values seen in the development corpus, between
  `// BEGIN GENERATED <format>` and `// END GENERATED <format>`, and the generated confidence level.

The table comes from the corpus, never from a person:

```bash
cargo build --release -p openreadout
CORPUS_RESULTS=/tmp/results.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus corpus_matches_oracle
MZ_RESULTS=/tmp/mz.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test mz_agreement
VENDOR_RESULTS=/tmp/vendor.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test chromeleon every_signal
SECOND_RESULTS=/tmp/second.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test second_fields
PLATE_RESULTS=/tmp/plate.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test plate_binary
QPCR_ROCHE_RESULTS=/tmp/qpcr.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test qpcr_roche
THERMO_DETECTOR_RESULTS=/tmp/thermo.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test thermo_detectors
HDF5_RESULTS=/tmp/hdf5.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test hdf5_structure
cargo xtask assurance-audit refresh --results /tmp/results.jsonl --results /tmp/mz.jsonl --results /tmp/vendor.jsonl --results /tmp/second.jsonl \
  --results /tmp/plate.jsonl --results /tmp/qpcr.jsonl --results /tmp/thermo.jsonl --results /tmp/hdf5.jsonl   # writes corpus/assurance/evidence.json
cargo xtask assurance-audit --write                                # regenerates the tables; writes target/reports/evidence.md
cargo xtask assurance-audit                                        # CI: fails when anything is stale
```

`refresh --format FMT` (repeatable) observes only the files of those formats and keeps the
evidence of every other format as it is, so a change to a few readers needs only their results.
`refresh` runs `info --json` on every development input on disk. It never runs on a held-out
file. It records each file's features, depositor (source record) and corpus-test result (a file both tests compare fails if either fails; the m/z agreement test, against vendor-library conversions, covers `spectra`): `pass`
or `FAIL`, whether the oracle is independent, and which outputs the comparison covered. A
feature value counts as **validated** by a file only when that file matched an independent oracle
on at least one of the outputs the feature affects. A codec is not validated by a file whose
oracle compared only its metadata. A descriptive feature (empty scope) is validated by any
passing file. Files written by our own tooling (synthetic fixtures) validate, but they are not
counted as depositors. The committed evidence lets the audit run in CI without the corpus. The
corpus test `assurance_evidence_is_current` fails when a reader's features on a corpus file
drift from the evidence, for example after a note is reworded.

### Derived values, and field coverage

Every normalized value is one of three things: read from a location whose meaning is known,
derived by a rule, or assumed. Readers report the last two: `Observations::assumed(field, …)`
and `Observations::derived(field, rule, …)`. A derivation becomes a `derivation` feature,
`<field> by <rule>` (the field without indices), which the audit validates like any other
feature, but only with development files whose oracle comparison checked that field. The plate
reader reports every measured read's mode this way (`mode_basis` in `extra.reads[]`:
`detector code`, `label keywords`, `read settings`; a mode nothing names is assumed).

A comparison with an oracle checks some values and not others: an image oracle compares
pixels, sizes and channels, but rarely the acquisition time. A passing file used to validate
every descriptive feature, whatever its oracle compared. For the values that questions ask about
and whose errors do not show in decoded data, the corpus test now records which ones each
comparison checked (`fields` in its results, `covered(...)` in `crates/openreadout-corpus-tests/tests/corpus/`),
and the core tracks them (`TRACKED_FIELDS`): the acquisition start
(`experiment.acquisition.started_at`), the instrument model (`experiment.instrument.model`) and
plate read modes (`tables[].extra.reads[].mode`). A file holding one becomes a `field` feature;
it is validated only by development files of its format whose comparison checked that field.
A value read from a field whose meaning a published specification or the vendor's own
documentation gives (its experiment provenance is `spec` or `vendor-impl`) is read from a
validated location and is not tracked. Second opinions count as such comparisons: the
field-level second-opinion test (`second_fields`) sets each
family's acquisition time (zones handled per reader) and instrument model against an independent
reader or the vendor's own export, and its results list the tracked fields a second reader
**agreed** with; a difference adjudicated in our favour lets the file pass but confirms nothing,
and one adjudicated against us (or `neither`) fails the file. A field no oracle has checked is withheld by `--strict` and named in `reasons`; the level does not change (the value is read along validated paths), and nothing else is refused because of it. A value derived by a rule no oracle confirmed makes the file `partially_validated`, as an assumed value does.

### Vendor-stored results

Some files carry results the vendor software computed from the raw data: Chromeleon's
integrated peaks, ChemStation's `Result.xml`, stored Cq values. When our decoded values
reproduce them closely, that is independent evidence: the vendor software is an independent
implementation that read the same raw data. It is recorded as its own evidence class,
`vendor_stored_result` (results lines with `"evidence": "vendor_stored_result"`;
`stored_result` and `stored_result_compared` in `corpus/assurance/evidence.json`), and it is
scoped: it validates only the features of the outputs the results were computed from (for
Chromeleon, `traces`: the signals' values, time axis and scaling), never a descriptive feature,
metadata, the decoding of the stored result table itself, or a tracked field. Within that scope
it validates only the signals the results were computed from: the results line lists their
features (`features`, `stored_result_features` in the evidence), so peaks integrated on a UV
signal do not confirm the codec or layout of a pressure or flow signal in the same archive
(since 2026-10-06; before, every trace feature of the archive counted). A file confirmed
this way counts as a confirmed file in the rubric; a stored result our values do not reproduce is a failure. Agreement must be
non-trivial: for Chromeleon, at least 10 stored peaks per archive, every one reproduced with area
and height within 1e-6 relative (`VENDOR_RESULTS=<file> cargo test -p openreadout-corpus-tests
--features corpus --profile corpus --test chromeleon every_signal`, then `refresh --results <file>`).

Validation is per feature value, not per combination of values. A file whose codec and whose
photometric interpretation were each validated separately passes even when the two never
appeared together. For that reason the TIFF profile folds the photometric interpretation into
the codec value of colour-sensitive codecs (`jpeg (rgb)` is a different value from
`jpeg (ycbcr)`). Structures a reader meets only while decoding (an unknown block type deep in a
plane) are not in the assurance block. Readers refuse those with exit 6 when they meet them
("right or refuse").

### Data a reader found but did not read

A structure a reader recognises and does not decode is `undecoded`. With a scope it makes those
outputs `unvalidated`; without one it is left out and the file is `partially_validated`. The plate
reader reports both since 2026-10-06: an export that yields no values, or a plate block without
values, is undecoded with the tables scope, and a read or plate section it refuses
by name (`multiple_reads_without_mean`, `plate_not_decoded`, `read_not_decoded`, `table_axis_not_decoded`,
`unsupported_read_type`) is left out. The UNICORN reader does the same for curves it refuses (a member cut short, or bare floats instead of a serialized array): `info` notes that the file is damaged, and the traces are undecoded, so the file is never `validated` for them, whatever its comparison covered. A table with no rows is not enough on its own: event tables
of electrophysiology files are often empty, and correctly so.

## Cross-validation on the development corpus

A held-out file comes from a depositor the reader has never seen. The development corpus can
imitate that: `cargo xtask assurance-audit cv` takes each depositor in turn, rebuilds the
validated tables without that depositor's files (synthetic fixtures stay), and assesses each of
its files against them, exactly as the core assesses a new file (`--json FILE` writes every
verdict). Every development file agrees with its oracle, so this measures the price of
`--strict` for a new depositor, not its recall: how often it would refuse an output that an
independent reader confirmed.

Most refusals come from variant features that only one depositor's files carry (a format
version, an acquisition mode, a layout). A format whose development files all come from one
depositor is refused whole for any new depositor, which is what the rule means. The
mass-spectrometry profiles scope the instrument generation instead of the exact model string,
because packet layouts follow the generation. An unseen format or writer version between two
validated versions of the same family and major version is partial instead of refused.

No development file fails its oracle, so recall cannot be cross-validated on it. Held-out runs
measure it instead (`docs/benchmark/heldout.md`). Errors in single values, such as a read mode or
a measurement time, are what `inferred`, field coverage and `strict_withholds` address.

## The confidence rubric

A reader's confidence level is computed from the same evidence (`cargo xtask assurance-audit`),
not assigned by hand:

| level | requires |
| --- | --- |
| **high** | at least 5 development files confirmed by an independent reader, from at least 3 depositors; at least 2 distinct format versions, writers or writer versions confirmed; no corpus file disagreeing with its oracle; no held-out failure; and either an open specification or vendor documentation, or at least 10 confirmed files from at least 5 independent depositors (a reverse-engineered format confirmed exactly on many independent sources can be high without a spec). When more than half of the normalized fields have inferred meaning, 5 depositors are required too. |
| **medium** | at least 3 confirmed files, from at least 2 depositors or covering at least 3 format versions, writers or writer versions, and at least 90 % agreement on files with an oracle. |
| **low** | anything less: no independent oracle, self-consistency only, one or two files. |

*Depositors* are distinct source records: a Zenodo record, a MetaboLights study, a PRIDE project,
a GitHub repository, an OME sample directory. The held-out input is the per-format pass/fail
count of the latest held-out run, which is only measured (`docs/benchmark/heldout.md`).
`cargo xtask assurance-audit --write` writes the current table, with every input, to
`target/reports/evidence.md`, which is not committed.

## Adding a reader

1. Write `crates/openreadout-<fmt>/src/assurance.rs`. Copy a neighbour's, pick the features that
   change how values are decoded (give them a scope) and the ones that only describe the file
   (no scope), and add an empty generated block.
2. In `impl FormatReader`, add `fn assurance(&self) -> Option<&'static AssuranceProfile> { Some(&assurance::FMT) }`
   and take `confidence` from the profile (`confidence: assurance::FMT.confidence`).
3. Run the commands above and commit the evidence and the tables.

A reader without a profile still works, but every file it reads is `unvalidated`, and
`openreadout-cli`'s test `every_reader_declares_an_assurance_profile` fails.

## Differential cross-checks

The main corpus test compares each reader with one primary oracle. Where a second independent
reader exists, the corpus compares against it too (`cargo test -p openreadout-corpus-tests
--features corpus --profile corpus --test second_opinion`):

- `second_opinions_agree`: `oracle/second_opinion.py` runs a second reader on every
  development file of seven image formats, as a black box, and records its plane hashes in
  `corpus/oracle/second/`. The readers are pylibCZIrw (ZEISS libCZI, LGPL) on CZI, readlif (GPL)
  on LIF, and Bio-Formats (GPL) on ND2, TIFF-family, MRC, DM and OIR. The primary oracles are
  czifile, liffile, the nd2 library, tifffile, mrcfile, dm3_lib and oirfile. OpenReadout's
  planes must match the second reader. Where Bio-Formats groups a file's series differently
  (pyramid levels or label images as series, file sets grouped by name), the file is listed as
  not compared rather than matched wrongly.
- `recorded_second_opinions`: second readers already run by the primary oracle scripts
  (fcsparser against FlowIO, brukeropusreader against brukeropus, oiffile and Bio-Formats
  against each other, jcamp against nmrglue, tifffile against Bio-Formats on plates) record
  whether they agree. Every recorded disagreement must be adjudicated.

A disagreement fails the test until `corpus/oracle/second/adjudications.toml` says which reader
is right (`openreadout`, `second` or `neither`) and why.
Whole-slide planes above 4 GiB, which OpenReadout
reads by region only, are refused (exit 6) and not compared.
