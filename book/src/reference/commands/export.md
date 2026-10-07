# export

`export` converts a file to an open format. To write one embedded attachment as it is stored, see [`extract`](extract.md).

```text
openreadout export [OPTIONS] <FILE>...
```

Each export is written under a temporary name, read back and compared with the source, then given its final name. The source file is not modified.

## Flags

### Target and output

- `--format FORMAT`: target format: `ome-tiff`, `ome-zarr`, `mzml`, `csv`, `asm`, `parquet`, `arrow`, `nwb`, `jcamp` or `rdml`. The default depends on the file; see [Export formats](#export-formats).
- `-o`, `--output PATH`: output path. Default: the input name with the target's extension. With several inputs, a directory.
- `--overwrite`: replace an existing output file or OME-Zarr store.
- `--compression MODE`: images `none`, `deflate` (default) or `lzw`; Parquet `none`, `snappy` (default) or `lz4`; Arrow `none` (default) or `lz4`.
- `--json`: print the export report as JSON.

### What to export

- `--image N`: only this image. Default: all images.
- `--select SEL`: plane selection such as `c=0`, `z=2-5`, `t=0,3`. Repeatable. Axes not mentioned are exported in full.
- `--table N`: CSV, Parquet, Arrow: this table. Default 0.
- `--trace N`: trace exports: this trace. Default 0 (NWB: every trace).
- `--sweep N`: trace exports: this sweep. CSV default 0; Parquet, Arrow, NWB and JCAMP-DX default every sweep.
- `--rows A-B`: zero-based inclusive row range (`A-B`, `A-` or `A`). For traces, samples of each sweep.
- `--spectra`: Parquet and Arrow: export the mass spectra of `--run` (one row per point, plus a per-scan summary file).
- `--run N`: mzML, Parquet, Arrow: run index. Default 0.
- `--labels`: CSV: add a second header line with column labels (FCS `$PnS`).

### Images: pyramids, levels and regions

- `--pyramid auto|source|mean|none`: lower resolutions to write. `auto` copies the source's own pyramid when it has one and the whole image is exported from level 0; otherwise it writes 2×2 mean levels for OME-Zarr and none for OME-TIFF.
- `--levels N`: resolution levels including full resolution. Exact for mean pyramids, a maximum for the source's own.
- `--level N`: export this source pyramid level as the full resolution. Default 0.
- `--region X,Y,W,H`: export only this rectangle of every plane, in the pixels of `--level`, read tile by tile.
- `--chunk-size N`: OME-Zarr chunk edge, or OME-TIFF tile edge (a multiple of 16). Default 512.
- `--embed-vendor`: embed the vendor metadata tree as an OME `StructuredAnnotation`.

### Plates

- `--well WELL`: export only the fields of this well (`C05`). Repeatable. OME-Zarr and `--per-image` OME-TIFF.
- `--skip-incomplete`: leave out fields whose plane files are missing instead of failing.
- `--no-plate`: OME-Zarr: write a `bioformats2raw.layout` collection instead of an OME-NGFF HCS plate.
- `--per-image`: OME-TIFF: one file per field of view, into the directory given by `-o`.

### Mass spectrometry and NMR

- `--centroid`: mzML: write the instrument's centroid lists instead of profiles where a scan has both.
- `--process`: NMR: export FIDs as spectra processed by OpenReadout. Stored spectra are unchanged.
- `--process-phase MODE`, `--process-lb HZ`, `--process-size N`, `--process-baseline MODE`: processing settings. See [`analyze nmr-peaks`](analyze.md#nmr-peaks).

`export` takes the flags in [Several inputs](index.md#several-inputs).

## Examples

```console
$ openreadout export doctor.tif -o doctor.ome.tiff
wrote doctor.ome.tiff (1 images, 2 planes, 2207 bytes, verified=true)
```

```bash
openreadout export slide.czi --format ome-zarr                       # pyramid kept
openreadout export slide.svs --region 20000,15000,4096,4096 -o roi.ome.tiff
openreadout export sample.fcs --format parquet
openreadout export run.raw --format mzml --centroid
openreadout export -r --skip-unknown raw/ -o ome/                # keeps the folder layout
```

## Export formats

| `--format` | for | default output |
| --- | --- | --- |
| `ome-tiff` | images (default) | `<stem>.ome.tiff` |
| `ome-zarr` | images, plates | `<stem>.ome.zarr` |
| `csv` | tables, traces, NMR and 1-D spectra (default) | `<stem>.csv` |
| `parquet`, `arrow` | tables, traces, mass spectra | `<stem>.parquet`, `<stem>.arrow` |
| `mzml` | mass spectrometry (default) | `<stem>.mzML` |
| `nwb` | electrophysiology traces | `<stem>.nwb` |
| `jcamp` | NMR, 1-D spectra, chromatograms | `<stem>.jdx` |
| `asm` | plate-reader exports | `<stem>.asm.json` |
| `rdml` | qPCR (RDML, `.eds`, `.rex`) | `<stem>.rdml` (`<stem>.export.rdml` for an RDML input) |

- **OME-TIFF** is one BigTIFF file with OME-XML in the first IFD. Pyramids go in SubIFDs. With a pyramid, a level, a region, or a plane above 4 GiB, the file is tiled and written block by block, so a whole-slide image exports in bounded memory.
- **OME-Zarr** is an OME-NGFF 0.5 (Zarr v3) store with 5-D `t, c, z, y, x` arrays. Several images become a `bioformats2raw.layout` collection; a screening plate becomes an HCS plate. Except for plates, `OME/METADATA.ome.xml` keeps what OME-NGFF has no place for: objective, instrument, acquisition mode, exposures.
- **CSV** of a table writes column names on line 1 and raw stored values. FCS values are not compensated or scaled. CSV of a trace starts with a `time_s` column (or the trace's own axis, such as `chemical_shift_ppm`) followed by one column per channel in physical units.
- **Parquet and Arrow** keep each column's stored type. Units, labels, provenance and the source's `info` are stored as field and file metadata.
- **mzML** is indexed mzML 1.1.0, one spectrum per scan of the chosen run. An mzML source also keeps its chromatograms (TIC, SRM traces) and its exact instrument, component and software terms. A run with chromatograms and no spectra (an MRM-only mzML, a Waters MRM `.raw`) is written as its chromatograms alone, a Waters MRM table as a TIC and one SRM chromatogram per transition.
- **NWB** writes one `TimeSeries` per trace, sweep and unit, in physical units.
- **JCAMP-DX** is version 5.01 or 6.00. Values round-trip exactly when they are integer multiples of one factor; the report says whether they did.
- **ASM** is Allotrope Simple Model plate-reader JSON, one document per plate and well.
- **RDML** is RDML 1.3 with the plate setup, Cq values, amplification curves and melt data.

A combination that does not fit, such as an image file `--format csv`, exits 6.

## JSON

[`export`](../json/export.md).

Run `openreadout export --help` for the full help of your installed version.
