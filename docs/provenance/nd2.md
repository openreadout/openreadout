# Provenance log — Nikon ND2

Rules: `docs/legal/clean-room-policy.md`.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Corpus files used:** `aics-ND2-dims-c2y32x32`, `aics-ND2-dims-t3c2y32x32`, `aics-ND2-dims-p1z5t3c2y32x32`, `aics-ND2-dims-p4z5t3c2y32x32`, `aics-ND2-dims-p2z5t3-2c4y32x32`, `aics-ND2-dims-rgb`, `aics-ND2-dims-rgb-t3p2c2z3x64y64`, `aics-ND2-maxime-BF007`, `aics-ND2-jonas-header-test2`, `aics-ND2-aryeh-but3-cont200-1` (BSD-3, Allen Institute), `ome-aryeh-MeOh-high-fluo-003`, `ome-jonas-nd2Test-Exception-2`, `ome-jonas-nd2Test-Exception9-e3` (CC-BY-4.0, OME sample images).

**Prior art consulted (documentation and BSD source):**
- `nd2` Python package, Talley Lambert et al., BSD-3-Clause, https://github.com/tlambert03/nd2 — the chunk magic `0x0ABECEDA`, the signature strings, the fact that the chunk map is located from the last 8 bytes, the existence of a rescue scan, the loop-type numbering (time, XY position, Z stack, NE time), and the note that the package "is not affiliated with Nikon" while acknowledging assistance from SDK developers at Laboratory Imaging (a provenance nuance recorded here; the package carries no Nikon EULA).
- Bio-Formats format page (documentation page only): https://bio-formats.readthedocs.io/en/stable/formats/nikon-nis-elements-nd2.html — two format generations, the legacy JPEG 2000 wrapper.

**Method:** hex-dumped the first chunk and the file tail of the smallest file, read the chunk map, and wrote a 40-line LV decoder from the byte pattern `type, name_len, UTF-16 name, value` (the level type's trailing u64 table was found by observing where the next sibling started). Verified for each file above that `8 + uiHeight × uiWidthBytes` equals the `ImageDataSeq|0!` payload length, that the number of `ImageDataSeq|N!` chunks equals `uiSequenceCount`, and that the product of loop counts equals `uiSequenceCount`. The frame-index flattening order (outermost loop slowest) is confirmed by matching per-frame plane hashes against the oracle for `T(3) × XY(4) × Z(5)`.

**Inferred, awaiting corroboration:** `ePixelType` 2 = float; `eCompression` 0/1 semantics; whether `uiWidthBytes` padding ever differs from `uiWidth × uiComp × bytes`.

## 2026-09-22 — corroboration and corrections

- **Version 2.x files** (`Ver2.1`, e.g. `aics-ND2-jonas-header-test2`) store metadata as XML "variant" documents in chunks named without the `LV` suffix (`ImageAttributes!`, `ImageMetadata!`, `ImageMetadataSeq|0!`, `ImageCalibration|0!`, `ImageTextInfo!`); the element `runtype` attribute gives the scalar type and byte arrays are base64. Derived from the chunk payloads directly (they are plain XML).
- **NE time loops** (`eType` 8): the effective frame count is the sum of `uiCount` over periods flagged valid in `pPeriodValid`, not the loop's own `uiCount` (which counts all periods). Derived from `aics-ND2-jonas-header-test2` where `uiCount` is 6, only the first period (4 frames) is valid, and the file holds 4 × 5 frames.
- **Lossless frames** (`eCompression` 0): the block after the 8-byte timestamp is a zlib stream (`78 01`) that inflates to `uiHeight × uiWidthBytes`. Derived from `ome-jonas-nd2Test-Exception-2`.
- **Oracle disagreement resolved with a third reader:** on `ome-jonas-nd2Test-Exception9-e3` the `nd2` package returns samples offset by one byte from ours. Bio-Formats 8.5.0 (`bfconvert`, run as a black box) produces a plane byte-identical to ours, so the manifest marks that file `oracle_skip` and the corpus report records the reason.

## 2026-09-22 — completing the reader: per-frame records, channels, loop tree, legacy files, events, ROIs (Richard Zimring with Claude as assistant)

**Corpus files used:** all smoke and standard ND2 files: the ten `aics-ND2-*` files; `ome-aryeh-MeOh-high-fluo-003/007/011`, `ome-aryeh-Time-sequence-24`, `ome-aryeh-b16-14-12`, `ome-aryeh-por003`, `ome-aryeh-weekend002`, `ome-jonas-control002`, `ome-jonas-header-test1`, `ome-jonas-nd2Test-Exception-2`, `ome-jonas-nd2Test-Exception61`, `ome-jonas-nd2Test-Exception9-e3`, `ome-karl-sample-image` (CC-BY-4.0, OME sample images). Added to the manifest (CC-BY-4.0, Zenodo): `zenodo8161776-VPA002` (Jacqui Ross), `zenodo21162526-NDacquisition`, `zenodo21162526-nested-loop`, `zenodo21162526-nd2jobs` (Maarten W. Paul).

**Prior art consulted (BSD-3 source and documentation of the `nd2` package 0.11.3, Talley Lambert, https://github.com/tlambert03/nd2, installed in `oracle/.venv`):** `_parse/_parse.py` (loop-tree walk: spectral loops do not add a level, same-kind siblings keep the larger count, `pItemValid` filters XY points, NE-time periods filtered by `pPeriodValid`; wavelength rule "probe spectrum, else first filter, point with the largest `dTValue`"; `uiColor` as ABGR; events and ROI tree layout; text-info item numbering), `_sdk_types.py` (loop-type numbering 1–10, modality-mask bit values and the `eModality` → mask table, event-meaning codes), `structures.py` (ROI shape/role/scope codes), `_parse/_clx_lite.py` (LV type 76 = zlib-compressed block after a 10-byte header; byte arrays that hold nested LV), `_readers/_modern/modern_reader.py` (custom-data tag table in `CustomDataVar|CustomDataV2_0!`, `CustomData|AcqTimesCache!`, `dTimeAbsolute` as a Julian day), `_readers/_legacy/legacy_reader.py` and `_parse/_legacy_xml.py` (legacy box map at the end of the file, `jp2c` codestreams per channel per frame). Its module name and comments ("the SDK looks for it") indicate that some of these tables were written with knowledge of the Nikon SDK's behaviour (the package README acknowledges help from Laboratory Imaging developers, noted in the first entry); we took only numeric codes and structural rules, confirmed each against corpus files where a file exercises it, and gave every value our own name. Bio-Formats 8.5.0 `showinf` was run as a black box on `ome-aryeh-b16-14-12` (it also reports T = 50).

**Derived from the files (hex dumps and decoded trees):**
- Only frame 0 has an `ImageMetadataSeqLV|N!` chunk in every corpus file; per-frame values live in `CustomData|<tag>!` arrays described by `CustomTagDescription_v1.0/Tag<n>/{ID, Type, Size, Desc, Unit}` (Type 3 = f64, 2 = i32 read from the arrays' sizes; Type 1 = UTF-16 strings from the `nd2` source, no corpus file). The stage-Z column is `Z` or a numbered drive (`ome-karl-sample-image` has only `Z2`, "Ti ZDrive").
- `dTimeAbsolute` is UTC: `ome-karl-sample-image` gives 09:15:06.980 UTC where `TextInfoItem_9` says 11:15:06 local (UTC+2 in June). `ome-jonas-control002` (397.7) and `ome-jonas-nd2Test-Exception9-e3` (1721424.5) carry uninitialized values, hence the 1900–2100 plausibility window. `TextInfoItem_13` is the objective (`Plan Apo λ 20x`), not the software version as the previous code assumed; the version is `TextInfoItem_14`.
- Camera settings: `sPicturePlanes/sSampleSetting/aK/{pCameraSetting, dExposureTime, sSpecSettings, pObjectiveSetting}` in v3 files; a per-plane `sCameraSetting/{sCameraName, dExposure, dGain, dCamBinningX/Y, sSpecSettings}` in v2 and legacy files. Older files store wavelengths as integral `uiWavelength` (e.g. 488/520 in `ome-jonas-nd2Test-Exception-2`), which the `nd2` package does not read; we do.
- `uiRepeatCount` 65 on the Z loop of `ome-jonas-control002` equals the enclosing NE-time count; no file has a repeat count on the root, so it is reported, not multiplied. `pItemValid` on XY loops: `ome-karl-sample-image` declares 15 points, one valid, 21 frames. The v2 file `ome-jonas-nd2Test-Exception-2` has a spectral loop outermost.
- Legacy files (`aics-ND2-aryeh-but3-cont200-1`, `ome-aryeh-Time-sequence-24`, `ome-aryeh-b16-14-12`, `ome-aryeh-por003`, `ome-aryeh-weekend002`): the tail signature `LABORATORY IMAGING ND BOX MAP 00` + u64 distance; map entries are 16 bytes (type, tag, u64 offset; the `nd2` source reads only a u32 of the offset); XML roots (table in `docs/formats/nd2.md`); `AIM1` wraps the tree in `vMetadata` next to `vUnknownData`; `AIMD` may list `LoopNo00`, `LoopNo01` as siblings (`b16-14-12`); `ppNextLevelEx` may repeat `no_name` (`por003`). Codestreams are raw J2K; `ihdr` gives the bit depth.
- JPEG 2000 decoder choice: `dicom-toolkit-jpeg2000` 0.5 (MIT OR Apache-2.0, a maintained fork of `hayro-jpeg2000`, `forbid(unsafe_code)`, no dependencies with default features off, MSRV 1.80) decodes all 15 sampled codestreams of the five legacy files bit-identically to `imagecodecs.jpeg2k_decode` (OpenJPEG), and survived 750 randomly truncated or corrupted codestreams with errors and no panics. `hayro-jpeg2000` (MSRV 1.92, 8-bit output) and `j2k` (frames-sg, MSRV 1.96) were not needed; `jpeg2k`/`openjpeg-sys` are C bindings.
- Lossy compression (`eCompression` 1): no sample. Every modern corpus file and the five ND2 files of Zenodo records 21162526, 8161776 and 5277605 (read over HTTP range requests) have `eCompression` 2; the `nd2` package's published sample metadata lists none and its reader has no lossy path. The codec is left undetermined.
- Truncation: frame chunks whose declared `data_len` runs past the end of the file now fail `read_plane`; a legacy file without its box map fails `check` as `truncated`; `uiSequenceCount` 0 (never finalized) falls back to the number of frame chunks found.

**Validation:** `cargo test -p openreadout-corpus-tests --features corpus` compares, per file, geometry, pixel type, physical sizes and up to 64 plane hashes per image (evenly spaced over the whole (c, z, t) range so the tail of the frame order is covered), plus channel names, colours and wavelengths, `acquired_at` and 24 sampled per-frame records (time and every custom-data column) against the `nd2` package (`oracle/gen.py`, `nd2_meta`). Result on 2026-09-22: 26 ND2 files pass (1392 plane hashes, 627 metadata values), 1 is skipped for pixels (Exception9-e3, see above). `zenodo8161776-VPA002` has no metadata oracle: `nd2` 0.11.3 raises IndexError building its channels (pseudo-wavelengths for a fourth RGB plane); its pixels match.

## 2026-09-22 — `acquired_at` fallback from the date text

Bug report (from main, where `acquired_at` was the raw `TextInfoItem_9` string such as `9/28/2021  9:34:47 AM`): on this branch `acquired_at` already comes from the Julian day. Added a fallback for files whose Julian day is missing or implausible: the text is normalized to ISO-8601 local time without an offset. Derived from the `TextInfoItem_9` values of all 27 ND2 corpus files compared with their Julian days: month-first US texts (`aics-ND2-dims-*`, `aics-ND2-maxime-BF007`, both with `AM`/`PM`), day-first texts with `AM`/`PM` (`aics-ND2-jonas-header-test2`, `ome-jonas-header-test1`: `06/03/2009` = 6 March), day-first 24-hour texts (`ome-aryeh-MeOh-*`, `ome-karl-sample-image`, `zenodo21162526-*` including `3-7-2026` = 3 July), lowercase `pm` (`zenodo8161776-VPA002`). In every file where both exist, text minus Julian day is a whole-hour UTC offset plausible for the lab (−4 h and −5 h US East, −7 h US West, +1 h/+2 h Europe and Israel, +12 h New Zealand), within 0–2 minutes (the text is sometimes written when the document is saved), confirming that the Julian day is UTC and the text local time. Because the locale is not recorded, ambiguous dates are not guessed. No prior art consulted.

## 2026-09-22 — metadata fidelity: refractive index exposed (Richard Zimring with Claude as assistant)

**Corpus files used:** all ND2 files (conformance walk in `crates/openreadout-corpus-tests/tests/metadata.rs`). **Prior art consulted:** none new; the OME 2016-06 schema (open standard) for `ObjectiveSettings/@RefractiveIndex`.

- `FrameMeta.refractive_index` (`dRefractIndex1`, else `dRefractIndex`, already parsed) is now reported as `images[].extra.refractive_index` when positive, so the OME-TIFF export can write `ObjectiveSettings/@RefractiveIndex`. No parsing logic changed.
- When `acquired_at` falls back to the local-time text, `info` notes that it carries no time zone (the normalization rule for zone-less timestamps). No parsing logic changed.

## 2026-09-22 — performance: channel extraction without per-pixel indexing (Richard Zimring with Claude as assistant)

**Corpus files used:** all ND2 files (corpus harness), `ome-karl-sample-image` and `ome-aryeh-MeOh-high-fluo-003` for timing. **Prior art consulted:** none.

Performance only; no parsing logic or interpretation changed. When a channel covers every component of a pixel and rows are unpadded, the frame bytes are returned as the plane instead of being copied pixel by pixel; otherwise rows and pixels are walked with chunk iterators instead of computed indices. Criterion `decode/nd2-raw` on `ome-aryeh-MeOh-high-fluo-003`: 460 MiB/s to 21.6 GiB/s. The corpus harness is bit-exact as before.
## 2026-09-22 — robustness hardening

**Scope:** no change to what the reader infers from a file; only bounds checks, checked
arithmetic, allocation caps (`openreadout_core::limits`) and error paths, so malformed input
yields `corrupt_file` / `unsupported_feature` instead of a panic, overflow or out-of-memory abort.
Chunk-map and metadata chunk sizes are capped; `uiComp` above 4096 is corrupt; a channel's samples must lie within `uiComp`; `uiWidthBytes` must hold one row; the loop tree may not claim more positions than there are chunks; LV levels nest at most 64 deep and variant XML at most 128.

**Corpus files used:** the smoke tier (malformed-file matrix: truncations, byte flips) and the
synthetic minimal files written by `fuzz/seeds.py`; fuzz findings are in
`crates/openreadout-nd2/tests/fixtures/malformed/`.

**Prior art consulted:** none (no format knowledge was needed; the limits follow from our own layout notes above).

## 2026-09-23 — robustness: a missing chunk map fails `check`

**Scope:** `check` severity only; reading is unchanged. The extended malformed-file matrix truncated `aics-ND2-dims-*` files (BSD-3-Clause) to 90 % and 50 %: the cut removed only the chunk map and the metadata after the frames, the reader recovered every frame by scanning, and `check` passed with two warnings. The chunk map is the last thing an acquisition writes, so a file without a usable one was cut short: `check` now adds a `truncated` error (exit 4) whenever chunks were recovered by scanning, as it already did for a legacy ND2 without its box map. **Prior art consulted:** none.

## 2026-09-23 — growing files (`write_state`)

**Scope:** a new, read-only judgement on top of the existing rescue scan; reading is unchanged. `Dataset::write_state` reports a chunked ND2 whose chunk map is unusable (chunks recovered by scanning) as unfinished when every frame chunk before the last one is whole; complete frames are frame chunks whose payload lies inside the file and holds a full frame; the expected plane count is the loop tree's product times the channels.

**Corpus files used:** `aics-ND2-dims-p1z5t3c2y32x32.nd2` (BSD-3-Clause; replayed chunk by chunk in `crates/openreadout-corpus-tests/tests/live.rs`).

**Inferred from the file:** the chunk layout of the corpus file (signature, metadata chunks, `ImageAttributesLV!` whose frame count reads as 0 so the reader counts frames, 15 frame chunks, a second metadata block with another `ImageAttributesLV!`, then the chunk map and the trailing offset): the chunk map is the last thing written and the frame count is filled in at the end. Nothing else.

**Prior art consulted:** none.

## 2026-09-23 — performance / robustness merge

Reconciled the existing performance branch with main; no new format layout was inferred.
Inputs: existing committed synthetic and malformed regression fixtures and the shared public
corpus (ids/licences unchanged in `corpus/manifest.toml`). Prior art: repository code and notes;
weezl 0.2.1 public buffer API (https://docs.rs/weezl/0.2.1/weezl/decode/struct.IntoStream.html,
MIT OR Apache-2.0) for a reusable bounded LZW staging buffer. Retained file-relative allocation
limits, checked geometry, bounded decoder output and capped sidecar reads alongside parallel
decoding and copy avoidance. The common plane-size guard also enforces the 4 GiB ceiling.

## 2026-09-23 — objective from the optics text

**Scope:** `images[].objective` when the frame metadata and the calibration chunk carry no objective. The text-info item `TextInfoItem_13` (already listed as "the optics" in `docs/formats/nd2.md`) holds the objective name, and `TextInfoItem_5` (the description) a `Numerical Aperture: 1.4` line. The objective name falls back to `TextInfoItem_13`, the NA to that description line; the magnification is parsed from the name as before, now also when the `x` is followed by letters (`Plan Fluor 20xC ELWD ADL` → 20; previously only a word ending in `x` counted).

**Corpus files used:** `ome-jonas-control002` (only `TextInfoItem_13` = `Plan Apo 60x Oil DIC H` and `Numerical Aperture: 1.4`; `wsObjectiveName` and `sObjective` empty, `dObjectiveMag`/`dObjectiveNA` = −1), `aics-ND2-jonas-header-test2` and `aics-ND2-dims-*` (where `TextInfoItem_13` equals `wsObjectiveName`/`sObjective`, which confirms the item's meaning), `ome-aryeh-por003`, `ome-aryeh-b16-14-12`, `ome-aryeh-weekend002`, `aics-ND2-aryeh-but3-cont200-1` (`20xC`/`40xC` names; `por003`'s description says `Numerical Aperture: .45`, equal to its `dObjectiveNA` 0.45).

**Prior art consulted:** `nd2` 0.11.3 (BSD-3-Clause, <https://github.com/tlambert03/nd2>), `nd2/_parse/_parse.py` `load_text_info` and `nd2/structures.py` `TextInfo`, which name item 13 `optics` (and items 3 and 4 `sampleId` and `author`; no corpus ND2 fills those, so they are not mapped). The oracle JSON (`text_info.optics`) is written by the same library and is the check.

## 2026-09-24 — RGB sample order (Richard Zimring with Claude as assistant)

**Trigger.** Finding M1 of `docs/benchmark/heldout-2026-09-24.md`: an RGB ND2 (colour camera) exported with red and blue swapped compared with NIS-Elements' own export. Per the held-out rules that file was not opened; the order was established on development-corpus files.

**Corpus files used:** `zenodo8161776-VPA002` (Zenodo 8161776, CC-BY-4.0: four RGB picture planes named DAPI, GFP, Texas Red and DIC in the record description, DS-Ri2 colour camera, modern container), `aics-ND2-dims-rgb` and `aics-ND2-dims-rgb-t3p2c2z3x64y64` (DS-Fi3, brightfield, modern), `ome-aryeh-Time-sequence-24` (legacy JPEG 2000 container, colour camera, phase contrast).

**Prior art consulted:** `nd2` 0.11.3 (BSD-3-Clause, https://github.com/tlambert03/nd2): `_parse/_parse.py` `RGB_COLORS = (420.0, 515.0, 590.0)` indexed by component (the package's own pseudo-wavelengths put blue at component 0 and red at component 2), `_nd2file.py` (it returns frames in stored order and labels the `S` coordinate Red, Green, Blue — the label contradicts its wavelengths), `_readers/_legacy/legacy_reader.py` (no reordering for legacy files).

**Observed:**
- `zenodo8161776-VPA002`, per picture plane, mean of samples 0/1/2 in stored order (all pixels; brightest 1 % in brackets): DAPI 22.0 / 0.8 / 0.0 (97.5 / 7.1 / 0.0), GFP 0.0 / 24.6 / 2.9, Texas Red 1.8 / 1.2 / 30.7 (9.9 / 8.5 / 120.9), DIC 96.7 / 115.9 / 138.5. The blue dye lights sample 0 and the red dye sample 2: stored order is B, G, R. The DIC plane then reads R 138.5, G 115.9, B 96.7, the warm cast of an un-white-balanced halogen/LED transmitted-light image.
- Legacy `ome-aryeh-Time-sequence-24`: every `jp2c` codestream is preceded by a `jp2h` header whose `colr` box declares the enumerated colour space 16 (sRGB), i.e. components R, G, B. Its content (phase contrast, samples 0 / 137 / 220 in stored order) does not settle the order by itself, and no legacy RGB file with a reference export or dye-coded planes is available.

**Decided.** Modern (chunked) 3-component planes are returned R, G, B (`bgr_to_rgb` on the frame samples); `info` states it per RGB image as `extra.sample_order` = `RGB` and `extra.stored_sample_order` = `BGR`. Legacy (JPEG 2000) planes keep the codestream order, which their JP2 colour box declares to be R, G, B (`stored_sample_order` = `RGB`, **inferred**, no reference). The corpus oracle (`oracle/gen.py` `nd2_`) reverses the `S` axis of nd2's modern RGB frames so the planes are compared as R, G, B; the oracle JSON of the three modern RGB files was regenerated (hashes only).

## 2026-09-24 — Plane-read throughput (no format change)

**Scope:** performance only; no new interpretation of the file. A multichannel frame is split into all its channels in one pass (`deinterleave.rs`, checked byte for byte against a naive gather in unit tests) and the other channels are kept for the next reads of that frame; frame chunks are found through a frame-number index built from the chunk map (`ImageDataSeq|N!` names, as before); the chunk header of a frame is read with one positional read and checked against the name the map gives (any other header layout takes the previous two-read path). Output unchanged: `check --planes` hashes identical on every ND2 of the corpus (`corpus_matches_oracle`, nd2: 27 files pass) and the region oracle of `ome-karl-sample-image`.

**Corpus files used:** `ome-karl-sample-image`, `zenodo21162526-nd2jobs`, `zenodo21162526-nested-loop`, `zenodo21162526-NDacquisition`, `ome-jonas-control002` (timings in `book/src/project/performance.md`). **Prior art consulted:** none.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the stored sample order, the loop kinds of `extra.loops`, multi-position files and the frame codec (`eCompression`, read from the dataset through `Dataset::assurance_observations`: 2 uncompressed, 0 zlib, 1 lossy; legacy files are JPEG 2000). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — differential cross-check with Bio-Formats

**Corpus files:** every development ND2 on disk up to 400 MB (`corpus/oracle/second/*.json`); no held-out file.
**Prior art consulted:** Bio-Formats 8.5.0 (GPL) `showinf`/`bfconvert`, run as black boxes by `oracle/second_opinion.py`.
**Observations.** OpenReadout agrees with Bio-Formats on every compared plane except the four colour-camera files: Bio-Formats splits the samples into channels in stored B, G, R order (`aics-ND2-dims-rgb`, `aics-ND2-dims-rgb-t3p2c2z3x64y64`), reads the interleaved samples of a JPEG 2000 RGB series as planes (`ome-aryeh-Time-sequence-24`) and reports one sample per pixel for the RGB picture planes of `zenodo8161776-VPA002`. On all four OpenReadout equals the nd2 library (with the R, G, B order of NIS-Elements' own export). Adjudicated in `corpus/oracle/second/adjudications.toml`.
**Inferred.** Nothing new; no reader change.

## 2026-09-26 — Filter-band wavelengths are band centres; one shared sample setting applies to every plane

**Why.** The per-field comparison with Bio-Formats (`oracle/metadata_compare.py`, docs/benchmark/microscopy-metadata.md): emission 500 vs 525 nm (`aics-ND2-aryeh-but3-cont200-1`, `ome-aryeh-weekend002`) and 615 vs 640 nm (`ome-aryeh-por003`); exposure missing on every channel but the first of `zenodo21162526-{nested-loop,NDacquisition,nd2jobs}`.

**Corpus files used:** those six; every development ND2 for the regression check. **Prior art consulted:** the `nd2` package (BSD-3-Clause), run for `metadata.channels[].channel.emissionLambdaNm` (None for the legacy aryeh files) and `unstructured_metadata()` (the `SampleSetting` entries).

**Observations.** In the aryeh files no fluorescent probe has a spectrum; the filter `GFP/fitc` has an excitation spectrum of two points, `eType` 2 at 450 nm and `eType` 3 at 490 nm, and an emission spectrum 500 / 550 nm, all with `dTValue` 0. "The point with the largest `dTValue`, first on ties" therefore picked the rising edge (450, 500 nm), an arbitrary end of the band. The three Zenodo 21162526 files (Nikon A1plus) have one `sSampleSetting` entry (`a0`) and every plane's `uiSampleIndex` is 0; the rule "sample = plane index when `uiSampleIndex` is 0" found no `a1`… for planes 1 to 4. `ome-karl-sample-image` has five sample settings (100, 50, 200, 20, 40 ms), one per plane, as we report (Bio-Formats reports 100 ms for plane 1 and nothing for planes 2–4).

**Rule implemented:** a filter spectrum made of a rising and a falling edge (`eType` 2 and 3) gives the centre of the edges as the excitation or emission wavelength (470 and 525 nm for `GFP/fitc`); the band stays in `emission_range_nm`. A probe spectrum's peak still takes precedence. When `sSampleSetting` holds a single entry, every plane uses it.

**ROIs on a real file (same day).** A remote survey (chunk map and `CustomData|RoiMetadata_v1!` only, over HTTP ranges) of 452 public Zenodo ND2 files found 10 with a non-empty ROI tree; `zenodo14231228-Sla2-WT-18-roi` (CC-BY-4.0) was added. Our `extra.rois` equals the `nd2` package's `rois` field for field (one global background rectangle). The stored centre and size are small signed fractions, not pixels or µm; their unit is not established and they are reported as stored (docs/formats/nd2.md). No ND2 with lossy frames (`eCompression` 1) was found among the 452; 3 files use NE-time sub-loops (6.5, 12.4 and 32.2 GB, too large to fetch within the budget).
