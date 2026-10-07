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

## 2026-10-06 — a UV-Vis `.sp`: the infrared settings are absent, not missing (Richard Zimring with Claude as assistant)

Held-out draw D reported that the reader returns no scan count, detector, source, beamsplitter or apodization for a UV-Vis `.sp`, where specio gives values for them (finding D-L2). No held-out file was opened.

**Corpus file used (new):** `pesp-figshare32604129-zb-uvvis` (figshare 32604129, CC-BY-4.0, V. Adoons: a Lambda 950 diffuse-reflectance spectrum written by UV WinLab 6.2, 2026), the first UV-Vis `.sp` in the development corpus. **Prior art consulted:** specio's `specio/plugins/sp.py` (BSD-3, commit 2963fd5) again.

**What was inferred from what:**
- The file's instrument record (block 123) holds only 35836 (2), 35837 model, 35838 serial, 35839 software and 35920 (empty text). The UV-Vis settings follow it under block 125 with other ids (35905–35973: slit and servo texts such as `3350/servo 860.8/4`, `UV/VIS`, the accessory text, numbers flagged set or unset). None of the infrared settings 35840 (scans), 35841 (detector), 35842 (source), 35843 (beamsplitter) or 35845 (apodization) is present. Every infrared development file holds all five.
- specio takes these settings by their position among the members of block 122, not by id, so on this file it reports the 9th, 11th, 12th, 13th and 15th members of other blocks: `detector` `3350/servo 860.8/4`, and 0 for scans, source, beamsplitter and apodization.

**Decided:** no reader change. A field the file does not hold stays absent. `oracle/spectro.py` now takes specio's positional infrared settings only when the file holds all five blocks (a u16 id, an i32 length and a `0x75xx` member type), so the oracle no longer expects them from a UV-Vis file. specio's spectrum (801 values) equals ours, and the depositor's ASCII export `ZB.asc` agrees to its six decimals.
