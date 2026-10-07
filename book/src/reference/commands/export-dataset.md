# export-dataset

`export-dataset` exports the data sets that a [`search`](search.md) query selects from an index as an ML-ready dataset: images as OME-Zarr, tables, traces and spectra as Parquet (or CSV), metadata JSON and a datasheet. It is resumable: a rerun continues where the last one stopped. Every file is verified.

```text
openreadout export-dataset [OPTIONS] <INDEX_DIR> <QUERY> <OUT>
```

The query has the syntax of [`search`](search.md#query-syntax); `""` selects everything.

## Flags

- `--limit N`: most data sets exported. Default: all.
- `--tables parquet|csv`: file format for tables, traces and spectra. Default `parquet`.
- `--redact`: replace values flagged as personal data with stable salted hashes.
- `--salt-file FILE`: file holding the redaction salt (or set `OPENREADOUT_REDACT_SALT`).
- `--license SPDX`: license of the whole export. Default: the nearest LICENSE file of each source.
- `--no-images`: leave images out.
- `--json`: print the JSON wrapper instead of text.

## Example

```bash
openreadout export-dataset /lab/index "sample~A12" dataset/ --redact --salt-file ~/.salt
```

Redaction and the personal-data rules: [Personal data](../../guides/lab-shares.md#personal-data).

## JSON

[`export-dataset`](../json/export-dataset.md).

Run `openreadout export-dataset --help` for the full help of your installed version.
