# Provenance log — Olympus FluoView OIF / OIB (`oif`, `oib`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-23 — reader (Richard Zimring with Claude as assistant)

**Prior art read (permissive):** `oiffile` 2026.2.8 by Christoph Gohlke, BSD-3-Clause, https://github.com/cgohlke/oiffile (`oiffile/oiffile.py`, README). Taken from it: that OIB is a compound file whose `OibInfo.txt` maps `StreamNNNNN` names to the files of an OIF folder (`MainFileName`, `StorageNNNNN` keys); that settings files are UTF-16 (or UTF-8) INI text with a binary `[ColorLUTData]` tail in LUT files; that plane TIFFs are grouped by the letters of their names and ordered by the numbers after each letter; that `AxisOrder` in `[Axis Parameter Common]` and `MaxSize` in `[Axis n Parameters Common]` describe the acquisition's axes; that `ValidBitCounts` in `[Reference Image Parameter]` is the significant bit depth. Names in our code are ours.

**Public specifications used:** Microsoft [MS-CFB] (compound files; the shared reader in `openreadout-core::cfb`, unchanged) and TIFF 6.0 (plane files; the existing `openreadout-tiff` decoder, which gained `from_bytes` to parse a TIFF held in memory).

**Bio-Formats as a black box:** Bio-Formats 8.5.0 `showinf` (with and without `-nometa`, `-omexml`) and `bfconvert` (oracle/bftools, `-Xmx2g`) were run on every corpus data set. Used to confirm: lambda bands appear as channels (25 channels for the XYCL file); reference images of line scans form a second series; the decimal-comma value `0,207.0` means 0.207 µm; Z step = the Z axis `Interval` (510 nm → 0.51 µm); channel names are `CH Name` of `[Channel n Parameters]`; per-plane `DeltaT` equals the T axis `AbsPositionValue` of each `.pty` in ms.

**Tools:** olefile 0.47 (BSD-2-Clause) as a compound-file browser to list streams and print settings text during exploration; tifffile (BSD-3-Clause) to print TIFF tag lists of plane streams.

**Corpus data sets used** (licence per deposit): Zenodo 7080902 "ImageJ OIB test files" (CC-BY-4.0, Nayana Gaur; also the OME sample `Olympus-FluoView/imagesc-71616`, same file, CC-BY-4.0 readme), Zenodo 4598136 (CC0-1.0, Nakamura et al.; two 2-channel LZW OIBs), Zenodo 6795923 (CC0-1.0, McCullagh et al.; a single plane and a 20-plane z stack), Zenodo 14925254 (CC-BY-4.0, Yael Kapon; a 25-band lambda scan), Zenodo 11075037 (CC-BY-4.0, Martínez-Castro, Guerrero, Covarrubias; a 100-frame RICS time series, one member of a zip fetched through Zenodo's container endpoint), Zenodo 4421962 (CC0-1.0, Hu, Botvinick; two OIF folders from `OIF.zip`: a two-channel frame and an XT line scan with reference images). No human material.

**Observed:**
- `OibInfo.txt` of `Spleenx20.oib`: `[OibSaveInfo]`, `MainFileName=Stream00010`, `Stream00010=Spleenx20.oif`, `Storage00001=Spleenx20.oif.files`, `Stream00001=Storage00001/s_C001.tif`, ...; streams `Storage00001/Stream00001..9` and root `Stream00010`, `OibInfo.txt`.
- Settings text starts `FF FE 5B 00` (BOM, `[`); lines end CR LF; `ImageCaputreDate='2020-02-10 13:07:52'` plus `ImageCaputreDate+MilliSec=469`.
- Plane TIFFs: little-endian, 16 bits, one sample, compression 1 or 5 (LZW, in the two Zenodo 4598136 files), tags 254, 256–259, 262, 272 (`Model`), 273, 277–281, 282/283, 284, 296, 305 (`Software`), 306; one page.
- Name patterns in the corpus: `s_C001.tif`, `s_C001Z001.tif`, `s_C001T001.tif`, `s_C001L001.tif`, `s_C001-R001.tif`; `AxisOrder` values `XYC`, `XYCZ`, `XYCT`, `XYCL`, `XTC`.
- Lambda: `[Axis 6 Parameters Common]` `AxisCode="L"`, `StartPosition=420.0`, `Interval=5`, `Resolution=10`, `MaxSize=25`, `Start Absolute Position=420`, `Stop Absolute Position=550`; file name "spectral 420-550": 420 + 24 × 5 + 10 = 550, so each band is `Resolution` wide.
- Line scan: `ScanMode="XT"`, `AxisOrder="XTC"`; `s_C001.tif` 228 × 10000; its `.pty` `[Image Parameters]` has `HeightUnit="ms"`, `HeightConvertValue=0.0`; `s_C001-R001.pty` has `ImageGroup="Reference"` and µm units.
- `.pty`: `[Axis 4 Parameters] AbsPositionValue` grows frame by frame (ms), `[Axis 3 Parameters] AbsPositionValue` is the z drive position in nm; `ObjectiveLens Name`, `Magnification`, `ObjectiveLens NAValue`, `Time Per Line` (µs).

**Inferred (flagged in the format notes):** the decimal-comma reading; that letters other than C, Z, T, L make separate images; the lambda band edges; that a missing `.pty` falls back to `[Reference Image Parameter]`.

**Validation (2026-09-23).** Corpus harness, 9 input data sets: all pass, 168 planes bit-identical to Bio-Formats' `bfconvert` output (`oracle/gen.py` `oif_`), and oiffile 2026.2.8 reading every plane TIFF agrees with Bio-Formats on every plane (`oiffile_agrees: true` in each oracle). Physical sizes agree with Bio-Formats' OME-XML except Y of the line scan, where Bio-Formats repeats the X size (the oracle drops it; documented in `gen.py`). The OIF published without its data folder (`zenodo7080902-…-oif-alone`, role `corrupt`) opens and `check` reports `container` and `planes` errors. Synthetic tests (`crates/openreadout-oif/tests/synthetic.rs`): OIB and OIF round trips, lambda folding, truncated OIB (exit 4 on open when the directory is cut, `check` errors and exit 4 on the plane read when a plane stream is cut), OIF with a truncated plane file, a missing plane file, a missing folder.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: compression, scan mode, reference images and the plane-file grouping notes (OIB and OIF). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.
