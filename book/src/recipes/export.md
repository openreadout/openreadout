# Convert to an open format

Convert a vendor file when you need to open it in software that cannot read the original, hand it to a collaborator, or deposit it in an archive. `export` writes OME-TIFF or OME-Zarr for images, mzML for mass spectrometry, CSV, Parquet or Arrow for tables and traces, NWB for electrophysiology, and a few others. It never modifies the source.

## Run it

```text
$ openreadout export mini.nd2
wrote mini.ome.tiff (1 images, 2 planes, 2612 bytes, verified=true)
```

`mini.nd2` is a small Nikon file in the repository at [`crates/openreadout-cli/tests/fixtures/mini.nd2`](../../../crates/openreadout-cli/tests/fixtures/mini.nd2). The other files on this page are public test files under [`fuzz/corpus/`](../../../fuzz/corpus/). Every output is real.

## What it tells you

- `wrote` names the new file. Without `-o` it goes next to the input, named after it with the target's extension.
- `verified=true` means the export was read back and compared with the source before it was renamed into place. Until then it exists only under a temporary name, so an interrupted export never leaves a half-written file behind.
- The target is chosen from the data: OME-TIFF for images, CSV for tables and traces, mzML for mass spectrometry. Pick another with `--to`.
- An existing output is never replaced unless you pass `--overwrite`. Without it, `export` stops with exit code 2.
- A combination that does not fit, such as an image file `--to csv`, exits 6 with a hint that names the target to use.

## Variations

### Other targets

```text
$ openreadout export mini.nd2 --to ome-zarr
wrote mini.ome.zarr (1 images, 2 planes, 2876 bytes, verified=true)

$ openreadout export fcsparser-cyflow-cube-8.fcs --to parquet
wrote fcsparser-cyflow-cube-8.parquet (parquet table 0, 725 rows x 10 columns, snappy, 52461 bytes, verified=true)

$ openreadout export pyteomics-tiny-pwiz.mzML -o tiny.mzML
wrote tiny.mzML (4 spectra, 40 points, 15667 bytes, verified=true)

$ openreadout export pyabf-2018-12-09-pclamp11-0001.abf --to nwb
wrote pyabf-2018-12-09-pclamp11-0001.nwb (NWB 2.7.0: 20 TimeSeries, 40000 samples, 783281 bytes, verified=true)
  acquisition/trace0_sweep0: trace 0, sweep 0, 2000 samples x 1 channels (A)
  ...
```

| `--to` | for |
| --- | --- |
| `ome-tiff`, `ome-zarr` | images; `ome-zarr` also for screening plates |
| `csv`, `parquet`, `arrow` | tables (FCS events, plate reads), traces; Parquet and Arrow also mass spectra |
| `mzml` | mass spectrometry |
| `nwb` | electrophysiology traces |
| `jcamp` | NMR, 1-D spectra, chromatograms |
| `asm` | plate-reader exports (Allotrope Simple Model JSON) |
| `rdml` | qPCR |

Table values are written as stored: FCS events are not compensated or scaled. Trace values are in physical units.

### Part of a file

`--image` picks one image (a CZI scene, an ND2 position, a LIF series). `--select` picks planes, and is repeatable; axes you do not mention are exported in full:

```text
$ openreadout export zstack.czi --select c=0 -o c0.ome.tiff
wrote c0.ome.tiff (1 images, 21 planes, 12639 bytes, verified=true)
```

`zstack.czi` is a copy of `fuzz/corpus/whole_czi/zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging.czi`, with 2 channels and 21 z-planes. For a whole-slide image, `--region X,Y,W,H` exports one rectangle and `--level N` one pyramid level.

### Compare the export with its source

The export is verified as it is written, but you can confirm it yourself at any time, for example after copying it to an archive:

```text
$ openreadout check mini.nd2 --against mini.ome.tiff
mini.nd2 (nd2)
mini.ome.tiff (tiff)
=> identical
metadata: same (0 differences)
image 0: geometry same, channel names same, physical size same
planes: 2 compared, 2 identical, 0 within tolerance, 0 mismatched
```

It exits 0 when the files are identical and 1 when they differ. The comparison covers metadata as well as pixels, and OME-Zarr does not store every vendor field, so `mini.nd2 --against mini.ome.zarr` reports `different` (objective and instrument are absent) even though all planes are identical. Add `--no-metadata` to compare the data only. Planes are compared only between images of the same geometry, so check a `--select` export by comparing plane hashes: `check zstack.czi --planes --select c=0` and `check c0.ome.tiff --planes` print the same xxh3-128 hash for each plane.

### A folder, or from an assistant

```bash
openreadout export -r --skip-unknown raw/ -o ome/      # keeps the folder layout
```

An assistant calls the MCP tool `openreadout_export` with `file` and `format`. It writes and verifies the same way, and replaces an existing output only with `overwrite: true`. CSV export is available only on the command line.

## More

- [`export` reference](../reference/commands/export.md): every flag, pyramids, plates and attachments.
- [Is this file intact?](check-files.md): `check` and `check --against`.
- [Metadata](../guides/metadata.md#ome-xml-export): what goes into the OME-XML.
- JSON: [`export`](../reference/json/export.md), [`check --against`](../reference/json/check-against.md).
