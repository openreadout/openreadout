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
cargo xtask assurance-audit refresh --results /tmp/results.jsonl --results /tmp/mz.jsonl --results /tmp/vendor.jsonl --results /tmp/second.jsonl   # writes corpus/assurance/evidence.json
cargo xtask assurance-audit --write                                # regenerates the tables and book/src/project/evidence.md
cargo xtask assurance-audit                                        # CI: fails when anything is stale
```

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
field-level second-opinion test (`second_fields`; `docs/benchmark/second-opinions.md`) sets each
family's acquisition time (zones handled per reader) and instrument model against an independent
reader or the vendor's own export, and its results list the tracked fields a second reader
**agreed** with; a difference adjudicated in our favour lets the file pass but confirms nothing,
and one adjudicated against us (or `neither`) fails the file. With them, 561 of the 1043
development files holding an acquisition time and 173 of the 646 holding an instrument model
(time and model not read from a documented field) have them confirmed, from 85 and 14 (refreshed
2026-09-26; SoftMax Pro text exports' save times no longer count as acquisition times). A field no oracle has checked is withheld by `--strict` and named in `reasons`; the level does not change (the value is read along validated paths), and nothing else is refused because of it. A value derived by a rule no oracle confirmed makes the file `partially_validated`, as an assumed value does.

### Vendor-stored results

Some files carry results the vendor software computed from the raw data: Chromeleon's
integrated peaks, ChemStation's `Result.xml`, stored Cq values. When our decoded values
reproduce them closely, that is independent evidence: the vendor software is an independent
implementation that read the same raw data. It is recorded as its own evidence class,
`vendor_stored_result` (results lines with `"evidence": "vendor_stored_result"`;
`stored_result` and `stored_result_compared` in `corpus/assurance/evidence.json`), and it is
scoped: it validates only the features of the outputs the results were computed from (for
Chromeleon, `traces`: the signals' values, time axis and scaling), never a descriptive feature,
metadata, the decoding of the stored result table itself, or a tracked field. A file confirmed
this way counts as a confirmed file in the rubric (the evidence page says how many were
confirmed only so); a stored result our values do not reproduce is a failure. Agreement must be
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
values, is undecoded with the tables scope (draw D's BMG kinetic export was `validated` with no
values, and three fuzz fixtures without plate data were too), and a read or plate section it refuses
by name (`plate_not_decoded`, `read_not_decoded`, `table_axis_not_decoded`,
`unsupported_read_type`) is left out. A table with no rows is not enough on its own: event tables
of electrophysiology files are often empty, and correctly so.

## Cross-validation on the development corpus

A held-out file comes from a depositor the reader has never seen. The development corpus can
imitate that: `cargo xtask assurance-audit cv` takes each depositor in turn, rebuilds the
validated tables without that depositor's files (synthetic fixtures stay), and assesses each of
its files against them, exactly as the core assesses a new file (`--json FILE` writes every
verdict). Every development file agrees with its oracle, so this measures the price of
`--strict` for a new depositor, not its recall: how often it would refuse an output that an
independent reader confirmed.

| 2026-09-26 | before (`main` at `8432da8e`) | after |
| --- | --- | --- |
| confirmed files | 839 | 864 |
| refused on an output their comparison checked | 314 (37.4 %) | 292 (33.8 %) |
| refused because of the instrument model | 33 | 10 |
| refused because of a plate export dialect | 30 | 23 |
| with a withheld field | – | 553 (64 %) |

The rest of the refusals are variant features that only one depositor's files carry (a format
version, an acquisition mode, a layout): formats with one depositor (all 22 ABF files come from
one repository) are refused whole for any new depositor, which is what the rule means. Two
changes lowered the price without a new depositor: the mass-spectrometry profiles scope the
instrument generation instead of the exact model string (packet layouts follow the generation),
and an unseen format or writer version between two validated versions of the same family and
major version is partial instead of refused. The rest came from new depositors.

No development file fails its oracle, so recall cannot be cross-validated on it. The reader
errors the third held-out draw found were reproduced on new development files before they were
fixed (`docs/provenance/plate-readers.md`, 2026-09-26); the assurance that `main` gave those
files before the fix is the closest development measure of recall: of six files with missing or
misassigned values, two were `unvalidated` for `tables` (a semicolon-delimited container never
seen), three `partially_validated` and one `validated` (an EnVision export returning no values).
Errors in single values (a read mode, a measurement time) are what `inferred`, field coverage and
`strict_withholds` now address.

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
count of the latest held-out run, which is only measured (`docs/benchmark/heldout.md`). The
current table, with every input, is the generated evidence page (`book/src/project/evidence.md`, "Evidence per format" in the book).

## Adding a reader

1. Write `crates/openreadout-<fmt>/src/assurance.rs`. Copy a neighbour's, pick the features that
   change how values are decoded (give them a scope) and the ones that only describe the file
   (no scope), and add an empty generated block.
2. In `impl FormatReader`, add `fn assurance(&self) -> Option<&'static AssuranceProfile> { Some(&assurance::FMT) }`
   and take `confidence` from the profile (`confidence: assurance::FMT.confidence`).
3. Run the four commands above and commit the evidence, the tables and the evidence page.

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
is right (`openreadout`, `second` or `neither`) and why. On 2026-09-26, 201 development files
had a second reader and 4,048 of their planes agreed with it; 87 recorded second opinions
agreed; 21 files were not compared because the series are grouped differently. Twenty-five
files were adjudicated (20 plane comparisons, 5 recorded opinions). Twenty-three are
conventions or errors of the second reader:
- rows flipped and image stacks as Z (Bio-Formats on MRC);
- colour samples split into channels in stored order, and other colour-ND2 layouts misread
  (Bio-Formats);
- Java JPEG and JPEG 2000 decoders one to 14 grey levels away from libjpeg-turbo and OpenJPEG;
- a MetaMorph file set grouped by name;
- pylibCZIrw's zstd1-HiLo decode of 48-bit RGB;
- readlif reading half-float FLIM maps as integers;
- brukeropusreader on three OPUS 8 files;
- fcsparser on an inconsistent FlowIO test file and on FCS 3.2 per-measurement types;
- a script artefact on per-scene time points.

In two (`zenodo10577621-PALM-OnlineVerrechnet`, `zenodo10577621-Palm-mitDrift`) neither reader
was right: OpenReadout mis-registered super-resolved PALM renderings. Fixed 2026-09-25 (each
stored-to-logical ratio is its own image with the rendering's pixel size, equal to czifile's
stored-size reading; `docs/provenance/czi.md`); the adjudications now name OpenReadout. Two whole-slide planes above 4 GiB, which OpenReadout
reads by region only, were refused (exit 6) and not compared.

## Measured on the held-out set

The profiles and tables were written and generated from development files only. Afterwards, on
2026-09-26, `info` was run on the 76 held-out inputs and each file's level was set against its
held-out corpus-test result. This was measurement only; nothing was tuned on it.

| held-out result | validated | partially validated | unvalidated |
| --- | --- | --- | --- |
| pass (68) | 36 | 24 | 8 |
| FAIL (6) | 1 | 3 | 2 |
| oracle error (2) | 2 | 0 | 0 |

Both held-out failures that are reader errors were `unvalidated`, and `--strict` would have
refused the affected outputs: a CZI written by a ZEN generation absent from the development
corpus, where the pyramid-level geometry differs, and Shimadzu traces returned without the
detector scaling LabSolutions applies. The other four failures come from the oracle or from a
converter's conventions (docs/benchmark/heldout-2026-09-24.md, findings O2, L1 and O-1): an
incomplete allotropy reading, RawConverter's charge and centroid conventions (two files), and
Bio-Formats disagreeing with tifffile. On those, `partially_validated` or `validated` is the
correct answer. The price of the guarantee is 8 held-out files that were read correctly but are
still `unvalidated`, because their variant has not yet been confirmed on a development file.

The CZI failure above later turned out to be an oracle error (factor-3 pyramids), so the
first measurement caught its one real reader error. A second measurement followed the third held-out draw
(docs/benchmark/heldout-2026-09-26c.md): 171 held-out inputs, 90 of them new, with every
disagreement adjudicated.

| verdict | validated | partially validated | unvalidated | refused (exit 6) |
| --- | --- | --- | --- | --- |
| agree (143) | 71 | 56 | 16 | 0 |
| oracle or converter error (7) | 2 | 4 | 1 | 0 |
| reader error (4) | 1 | 1 | 2 | 0 |
| unresolved (1) | 1 | 0 | 0 | 0 |
| unsupported variant, no oracle (4) | 0 | 0 | 0 | 4 |

`--strict` would have refused both reader errors in which values were missing or unscaled: a BMG
spectrum export that yields no values, and the Shimadzu traces. It did not refuse the two metadata
errors, an EnVision read mode left `unknown` and a JASCO measurement time, because the signal
describes how values are decoded, not how fields are labelled. It did not refuse the unresolved
ÄKTA peak heights either. So `unvalidated` has 2/2 recall for value errors and 2/5 over all
errors, at a precision of 2/19. 17 correctly read files are refused, because their variant has
not yet been confirmed on a development file. The four variants that no third-party reader
supports either were refused by the readers themselves, with exit 6 and a hint, not read wrongly.

The same 171 inputs were measured once more after derived values, field coverage, vendor-stored
results and instrument generations were added (commit `1eebdb43`; the release binary, nothing
tuned on the result). The BMG spectrum export and the EnVision read mode had been reproduced on
new development files and fixed, and both now agree with their oracles; the EnVision mode is
reported in `inferred` (derived from the detector code) and the file is `partially_validated`.

| verdict | validated | partially validated | unvalidated | refused (exit 6) |
| --- | --- | --- | --- | --- |
| agree (143) | 71 | 57 | 15 | 0 |
| fixed since (2) | 1 | 1 | 0 | 0 |
| oracle or converter error (7) | 2 | 5 | 0 | 0 |
| reader error (2) | 1 | 1 | 0 | 0 |
| unresolved (1) | 1 | 0 | 0 | 0 |
| unsupported variant, no oracle (4) | 0 | 0 | 0 | 4 |

No remaining reader error is refused whole: the Shimadzu traces are now scaled (the remaining
error is a 0.01 min time-axis offset) and their layout is `partially_validated`, and neither the
ÄKTA peak heights nor the JASCO file are. The JASCO error is a measurement time, which `--strict`
now withholds (`experiment.acquisition.started_at` has never been compared on a JASCO
development file), so one of the three remaining errors is kept out of strict output, at field
level. File-level refusals fall from 17 correct files to 15 (0/15 precision, against 2/19); the
Orbitrap ID-X, an FT-ICR mzML and the semicolon BMG export are no longer refused. Withholding is
cheap but not precise: 99 of the 152 files read correctly have a withheld field (measurement time
96, instrument model 68), because most formats have no oracle that compares those fields.


Draw D (2026-10-06, `docs/benchmark/heldout-2026-10-06d.md`) measured 104 new inputs with an
independent verdict on 93: `--strict` refused 2 of the 6 reader errors that returned data, at a
precision of 2 of 15, and 13 correctly read files were `unvalidated`. D-H5, D-M1, D-L4, D-G1 and
D-G4 were then fixed on development files, D-L1 and D-L2 turned out to be oracle conventions, and
three changes went into the signal: the plate rule above, the ion polarity of
a timsTOF mobility run as a variant feature (a negative run's 1/K0 is derived by a rule no vendor
conversion has confirmed, so `--strict` withholds it), and the columns of EC-Lab text exports as
descriptive features (a text export reads every column with one number parser). Re-measured after
the fixes (a re-measurement, not a fresh test): 2 of the 3 remaining reader errors that return data
are refused, at a precision of 2 of 16, and 14 correct files are `unvalidated`, the added one a
headerless Empower export whose layout no development file confirms.
