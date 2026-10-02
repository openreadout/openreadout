# Agilent (Varian) Cary UV-Vis

The Cary UV-Vis applications Scan, batch Scan and Scanning Kinetics save their data as `.dsw`, `.bsw` and `.bsk` files (`.csw` uses the same container). OpenReadout returns every spectrum and baseline as stored, absorbance in AU against wavelength in nm, with the scan parameters and run metadata listed below.

Derived from public files of two depositors (a Cary 4000 with Scan 3.00 and a Cary 60 with Scan 5.0) and the software's own CSV exports. Provenance: `docs/provenance/agilent-cary.md`. Crate: `openreadout-spectro`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `agilent-cary` | `.dsw` (Scan), `.bsw` (batch Scan), `.bsk` (Scanning Kinetics), `.csw` (same container) | every spectrum and baseline as stored (float32 x, y), absorbance in AU and wavelength in nm, spectrum names, collection times, operator, instrument model, application and version, scan parameters (rate, interval, averaging time, bandwidth, start/stop, source changeover, beam mode, baseline settings), kinetics schedule | medium (evidence rubric, `docs/assurance.md`) |

Not read: the graph, report, database and baseline-information stores (listed by `info --view structure` with their
offsets and sizes); `.csw` is accepted by extension on the evidence of the shared magic only (no
corpus file).

## Container

| offset | content |
| --- | --- |
| 0x00 | `0x11` then `Varian UV-VIS-NIR` (a length-prefixed text): detection |
| 0x3E | the first *store* |

A store is a u32-length-prefixed class name (`TContinuumStore`, `TBaselineStore`,
`TGraphStore`, `TReportStore`, `TDatabaseStore`, `TBaselineInfoStore`), then a u32 **store size
counted from the store's first byte**; the next store starts at `start + size`. In every corpus
file the last store ends 4 bytes before the end of the file; any other end is a finding
(`store_chain_end`). A store that runs past the end of the file is corrupt (exit 4). Files over
256 MiB are refused (exit 6; the corpus files are under 200 kB and are read whole).

### Spectrum store (`TContinuumStore`) and baseline store (`TBaselineStore`)

After the size field, a 1,028-byte header:

| offset in header | type | content |
| --- | --- | --- |
| 0 | u32 | (not decoded; `word_0` in the vendor tree) |
| 4 | u32 | store version: **149** in every file; any other is refused (exit 6) |
| 8, 12 | f32 | x minimum, x maximum |
| 16, 20 | f32 | y minimum, y maximum |
| 24 | u32 | **point count** |
| 28 | u32 | (not decoded; `word_7`) |

Then **point count × (f32 x, f32 y)** pairs; points that do not fit in the store are corrupt. The
stored extremes are compared with the values' (a finding, `stored_extremes`, when they differ).

Then u32-length-prefixed Latin-1 texts, up to the first bytes that are not one:

| text | our reading |
| --- | --- |
| first text | the spectrum's **name** (the CSV export's title row) |
| `Collection Time: 12/18/2024 10:12:52 PM` | collection time, local clock, **month first** (every file with a day above 12 puts the month first, the Polish depositor's too) |
| `Operator Name : …` | operator |
| `Scan Software Version: 3.00(182)`, `Scan Version 5.0.0.999`, `Scanning Kinetics Version 5.0.0.999` | application and version |
| `Parameter List : ` then one text per parameter | parameters: split on runs of two or more spaces into a name and values (Scan 3.00 writes fixed 302-character fields, 5.0 `␣␣name␣␣value[␣␣value]`) |
| `Method Log :` … `End Method Modifications` | method log; `Method Name :`, `Date/Time stamp:` |
| `<SBW (nm)> , 2.000`, `<Current Wavelength> , 300.00`, `[Time] , 2.000` | status texts; `[Time]` is the kinetics method's **scheduled** time of the spectrum (minutes) |

The bytes after the last text are counted (`trailing_bytes`), not decoded.

### Axes

`X Mode` `Nanometers` → wavelength (nm). A Scanning Kinetics file names no X mode: nm is assumed
(its method log's `X Start nm`/`X Stop nm` bound the values; a note says so and the assurance
profile records it). Any other X mode returns the values without a unit, with a finding
(`x_mode`). `Y Mode` (`Ordinate mode` in kinetics files) `Abs` → absorbance (AU), validated
against the CSV exports; `%T`/`%R` → transmittance/reflectance (%) with a finding (`y_mode`: no
export checks them); anything else under its own lower-cased name without a unit.

## What the reader returns

- **Traces:** one per *set* of spectra that share their x values (bit for bit), y mode and kind
  (sample spectra first, then baselines): name `absorbance` (`baseline absorbance` for
  baselines), one sweep per spectrum, values as stored (float32). `extra.axis` is the regular
  wavelength axis when the x values are evenly spaced, else the listed x values;
  `extra.spectrum_names`, `extra.y_mode`, `extra.x_mode`, `extra.data_type` `UV/VIS SPECTRUM`.
- **Tables:** for a set of several spectra (batch and kinetics files), one row per spectrum:
  `collected_s` (seconds after the set's first `Collection Time`) and, in kinetics files,
  `scheduled_min` (the `[Time]` text). The two differ: the Scanning Kinetics example's spectra are
  scheduled at 0, 2, 4, 6, 36, … min and collected 0, 0.9, 2.9, 4.9, 6.9, 36.9, … min after the first.
- **Experiment** (from the first sample spectrum): vendor `Agilent (Varian)`, model (parameter
  `Instrument`: `Cary 4000`, `Cary 60`), software and version, operator, sample name (the
  spectrum's name), `started_at` (its `Collection Time`, local clock, no zone), method name, and
  the method parameters `scan_rate` (nm/min), `data_interval` (nm), `averaging_time` (s),
  `spectral_bandwidth` (nm), `scan_start`, `scan_stop`, `source_changeover` (nm), `beam_mode`,
  `baseline_correction`, `baseline_type`, `slit_height`, `signal_to_noise_mode`, `cycle_mode` —
  all `inferred`.
- **Vendor tree** (`info --view full`): `cary.stores` (class, offset, size), `chain_end`, `file_tail`, and per
  spectrum its header words, texts, parameters, method log and status texts.

## Validation

`corpus/oracle/series/cary-*.json` (oracle/series_oracle.py on each file's CSV export): every
point's x (to 5·10⁻⁴ nm) and y (the CSV writes the float32 values to ten significant digits). The
Cary 60 files have no export: checked for self-consistency (stored extremes; the parameter
Start/Stop against the x range; the store chain ending 4 bytes before the end of the file).

## Vocabulary

| name | meaning |
| --- | --- |
| `CaryReader` | the `FormatReader` for `agilent-cary` |
| `CARY_FORMAT_ID` | `"agilent-cary"` |
| `AGILENT_CARY` | the assurance profile (`crates/openreadout-spectro/src/assurance.rs`) |
| `spectrum_names`, `y_mode`, `x_mode`, `collected_s`, `scheduled_min` | trace and table fields above |
| `scan_rate`, `data_interval`, `averaging_time`, `spectral_bandwidth`, `scan_start`, `scan_stop`, `source_changeover`, `beam_mode`, `baseline_correction`, `baseline_type`, `slit_height`, `signal_to_noise_mode`, `cycle_mode` | method parameters above |
| `store_chain_end`, `stored_extremes`, `x_mode`, `y_mode` | finding codes |
| `stores`, `chain_end`, `file_tail`, `word_0`, `word_7`, `trailing_bytes`, `method_saved`, `method_log`, `status` | vendor-tree keys |
