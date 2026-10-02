# Validation

OpenReadout checks its readers against public instrument files, and compares what it reads with what other, independent readers return for the same files. This page explains how. The results per format are on the [evidence](evidence.md) page, and the current pass counts are on the [project health](health.md) page.

## The test corpus

[`corpus/manifest.toml`](https://github.com/openreadout/openreadout/blob/main/corpus/manifest.toml) lists every test file with its download URL, checksum, license, source and attribution. A file is admitted only if its license allows redistribution, or if its owner gave written permission. Medical or patient-identifiable data and vendor demo datasets are never admitted.

The manifest has 3,610 entries. Of these, 1,565 are development input files for 95 format ids. The rest are parts of multi-file documents, companion files, exports made by the depositors' own software, and deliberately damaged files. Another 187 input files form the held-out set described below.

Files are downloaded, not committed. They are grouped in tiers, and each tier includes the ones above it:

| tier | used |
| --- | --- |
| `smoke` | on every push, in CI |
| `standard` | before a release |
| `full` | on demand; multi-gigabyte stress files |
| `hold` | never fetched automatically; the license is not yet confirmed |

```bash
cargo xtask corpus fetch --tier smoke      # resumable; verifies checksums
cargo xtask corpus status
```

The declared size of each tier is on the [project health](health.md) page.

## Comparing with reference readers

An **oracle** here is an independent reader used as reference: a third-party program that opens the same file, run as a black box. The comparison works in four steps.

1. **Ground truth.** `oracle/gen.py` opens each file with reference readers, for example `czifile`, `nd2` and `liffile` for microscopy, or FlowIO and fcsparser for FCS. It records the geometry, pixel type, physical pixel sizes, channel names and a hash of every plane's raw samples. The result is committed as `corpus/oracle/<id>.json`. For Thermo `.raw` files, the reference is the mzML or mzXML that the depositing lab produced with the vendor's own software.
2. **The same measurements.** `cargo test -p openreadout-corpus-tests --features corpus` opens every downloaded file with OpenReadout and computes the same values in the same way.
3. **Comparison.** Geometry and pixel type must match exactly. Physical sizes must agree to a relative 1e-6. Every hashed plane must be identical bit for bit. Because the hash covers raw samples in a fixed order, any difference is a real difference in decoded pixels. Lossy codecs, such as JPEG, may set a small per-sample tolerance in the manifest.
4. **Report.** Each file ends as `pass`, `FAIL`, `skip` (with the reason from the manifest), `no-oracle` or `oracle-error`. CI adds the table to the job summary.

Exports are checked too. `oracle/omexml_validate.py` validates the OME-XML of an OME-TIFF export against the OME schema. `oracle/omezarr_validate.py` validates OME-Zarr exports, reads them with `ome_zarr` and bioio, and compares their pixels with the source.

## When readers disagree

A disagreement is never resolved by changing OpenReadout's output until it matches.

1. **A third reader decides.** Bio-Formats (`bfconvert`, run as a black box) shares no code with either side.
2. **If OpenReadout is wrong,** the reader is fixed and the finding goes into the format's provenance log.
3. **If the reference reader is wrong,** the manifest entry gets an `oracle_skip` field that names the reader, its version and the reason. The file is still opened on every run, so a crash would still fail the test; it is only not compared. For example, the `nd2` package reads one ND2 file one byte off, and Bio-Formats agrees with OpenReadout.
4. **If the readers interpret the file differently,** the manifest and the format notes say which interpretation OpenReadout uses and why.
5. **Files OpenReadout detects but cannot read yet** are `skip`, and `openreadout self formats` lists the gap.

## Held-out files

Files on the `heldout` tier come from sources that were not used during development. No reader may be developed or debugged with them, and no provenance log may cite them. They measure whether the readers work on files they have not seen. The rules are in [`docs/benchmark/heldout.md`](https://github.com/openreadout/openreadout/blob/main/docs/benchmark/heldout.md).

## Why GPL readers can be references

Running a program and comparing its output is not copying it. GPL and LGPL readers run in a separate Python environment (`oracle/`). They are never linked or shipped, and their source is never read while writing a parser. See rule 3 of the [clean-room policy](clean-room.md).

## Contributing a file

Open a [format request](https://github.com/openreadout/openreadout/issues/new/choose) with a link to the file and its license, or add the manifest entry yourself (see [`corpus/README.md`](https://github.com/openreadout/openreadout/blob/main/corpus/README.md)). Files from instrument or software versions that the corpus does not have yet are the most valuable.
