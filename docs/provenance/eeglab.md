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
