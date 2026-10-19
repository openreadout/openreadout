# Provenance log — EEGLAB datasets (`.set`, `.fdt`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- SciPy 1.15 `scipy/io/matlab/_mio5_params.py` (BSD-3-Clause, https://github.com/scipy/scipy, the
  copy installed in `oracle/.venv`): the MAT-file element types (`miINT8` = 1 … `miUTF32` = 18,
  `miMATRIX` = 14, `miCOMPRESSED` = 15), the array classes (`mxCELL_CLASS` = 1, struct 2, object
  3, char 4, sparse 5, double 6 … uint64 15), the 128-byte file header (116 bytes of text, an
  8-byte subsystem offset, a 2-byte version, the 2-byte endian indicator), the full tag (type,
  byte count) and the small-element tag (byte count in the upper 16 bits), and the array-flags
  element (class in the low byte, flags above it).
- MNE-Python 1.13.2 `mne/io/eeglab/eeglab.py` (BSD-3-Clause, https://github.com/mne-tools/mne-python):
  the `EEG` structure as one variable or as one variable per field, `nbchan`, `pnts`, `trials`,
  `srate`, `xmin`, `chanlocs.labels`, `data` as a `.fdt` file name or the array itself, the `.fdt`
  as little-endian float32 in `nbchan` × `pnts` × `trials` column-major order, the `.set`'s own
  stem when the named `.fdt` is missing, `.dat` files refused, and events with latencies counting
  from 1 (onset = (latency − 1) / srate).
- SciPy's `loadmat` and MNE-Python are also run as the oracle (`oracle/gen.py`, `eeglab()`), as
  black boxes.

**What the reader takes from the element layout above:** a struct's body is a field-name length
(`miINT32`), the names (`miINT8`, each padded to that length), then one array per field for each
element in column-major order; a cell's body is one array per element; character data is UTF-16
code units (`miUINT16`) in column-major order for m×n arrays. A numeric array's values are
decoded by the type in their own element's tag, which can be narrower than the array's class
(SciPy's table lists the class and the stored type separately).

## 2026-10-19 — MAT 7.3 (HDF5) datasets (Richard Zimring with Claude as assistant)

**Corpus files used:** `openneuro-ds005628-sub-1-task-edzna-run-1` (samples inside the `.set`)
and `openneuro-ds005048-sub-02-task-40hzauditoryentrainment` (samples in the `.fdt`), OpenNeuro
ds005628 and ds005048 (CC0), both written by MATLAB on PCWIN64.

**Prior art consulted:** `hdf5-pure` 0.47 (MIT OR Apache-2.0, https://github.com/CramBL/hdf5-pure),
its `mat` module's documentation of the MAT 7.3 conventions (the 512-byte userblock,
`MATLAB_class`, reversed shapes, UTF-16 `char`, `#refs#`), and its reading API (`dereference`,
`read_raw_rows`). The datasets' tree was listed with h5py (BSD-3-Clause).

**What was inferred from the files:**
- Both files store the EEG fields as top-level variables. Scalars are (1, 1) float64 datasets,
  text is (n, 1) `uint16` `char`, and empty fields are (2,) `uint64` datasets of their dimensions
  with `MATLAB_empty`.
- `chanlocs` and `event` are groups without their own `MATLAB_class` on the fields: each field is
  a dataset of object references, (1, 8) in one file and (19, 1) in the other, one per element.
- ds005628's `data` is a (15250, 8) float32 dataset: HDF5 rows are samples and the 8 channels are
  contiguous within a row, the same order as a `.fdt`.
- pymatreader (BSD-2-Clause), run as the oracle for these files, reads the same structure.
