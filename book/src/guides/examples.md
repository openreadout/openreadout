# Examples

Runnable scripts are in [`examples/`](https://github.com/openreadout/openreadout/tree/main/examples). Continuous integration runs the shell scripts and the MCP client against a set of public test files, so they keep working. The descriptions below are the scripts' own header comments, included from the source.

Shell scripts (need `openreadout` and `jq`):

- `batch_summarize.sh`: `info --json` over a directory, as a tab-separated table.
- `triage_corruption.sh`: sorts files into OK, corrupt and unsupported by the exit code of `check`.
- `export_planes.sh`: exports single planes for a quick look, with `export --image --select`.

Python scripts (need the [Python package](python.md)):

- `read_planes.py`: NumPy planes and a maximum projection with the `openreadout` package.
- `bioio_read.py`: the same file through bioio, with the plugin or through an OME-TIFF export.

MCP:

- `mcp_client.py`: a small MCP client over stdio that uses only the Python standard library.

## Shell

### batch_summarize.sh

```bash
{{#include ../../../examples/shell/batch_summarize.sh:2:13}}
```

### triage_corruption.sh

```bash
{{#include ../../../examples/shell/triage_corruption.sh:2:13}}
```

### export_planes.sh

```bash
{{#include ../../../examples/shell/export_planes.sh:2:13}}
```

## Python

### read_planes.py

```python
{{#include ../../../examples/python/read_planes.py:2:12}}
```

### bioio_read.py

```python
{{#include ../../../examples/python/bioio_read.py:2:14}}
```

## MCP

### mcp_client.py

```python
{{#include ../../../examples/mcp/mcp_client.py:2:15}}
```

Its output on `mini.nd2`, a small test file in the repository (tool descriptions are cut at 70 characters):

```text
$ python examples/mcp/mcp_client.py mini.nd2
server: openreadout 0.1.0, protocol 2025-11-25
tools:
  openreadout_analyze      An analysis with a documented method, picked by kind. options holds th
  openreadout_batch        One measure over many files as one tidy table, optionally joined to sa
  openreadout_check        Integrity check: ok plus findings (severity, code, message) for trunca
  openreadout_export       Convert to an open format: a new file, read back and verified; the sou
  openreadout_formats      Supported formats with read/write support, confidence and known gaps (
  openreadout_index        Catalog every data set under roots into index_dir (Parquet: one row pe
  openreadout_info         What an instrument file or data-set directory holds, from its headers
  openreadout_link         Groups files that measured the same sample across instruments and form
  openreadout_preview      A picture of the data as image content plus what was drawn: an image p
  openreadout_search       Query an index from openreadout_index: 'format=nd2 objective=60x', 'ch
  openreadout_spectra      Mass spectra of an MS run. Without scan, index or nth: the scan header
  openreadout_stats        Pixel statistics per channel and image: count, min, max, mean, std, pe
  openreadout_table        Rows of a table {columns, labels, rows, total_rows, truncated}: FCS ev
  openreadout_trace        One window of one sweep of a sampled signal or 1-D spectrum in physica
  openreadout_watch        Follow directories an instrument writes to: each call looks once and r

openreadout_info -> Nikon ND2, 1 image(s), 2 plane(s)
  [0] 8x8 z=1 c=1 t=2 uint16

openreadout_check -> ok=True, 0 error finding(s)

expected failure -> JSON-RPC error -32603: I/O error on /definitely/not/here.czi: No such file or directory (os error 2); data={"code": "io", "exit_code": 5, "hint": "Check that the path exists and is readable."}

openreadout_search output schema -> 8 top-level properties
server exited with 0
```

The tools are described in [MCP tools](../reference/mcp.md).
