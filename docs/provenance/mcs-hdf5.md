# Provenance log — Multi Channel Systems HDF5 (`.h5`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — first reader (Richard Zimring with Claude as assistant)

**Vendor documentation:** "MCS HDF5 Format Definitions", `McsPyDataTools/docs/mcs_hdf5_protocol.rst`
in https://github.com/multichannelsystems/McsPyDataTools (commit 3d2a863, BSD-3-Clause, published
by Multi Channel Systems): the root and `/Data` attributes, recordings, analog streams
(`InfoChannel` fields, `ChannelData` rows by `RowIndex`, `ChannelDataTimeStamps` segments, the
value formula), event, segment, time-stamp and frame streams.

**Prior art consulted:** SpikeInterface `mcsh5extractors.py` (MIT,
https://github.com/SpikeInterface/spikeinterface): stream paths. McsPyDataTools 0.4.3 (BSD-3) is
run as a black box in the oracle (`oracle/gen.py`, `mcs_hdf5()`); its source was not read. The
HDF5 global heap layout (collection header, object headers, variable-length references) is from
the HDF5 file format specification.

**Corpus files used:** `mcspydatatools-w8-standard-all-500hz`,
`zenodo-13121249-singlewell-hipsc-cardio-control`, `zenodo-13121249-mcsrecording-2023-03-09t15-51-00`,
`zenodo-3814727-apr1218-10003`, `zenodo-3814727-apr1218-10004`.

**Inferred from the files:** digital streams store conversion factor 0 or 1 with unit `NoUnit`,
so their stored numbers are kept; the `/Data` date in ticks matches the local time in the file
names (`2014-07-09T10-17-35`), so it is reported without a zone.
