# Bruker flex MALDI-TOF (`fid` + `acqus`)

flexControl, the acquisition software of Bruker's MALDI-TOF instruments (autoflex, ultraflex, microflex, MALDI Biotyper, rapifleX), writes each spectrum as a directory holding `fid` (the summed detector signal) and `acqus` (its parameters). OpenReadout reads every spectrum below the path it is given, with m/z from the time-of-flight calibration in `acqus`. Bruker publishes no description of these files. The reader follows OpenMS's XMASS reader (BSD-3-Clause), read as documentation, and was checked against pyOpenMS run on the same files. Provenance: `docs/provenance/bruker-flex.md`. Reader: crate `openreadout-bruker-tims`, module `flex`, format id `bruker-flex`.

## Layout

A run is a directory tree `<run>/<spot>/<acquisition>/<kind>/`, for example `0_A4/1/1SLin/`:

- `<spot>` names the target position (`0_A4`);
- `<acquisition>` numbers the acquisitions of that spot (`1`, `2`);
- `<kind>` says how the spectrum was acquired: `1SLin` linear, `1SRef` or `1Ref` reflector, and `<precursor m/z>.LIFT/1SRef` a LIFT fragment spectrum.

A spot directory (`is_spot`) holds `fid` and an `acqus` (or `acqu`) whose first lines name XMASS (`##TITLE= XMASS Parameter file` or `##ORIGIN= XMASS, Bruker-Daltonics`). flexAnalysis adds `pdata/1/` with the processed spectrum `1r` and its `procs`. The reader does not read `pdata`.

**Detection.** The input is a spot directory, its `fid` or `acqus`, or any directory above spot directories. `find_spots` walks at most 7 levels down and skips `pdata`; detection looks at no more than 400 directories. A directory above spots counts only when each of its subdirectories holds a spot (hidden folders, `__MACOSX` and `pdata` aside), so a folder that also holds other data is not taken for a flex run. Bruker NMR (TopSpin) directories also hold `fid` and `acqus`, but their `acqus` does not name XMASS, so the NMR reader leaves flex directories alone and the flex reader leaves NMR directories alone.

## Parameters

`acqus` is JCAMP-DX style text: `##$KEY= value` lines (some keys use `##.` or no prefix), strings in angle brackets, arrays as `(0..n)` followed by value lines. The reader keeps every key, with its first line of text for arrays. Keys it uses:

| key | meaning |
| --- | --- |
| `TD` | number of points in `fid` |
| `BYTORDA` | byte order of `fid`: 0 little-endian, 1 big-endian |
| `DW`, `DELAY` | time per point and time of the first point (ns) |
| `ML1`, `ML2`, `ML3` | calibration constants (below) |
| `IONIZATION MODE` | `LD+` positive ions, `LD-` negative ions |
| `AQ_mod`, `SPType` | acquisition mode: in the corpus `AQ_mod` 1 is linear, 2 reflector, and `SPType` 2 marks a LIFT spectrum |
| `Parent` | the LIFT precursor m/z |
| `PATH` | the spot directory as the instrument wrote it |
| `NoSHOTS`, `INSTRUM`, `InstTyp`, `FCVer`, `AQ_DATE`, `CMT1`, `SPOTNO`, `PATCHNO` | laser shots, the instrument PC's host name, a numeric instrument type, the flexControl version, the acquisition time, a comment, the spot name |

`acqus` does not name the instrument model.

## Points and m/z

`fid` is `TD` signed 32-bit integers in the `BYTORDA` byte order. Point `i` has the flight time `tof = DW · i + DELAY` (ns). With `b = √(10¹² / ML1)` and `c = ML2 − tof`, `s = √(m/z)` solves `ML3 · s² + b · s + c = 0`:

- `ML3 = 0`: `s = −c / b`;
- otherwise `s = (√(b² − 4 · ML3 · c) − b) / (2 · ML3)`;

and m/z is `s²` (`Calibration`, `mz`). A spot whose `acqus` has no usable `DW`, `DELAY`, `ML1` or `ML2`, or a non-positive `ML1`, is listed with a note and its points cannot be read.

**LIFT spectra.** A LIFT fragment spectrum gets m/z from the same constants. flexControl stores the fragment calibration in `NTBCal`, whose layout is not public, and the reader does not apply it, so fragment m/z can differ from flexAnalysis's. `notes` and each such spectrum's `extra.mz_note` say so.

## Spectra

The run is one list of spectra, one per spot directory in path order. Each spectrum is a profile (`centroided` false), MS level 1, or 2 for LIFT with the precursor from `Parent`; polarity from `IONIZATION MODE`. `native_id` is the spot directory relative to the input. Intensities are 32-bit floats, exact up to 2²⁴.

| key | meaning |
| --- | --- |
| `spectra[0].extra.modes` | spectra per acquisition mode (`linear`, `reflector`, `LIFT`, `unknown`) |
| `spectra[0].extra.total_points`, `instruments`, `instrument_types`, `software_versions` | points in all spectra, `INSTRUM`, `InstTyp` and `FCVer` values |
| `spectra[0].extra.spots` | the first 1000 spots: `path`, `spot`, `mode`, `points` |
| spectrum `extra.spot`, `mode`, `points` | spot name (`SPOTNO`, else `PATCHNO`, else the path), acquisition mode, point count |
| spectrum `extra.shots`, `instrument`, `instrument_type`, `software_version`, `acquired`, `comment` | `NoSHOTS`, `INSTRUM`, `InstTyp`, `FCVer`, `AQ_DATE`, `CMT1` |
| spectrum `extra.calibration` | `DW`, `DELAY`, `ML1`, `ML2`, `ML3` |
| spectrum `extra.processed` | `pdata/1/1r` when flexAnalysis wrote a processed spectrum |
| spectrum `extra.mz_note` | LIFT spectra: the fragment calibration is not applied |

The acquisition mode comes from the directory names, else from the names at the end of `PATH`, else from `AQ_mod`.

`vendor` (`info --view full`) lists the first 200 spots with every `acqus` key (`spots`: `path`, `acqus`; `spot_count`).

## `check`

`check --headers-only` parses every `acqus` (`no_calibration`, `too_many_spots` past 200,000 spots). `check` also reads every `fid` and computes its m/z (`short_fid` when `fid` holds fewer points than `TD`, `bad_spectrum`).

## Vocabulary

| identifier | meaning |
| --- | --- |
| `BrukerFlexReader` | the flex `FormatReader` |
| `FLEX_FORMAT_ID` | `bruker-flex` |
| `FlexDataset` | an open run; `spectrum` returns one spectrum with its points |
| `Params`, `parse`, `get`, `is_xmass`, `Calibration`, `from`, `mz`, `is_spot`, `find_spots`, `dw`, `delay`, `ml1`, `ml2`, `ml3`, `map` | parameter file and calibration (sections above) |
| `modes`, `total_points`, `instruments`, `instrument_types`, `software_versions`, `spots`, `path`, `spot`, `mode`, `points`, `shots`, `instrument`, `instrument_type`, `software_version`, `acquired`, `comment`, `calibration`, `processed`, `mz_note`, `acqus`, `spot_count` | JSON keys (tables above) |
| `no_calibration`, `too_many_spots`, `short_fid`, `bad_spectrum` | `check` finding codes |
| `linear`, `reflector`, `LIFT`, `unknown` | acquisition modes |
