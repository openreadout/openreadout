# Arbin MITS Pro `.res`

Arbin MITS Pro 4–8 battery testers store their results as `.res` files, which are Microsoft Jet 4 (Access) databases. OpenReadout returns every data record as one trace of Arbin's columns (test time, voltage, current, capacities, energies and the rest), with the test and channel metadata.

Derived from public files of several depositors. The Jet database layout was read from the documentation of access_parser (Apache-2.0) and Jackcess (Apache-2.0); access_parser is also the independent reader the corpus is compared against. Provenance: `docs/provenance/arbin-res.md`.
Crate: `openreadout-echem` (the reader), `openreadout-core::jet` (the database).

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `arbin-res` | `.res` result files of Arbin MITS Pro 4-8 battery testers (a Microsoft Jet 4 database) | `Results File` versions 4.145, 5.23, 5.26 and 20171213 | evidence rubric (`docs/assurance.md`) |

Not read: MITS 10+ results (kept in SQL Server, exported as `.xlsx`/`.csv`), the schedule
(`.sdu`), and Jet 3 (Access 97) or encrypted databases (refused, exit 6).

## The container: a Jet 4 database

4096-byte pages. Page 0: `00 01 00 00`, `Standard Jet DB` (or `Standard ACE DB`), the version at
0x14 (0 Jet 3 — refused; 1 Jet 4; 2-5 ACE). Every Arbin file seen is Jet 4.

- **Table definition** pages begin `02 01`; a `u32` at 4 chains continuation pages, whose bytes
  from 8 on are appended. Relative to byte 8 of the first page: `u32` row count (8), `u16` column
  count (37), `u32` count of real indexes (43); from 55, 12 bytes per real index, then 25 bytes per
  column (type 0, `u16` column number 5, `u16` variable-column slot 7, `u16` column index 9,
  flags 15 — bit 0 set for a fixed-length column —, `u16` fixed offset 21, `u16` length 23), then
  the names (`u16` byte length, UTF-16LE).
- **Catalog**: the table whose definition is page 2 (`MSysObjects`); a row of `Type` 1 whose
  `Flags` has neither 0x80000000 nor 0x2 set is a user table, and its `Id` is the page of its
  definition.
- **Data pages** begin `01 01`; `u32` owning definition page at 4, `u16` row count at 12, `u16`
  row starts from 14. A row ends where the previous one starts (the first ends at the page end);
  0x8000 marks a deleted row (also set on the target of an overflow pointer), 0x4000 a row whose
  four bytes point (row number, 3-byte page) to where it is stored; the offset is the low 13 bits.
  Rows are read from every page a table owns, in page order.
- **A row**: `u16` column count; the fixed-length columns at 2 + their offset; at the end the null
  mask (one bit per column number, set = has a value; a Boolean's value is its bit), before it
  `u16` count of variable columns, then (backwards) their `u16` start offsets and the end of the
  variable data. A column the row predates is null.
- **Values**: Byte (unsigned), Integer `i16`, Long `i32`, Currency `i64`/10000, Single, Double,
  Date/Time (Double days since 1899-12-30), Text (UTF-16LE, or `FF FE` then segments split by a
  zero byte alternating one byte per character and UTF-16LE), Memo (12-byte definition: length with
  two flag bits — 0x80 stored inline after the definition, 0x40 in one row elsewhere, 0 in a chain
  of rows each starting with a pointer to the next). Binary, OLE, GUID and Decimal columns are not
  decoded (none is used by Arbin's data tables).

## Arbin's tables

| table | holds | returned as |
| --- | --- | --- |
| `Global_Table` | one row per test: name, channel, start, creator, comments, schedule, software version, tester serial, mass, specific capacity | experiment facts; every column in `vendor.global` |
| `Channel_Normal_Table` | one row per data point (`Test_ID`, `Data_Point`, times, current, voltage, capacities, energies, …) | the trace |
| `Auxiliary_Table` + `Aux_Global_Data_Table` | auxiliary inputs per data point (`Auxiliary_Index`, `Data_Type`, `X`, `dX_dt`) and their units | trace channels |
| `Channel_Statistic_Table` | per-cycle values at a data point (`Vmax_On_Cycle`, charge and discharge time) | table `statistics` |
| `Event_Table` | test start, end, pauses | table `events` |
| `Resume_Table` | the channel's state at the end | `vendor.resume` |
| `Version_Table` | `Results File <version>` | `format_version` |
| smart battery, CAN BMS, multi-cell ACI | | listed (`info --view structure`); a finding names those with rows |

Rows are ordered by test and data point (the pages do not keep them in order). A file with more
than one test gives one trace per test (`test <id>`).

## What the reader returns

One trace `records` (all values as stored):

| channel | unit | Arbin column |
| --- | --- | --- |
| `time` | s | `Test_Time` (the abscissa) |
| `record` | — | `Data_Point` |
| `step_time` | s | `Step_Time` |
| `date_time` | d | `DateTime`: days since 1899-12-30, the tester's local time |
| `step_index` | — | `Step_Index` (schedule step) |
| `cycle` | — | `Cycle_Index` |
| `fc_data` | — | `Is_FC_Data` |
| `current` | A | `Current` (negative while discharging) |
| `voltage` | V | `Voltage` |
| `charge_capacity`, `discharge_capacity` | A·h | `Charge_Capacity`, `Discharge_Capacity` |
| `charge_energy`, `discharge_energy` | W·h | `Charge_Energy`, `Discharge_Energy` |
| `dv_dt` | V/s | `dV/dt` |
| `internal_resistance` | Ω | `Internal_Resistance` |
| `ac_impedance` | Ω | `AC_Impedance` |
| `aci_phase_angle` | — | `ACI_Phase_Angle` |
| other columns (`pulse_stage_index`, `acr`, `tc_counter1`, …) | — | Arbin's name in snake case (`extra.label` keeps it) |
| `aux_voltage_n`, `aux_temperature_n`, `aux_type<t>_n` | V, °C, the recorded unit | `Auxiliary_Table` `X` of `Data_Type` 0, 1, t and `Auxiliary_Index` n − 1 |
| `aux_…_n_rate` | unit/s | `dX_dt` when stored |

The auxiliary data type is named from the unit `Aux_Global_Data_Table` records (`V` for type 0,
`C` for type 1 in every file seen).

Experiment: vendor Arbin Instruments, software MITS Pro and its version, tester serial, sample
name (`Test_Name`), sample id (`Item_ID`), operator (`Creator`), comment, method (the schedule
file), start (`Start_DateTime`, local time, no zone), duration (the last test time), mass (g),
specific capacity (A·h/g) and nominal capacity (A·h) when set, channel. The units of mass and
specific capacity are inferred from the magnitudes (a silicon anode: 0.00085 and 3.579).

## Validation

`tests/series_oracle/mod.rs` against access_parser 0.0.6 (Apache-2.0, black box,
`oracle/arbin_oracle.py`): every data-point column (sampled at 256 rows), every auxiliary input,
the statistics table and the test facts agree exactly on all 7 development files. For
`echem-cellpy-arbin-test001`, cellpy's own golden output (a third reader, MIT) agrees with
access_parser on 13 columns of all 10 261 rows.

## Vocabulary (public API of `openreadout-echem`, Arbin)

| identifier | meaning |
| --- | --- |
| `ArbinReader` | reader of Arbin `.res` files |
| `ARBIN_FORMAT_ID` | `arbin-res` |
| `FORMAT_ID` | `arbin-res` (module constant) |
| `parse` | parse a `.res` database into a series file |
