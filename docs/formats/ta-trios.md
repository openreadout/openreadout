# TA Instruments TRIOS `.tri`

TRIOS 5 writes the data of TA Instruments DSC, TGA and DMA analyzers and Discovery rheometers as `.tri` files. OpenReadout returns every procedure step with its stored signals, the instrument and sample facts, and for parallel-plate oscillation steps the moduli TRIOS computes. Files of TRIOS 3 and 4 are refused.

Derived from public TRIOS 5.1 to 5.9 files of several Zenodo depositors (Discovery HR-2 and HR30, DSC25, DSC2500, TGA550, DMA850), compared with TRIOS's own exports where the depositor published them. Provenance: `docs/provenance/ta-trios.md`. Crate: `openreadout-thermal`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `ta-trios` | `.tri` files written by TRIOS 5 (DSC, TGA, DMA, Discovery rheometers) | every procedure step with its stored signals; the instrument, serial, TRIOS version, operator, sample, procedure and geometry; for parallel-plate oscillation steps the storage and loss moduli, tan δ and complex viscosity TRIOS computes | generated (`book/src/reference/evidence.md`) |

Not read: files of the 2019 generation (TRIOS 3/4; byte 2 = `0C`), whose steps hold no signal
records of this layout — refused with exit 6 and a hint to export from TRIOS; variables TRIOS
computes and does not store (normalised heat flow, derivatives, viscosities of flow steps, DMA
moduli, the moduli of other geometries); per-point oscillation waveforms; analysis results
(fits, peak integrations) stored in the file.

## Layout

| part | content | our reading |
| --- | --- | --- |
| bytes 0-24 | `00 25`, a generation byte (`0E`), a size, `08 25 02`, …, `14 20 01`, u32 header length, u32 entry count | generation → `format_version` |
| header | entry count × (key, value) of 7-bit-length UTF-8 strings: `instrumenttype`, `instrumentserialnumber`, `instrumentname`, `companyname`, `rundate`, `culture`, `ticks` (.NET ticks of the PC clock), `holder` (geometry), `operator`, `project`, `samplename`, `comments`, `procedurename`, `proceduresegments` (`;` or `#`), `proceduresignals`, `instrumentmode`, `testtype`, `samplesize` | vendor tree `trios`; experiment facts |
| thumbnail | `01 00 01`, u32 length, a PNG | skipped |
| objects | `21 06 <u32 length> <GUID>`; a procedure step's payload starts with 16 zero bytes and `F2 21 01 04 00 00 00 00 00 00 00 01 00 11 20 02` and contains its properties and signals | one trace per step with signals |
| properties | `20 04 <u32 length> <key GUID> …`, `06 20 01 <u32 length> <u32 type> <u32 1> <value>` (type 2 float64, 4 string) | step name (key `d467abca-…`), TRIOS version (`ed6fea85-…`), the moduli constants below |
| signal | `21 06 <u32 length> <signal GUID>` `01000000 01000000 00 F2` `21 01 04000000 00000000` `01 00 <u32 n>` `01 10` `21 01 <u32 L> <L bytes>` `01 00 <u32 n>` n × float32, then 3 bytes | a channel; the L-byte block is a zero (L = 4) or n per-point u32 flags (`extra.flagged_points` counts the non-zero ones) |
| other `21 06` records | second u32 `0x02800001`: a variable TRIOS computes (no values); `0x101`: the step's signals again with n + 1 points and no values; first u32 2: 64 values per point (oscillation waveforms) | left out (`assurance.undecoded`) |

Each step's signal GUID is global: the same quantity has the same GUID in every file and
instrument. Values are SI (time s, heat flow W, weight kg, torque N·m, angles rad, gap m) except
temperature (°C).

## Channels (our vocabulary)

The first channel is the step's time axis: `time` when stored (DSC, TGA), else `step_time`
(rheometers), else the point index (`extra.axis.quantity` = `point`). Channels keep the stored
order after it. `extra.label` is TRIOS's name, `extra.guid` the signal GUID.

| name | TRIOS label | unit | how it was named |
| --- | --- | --- | --- |
| `time` | Time | s | `proceduresignals` position (DSC, TGA); DSC/TGA exports |
| `temperature` | Temperature | °C | every export |
| `heat_flow` | Heat Flow | W | DSC `proceduresignals`; = −(exported Heat Flow (Normalized)) × sample size (the files' display setting is Exo Up) |
| `delta_t`, `delta_t_zero`, `t_zero_temperature`, `cell_purge`, `heater_temperature`, `power_delivered`, `reference_junction_temperature`, `flange_temperature`, `heat_flow_phase`, `total_heat_capacity` | Delta T, Delta Tzero, Tzero Temperature, Cell Purge, Heater Temp, Power Delivered, Reference Junction Temperature, Flange Temperature, Heat Flow Phase, Total Heat Capacity | °C for the temperatures, else none | DSC25 `proceduresignals` (13 names for 13 records), the same GUIDs in the same positions in the DSC2500 file |
| `heat_capacity` | Heat Capacity | none | DSC2500 `proceduresignals` (14 names for 14 records) |
| `weight` | Weight | kg | TGA `proceduresignals`; TGA export (mg) |
| `temperature_difference`, `sample_purge`, `balance_purge`, `set_point_temperature`, `ramp_rate` | Temperature Difference, Sample Purge, Balance Purge, Set Point Temperature, Ramp Rate | °C for the set point, else none | TGA550 `proceduresignals` positions (9 names, 10 records; the tenth is unnamed) |
| `angular_frequency` | Angular frequency | rad/s | rheometer exports (30 of 30) |
| `step_time` | Step time | s | rheometer exports |
| `raw_phase` | Raw phase | rad | rheometer exports (in °) |
| `oscillation_torque` | Oscillation torque | N·m | rheometer exports (in µN·m) |
| `oscillation_displacement` | Oscillation displacement | rad | rheometer exports |
| `gap` | Gap | m | the moduli (TRIOS's strain constant needs it) |
| `signal_<first 8 hex of the GUID>` | — | none | unknown meaning |
| `storage_modulus`, `loss_modulus`, `tan_delta`, `complex_viscosity` | Storage modulus, Loss modulus, Tan(delta), Complex viscosity | Pa, Pa, –, Pa·s | computed (below); `extra.computed` says so |

## Oscillation moduli (computed)

For steps with angular frequency ω, torque M, raw phase φ, displacement θ and gap h, when the
`holder` names a parallel plate and the file stores the geometry's stress constant K_τ
(`e4fea4e5-…`, 2/(πR³)), the instrument inertia (`6594d858-…`), the geometry inertia
(`2dad9ebd-…`) and the sample density ρ (`a58eb6fe-…`):

- R = (2 / (π K_τ))^(1/3), strain constant K_γ = R / h;
- I = instrument inertia + geometry inertia + π ρ h R⁴ / 6 (the sample);
- G′ = K_τ (M cos φ + I ω² θ) / (K_γ θ), G″ = K_τ M sin φ / (K_γ θ), tan δ = G″ / G′,
  |η*| = √(G′² + G″²) / ω.

These are TRIOS's numbers: in 30 of 30 exports of one depositor (three sessions with different
inertia calibrations, a 12 mm plate) within 2e-5 relative in time sweeps and 4e-4 in frequency
sweeps (at 628 rad/s the inertia term cancels 99 % of the torque, and the stored float32 values
limit the difference). Other plate diameters are computed the same way but never compared: the
assurance feature `oscillation moduli, parallel plate <d> mm` says which.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/series_oracle/mod.rs`,
`oracle/series_oracle.py --trios-export`): every column TRIOS exported whose label a channel
shares is compared row by row (sampled at up to 256 rows): rheometer exports to the export's
6 significant digits (stored signals) and 1e-3 relative (moduli), DSC time, temperature and heat
flow to the CSV's rounding, TGA time, temperature and weight to the workbook's rounding; the
operator and sample name of the rheometer exports' `Details` sheet.

## Vocabulary (public API of `openreadout-thermal`, TRIOS)

| identifier | meaning |
| --- | --- |
| `TriosReader` | reader of TA Instruments TRIOS `.tri` files |
| `TRIOS_FORMAT_ID` | `ta-trios` |
