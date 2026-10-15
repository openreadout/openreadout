# spectrum

`spectrum` returns one mass spectrum: its scan metadata, its most intense peaks with their charge states, and its m/z and intensity arrays. To list the scans of a run, see [`scans`](scans.md).

```text
openreadout spectrum [OPTIONS] <--rt <MIN>|--scan <N>|--spectrum <I>> <FILE>
```

MCP: `openreadout_spectrum`.

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--run N`: run index, for files with more than one run (Sciex samples). Default 0.
- `--rt MIN`: the scan nearest this retention time in minutes, of MS level 1 or `--ms-level`. Use it for the spectrum at a chromatogram's apex.
- `--scan N`: this scan number as the instrument counts it (1-based in Thermo files).
- `--spectrum I`: this zero-based spectrum index, counted within `--ms-level` when it is given.
- `--ms-level L` with `--spectrum I`: count `I` among the spectra of MS level `L` only. With `--rt`, the MS level of the scan. MS1 and MS/MS scans are interleaved, and `--ms-level 2 --spectrum 0` is the first MS/MS scan.
- `--centroid`: the instrument's stored centroid list instead of the profile, when a scan has both.
- `--exclude-flagged`: Thermo `.raw`: leave out peaks the instrument flags as reference or background ions. Otherwise they are listed in `extra.flagged_peaks`.
- `--lock-mass MZ|auto`: Waters `.raw`: correct m/z with the lock-spray reference scans, `MZ` being the lock compound's m/z (`auto`: the Lock Mass the file records). Each scan is multiplied by the reference over the lock peak measured in the lock-spray scans within 60 s of it; `extra.lock_mass_correction` gives the factor. Off by default: m/z are as stored. Against MassLynx's own correction of the same runs the result agrees to about 2 ppm. `chromatogram` takes the same option for accurate-mass XICs.
- `--max-points N`: return at most this many points, the most intense ones, in m/z order. `point_count` still reports the full size. MCP returns 1000 by default.

## Examples

```bash
openreadout spectrum run.raw --rt 93.15
openreadout spectrum run.raw --scan 1200 --json
openreadout spectrum run.raw --ms-level 2 --spectrum 0 --centroid --json
```

## Peaks and charge states

`peaks` lists the 20 most intense peaks of the whole spectrum (MCP `top_peaks` changes the number). In a profile spectrum the peaks are its local maxima; a centroided one is used as stored. For each peak the charge is the z whose isotope spacing, 1.00336/z, finds the most isotope peaks in a row above it (each at least 2 % of the peak, within 15 ppm or 0.003). `monoisotopic_mz` steps down the same pattern while the lower isotope is present, and `neutral_mass_da` is z × (monoisotopic m/z − 1.00728) in positive mode (+ in negative mode). A peak inside the pattern of a more intense one is not listed again. This is a quick reading of the pattern, not a deconvolution: overlapping patterns can give the wrong charge.

## Scan numbers

`--scan N` follows each format's own numbering. For mzML it is the `scan=N` of the native id (else position + 1); for mzXML, `scan/@num`. For Bruker timsTOF, Waters and Sciex files it is the position + 1, and `native_id` says what the spectrum is (a frame, a PASEF precursor or window, a function and scan, or an MRM cycle and experiment). Per-format details are in [Formats](../../formats/index.md).

## JSON

[`spectrum`](../json/spectrum.md).

Run `openreadout spectrum --help` for the full help of your installed version.
