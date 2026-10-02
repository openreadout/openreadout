# Thermo Fisher OMNIC `.spa` / `.spg`

Thermo Fisher OMNIC (Nicolet FT-IR spectrometers) saves one spectrum per `.spa` file, a group of spectra per `.spg` file and series as `.srs` files. OpenReadout returns the spectra as traces, with the sample and background interferograms when OMNIC kept them. OMNIC Atlµs map files (`.map`) are not read.

Derived from SpectroChemPy's OMNIC reader (CeCILL-B, a BSD-style licence; read as prior art and run as a reference reader), hex dumps of public files and the CSV files OMNIC exported from them. Provenance: `docs/provenance/thermo-omnic.md`. Format id `thermo-omnic`, family `spectroscopy`, crate `openreadout-spectro` (`OmnicReader`, `OMNIC_FORMAT_ID`).

`.spa` holds one spectrum (with the sample and background interferograms when OMNIC kept them); `.spg` a group of spectra. **Detection:** the file starts with `Spectral Data File` (`.spa`, `.spg`) or `Spectral Exte File` (`.srs` series: below).

## Layout

| offset | type | meaning |
| --- | --- | --- |
| 0 | 18 bytes | `Spectral Data File` |
| 30 | 256 bytes | title (the spectrum's title; the group's file name in `.spg`) |
| 294 | u16 | number of key records |
| 296 | u32 | timestamp: seconds since 1899-12-31T00:00:00Z (UTC) |
| 304 | 16 × n | key records: key byte, u32 offset at +2, u32 length at +6, u16 spectrum number at +10 (`.spg`) |

Keys used: 2 spectrum header, 3 intensities (float32), 4 comment, 27 history text, 102/103 sample/background interferogram (float32), 106 acquisition parameters, 107 spectrum title (256 bytes) and timestamp (u32 at +256) in groups, 130 experiment information (first byte 0x79: fixed text slots — experiment path, title, description, accessory). A spectrum of a group starts at its key 2; its other records carry the same spectrum number.

**Spectrum header (key 2)**, offsets from its start: +4 points (u32), +8 x-unit code, +12 y-unit code, +16 x of the first stored point (f32), +20 x of the last point (f32), +28 scan points, +32 interferogram peak position, +36 sample scans, +44 FFT points, +52 background scans (u32), +56 background gain, +68 collection time in 1/100 s (u32), +80 laser frequency (cm⁻¹, f32), +84 sample spacing, +92 aperture, +96 Raman excitation (cm⁻¹), +188 optical velocity.

- **x-unit codes:** 1 `wavenumber` (`1/cm`), 2 `points` (interferograms), 3 `wavelength` (`nm`), 4 `wavelength` (`µm`), 32 `raman_shift` (`1/cm`); others `x`.
- **y-unit codes** (channel name, unit): 11 `reflectance` (%), 12 `log_inverse_reflectance`, 15 `single_beam`, 16 `transmittance` (%), 17 `absorbance` (AU), 20 `kubelka_munk`, 21 `reflectance`, 22 `detector_signal` (V), 26 `photoacoustic`, 31 `raman_intensity`; others `intensity`. Codes 17 and 22 are confirmed by the corpus (history "Final format: Absorbance / Volts"); the others follow SpectroChemPy.

The x axis runs evenly from the header's first to last value (`x_at(i)` = first + i × (last − first)/(n − 1)); OMNIC's CSV exports confirm it point by point (they list the same points in ascending x and add one row with value 0 one step below the range).

## Traces

- **`.spa`:** trace 0 is the spectrum (name = the title), then `sample interferogram` and `background interferogram` when stored (x in `points`, y `detector_signal` in V).
- **`.spg`:** spectra sharing points, axis ends and units form one trace with one sweep per spectrum, in file order; a group with different axes gives one trace per axis. Table `spectra` (one row per sweep: `spectrum` number, `acquired_unix_s`, `elapsed_s`) and `extra.spectrum_titles` list them.

### `traces[].extra` (our vocabulary)

| key | from |
| --- | --- |
| `axis`, `data_type`, `y_quantity` | header codes (`data_type`: `INFRARED SPECTRUM`, `INFRARED INTERFEROGRAM`, `RAMAN SPECTRUM`, `UV/VIS SPECTRUM`) |
| `scans`, `background_scans` | header +36, +52 |
| `laser_wavenumber_cm1` | header +80 (the reference laser) |
| `laser_wavelength_nm`, `raman_excitation_cm1` | header +96, Raman spectra only |
| `optical_velocity`, `aperture`, `background_gain`, `sample_spacing` | header |
| `collection_time_s` | header +68 / 100 |
| `scan_points`, `peak_position`, `fft_points` | header |
| `resolution_cm1` | history text `Resolution:` / `Résolution:` (decimal comma accepted) |
| `final_format`, `instrument_serial` | history text `Final format:`, `Bench Serial Number:` |
| `history` | key 27 |
| `title`, `acquired_at` | offset 30, offset 296 (`.spa`); key 107 (`.spg`, first spectrum) |
| `spectrum_titles` | key 107 of each spectrum (`.spg`) |
| `x_units_code`, `y_units_code` | header +8, +12 |
| `spectrum_role` | `sample` / `background` on interferogram traces |

**Experiment.** `sample.id` = the title (`.spa`), `instrument` (Thermo Fisher Scientific, OMNIC, serial from the history), `method.name` = the experiment title (key 130) or the group title, `method.parameters`: `resolution`, `scans`, `background_scans`, `laser_wavenumber`, `laser_wavelength` (Raman), `accessory` (key 130), `final_format`; `acquisition.started_at` (UTC), `acquisition.comment` (key 4).

**Listing and vendor tree.** `info --view structure` lists every key record; `info --view full` holds the title, timestamp, key table, comments, the experiment-information records and the acquisition-parameter blocks (key 106: digitizer bits, high/low-pass filters, sample gain, optical velocity).

## Validation

- **SpectroChemPy 1.0.0** `read_omnic` on 14 files (8 Toffolo `.SPA`, Orange's `sample1.spa`, and five held SpectroChemPy test files: three `.spg` groups of 2, 19 and 55 spectra, an interferogram and a spectrum): every value bit for bit; scans, background scans, laser frequency; x axis within SpectroChemPy's 3-decimal rounding.
- **OMNIC's CSV exports** of 144 Toffolo spectra agree point by point to their 7 significant digits (8 of them in the automated corpus test).

## Series (`.srs`)

Rapid-scan, high-speed real-time, GC-IR and TGA-IR series. The file keeps the `.spa` header (title
at 30, record count at 294, timestamp at 296) but its key records at 304 are **22 bytes**: u16
key, u64 offset, u32 length, u32 set (1 the series, 0 the backgrounds, 2 and 3 processing), u32
index (which background, or packed codes in key 130). The reader follows the key table; it does
not search for byte signatures, so key tables in another order read the same.

- **Key 301, the series:** a 140-byte spectrum header (the key-2 layout), a 56-byte acquisition
  block, then per spectrum 16 bytes (u32, then the spectrum's time in 1/100 s), an 84-byte record
  starting with its name (`Linked spectrum at 0.083 min.`) and `points` float32 values. The record
  must be exactly 196 + n × (100 + 4 × points) bytes (else exit 4); n is checked against the
  series information.
- **Key 325, series information:** title text at +2, first time, last time and time step in
  minutes (f32) at +66, +70, +74, the spectrum count (u32) at +90.
- **Backgrounds:** each set-0 key-2 header with its key-3 values (index from the record).
- **Axis order:** series and background values are stored from the smaller x to the larger (a
  TGA-IR series read that way shows CO₂ at 2349 and 667 1/cm and water at 1500–1700, read the
  other way it would not), unlike `.spa`; `extra.axis` runs ascending. Interferogram series
  (x code 2) run over points 0 … n − 1.

Traces: 0 = the series (named by the series title; one sweep per spectrum; `extra`:
`series_title`, `first_time_min`, `last_time_min`, `time_step_min`, `spectrum_names` (first and
last), `scans`, `background_scans`, `laser_wavenumber_cm1`, `x_units_code`, `y_units_code`),
then `background 0`, `background 1`, … (`extra.spectrum_role` = `background`, `title`). Table
`series`: `spectrum`, `time_min` (from each spectrum's own time). Experiment: `method.name` = the
series title, `acquisition.comment` = the series history (key 27), `acquisition.started_at` =
the timestamp at 296 only when the series header repeats it (at +836, +368 or +828; reprocessed
and GC files hold none). Keys 110, 111 (profiles over time: u32 count, u32 m, m × count float32),
112, 130, 146 and 300 are listed, not decoded (a note; assurance `undecoded` without scope).

Validation (series): SpectroChemPy `read_srs` on six held test files (GC-IR, TGA-IR, rapid scan
as interferograms and reprocessed to absorbance, high-speed with two backgrounds, a TGA-IR file
whose records are in another order): every spectrum of every series and the first background,
bit for bit, and the axis ends.

## Known gaps

`.srs` files with a licence confirmed for redistribution were not found: the series layout rests
on SpectroChemPy's held test files. The profile records of a series (Gram–Schmidt, chemigrams)
are not decoded. OMNIC Atlµs map files (`.map`) are not recognised. Resolution and the serial number are only known when the history text records them.

## Vocabulary (public API of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `OmnicReader` | the reader (`FormatReader`): detection by the first bytes, `open`/`open_input` |
| `OMNIC_FORMAT_ID` | the format id, `thermo-omnic` |
| `SpectroDataset` | an opened file (the `Dataset` the four spectroscopy readers share): traces, tables, map images and attachments read lazily |

Everything else — trace names, channel names, `extra` keys and their values — is listed in the tables above in our own words.
