# JASCO Spectra Manager `.jws`

JASCO Spectra Manager saves FT-IR, Raman, UV-Vis, circular dichroism and fluorescence measurements as `.jws` files. OpenReadout returns every channel as a spectrum with its axis, plus instrument, sample and acquisition settings (table below). Derived from public files written by FT/IR-4600, FT/IR-4700, NRS-5100, V-630, V-730, J-1500 and FP-8300 instruments, Spectra Manager text and CSV exports of the same measurements, and jws2txt (MIT) as a second opinion. Provenance: `docs/provenance/jasco-jws.md`. Crate: `openreadout-spectro`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `jasco-jws` | `.jws` of JASCO Spectra Manager (compound-file and flat containers); `.jrs` from the instruments' firmware (flat) | every channel as a spectrum (%T, %R, absorbance, single-beam, intensity, CD, HT voltage), its axis (wavenumber, Raman shift, wavelength, time), instrument model and serial, sample name and comment, measurement time, FT-IR acquisition settings | high (evidence rubric, `docs/assurance.md`) |

Not read: interval/kinetics files (`.jwb`), spectra-manager projects, JASCO's text exports (they
are text; `jcamp-dx`-like but not JCAMP).

## Containers

The first bytes decide; the extension is the same.

### Compound file (`D0 CF 11 E0`; Spectra Manager 1.x and 2.x)

| stream | content | our reading |
| --- | --- | --- |
| `Header` | UTF-16 `L~`, `SPCMAN2`, `R2.00.00`, `80x86`, `VC++6.0` | detection (`L~` … `SPCMAN`); `format_version` |
| `DataInfo` | u32 version (3); u32; u32; u32 **channel count**; u32 **grid flag** (1 regular, 0 explicit `X-Data`); u32 **points**; f64 **first x**, **last x**, **step** (negative when descending); four u32 **descriptors**: the x axis, then one per channel, the remaining slots repeating from the start; then the saved plot view. 96 bytes for one channel, +44 per further channel | axis, channels |
| `Y-Data` | float32, channel-major, exactly 4 × channels × points bytes | one trace per channel |
| `X-Data` | float32 per point (when the grid flag is 0: Raman CCD axes) | irregular axis (trace channel 0) |
| `ModuleInfo` | u32 version, module name, u16, u16 module id, **model**, **serial** (UTF-16 strings with a u32 byte length) | instrument |
| `SampleInfo` | u32, **sample name**, **comment**, a record count, then records (`u32 tag, u16 type, value`); record 1 is the **measurement time** (OLE date, UTC) | sample, comment, `started_at` |
| `MeasParam` | u32, u32, a record count, records | FT-IR settings (module 9, below); other instruments' records raw in the vendor tree |
| `BaseInfo` | u32, 16-byte id, u8, **original path**, f64 saved, f64 **measured** (OLE dates, UTC) | vendor tree; `started_at` when `SampleInfo`/`MeasParam` have none (Raman) |

Record value types: 2 u16, 3 u32, 4 f32, 5 and 7 f64, 8 UTF-16 string; records after a type not
in this list are kept as an undecoded byte count.

Axis descriptors: `0x10000100` wavenumber (1/cm, `INFRARED SPECTRUM`), `0x10000101` Raman shift
(1/cm, `RAMAN SPECTRUM`), `0x10000103` wavelength (nm), `0x20000203` time (a V-630 time course;
**no unit**: the time unit is not in a validated field). Any other descriptor is refused (exit 6).

Channel codes: `0x00` transmittance (%), `0x03` absorbance (AU), `0x08` single-beam (FT-IR
background), `0x0E` intensity (FP-8300 fluorescence, NRS-5100 CCD counts), `0x1001` circular
dichroism (mdeg when the file's CD sensitivity text, `MeasParam` record 20, says mdeg), `0x2001`
HT voltage (V). A code not in this list is returned as `channel_0x…` with a finding.

Refused (exit 6, with a hint): `DataInfo` version other than 3, more than three channels,
descriptor slots that do not follow the pattern or hold an axis descriptor where a channel code
belongs, grid flags other than 0 and 1. `Y-Data`/`X-Data`
sizes that do not match are corrupt (exit 4).

### Flat file (`L~S `; Spectra Manager 2.x, instrument firmware)

| offset | content |
| --- | --- |
| 0x08 | `SPECMAN` (Spectra Manager) or `SPECIRM` (firmware) | 
| 0x20 | `R2.0.0` |
| 0x84 | i32 points |
| 0x88, 0x90, 0x98 | f64 first x, last x, step |
| 0xA0 | x unit: 0 cm⁻¹, 3 nm (followed by `01 00 10`) |
| 0xA4 | y mode: 0 %T, 2 %R, 3 absorbance, 9 single-beam reference, 10 single-beam sample (followed by `00 00 00`) |
| 0xC8 | i64 data length (= 4 × points) |
| 0x140, 0x160, 0x180, 0x1C0 | model, serial, title, comment (NUL-terminated) |
| 0x2C0 | i32 time (Unix seconds, UTC) |
| 0x740 to the end | float32 y values (the data run from file length − data length, which is 0x740 in every file; any other value is refused as a truncated or extended file) |

Other container ids, versions, x units or y modes are refused.

## What the reader returns

- **One trace per channel**, one sweep: the channel's name as trace name and y quantity
  (`transmittance`, `absorbance`, `reflectance`, `single_beam`, `single_beam_reference`,
  `single_beam_sample`, `intensity`, `circular_dichroism`, `ht_voltage`), values as stored
  (float32; JASCO's −1.18e-38 invalid-point marker is kept, as its exports print it).
  `extra.axis` = `{quantity, unit, first, last, step, size}` with x = first + step × i (the stored
  last x is only compared: a finding when it disagrees by more than one step); an `X-Data` axis is
  `{irregular: true, channel: 0}` and the trace has two channels. `extra.data_type`:
  `INFRARED SPECTRUM`, `RAMAN SPECTRUM`, `UV/VIS SPECTRUM`, `CIRCULAR DICHROISM SPECTRUM`,
  `FLUORESCENCE SPECTRUM`, `UV/VIS KINETICS`; `extra.channel_code`, `extra.title`.
- **Experiment**: instrument vendor JASCO, model and serial; sample name; acquisition comment and
  `started_at` (UTC); for FT-IR (module 9) method parameters `accumulations`, `resolution` (cm⁻¹),
  `aperture` (mm), `scan_speed` (mm/s), `gain`, `filter` (Hz), `light_source`, `detector` — named
  from the exports' footers (records 1, 2, 3, 4, 6, 32, 47, 48).
- **Vendor tree** `jasco`: the container, header text, `DataInfo` fields and descriptors, module,
  sample records, measurement-parameter records (tag, type, value), base info (original path,
  saved and measured times), the stream list; for flat files every header field.
- `info --view structure`: the compound file's streams, or the flat file's header and data blocks.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/jasco_oracle/mod.rs`,
`oracle/jasco_oracle.py`):

- Spectra Manager exports of five measurements (FT-IR %T compound files in two locales, the Raman
  compound file with its `X-Data` axis, the V-730 flat file): point count, axis unit, y quantity, x
  within 0.005 and y to the export's six significant digits at 256 rows; model and serial; the
  eight FT-IR settings; the measurement time equal to the export's local time minus whole hours.
  One more export has the same name but is another acquisition: its instrument facts only.
- jws2txt (MIT, black box) on every compound file: the same channels (count, names), point counts
  and values at 64 points per channel (CD/HT/absorbance files included).
- One further depositor's file is held out (`docs/benchmark/heldout.md`).

## Vocabulary (public API of `openreadout-spectro`, JASCO)

| identifier | meaning |
| --- | --- |
| `JwsReader` | reader of JASCO `.jws`/`.jrs` files |
| `JWS_FORMAT_ID` | `jasco-jws` |
| `SpectroDataset` | the dataset the spectroscopy readers share |
