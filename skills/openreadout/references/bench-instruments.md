# Bench instruments, materials and electrochemistry

Where the numbers live for protein purification, biophysics, gels, EPR, X-ray diffraction, electrochemistry, battery cycling, thermal analysis, rheology and capillary electrophoresis. All of these are `traces[]` and `tables[]` in `info`; read curves with `trace` (`--trace N`, `--channel NAME`, `--x-range A:B` on the trace's own axis) and tables with `table --table K`. Values are as stored: no baseline, no Kα2 stripping, no iR compensation, no DSC sensitivity calibration; time axes are seconds from the run start.

## Nucleic acid and protein quantification

- **NanoDrop** (8000 `.txt`, Eight `.txt`/`.tsv`, One `.csv` and UV-Vis spectra `.tsv`) and **Qubit** (4, Flex `.csv`): table 0 `samples`, one row per measurement: `sample`, `measured_at`, `concentration` (NanoDrop) or `tube_concentration`/`sample_concentration` (Qubit) with the unit in the column's `unit`, `a260`, `a280`, `a260_a280`, `a260_a230`, RFU columns. Text such as `Out of range` is in `<column>_note`. NanoDrop spectra are trace 0, one sweep per sample on a wavelength axis (`trace --sweep N`). Many exports into one table: `batch table FOLDER -o all.csv` (one row per sample, with a `path` column).
- **Incucyte** metric exports (`.txt`): a kinetic plate (`plate` format), so `analyze growth` and the plate previews apply; the metric is the read (`mode: imaging`, unit `%` for confluence).

## Protein purification and biophysics

- **ÄKTA/UNICORN** (`.res`, `.zip`): one trace per curve; pick it by `info` → `traces[].extra.kind` (`uv`, `conductivity`, `concentration_b`, `ph`, `pressure`). The axis is volume in ml from the method start (UNICORN's own zero is the last injection; `info` notes it), so `argmax_axis_value` is an elution volume and `--x-range 10:20` is in ml. Fractions, injections and UNICORN's own peak tables are `tables[]` by `name`: quote them rather than re-integrating.
- **MicroCal ITC** (`.itc`): trace 0 is the thermogram (differential power, µcal/s), table 0 the injections (volume µL, duration, spacing); concentrations, temperature and stirring are in `experiment.method.parameters`. Integrated heats are not in the file.
- **Biacore** `.blr`: one trace per stored curve (`extra.cycle`, `flow_cell`, `reference_subtracted`, `keywords`); `report_points` (`AbsResp`, `RelResp`) and the `cycles` table (sample and concentration per cycle). Subtracted curves and report points are the control software's, returned as stored. Evaluation files (`.bme`): the `fits` table has one row per fit (`KD`, `KD_se`, `ka`, `kd`, `Rmax`, `chi2`; KD in M), `fit_points` which concentrations were fitted; `curve` is the flow cell (`Fc=2-1`). Fits are returned as stored, not refitted.
- **Seahorse XF** (`.asyr`): table 4 `rates` (measurement, well, group, time, ocr, ecar, per), table 2 injections per port, table 0 wells (background wells), table 1 measurements. OCR/ECAR/PER are computed with the published compartment model, validated against Wave on the standard 96-well plate only (elsewhere no `rates` table and a note); `tables[4].extra.agreement_with_wave` states the tolerance; PER assumes the buffer factor in `extra.kvol`.
- **Octet BLI** (`.frd`, one biosensor per file): trace 0 channel `response` (nm), channel `step` numbers the assay steps; table 0 lists the steps (type, sample, concentration, start/end). kon/koff/KD are not in `.frd` files.
- **Zetasizer** (`.dts`): table 0 has one row per record (`record`, `kind`, `sample_name`, `measured_at`, `z_average` in nm, `pdi`, intensity peak means and areas, zeta potential, mobility, conductivity). Peak widths and the number and volume peaks are withheld (finding `size_values_withheld`): use the Zetasizer export for those.
- **GenePix** `.gpr`: table 0 with GenePix's own column names (`F635 Median`, `B635 Median`, `Flags`; `Name`/`ID` are categories); count spots with `table FILE --filter 'F635 Median > 1000' --count`.
- **Gels and blots** (Image Lab `.scn`, Typhoon `.gel`, imager TIFFs): `images[0].channels[0].exposure_ms`, `experiment.instrument`. Band quantification: `stats FILE --lanes` gives each lane's `total_volume` and its bands' `volume`, `fraction_of_lane` and `relative` (to the strongest band), background-subtracted; `saturated_px > 0` means the volume is a lower bound. `--lanes=N` when you know the lane count, `--ladder 1:250,150,100` for sizes, `--region` to leave out wells or a marker strip. Look at `preview --marks lanes` before quoting volumes.

## EPR, X-ray diffraction

- **Bruker EPR** (`.DSC`/`.DTA`, `.par`/`.spc`): `argmax_axis_value` is in G; `experiment.method.parameters` has `microwave_frequency` (GHz), `microwave_power`, `modulation_amplitude`. 2-D sets: one sweep per slice, the second axis in `table FILE --table 0`.
- **Diffractograms** (XRDML, Bruker `.raw`/`.brml`, Rigaku `.ras`/`.rasx`): `argmax_axis_value` in °2θ; `parameters` `anode`, `wavelength_kalpha1`; counts or cps as stored.

## Electrochemistry and battery cycling

- **EC-Lab** `.mpr`/`.mpt`, **Gamry** `.DTA`: channel 0 is time (s); `extra.label` is the vendor's column name, `extra.technique` the method; channels such as `ewe`, `current`, `z_real`. Gamry CV: one sweep per cycle.
- **Neware** `.nda`/`.ndax`: table 0 is the steps (cycle, step type, duration, charge/discharge capacity in mA·h per step); `trace --channel voltage`. Split `.ndax`: only logged records carry capacities (NaN between; use the steps table).
- **Arbin** `.res`: channels `discharge_capacity`, `voltage`, `current`, `cycle`, `aux_temperature_1`; tables `statistics` (per cycle) and `events` (indices in `info`). Units are Arbin's: A, V, A·h, W·h (not mA); capacities accumulate within a cycle; `date_time` is days since 1899-12-30.

## Thermal analysis and rheology

- **NETZSCH** `.ngb-*`, **TA** `.001`: channels such as `dsc_signal`, `heat_flow`, `mass`, `sample_temperature`. NETZSCH DSC is in µV, uncalibrated (not mW/mg); `.ngb-ds3`/`-dla`: trace 1 is the correction run.
- **TA TRIOS** `.tri` (DSC, TGA, DHR/HR/ARES rheometers): one trace per procedure step, SI units (heat flow W, weight kg). Rheology channels `storage_modulus`, `loss_modulus`, `tan_delta`, `complex_viscosity`, `angular_frequency`: moduli in Pa computed with TRIOS's own definitions from the stored torque, displacement and geometry.
- **JASCO** `.jws`: a CD run is traces 0-2 (`circular_dichroism` mdeg, `ht_voltage` V, `absorbance`); time-course axes have no recorded unit.

## Capillary electrophoresis

- **Sanger** `.ab1`: table 0 `base calls` (position, base, quality, scan); `experiment.method.parameters` has `read_length`, `mean_quality`, `bases_q20`. Trace 0 is the analysed A/C/G/T channels on a scan axis. `preview` draws the chromatogram (`x_range` in base positions), and so does `plot` (`show: sanger`, the default for a read; x `range` in bases); `export` writes FASTQ by default.
- **Fragment analysis** `.fsa`/`.hid`: trace 0 `sized` has channel 0 `size` (bp) as its axis and one channel per dye; `extra.sized_by` is `instrument` (the 3500's own sizing) or `openreadout` (Local Southern against `extra.size_standard`). Allele sizes: `peaks --trace 0 --channel K` (positions in bp); the instrument's own peak table is the table named `peaks` when present, the ladder fit the table `size standard`.
- **Bioanalyzer** XML exports and **TapeStation** XML (with `<name>_Electropherogram.csv` beside it): one trace per well (`extra.well`, `sample`, `rin`/`din`), channel 0 `size` the axis; table `samples` (RIN/RINe/DIN, concentration), table `peaks` (the instrument's sizes, concentrations, molarities). Native `.xad` and tape files are not read: ask for the export.
