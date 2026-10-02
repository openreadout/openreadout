# MCP tools

`openreadout mcp` runs OpenReadout as a Model Context Protocol server. It speaks over standard input and output by default, or over HTTP in builds that include it (see [Transports](#transports)). To connect a client, see [AI agents](../guides/agents.md).

The tools mirror the command line and return the same JSON as its `--json` output (the `data` part). Each result is sent twice: as `structuredContent`, checked against the tool's output schema, and as one text block with the same JSON for clients that only read text.

When a client connects, the server sends short instructions: indices are zero-based; values are raw as stored unless a tool says it processed them; each file reports an assurance level; only `openreadout_export` writes next to the data; errors carry a code and a hint.

## Tools

The server has 15 tools.

| tool | what it does | command |
| --- | --- | --- |
| `openreadout_info` | what a file holds, from its headers | [info](commands/info.md) |
| `openreadout_check` | integrity check, or comparison of two files | [check](commands/check.md) |
| `openreadout_preview` | a picture of the data | [preview](commands/preview.md) |
| `openreadout_stats` | pixel statistics, or per-well statistics of a plate | [stats](commands/stats.md) |
| `openreadout_trace` | samples and statistics of one sweep | [trace](commands/trace.md) |
| `openreadout_table` | rows of a table | [table](commands/table.md) |
| `openreadout_spectra` | mass-spectrum scan headers, or one spectrum | [spectra](commands/spectra.md) |
| `openreadout_analyze` | an analysis with a documented method | [analyze](commands/analyze.md) |
| `openreadout_export` | export to an open format | [export](commands/export.md) |
| `openreadout_batch` | one measure over many files as one table | [batch](commands/batch.md) |
| `openreadout_link` | group files of the same sample | [link](commands/link.md) |
| `openreadout_index` | catalog every data set under directories | [index](commands/index-cmd.md) |
| `openreadout_search` | query an index | [search](commands/search.md) |
| `openreadout_watch` | follow directories an instrument writes to | [watch](commands/watch.md) |
| `openreadout_formats` | supported formats and their known gaps | [self formats](commands/self.md) |

Every tool that reads a file takes `file`: an absolute path, or a path relative to the server's working directory. It can also be a data-set directory, such as a Bruker experiment or an Agilent `.D` folder. The output schemas are listed by the server and described in the [JSON output reference](json/index.md).

### Strict mode

`openreadout_info`, `openreadout_check`, `openreadout_preview`, `openreadout_stats`, `openreadout_trace`, `openreadout_table`, `openreadout_spectra`, `openreadout_analyze` and `openreadout_export` take `strict: true`. A strict call refuses values the file's assurance does not validate, with an error whose `exit_code` is 6. Start the server with `OPENREADOUT_STRICT=1` (or `openreadout --strict mcp`) to make every call strict. See [Assurance and strict mode](assurance.md).

### openreadout_info

Arguments:

- `file`
- `view`: `summary` (default), `full`, `structure`, `explain` or `format`, as in `info --view`.
- `ask`: a question in plain words, answered under `answers` with the fields each answer came from. Implies `view: "explain"`.
- `max_images`: list at most this many images (0 = all).
- `thumbnail`: with `view: "summary"`, attach a small picture of image 0 (default `true`). See [Pictures](#pictures).
- `vendor`: with `view: "full"`, include the vendor's metadata tree (default `false`; it can be megabytes).
- `max_frames`: with `view: "full"`, per-frame records per image (default 100; 0 for none, -1 for all).

A file still being written gets the same `acquisition` block as on the command line.

### openreadout_check

Without `against` or `report`, returns the integrity report: `ok` and a list of findings.

- `against`: compare `file` with this second file (for example its export): metadata differences, geometry, channel names and per-plane hashes. `image`, `select`, `tolerance`, `ignore` and `no_pixels` work as in `check --against`.
- `report: true`: build the privacy-reviewed diagnostic bundle of `check --report`, for a file that fails or is not validated. It contains no data values, and free text only with `include_text: true`. It is written to a file only when `output` is given.

### openreadout_preview

Returns a picture as image content, followed by JSON that says what was drawn. Arguments as in [`preview`](commands/preview.md): `image`, `select`, `mip`, `composite`, `level`, `region` (`{x, y, width, height}`), `max_size` (default 768, at most 2048), `axes` (rulers and scale bar; default `true`), `grid`, `contrast`, `lut` and `format`; for traces `trace`, `sweep` and `channels`; for spectra `run`, `spectrum`, `scan` and `centroid`; for plates `table` and `column`.

### openreadout_stats

Per channel and image (the default), with every plane (`per: "plane"`), or per well or field of a screening plate (`per: "well"` or `"field"`, with `wells` to choose wells). Other arguments: `image`, `select`, `level`, `region`, `bins` (default 32), `scale` (`linear` or `log`) and `mip` (`z` or `t`).

### openreadout_trace

One window of one sweep, in physical units, with per-channel statistics (including `argmax_axis_value`, the position of the maximum on the trace's axis) and the first `max_samples` values (default 200, at most 10000). Arguments: `trace`, `sweep`, `channels`, `first_sample` and `count`, or `x_range` on the trace's own axis; `process: true` turns an NMR FID into a spectrum.

### openreadout_table

Rows of a table: `table`, `first_row`, `max_rows` (default about 5000 values, at most 10000 rows). For FCS files, `compensate`, `transform`, `transform_parameters`, `workspace`, `gatingml`, `sample` and `populations` work as in [`table`](commands/table.md). `filter` takes conditions such as `"FITC-A > 1000"`; with `count: true`, only the number of matching rows is returned.

### openreadout_spectra

Without `scan`, `index` or `nth`, lists the scan headers of a run without decoding peaks, filtered by `ms_level`, `polarity`, `rt_range` (minutes), `precursor_mz` (within 0.01 m/z, or `ppm`), `charge`, `activation` and `scan_filter`. Every match is counted; `limit` (default 100, at most 5000) and `offset` page the list. With `scan`, `index`, or `ms_level` and `nth`, returns one spectrum's `mz` and `intensity`, at most `max_points` (default 2000); `centroid: true` returns the stored centroid list.

### openreadout_analyze

`kind` picks the analysis, and `options` holds its settings. An option the kind does not take is an error that names the ones it does. The options are the same as the flags of [`analyze`](commands/analyze.md), and `openreadout_batch` takes the same ones.

- `peaks`: chromatographic peaks with areas, widths, tailing, plates, resolution and S/N; compound lists; bands and regions of IR, Raman, UV-Vis and NMR spectra. Options: the source options of `chromatogram`, plus `smooth`, `min_snr`, `min_height`, `min_width`, `baseline`, `area_seconds`, `rt`, `window`, `pick`, `integrate`, `x_range`, `compounds` and `summary_only`. See [Chromatograms and peaks](../guides/quantitation.md).
- `chromatogram`: TIC, BPC, XIC and SRM chromatograms, or stored detector traces. Options: `tic`, `bpc`, `mz`, `ppm`, `da`, `transitions`, `transition_tol`, `traces`, `channel`, `sweep`, `run`, `ms_level`, `polarity`, `scan_filter`, `precursor`, `precursor_tol`, `rt_range`, `mz_range`, `profile`, `aggregate` and `max_points`.
- `nmr-peaks`: NMR peak list and region integrals. Options: `from`, `trace`, `sweep`, `phase`, `lb`, `size`, `baseline`, `min_snr`, `min_prominence`, `min_height_fraction`, `negative`, `range_ppm`, `max_peaks`, `integrate` and `integral_reference`. See [NMR processing](../guides/nmr.md).
- `ephys-features`: patch-clamp features per sweep and per cell. Options: `trace`, `channel`, `sweeps`, `peak_threshold_mv`, `dvdt_threshold` and `max_spikes`. See [Electrophysiology](../guides/ephys.md).
- `spikes`: extracellular spike detection per channel. Options: `trace`, `channels`, `sweeps`, `band_hz`, `threshold`, `sign`, `max_seconds` and `max_times`.
- `qpcr`: Cq and Tm per well and target, ΔΔCq and standard curves. Options: `well`, `target`, `sample`, `run`, `compute_cq`, `threshold`, `baseline_start`, `baseline_end`, `ddcq`, `reference_targets`, `control_sample`, `standard_curve`, `undetermined_cq` and `max_records`.
- `assay`: plate-reader analysis: blanks and replicates, standard curves, IC50/EC50, kinetics, growth and Z′. `analysis` picks one of `wells`, `curve`, `dose-response`, `kinetics`, `growth` and `qc`; `plot: true` adds the fitted curve as an image. The other options are listed in [Plate-reader assays](../guides/plate-analysis.md).
- `gate`: population counts, percentages and medians from a FlowJo workspace or Gating-ML file. Options: `workspace`, `gatingml`, `sample`, `populations`, `table` and `medians`. Without `workspace` or `gatingml`, `file` is the gating file itself, described without counts.

### openreadout_export

Writes a new file, reads it back and verifies it, then renames it into place. The source is never modified. An existing output is replaced only with `overwrite: true`.

- `format`: `ome-tiff`, `ome-zarr`, `mzml`, `asm`, `rdml`, `parquet`, `arrow`, `nwb` or `jcamp`. The default is `mzml` for mass-spectrometry files and `ome-tiff` otherwise. CSV export is available only on the command line.
- `output`: the output path. The default is next to the input.
- Images: `image`, `select`, `level`, `region` and `wells` (OME-Zarr plates).
- Tables, traces and spectra: `table`, `trace`, `sweep`, `rows`, `spectra`, `run` and `centroid`.
- `attachment`: write one embedded attachment, such as a slide label or thumbnail, instead. Names come from `openreadout_info` with `view: "structure"`.

Compression, chunk size, pyramid levels and vendor metadata embedding are command-line options only. The result has no output schema, because its shape depends on the format.

### openreadout_batch

Runs one `measure` over many files and returns one tidy table: `stats`, `trace`, `table`, `info`, `spectra`, `gate`, any `analyze` kind, or `summarize` to regroup a table written earlier. Inputs come from `inputs` (files, directories or globs, with `recursive`) or from an index (`from_index` and `query`); `formats` limits the formats. `options` holds the measure's settings.

`sample_sheets`, `worksheet`, `keys`, `where`, `fields`, `by`, `values`, `replicate`, `exact_by`, `test` and `control` work as the flags of [`batch`](commands/batch.md). `limit` (default 20, at most 500) and `offset` page the rows; `output` writes the whole table to a file. A file that fails becomes a row with an `error`. See [Many files](../guides/batch.md).

### openreadout_link

Groups files that measured the same sample, from their headers. Every link has its evidence and a confidence. Arguments: `paths`, `no_recursive`, `min_confidence` (`low`, `medium` or `high`; weaker links are listed under `weak_links`), `formats`, `from_index` and `query`.

### openreadout_index, openreadout_search and openreadout_watch

`openreadout_index` catalogs every data set under `roots` into Parquet tables in `index_dir`, reading headers only. One call stops after `max_files` (default 20000) or `max_seconds` (default 45) and returns `complete: false`; the same call again continues where it stopped. Other arguments: `check` (`headers`, `full` or `none`), `exclude`, `restart`, `full_rescan`, `pii` and `threads`. It writes only inside `index_dir`.

`openreadout_search` queries an index with the query language of [`search`](commands/search.md), for example `format=nd2 objective=60x`. `total` counts every match; `limit` (default 50, at most 1000) caps the results, and `fields: ["all"]` returns every column.

`openreadout_watch` looks once at `dirs` per call and returns the events since `cursor`: new data sets, planes and scans, completed and stalled data sets, QC findings (with `qc: true`) and errors. Poll with the returned `cursor`. The watcher persists between calls with the same `dirs`. Files are never locked. See [Lab shares, indexes and live acquisitions](../guides/lab-shares.md).

### openreadout_formats

No arguments. Returns every supported format with its read and write support, confidence and known gaps.

## Pictures

`openreadout_preview` lets an agent look at the data. By default it draws image 0 (channel 0, middle z, first time point); for files without images, trace 0, spectrum 0 or the first plate table. Large images are read from the pyramid level nearest `max_size`. The encoded picture is kept under 750 kB: a PNG over that size is sent as JPEG, then halved until it fits, and `notes` says so. The same request always gives the same bytes.

Image previews have rulers labelled in full-resolution pixels, and a µm scale bar when the pixel size is known. An agent can read the coordinates of a feature off the rulers and call again with `region` in the same numbers; only the tiles the region touches are read. The JSON gives the exact mapping from picture pixels to source pixels. Measure intensities with `openreadout_stats`, not from the picture.

`openreadout_info` with `view: "summary"` attaches a smaller picture of the same kind, about 384 px, taken from the smallest pyramid level that is large enough. When that would decode too much data, it adds a note pointing to `openreadout_preview` instead. `thumbnail: false` skips it.

## Annotations

Every tool has a title and the four MCP behaviour hints:

| tools | read-only | destructive | idempotent |
| --- | --- | --- | --- |
| info, preview, stats, trace, table, spectra, analyze, link, search, formats | yes | no | yes |
| watch | yes | no | no |
| index, check | no | no | yes |
| export, batch | no | yes | yes |

`openreadout_export` and `openreadout_batch` are marked destructive because, with `overwrite: true`, they replace an existing output file. Without it they refuse to touch an existing path. `openreadout_check` writes only with `report: true` and `output`. No tool reaches the network (`openWorldHint` is `false` for all).

## Progress

When a request carries a `progressToken`, `openreadout_export` sends progress notifications: `progress` counts the planes or spectra read so far, and `total` the number to read. Verification of the written file follows the last notification. `openreadout_index` sends a notification after each chunk of files, with the number read so far and no `total`.

## Errors

Errors are JSON-RPC errors. Their `data` carries `code`, `exit_code` and `hint`, the same as the command line's [JSON errors](../getting-started/reading-json.md#the-json-wrapper):

```json
{"code": "io", "exit_code": 5, "hint": "Check that the path exists and is readable."}
```

## Resources and prompts

Resources:

- `openreadout://formats`: the `openreadout_formats` JSON.
- `openreadout://file/{path}`: the header-only `openreadout_info` JSON, plus the plain-English explanation of `view: "explain"`.
- `openreadout://preview/{path}`: the default preview as PNG or JPEG, at most 768 px.

`{path}` is an absolute path, percent-encoded or not: `openreadout://file//data/a.czi` and `openreadout://file/%2Fdata%2Fa.czi` are the same file.

Prompts, which clients often show as slash commands:

- `summarize_file` ("Summarize this file"), with `file`: what the file is, what was measured, how and when, with a look at the data.
- `check_file` ("Check this file and explain problems"), with `file`: an integrity check, with each problem explained.
- `convert_to_open_format` ("Convert to an open format"), with `file` and optional `format` and `output`: an export to the open format that fits the data, verified by reading it back.

## Transports

### Standard input and output

`openreadout mcp` with no options serves over stdio. This is what the client configurations in [AI agents](../guides/agents.md) use.

### Streamable HTTP

The HTTP transport is in builds with the `mcp-http` cargo feature only. Release binaries, the Docker image and the `.mcpb` bundles are built without it, so they contain no networking code. A build without the feature answers `--http` with exit code 6 and a hint.

```bash
cargo install openreadout --features mcp-http
openreadout mcp --http 127.0.0.1:8765                     # endpoint http://127.0.0.1:8765/mcp
OPENREADOUT_MCP_TOKEN=$(openssl rand -hex 16) openreadout mcp --http 127.0.0.1:8765
```

Security model:

- **Loopback only by default.** A non-loopback address, such as `0.0.0.0:8765` or a LAN address, is refused unless `--allow-remote` is given.
- **DNS rebinding.** The `Host` header must name `localhost`, `127.0.0.1` or `[::1]` (or the bound address, with `--allow-remote`). Any other host gets 403.
- **Browsers.** Any request with an `Origin` header gets 403, so a web page cannot drive the server, even from the same machine.
- **Token.** When `OPENREADOUT_MCP_TOKEN` is set, every request must carry `Authorization: Bearer <token>`, or it gets 401. The token is compared in constant time. Set one whenever other users or processes on the machine should not reach the server, and always with `--allow-remote`.
- **What a client can do.** The tools read any file the user running the server can read, and `openreadout_export` writes new files next to them. Anyone who can reach the port has that power. There is no TLS; put a TLS-terminating proxy in front for anything beyond loopback.
- Only `/mcp` is served. Sessions are kept in memory and end when the process stops.

## Testing a server

[`oracle/mcp_smoke.py`](https://github.com/openreadout/openreadout/blob/main/oracle/mcp_smoke.py) is a client that uses only the Python standard library. It checks capabilities, every tool's annotations and output schema, resources, prompts, the error model and a call of every tool. With `--http`, it also checks that a browser `Origin`, a foreign `Host` and a missing token are refused. `--synthetic` writes its own small test files, so it needs no other data:

```bash
python3 oracle/mcp_smoke.py target/debug/openreadout crates/openreadout-lif/tests/fixtures/synthetic-dims.lif --synthetic
```
