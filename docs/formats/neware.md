# Neware BTS `.nda` / `.ndax`

Neware BTS battery testers save cycling data as `.nda` files (BTS 7–9) and `.ndax` zip files (BTS 8 and later). OpenReadout returns every measurement as one trace with time, voltage, current, capacity, energy, cycle and step channels, and a table of the executed steps. Derived from public files of five depositors (NewareNDA's test files, navani, cellpy/IFE, SINTEF), with NewareNDA (BSD-3-Clause) read as documentation and used as a reference reader. Provenance: `docs/provenance/neware.md`. Crate: `openreadout-echem`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `neware-nda` | `.nda` beginning `NEWARE` (BTS 7-9 battery testers) | versions 29 and 130 (both record layouts) | evidence rubric (`docs/assurance.md`) |
| `neware-ndax` | `.ndax`, a zip with `data.ndc` and XML descriptions (BTS 8+) | `.ndc` versions 5, 11, 14, 16, 17 | evidence rubric |

## `.nda` layout

`NEWARE` and an eight-digit stamp (`20200325`), the version byte at offset 14.

- **Version 29** (BTS 7). The header names its data section: `u32` offset at 0x40, `u32` length
  at 0x44, a whole number of 86-byte records. `0x55 0x00` records are measurements: `u32` index
  (2), `u32` cycle (6, zero-based), `u16` program step (10), `u8` step type (12), `u64` step time
  in ms (14), `i32` voltage in 0.1 mV (22), `i32` current (26), four `i64` accumulators (38:
  charge and discharge capacity, charge and discharge energy), the date as `u16` year and five
  bytes (70), `i32` current range (78). Current and accumulators are integers scaled by a factor
  that depends on the current range (NewareNDA's table; the accumulators are in mA·s and mW·s).
  `0xAA` records and `0x65` auxiliary records are not measurements (neither is decoded). The active
  mass is a `u32` in µg at offset 152.
- **Version 130, BTS 9.0.** 88-byte records from offset 1024 whose first six bytes repeat
  (`0x12`, …, `0x55`): from offset 4 of the record, step (5), step type (6), index (12), `u64` step
  time in µs (24), `f32` voltage (32) and current in mA (36), `f32` charge capacity, charge energy,
  discharge capacity, discharge energy in mA·s / mW·s (48), `u64` timestamp in µs since 1970 (64).
  No cycle number. A `0x81` record starts the footer.
- **Version 130, BTS 9.1.** `0x55` records from offset 1024, 52 bytes (or 56 with a temperature):
  step (2), step type (3), index (8), `u32` seconds and nanoseconds of **test time** (12, 16; it
  does not restart at a step), `f32` current in mA (20) and voltage (24), one signed `f32`
  capacity and one energy accumulator (28, 32; positive while charging), `u32` cycle (36), `u32`
  seconds and nanoseconds since 1970 (44), `f32` temperature (52). A `0x81` record starts the
  footer.

## `.ndax` layout

A zip: `data.ndc` and, from version 11, `data_step.ndc`, `data_runInfo.ndc`, one
`data_AUX_<channel>_<type>_<n>.ndc` per auxiliary channel, `data_log.ndc`, `data_es.ndc`, and the
XML files `VersionInfo.xml` (software and firmware versions), `TestInfo.xml` (channel, program
name, barcode, start and end, auxiliary channels), `Step.xml` (the program). The XML declares
GB2312.

Every `.ndc` file: a 4096-byte header (byte 0 file type: 1 data, 5 auxiliary, 7 step, 18 run
information; byte 2 version) and 4096-byte pages. A page: `u16` kind (file type + 1), `u16`
record count, a validity bitmap, the records (from offset 132; 125 in version 5), and the CRC-32
of the first 4092 bytes in its last four. The reader refuses a page whose CRC or count does not
hold.

| file | version | record |
| --- | --- | --- |
| `data.ndc` | 5 | 87 bytes, marker `0x55` at 7: index (8), cycle (12), step (16), step type (17), `u64` step time in ms (23), `i32` voltage in 0.1 mV (31), `i32` current (35), four `i64` accumulators (43), date (75), current range (82) |
| `data.ndc` | 11, 16 | `f32` voltage in 0.1 mV, `f32` current in mA |
| `data.ndc` | 14, 17 | `f32` voltage in V, `f32` current in A |
| `data_step.ndc` | 11, 14 (37 bytes); 16, 17 (100 bytes) | `i32` cycle (zero-based), `i32` program step, step type at 24 |
| `data_runInfo.ndc` | 11 (47), 14 (55), 16, 17 (100 bytes) | `i32` step time in ms (0), four `f32` accumulators (5; A·s in 11/16, A·h in 14/17), `i32` logging interval in ms (29), `i32` seconds since 1970 (33), `i32` step number (37), `i32` record index (41), `i16` milliseconds (45) |
| `data_AUX_c_t_n.ndc` | 14, 17 | one `f32` per record; `t` + 100 is the `ChlType` of `TestInfo.xml` (102 voltage, 103 temperature) |

Only some records have a run-information record: the tester logs one when the logging interval
changes, at a step change, and periodically. A record in between took the previous logged
record's interval; its step time is reconstructed from it (reported as `assumed`). Capacities
and energies exist only on logged records; the reader returns NaN for the others.

## What the reader returns

One trace `records` (every measurement, in file order):

| channel | unit | from |
| --- | --- | --- |
| `time` | s | test time: stored (BTS 9.1), or the step times accumulated (a new step begins on the instant the last one ended) |
| `step_time` | s | stored (not in BTS 9.1 files) |
| `voltage` | V | |
| `current` | mA | negative while discharging |
| `charge_capacity`, `discharge_capacity` | mA·h | per step, as the tester accumulates them |
| `charge_energy`, `discharge_energy` | mW·h | per step |
| `cycle` | — | stored cycle + 1 (absent in BTS 9.0 files) |
| `step_index` | — | program step |
| `step_type` | — | code; `extra.step_types` names them |
| `record` | — | the record's index |
| `temperature` | °C | BTS 9.1 56-byte records |
| `aux_voltage_n`, `aux_temperature_n`, `aux_<type>_n` | V, °C, — | `.ndax` auxiliary files |

Step types: 1 `cc_charge`, 2 `cc_discharge`, 3 `cv_charge`, 4 `rest`, 5 `cycle`, 7
`cccv_charge`, 8 `cp_discharge`, 9 `cp_charge`, 10 `cr_discharge`, 13 `pause`, 16 `pulse`, 17
`simulation`, 19 `cv_discharge`, 20 `cccv_discharge`, 21 `control`, 22 `ocv`, 26
`cpcv_discharge`, 27 `cpcv_charge`.

A table `steps`: one row per executed step (first and last record, cycle, program step, step
type, duration on the time axis, the last logged charge and discharge capacity).

Experiment: vendor Neware; start from the first record (tester local time for the calendar
stamps of version 29 and `.ndc` 5, UTC for the others); duration; active mass (version 29);
program name, barcode and end time (`.ndax`); software version. Findings: `time_vs_clock` when
the tester's clock advances more than the time axis (pauses, clock changes), `step_time_gap`,
`logged_records`, `aux_not_decoded`, `aux_type_unknown`, `no_footer`, `index_order`.

## Validation

`tests/series_oracle/mod.rs`: NewareNDA 2024.x (BSD-3-Clause, run as a black box) returns the same
voltage, current, step time (test time in BTS 9.1), capacities and energies (on logged records),
cycle, program step, step type and record index on all 9 development files, and the same values
for each auxiliary file. NewareNDA names the auxiliary channels of `echem-cellpy-ife-ndax` by
sorted file name and so swaps them; we name them by the channel type in the file name. One `.ndax`
depositor (SINTEF, Zenodo 20802274) is held out.

## Vocabulary (public API of `openreadout-echem`, Neware)

| identifier | meaning |
| --- | --- |
| `NdaReader` | reader of Neware `.nda` files |
| `NdaxReader` | reader of Neware `.ndax` files |
| `NDA_FORMAT_ID` | `neware-nda` |
| `NDAX_FORMAT_ID` | `neware-ndax` |
