# BioLogic EC-Lab (`.mpr` binary, `.mpt` text export) provenance

## 2026-09-26 — before the readers were written (Richard Zimring with Claude as assistant)

**Prior art:** none. yadg's and galvani's *test data* (`.mpr` files with EC-Lab's own `.mpt` export of the same run) are
used as corpus files under those repositories' licences.

**Corpus files used** (listed in `corpus/manifest.toml` with their licences): the `.mpr`/`.mpt`
pairs of yadg's `tests/test_x_eclab` (GPL-3.0 data, commit 61d55fe; 28 runs from many users'
issue reports: CA, CP, CV, CVA, LSV, OCV, Wait, GCPL, MB, BCD, CC, CV (constant voltage), PEIS,
GEIS, ZIR; EC-Lab 11.x on SP-150/200/240/300, VMP3, VMP-300, VSP, VSP-3e, MPG-2, BCS-805), and
further depositors' files as they are added below.

### Inferred from hex dumps and the paired `.mpt` exports (scratch scripts, not committed)

- File: `BIO-LOGIC MODULAR FILE\x1a` padded with blanks to 0x30, four bytes, then modules from
  0x34 to the end. Module: `MODULE`, a 10-byte short name (`VMP Set`, `VMP data`, `VMP LOG`,
  `VMP loop`, `VMP ExtDev`), a 25-byte long name, then either (older files) u32 length, u32
  version, 8-byte date text — a 57-byte header — or (EC-Lab 11.50 and later) u32 0xFFFFFFFF,
  u32 length, u32 0, u32 version, 8-byte date — 65 bytes. The next module follows the body. This
  accounts for every byte of all 28 files.
- `VMP data` body: u32 point count; the column count as u8 (versions 2, 3) or u16 (version 10/11),
  then that many u16 column ids; the records start at a fixed offset from the body start: 405
  (version 2), 406 (version 3), 1007 (version 11); body length = offset + points × record size in
  every file. A record is: one flag byte when any of the flag ids 1, 2, 3, 21, 31, 65 is listed
  (bits: 0-1 `mode`, 2 `ox/red`, 3 `error`, 4 `control changes`, 5 `Ns changes`, 7
  `counter inc.`), then the other ids' values in list order, little-endian.
- Column ids, their `.mpt` names and stored types (every value of every row equals the export's,
  float32 to the export's 8 significant digits, float64 to 16): 4 time/s f64; 5 control/V/mA
  f32; 6 Ewe/V f32 (`Ecell/V` in battery modes); 7 dq/mA.h f64; 8 I/mA f32; 9 Ece/V f32; 11
  <I>/mA f64; 13 (Q-Qo)/mA.h f64; 16, 17 Analog IN 1/2 /V f32; 19 control/V f32; 20 control/mA
  f32; 23 dQ/mA.h f64; 24 cycle number f64; 32 freq/Hz f32; 33 |Ewe|/V f32; 34 |I|/A f32; 35
  Phase(Z)/deg f32; 36 |Z|/Ohm f32; 37 Re(Z)/Ohm f32; 38 -Im(Z)/Ohm f32; 39 I Range u16; 70 P/W
  f32; 74 |Energy|/W.h f64; 76 <I>/mA f32; 77 <Ewe>/V f32; 96 |Ece|/V, 98 Phase(Zce)/deg, 99
  |Zce|/Ohm, 100 Re(Zce)/Ohm, 101 -Im(Zce)/Ohm f32; 123, 124 Energy charge/discharge /W.h f64;
  125, 126 Capacitance charge/discharge /µF f64; 131 Ns u16; 168 Rcmp/Ohm f32; 169 Cs/µF, 172
  Cp/µF f32; 174 <Ewe>/V f32; 430-433 Phase/|Z|/Re/-Im of Zwe-ce f32; 434 (Q-Qo)/C f32; 435 dQ/C
  f32; 438 step time/s f64; 441 <Ece>/V f32; 467 Q charge/discharge/mA.h f64; 468 half cycle u32;
  469 z cycle u32; 471 <Ece>/V f32; 473/474 THD Ewe/I /%, 476/477 NSD, 479/480 NSR, 486-491
  |Ewe h2-h7|/V, 492-497 |I h2-h7|/A f32; 880 Energy we/W.h f64. The export's column order is the
  stored order, followed by quantities EC-Lab computes at export (energies, capacities,
  efficiency, P/W, Ewe-Ece, …), which the `.mpr` does not store.
- EIS harmonic-analysis columns (THD, NSD, NSR, harmonics): the export prints -1 and 0 at the
  highest frequencies (136.8 and 200 kHz in two files; values stored below 91 kHz are printed)
  where the `.mpr` stores numbers; the stored numbers are returned.
- `VMP Set` byte 0 and `VMP LOG` byte 2: the technique code (0x04 GCPL, 0x06 CV, 0x0B OCV, 0x18
  CA, 0x19 CP, 0x1C Wait, 0x1D PEIS, 0x1E GEIS, 0x32 ZIR, 0x33 CVA, 0x6C LSV, 0x75 constant
  voltage, 0x76 constant current, 0x7F Modulo Bat, 0x88 battery capacity determination; names
  from the export's fourth line). `VMP LOG` byte 9: channel − 1 (numbered channels). `VMP LOG`
  +585: float64 OLE date of "Acquisition started on" (local time; all 25 files with a log).

## 2026-10-06 — `.mpt` exports whose time column holds dates and times (Richard Zimring with Claude as assistant)

Held-out draw D reported an EC-Lab text export rejected as corrupt because its time column holds absolute dates and times (finding D-H5). No held-out file was opened.

**Corpus files used (new):** `echem-figshare30080953-cp-mpt` and `echem-figshare30080953-ca-mpt` (figshare 30080953, Apache-2.0, A. Bhadouria: a chronopotentiometry and a chronoamperometry export of 2024, without EC-Lab's header block). Also looked at, not added: PyProBE's `tests/sample_data/biologic/Sample_data_biologic_timestamped.txt` (BSD-3-Clause, a `BT-Lab ASCII FILE` export trimmed to 7 rows and re-saved).

**Prior art consulted:** none.

**What was inferred from what:**
- Both files label the column `time/s`, as the other exports do, but write each cell as `MM/DD/YYYY hh:mm:ss.ffff` (`07/19/2024 16:53:36.5000`). `step time/s` stays in seconds. The day (19) shows that the date is month/day/year, the order EC-Lab's header uses for `Acquisition started on`. The PyProBE file writes its header's `Acquisition started on : 11/20/2024 11:38:41.707` and its first time cell `11/20/2024 11:38:41.707` alike.
- Successive cells step by the sampling interval (0.05 s in the CP file, 0.1 s in the CA file), so the column is the sampling time.

**Decided:**
- A column whose first cell is a date and time is read as dates and times in every row (a row that is not one is corrupt) and returned as seconds from its first row. The first row's date and time is the acquisition start when the file has no `Acquisition started on` header line (local time, as the header's).
- Month/day/year is read unless a date's first field is above 12. When both orders fit every date, month/day/year is taken unless only day/month/year keeps the times from running backwards, and the start date is reported as assumed in that case.
- `check` reports `absolute_times` (info), and the assurance profile observes the layout `absolute time column`.

**Validation:** `oracle/series_oracle.py --mpt` reads the export's text with the Python standard library (`datetime.strptime`), now including the date-time column as seconds from the first row. Every column of both files agrees. This oracle is a second implementation of the same text, not an independent reader, so these files do not validate the layout.

## 2026-10-06 — `.mpr` data module version 0 (Richard Zimring with Claude as assistant)

Held-out draw D reported an older `.mpr` refused because its data module has version 0 (finding D-G1). No held-out file was opened.

**Corpus files used (new):** `echem-figshare1228760-bio-logic1` and `echem-figshare1228760-bio-logic4`, each with EC-Lab's `.mpt` export of the same run (figshare 1228760 "galvani test data", CC-BY-4.0, C. Kerr, K. Ogata, M. Richter, J. B. Warrington; the same bytes as galvani's `tests/testdata/bio_logic1.mpr` and `bio_logic4.mpr`, whose `.reuse/dep5` also licenses them CC-BY-4.0). Their `VMP Set`, `VMP data` and `VMP LOG` modules all have version 0, dated 10/29/11 and 11/01/11. Zenodo 3631156 holds more version-0 files but comes from the authors of a held-out record, so it was not used.

**Prior art consulted:** none read. galvani 0.5.0 (GPL-3.0) was run as a black box: it reads both files (3,119 and 4,137 rows).

**What was inferred from what (hex dumps against the paired exports):**
- The 57-byte module header is the older one (length, version 0, date).
- `VMP data` body: u32 point count (3119; 4137), then a u8 column count (11; 10), then one **u8** column id per column (`01 02 03 15 1f 41 04 05 06 07 46`: the flag ids 1, 2, 3, 21, 31, 65, then 4 time/s, 5 control/V/mA, 6 Ewe/V, 7 dq/mA.h, 70 P/W; and `01 02 03 15 1f 41 04 13 06 07`: 19 control/V instead of 5 and no P/W). Versions 2 and 3 store the ids as u16.
- The records start 100 bytes after the body start: 90,551 = 100 + 3,119 × 29 and 103,525 = 100 + 4,137 × 25, with the record sizes the known column types give (a flag byte, f64 time and dq, f32 control, Ewe and P/W). The first record's flag byte is 0x0b: mode 3 and error 1, the export's first row (`3 0 1 0 0 0`).
- `VMP LOG` holds the acquisition start as an OLE date at +465 in these files, where files with later data modules hold it at +585 (their logs can have version 0 too: `echem-yadg-cp`, data module 3, log version 0, start at +585): 40845.814120 (2011-10-29 19:32:20) and 40848.641852 (2011-11-01 15:24:16), the start times galvani reports. Byte 2 holds the technique code, as in later logs (0x04 GCPL in the GITT file, 0x05 in the PITT file, a code no other development file has).
- Every stored column equals galvani's on every row and the export's to its printed digits. These exports print `time/s` with 5 decimals (`10.00020` for the stored 10.0001997…), so `oracle/series_oracle.py` now compares an export's times within half a unit of its last printed decimal.

**Decided:** data module version 0 is read with u8 column ids and records from byte 100 of the body. The record-size check (body length = 100 + points × record size) still applies. A file whose data module has version 0 gives the start at log +465.
