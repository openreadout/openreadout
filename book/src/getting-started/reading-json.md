# Reading the JSON output

Add `--json` to a command and it prints one JSON document on standard output. This page explains the wrapper around every result, walks through one result field by field, and shows how to use it from the shell. The examples use `mini.nd2` from [Your first file](first-file.md).

## The JSON wrapper

Every JSON result has the same outer object. These docs call it the *envelope*:

```json
{
  "ok": true,
  "schema_version": "1",
  "tool": { "name": "openreadout", "version": "0.1.0" },
  "data": { "...": "the command's result" }
}
```

- `ok` says whether the command ran.
- `schema_version` is the version of the output format (see [Compatibility](#compatibility)).
- `tool` names the program and its version.
- `data` holds the command's result. Its shape depends on the command.

When the command fails, `ok` is `false`, there is no `data`, and `error` says what went wrong and what to do about it:

```json
{
  "ok": false,
  "schema_version": "1",
  "tool": { "name": "openreadout", "version": "0.1.0" },
  "error": {
    "code": "unknown_format",
    "message": "unrecognized file format: notes.txt",
    "hint": "Run `openreadout self formats` to list supported formats; the file may be a format not yet implemented, or a renamed export.",
    "exit_code": 3
  }
}
```

- `code` is a stable identifier. Branch on it, not on `message`.
- `message` describes the problem for a person.
- `hint` says what to try next. Most errors have one.
- `exit_code` equals the process exit code. The codes are listed in [Commands](../reference/commands/index.md#exit-codes).

There is no separate warnings list in the envelope. Each result carries its own warnings in `data`, usually as `notes` (things the reader wants you to know) or, for `check`, as `findings` with a severity.

Note the two different "ok"s in `check`. The envelope's `ok` says whether the command ran. `data.ok` says whether the file is intact. A truncated file gives `{"ok": true, "data": {"ok": false, "findings": [...]}}` and exit code 4.

### Several files

When a command gets several inputs (several paths, a directory or a glob), `--jsonl` prints one compact envelope per line, and `--json` prints a JSON array of envelopes. Each envelope then has a `path` naming its input, on success and on error:

```json
{"ok":true,"schema_version":"1","tool":{...},"path":"mini.nd2","data":{...}}
{"ok":false,"schema_version":"1","tool":{...},"path":"notes.txt","error":{"code":"unknown_format",...,"exit_code":3}}
```

### Only some values

`--only` keeps only the values you name, as JSON pointers. `*` stands for every element of a list. Pointers that match nothing are listed under `missing`:

```text
$ openreadout info mini.nd2 --only '/images/0/physical_size,/images/*/channels/0/name,/nope' --compact
{"ok":true,...,"data":{"/images/0/physical_size":{"x":0.25,"y":0.25,"unit":"µm"},"/images/*/channels/0/name":["DAPI"],"/nope":null,"missing":["/nope"]}}
```

## One image, field by field

`info --json` returns the file's format, its images (or tables, traces and spectra), an `experiment` summary and an `assurance` block. Here is the first image of `mini.nd2`, without its `extra` object:

```json
{
  "index": 0,
  "size_x": 8,
  "size_y": 8,
  "size_z": 1,
  "size_c": 1,
  "size_t": 2,
  "dimension_order": "XYCZT",
  "pixel_type": "uint16",
  "samples_per_pixel": 1,
  "physical_size": { "x": 0.25, "y": 0.25, "unit": "µm" },
  "time_increment_s": 0.1,
  "channels": [
    { "index": 0, "name": "DAPI", "acquisition_mode": "Widefield Fluorescence" }
  ],
  "objective": { "model": "Plan Fluor 10x", "nominal_magnification": 10.0 },
  "instrument": { "manufacturer": "Nikon", "software": "NIS-Elements" },
  "pyramid_levels": 1,
  "plane_count": 2
}
```

- `size_x` and `size_y` are in pixels. `size_z`, `size_c` and `size_t` are counts, and `plane_count` is their product.
- `pixel_type` uses the OME-XML names (`uint8`, `uint16`, `float` and so on). `samples_per_pixel` is 3 for RGB images.
- `physical_size` is the pixel size; `unit` gives the unit. The number is what the file recorded. Round it for display, not for computing.
- `time_increment_s` is the time between time points, in seconds. Times are in seconds unless the field name says otherwise (for example `_ms`).
- If the file doesn't record a value, the key is left out rather than set to `0` or `""`. Test whether a key is present, not whether it is truthy. One exception: a mass spectrum's `rt_s` (retention time) is `null` when the file gives no time for that scan.
- `extra` holds format-specific values, such as the ND2 loop structure or the CZI compression.

The `experiment` object describes the measurement in instrument-independent terms: the sample, the instrument, the method and its parameters with units, and a sentence saying what was measured. For `mini.nd2`, `experiment.measurements[0].what` is "fluorescence, 1 channel (DAPI), 2 time points every 100 ms, 8 × 8 px at 0.25 µm/px". See [Metadata conventions and the experiment model](../guides/metadata.md).

The `assurance` object says whether files like this one were confirmed against an independent reader during development. See [Assurance and strict mode](../reference/assurance.md).

The [JSON output reference](../reference/json/index.md) lists each field with its type and meaning. `openreadout self schema <command>` prints the same schemas.

## Provenance: where a field's meaning came from

`info --view full --json` adds two objects: `vendor`, the vendor's own metadata tree with the vendor's names, and `provenance`, which maps each normalized field to the source of its meaning:

```json
"provenance": {
  "images[].acquired_at": "prior-art",
  "images[].physical_size": "inferred",
  "images[].pixel_type": "prior-art",
  "images[].size_c": "inferred"
}
```

The four values are:

- `spec`: a published specification or open standard.
- `vendor-impl`: the vendor's own published implementation or documentation.
- `prior-art`: the documentation of a permissively licensed community reader.
- `inferred`: our own comparison of many example files.

`inferred` doesn't mean guessed: it means no one outside this project has written the rule down. Inferred fields are tested against independent readers like the rest. The notes for each format in [`docs/formats`](https://github.com/openreadout/openreadout/tree/main/docs/formats) say how each field was derived.

## With jq

```bash
# One line per image
openreadout info mini.nd2 --json \
  | jq -c '.data.images[] | {index, size_x, size_y, size_t, px: .physical_size.x, channels: [.channels[].name]}'
# {"index":0,"size_x":8,"size_y":8,"size_t":2,"px":0.25,"channels":["DAPI"]}

# Only the errors that check found
openreadout check cut.nd2 --json \
  | jq -r '.data.findings[] | select(.severity == "error") | "\(.code): \(.message)"'

# The format id, or the error code when the file cannot be read
openreadout info maybe.bin --json | jq -r 'if .ok then .data.format.id else .error.code end'
```

## Compatibility

- `schema_version` changes only when an existing field is renamed or removed, or changes type.
- New fields can appear in any release. Ignore keys you do not know.
- Key order is stable, so the output of two runs can be compared with `diff`.
