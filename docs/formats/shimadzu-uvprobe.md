# Shimadzu UVProbe `.spc`

Shimadzu's UVProbe software (UV-1800, UV-2600, UV-3600 and other UV-Vis and UV-Vis-NIR spectrophotometers) saves a spectrum as a `.spc` file. It is not a Galactic SPC file, which uses the same extension: it is a compound file (OLE2) with the spectrum as two float64 arrays. OpenReadout returns each data set as a trace, with the software, instrument model, serial number and creation date.

Derived from public files of two depositors and checked against the UVProbe text exports the depositors saved beside them. See `docs/provenance/shimadzu-uvprobe.md`.

Crate: `openreadout-spectro`, format id `shimadzu-uvprobe` (`UVPROBE_FORMAT_ID`), extension `spc`, reader `UvProbeReader`; the dataset is the shared `SpectroDataset`.

## Detection

A compound file (`D0 CF 11 E0 A1 B1 1A E1`) holding `DataStorage1/DataSetGroup/DataSet1/DataSpectrumStorage/Data/X Data.1` → definite. A compound file with the `.spc` extension and no such stream → likely (opening it then fails as unsupported). Galactic SPC files have no compound-file header and stay with `galactic-spc`.

## Layout

Streams under `DataStorage1/DataSetGroup/`, one storage `DataSet<n>` per data set (`DataSetGroupHeaderInfo`: u32 count; 1 in every development file):

| stream | content | our reading |
| --- | --- | --- |
| `DataSpectrumStorage/Data/Data Header.1` | u32 point count, u32 value code | `points`; `value_code`: 2 absorbance, 4 reflectance (%); other codes unnamed (`unknown_value_code` finding) |
| `DataSpectrumStorage/Data/X Data.1`, `Y Data.1` | float64 × points | wavelength (nm, from high to low) and values |
| `DataSpectrumStorage/DataHeader/Header Info` | three float64: first x, last x, step | the axis is regular when the x array follows them |
| `DataSetHeaderInfo` | one-byte-length texts and numbers: data set name (`RawData`), u32, float64 OLE Automation date (local time), 6 bytes, organization, 8 bytes, software, version, model, series, u32, a second model text, serial number | trace name, `instrument.software`/`software_version`/`model`/`serial`, `acquisition.started_at` (the data set's creation, local time) |
| `MethodStorage/PageTexts<n>` | u32, u32, a title, u32 count, then label and value texts | vendor tree (`method_pages`), not named |
| `DataHistoryStorage/DataSetHistory` | the creation record | listed |
| `Version/File Format Version` | u32 (0x120) | `format_version` (`file format 0x120`) |

Texts are UTF-8 when valid, else Latin-1; files written on Chinese-language PCs store GBK, which then reads as Latin-1.

## What the reader returns

- **One trace per data set**: x `wavelength` in nm, one channel named by the value code (`absorbance`; `reflectance` in %; `y` when unknown); `extra`: `value_code`, `data_set`, `data_set_name`, `data_type` `UV/VIS SPECTRUM`.
- **Experiment**: instrument vendor Shimadzu, model, serial, software and version; acquisition start (the data set's creation time, local, no zone).
- `check`: array lengths against the point count, non-finite values; a data set with `.2` arrays (several spectra) reads the first and reports `unsupported_data_set`.

## Validation

Six development files with UVProbe's own text export (wavelength and value, three decimals): every wavelength equal, every value within half the last printed digit. Two reflectance files from a second depositor (UVProbe 2.71, Chinese-language PC) have no export; their method text names the measuring mode. The oracle (`oracle/spectro.py`, `uvprobe`) reads the same streams with olefile (BSD-2-Clause) as a black box.

## Known gaps

Transmittance, photometric, kinetics and quantitation files have no development file; value codes other than 2 and 4 are returned unnamed. Only the first spectrum of a data set is read. The method pages are kept as text.

## Vocabulary (public API of `openreadout-spectro`, UVProbe)

| identifier | meaning |
| --- | --- |
| `UVPROBE_FORMAT_ID`, `UvProbeReader` | format id and reader |
| `is_uvprobe`, `parse`, `Opened`, `parsed`, `values`, `MAX_DATA_SETS` | detection, parsing and the parsed file with its y values |
| `HeaderInfo`, `name`, `created`, `organization`, `software`, `version`, `model`, `series`, `instrument`, `serial`, `header_info`, `method_page`, `value_kind`, `pstr` | `DataSetHeaderInfo` and method pages |
| `value_code`, `data_set`, `data_set_name`, `method_pages`, `header_first_x`, `header_last_x`, `header_step`, `points` | trace `extra` and vendor-tree keys |
| `absorbance`, `reflectance`, `unsupported_data_set`, `unknown_value_code`, `unreadable_header_info`, `data_set_count`, `compound_file` | channel names and `check` finding codes |
