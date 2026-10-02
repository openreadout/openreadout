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
