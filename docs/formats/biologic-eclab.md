# BioLogic EC-Lab (`.mpr`, `.mpt`)

BioLogic potentiostats and battery cyclers run by EC-Lab save binary `.mpr` files; EC-Lab exports them as `.mpt` text. OpenReadout reads both and returns every stored column, the technique, the channel and the acquisition start.

The binary layout was reverse-engineered from hex dumps of public `.mpr` files of several depositors (EC-Lab 10.x–11.5x, many instruments and techniques) against EC-Lab's own `.mpt` exports, which are the ground truth; galvani (GPL-3.0) is run as a black box as a second reader. No reader's source was read. Provenance: `docs/provenance/biologic-eclab.md`. Crate:
`openreadout-echem`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `biologic-mpr` | `.mpr` (`BIO-LOGIC MODULAR FILE`) | every stored column of the data module (about 70 column ids known), the technique, the channel and the acquisition start | evidence rubric (`docs/assurance.md`) |
| `biologic-mpt` | `.mpt` text export (with or without its `EC-Lab ASCII FILE` header block; also `.txt`) | every exported column, header facts (device, software, start, electrode area, mass, reference electrode) | evidence rubric |

## `.mpr`

Modules after byte 0x34: `MODULE`, 10-byte short name, 25-byte long name, then either u32 length,
u32 version, 8-byte date (57-byte header) or, since EC-Lab 11.50, u32 0xFFFFFFFF, u32 length, u32
0, u32 version, date (65 bytes). Modules: `VMP Set` (settings; byte 0 the technique code),
`VMP data`, `VMP LOG` (byte 9 channel − 1; float64 OLE date of the acquisition start at +585, +465 in a version-0 log; local
time), `VMP loop` (u32 count and u32 start index per loop), `VMP ExtDev`.

`VMP data`: u32 points; column count u8 (versions 0, 2, 3) or u16 (10, 11); column ids, u8 in
version 0 and u16 otherwise; records from +100 (v0), +405 (v2), +406 (v3) or +1007 (v10/11);
body length = offset + points × record size (else corrupt). A record: one flag byte when any flag id is listed, then the other columns in list
order. **A column id not in the table below is refused** (exit 6: the record layout cannot be
known), with the advice to read the `.mpt` export instead.

Flag byte: `mode` (bits 0-1, id 1), `ox_red` (bit 2, id 2), `error` (bit 3, id 3),
`control_changes` (bit 4, id 21), `sequence_changes` (bit 5, id 31, EC-Lab's `Ns changes`),
`counter_increment` (bit 7, id 65).

| id | EC-Lab label | our channel | unit | stored |
| --- | --- | --- | --- | --- |
| 4 | time/s | `time` | s | f64 |
| 5 | control/V/mA | `control` | — | f32 |
| 6 | Ewe/V (Ecell/V) | `ewe` | V | f32 |
| 7 | dq/mA.h | `dq` | mA·h | f64 |
| 8 | I/mA | `current` | mA | f32 |
| 9 | Ece/V | `ece` | V | f32 |
| 11 | <I>/mA | `current_mean` | mA | f64 |
| 13 | (Q-Qo)/mA.h | `charge` | mA·h | f64 |
| 16, 17 | Analog IN 1/V, 2/V | `analog_in_1`, `analog_in_2` | V | f32 |
| 19 | control/V | `control_potential` | V | f32 |
| 20 | control/mA | `control_current` | mA | f32 |
| 23 | dQ/mA.h | `delta_q` | mA·h | f64 |
| 24 | cycle number | `cycle` | — | f64 |
| 32 | freq/Hz | `frequency` | Hz | f32 |
| 33 | \|Ewe\|/V | `ewe_amplitude` | V | f32 |
| 34 | \|I\|/A | `current_amplitude` | A | f32 |
| 35 | Phase(Z)/deg | `z_phase` | ° | f32 |
| 36 | \|Z\|/Ohm | `z_modulus` | Ω | f32 |
| 37 | Re(Z)/Ohm | `z_real` | Ω | f32 |
| 38 | -Im(Z)/Ohm | `z_imag_neg` | Ω | f32 |
| 39 | I Range | `current_range` | — | u16 |
| 70 | P/W (Pwe/W) | `power` | W | f32 |
| 74 | \|Energy\|/W.h | `energy_abs` | W·h | f64 |
| 76 | <I>/mA (impedance) | `current_mean` | mA | f32 |
| 77, 174 | <Ewe>/V | `ewe_mean` | V | f32 |
| 96, 98-101 | \|Ece\|, Phase(Zce), \|Zce\|, Re(Zce), -Im(Zce) | `ece_amplitude`, `zce_phase`, `zce_modulus`, `zce_real`, `zce_imag_neg` | V, °, Ω | f32 |
| 123, 124 | Energy charge/discharge /W.h | `energy_charge`, `energy_discharge` | W·h | f64 |
| 125, 126 | Capacitance charge/discharge /µF | `capacitance_charge`, `capacitance_discharge` | µF | f64 |
| 131 | Ns | `sequence` | — | u16 |
| 168 | Rcmp/Ohm | `r_compensation` | Ω | f32 |
| 169, 172 | Cs/µF, Cp/µF | `capacitance_series`, `capacitance_parallel` | µF | f32 |
| 430-433 | Phase, \|Z\|, Re, -Im of Zwe-ce | `zwece_phase`, `zwece_modulus`, `zwece_real`, `zwece_imag_neg` | °, Ω | f32 |
| 434 | (Q-Qo)/C | `charge_coulomb` | C | f32 |
| 435 | dQ/C | `delta_q_coulomb` | C | f32 |
| 438 | step time/s | `step_time` | s | f64 |
| 441, 471 | <Ece>/V | `ece_mean` | V | f32 |
| 467 | Q charge/discharge/mA.h | `charge_half_cycle` | mA·h | f64 |
| 468 | half cycle | `half_cycle` | — | u32 |
| 469 | z cycle | `z_cycle` | — | u32 |
| 473, 476, 479 | THD, NSD, NSR Ewe /% | `thd_ewe`, `nsd_ewe`, `nsr_ewe` | % | f32 |
| 474, 477, 480 | THD, NSD, NSR I /% | `thd_current`, `nsd_current`, `nsr_current` | % | f32 |
| 486-491 | \|Ewe h2\|…\|Ewe h7\| /V | `ewe_h2` … `ewe_h7` | V | f32 |
| 492-497 | \|I h2\|…\|I h7\| /A | `current_h2` … `current_h7` | A | f32 |
| 880 | Energy we/W.h | `energy_we` | W·h | f64 |

Technique codes: 0x04 GCPL, 0x06 CV, 0x0B OCV, 0x18 CA, 0x19 CP, 0x1C Wait, 0x1D PEIS, 0x1E GEIS,
0x32 ZIR, 0x33 CVA, 0x6C LSV, 0x75 constant voltage, 0x76 constant current, 0x7F Modulo Bat, 0x88
battery capacity determination (others: `technique code 0x..`).

EC-Lab computes some columns only when it exports text (energies and capacities per cycle,
efficiency, `P/W`, `Ewe-Ece/V`, `x`, `Q charge`/`Q discharge`, `Capacity`); an `.mpr` returns what
it stores. The export prints -1 (THD/NSD/NSR) and 0 (harmonics) at the highest impedance
frequencies (above about 100 kHz) where the `.mpr` stores numbers; the stored numbers are
returned (info finding `harmonics_as_stored`).

## `.mpt`

`EC-Lab ASCII FILE`, `Nb header lines : N`, the header block (technique name on line 4, then
`key : value` lines), the column labels on line N, tab-separated rows (Latin-1; decimal commas in
some locales, decided on the first row). A file without the header block starts with the labels.
EC-Lab sometimes announces a column after an empty label that its rows do not carry; those labels
are dropped (info finding). Columns are named as in the table above; a label not in it keeps a
name made from the label (`Energy we charge/W.h` → `energy_we_charge`, unit W·h).

EC-Lab can export `time/s` as absolute dates and times (`07/19/2024 16:53:36.5000`). Such a
column is returned as seconds from its first row (info finding `absolute_times`), and its first
row is the acquisition start when the header gives none. Dates are month/day/year, as in the
header, unless a first field is above 12. When both orders fit, month/day/year is taken unless
only day/month/year keeps the times rising, and the start date is reported as assumed.

## What the readers return

One trace sampled in time: channel 0 `time` (s, the irregular abscissa), then the columns in
file order; `extra.label` holds EC-Lab's label, `extra.technique` the technique. Experiment:
vendor BioLogic, potentiostat, EC-Lab; the technique as method name and term (CV CHMO:0000025, LSV
CHMO:0000028, CA CHMO:0000005, CP CHMO:0000017, GCPL CHMO:0002936, PEIS CHMO:0002937, GEIS
CHMO:0000423, OCV CHMO:0002933); start (local time); from `.mpt` headers also model, serial,
software version, operator, comment, `electrode_area` (cm²), `characteristic_mass` (mg),
`reference_electrode`. Vendor tree `eclab` (modules, column ids, technique code, channel index,
loop start indices) or `eclab_text` (header lines).

## Validation

`tests/series_oracle/mod.rs`: for every development `.mpr`, every column EC-Lab exported that the
`.mpr` stores (identified by galvani's reading) equals the export at 256 rows (float32 to the
export's 8 digits), and galvani's values equal ours; for every `.mpt`, galvani's reading of the
paired `.mpr` (or, where galvani cannot read it, a standard-library reading of the table).

## Vocabulary (public API of `openreadout-echem`, EC-Lab)

| identifier | meaning |
| --- | --- |
| `MprReader` | reader of `.mpr` files |
| `MptReader` | reader of `.mpt` text exports |
| `MPR_FORMAT_ID` | `biologic-mpr` |
| `MPT_FORMAT_ID` | `biologic-mpt` |
