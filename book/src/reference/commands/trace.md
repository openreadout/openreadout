# trace

`trace` returns the samples and statistics of one sweep of a sampled signal: electrophysiology, chromatography detector traces, NMR FIDs and spectra, IR, Raman and UV-Vis spectra, qPCR curves and other 1-D data.

```text
openreadout trace [OPTIONS] [FILE]...
```

`info` lists a file's traces under `traces[]`, with their sweeps, channels, units and sample rate.

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--trace N`: trace index. Default 0 (in a batch table, every trace).
- `--sweep N`: sweep (episode or segment) index. Default 0 (in a batch table, every sweep).
- `--channel N`: channel index. Repeatable. Default: all channels.
- `--first-sample N`: first sample of the window, zero-based within the sweep. Default 0.
- `--count N`: window length in samples. Default: to the end of the sweep.
- `--x-range A:B`: a window on the trace's own axis instead of `--first-sample`/`--count`: cm⁻¹, nm, ppm, or a chromatogram's retention time. Seconds for signals without an axis.
- `--max-samples N`: samples returned per channel, at most 100000. Default 1000. The statistics always cover the whole window.
- `--process`: NMR: read FIDs as spectra processed by OpenReadout.
- `--process-phase MODE`, `--process-lb HZ`, `--process-size N`, `--process-baseline MODE`: processing settings. See [`analyze nmr-peaks`](analyze.md#nmr-peaks).

Several files, a directory or a glob give one table with a row per file, trace, sweep and channel. `trace` takes the flags in [Several inputs](index.md#several-inputs) and the [batch table flags](batch.md#batch-table-flags).

## Examples

```console
$ openreadout trace synthetic-timeseries.nwb --trace 2 --channel 0
synthetic-timeseries.nwb (nwb): trace 2 sweep 0/1  samples 0..1000 of 1000  2000 Hz
  ch0 voltage[0]       volts  min -0.205000  max 0.184610  mean -0.010195  std 0.112583
      max at sample 857 = 1.928500 s
      first: -0.205000, -0.202270, -0.199540
```

The wavenumber of the strongest band of an IR spectrum, and the statistics of one band:

```bash
openreadout trace sample.0 --only /channels/0/stats/argmax_axis_value
openreadout trace sample.0 --x-range 1000:1100
```

## Reading the positions

- `argmin` and `argmax` are sample indices counted from the start of the sweep, not the window.
- `argmax_time_s` gives the position on the trace's clock. For a chromatogram this is the retention time, which can be negative when acquisition started before injection.
- `argmax_axis_value` gives the position on the trace's axis (ppm, cm⁻¹, nm, minutes), so no arithmetic is needed.
- Files without traces (images, FCS) exit 6. An index out of range exits 2.

Domain walkthroughs: [Electrophysiology](../../guides/ephys.md), [NMR](../../guides/nmr.md).

## JSON

[`trace`](../json/trace.md), [batch table](../json/batch-table.md).

Run `openreadout trace --help` for the full help of your installed version.
