# EEGLAB datasets (`.set`, `.fdt`)

EEGLAB, the MATLAB toolbox for EEG analysis, saves a dataset as a `.set` file: a MATLAB MAT-file
holding the `EEG` structure. The samples are either inside the `.set` or in a `.fdt` file next
to it. MNE-Python, eeglabio and FieldTrip write the same files, and BIDS accepts them for EEG.
OpenReadout reads level-5 MAT-files (MATLAB's `-v6` and `-v7`, compressed or not, and files
written by SciPy or Octave) and MAT 7.3 files, which are HDF5 and which EEGLAB writes for
datasets over 2 GB.

Derived from the BSD-licensed readers that document both layers (SciPy's MAT-file reader,
MNE-Python's EEGLAB reader, pymatreader) and from the corpus files
(`docs/provenance/eeglab.md`). SciPy and MNE-Python are run as the reference readers.

Crate: `openreadout-ephys`, format id `eeglab` (`EEGLAB_FORMAT_ID`), extensions `set` and `fdt`,
reader `EeglabReader`, dataset `EeglabDataset` (`open`, `open_input`, `eeglab`). Detection: a
`.set` that starts with a MAT-file header (`looks_like_mat`, `mat_version`) → definite; a `.fdt`
with a `.set` of the same stem next to it → likely. Axona `.set` files are text and keep their
own reader.

## The MAT-file (`mat::parse_mat` → `MatFile`)

- **Header**, 128 bytes: 116 bytes of text (`header_text`: `MATLAB 5.0 MAT-file, Platform:
  GLNXA64, Created on: …`; SciPy writes `MAT-file Platform: posix`, without the comma), 8 bytes of
  subsystem offset, the version `0x0100`, and `IM` (little-endian) or `MI` (big-endian,
  `big_endian`, `endian`). `MAT_HEADER_LEN`.
- **Data elements**: an 8-byte tag (type `MiType`, byte count) and the data padded to 8 bytes,
  or a small element (byte count in the tag's upper half, up to 4 bytes of data in its second
  word). Types: `MI_INT8`, `MI_UINT8`, `MI_INT16`, `MI_UINT16`, `MI_INT32`, `MI_UINT32`,
  `MI_SINGLE`, `MI_DOUBLE`, `MI_INT64`, `MI_UINT64`, `MI_MATRIX`, `MI_COMPRESSED`, `MI_UTF8`,
  `MI_UTF16`, `MI_UTF32` (`mi_size` bytes each).
- **Variables** (`MatVariable`: `name`, `value`, `compressed`): `MI_MATRIX` elements, or in
  version-7 files `MI_COMPRESSED` elements whose zlib stream holds one `MI_MATRIX`
  (`compressed` is the stream's place in the file). `variable` finds one by name.
- **Arrays** (`MatValue`): flags (class code, complex bit), dimensions, name, then by class:
  `Numeric` (classes double to uint64 and logical; `class`, `dims`, `data` a `MatNumbers` with
  `stored` type, `count` and either the `values` or only their place `at` in the variable's
  stream: arrays over `MAX_MAT_INLINE` bytes are not read at parse time), `Char` (`text`, UTF-16,
  UTF-8 or Latin-1 characters in column-major order; rows of an m×n array are joined with
  newlines), `Cell` (`items`), `Struct` (`fields`, `values` element by element, `class_name` for
  objects), `Other` (sparse, function handles and opaque objects, not decoded). Helpers: `len`,
  `is_empty`, `as_str`, `as_f64`, `as_f64s`, `field`, `field_names`. Limits: `MAX_MAT_DEPTH`,
  `MAX_MAT_NODES`, `MAX_MAT_ELEMENTS`.
- `read_stream` reads located values later (re-inflating a compressed variable), and
  `decode_values` turns them into f64.

## MAT 7.3 files (`mat73::parse_mat73`)

HDF5 after a 512-byte userblock that starts with `MATLAB 7.3 MAT-file`, read with `hdf5-pure`
(`openreadout-hdf5`'s byte-source adapter). Each variable is a dataset or group with a
`MATLAB_class` attribute. HDF5 shapes are MATLAB's dimensions reversed, so the bytes are in
MATLAB's column-major order, as in level-5 files; characters are `uint16` UTF-16 code units; an
empty array is a dataset of its dimensions with `MATLAB_empty`; a struct is a group (a struct
array's fields are datasets of object references, one per element, into `#refs#`); a cell is a
dataset of object references. They become the same `MatValue` tree (`hdf5` is set on the
`MatFile`). The `data` array is read later by rows along HDF5's first dimension (`read_rows`):
samples, or whole epochs when it has three dimensions (`EeglabData::Hdf5`: `dataset`,
`epochs_are_rows`).

## The EEG structure (`read_set` → `EeglabSet`)

- The fields are those of the variable `EEG` (`single_variable`), or each field is its own
  variable (EEGLAB's newer save, and files written by Python). A study file (`ALLEEG`) is
  refused.
- `nbchan` (`channels`), `pnts` (`points`), `trials` (1 for continuous data), `srate`, `xmin`
  (the time of an epoch's first sample), `setname`, `ref` (`reference`), `chanlocs.labels`
  (`labels`) and `chanlocs.type` (`types`), `icaweights` rows (`ica_components`),
  `etc.eeglabvers` (`eeglab_version`). `mat_writer` names the MAT-file's writer (`MATLAB
  <platform>`, `SciPy`, `Octave`); `compressed` says whether it is version 7.
- `data` (`EeglabData`): a file name (`Fdt`: the `.fdt` next to the `.set`, as named, without
  regard to case, or under the `.set`'s stem; `datfile` wins when set), or the array itself
  (`Embedded`: `variable`, `stored`, `at`, `values`, `class`). An old-style `.dat` file is
  refused.
- `.fdt` samples: little-endian float32, a frame of every channel per sample, epochs one after
  another (`nbchan` × `pnts` × `trials` in column-major order). An embedded array has the same
  order.
- `event` (`EeglabEvent`): `type` as `kind` (numbers as text), `latency` in samples counting from
  1, `duration` in samples (0 without the field, NaN when it is empty), `epoch`.

## Mapping

- One trace, `signals` (continuous) or `epochs`: channels named by `chanlocs.labels` (`ch<i>`
  when there are none), dtype of the stored samples (`float32` for `.fdt`), scale 1, offset 0, no
  unit (EEGLAB records none; its convention is µV); channel `extra.type`. Sweeps are the epochs,
  each `pnts` samples; `start_s` is `xmin` for epoched data and 0 for continuous data.
- Trace `extra`: `mat_version`, `mat_writer`, `application` (`EEGLAB`), `application_version`,
  `sample_storage` (`fdt` or `embedded`), `setname`, `reference`, `ica_components`,
  `epoch_start_s`.
- **Table** `events` (when there are any): `onset_s` = (latency − 1) / srate (after the first
  sample of the first epoch), `duration_s` = duration / srate, `latency`, `duration`, `epoch`, and
  `type` as a code into the column's `extra.categories`.
- `format_version` is `MAT 5.0` or `MAT 7.3` (`mat_version`). `member_files` lists the `.set`
  and the `.fdt`.
- Reads are capped at `MAX_EEGLAB_READ` samples per channel and `MAX_EEGLAB_READ_BYTES` bytes;
  samples inside a compressed `.set` are inflated once, up to `MAX_EEGLAB_INFLATED` bytes.

## `check` finding codes

`truncated` (error); `chanlocs_count`, `event_without_latency`, `extra_bytes`,
`event_outside_data` (warnings).

## Oracle mapping

`oracle/gen.py` `eeglab()`: SciPy's `loadmat` (pymatreader for MAT 7.3) reads the EEG structure,
NumPy the `.fdt` as MNE does; MNE-Python's values must agree with them. Events from SciPy's struct, compared order-free.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/eeglab/` must appear here)

| identifier | meaning |
| --- | --- |
| `EeglabReader`, `EEGLAB_FORMAT_ID`, `EeglabDataset`, `open`, `open_input`, `eeglab` | entry points |
| `EeglabSet`, `read_set`, `single_variable`, `setname`, `channels`, `points`, `trials`, `srate`, `xmin`, `labels`, `types`, `reference`, `data`, `events`, `ica_components`, `eeglab_version`, `findings`, `mat_writer`, `compressed` | the EEG structure |
| `EeglabData`, `Fdt`, `Embedded`, `Hdf5`, `variable`, `stored`, `at`, `values`, `class`, `dataset`, `epochs_are_rows`, `mat_version` | where the samples are |
| `mat73`, `parse_mat73`, `read_rows`, `hdf5` | MAT 7.3 files |
| `EeglabEvent`, `kind`, `latency`, `duration`, `epoch` | an event |
| `MAX_EEGLAB_READ`, `MAX_EEGLAB_READ_BYTES`, `MAX_EEGLAB_INFLATED`, `looks_like_mat` | limits and detection |
| `mat`, `MatFile`, `parse_mat`, `mat_version`, `header_text`, `big_endian`, `endian`, `variables`, `variable`, `MatVariable`, `name`, `value`, `MAT_HEADER_LEN` | the MAT-file |
| `MatValue`, `Numeric`, `Char`, `Cell`, `Struct`, `Other`, `dims`, `text`, `items`, `fields`, `class_name`, `len`, `is_empty`, `as_str`, `as_f64`, `as_f64s`, `field`, `field_names` | MATLAB arrays |
| `MatNumbers`, `count`, `read_stream`, `decode_values`, `MAX_MAT_INLINE`, `MAX_MAT_DEPTH`, `MAX_MAT_NODES`, `MAX_MAT_ELEMENTS` | numeric values |
| `MiType`, `mi_size`, `MI_INT8`, `MI_UINT8`, `MI_INT16`, `MI_UINT16`, `MI_INT32`, `MI_UINT32`, `MI_SINGLE`, `MI_DOUBLE`, `MI_INT64`, `MI_UINT64`, `MI_MATRIX`, `MI_COMPRESSED`, `MI_UTF8`, `MI_UTF16`, `MI_UTF32` | element types |
