# PerkinElmer `.sp` provenance

## 2026-09-23 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used:** specio's `spectra.sp` (BSD-3-Clause, github.com/paris-saclay-cds/specio),
FTIR `.sp` files of Zenodo record 8161216 (CC-BY-4.0, "Preservation of corneous beta proteins in
Mesozoic feathers", PerkinElmer Spotlight/Spectrum), and Orange-Spectroscopy's
`single_PE_spectrum.sp` (GPL-3.0-or-later data file, commit fc7cc69).

**Permissive prior art consulted:** specio (G. Lemaître, BSD-3-Clause; LICENSE read first),
`specio/plugins/sp.py`: the `PEPE` signature and 40-byte description, blocks of a uint16 id and
an int32 size, nested blocks, the member ids of the axis range (two float64), the axis step,
the point count (uint32) and the data (float64 array), and the text members of the instrument
block.

## 2026-09-23 — implementation and validation (Richard Zimring with Claude as assistant)

**Our findings:** the member tags are 0x75xx words (`#u` text, `\x1bu`/`\x1cu` float64 — the latter
with a `$u` validity flag —, `\x1du` pair, `+u`/`\x1au` uint32, `,u` uint16 with a flag, `\x16u`
float64 array); history records nest the previous record; the acquisition time is the record whose
operation is `Created as New Dataset`; instrument settings 35840 (scans), 35844 (resolution), 35882
(laser wavenumber) were named from their values across Spectrum One, Frontier and Spotlight 400
files. A data set may end with bytes that do not parse as blocks (Zenodo 8161216 files): the
blocks read before them are kept.

**Validation:** specio on the 5 files bit for bit, and its instrument texts (model, serial,
accumulations, detector, source, beamsplitter, apodization) where it can decode them.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the data-set kind, data type and beam type. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — deep history chains; six new depositors

**Corpus files:** `zenodo7500539-pda-cmcsza-hec-sp`, `zenodo13684228-biocarbons-fgases-ftir-cc-1-3h3po4-sp`, `zenodo22967314-ftir-r1-raw-data-sp` (Zenodo 7500539, 13684228, 22967314; CC-BY-4.0). Five files from three further records were added the same day and removed again when held-out draw C reserved those records; they passed unchanged and nothing was inferred from them. No held-out file.
**Prior art:** specio (BSD-3) as before, black box; for one file (22967314) specio's UTF-8 decoding of the 40-byte description fails on bytes after its NUL, so the oracle makes those 40 bytes ASCII before handing the file to specio's reader (the blocks are untouched).
**What was inferred from what.** `zenodo7500539-pda-cmcsza-hec-sp` nests its history records seven levels deep (each 121 record holds the previous one under 35703), which put the instrument record (123) below the 16-level walk limit: its model and serial were silently missing while specio found them. The history nesting is unbounded in principle (one level per processing step), so the walk limit is now 256 levels (each level costs at least 6 bytes of file, so the walk stays bounded by the 64 MiB tree cap).
