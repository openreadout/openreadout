# info

`info` reports what a file holds: its format, images, tables, traces, spectra, plate and experiment. It reads headers only, so it stays fast on multi-gigabyte files.

```text
openreadout info [OPTIONS] [FILE]...
```

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--view summary|full|structure|explain|format`: what to show. Default `summary`.
  - `full` adds per-frame records, per-field provenance and the vendor's metadata tree.
  - `structure` lists the container: images, planes, blocks, attachments and pyramid levels.
  - `explain` describes the file in plain words, with caveats and suggested next commands.
  - `format` only identifies the format from the file's signature.
- `--ask QUESTION`: answer a question in plain words ("which channel is DAPI?") from the experiment model. Implies `--view explain`.
- `--max-images N`: list at most N images (0 = all). Screening plates list their first 16 field images by default; `images_total` gives the count.

### With `--view full`

- `--no-vendor`: leave out the vendor metadata tree.
- `--no-provenance`: leave out the provenance map.
- `--all-frames`: embed every per-frame record (default: the first 100 per image).
- `--redact`: replace values flagged as personal data with stable salted hashes. Needs `--salt-file` or `OPENREADOUT_REDACT_SALT`. See [Personal data](../../guides/lab-shares.md#personal-data).
- `--salt-file FILE`: file holding the redaction salt.
- `--sidecar[=DIR]`: write each input's full metadata to `<file>.openreadout.json` next to it, or under `DIR` mirroring the input paths, instead of printing it. An up-to-date sidecar is left alone.

### Several inputs and tables

`info` takes the flags in [Several inputs](index.md#several-inputs). With `--tidy`, `--fields`, `--sample-sheet` or `-o` it writes one metadata row per data set; see the [batch table flags](batch.md#batch-table-flags). For `info`, `--fields` takes index field names or `all`.

## Examples

```console
$ openreadout info mini.czi
mini.czi
  format: Zeiss CZI (czi) v1.0  size: 2.0 KiB  images: 1  planes: 1
  assurance: validated - …
  [0] -  8x8 z=1 c=1 t=1  uint8 XYCZT  px=0.1000 µm
      ch0 c0
```

Pick single values out of the JSON with the global `--only` flag:

```bash
openreadout info mini.czi --only /images/0/physical_size,/images/0/pixel_type
```

Explain a file, or ask about it:

```bash
openreadout info run.raw --view explain
openreadout info run.raw --ask "what was the gradient?"
```

Write metadata sidecars for a whole folder:

```bash
openreadout info --view full --sidecar -r --skip-unknown /data/2026-09-21
```

## Notes

- Pyramidal and tiled images report `resolution_levels`: one entry per level, full resolution first, with sizes, downsample factors and tile sizes. Use `--level` and `--region` on `stats`, `preview`, `export` and `check --planes` to read them.
- A screening plate (Harmony, ImageXpress, CellVoyager, OME-Zarr HCS) is one data set whose images are its fields. `plate.wells[].images` lists the image indices of each well, and `images[i].extra.well` and `field` go the other way. See [`stats --per well`](stats.md) for per-well numbers.
- How units, timestamps and the experiment model are normalized: [Metadata](../../guides/metadata.md).

## JSON

- [`info`](../json/info.md), [`info --view full`](../json/info-full.md), [`info --view structure`](../json/info-structure.md), [`info --view explain`](../json/info-explain.md), [`info --view format`](../json/info-format.md)
- [`--sidecar` report](../json/sidecar.md) and [sidecar file](../json/sidecar-file.md)
- [Batch table](../json/batch-table.md) and [summary](../json/batch-summary.md)

Run `openreadout info --help` for the full help of your installed version.
