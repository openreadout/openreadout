# Provenance log — WinWCP `.wcp` (Strathclyde Electrophysiology Software)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- Neo 0.14.5 `neo/rawio/winwcprawio.py` (BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo,
  the copy installed in `oracle/.venv`): the 1024-byte `KEY=value` text header (`VER`, `NC`, `NR`,
  `NBA`, `NBD`, `ADCMAX`, `DT`, `YN<c>`, `YU<c>`, `YG<c>`, `YO<c>`, `RTIME` from version 9, decimal
  commas), records of `NBA` × 512 bytes of analysis header then `NBD` × 512 bytes of interleaved
  int16 samples, the analysis header's first fields (status, type, group, time recorded,
  sampling interval, `VMax` per channel) and the gain `VMax / ADCMAX / YG`.
- Neo is also run as the oracle (`oracle/gen.py`, `winwcp()`), a black box.

**Corpus files used:** `winwcp-file-winwcp-1` and `winwcp-file-winwcp-2` (NeuralEnsemble
ephy_testing_data on G-Node GIN, CC-BY-SA-4.0; file versions 8 and 9, WinWCP 5.3.7).

**What was inferred from the files:**
- Records start at 1024 + k × (`NBA` + `NBD`) × 512 in both files (Neo writes the analysis block
  as a fixed 1024 bytes; `NBA` = 2 in both, so the two agree) and the file is exactly
  `NR` records long.
- The analysis header's "time recorded" is the record's start in seconds since the first record
  (0, 5.03, 10.03 … and 0, 4.99, 10.0 …: the stimulus repeat period); it is reported as each
  sweep's start (Neo starts every segment at 0).
- `VMax` is stored per record and per channel; it is constant within both files. Values are
  scaled with each record's own `VMax` (Neo uses the last record's for all).
- `YO<c>` is the channel's position in each interleaved sample frame (0, 1 in both files, equal to
  the header order Neo uses); the reader takes it as the column when all are distinct and in range.
- `YZ<c>` (0 in both files) is reported, not applied, as Neo does.
- `RTIME` (`dd/mm/yyyy hh:mm:ss`, version 9) is the recording start in local time; `CTIME` the
  file's creation; `VERPROG` the program version.
