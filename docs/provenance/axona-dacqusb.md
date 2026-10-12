# Provenance log — Axona DacqUSB recordings (`.set` + `.bin`, tetrode `.1` … `.32`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-12 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- Neo 0.14.5 `neo/rawio/axonarawio.py` (BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo,
  the copy installed in `oracle/.venv`): a session is files sharing a stem; the `.set` file and
  the headers of the other files are `key value` text lines ending at `data_start`; the `.bin`
  raw file is a run of 432-byte packets (32 bytes of header, 3 samples × 64 int16 channels,
  16 bytes of trailer) with the channels in a fixed permuted order (Neo's 64-entry offset table);
  active tetrodes from `collectMask_<n> 1` (four channels each); the sampling rate `rawRate`;
  the conversion to µV `1000 × ADC_fullscale_mv / (gain_ch_<i> × 32768)`; tetrode files `.1`,
  `.2` … with `num_spikes` records of four (big-endian int32 time stamp, 50 int8 samples) groups
  and times in units of the header's `timebase`.
- Neo cites an Axona file-format document hosted by a third party; it was not opened, because it
  is not published by Axona.
- Neo is also run as the oracle (`oracle/gen.py`, `axona()`), a black box.
- pyxona (CINPLA, GPL-3.0) and BinConverter (GPL-3.0) were not read.

**Corpus files used** (NeuralEnsemble ephy_testing_data on G-Node GIN, commit bf392aa,
CC-BY-SA-4.0, files contributed by the Hussaini lab): `gin-axona-raw` (`axona_raw.set` +
`.bin`), `gin-axona-multi-modal` (`.set`, `.bin`, four tetrode files, `.eeg`–`.eeg4`,
`.egf`–`.egf4`, `.pos`), `gin-axona-unit-spikes` (`.set`, four tetrode files, `.eeg`, `.pos`).

**What was inferred from the files:**
- Every `.bin` packet starts with `ADU1` or `ADU2` and a little-endian uint32 that counts up by
  one per packet (1165, 1166 … in `axona_raw.bin`); the reader checks the magic and the count.
- The tetrode files end with `\r\ndata_end\r\n` after the `num_spikes` records; the four time
  stamps of a record are equal in all three files, so the first is the spike's time (as Neo uses).
- The `.set` `trial_date` (`Sunday, 4 Oct 2020`) and `trial_time` (`11:07:07`) give the local
  recording start.
- The `.set` `duration` does not bound the `.bin`: `axona_raw.bin` holds 576 samples (12 ms) of
  a 6.00006 s trial, and in `gin-axona-multi-modal` the `.bin` holds the 1.2 s the `.set` gives
  while the `.eeg` and `.egf` headers declare 601 s of samples. The reader takes the sample count
  from the `.bin` length (Neo does the same).
