# Validation

A reader for an undocumented file format needs evidence that it reads real files correctly. OpenReadout collects that evidence in four ways:

- **Independent readers.** When another reader can open a development file, we open it with both and compare the results plane by plane.
- **Vendor-software exports.** Where a conversion made with the vendor's own library exists (for mass spectrometry: ProteoWizard's reference conversions and depositors' msconvert output), or the file stores the vendor software's own results (such as Chromeleon's stored peaks), we compare OpenReadout's values with those.
- **A held-out set.** Files from sources we didn't use during development show whether the readers work on files they haven't seen.
- **Second opinions** on metadata fields (units, channel names, acquisition times, instrument models) from a second reader or the vendor's export.

This page explains each one and gives the headline numbers, each with the page or file it comes from.

## Headline numbers

From [Project health](health.md), which `cargo xtask health --write` generates from the repository:

| what | number |
| --- | --- |
| formats, reader crates | 96 formats in 40 reader crates |
| confidence (computed, not set by hand) | 44 high, 38 medium, 14 low |
| corpus manifest | 3,959 entries; 1,653 development inputs of 95 formats from 528 depositors; 292 held-out inputs |
| development files read | 1,629 |
| compared with an independent reader | 1,514, all of which agree (100%) |
| confirmed by an independent reader | 1,488 |
| variant-feature values seen, validated | 2,212 seen, 2,098 validated (95%) |
| development files `info` cannot read | 3 |
| latest held-out run | 292 inputs: 237 clean, 5 clean partial copies (`check` rightly exits 4), 36 disagree, 12 fail, 2 read but fail `check` or dump |

From the held-out report [`heldout-2026-10-06d.md`](../../../docs/benchmark/heldout-2026-10-06d.md): of the 94 inputs of its newest draw that have an independent oracle, 77 agree as read (81.9%, 95% CI 72.9–88.4%) and 85 (90.4%, CI 82.8–94.9%) after adjudication, which does not count oracle and converter errors against OpenReadout. Mass spectrometry agrees on 8 of those 13 files, the other families on 77 of 81. Over all 252 counted held-out inputs of the four draws, 219 agree as read (86.9%) and 240 (95.2%) after adjudication.

From [m/z agreement](mz-agreement.md) (2026-09-26): for Bruker timsTOF, Waters MassLynx, Thermo RAW and Agilent MassHunter files, every reference point in the vendor-library conversion has a counterpart, and the largest m/z difference is about 0.06 ppm, the rounding of the references' 32-bit m/z arrays.

From [`second-opinions.md`](../../../docs/benchmark/second-opinions.md) (2026-09-26, development files only): 775 files, 6,344 field checks, 6,114 agree; the 123 differences were all adjudicated in OpenReadout's favor.

## The test corpus

[`corpus/manifest.toml`](../../../corpus/manifest.toml) lists every test file with its download URL, checksum, license, source and attribution. A file can be added only if its license allows redistribution or its owner has given written permission. Medical or patient-identifiable data and vendor demo datasets are not accepted. Besides development inputs, the manifest holds parts of multi-file documents, companion files, exports made by the depositors' own software, and deliberately damaged files.

Files are downloaded, not committed. They are grouped in tiers, and each tier includes the ones above it:

| tier | used |
| --- | --- |
| `smoke` | on every push, in CI |
| `standard` | before a release |
| `full` | on demand; multi-gigabyte stress files |
| `hold` | not fetched automatically; the license is not yet confirmed |

```bash
cargo xtask corpus fetch --tier smoke      # resumable; verifies checksums
cargo xtask corpus status
```

The declared size of each tier is on the [project health](health.md) page.

## Comparing with reference readers

An **oracle** here is an independent reader used as reference: a third-party program that opens the same file, run as a black box.

1. **Ground truth.** `oracle/gen.py` opens each file with reference readers, for example `czifile`, `nd2` and `liffile` for microscopy, or FlowIO and fcsparser for FCS. It records the geometry, pixel type, physical pixel sizes, channel names and a hash of every plane's raw samples, and commits the result as `corpus/oracle/<id>.json`. For Thermo `.raw` files, the reference is the mzML or mzXML that the depositing lab produced with the vendor's own software.
2. **The same measurements.** `cargo test -p openreadout-corpus-tests --features corpus` opens every downloaded file with OpenReadout and computes the same values in the same way.
3. **Comparison.** Geometry and pixel type must match exactly. Physical sizes must agree to a relative 1e-6. Every hashed plane must be identical bit for bit, so any difference is a real difference in decoded pixels. Lossy codecs, such as JPEG, may set a small per-sample tolerance in the manifest.
4. **Report.** Each file ends as `pass`, `FAIL`, `skip` (with the reason from the manifest), `no-oracle` or `oracle-error`. CI adds the table to the job summary.

Exports are checked too. `oracle/omexml_validate.py` validates the OME-XML of an OME-TIFF export against the OME schema. `oracle/omezarr_validate.py` validates OME-Zarr exports, reads them with `ome_zarr` and bioio, and compares their pixels with the source.

## When readers disagree

We don't settle a disagreement by tweaking OpenReadout's output until it matches.

1. **A third reader decides.** Bio-Formats (`bfconvert`, run as a black box) shares no code with either side.
2. **If OpenReadout is wrong,** the reader is fixed and the finding goes into the format's provenance log.
3. **If the reference reader is wrong,** the manifest entry gets an `oracle_skip` field that names the reader, its version and the reason. The file is still opened on each run, so a crash still fails the test, but its results aren't compared. For example, the `nd2` package reads one ND2 file one byte off, and Bio-Formats agrees with OpenReadout.
4. **If the readers interpret the file differently,** the manifest and the format notes say which interpretation OpenReadout uses and why.
5. **Files OpenReadout detects but cannot read yet** are `skip`, and `openreadout self formats` lists the gap.

## Held-out files

Files on the `heldout` tier come from sources that were not used during development. No reader may be developed or debugged with them, and no provenance log may cite them. A finding on a held-out file is first reproduced on a new public file, which joins the development corpus; the held-out file only confirms afterwards that the fix generalizes. The rules are in [`docs/benchmark/heldout.md`](../../../docs/benchmark/heldout.md).

## Why GPL readers can be references

Running a program and comparing its output doesn't copy its code. GPL and LGPL readers run in a separate Python environment (`oracle/`). We don't link or ship them, and we don't read their source while writing a parser. See rule 3 of the [clean-room policy](clean-room.md).

## The appendices

- **[Evidence per format](evidence.md):** for each format, its confidence level, knowledge basis, files read and confirmed, depositors, versions, agreement, share of inferred fields and held-out results. Generated from `corpus/assurance/evidence.json`.
- **[m/z agreement](mz-agreement.md):** spectrum-by-spectrum comparison of mass-spectrometry readers with conversions made by the vendors' own libraries, per vendor and per file.
- **[Performance](performance.md):** time and memory compared with Bio-Formats and bioio on the same corpus files, with the conditions and the commands to reproduce them. For example, `info` on a 3.7 GB CZI took 19 ms there, against 991 ms for `showinf -nopix`.
- **[Project health](health.md):** the current counts behind this page, the variant values no independent reader confirms yet, open findings and the maintainers' safety nets.

How a file's evidence becomes the `assurance` line in `info` is explained in [Assurance and strict mode](../reference/assurance.md).

## Contributing a file

Open a [format request](https://github.com/openreadout/openreadout/issues/new/choose) with a link to the file and its license, or add the manifest entry yourself (see [`corpus/README.md`](../../../corpus/README.md)). Files from instrument or software versions that the corpus does not have yet are the most valuable.
