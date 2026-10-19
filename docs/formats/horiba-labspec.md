# HORIBA LabSpec 6 (`.l6s`, `.l6m`)

LabSpec 6 is the software of HORIBA's Raman and photoluminescence microscopes (LabRAM, XploRA, T64000). It saves a spectrum as `.l6s` and a map as `.l6m`. OpenReadout reads the spectrum or map of both (format `horiba-labspec`, crate `openreadout-spectro`). Everything here was inferred from public corpus files and checked against LabSpec's own text export of each file (`oracle/series_oracle.py`). See `docs/provenance/horiba-labspec.md`.

## Container

- Bytes 0–7 are `LabSpec6`. The u32 at byte 12 is 0 or 8 in the files that are read. In files with 1 there (Zenodo 21324687, figshare 27053842) the rest of the file is encoded: it looks random, so those files are refused (exit 6) and the hint asks for LabSpec's text export.
- The body is a sequence of 24-byte records: a u32 type, a u32 id, a 4-byte key, 4 more bytes (0, 1, or leftovers of a pointer), and an 8-byte value. Keys are four characters (`typm`, `uni|`, `tam`) or numbers. Types seen: 2 to 9 (single values; type 7 is up to 8 bytes of text, type 8 up to four UTF-16 characters) and 0x1006 to 0x1009 (values stored elsewhere, with a count in the last four bytes).
- A float32 array is a record of type 0x1006 and key `db d4 6c 7d`, whose count is the number of values. The values follow the run of records written right after the array record: the first place after it where no record of a known type starts. Every array of the corpus files ends where another record or the end of the file begins, and the reader requires that.
- An array is named by the type-7 records `typm` (axis: `Intens`, `Spectr`, `X`, `Y`) and `uni|` (unit: `Cnt`, `1/cm`, `nm`, `µm`) in the run after it, or, when that run has none, in the run before it.

## Data set

The first `Intens` array and the arrays after it up to the next `Intens` make the data set. `Spectr` is the spectral axis (one value per point). With `X` and `Y` arrays, the file is a map of `X` × `Y` spectra: the intensities are stored spectrum after spectrum with `X` changing fastest. The intensity count must equal points × X × Y, or the file is refused as corrupt.

- Trace 0 holds the spectra, one sweep per spectrum, on the listed spectral axis: `raman_shift` in 1/cm, `wavelength` in nm, or `x` with the stored unit. The channel is `intensity` in `counts` (`Cnt`). `data_type` is `RAMAN SPECTRUM` for a Raman-shift axis.
- A map is also image 0 (one channel per spectral point) and table `positions` (`spectrum`, `x`, `y` in µm).
- Further `Intens` arrays (a second data object, as in `figshare25126925-beads-02`) are listed in `info --view structure` and named in a note, but not read.

`format_version` is `LabSpec 6 (header word N)`.

## Not read

- The title, sample name, acquisition time, exposure, accumulations, laser, grating, objective, hole and stage settings. LabSpec's text export writes them as `#Key=` lines.
- LabSpec 5 files (`.l5s`, `.ngs`; the `.ngs` files of figshare 25137059 and 32030553 start `NGSNextGen`) and LabSpec's own XML export.

## Vocabulary (public API of `openreadout-spectro`, LabSpec)

| identifier | meaning |
| --- | --- |
| `LabSpecReader` | reader of LabSpec 6 `.l6s`/`.l6m` files |
| `LABSPEC_FORMAT_ID` | `horiba-labspec` |
