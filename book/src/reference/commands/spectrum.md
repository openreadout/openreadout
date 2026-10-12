# spectrum

`spectrum` returns one mass spectrum's m/z and intensity arrays with its scan metadata. To list the scans of a run, see [`scans`](scans.md).

```text
openreadout spectrum [OPTIONS] <--scan <N>|--spectrum <I>|--nth <K>> <FILE>
```

MCP: `openreadout_spectrum`.

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--run N`: run index, for files with more than one run (Sciex samples). Default 0.
- `--scan N`: this scan number as the instrument counts it (1-based in Thermo files).
- `--spectrum I`: this zero-based spectrum index.
- `--nth K` with `--ms-level N`: the K-th spectrum (from 1) of that MS level. `--ms-level 2 --nth 1` is the first MS/MS scan.
- `--centroid`: the instrument's stored centroid list instead of the profile, when a scan has both.
- `--exclude-flagged`: Thermo `.raw`: leave out peaks the instrument flags as reference or background ions. Otherwise they are listed in `extra.flagged_peaks`.
- `--lock-mass MZ|auto`: Waters `.raw`: correct m/z with the lock-spray reference scans, `MZ` being the lock compound's m/z (`auto`: the Lock Mass the file records). Each scan is multiplied by the reference over the lock peak measured in the lock-spray scans within 60 s of it; `extra.lock_mass_correction` gives the factor. Off by default: m/z are as stored. Against MassLynx's own correction of the same runs the result agrees to about 2 ppm. `chromatogram` takes the same option for accurate-mass XICs.
- `--max-points N`: return at most this many points. `point_count` still reports the full size.

## Examples

```bash
openreadout spectrum run.raw --scan 1200 --json
openreadout spectrum run.raw --ms-level 2 --nth 1 --centroid --json
```

## Scan numbers

`--scan N` follows each format's own numbering. For mzML it is the `scan=N` of the native id (else position + 1); for mzXML, `scan/@num`. For Bruker timsTOF, Waters and Sciex files it is the position + 1, and `native_id` says what the spectrum is (a frame, a PASEF precursor or window, a function and scan, or an MRM cycle and experiment). Per-format details are in [Formats](../../formats/index.md).

## JSON

[`spectrum`](../json/spectrum.md).

Run `openreadout spectrum --help` for the full help of your installed version.
