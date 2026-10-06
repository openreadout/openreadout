# MCP tools

`openreadout mcp` runs OpenReadout as a Model Context Protocol server. It speaks over standard input and output by default, or over HTTP in builds that include it (see [Transports](#transports)). To connect a client, see [AI agents](../guides/agents.md).

The tools are the commands of the command line and return the same JSON as their `--json` output (the `data` part). Each result is sent twice: as `structuredContent`, checked against the tool's output schema, and as one text block with the same JSON for clients that only read text.

When a client connects, the server sends short instructions: indices are zero-based; values are raw as stored unless a tool says it processed them; each file reports an assurance level; only `openreadout_export` writes next to the data; tools and arguments have the command line's names; errors carry a code and a hint.

## Names

A tool is `openreadout_` plus the command, or the `analyze` subcommand, with `-` as `_`: `check` is `openreadout_check` and `analyze nmr-peaks` is `openreadout_nmr_peaks`. Its arguments are the command's long flags with `-` as `_`: `--rt-range` is `rt_range`. Two rules cover the rest:

- A flag that can be repeated is singular on the command line and a plural list in MCP: `--channel 0 --channel 2` is `channels: [0, 2]`.
- A flag that turns off something on by default is `--no-X` on the command line and `X: false` in MCP: `index --no-pii` is `pii: false`.

The command pages list each flag. Flags that only make sense in a terminal or a script (output files of a plot, compression levels, man pages) have no MCP argument; the command pages say which.

## Tools

The server has 29 tools.

| tool | what it does | command |
| --- | --- | --- |
| `openreadout_info` | what a file holds, from its headers | [info](commands/info.md) |
| `openreadout_check` | integrity check | [check](commands/check.md) |
| `openreadout_compare` | compare two files, such as a raw file and its export | [compare](commands/compare.md) |
| `openreadout_report` | a diagnostic bundle for a file OpenReadout cannot read | [report](commands/report-cmd.md) |
| `openreadout_preview` | a picture of the data | [preview](commands/preview.md) |
| `openreadout_stats` | pixel statistics, or per-well statistics of a plate | [stats](commands/stats.md) |
| `openreadout_trace` | samples and statistics of one sweep of a signal or 1-D spectrum | [trace](commands/trace.md) |
| `openreadout_table` | rows of a table | [table](commands/table.md) |
| `openreadout_spectra` | mass-spectrometry scan headers, or one spectrum | [spectra](commands/spectra.md) |
| `openreadout_peaks` | chromatographic peaks; bands and regions of spectra | [analyze peaks](commands/analyze.md#peaks-and-chromatogram) |
| `openreadout_chromatogram` | TIC, BPC, XIC, SRM and detector chromatograms | [analyze chromatogram](commands/analyze.md#peaks-and-chromatogram) |
| `openreadout_nmr_peaks` | NMR peaks and integrals | [analyze nmr-peaks](commands/analyze.md#nmr-peaks) |
| `openreadout_ephys_features` | patch-clamp features | [analyze ephys-features](commands/analyze.md#ephys-features) |
| `openreadout_spikes` | extracellular spikes | [analyze spikes](commands/analyze.md#spikes) |
| `openreadout_qpcr` | qPCR Cq, Tm, ΔΔCq and standard curves | [analyze qpcr](commands/analyze.md#qpcr) |
| `openreadout_gate` | flow-cytometry gating | [analyze gate](commands/analyze.md#gate) |
| `openreadout_assay_wells` | plate-reader wells, blanks and replicates | [analyze assay-wells](commands/analyze.md#plate-reader-assays) |
| `openreadout_assay_curve` | a plate-reader standard curve | [analyze assay-curve](commands/analyze.md#plate-reader-assays) |
| `openreadout_dose_response` | IC50/EC50 from a plate | [analyze dose-response](commands/analyze.md#plate-reader-assays) |
| `openreadout_kinetics` | per-well rates of a kinetic read | [analyze kinetics](commands/analyze.md#plate-reader-assays) |
| `openreadout_growth` | growth rate and doubling time per well | [analyze growth](commands/analyze.md#plate-reader-assays) |
| `openreadout_assay_qc` | Z′ and other plate quality metrics | [analyze assay-qc](commands/analyze.md#plate-reader-assays) |
| `openreadout_export` | export to an open format | [export](commands/export.md) |
| `openreadout_batch` | one measure over many files as one table | [batch](commands/batch.md) |
| `openreadout_link` | group files of the same sample | [link](commands/link.md) |
| `openreadout_index` | catalog every data set under directories | [index](commands/index-cmd.md) |
| `openreadout_search` | query an index | [search](commands/search.md) |
| `openreadout_watch` | follow directories an instrument writes to | [watch](commands/watch.md) |
| `openreadout_formats` | supported formats and their known gaps | [self formats](commands/self.md) |

Every tool that reads a file takes `file`: an absolute path, or a path relative to the server's working directory. It can also be a data-set directory, such as a Bruker experiment or an Agilent `.D` folder. The output schemas are listed by the server and described in the [JSON output reference](json/index.md).

### Strict mode

The tools that read values from a file take `strict: true`. A strict call refuses values the file's assurance does not validate, with an error whose `exit_code` is 6. Start the server with `OPENREADOUT_STRICT=1` (or `openreadout --strict mcp`) to make every call strict. See [Assurance and strict mode](assurance.md).

### openreadout_info

Arguments:

- `file`
- `view`: `summary` (default), `full`, `structure`, `explain` or `format`, as in `info --view`.
- `ask`: a question in plain words, answered under `answers` with the fields each answer came from. Implies `view: "explain"`.
- `max_images`: list at most this many images (0 = all).
- `thumbnail`: with `view: "summary"`, attach a small picture of image 0 (default `true`). See [Pictures](#pictures). MCP only.
- `vendor`: with `view: "full"`, include the vendor's metadata tree (default `false`; it can be megabytes).
- `max_frames`: with `view: "full"`, per-frame records per image (default 100; 0 for none, -1 for all).

A file still being written gets the same `acquisition` block as on the command line.

### openreadout_check, openreadout_compare and openreadout_report

`openreadout_check` returns the integrity report: `ok` and a list of findings. `headers_only: true` checks the structure without decoding data.

`openreadout_compare` compares `file` with `against` (for example its export): metadata differences, geometry, channel names and per-plane hashes. `image`, `select`, `level`, `tolerance`, `ignore` and `no_pixels` work as in [`compare`](commands/compare.md). `identical` says whether the files hold the same data.

`openreadout_report` builds the privacy-reviewed diagnostic bundle of [`report`](commands/report-cmd.md), for a file that fails or is not validated. It contains no data values, and free text only with `include_text: true`. It is written to a file only when `output` is given.

### openreadout_preview

Returns a picture as image content, followed by JSON that says what was drawn. Arguments as in [`preview`](commands/preview.md): `image`, `select`, `mip`, `composite`, `level`, `region` (`{x, y, width, height}`), `max_size` (default 768, at most 2048), `axes` (`rulers`, `grid` or `none`; default `rulers`), `contrast`, `lut` and `format`; for traces `trace`, `sweep` and `channels`; for spectra `run`, `spectrum`, `scan` and `centroid`; for plates `table` and `column`.

### openreadout_stats

Per channel and image (the default), with every plane (`per: "plane"`), or per well or field of a screening plate (`per: "well"` or `"field"`, with `wells` to choose wells). Other arguments: `image`, `select`, `level`, `region`, `bins` (default 32), `scale` (`linear` or `log`) and `mip` (`z` or `t`).

### openreadout_trace

One window of one sweep, in physical units, with per-channel statistics (including `argmax_axis_value`, the position of the maximum on the trace's axis) and the first `max_samples` values (default 200, at most 10000). Arguments: `trace`, `sweep`, `channels`, `first_sample` and `count`, or `x_range` on the trace's own axis; `process: true` turns an NMR FID into a spectrum.

### openreadout_table

Rows of a table: `table`, `first_row`, `max_rows` (default about 5000 values, at most 10000 rows). For FCS files, `compensate`, `transform`, `parameters`, `workspace`, `gatingml`, `sample` and `populations` work as in [`table`](commands/table.md). `filter` takes conditions such as `"FITC-A > 1000"`; with `count: true`, only the number of matching rows is returned.

### openreadout_spectra

Mass spectrometry only: IR, Raman, UV-Vis and NMR spectra are traces (`openreadout_trace`). Without `scan`, `spectrum` or `nth`, lists the scan headers of a run without decoding peaks, filtered by `ms_level`, `polarity`, `rt_range` (minutes), `precursor` (within `precursor_tol`, default 0.01 m/z, or `precursor_ppm`), `charge`, `activation` and `scan_filter`. Every match is counted; `limit` (default 100, at most 5000) and `offset` page the list, and `count: true` lists none. With `scan`, `spectrum`, or `ms_level` and `nth`, returns one spectrum's `mz` and `intensity`, at most `max_points` (default 2000); `centroid: true` returns the stored centroid list.

### The analysis tools

Each `analyze` subcommand is a tool whose arguments are its flags. An argument the analysis does not take is an error that names the ones it does. `openreadout_batch` runs the same analyses over many files with the same arguments as `options`.

- `openreadout_peaks`: chromatographic peaks with areas, widths, tailing, plates, resolution and S/N; compound lists; bands and regions of IR, Raman, UV-Vis and NMR spectra. Arguments: those of `openreadout_chromatogram` to choose the signal, plus `smooth`, `min_snr`, `min_height`, `min_width`, `baseline`, `area_seconds`, `rt`, `window`, `pick`, `integrate`, `x_range`, `compounds` and `summary_only`. See [Chromatograms and peaks](../guides/quantitation.md).
- `openreadout_chromatogram`: TIC, BPC, XIC and SRM chromatograms, or stored detector traces. Arguments: `tic`, `bpc`, `mz`, `ppm`, `da`, `transitions`, `transition_tol`, `traces`, `channel`, `sweep`, `run`, `ms_level`, `polarity`, `scan_filter`, `precursor`, `precursor_tol`, `rt_range`, `mz_range`, `profile`, `aggregate` and `max_points`.
- `openreadout_nmr_peaks`: NMR peak list and region integrals. Arguments: `from`, `trace`, `sweep`, `phase`, `lb`, `size`, `baseline`, `min_snr`, `min_prominence`, `min_height_fraction`, `negative`, `range_ppm`, `max_peaks`, `integrate` and `integral_reference`. See [NMR processing](../guides/nmr.md).
- `openreadout_ephys_features`: patch-clamp features per sweep and per cell. Arguments: `trace`, `channel`, `sweeps`, `peak_threshold_mv`, `dvdt_threshold` and `max_spikes`. See [Electrophysiology](../guides/ephys.md).
- `openreadout_spikes`: extracellular spike detection per channel. Arguments: `trace`, `channels`, `sweeps`, `band_hz`, `threshold`, `sign`, `max_seconds` and `max_times`.
- `openreadout_qpcr`: Cq and Tm per well and target, ΔΔCq and standard curves. Arguments: `well`, `target`, `sample`, `run`, `compute_cq`, `threshold`, `baseline_start`, `baseline_end`, `ddcq`, `reference_targets`, `control_sample`, `standard_curve`, `undetermined_cq` and `max_records`.
- `openreadout_gate`: population counts, percentages and medians from a FlowJo workspace or Gating-ML file. Arguments: `workspace`, `gatingml`, `sample`, `populations`, `table` and `medians`. Without `workspace` or `gatingml`, `file` is the gating file itself, described without counts.
- The plate-reader tools `openreadout_assay_wells`, `openreadout_assay_curve`, `openreadout_dose_response`, `openreadout_kinetics`, `openreadout_growth` and `openreadout_assay_qc` take `layout`, `blank_wells`, `positive_wells` and `negative_wells`, the arguments of their analysis (`reduce`, `window`, `normalize`; `model`, `weighting`, `confidence`, `plot`; `standards`, `fit_on`, `lloq`, `uloq`; `wells`, `growth_threshold`), and the rarely needed plate options in `plate_options`: `table`, `read`, `wavelength_nm`, `embedded_layout`, `layout_text`, `empty_wells`, `roles`, `blank_subtraction`, `outliers`, `outlier_threshold` and `exclude_outliers`. `plot: true` adds the fitted curve as an image. See [Plate-reader assays](../guides/plate-analysis.md).

### openreadout_export

Writes a new file, reads it back to check it, then gives it its final name. It doesn't modify the source. An existing output is replaced only with `overwrite: true`.

- `format`: `ome-tiff`, `ome-zarr`, `mzml`, `csv`, `asm`, `rdml`, `parquet`, `arrow`, `nwb` or `jcamp`. The default is `mzml` for mass spectra, `csv` for tables and traces, and `ome-tiff` for images, as on the command line.
- `output`: the output path. The default is next to the input.
- Images: `image`, `select`, `level`, `region` and `wells` (OME-Zarr plates).
- Tables, traces and spectra: `table`, `trace`, `sweep`, `rows`, `labels` (CSV), `spectra`, `run` and `centroid`.
- `attachment`: write one embedded attachment, such as a slide label or thumbnail, instead. Names come from `openreadout_info` with `view: "structure"`.

Compression, chunk size, pyramid levels and vendor metadata embedding are command-line options only. The result has no output schema, because its shape depends on the format.

### openreadout_batch

Runs one `measure` over many files and returns one tidy table: `stats`, `trace`, `table`, `info`, `spectra`, `gate`, any analysis (`peaks`, `chromatogram`, `nmr-peaks`, `ephys-features`, `spikes`, `qpcr`, or `assay` with `analysis`), or `summarize` to regroup a table written earlier. Inputs come from `paths` (files, directories or globs, with `recursive`) or from an index (`from_index` and `query`); `formats` limits the formats. `options` holds the measure's settings: for an analysis, the arguments of its tool.

`sample_sheets`, `worksheet`, `keys`, `where`, `fields`, `by`, `values`, `replicate`, `exact_by`, `test` and `control` work as the flags of [`batch`](commands/batch.md). `limit` (default 20, at most 500) and `offset` page the rows; `output` writes the whole table to a file. A file that fails becomes a row with an `error`. See [Many files](../guides/batch.md).

### openreadout_link

Groups files that measured the same sample, from their headers. Every link has its evidence and a confidence. Arguments: `paths`, `no_recursive`, `min_confidence` (`low`, `medium` or `high`; weaker links are listed under `weak_links`), `formats`, `from_index` and `query`.

### openreadout_index, openreadout_search and openreadout_watch

`openreadout_index` catalogs every data set under `roots` into Parquet tables in `index_dir`, reading headers only. One call stops after `max_files` (default 20000) or `max_seconds` (default 45) and returns `complete: false`; the same call again continues where it stopped. Other arguments: `check` (`headers`, `full` or `none`), `exclude`, `restart`, `full_rescan`, `pii` and `threads`. It writes only inside `index_dir`.

`openreadout_search` queries an index with the query language of [`search`](commands/search.md), for example `format=nd2 objective=60x`. `total` counts every match; `limit` (default 50, at most 1000) caps the results, and `fields: ["all"]` returns every column.

`openreadout_watch` looks once at `dirs` per call and returns the events since `cursor`: new data sets, planes and scans, completed and stalled data sets, QC findings (with `qc: true`) and errors. `stall_after` sets the seconds without growth before a data set counts as stalled. Poll with the returned `cursor`. The watcher persists between calls with the same `dirs`. It doesn't lock files. See [Lab shares, indexes and live acquisitions](../guides/lab-shares.md).

### openreadout_formats

No arguments. Returns every supported format with its read and write support, confidence and known gaps.

## Pictures

`openreadout_preview` lets an agent look at the data. By default it draws image 0 (channel 0, middle z, first time point); for files without images, trace 0, spectrum 0 or the first plate table. Large images are read from the pyramid level nearest `max_size`. The encoded picture is kept under 750 kB: a PNG over that size is sent as JPEG, then halved until it fits, and `notes` says so. The same request always gives the same bytes.

Image previews have rulers labelled in full-resolution pixels, and a µm scale bar when the pixel size is known. An agent can read the coordinates of a feature off the rulers and call again with `region` in the same numbers. OpenReadout reads only the tiles in that region. The JSON gives the exact mapping from picture pixels to source pixels. Measure intensities with `openreadout_stats`, not from the picture.

`openreadout_info` with `view: "summary"` attaches a smaller picture of the same kind, about 384 px, taken from the smallest pyramid level that is large enough. When that would decode too much data, it adds a note pointing to `openreadout_preview` instead. `thumbnail: false` skips it.

## The viewer

In clients that support [MCP Apps](https://modelcontextprotocol.io/extensions/apps/overview), the data shows up in the chat. When the assistant calls `openreadout_info`, `openreadout_preview`, `openreadout_stats`, `openreadout_trace`, `openreadout_spectra`, `openreadout_table` or one of the analysis tools, the client opens the OpenReadout viewer next to the result. What it shows depends on the file:

| data | view | what you can do |
| --- | --- | --- |
| images | the image plane | pick the image, channel or composite, z, time point, pyramid level and contrast; drag a box to zoom into full-resolution pixels; hover for coordinates in px and µm |
| electrophysiology, detector traces, 1-D spectra | sweeps, one panel per channel | step through sweeps, choose channels, drag to zoom into the samples |
| NMR | the spectrum on a ppm axis (an FID is processed first) with its peaks | drag to zoom |
| mass spectrometry, chromatography | the TIC or a detector trace, with peaks | type m/z values for extracted-ion chromatograms, click a point to see the spectrum there, step through scans |
| plates | a heat map of the wells | pick the table and value column, hover for a well's value |
| flow cytometry | a density plot of two parameters, or a histogram of one | pick parameters, linear, log or arcsinh scales, and how many events to sample |

The viewer is one HTML page served as the resource `ui://openreadout/viewer.html`. It runs in the client's sandbox, loads nothing from the network and asks the server for data with the tool `openreadout_view`. That tool is only for the viewer: clients that support MCP Apps hide it from the assistant. Its results are kept small. Pictures are at most 1600 px on their longest side and 750 kB. Plots have at most 4000 points per series, and longer signals are drawn as the minimum and maximum of each slice, so single-sample spikes stay visible. Flow-cytometry plots sample at most 50,000 events. Each result says what was reduced.

The server offers the viewer only to clients that declare the extension `io.modelcontextprotocol/ui` when they connect. Other clients see the 29 tools exactly as before. Set `OPENREADOUT_MCP_APPS=off` to turn the viewer off, or `on` to offer it to a client that supports MCP Apps without declaring it.

The viewer also tells the client what you are looking at (for example "image 0, channel 1, z 12, region x 400–800"), so you can ask the assistant about it.

In apps that open files with an MCP App (the ChatGPT and Codex desktop apps), the viewer is also the file viewer for `.czi`, `.nd2`, `.lif`, `.lof`, `.oir`, `.oib`, `.oif`, `.vsi`, `.ims`, `.zvi`, `.mrxs`, `.ndpi`, `.svs`, `.dm3`, `.dm4`, `.fcs`, `.abf`, `.smr`, `.smrx`, `.wcp`, `.wiff`, `.wiff2` and `.asyr` files: opening one in a thread shows it in the viewer. OpenReadout claims only extensions that belong to one instrument format, not general ones such as `.tif`, `.csv` or `.raw`.

## Annotations

Every tool has a title and the four MCP behaviour hints:

| tools | read-only | destructive | idempotent |
| --- | --- | --- | --- |
| info, check, compare, preview, stats, trace, table, spectra, the analysis tools, link, search, formats | yes | no | yes |
| watch | yes | no | no |
| index, report | no | no | yes |
| export, batch | no | yes | yes |

`openreadout_export` and `openreadout_batch` are marked destructive because, with `overwrite: true`, they replace an existing output file. Without it they refuse to touch an existing path. `openreadout_report` writes only when `output` is given. None of the tools reach the network (`openWorldHint` is `false` for all).

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
- **What a client can do.** The tools read any file the user running the server can read, and `openreadout_export` writes new files next to them. So can anyone who can reach the port. There is no TLS; put a TLS-terminating proxy in front for anything beyond loopback.
- Only `/mcp` is served. Sessions are kept in memory and end when the process stops.

## Testing a server

[`oracle/mcp_smoke.py`](https://github.com/openreadout/openreadout/blob/main/oracle/mcp_smoke.py) is a client that uses only the Python standard library. It checks capabilities, every tool's annotations and output schema, resources, prompts, the error model and a call of every tool. With `--http`, it also checks that a browser `Origin`, a foreign `Host` and a missing token are refused. `--synthetic` writes its own small test files, so it needs no other data:

```bash
python3 oracle/mcp_smoke.py target/debug/openreadout crates/openreadout-lif/tests/fixtures/synthetic-dims.lif --synthetic
```
