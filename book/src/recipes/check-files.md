# Is this file intact?

Run this when a file came off the microscope computer, a USB disk or a network copy and you want to know that it is complete before you delete the original or start an analysis. `check` validates the file's structure and exits 4 if it is corrupt or truncated.

## Run it

```text
$ openreadout check mini.nd2
mini.nd2 (nd2): OK
  info     frames_checked     2 frame chunks verified
  assurance: validated - every variant feature of this file was read correctly in ...
  checks performed:
    - file signature chunk and version string
    - chunk map located from the trailing offset and parsed (or rebuilt by scanning)
    - every chunk in the map has a valid header with the same name at that offset
    - every frame chunk declared by uiSequenceCount exists and holds a full frame
    - loop-tree frame count equals uiSequenceCount
```

`mini.nd2` is a small Nikon file in the repository at [`crates/openreadout-cli/tests/fixtures/mini.nd2`](../../../crates/openreadout-cli/tests/fixtures/mini.nd2). The output on this page is real, with long lines trimmed.

Here is the same file cut off after 1200 bytes (`head -c 1200 mini.nd2 > cut.nd2`), as happens when a copy is interrupted. `--live-window 0` is needed only because the cut file is brand new (see below):

```text
$ openreadout check cut.nd2 --live-window 0; echo "exit=$?"
cut.nd2 (nd2): PROBLEMS FOUND
  error    truncated          the chunk map that ends every ND2 is missing or unusable; ...
  warning  structure          trailing chunk-map offset is missing or out of range @1192
  warning  structure          chunk map unusable; recovered 5 chunks by scanning (interrupted ...
  error    truncated          chunk 'ImageDataSeq|0!' extends past end of file @1155
  error    missing_planes     0 of 2 frames present
  info     frames_checked     0 frame chunks verified
  assurance: UNVALIDATED - damaged container index: present but not decoded; ...
  ...
exit=4
```

## What it tells you

- The first line is the verdict: `OK` or `PROBLEMS FOUND`, with the format in brackets.
- Each finding has a severity (`error`, `warning` or `info`), a code such as `truncated` or `missing_planes`, and a message. `@1155` is the byte offset where the problem was found.
- `checks performed` lists what was verified for this format, so you know what `OK` covers.
- The exit code is what scripts should test: 0 for a sound file, 4 for a corrupt, truncated or empty one, 3 for a file that is not a known instrument format, 5 for a file that cannot be read. All codes are in [Commands](../reference/commands/index.md#exit-codes).
- A file changed in the last five minutes may still be in the middle of an acquisition. On OME-TIFF, OME-Zarr, ND2 and CZI, `check` then reports `acquisition_in_progress` warnings and exits 0. Without `--live-window 0`, the freshly cut file above gives exactly that. `--live-window 0` turns it off.

## Variations

### A folder

Give a directory, and `-r` to walk its sub-directories. Each file gets its report, followed by a summary table. The exit code is the worst code of any file.

```text
$ openreadout check run42; echo "exit=$?"
...
path            format  contents    status
──────────────  ──────  ──────────  ───────────
run42/cut.nd2   nd2     6 findings  corrupt (4)
run42/mini.czi  czi     1 findings  ok
run42/mini.lif  lif     intact      ok
run42/mini.nd2  nd2     1 findings  ok
4 inputs: 3 ok, 1 failed
exit=4
```

For scripts, write one JSON line per file:

```bash
openreadout check -r --jsonl /data/run42 > report.jsonl
```

Each line is a JSON wrapper with the file's `path`; the verdict is `data.ok`.

### After an export

`compare` reads both files and answers "is this export the same data as the source?":

```text
$ openreadout compare mini.nd2 mini.ome.tiff; echo "exit=$?"
mini.nd2 (nd2)
mini.ome.tiff (tiff)
=> identical
metadata: same (0 differences)
image 0: geometry same, channel names same, physical size same
planes: 2 compared, 2 identical, 0 within tolerance, 0 mismatched
exit=0
```

It exits 0 when the files are identical and 1 when they differ. See [Convert to an open format](export.md).

### A file OpenReadout cannot read

`report` writes a diagnostic bundle you can attach to a bug report without sharing the file. It holds the structure map, every decode stage and its error, and the metadata's numbers, but no pixel, trace or table values, no path and no free text:

```text
$ openreadout report cut.nd2
openreadout 0.1.0 report (bundle version 1)
  wrote openreadout-report-8cd292920dc7.json (17526 bytes). Nothing was sent anywhere.
  privacy: structure only (19 strings from the file replaced)
  ...
    first_read  error   [corrupt_file] nd2: corrupt file: frame chunk ImageDataSeq|0! declares ...
```

Add `--dry-run` to print the bundle and write nothing.

### From an assistant

The MCP tool is `openreadout_check`, with `file`. It returns the same `ok` and findings. With `against` it compares two files, and with `report: true` it builds the bundle.

## More

- [`check` reference](../reference/commands/check.md): plane hashes, tolerances and every flag.
- [Lab shares, indexes and live acquisitions](../guides/lab-shares.md#files-still-being-written): files that are still being written.
- JSON: [`check`](../reference/json/check.md), [`compare`](../reference/json/compare.md), [`report`](../reference/json/report.md).
