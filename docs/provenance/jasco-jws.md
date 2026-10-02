# JASCO `.jws` provenance

## 2026-09-26 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml` with their licences):

- JASCOFiles.jl (Garrek Stemo, MIT, commit a4083de), `test/data`: modern flat files
  `jws-jascofiles-ftir-modern` (FT/IR-4600, absorbance), `jws-jascofiles-uvvis-abs`, `-uvvis-trans`,
  `-uvvis-refl`, `-uvvis-sbref`, `-uvvis-sbsample` (V-730, five photometric modes) with the V-730
  text export of the absorbance file; compound-file (Spectra Manager 1.x/2.x) files
  `jws-jascofiles-legacy-abs-512`, `-legacy-bg-512`, `-legacy-trans-512`, `-legacy-trans-4096`
  (FT/IR-4600, 512- and 4096-byte sectors) and `-legacy-raman` (NRS-5100) with the Spectra Manager
  CSV exports of `legacy-trans-512` and `legacy-raman`.
- jws2txt (J. Tran, MIT, commit fb76ef6), `temp/`: `jws-jws2txt-001hg`, `-smth` (J-1500 circular
  dichroism, three channels), `-bgr` (J-1500, two channels), `-14` (V-630 time course).
- jasco_jws_reader (odoluca, GPL-3.0, commit 7a44459), `Sample JWS files/`: `jws-jwsreader-cd-1`,
  `-cd-2` (J-1500 with CD-1500 module: CD, HT, absorbance) and `-fluor-1` … `-fluor-3` (FP-8300).
- Zenodo 13347737 (J. Szewczyk, CC-BY-4.0): `jws-zenodo13347737-da`, `-da-ba`, `-da-cu`
  (FT/IR-4700 with ATR PRO ONE) with the Spectra Manager text exports of the same names
  (Polish locale: decimal commas; footer with model, serial, accessory, measurement date,
  accumulation, resolution, gain, aperture, scan speed, filter, light source, detector).
- Zenodo 10203577 and 10377355 (A. Skwira, CC-BY-4.0): `jws-zenodo10203577-20220221-1`,
  `jws-zenodo10377355-20220912` (FT/IR-4700, %T).
- One further depositor's files are held out (`docs/benchmark/heldout.md`) and were not opened.

**Prior art consulted (read as documentation):**

- JASCOFiles.jl (MIT), `docs/src/guide/file-formats.md` and
  `docs/superpowers/specs/2026-06-04-jws-binary-reader-design.md`,
  `2026-06-11-legacy-jws-ole-reader-design.md`
  (<https://github.com/garrekstemo/JASCOFiles.jl/tree/a4083de88aae576099f67ef8d0e864a0501b59bc/docs>):
  the modern flat layout (`L~S ` magic; point count, first/last/step x as float64 at 0x84-0x98;
  x-unit code at 0xA0; y-mode code at 0xA4; data length at 0xC8; model, serial, title, comment
  strings at 0x140-0x1C0; Unix time at 0x2C0; float32 data at the end of the file); the compound
  file's streams (`Header`, `DataInfo`, `Y-Data`, `X-Data`, `SampleInfo`, `ModuleInfo`,
  `MeasParam`, `UserInfo`, `BaseInfo`), the `DataInfo` field table, the y-mode and x-unit codes,
  UTF-16 strings with a byte length, TLV records, OLE dates in UTC, and the FT-IR `MeasParam` tags.
- jws2txt (MIT), `jws2txt/helpers/helpers.py`
  (<https://github.com/jzftran/jws2txt/blob/fb76ef65a0aa6547c382baaa16e50d8804228007/jws2txt/helpers/helpers.py>):
  channel count at `DataInfo` word 3, channel-major `Y-Data`, and the channel codes of CD
  instruments (0x10000103 wavelength, 0x1001 CD, 0x2001 HT voltage, 3 absorbance, 14
  fluorescence, 0x20000103 time).

### Inferred from the files (scratch comparisons with olefile/numpy, not committed)

- Two containers share the `.jws` extension; the first bytes decide: `D0 CF 11 E0` (compound
  file; its `Header` stream is UTF-16 `L~` … `SPCMAN2` `R2.00.00`) or `L~S ` (flat,
  `SPECMAN`/`SPECIRM` `R2.0.0`).
- `DataInfo` is 96 bytes for one channel, 140 for two, 184 for three (44 more per channel).
  Word 3 is the channel count (1, 2, 3), word 5 the point count, then first x, last x, step (all
  float64; the step is negative for descending scans) and four descriptor words: the x descriptor
  then one per channel, the remaining slots repeating from the start (one channel: `x y x y`;
  two: `x y1 y2 x`; three: `x y1 y2 y3`). `Y-Data` is exactly 4 × channels × points bytes in all
  21 compound files, channel-major. Word 4 is 1 on a regular grid and 0 in the Raman file, which has
  `X-Data` (float32 per point) and step 0.
- x descriptors: 0x10000100 wavenumber (cm⁻¹; FT-IR), 0x10000101 Raman shift (cm⁻¹; NRS-5100),
  0x10000103 wavelength (nm; J-1500, FP-8300), 0x20000203 in the V-630 time course (x from 0 to
  6529 in steps of 1; the time unit is not established).
- channel codes: 0 %T, 3 absorbance, 8 single-beam intensity (FT-IR background), 0x0E intensity
  (FP-8300 fluorescence; NRS-5100 CCD counts), 0x1001 CD (J-1500), 0x2001 HT voltage (J-1500,
  values 206-672: volts).
- `ModuleInfo`: u32 version (1-3), a UTF-16 string (module name: `CD-1500`, `FT/IR-4600`, often
  empty), u16, u16 module id, the model string (`J-1500`, `FT/IR-4700typeA`, `FP-8300`,
  `NRS-5100`, `V-630`), the serial string, then more (a second module on the J-1500: `PM-539`).
- `SampleInfo`: u32 1, sample name, comment (UTF-16 with byte length), a record count, then TLV
  records `u32 tag, u16 type`; tag 1 (type 5 or 7, float64) is the measurement date as an OLE date
  in UTC: `-da-ba` 10:48:46 and `-da-cu` 11:24:56 UTC where their exports say "Measurement Date
  05.01.2023 11:48" and "12:24" (Poland, UTC+1).
- `jws-zenodo13347737-da` and its export are not the same acquisition: the export's first value
  (90.1726 %T) and measurement time (11:26 local) differ from the file's (93.4457 %T, 10:14 UTC).
- Modern flat files: `0xA4` y-mode 0 %T, 2 %R, 3 absorbance, 9 and 10 single-beam reference and
  sample; x-unit 0 cm⁻¹ and 3 nm; data at file length − data length (0x740).
