# PerkinElmer Spotlight `.fsm` images

PerkinElmer Spotlight FT-IR imaging systems (with Spectrum IMAGE) write a map of spectra to one `.fsm` file. OpenReadout returns the spectra as one trace with a sweep per pixel, a table of pixel positions, and an image with one channel per spectral point. The instrument settings use the same blocks as [`.sp` files](perkinelmer-sp.md).

Derived from specio's FSM plugin (BSD-3-Clause, read as documentation and run as a reference reader), the `.sp` notes and hex dumps of two public files from two depositors (Spotlight with Spectrum One, and Spotlight 400 with Spectrum 3). See `docs/provenance/perkinelmer-fsm.md`.

Crate: `openreadout-spectro`, format id `perkinelmer-fsm` (`FSM_FORMAT_ID`), extension `fsm`,
reader `FsmReader`; the dataset is the shared `SpectroDataset`.

## Detection

`PEPE`, then a 40-byte description containing `4D` (`DataSet - 4DConst3DInterval`) → definite.
The `.sp` reader claims `PEPE` files whose description does not contain `4D`.

## Layout

After the 44 bytes, blocks: u16 id, i32 body size, body. Unknown blocks are skipped (listed in the
vendor tree); a block past the end is a `truncated` finding.

- **5100 — geometry:** u16-length name (may be empty), ten f64 (x step, y step, z step, z first,
  z last, two 4-D limits, x origin, y origin, z origin), three i32 (width, height, points), four
  u16-length texts: the y and x stage-axis labels, the spectral unit (`cm-1`) and the value
  unit (`%T`, `A`, `%R`, `KM`).
- **5104 — records:** a block tree like an `.sp` data set: history records (121: user 35698,
  operation 35699, date 35700) and the instrument record (123: model 35837, serial 35838,
  firmware 35839, scans 35840, detector 35841, source 35842, beamsplitter 35843, resolution
  35844, apodization 35845, spectrum type 35846, beam type 35847, phase correction 35849,
  accessory 35854; laser wavenumber 35882).
- **5105 — one spectrum:** `points` float32 values; one block per pixel, along x first.

## Mapping

One trace (named by the image name, else `<quantity> spectra`) with one sweep per pixel, the x
axis regular from z first to z last, the y quantity from the value unit as in `.sp`
(`transmittance` %, `absorbance` AU, `reflectance` %, `kubelka_munk`); table `positions`
(`x_px`, `y_px`, `x_um`, `y_um`: origin + index × step, µm); one image, width × height, one float
channel per point, pixel size = the steps. Rows are in acquisition order (the stage y grows by
one step per row); which way up Spectrum IMAGE draws them is not stated in the file.

Trace `extra` (our vocabulary): the `.sp` instrument keys (`instrument`, `instrument_serial`,
`instrument_firmware`, `detector`, `source`, `beamsplitter`, `apodization`, `spectrum_type`,
`beam_type`, `phase_correction`, `accessory`, `scans`, `resolution_cm1`,
`laser_wavenumber_cm1`, `acquired_at`) and `size_x`, `size_y`, `x_units_text`, `y_units_text`,
`stage_axis_labels`, `origin_um`, `step_um`. Image `extra`: `row_order`.

**Experiment.** `instrument` (vendor PerkinElmer, model, serial, firmware as software version),
`acquisition` (`started_at` and `operator` from the oldest history record), `sample.id` (the image
name), `method.parameters`: `resolution`, `scans`, `laser_wavenumber`, `detector`, `source`,
`beamsplitter`, `apodization`, `accessory`, `pixel_size`.

## `check` finding codes

`truncated`, `block_size`, `spectrum_count` (errors), `x_interval` (warning).

## Validation

specio 0.1.0 on both corpus files: spectrum and point counts, six whole spectra bit for bit, the
x axis (24 samples, 1e-9), the middle-band image bit for bit, model and serial.

## Known gaps

Only the 4-D constant-interval layout has been seen. The value unit text can disagree with the
spectrum type (`%T` on a `Ratio (%R)` image); the unit text is used and both are reported.

## Vocabulary (public identifiers of the FSM module of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `FsmReader`, `FSM_FORMAT_ID` | the reader and its format id |
