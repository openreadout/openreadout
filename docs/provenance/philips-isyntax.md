# Provenance log — Philips iSyntax (`.isyntax`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-12 — initial reader (Richard Zimring with Claude as assistant)

**Corpus file:** `zenodo5037046-isyntax-testslide` — `testslide.isyntax` from Zenodo record 5037046 ("Testing whole slide image for OpenPhi", MIT licence on the record), 495 MB, written by a Philips UFS scanner (software 1.8.6824), data model 5.0, compressor 16.

**Prior art consulted:** libisyntax by Pieter Valkema and contributors, BSD-2-Clause, https://github.com/amspath/libisyntax at commit 92471dbf6c70 (2026-07-03): `src/isyntax/isyntax.c`, `isyntax.h`, `isyntax_dwt.c` (its 5/3 lifting comes from OpenJPEG, BSD-2-Clause), `isyntax_reader.c`, `libisyntax.c`. The decoder in `crates/openreadout-wsi/src/isyntax/` is a port of its decoding path and keeps its licence (`NOTICE`, `licenses/libisyntax-BSD-2-Clause.txt`). No Philips SDK or Philips document was used. libisyntax names a Philips format document; it could not be reached and was not read.

**Oracle:** libisyntax run as a tool through pyisyntax 0.1.7 (anibali/pyisyntax, MIT; it bundles libisyntax), `read_region` at every level used by the region test.

**What came from where:**
- From libisyntax: the file layout (XML header ended by byte 0x04, then a seek table of 40-byte entries tagged 0x301D,0x2015, then the code blocks); the meaning of the header attributes it reads (image type, block header table and its 48-byte partial entries, block header templates and their dimension ranges, the general header's ranges, the dimension scale factor as micrometres per pixel, the compression method numbers 16 and 19, the base64 label and macro JPEGs and ICC profile, the interface version that separates data model 5 from 100); the per-level padding `3·2^levels − 3` and the code-block grid offsets; the block id rule that indexes the seek table; the code-block codec (bit-plane masks, a Huffman tree stored as a walk, zero runs, bit planes in 4 × 4 snake order, signed magnitude); the tile reconstruction (LL from the parent tile's inverse transform, four-coefficient margins from the eight neighbours, missing neighbours as LL 255 for Y and 0 elsewhere, the 5/3 inverse transform starting on an odd sample, the output window from coefficient 7, Y as an absolute value, YCoCg to RGB with arithmetic shifts and clamping to 0–255); white for tiles that do not exist; and the region origin (level-0 padding shifted right by the level).
- From the corpus file (its XML header read as text): every attribute name and value listed in `docs/formats/philips-isyntax.md` (one WSI, one LABELIMAGE and one MACROIMAGE; 27 block header templates of 128 × 128; 8 scales; 0.25 µm per pixel; the image ranges x 13531–52442, y 22053–96804).
- Not implemented, because no public file shows it: data model 100 and later (cluster headers) and compressor 19. The reader refuses them (exit 6).
