# Provenance log — Multi Channel Systems MC_DataTool binary export (`.raw` with header)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-12 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- Neo 0.14.5 `neo/rawio/rawmcsrawio.py` (BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo,
  the copy installed in `oracle/.venv`): the text header ends at `EOH` and the samples start two
  bytes after it (after `EOH\r\n`); `Sample rate = …`, `ADC zero = …`, `El = <gain><unit>/AD` and
  `Streams = name;name;…`; samples are interleaved uint16 frames of every stream, value =
  raw × gain − gain × ADC zero.
- Neo is also run as the oracle (`oracle/gen.py`, `mcs_raw()`), a black box.
- MC_Rack `.mcd` files are not read: Neo's own notes say no specification is public.

**Corpus file used:** `gin-rawmcs-raw-mcs-with-header-1` (NeuralEnsemble ephy_testing_data on
G-Node GIN, commit bf392aa, CC-BY-SA-4.0; written by MC_DataTool 2.6.10).

**What was inferred from the file:**
- The header's first line is `MC_DataTool binary conversion`, the second `Version 2.6.10`, the
  third names the source `.mcd`; lines end with CR LF; the unit is `µV` in Windows-1252.
- After the header the file holds exactly 100,000 frames of the 60 streams (12,000,000 bytes).
- Every stream name in the file starts with `El_` (electrode channels), which the `El` gain
  covers. Files with other stream kinds are refused, since no corpus file shows how their gains
  are written.

## 2026-10-12 — MCS HDF5 exports: searched, not read

The HDF5 files that Multi Channel DataManager exports (MCS-HDF5 raw-data protocol) were
considered for a reader in `openreadout-hdf5`. Searched: NeuralEnsemble ephy_testing_data on GIN
(no MCS HDF5 files), Zenodo full-text searches (`"Multi Channel DataManager" h5`,
`multichannelsystems hdf5`: no MCS HDF5 deposit), and the test data of McsPyDataTools (BSD-3-Clause, Multi Channel
Systems). The only public file with a known licence was McsPyDataTools'
`2014-07-09T10-17-35W8 Standard all 500 Hz.h5`: protocol version 1, written by Multi Channel
Experimenter 0.9.8.2, while DataManager writes version 3. One file of an older protocol cannot
validate a reader of the current one, so no reader was written. The file's group and attribute
names were listed with h5py; McsPyDataTools' source was not read.
