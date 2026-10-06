# Maintaining OpenReadout

OpenReadout reads over ninety vendor formats, most of them reverse-engineered, and vendors keep shipping new software versions. This page is the answer to two fair questions: *who maintains this when a vendor ships version N+1*, and *does anyone understand this code*. The answer is not a person. It is a process, and a set of assets that let a new maintainer (a person or an agent, with no history in the project) take a file that fails today and turn it into a validated variant in an afternoon:

| asset | what it gives a new maintainer | where |
| --- | --- | --- |
| a maintainer guide per reader | the decode pipeline, where versions branch, invariants, how to debug, fragile spots; generated facts (evidence, the corpus files that pin each variant) that CI keeps current | `crates/openreadout-<name>/MAINTAINING.md` |
| format notes and provenance logs | every field's meaning in our own vocabulary, and how each was derived from which file | `docs/formats/`, `docs/provenance/` |
| per-file assurance | whether a user's file lies inside what the reader was validated on, and what is not | [assurance.md](assurance.md) |
| the corpus and its oracles | over a thousand public development files with independent ground truth; a held-out set that measures generalisation | `corpus/`, [corpus README](../corpus/README.md) |
| golden snapshots | every reader's output on every fixture and corpus file, so an unintended change anywhere is visible in review | `corpus/snapshots/`, `crates/openreadout-cli/tests/snapshots/` |
| the new-variant loop | a privacy-reviewed report from the user, an intake that turns it into a failing test, a playbook to make it pass | this page |
| project health | the numbers above, generated | [Project health](../book/src/project/health.md) (`cargo xtask health`) |

## The new-variant loop

A vendor ships a new software version, a lab buys a new instrument, or a codec appears that the corpus has never seen. One of three things happens to a user's file: it is **refused** (exit 3 unknown format, 4 corrupt, 6 unsupported feature), it **fails** (a command errors or, a bug, crashes), or it is **read but not validated** (`assurance.level` is `unvalidated` or `partially_validated`; `--strict` refuses the unvalidated outputs). None of these is silent: that is the point of the assurance signal. The loop below turns each into a validated variant.

```
user                          maintainer                                   CI / release
────                          ──────────                                   ────────────
openreadout report FILE  ──►  triage (severity)
+ vendor export               cargo xtask variant intake FILE --export X   corpus/intake/<id>.toml
+ public deposit (Zenodo)     ground truth: oracle/gen.py                  intake test fails
                              provenance stub → fix the reader             intake test passes
                              evidence refresh, snapshot review            assurance tables, snapshots
                              status = validated                            released; the variant is pinned
```

### 1. The user: report, export, deposit

`openreadout report FILE` (MCP: `openreadout_report`) writes `openreadout-report-<hash>.json` in the current directory and sends nothing. By default it holds the file's size, signature bytes and SHA-256, every decode stage with its error code, message and byte offset, the assurance fingerprint, the structure map, the key names of the vendor metadata tree and the metadata's numbers; no pixel, spectral or trace values, no sample, image or channel names, no serial numbers, no path (details: [cli.md § `report`](../book/src/reference/commands/check.md#--report)). The user reads it and attaches it to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml), which also asks for:

- **a vendor export of the same acquisition** — the ground truth that lets every value be checked, not only that the file opens (OME-TIFF for images, mzML for mass spectrometry, CSV/ASCII/AIA for traces, the software's table export for plates and qPCR);
- **a public deposit** when possible (see *Getting a file* below).

### 2. Triage

Label the issue with the format and a severity:

| severity | meaning | target |
| --- | --- | --- |
| **S1** | a file reads **wrong values that assurance calls `validated`** (the signal lied), or a reader **crashes** (a panic is a bug by contract: a reader must refuse, never crash) | fix before the next release; the file (or a sibling from another source) joins the corpus so the case stays pinned |
| **S2** | wrong or missing values on a file assurance already calls `unvalidated` (the user was warned; `--strict` refused them) | next release |
| **S3** | the file is refused (exit 3, 4 or 6) with a correct hint | when a public file or a donation is available |
| **S4** | a descriptive field is wrong or missing (writer version, instrument model, a label); values are right | batch into a release |

An S1 also gets a line in the release notes and an adjudication or a new corpus file; if the reader's confidence was `high`, check that the rubric (docs/assurance.md) still grants it after the fix.

### 3. Intake

```bash
cargo build -p openreadout
cargo xtask variant intake path/to/file.ext --export path/to/vendor-export.ome.tiff \
    --issue https://github.com/openreadout/openreadout/issues/123 \
    --url https://zenodo.org/records/…/files/file.ext --license CC-BY-4.0 --source "Zenodo record …: …"
```

The intake refuses held-out files (clean-room rule 11) and files already in the corpus, then:

- copies the file (and `--companion` files, and the exports) under `corpus/files/intake/<id>/`;
- appends a manifest entry (`role = "input"`, tier `hold` until the licence is confirmed) and one `oracle-export` entry per export with the same id;
- writes `corpus/intake/<id>.toml`: what the report said (fingerprint, level, unvalidated features, failing stages) and the **expectations** the corpus test `intake` checks — the file opens, its assurance reaches at least `partially_validated` (or `validated` when it already was), its ground truth `corpus/oracle/<id>.json` exists (files over 1 MiB are stored as `<id>.json.gz` (`cargo xtask corpus compress`; view one with `gzip -dc`)). These fail at intake: that is the failing test;
- appends a dated stub to the format's provenance log;
- prints the exact next steps, including the oracle commands the format's notes already use.

A report bundle without a file (`cargo xtask variant intake openreadout-report-….json`) becomes a record with status `awaiting-file`. It is triage information; it is not a parser input (clean-room rule 1), so no parser change is derived from it until a file we may hold arrives.

`cargo xtask variant status` lists the intakes; `cargo xtask variant check` (CI) keeps them consistent with the manifest.

### 4. Ground truth, then reproduce

```bash
cd oracle && uv run python gen.py --export ../corpus/files/intake/<id>/<export> --id <id> ../corpus/files/intake/<id>/<file>
cargo test -p openreadout-corpus-tests --features corpus --test intake        # fails: this is the target
CORPUS_ONLY=<id> cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus corpus_matches_oracle -- --nocapture
```

Formats whose oracle is a vendor export (Thermo, Sciex, Waters, Agilent MassHunter: the depositor's mzML; Shimadzu, Chromeleon, Empower: the ASCII export) take `--export`; the others run an independent open-source reader. `gen.py` dispatches on the file; format-specific scripts are listed in each format note and in the intake's output.

### 5. Understand, record, fix

1. Read the crate's `MAINTAINING.md`: where does this variant branch? The generated *Validated variants* table names the corpus files closest to the new one.
2. `openreadout report`, `openreadout info --view structure` and `openreadout info --view full` on the new file and on the closest corpus file; diff structure, not values. `openreadout report FILE --hex 64 --include-text` gives header excerpts.
3. **Write the provenance entry first** (clean-room rule 3): corpus files used, prior art consulted (URL and licence), what was inferred from what. Never a vendor SDK, header, DLL or non-public specification; GPL readers only as black boxes ([clean-room policy](legal/clean-room-policy.md)).
4. Pin the new structure in a unit test built from the observed bytes (every crate's guide names the tests to copy), then change the parser. New public names go in the format note's vocabulary table (`cargo xtask vocab-check`).
5. If the reader now decodes something it did not, make its assurance profile observe it (a new feature value, or `undecoded`/`assumed` entries) so the next unseen variant is flagged too.

### 6. Evidence, snapshots, close

```bash
cargo build --release -p openreadout
CORPUS_RESULTS=/tmp/results.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus corpus_matches_oracle
cargo xtask assurance-audit refresh --results /tmp/results.jsonl     # (+ --results /tmp/mz.jsonl for MS)
cargo xtask assurance-audit --write                                   # validated tables, confidence, evidence page
cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test snapshots
cargo xtask snapshot review && cargo xtask snapshot accept            # every changed output, reviewed
cargo xtask guides --write && cargo xtask health --write
```

Then set `status = "validated"` in `corpus/intake/<id>.toml` (from now on the intake test pins the variant), confirm the manifest entry's URL and licence and move it from `hold` to `standard` (or `smoke` if it is small enough for CI), add a CHANGELOG line, and link the pull request in the issue.

**How long this takes.** For a new version of a layout the reader already knows (the common case: a vendor release that adds a field or bumps a version number), an afternoon: the intake and oracle take minutes, the guide points at the branch, the unit tests show the pattern, and the snapshot review shows exactly what else moved. A new codec or container generation takes longer and should be planned as its own work.

### Getting a file

Nothing is fixed without a file we may legally hold ([clean-room policy](legal/clean-room-policy.md) rules 1 and 10). In order of preference:

1. **A public deposit**: the user acquires or picks a file with no sensitive content (a bead slide, a blank, a standard), uploads it with its vendor export to Zenodo under CC0-1.0 or CC-BY-4.0, names the instrument and software version in the description, and posts the record link. It joins the corpus with that URL.
2. **A donation with written permission** to use the file as a test file (recorded in the manifest's `license` field, e.g. "donated with written permission, 2026-10-02, issue #123"). Nothing medical or identifying.
3. **A public file from elsewhere** with the same variant (a data repository, a vendor's public demo *is not* acceptable: vendor demo datasets are not redistributed).

Never develop a fix on a held-out file, or on a sibling from the same source record: a held-out failure is fixed on a new file ([heldout.md](benchmark/heldout.md)).

## Regression snapshots

Every reader's `info` (as an xxh3-128 of the whole output plus a readable projection; the assurance block and the reader confidence are recorded as level, fingerprint and refused outputs instead, so an evidence refresh alone does not move every snapshot), `check --headers-only` verdict and assurance are recorded for every committed fixture (`crates/openreadout-cli/tests/snapshots/fixtures.jsonl`, checked by `cargo test`, no corpus needed) and every development-corpus file on disk (`corpus/snapshots/<format>.jsonl`, checked by the corpus test `snapshots`). A change in any output of any reader on any of those files fails the test with a field-level summary.

- `cargo xtask snapshot review` prints every difference; for corpus files it also diffs the full `info` against the last accepted output when the test has run once on the base branch (`target/tmp/snapshots/corpus/baseline/`).
- `cargo xtask snapshot accept [--only ID]` takes the new records; commit them with the change that caused them, so review sees both.
- Records are split by format, so parallel changes to different readers do not conflict. Two branches that both change one format's outputs: merge, rerun the test, review, accept.
- The corpus test opens every input twice and fails when the two outputs differ: a reader whose output follows a hash map's iteration order (HDF5 attributes, for one) changes from run to run. Sort such values before they reach the output.
- Snapshots are not an oracle: they say *something changed*, the corpus oracles say *whether it is right*.

## Confidence, assurance and evidence

Nothing in a reader's trust level is assigned by hand ([assurance.md](assurance.md)):

- a **feature value** (a format version, writer, codec, layout) is `unseen` until a development file has it, `seen` when files with it are read, and `validated` once one of them matches an independent reader on the outputs the feature affects;
- a **file** is `validated` when every feature value it uses is validated and nothing is undecoded, assumed or uncalibrated; `--strict` refuses outputs that depend on anything unvalidated;
- a **reader's confidence** (`high`, `medium`, `low`) follows the rubric from the same evidence: confirmed files, depositors, versions, agreement, the knowledge basis and the held-out results. A held-out failure caps it at medium until fixed on a new file.

Evidence only grows through the corpus: a new file with an oracle, `assurance-audit refresh`, `assurance-audit --write`. The held-out benchmark ([heldout.md](benchmark/heldout.md)) is measured, never tuned on, and a new draw is taken when the development corpus has absorbed the previous one.

## Release cadence

- **Monthly minor releases** while the version is 0.x, from `main` when CI is green; patch releases for S1 fixes as soon as they land. Steps: [release-process.md](release-process.md).
- Before tagging: refresh the assurance evidence on the full development corpus, rerun the corpus snapshot test (review every difference since the last release), run `cargo xtask guides`, `cargo xtask variant check`, `cargo xtask health --write`, and make sure no intake is `validated` while its manifest entry is still on `hold`.
- The release notes list: new variants validated (from closed intakes), S1/S2 fixes, confidence changes (the evidence page's diff), readers deprecated.
- A held-out draw is re-run at least every other release; its report goes in `docs/benchmark/`.

## Deprecating a reader

A reader is deprecated when it cannot be kept honest: its corpus sources disappear and none replace them, a legal problem arises (a takedown is handled as the clean-room policy says, not by reflex), or the format is obsolete and nobody reads it.

1. Open an issue explaining why; add a `known_gaps` line `deprecated since vX.Y: REASON; use vA.B or ALTERNATIVE` and a note in every `info`.
2. Keep reading the format for two minor releases. Its confidence keeps following the evidence.
3. Then make `open` refuse with exit 6 and a hint naming the last version that read it and an open alternative; keep detection so the error is precise.
4. Remove the crate from the registry one release later; keep its format note and provenance log (they are the record of how it was derived), and move its corpus entries to tier `hold`.

## Who does what

| role | does |
| --- | --- |
| reporter | runs `openreadout report`, provides an export and, if possible, a deposit |
| triager | labels format and severity within a week, asks for what is missing, runs `variant intake` |
| format maintainer (per crate, listed in the crate guide's header when there is one) | fixes, reviews snapshots for their formats, keeps the guide's hand-written part true |
| release manager | runs the pre-release steps above and the release process |
| agents | may do any of the above under the same rules: they read the guide, never vendor code (rule 2 of AGENTS.md), write provenance before code, and run the gates; their pull requests are reviewed like anyone's |

The gates every change passes: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo nextest run` (includes the fixture snapshots), `cargo xtask vocab-check`, `heldout-check`, `assurance-audit`, `skill-parity`, `guides`, `variant check`, `cargo deny check`; with the corpus: the corpus, snapshot and intake tests.

## Code health

- `cargo xtask coverage [--crate NAME] [--corpus]` measures line coverage per crate with cargo-llvm-cov (optional tool) and writes `target/coverage/summary.md`; with `--corpus` the corpus tests run under coverage too, which is what tells the reader code no test or corpus file reaches. Code paths nothing reaches are either given a synthetic test built from the observed structure, or made visible to users as `undecoded`/`unvalidated` in the assurance profile.
- Shared parsing utilities live in `openreadout-core` (`bytes`, `cfb`, `zip`, `gzip`, `xmljson`, `source`) and `openreadout-codecs`; a reader that needs a container another crate already parses should use the shared one (see the guides' *Fragile spots* for what a change there affects).
- Every public item of a published crate is documented (`#![warn(missing_docs)]`, an error in `openreadout-core` and under CI's `-D warnings`; CI also builds the API docs with `RUSTDOCFLAGS=-D warnings`); parser internals that tests and fuzz targets need are `#[doc(hidden)]`.

### Known duplication

Consolidated on 2026-10-01 (pure refactors, snapshots unchanged): bounds-checked scalar, UTF-16, Latin-1 and Windows-1252 field access (`openreadout_core::bytes`: `le_u32`, `be_f64`, `Endian`, `utf16le_z`, `latin1_field`, `windows1252`, `find`, `Block`, `read_block`, `read_range`; the private copies in the format crates and `openreadout-abf`'s copy of `Block` are gone), the Thermo method container (read with `openreadout_core::cfb`), the zip re-export stubs (`openreadout_core::zip` is used directly), civil-date arithmetic, month abbreviations and two-digit years (`openreadout_core::time`), and local-name `roxmltree` lookups (`openreadout_core::xml::{child, children, path}`). Earlier: half-float widening (`openreadout_core::pixel::half_to_f32`) and the corpus tests' reader lists (`crates/openreadout-corpus-tests/tests/support/registry.rs`). Left on purpose:

| helper | copies | why |
| --- | --- | --- |
| OLE Automation dates | `core::jet`, `em`, `plate` (Gen5) | each formats differently (zone, milliseconds, valid range) |
| zlib inflation | `chrom` (Shimadzu TLM), `plate` (Gen5), `qpcr` (`.ixo`), `core::zip` | the callers differ in how an over-long stream is treated and the TLM header read inflates partially |
| byte cursors | `thermo` (errors with offsets), `plate` (`Option`), `intan`, `chrom` (ChemStation results, netCDF) | sequential readers with different error types (corrupt error with offset, `Option`, `String`, a format error enum) |
| XML helpers with other predicates | `lif` (`has_tag_name`), `mzml` (streaming `quick_xml` nodes), `oir` (custom local name) | different matching or node types |

One behaviour to know when touching error handling: `openreadout-plate` catches panics inside the workbook parsers with `catch_unwind`, but the CLI's panic hook exits the process before unwinding reaches it, and release builds abort on panic. Such guards only protect library users; the CLI relies on dependencies not panicking (debug builds disable calamine's overflow checks for that reason, like image-webp's).

## Adding a format

1. Open `docs/formats/<fmt>.md` and write down what you know and how you know it (magic bytes, layout, vocabulary table). Start `docs/provenance/<fmt>.md`.
2. Add corpus files to `corpus/manifest.toml` with URL, checksum, and license. Generate ground truth with `oracle/gen.py` (it writes `corpus/oracle/<id>.json`; files over 1 MiB are stored as `<id>.json.gz` (`cargo xtask corpus compress`; view one with `gzip -dc`)).
3. Create `crates/openreadout-<fmt>` implementing `FormatReader` and `Dataset` from `openreadout-core`. `#![forbid(unsafe_code)]`. Read through the `Input` you are given (`open_input`, `SourceFile`, `Fs`; see [architecture.md](architecture.md)), not `std::fs`, so the reader also works on buffers, Python file objects and in the browser; `open(path)` becomes `self.open_input(&Input::local(path))`.
4. Register it in `crates/openreadout-cli/src/registry.rs` and in the corpus harness's copy, `crates/openreadout-corpus-tests/tests/support/registry.rs` (a test keeps the two identical).
5. `cargo test --features corpus` must pass for every file of that format in the smoke tier.
6. Write an assurance profile (`docs/assurance.md` § Adding a reader) and the crate's maintainer guide: `cargo xtask guides --write` creates `MAINTAINING.md` with its generated facts; fill in its four hand-written sections (CI fails while they hold TODO).
7. Record the golden outputs: `SNAPSHOT_ACCEPT=1 cargo test -p openreadout --test golden` for committed fixtures, and the corpus snapshot test for corpus files.

## Adding a corpus file

Only files with a recorded license that allows redistribution, or donated with explicit written permission. No patient-identifiable data. Add the entry to `corpus/manifest.toml`; `cargo xtask corpus fetch` records the SHA-256 on first download.

## Gates

Every pull request must pass `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo nextest run` (includes the golden snapshots of every committed fixture), `cargo xtask vocab-check`, `heldout-check`, `assurance-audit`, `skill-parity`, `guides`, `variant check`, `cargo deny check`.

## Public API

Public API of the published crates: every public item needs a doc comment (`#![warn(missing_docs)]` is on in each crate). A format crate documents its reader type and format-id constants; parser internals that tests or fuzz targets need go in a `#[doc(hidden)] pub mod`. New enums and option structs that may grow get `#[non_exhaustive]` (option structs then need a `Default`).
