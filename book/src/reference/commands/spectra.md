# spectra

`spectra` lists the scan headers of a mass-spectrometry run, or returns one spectrum's m/z and intensity arrays.

```text
openreadout spectra [OPTIONS] <FILE>
```

It reads Thermo `.raw`, mzML (also `.mzML.gz` and mzMLb), mzXML, imzML, Bruker `.d`, Agilent `.d`, Waters `.raw`, Sciex `.wiff`, ANDI/MS `.cdf` and ChemStation `.ms`.

## Flags

### Listing scans

- `--json`: print the JSON wrapper instead of text.
- `--run N`: run index, for files with more than one run (Sciex samples). Default 0.
- `--ms-level N`: only scans of this MS level (1 = full scans, 2 = MS/MS). With `--nth`, the level to count in.
- `--polarity positive|negative`: only scans of this polarity.
- `--rt-range START:END`: retention-time window in minutes. Either side may be empty (`5:`, `:12.5`).
- `--precursor MZ`: only MS/MS scans whose precursor m/z is within the tolerance.
- `--precursor-tol DA`: precursor tolerance in m/z units. Default 0.01.
- `--precursor-ppm PPM`: precursor tolerance in ppm instead.
- `--charge Z`: only precursors of this charge state.
- `--activation METHOD`: only this activation (`HCD`, `CID`, `ETD`, case-insensitive).
- `--scan-filter TEXT`: only scans whose filter string contains this text.
- `--offset N`: skip this many matching scans. Default 0.
- `--limit N`: list at most this many matching scans. Default 50, or every match with `--csv`. All matches are still counted.
- `--count`: only count the matching scans per MS level.
- `--csv`: write the matching scans as CSV to stdout, one row per scan.

### One spectrum

- `--scan N`: this scan number as the instrument counts it (1-based in Thermo files).
- `--spectrum I`: this zero-based spectrum index.
- `--nth K`: the K-th spectrum (from 1) of `--ms-level`. `--ms-level 2 --nth 1` is the first MS/MS scan.
- `--centroid`: the instrument's stored centroid list instead of the profile, when a scan has both.
- `--exclude-flagged`: Thermo `.raw`: leave out peaks the instrument flags as reference or background ions. Otherwise they are listed in `extra.flagged_peaks`.
- `--max-points N`: return at most this many points. `point_count` still reports the full size.

## Examples

```bash
openreadout spectra run.raw --ms-level 2 --count
openreadout spectra run.raw --ms-level 2 --precursor 445.12 --precursor-ppm 10 --csv > ms2.csv
openreadout spectra run.raw --ms-level 2 --nth 1 --centroid --json
```

## Scan numbers

`--scan N` follows each format's own numbering. For mzML it is the `scan=N` of the native id (else position + 1); for mzXML, `scan/@num`. For Bruker timsTOF, Waters and Sciex files it is the position + 1, and `native_id` says what the spectrum is (a frame, a PASEF precursor or window, a function and scan, or an MRM cycle and experiment). Per-format details are in [Formats](../../formats/index.md).

Chromatograms and peak areas are in [`analyze chromatogram` and `analyze peaks`](analyze.md#peaks-and-chromatogram).

## JSON

[`spectra`](../json/spectra.md) (scan headers), [`spectra --scan`](../json/spectrum.md) (one spectrum).

Run `openreadout spectra --help` for the full help of your installed version.
