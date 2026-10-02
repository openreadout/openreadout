# Provenance log — Zeiss AxioVision ZVI (`zvi`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — reader (Richard Zimring with Claude as assistant)

**Public specifications used:** Microsoft [MS-CFB] Compound File Binary File Format (the container, already implemented for Shimadzu from the same document; the reader moved to `openreadout-core::cfb` unchanged apart from carrying the caller's format id) and Microsoft [MS-OAUT] (the `VARENUM` type-code numbers, to know the data size of each typed value once the hex dumps showed that the leading u16 of every value is such a code: 3 before 4-byte integers, 5 before 8-byte doubles, 8 before length-prefixed UTF-16 text).

**Bio-Formats as a black box:** Bio-Formats 8.5.0 `showinf -omexml` and `bfconvert` (oracle/bftools) were run on every corpus file; their output values (size, channel names, exposure 0.05 s, 0.065 µm pixel size, objective 100×/1.4, excitation 498 nm) were searched for among the parsed tag values to decide which tag id carries what. Tag ids are numbers read from our own parse; all names are ours. The `Scale Unit` code 76 was matched to µm because Bio-Formats reports µm wherever a file stores 76 and the values agree. Bio-Formats' plane order for `bfconvert` output was established from its own `Dimension order` line (a first oracle run that assumed XYCZT mislabelled the z-stack planes; `oracle/gen.py` `_bf_planes` now follows the reported order).

**Tools:** olefile 0.47 (BSD-2-Clause) was used as a compound-file browser to list streams and dump bytes during exploration (the reader itself uses our MS-CFB code); a throw-away Python script parsed tag lists with the grammar read off the dumps.

**Corpus files used** (licence per deposit): Zenodo 11375101 (CC-BY-4.0, Neal, Shukla, Mast; three maximum-intensity projections and one 26-plane z stack from its zips), Zenodo 12592034 (CC-BY-4.0, Staneczek, Szaniawski, Marynowski; a 48-bit colour snapshot), Figshare 26880337, 24769857, 15042921, 12133278 (CC-BY-4.0) and 11341661 (CC0). Human material: HeLa (a cell line) only; no patient data.

**Observed (hex dumps):**
- Streams `Image/Contents`, `Image/Tags/Contents`, `Image/Item(n)/Contents`, `Image/Item(n)/Tags/Contents`, `Image/Layers/…`, `Image/Scaling/…`, root `Tags`, `Thumbnail`, property-set streams.
- Tag lists: i32 `0x20001000`, i32 count, then (value, i32 id, i32 attribute) triples; e.g. `03 00 e9 05 00 00 | 03 00 03 02 00 00 | 03 00 88 00 00 00` = value 1513, id 515, attribute 0x88.
- Item `Contents`: typed values, then `00 20 00 10` (u32 `0x10002000`) at offset 296 followed by width, height, 1, bytes per pixel (2 or 6), format (4 or 8), valid bits (16), then exactly width × height × bytes-per-pixel sample bytes to the end of the stream.
- Differential: across the 52 items of the z stack tag 2819 runs 0–25 and tag 2820 takes 2 then 4 while the channel name changes; in the channel-interleaved stack 2820 alternates 0/1 and 2819 runs 13–30. Tag 300 grows item by item (1.8336e-5 days = 1.584 s per plane); tag 1025 is an OLE date (2023-04-05 15:26:55).
- Colour: `0x00FF00` for eGFP and `0xFF0000` for BFP, which Bio-Formats reports as green and blue: the integer is `0x00BBGGRR`. 48-bit colour samples match Bio-Formats only when swapped from B, G, R to R, G, B.
- `Thumbnail`: 16-byte identifier, 10 further bytes, then `BM` and a Windows bitmap.

**Inferred without corpus evidence (flagged in `known_gaps`):** tag 2821 as the time index; tags 2822/2823/2827 as further position indices; 8-bit grey and 24-bit colour layouts.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the per-item pixel format and the position-tag, header and unit notes. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-25 — six more depositors; image-level exposure; where Bio-Formats differs

**Corpus files used** (all in `corpus/manifest.toml`, directory `zvi-depth/`, CC-BY-4.0): Figshare 32769939 (`a1.zvi`, one DIC channel, 2025), 31813711 (3 fluorescence channels, Axio Observer.Z1), 10732844 (3 channels, ApotomeCam on an Axio Imager.Z2), 6798824 (3 channels), 6840530 (48-bit colour snapshot, Axio Imager.M1, uncalibrated), 12132993 (2 channels, Axio Imager Z1, 2008). With the 10 earlier files: 16 files, 12 depositors, 8 microscope stands, 7 cameras, acquisitions 2003–2025.

**Prior art consulted:** olefile 0.47 (BSD-2-Clause, https://github.com/decalage2/olefile) run as a tool: `oracle/gen.py` `_zvi_olefile` reads the planes with olefile as the compound-file reader and the plane layout of `docs/formats/zvi.md`; on the 4 files cross-checked (3 grey planes, 1 colour plane) its hashes equal Bio-Formats'. Bio-Formats 8.5.0 run as a black box (`showinf`, `bfconvert`, `oracle/metadata_compare.py`).

**Observed / inferred:**
- Colour snapshots store the exposure (2564) only in `Image/Tags`, not in the item's tag list (20 ms and 26 ms, equal to Bio-Formats' ExposureTime): the channel's exposure now falls back to the image tags, as its name and colour already do.
- **Bio-Formats cannot open `a1.zvi`** (its POI-based compound-file parser throws a NullPointerException while building the directory tree). Our MS-CFB reader and olefile both open it; the olefile oracle and our plane agree. The file's storage tree also holds an acquisition-setup folder (`Image/RootFolder/I0/V0/Locations/…/Segments/…/{C,Z,T,Mosaic,Pos}`) and a `Tags/V1` sub-storage under the item: listed by `info --view structure`, not interpreted.
- **Bio-Formats' AcquisitionDate is the second plane's time**: in all 11 multi-plane files with dates its value equals tag 1025 of `Image/Item(1)`, and for the 3 single-plane files with a date it reports none (e.g. 14:34:15.556 where item 0 — and the image-level 1025 we report — is 14:33:16.230; the 3 channels were exposed one after another). We report the first item's time (the start of the acquisition; in one file the image-level tag 1025 is 8.7 s earlier still).
- **Bio-Formats reports an exposure of 0** for one channel in 3 files where the item's 2564 holds 100 ms, 5893.982 ms and 1000.572 ms; we report the stored values.
- Bio-Formats reports 1.0 µm for uncalibrated axes (scale unit 0); we report none (unchanged).
- Still no public file with 8-bit grey, 24-bit colour, a time series or several positions: searched Figshare (every article with a `.zvi` file: 20), Zenodo (1), Dryad (Agrobacterium FtsK deposits hold ~80 ZVI files, CC0, but Dryad refuses unauthenticated downloads), OME sample images (no ZVI folder), IDR.
