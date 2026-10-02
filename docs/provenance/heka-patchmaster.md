# Provenance log — HEKA PatchMaster bundle files

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Vendor documentation consulted (public).** HEKA publishes the PatchMaster file-format
descriptions for third-party readers on its public server (`ftp://server.hekahome.de/`, see the
PatchMaster manual, https://www.heka.com/downloads/software/manual/m_patchmaster.pdf); they are
not behind a login, a licence agreement or an NDA. We read the copies redistributed in
load-heka-python's `spec/` directory (https://github.com/easy-electrophysiology/load-heka-python,
commit `185f6edb54ac562de26aa7ce33643b453c45894d`, repository MIT; the directory's README says the
files are "local copies of HEKA's published file format specifications"):
`FileFormat_2x90/FileFormat.txt` (the "Tree" container: magic `Tree`/`eerT`, level count, level
sizes, records top-down with a child count after each record, "never assume you know the record
sizes"; where trace samples live and that they are stored leak-subtracted but not zero-subtracted),
`DataFile_v9.txt` (the bundle header: signature `DAT1`/`DAT2`, version text, time, item count,
endian flag, twelve 16-byte items with start, length and extension; interleaved trace storage;
non-stored segments), `PulsedFile_v9.txt` (record offsets of the Root, Group, Series, Sweep and
Trace records; the data-kind bits; the recording-mode and data-format enumerations),
`TimeFormat.txt` (stored times are seconds with a documented conversion to calendar dates), and the
v1000 / v2000 variants of `PulsedFile` and `DataFile` (PatchMaster 2x90.4 and PatchMaster Next
1.6: larger Sweep and Series records; v2000 moves to a 352-byte bundle header with 64-bit item and
trace offsets).

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- load-heka-python, MIT, commit `185f6ed` (`load_heka_python/load_heka.py`,
  `readers/data_reader.py`): which version strings map to which record layouts, how interleaved
  traces are gathered, that int16/int32 samples are multiplied by the trace's data scaler and
  float32 samples are not, and that the reader asserts a zero Y offset and zero leak conductance
  (it has not met others either).
- load-heka-python is also run as a black-box oracle (`oracle/gen.py`, `heka()`).

**Corpus files used** (`corpus/manifest.toml`): `zenodo3827171-w2019-07-08b` (PatchMaster v2x90.2,
CC-BY-4.0), `zenodo4311847-feb0821c` (v2x65, CC-BY-4.0), `zenodo7530512-2018-12-21c4` (v2x60,
CC-BY-4.0), `hekareader-180514s1c1r1` (v2x90.3, EPC 10 quadro, 8 channels; ZeitgeberH/HekaReader
test data, BSD-3-Clause).

**Observed in the corpus** (a Python walker written from the documents above, `struct` only):
- Every file is a `DAT2` bundle, little-endian, with `.dat`, `.pul`, `.pgf`, `.amp` items (and
  `.sol`, `.mrk`, `.onl`, `.mth` in newer files); the `.pul` tree ends exactly where its item ends.
- Stored record sizes differ by version, as the documents warn: v2x60/v2x65 write Root 544, Group
  128, Series 1408, Sweep 160 and Trace 408 bytes; v2x90.2 Root 640, Group 144, Series 1408,
  Sweep 288, Trace 512; v2x90.3 (root version 1000) Series 1728 and Sweep 352. Fields past a
  record's stored size are absent, never read from the next record.
- Every trace is int16 (`TrDataFormat` 0), not interleaved, with a zero Y offset; sweeps of a
  series have the same number of traces; the traces of a sweep share the sample interval and point
  count; each trace's samples are contiguous at `TrData` inside the `.dat` item.
- `TrZeroData` is non-zero on some voltage traces (−70 mV) and changes from sweep to sweep.
- Stored times convert with the documented rule to dates that match the file names (2018-05-14
  for `180514s1c1r1`).

**Our choices (Inferred):**
- A series is a trace; its sweeps are sweeps; the trace records of a sweep are channels. Channels
  whose sample interval or point counts differ go into separate traces of the same series.
- Values are raw × `TrDataScaler` for integer formats and as stored for float formats, in the
  trace's recorded unit (`A`, `V`); `TrZeroData` is reported (`zero_offset`) but not subtracted,
  as the document says samples are stored "not Zero Subtracted".
- Features the corpus never shows are refused rather than guessed: a non-zero Y offset, virtual
  traces, float formats with a scaler other than 1, the v2000 64-bit layout, `DAT1`/`DATA`/PULSE
  files without a bundle.

## 2026-09-26 — PatchMaster 2.11 files and virtual traces (Richard Zimring with Claude as assistant)

**Corpus file used:** `zenodo4992914-04-11-12-hek-prestin` (Zenodo 4992914, CC0-1.0, member
`HEK_NLC recordings/04-11-2012/04-11-12-HEK_PRESTIN.dat` of `HEK_NLC recordings.zip`).
**Prior art consulted:** none new. **Oracle:** pyHEKA 1.0.1 (AGPL-3.0), black box through its
README's calls (load-heka-python does not know version 2.11).

**Observed:** bundle version `v2.11, 14-Mar-2006`; stored record sizes Root 536, Group 128,
Series 1120, Sweep 160, Trace 280 — trace records end before the interleave fields (292), so
such traces are contiguous. Every "LockIn Volt" series holds an Imon trace (int16, 100 points,
20 kHz) and a lock-in capacitance trace `CM` whose data kind has the virtual bit set (float32,
scaler 1, 4 points at 1.25 ms, unit F); the virtual trace's samples are stored in the `.dat`
item like the others, and pyHEKA returns the same values.

**Changed:** trace records need only reach the fields this reader uses (128 bytes, not 300);
virtual traces are read (flagged `virtual` per channel) instead of refused.
