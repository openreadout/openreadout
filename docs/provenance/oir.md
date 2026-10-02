# Provenance log — Olympus/Evident OIR

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`, all CC-BY-4.0):
- `zenodo13680725-map-a01`, `zenodo13680725-tiling-brain-0001`, `zenodo13680725-stitch-a01-g001` + its continuation file `zenodo13680725-stitch-a01-g001-part1` (Zenodo 13680725, Nicolas Chiaruttini and Lucie Dixsaut; mirrored on the OME server under `Olympus-OIR/gh-4205/`)
- `zenodo12773657-dapi-mcherry-4z-5lambda`, `-3t-4z-5lambda`, `-22lambda` (Zenodo 12773657, Nicolas Chiaruttini)
- `ome-etienne-amy-slice-z-stack-0001`, `ome-etienne-coupe-shg-stack-0001`, `ome-etienne-venus-stack` (OME sample images `Olympus-OIR/etienne/`, COPYING = CC-BY-4.0, Étienne Labrie-Dion)
- `ome-imagesc105684-interval-30sec`, `-interval-30sec-z-stack`, `-interval-freerun` (OME sample images `Olympus-OIR/imagesc-105684/`, COPYING = CC-BY-4.0, Chin Hung; Zenodo 14417737)
- `zenodo16598994-fibre-0001` (Zenodo 16598994, Rémy Dornier)
- Searched (Zenodo API, queries `oir`, `olympus fluoview oir`, 2026-09-22): Zenodo 18303030 (76 FV OIR files of 170–1000 MB each) was left out for size; no other licensed multi-file (continuation) series than `Stitch_A01_G001` exists.

**Method, hex dumps first** (`xxd` and a 40-line Python block walker in the session scratchpad, before reading any prior art):
- Every file starts with the 16 ASCII bytes `OLYMPUSRAWFORMAT`, then little-endian words `12, 0, 1, 2` (constant in all 13 files), then at 0x20 a u64 equal to the file size and at 0x28 a u64 offset near the end of the file. At that offset: u32 `0xFFFFFFFF`, then u64 offsets to the end of the file. At 0x30 a u32 equals the number of those offsets (4 in `map-a01`, 2017 in `4z-5lambda`, 541 in `tiling-brain-0001`). At 0x40 a u64 is either `0xFFFFFFFFFFFFFFFF` (`map-a01`) or the offset of the block whose payload starts `BMP BM` (a Windows bitmap thumbnail, 116×86×24 bit in `4z-5lambda`). At 0x48 the 8 ASCII bytes `FLUOVIEW`. The first block is at 0x60.
- Each offset points at a block `u32 payload_len, u32 kind, payload`. Blocks tile the file contiguously from 0x60 to the index (verified in every file: each block ends where the next listed one starts, the last one ends at the index offset). Kinds seen: 0 (XML documents), 1 (one XML document, `lsmframe:frameProperties`), 2 (`BMP ` + bitmap), 3 (a short record with an ASCII name), 4 (raw samples), 5 (empty, `payload_len` 0, always right after the pixel blocks of one frame).
- Kind-3 records are `u32 a, u32 b, u32 name_len, name`; the next listed block is always kind 4 with `payload_len == b`. Across the 47 records of one plane in `4z-5lambda`, `a` runs 0, 11264, 22528, … 518144 and `b` is 11264 except the last (6144): `a` is the byte offset of the following pixel block inside its plane and `b` its length; 46·11264 + 6144 = 524288 = 512·512·2. In `tiling-brain-0001` the two channels' chunks alternate (A0, B0, A1, B1), so `a` and not block order places a chunk. Names are `l001z001_0_1_<uuid>_<chunk>`, `z007_0_1_<uuid>_<chunk>`, `t003z002_0_1_<uuid>_<chunk>` (1-based lambda/z/t indices, a fixed `_0_1` pair, the channel uuid, the chunk number) and `REF_LSM0_<uuid>_<chunk>` (a second, single-plane image per channel).
- Kind-0 and kind-1 payloads: 36 bytes of words, then `u32 n` and an n-byte ASCII XML document; kind-0 blocks carry many such documents, each preceded by its u32 length (checked: the four bytes before every `<?xml` equal the document's length, in all files). Root elements seen: `fileinfo:fileInfomation` (sic), `lsmimage:imageProperties`, `annotation:annotationStore`, `overlay:contents`, `lut:LUT` (each preceded by `u32 36` + the ASCII uuid of the channel it belongs to), `lsmimage:lsmChannel`, `base:imageDefinition`, `event:eventList`. Two kind-0 blocks exist per file: one after the first frame and one at the end; the last one is the complete one (e.g. its `imageProperties` is 48 862 bytes vs 48 049).
- `map-a01` has no kind-3/4 blocks at all: its frame properties declare a 45858×30169 canvas but no pixels are stored (an overview map of a tiling acquisition).

**Prior art consulted afterwards** (BSD-3-Clause, may be read; no code copied): `oirfile` 2026.9.6 module docstring and source (`oirfile.py`), Christoph Gohlke, https://github.com/cgohlke/oirfile, installed in `oracle/.venv`. What it confirmed or added:
- The header layout (file size and index offset at 32..48; "unknown fields (12, 0, 1, 2)"), the `0xFFFFFFFF` index marker, the block kinds (its names: metadata, frame properties, bitmap, uid, pixel, null) and the pairing of kind-3 and kind-4 blocks. oirfile ignores the two words we identified as plane offset and chunk length; it concatenates chunks in index order.
- Continuation ("companion") files: `<basename>_00001`, `_00002`, … next to the `.oir`, without extension, same binary layout, holding further pixel and frame-property blocks; the main file holds the metadata. oirfile stops at the first missing number.
- Plane grouping by the lambda/z/t indices in the name and the channel uuid; dimension order T, L, Z, C (slowest to fastest) in its arrays; distinct index values mapped to consecutive positions.
- Sample type from `depth` in the frame properties' `imageDefinition`: 1 → uint8, 2 → uint16, 4 → float32 (no corpus file has depth 4: **inferred from oirfile only**).
- Channel list: elements named `channel` with an `id` attribute in `imageProperties`, first occurrence per id, stably sorted by the `order` attribute; the uuids that have pixels are taken in that order.
- Pixel size from the first `length/x`, `length/y` in `imageProperties`; per-frame positions from `frameProperties/axisValue/{axisType,position}` (`TIMELAPSE` in ms, `ZSTACK` in µm, `LAMBDA` in nm); acquisition time from `creationDateTime`.
- Reference images (`REF_` names): geometry from the `base:imageDefinition` document when its width × height matches the stored samples.
- Line scans (`Y` taller than the frame height when a plane holds more bytes than width × height × depth) — no corpus file; we only take the plane height from the stored bytes, as oirfile does.

The Bio-Formats format page (https://bio-formats.readthedocs.io/en/stable/formats/olympus-oir.html) was read for the list of extensions.

**Inferred by us, not in oirfile:** the plane-offset/length meaning of the two kind-3 words (used to place chunks and to check coverage); the header block count at 0x30 and thumbnail offset at 0x40; the `u32 36 + uuid` prefix that ties each `lut:LUT` document to a channel; the choice of the last kind-0 block as authoritative; channel colour from the LUT's `lut:name` when it is a plain colour name.

**Validation (2026-09-22, same session, after merging origin/main).** Corpus harness (release, all 13 OIR files present): pass; 502 planes (main and reference images, up to 64 per image in (c, z, t) order) bit-identical to oirfile 2026.9.6, including the two-file `zenodo13680725-stitch-a01-g001` series (60 planes in the `.oir`, 46 in `_00001`); the overview map `zenodo13680725-map-a01` has no pixel blocks and both readers expose no image. Sizes, pixel type and physical sizes match oirfile for every image. `check` reports no errors on any corpus file (one `trailing_bytes` warning: 237 stale bytes after the recorded end of `Stitch_A01_G001.oir`). Robustness (session scratchpad copies, not committed): a copy truncated to 9 MB of `4z-5lambda` opens by walking the block chain and `check` reports `truncated`, `bad_index`, `incomplete_plane` and `missing_planes` (exit 4); the `.oir` of the two-file series without its `_00001` opens with 60 planes and `check` reports `missing_planes` naming the absent continuation file (exit 4).

## 2026-09-22 — robustness (fuzzing)

**Scope:** overflow check only; nothing the reader infers changed. The `whole_oir` fuzz target (3 min) found `index offset + 4` overflowing `u64` for an index offset near 2^64 in a 102-byte file; the comparison saturates now and the index is reported unusable (block-chain walk). Fixture: `crates/openreadout-oir/tests/fixtures/malformed/file-fuzz-index-offset-overflow.oir` (fuzzer output from a seed cut from `zenodo13680725-map-a01.oir`, CC-BY-4.0). **Prior art consulted:** none.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the FLUOVIEW major version, lambda scans, reference images, tiles, colour type, metadata-only files and continuation files. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — differential cross-check with Bio-Formats

**Corpus files:** every development file of this format on disk up to 400 MB (`corpus/oracle/second/*.json`); no held-out file.
**Prior art consulted:** Bio-Formats 8.5.0 (GPL) `showinf`/`bfconvert`, run as black boxes by `oracle/second_opinion.py`.
**Observations and adjudications:** `corpus/oracle/second/adjudications.toml` (differences are conventions or decoder rounding of the second reader; files whose series Bio-Formats groups differently are not compared).
**Inferred.** Nothing new; no reader change.
