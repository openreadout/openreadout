# Provenance log — Molecular Devices ImageXpress / MetaXpress plates (`imagexpress`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-23 — reader (Richard Zimring with Claude as assistant)

**Prior art read (permissive):** the plane files' MetaMorph conventions (STK `UIC1`–`UIC4` tags and MetaSeries `<MetaData>` XML in `ImageDescription`) are decoded by our TIFF crate, whose provenance is `docs/provenance/tiff.md` (tifffile, BSD-3-Clause, read as documentation there). Nothing else.

**Bio-Formats as a black box:** Bio-Formats 8.5.0 `showinf -nopix -omexml` and `bfconvert` (oracle/bftools) were run on the corpus HTD. Observed: one series per (well, site) — 384 for idr0081 (384 wells x 1 site), including wells with no files ("Well A03 expected 2 files; found 0"); channels = wavelengths.

**Corpus files used** (`corpus/manifest.toml`, ids `hcs-imagexpress-*`):
- `hcs-imagexpress-idr0081-*` — CC-BY-4.0 (OME sample images `MetaXpress/idr0081`; IDR study idr0081-georgi-adenovirus, study file licence CC BY 4.0). `BSF018292-1A.HTD`, 384 wells, no sites, 2 wavelengths; single-plane STK-style TIFFs (`UIC1`–`UIC4`, MetaMorph 6.2.3). Partial copy: A01 (w1, w2), A02 (w1).
- `hcs-imagexpress-jump-a1170383-*` — CC0-1.0 (Cell Painting Gallery, `cpg0016-jump/source_8`). A plate folder without its HTD: `HTS_A01_s<site>_w<wave><GUID>.tif` plus `…_w<wave>_thumb<GUID>.tif` thumbnails; MetaSeries XML (MetaMorph 6.6.3). Partial copy: well A01, sites 1–2, 5 wavelengths.
- Read for comparison only (not in the corpus, licence CC BY-NC-SA 3.0): the HTD files of OME samples `MetaXpress/idr0005`, `idr0006`, `idr0008` — they show the `Sites`/`XSites`/`YSites`/`SiteSelection<r>` keys, `WaveCollect<w>`, `UniquePlateIdentifier`, a `TimePoint_1/` sub-folder, and names with bracketed GUIDs (`…_A01_s1_[GUID].tif`). Nothing from their pixels is used.

**Observed (the HTD text):** a line-oriented list of comma-separated values with quoted keys: `"HTSInfoFile", Version 1.0` first and `"EndFile"` last; `"Description"`, `"PlateType"`, `"TimePoints"`, `"ZSeries"`, `"ZSteps"`, `"ZProjection"`, `"XWells"`, `"YWells"`, `"WellsSelection<row>"` (one `TRUE`/`FALSE` per column), `"Sites"`, `"XSites"`, `"YSites"`, `"SiteSelection<row>"`, `"Waves"`, `"NWavelengths"`, `"WaveName<w>"`, `"WaveCollect<w>"`, `"UniquePlateIdentifier"`. CRLF line ends; the idr0008 `Description` contains raw non-UTF-8 bytes.

**Observed (file names):** `<plate>_<well>[_s<site>][_w<wave>][_thumb][<GUID> | _[<GUID>]].tif|TIF`; no `_s` when `"Sites", FALSE`; the GUID follows the wave number without a separator; thumbnails carry `_thumb`. idr0081: 768 files = 384 wells x 2 wavelengths.

**Inferred (flagged in the format notes):** that sites are numbered 1.. in reading order over the selected `SiteSelection` cells (idr0008 selects 4 of 25 cells and has `_s1`…`_s4`); that `TimePoint_<t>` and `ZStep_<z>` sub-folders hold time points and Z planes (the `TimePoint_1` folder is observed, `ZStep_` is not in the corpus); plate geometry of a folder without HTD from the well names (at most 8 x 12 → 96 wells, 16 x 24 → 384, else 32 x 48).

## 2026-09-24 — detection of folders without HTD (Richard Zimring with Claude as assistant)

**Problem:** a folder was claimed as an ImageXpress plate when two of its files had the name shape `<prefix>_<well>...tif`. Olympus FluoView `.oif.files` folders (`olympus-fv/zenodo4421962-oif/Bead12/50x.oif.files`, `Bead4/linescan-4hz.oif.files`: `s_C001.tif`, `s_C002.tif`) matched as prefix `s`, well `C1`/`C2`, so `info --view format`, batch walks and `index` treated them as plates instead of walking into them.

**Corpus files used:** the two ImageXpress plates above (`hcs-imagexpress-idr0081-*`: STK `UIC1` tag in every plane; `hcs-imagexpress-jump-a1170383-*`: MetaSeries `<MetaData>` XML in every plane, as dumped by tifffile's tag listing), the two `.oif.files` folders, and every other non-held-out corpus folder (a `info --view format -r` sweep compared with the manifest's `format`, now the corpus test `detect_matches_manifest`).

**Prior art:** none beyond the entry above.

**Inferred:** MetaXpress writes wells as row letters and a two-digit column (`A01`: every plane name of the two corpus plates, 3 + 20 files, and the name forms quoted in the entry above), and writes MetaMorph metadata into every plane. A folder without HTD now needs both, at least two plane files sharing one prefix, and no sign that another reader owns it (folder-name suffix or marker file; list in `docs/formats/imagexpress.md` § Detection).

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the index generation, the writer and missing plane files. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.
