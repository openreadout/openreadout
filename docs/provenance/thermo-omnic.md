# Thermo Fisher OMNIC provenance (`.spa`, `.spg`, `.srs`)

## 2026-09-23 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used:** the OMNIC `.SPA` files and their OMNIC CSV exports in M. Toffolo's
"FTIR spectral library of the major components of archaeological sediments" (Zenodo record
14170891, CC-BY-4.0; Nicolet iS5, OMNIC); `sample1.spa` of Orange-Spectroscopy (GPL-3.0-or-later
data file, commit fc7cc69); and, held out of the fetched tiers because the repository states no
licence, the `.spa`, `.spg` and `.srs` test files of `spectrochempy/spectrochempy_data`
(commit 08bb9b0), used locally for format coverage (group files and series).

**Permissive prior art consulted:** SpectroChemPy 1.0.0 (LCS Caen, CeCILL-B — a BSD-style
licence with an attribution obligation and no copyleft; `LICENSE` read first),
`spectrochempy/core/readers/read_omnic.py`: the 16-byte key table at 304 with its count at 294
(key byte, uint32 offset at +2, uint32 length at +6), the meaning of keys 2 (spectrum header),
3 (intensities), 4 (comments), 27 (history), 102/103 (sample/background interferograms),
106 (acquisition parameters), 107 (spectrum title and timestamp in group files), 130 (experiment
information); the spectrum-header field offsets (+4 points, +8 x-unit code, +12 y-unit code,
+16/+20 axis ends, +36 scans, +52 background scans, +80 laser frequency, +96 Raman excitation,
+188 optical velocity); the timestamp epoch (seconds since 1899-12-31 UTC at byte 296); the
series (`.srs`) record layout. Our names are our own.

**To be verified against the corpus** (recorded in the format notes once done): the axis order
(+16 is the x value of the first stored point), the y-unit and x-unit codes, and every value
against the CSV exported by OMNIC from the same spectrum.

## 2026-09-23 — implementation and validation (Richard Zimring with Claude as assistant)

**Our findings:** the u16 at +10 of a key record numbers the spectrum of a group (0, 1, …) and a
second u16 at +12 numbers repeated records (several key-130 blocks); the resolution and bench
serial number are only in the history text (English `Resolution:` and French `Résolution:` with a
decimal comma); OMNIC's CSV export lists the stored points in ascending x and adds one zero row one
step below the range; y-unit code 22 (volts) and x-unit code 2 (points) mark interferograms.

**Validation:** SpectroChemPy on 14 files bit for bit (5 of them held: no licence); 144 of M.
Toffolo's spectra against the CSVs OMNIC exported (all points within the 7 printed digits).

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: spa/spg, data type and final format. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — `.srs` series (Richard Zimring with Claude as assistant)

**Corpus files used:** `omnic-srs-scp-gc-demo`, `omnic-srs-scp-tga-demo`, `omnic-srs-scp-rapid-scan`,
`omnic-srs-scp-rapid-scan-reprocessed`, `omnic-srs-scp-high-speed`, `omnic-srs-scp-tgair-unreadable`
(SpectroChemPy test data, commit 08bb9b0; held: the repository grants no licence, so the files
are on the `hold` tier like the other SpectroChemPy OMNIC files).

**Prior art consulted:** SpectroChemPy 1.0.0 (CeCILL-B), `spectrochempy/core/readers/read_omnic.py`
`_read_srs`, `_read_header`, `_read_srs_spectra`, `_read_srs_acquisition_date` (read as
documentation): it locates the series header, the background and the spectra by searching for
16-byte signatures, reads the series name at header +938, the first time, last time and time
step (minutes, float32) at +1002, +1006, +1010, the spectrum count at +1026, 84-byte name
records followed by float32 values with 16 bytes between spectra, the timestamp at 296 only when
a copy of it sits at a known offset of the header, and states that series spectra are stored in
ascending x.

**What was inferred from the files (hex dumps; our reader does not search for signatures):**
- `.srs` keeps the `.spa` header (title at 30, record count at 294, timestamp at 296) but its key
  records at 304 are 22 bytes: u16 key, u64 offset, u32 length, u32 set (1 = the series, 0 = the
  background, 2 and 3 = processing), u32 (0, or packed codes in key 130). All six files follow it.
- The series spectra are one record, key 301: a 140-byte spectrum header (the key-2 layout: +4
  points, +8 x code, +12 y code, +16/+20 axis ends), a 56-byte acquisition block, then per
  spectrum 16 bytes (u32, then the spectrum's time in 1/100 s as u32: 496 → 0.083 min, 232 →
  0.039 min, matching the names), an 84-byte record starting with the spectrum's name
  (`Linked spectrum at 0.083 min.`) and the float32 values. The record length is exactly
  196 + count × (100 + 4 × points) in all six files, and the count equals the u32 at +90 of the
  series-information record (key 325), whose +2 holds the series title and +66/+70/+74 the first
  time, last time and step in minutes (SpectroChemPy's +1002/+1006/+1010 from the key-2 header,
  which key 325 follows at +936).
- Axis order: in the TGA-IR file, read with x ascending from the smaller end (500 → 4000 1/cm)
  the strongest bands are CO₂ at 2300–2400 (0.67 AU) and 650–690 (0.41) and water at 1500–1700
  (0.66), a TGA off-gas spectrum; read the other way they would be at 2100–2200 and 2900–3100
  with almost no CO₂. Series spectra (and the interferograms, whose ends are 0 and 4159) are
  stored from the smaller x to the larger, unlike `.spa` files (stored from +16).
- The background is the set-0 records: key 2 header, key 3 values (ascending likewise).
- Key 111 (u32 count = the spectrum count, u32 m, then m × count float32) and keys 110, 112, 130,
  146, 300 are not decoded.

## 2026-10-12 — map files (`.map`) (Richard Zimring with Claude as assistant)

**Files:** figshare 32180061 (CC BY 4.0, nine OMNIC Atlµs FT-IR point maps, files 64265535,
64265538, 64265541, 64265544, 64265547, 64265550, 64265553, 64265556, 64265562) and Zenodo
10598031 (CC BY 4.0, three Raman maps `HeLa_18,75.map`, `HeLa_37,5.map`, `HeLa_50microg.map`).
No independent reader opens `.map` files (SpectroChemPy refuses them), so the layout rests on
hex dumps and on the files agreeing with themselves.

**Prior art consulted:** none beyond the entries above.

**What was inferred from the files:**
- Two layouts. The FT-IR maps start with `Spectral Data File` and use the 16-byte key records of
  `.spa`; the Raman maps start with `Spectral Exte File` and use the 22-byte records of `.srs`.
- Both hold the stage positions in a key-6 record: u16 count, u16 kind, two bytes, then float32
  values. Kind 2 is a list of `count` (x, y) pairs in µm (the axis titles in key 146 read
  `Position (micrometers)`); kind 4 holds one rectangle (x0, y0, x1, y1) that contains every
  position in all twelve files; kind 1 holds one value per position (zeros, 0xFFFFFFFF or one
  repeated number), not decoded. The kind-2 count equals the number of spectra in all twelve
  files, and the spectrum names count up to it (`Spectrum 3 of 8`).
- FT-IR maps: the spectrum-1 header (key 2) comes first and describes the map spectra; then
  spectrum 0 (key 2, key 3) is a single-beam background; then for k = 1 … n a 96-byte key-148
  record (name at +16, as in a series record) and a key-3 record with the values of spectrum k.
- Raman maps: the spectra are the series record (key 301, the `.srs` layout, values in
  ascending x: the 50 µg/mL GO map has its strongest band at 1595 1/cm read that way, the G band
  of graphene oxide, and none at 2095 read the other way). Key 325 holds the grid: y first, last
  and step at +66, +70, +74 (where a series keeps its times) and x first, last and step at +78,
  +82, +86. These agree with the positions (to 0.1 µm), so the series times are not reported
  for maps.
- Key 147 is a Windows BMP (24-bit, uncompressed) after a u32 byte count: the video image of the
  sample. The kind-4 rectangle's aspect ratio matches the image's to within 0.5 %, which
  suggests it is the image's field of view; that is not confirmed, so no pixel size is reported.

## 2026-10-15 — map point count bounded by the data (Richard Zimring with Claude as assistant)

**Files:** a fuzz input (`tests/fixtures/malformed/file-fuzz-omnic-map-points-alloc.spa`) whose
map spectrum header gives 10,878,979 points while each intensity record holds 3 values.

**Prior art consulted:** none.

**What was inferred:** nothing new about the format. A map spectrum's key-3 record holds its
point count of float32 values, so the reader now refuses a map whose point count no intensity
record holds, before it lists one image channel per point. The corpus maps
(figshare32180061 mp2 and mp6, zenodo10598031) open as before.

## 2026-10-17 — map records that run past the end of the file (Richard Zimring with Claude as assistant)

**Corpus files:** none; the nightly fuzz run's input (`crates/openreadout-spectro/tests/fixtures/malformed/file-fuzz-omnic-map-record-past-end.spa`, 1,455 bytes).

**What was inferred:** nothing new about the format. The 2026-10-15 guard compared a map's point count with the declared length of its longest intensity record, and a damaged key can declare far more bytes than the file has. It now counts a record only up to the end of the file, so the 10.9-million-point header of this input is refused instead of listed as 10.9 million image channels.
