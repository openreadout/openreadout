# scans

`scans` lists the scan headers of a mass-spectrometry run without decoding any peaks: scan number, MS level, retention time, polarity, precursor m/z and charge, isolation window, activation, collision energy, filter string and the stored total ion current. To read one spectrum's arrays, see [`spectrum`](spectrum.md).

```text
openreadout scans [OPTIONS] <FILE>
```

MCP: `openreadout_scans`.

It reads Thermo `.raw`, mzML (also `.mzML.gz` and mzMLb), mzXML, imzML, Bruker `.d`, Agilent `.d`, Waters `.raw`, Sciex `.wiff`, ANDI/MS `.cdf` and ChemStation `.ms`. IR, Raman, UV-Vis and NMR spectra are traces: see [`trace`](trace.md).

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--run N`: run index, for files with more than one run (Sciex samples). Default 0.
- `--ms-level N`: only scans of this MS level (1 = full scans, 2 = MS/MS).
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

## Examples

```bash
openreadout scans run.raw --ms-level 2 --count
openreadout scans run.raw --ms-level 2 --precursor 445.12 --precursor-ppm 10 --csv > ms2.csv
```

Chromatograms and peak areas are in [`analyze chromatogram` and `analyze peaks`](analyze.md#peaks-and-chromatogram).

## JSON

[`scans`](../json/scans.md).

Run `openreadout scans --help` for the full help of your installed version.
