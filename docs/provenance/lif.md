# Provenance log — Leica LIF

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`):
- `ome-michael-PR2729-frameOrderCombinedScanTypes` (CC-BY-4.0, OME sample images)
- `aics-s-1-t-4-c-2-z-1`, `aics-s-1-t-1-c-2-z-1` (BSD-3-Clause, Allen Institute test resources)
- `bsst749-2a-ishi-hf-fshr-dmso`, `-dmso-fsh-5`, `-dyngo`, `-dyngo-fsh-5`, `bsst749-4i-bodipy-ctl` (EMBL-EBI terms, BioStudies S-BSST749)

**Prior art consulted** (documentation only, no code copied):
- `liffile` README and API docs, Christoph Gohlke, BSD-3-Clause, https://github.com/cgohlke/liffile — confirms: `0x70` block marker, `0x2A` delimiters, header size `2 × XML length + 5`, UTF-16 XML header, contiguous data blocks, version-2 files use 64-bit block sizes, the LIF/LOF/XLEF/XLLF/LIFEXT family, and the known gaps (mosaics, FLIM histograms, bit increments).
- Bio-Formats format page for Leica LIF (documentation page only): https://bio-formats.readthedocs.io/en/stable/formats/leica-lif.html — used for the list of related extensions.

**Method:** hex-dumped the file header and the first memory-block headers of every file above (`hexyl`, a 40-line Python walker). Confirmed for each file that (a) `block_len == 2 * xml_len + 5`, (b) every memory block header is `0x70, u32, 0x2A, u64, 0x2A, u32, utf16` with `block_len == 2 * id_len + 14`, (c) the chain of blocks ends exactly at the file size, (d) `Memory/@Size` in the XML equals the block's `data_len`, and (e) the image's `Memory/@Size` equals the product of `NumberOfElements` over all dimensions times channel count times sample width (e.g. 64·64·3·4·2·2·1 = 196608 for the OME file).

**Inferred, not yet corroborated:** the version-1 memory-block layout (u32 `data_len`) — no version-1 file in the corpus yet. The mapping of `DimID` 5–8 names is from `liffile` docs and is not exercised by any corpus file.

## 2026-09-22 — tile stitching, lambda/rotation/slice axes, LOF/XLEF family, LIFEXT, FLIM detection (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`):
- Existing: `aics-tiled`, `aics-merged-tiles` (BSD-3-Clause), `ome-michael-PR2729-frameOrderCombinedScanTypes` (CC-BY-4.0), `zenodo6606445-Project007` (CC-BY-4.0).
- Added today: `ome-imagesc-110520-AMR1` + `ome-imagesc-110520-AMR1-lifext` (CC-BY-4.0, OME sample images, also Zenodo 15353569), `zenodo14976703-Convalaria-LambdaScan` (CC-BY-4.0), `zenodo13752242-FLIM250523` (CC-BY-4.0).
- Searched and **not** added: the OME `Leica-XLEF/` tree (https://downloads.openmicroscopy.org/images/Leica-XLEF/) has no COPYING/license file, so it stays out of the manifest and was not downloaded. Zenodo search (queries `xlef`, `"lof" leica`, `.lof leica object file`, `lifext`, `FALCON FLIM lif`, `leica lambda scan lif`, 2026-09-22) found no licensed `.lof`/`.xlef`/`.xlif`/`.xlcf`/`.xllf` files; it found the lambda, FLIM and LIFEXT files above.

**Prior art consulted** (BSD-3-Clause, may be read; no code copied): `liffile` 2026.7.14 module docstring and source, Christoph Gohlke, https://github.com/cgohlke/liffile. What it told us:
- It does **not** stitch mosaics ("Unsupported features currently include XLLF, image mosaics and pyramids"); it exposes the tile axis as `M` and a `TileScanInfo` record with `FlipX`, `FlipY`, `SwapXY` flags and per-tile `FieldX/FieldY/PosX/PosY/PosZ`.
- DimID names 5–11: 5 emission wavelength, 6 rotation, 7 XT slices, 8 T slices, 9 excitation wavelength, 10 mosaic, 11 loop. It keeps each as its own array axis.
- LOF layout: a LIF-style header block whose text is `LMS_Object_File`, then `0x2A u32 0x2A u32` (two version numbers), then `0x2A u64` payload length and the payload, then a second `0x70 u32 0x2A u32` block with the UTF-16 XML (`LMSDataContainerHeader`, or in older files a bare `<Data>` fragment).
- XLIF/XLEF/XLCF are plain XML files (UTF-8 or UTF-16, recognised by the first four bytes). XLEF/XLCF list children as `Element/Children/Reference/@File` (relative, URL-quoted, may use `\`); XLEF has `Data/Experiment`, XLCF has `Data/Collection`. XLIF holds one `Element/Data/Image` whose `Memory` element's children carry `File`, `Offset`, `Size` (and `UUID`) attributes: frames of the memory block stored in `.lof` (or TIFF/JPEG/PNG/BMP) files next to it.
- LIFEXT: a LIF-style container whose XML root is `LMSDataContainerEnhancedHeader` (Version 1) with `ChildrenOf/@MemoryBlockID` naming the parent LIF image's memory block; its memory blocks use the 64-bit length layout regardless of the header version.
- FLIM/TCSPC: elements with `Data/SingleMoleculeDetection[@IsImage="true"]`; geometry in `Dataset/RawData/Dimensions/Dimension/{DimensionIdentifier,Size}`; `LaserPulseFrequency`, `ClockPeriod`, `VoxelSizeX/Y`; histogram bins = floor(1 / frequency / clock period). liffile itself refuses to decode the raw data (format described as patent-pending). Float channels with `Resolution` 16 are IEEE half floats.

**Black-box oracle runs** (GPL, run only): `bioio-lif` 1.5.0 on `ome-michael-PR2729-...` and `zenodo6606445-Project007`. It returns stitched mosaics of 127×127 (2×2 tiles of 64) and 1023×1023 (2×2 tiles of 512), i.e. it places tiles on the `FieldX/FieldY` grid with a one-pixel overlap and ignores the stage positions (Project007's positions put the tiles 460 px apart). On `zenodo14976703-Convalaria-LambdaScan` it reports `C=1` and returns only the first lambda plane. We do not follow either behaviour: both lose information.

**Method and inferences:**
- Hex dumps (`xxd`, a 30-line Python block walker in the session scratchpad) of the first 6 MB of `AMR1.lif` and 4 MB of `AMR1.lifext` (HTTP range requests before the full download) and of the lambda and FLIM files: every file follows the documented `0x70/0x2A` block layout; `AMR1.lifext` XML starts `<LMSDataContainerEnhancedHeader Version="1"><ChildrenOf MemoryBlockID="MemBlock_16">` and its first block (`MemBlock_17`, 55 050 240 B) uses a u64 length although the header says Version 1. The `.lif` XML never names its `.lifext`; the pairing is by file stem. The sidecar's images are 8-bit reduced copies (`R 1_pmd_0..5`, halving each level) and `*_histo` images of the LIF's tile scans and merged images.
- **Tile placement** (inferred from `aics-tiled` vs `aics-merged-tiles`, both from the same LAS X project): `aics-tiled` has 165 tiles of 512×512, `FlipX=FlipY=SwapXY=1`, `FieldX` 0..10, `FieldY` 0..14; `aics-merged-tiles` (LAS X's own merge) is 7666×5622. Pixel step = `Length / (NumberOfElements − 1)` = 2.00613e-7 m; stage steps are 1.02513e-4 m = 511.0 px. Brute-force matching of tile interiors against the merged image placed tile (FieldX=fx, FieldY=fy) at row `10 − fx`, column `14 − fy` with exact equality. Hence: negate `PosX` when `FlipX`, negate `PosY` when `FlipY`, then swap the two when `SwapXY`; subtract the minimum; divide by the pixel step; round half up. Pasting tiles in file order with later tiles overwriting earlier ones reproduces LAS X's merged image **bit for bit on all 4 channels** (0 of 43 098 252 pixels differ per channel). When only one flip flag is set together with `SwapXY`, which image axis it reverses is not corroborated by any corpus file (all three flags are equal in the only file that sets them).
- Project007 and AMR1 have their own LAS X `Merged` images (981×975, 2867×1956) that do **not** equal a position-based placement (Project007 positions give 972×972); LAS X evidently registers overlapping tiles there. We document this and do not attempt registration.
- Lambda (DimID 5) in `zenodo14976703-Convalaria-LambdaScan`: `Origin` 4.2e-7 m, `Length` 2.8e-7 m, 29 elements → 420..700 nm in 10 nm steps; `BytesInc` 524 288 = one 512×512 u16 plane. We expose lambda × channel as the channel axis (channel-major: `c * n_lambda + l`) and put the wavelength in the channel name and `emission_nm`.
- DimIDs 6, 7, 8: no corpus file has them. Their addressing uses the same `BytesInc` rule as every other axis (no evidence of anything else); rotation (6) and unknown ids become separate exposed images, XT/T slices (7/8) are folded into T. Validated only on synthetic files written by `crates/openreadout-lif/tests/fixtures/make_fixtures.py` and cross-read by liffile.
- FLIM: `zenodo13752242-FLIM250523` has 16 `SingleMoleculeDetection` elements (`Format` `LMSCOMPRESSED`), each followed in document order by child images (`Intensity` float32, `Fast Flim` etc. as Resolution-16 DataType-1 half floats, `Phasor Mask` u32).
- LOF/XLIF/XLEF/XLCF/XLLF: no licensed corpus file; the synthetic fixtures follow the liffile-documented structure above and are cross-read with liffile 2026.7.14 as the oracle. XLLF handling (treated like XLCF: follow `Children/Reference`) is inferred from the name only and marked so.

**Validation (2026-09-22, same session).** Corpus harness, all 18 LIF-family files present: pass; 2 473 planes bit-identical to liffile-read ground truth; 309 tile placements verified (each tile's uncovered region found unchanged at its offset in our stitched plane); 16 FLIM elements return `unsupported`. `aics-tiled` stitched planes hash equal to `aics-merged-tiles` (LAS X's merge) — checked both in `oracle/gen.py`'s independent numpy stitching and in `crates/openreadout-cli/tests/cli.rs`. `ome-imagesc-110520-AMR1`: 13 images, tile scans stitch to 2865×1945 / 1945×2865 / 2865×2865 (LAS X's registered merges: 2867×1956 / 1961×2862 / 2857×2871). `ome-imagesc-110520-AMR1-lifext` opened directly: 42 images in 6 `ChildrenOf` groups (liffile 2026.7.14 by itself lists only the first group's 7); mosaic pyramid levels inherit the parent's `TileScanInfo` (inferred; level 0 stitches to 1433×972, half the parent's 2865×1945, consistent with LAS X's merged level 0 at 1433×978). The `*_histo` images (65536×1, u32, mosaic axis) stitch on the field grid. Synthetic fixtures (`crates/openreadout-lif/tests/fixtures/`): LOF (current and bare-`<Data>` XML), XLIF with one and two `.lof` frames, XLCF, XLEF, lambda × 2 channels, rotation, XT/T slices, FlipX-only tile scan, half floats (including subnormals, infinities, quiet and signalling NaNs: widened like NumPy/F16C) and a FLIM element all match liffile.
## 2026-09-22 — robustness hardening

**Scope:** no change to what the reader infers from a file; only bounds checks, checked
arithmetic, allocation caps (`openreadout_core::limits`) and error paths, so malformed input
yields `corrupt_file` / `unsupported_feature` instead of a panic, overflow or out-of-memory abort.
The XML header is capped at 512 MiB; block ids above 4096 UTF-16 units are corrupt; `Version` is parsed from the first 400 bytes cut at a character boundary; plane offsets from `BytesInc` use checked arithmetic; zero-width planes are corrupt; at most 65 536 tiles are exposed per image; `Element` nesting is followed 256 levels deep.

**Corpus files used:** the smoke tier (malformed-file matrix: truncations, byte flips) and the
synthetic minimal files written by `fuzz/seeds.py`; fuzz findings are in
`crates/openreadout-lif/tests/fixtures/malformed/`.

**Prior art consulted:** none (no format knowledge was needed; the limits follow from our own layout notes above).

## 2026-09-23 — performance / robustness merge

Reconciled the existing performance branch with main; no new format layout was inferred.
Inputs: existing committed synthetic and malformed regression fixtures and the shared public
corpus (ids/licences unchanged in `corpus/manifest.toml`). Prior art: repository code and notes;
weezl 0.2.1 public buffer API (https://docs.rs/weezl/0.2.1/weezl/decode/struct.IntoStream.html,
MIT OR Apache-2.0) for a reusable bounded LZW staging buffer. Retained file-relative allocation
limits, checked geometry, bounded decoder output and capped sidecar reads alongside parallel
decoding and copy avoidance. The common plane-size guard also enforces the 4 GiB ceiling.

## 2026-09-23 — acquisition mode, and the LAS AF hardware-setting list

**Scope:** `images[].channels[].acquisition_mode`, and the objective and system name of files written by LAS AF (the `HardwareSettingList` attachment), for the experiment model.

**Corpus files used:** all LIF inputs (the XML header read with a few lines of Python: the UTF-16 XML after the 0x70/0x2A header, as `docs/formats/lif.md` describes). `zenodo6606445-Project007`, `aics-merged-tiles`, `aics-tiled`, `ome-imagesc-*`, `zenodo14976703-*`, `zenodo13752242-*`, `ome-michael-*` (TCS SP8 / STELLARIS / SIMULATOR) carry an `ATLConfocalSettingDefinition` (with `Pinhole`, `ScanSpeed`, `Zoom`) inside `Attachment[@Name="HardwareSetting"]`; `aics-s-1-t-1-c-2-z-1` and `aics-s-1-t-4-c-2-z-1` (`SystemTypeName="AF 6000LX"`, a camera system) an `ATLCameraSettingDefinition` (with `TheoCamSensorPixelSize*`, `CameraFormat`). The LAS AF files `bsst749-*` and `zenodo3382102-*` have `Attachment[@Name="HardwareSettingList"]` instead: `HardwareSetting/ScannerSetting/ScannerSettingRecord` rows (`Identifier`, `Variant`: `SystemType` = `TCS SP5`, `dblPinhole`, `dblPinholeAiry`, `csScanMode`, `dblZoom`) and `FilterSetting/FilterSettingRecord` rows (`Attribute`, `Variant`: `Objective` = `HCX PL APO lambda blue  63.0x1.40 OIL  UV`, `NumericalAperture` = `1.4`); `bsst749-4i`/`4ii` have an `ATLConfocalSettingDefinition` in that list and no `SystemType` row. The depositor describes the `bsst749-*` files as "small real confocal LIFs".

**Inferred:** an image whose hardware setting holds an `ATLConfocalSettingDefinition`, or a scanner setting with a `dblPinhole` row, was acquired by laser-scanning confocal microscopy (`acquisition_mode` `Laser Scanning Confocal`); one with an `ATLCameraSettingDefinition` by a widefield camera (`Widefield`). The `SystemType` scanner row is the system name (as `SystemTypeName` is in LAS X files); the `Objective` and `NumericalAperture` filter rows are the objective, its magnification the number before `x` in the name (`63.0x1.40` → 63). No corpus LIF records a user or experimenter name: LAS X writes `UserManagementUserName="UserManagementFeatureInactive"` (`aics-s-1-t-*`), which is not a name, and nothing else.

**Prior art consulted:** none beyond the notes above (liffile 2026.7.14 does not expose these settings; its output is unchanged).

## 2026-09-23 — `images[].extra.bits_significant` from `Resolution` (Richard Zimring with Claude as assistant)

**Scope:** a new normalized value from a field already parsed; pixel decoding is unchanged.

`ChannelDescription@Resolution` is already read (it picks the storage type: 12 → `uint16`). When every channel of an image is an integer channel (`DataType` 0) with the same `Resolution`, it is now also reported as `images[].extra.bits_significant` (the key ND2, OIB/OIF and ZVI already use), so `stats` and `watch --qc` count saturation at 2^bits − 1 (4095 for 12-bit data) instead of the storage type's 65535. A walk of `info --view structure --json` over the 18 LIF corpus inputs found 12-bit channels in `zenodo3382102-y293-Gal4-vmat-GFP-f01`, `aics-s-1-t-4-c-2-z-1` and `ome-imagesc-110520-AMR1` (65 channels), 8-, 16- and 32-bit elsewhere; no image mixes resolutions (FLIM result sets hold 8/16/32-bit data in separate images). The largest sample of the 12-bit images stays at or below 4095 (`stats`), consistent with the reading; `stats` falls back to the type's maximum should a sample ever exceed the recorded depth.

**Corpus files used:** the 18 LIF inputs. **Prior art consulted:** liffile 2026.7.14 (BSD-3-Clause) documents `Resolution` as bits per sample, as `docs/formats/lif.md` already notes.

## 2026-09-25 — Objective text trimmed, immersion and acquisition mode in the shared spelling

**Scope:** normalization only, in the core (`ImageInfo::finish`), no change to what the LIF reader parses. LAS X writes `ObjectiveName` with a trailing blank (`HC PL APO CS2    10x/0.40 DRY ` in `zenodo14976703-Convalaria-LambdaScan`) and `Immersion="DRY"`; the model now trims text fields and spells immersion `Air` (`dry` → `Air`, as the OME enumeration does), so the value survives an OME-TIFF round trip unchanged. `Laser Scanning Confocal` / `Widefield` are already labels of the shared acquisition-mode vocabulary (`book/src/guides/metadata.md` § General rules). λ-scan detection windows (`LambdaEmission`) and `ChannelAttachment/Band` bands are exported as OME emission filters and read back identically.

**Corpus files used:** `zenodo14976703-Convalaria-LambdaScan`, `bsst749-*`, `aics-s-1-t-*`, `ome-michael-PR2729-frameOrderCombinedScanTypes` (export + `check --against`). **Prior art consulted:** none.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the LAS writer, tile scans, lambda scans, pyramids, FLIM images (`extra.flim.decoded`), LIFEXT sidecars and truncation notes. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — differential cross-check with a second reader

**Corpus files:** every development LIF on disk (`corpus/oracle/second/*.json`); no held-out file.
**Prior art consulted:** readlif 0.6.6 (GPL-3.0), run as a black box by `oracle/second_opinion.py` (`get_image(i).get_frame(z, t, c)`; mosaic images skipped).
**Observations.** Every compared plane agrees except the FLIM-derived images of `zenodo13752242-FLIM250523`, which store 16-bit half floats: readlif returns their raw bits as uint16, liffile and OpenReadout decode half floats (adjudicated in `corpus/oracle/second/adjudications.toml`). readlif leaves out images it cannot read (the raw FLIM image), so images are matched by name.
**Inferred.** Nothing new; no reader change.

## 2026-09-25 — XLIF frames stored as image files; first real LAS X XLLF/XLIF exports

**Corpus files used** (all in `corpus/manifest.toml`, CC-BY-4.0; the Figshare deposits flatten folders, so the files were placed where their references point — the images next to a `LeicaMetadata/` folder holding the `.xllf` and `.xlif` files, as the references `.\9a.xlif` and `..\9a.tif` and the XLLF's recorded path `…\leicametadata\LMSIOManagerFolder.xllf` say):
- `figshare23522880-lasx-*` (Harikrishnan Vijayakumar Sreelatha et al.): LAS X 3.7.4 "export with metadata" of a DM4000B-M with a colour camera: one XLLF folder list referencing 7 XLIF files, each XLIF's memory block one `Frame` = an RGB TIFF (1920 × 1440 or 1920 × 1378) in the parent folder.
- `figshare30597152-lasx-*` (Magdalena Wolska et al.): one XLIF whose frame is a 4000 × 3000 RGB TIFF (`..\d6%2020x%201.1.tif`, URL-quoted, lower case where the file is `D6 20X 1.1.tif`).

**Prior art consulted:** `liffile` 2026.7.14 (BSD-3-Clause, https://github.com/cgohlke/liffile), source read as documentation: an XLIF `Memory` element's children with a `File` attribute are frames; their `Offset`/`Size` address the memory block; a frame whose file is `.tif`/`.jpg`/`.png`/`.bmp` is the *decoded* image of that file (not its bytes), and when the decoded image is RGB and three times the frame size its first sample is used as grey; references are URL-unquoted with `\` as separator and matched case-insensitively; the BGR→RGB reordering liffile applies to LIF memory blocks (channel tags 3, 2, 1) is not applied to image-file frames, whose samples it returns in the file's order.

**Inferred (hex/XML inspection of the files above):**
- The XLIF channel descriptions of these RGB images keep LAS X's memory order (`ChannelTag` 3 = blue at `BytesInc` 0, 2 = green at 1, 1 = red at 2, as in LIF files), while the TIFF frames are photometric-RGB TIFFs (sample 0 declared red by the TIFF itself). So a decoded RGB frame is placed into the memory block by channel tag — the TIFF's red sample where the channel tagged 1 (red) lives — and our channel named `Red` holds the TIFF's red samples. With liffile's convention (S returned as R, G, B for both LIF memory blocks and image-file frames), our channel with tag t equals liffile's sample t − 1; `oracle/gen.py` now hashes RGB images that way (previously they were left unhashed).
- XLLF (`LMSIOManagerFolder.xllf`, `Data/Experiment`, `Children/Reference`) behaves as the XLEF-like folder list we had inferred from the name: 7 references resolved, liffile 2026.7.14 reads the same 7 images.
- The TIFF frames' decoded size equals the XLIF `Size` exactly (8 294 400 = 1920 × 1440 × 3; 7 937 280; 36 000 000).

**Not validated (no public file): JPEG and PNG frames are decoded by the same rule (a note says so); BMP frames, multi-page OME-TIFF/Aivia frames and frames that are grey images stored as RGB are refused (exit 6) or flagged.

## 2026-09-26 — Start time from `TimeStamp` children of `TimeStampList`

**Why.** The per-field comparison with Bio-Formats (`oracle/metadata_compare.py`, docs/benchmark/microscopy-metadata.md) found `acquired_at` missing on 45 images of 8 LIF files that Bio-Formats dates (`bsst749-2a-ishi-hf-fshr-*`, `bsst749-4i-bodipy-ctl`, `bsst749-4ii-bodipy-ctl`, `zenodo3382102-y293-Gal4-vmat-GFP-f01`, …).

**Corpus files used:** the files above; every development LIF for the check. **Prior art consulted:** liffile (BSD-3-Clause, https://github.com/cgohlke/liffile), run as a second reader of `timestamps` (not its source).

**Observations.** In these files (LAS AF generation) `TimeStampList` has no hex text; it holds `TimeStamp` elements with `HighInteger` and `LowInteger` attributes, the two halves of a Windows FILETIME (`bsst749-2a-ishi-hf-fshr-dmso` Series003: High 30865730, Low 1956591768 → 2021-02-02T09:07:52.659, as liffile and, to the second, Bio-Formats report).

**Rule implemented:** `timestamps` = the hex FILETIMEs of `TimeStampList`'s text; when there are none, the `(HighInteger << 32) | LowInteger` of its `TimeStamp` children, in order.

## 2026-10-07 — channel names from the dye names LAS X records

**Why.** The imaging hunt (`docs/benchmark/hunt-2026-10-imaging.md`) noted that LIF channels were named after their display colour (`Red`, `Gray`) while the XML also records dye names.

**Corpus files used** (development inputs only; no held-out file): every development LIF on disk — `zenodo6606445-Project007`, `zenodo20760621-stellaris-metadata`, `ome-imagesc-110520-AMR1`, `bsst749-4i-bodipy-ctl`, `bsst749-4ii-bodipy-ctl`, `zenodo6643649-ki67-untreated`, `zenodo5576217-nmj-starved`, `zenodo6259698-ob-f8211`, `zenodo7509202-ileum-ctrl`, `zenodo19217336-dnge-notjammed`, `ome-imagesc-30856-20191025-Test-FRET-585-423-426`, `zenodo7840078-bodipy-mcf7`, `zenodo5895076-polymersome-unstain`, `zenodo3382102-y293-Gal4-vmat-GFP-f01`, `zenodo13752242-FLIM250523`, `zenodo14976703-Convalaria-LambdaScan`, `zenodo18462868-raman-spombe`, the `aics-*`, `bsst749-2a-*` and `ome-michael-*` files, and the two XLIF exports.

**Prior art consulted:**
- `liffile` 2026.7.14 (BSD-3-Clause, https://github.com/cgohlke/liffile), `LifImage._channel_names` read as documentation: it takes `ChannelProperty[Key='DyeName']` of each `ChannelDescription` first, then the non-empty `MultiBand/@DyeName` values of the main `ATLConfocalSettingDefinition` when their count equals the channel count, then one dye per sequential step; the `Leica/` prefix is dropped.
- Bio-Formats 8.5.0 (GPL), run as a black box: `showinf -nopix -omexml-only`, `Channel/@Name` per image.

**Observations (XML inspection of the files above):**
- LAS X 4 and STELLARIS files (`Project007`, `stellaris-metadata`, `AMR1`) give each `ChannelDescription` a `ChannelProperty` list with `DyeName` (`Leica/ALEXA 488`), `DetectorName` and `SequentialSettingIndex`. Their dyes agree with the display colours (DAPI blue, ALEXA 488 green, ALEXA 546 yellow, ALEXA 647 red; Cerulean cyan, EGFP green, mOrange yellow, mCherry red). liffile reports the same names; Bio-Formats reports empty names.
- Older LAS AF and LAS X 3 files keep dye names only on the detection bands of the hardware setting: `Spectro/MultiBand/@DyeName` (`Leica/FITC`, `None` or empty), keyed by `@Channel`, the detector number, next to `DetectorList/Detector/@IsActive`. A setting can name a dye on a band whose detector is off, so the dyes present in a setting do not tell which channels were recorded.
- Sequential scans list one `ATLConfocalSettingDefinition` per step under `LDM_Block_Sequential/LDM_Block_Sequential_List`. Taking each step's active detectors in order, and the dye of that detector's band in the same step, gives exactly one entry per image channel, with at least one dye, in 32 images of 6 files, and every dye fits its channel's colour and the step's laser (`ki67-untreated`: ATTO 590 with the 561 nm line on the red channel, ATTO 647N with 633 nm on magenta, DAPI with 405 nm on gray, EGFP with 488 nm on green). Bio-Formats agrees on `nmj-starved` and on two of four channels of `ileum-ctrl`; on `ki67-untreated`, `ob-f8211` and `bodipy-ctl` its names are shifted by one channel or swapped against the colours and lasers (for example `Leica/FITC` on the blue channel recorded with the 405 nm DAPI step). liffile's step rule needs one dye per step and found none of these.
- In `ileum-ctrl` the third step records the band 570–614 nm, excited at 561 nm, as `Leica/ALEXA 488`. That is what the file says, so we report it; Bio-Formats reports `Leica/TRITC` there.
- Without sequential steps, the main setting's active detectors give the names the same way (`dnge-notjammed`: FITC on its one channel, as liffile and Bio-Formats report; the `Bleach003` image of the FRET file: EYFP, as Bio-Formats reports).
- In the FRET file the steps of the other images carry no `Spectro` element of their own. liffile names them from the main setting's bands (ECFP, EYFP), which in `DonAcc_PreBleach` belong to a detector that was off. We give no dye there.

**Rule implemented:** a channel's dye is its `ChannelProperty` `DyeName` when any channel of the image has one. Otherwise, the sequential steps of the hardware setting (or the main setting when there are no steps) are walked in order, each step's active detectors in order, and each detector takes the `DyeName` of the band with its `Channel` in the same step. This is used only when every step has its own bands, every active detector except the transmission detector (`Channel` 100) has a band, the active detectors match the channels one for one, and no dye is named twice. `Leica/` is dropped; `None` and empty values mean no dye. The channel is then named after the dye and the dye is also reported as `fluorophore`; the display colour stays in `color`. A λ-scan channel is named `<dye or colour> <wavelength> nm`.
