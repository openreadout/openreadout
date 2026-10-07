# Metadata conventions and the experiment model

Each reader maps its vendor's metadata onto one model with OME-style names. The same rules apply to every format, so a script written against a CZI file also works on an ND2 or a LIF file. This page lists those rules: units, timestamps, colours, axes and missing values. It then describes the `experiment` block, which says what a file's contents mean as an experiment, and how the model maps onto OME-XML when you export.

For a first tour of the JSON output, read [Reading the JSON output](../getting-started/reading-json.md).

## Missing values

- **Missing is omitted.** If the file doesn't record a value, it is left out of the JSON rather than written as `0`, `""` or a default. A value the file stores as `0` where `0` is impossible, such as the pixel size of an uncalibrated axis, also counts as missing.
- **Empty text is missing.** A text field stored empty or blank is left out, just like one the file doesn't have, so a missing key means either "not stored" or "stored empty". Two kinds of name are always present, even when empty: table column names and signal-channel names, because an unnamed column is still a column.
- **Vendor text is kept verbatim** in the vendor tree of `info --view full`. The normalized field holds the converted value. When a reader converts local text, it keeps the original in `extra`, for example ND2's `extra.acquired_at_text`.
- **Text is trimmed.** Leading and trailing blanks are removed from objective and instrument names; inner spacing is kept.

## Units

No field carries a unit string, except `physical_size.unit` (always `µm`) and the free-text units of signal channels and table columns. The unit is fixed by the field name or by this table:

| quantity | unit | UCUM | fields |
| --- | --- | --- | --- |
| length, pixel size, stage position | µm | `um` | `physical_size.{x,y,z}`, `*_um` |
| wavelength | nm | `nm` | `excitation_nm`, `emission_nm`, `emission_band_*_nm` |
| time, interval | s | `s` | `time_increment_s`, `start_s`, `delta_t_s`, `*_s` |
| short durations | ms | `ms` | fields ending in `_ms` (`exposure_ms`) |
| sampling rate | Hz | `Hz` | `sample_rate_hz` |
| mass-to-charge | m/z | `{m/z}` | `mz`, `*_mz` (`precursor_mz`) |
| ion mobility | V·s/cm² (1/K0) | `V.s/cm2` | `inverse_reduced_mobility` |
| volume | nL | `nL` | `volume_nl` |
| temperature | °C | `Cel` | `*_c` (`camera_temperature_c`) |

Irregularly sampled traces, such as chromatograms, have `sample_rate_hz` 0. They set `extra.irregular_sampling`, and channel 0 (`time`) holds each sample's time in seconds.

## Timestamps

- Timestamps are ISO-8601 with a `T`: `YYYY-MM-DDTHH:MM:SS[.fraction]` and a zone. Readers convert to UTC and write `Z`. An explicit `±hh:mm` offset is kept when the source writes one.
- When the source records local time with no zone, the value has no zone designator, and the file's `notes` include a sentence that says so. FCS files, Waters `.raw` headers and some ChemStation dates are like this.
- Fractional seconds keep the source's precision.
- Years outside 1900 to 2100 come from unset clocks and give no timestamp.
- Date-only values are `YYYY-MM-DD`.

## Colours

`channels[].color` and ROI colours are `#RRGGBB`: six upper-case hex digits, no alpha. They record how the acquisition software displayed the channel, not what the dye emits. A white channel (`#FFFFFF`) is a grey-scale display.

## Images and axes

- `images[].index` runs from 0 and is what `--image` takes.
- `size_x`, `size_y`, `size_z`, `size_c` and `size_t` are all at least 1. An image's `plane_count` is `size_z × size_c × size_t`; the file's `plane_count` is the sum over images.
- `dimension_order` is `XY` followed by `C`, `Z` and `T` in the order the source stores its planes, fastest first, as in OME. It is for information only. Planes are always addressed by `(c, z, t)`, and exports always write `XYCZT`.
- `channels[i].index` is the zero-based C index, the value `--select c=` takes. Channels are listed in order, one per C.
- `samples_per_pixel` is 1 for grey or 3 for RGB. An RGB image has one channel per C; its three samples are not separate channels.
- `pyramid_levels` is at least 1; 1 means no pyramid.

### Channels

**Emission.** `emission_nm` is the single emission wavelength the file records, and its meaning differs by format. When the file records a detection band, such as a spectral detector window or an emission filter's edges, `emission_band_start_nm`, `emission_band_end_nm` and `emission_band_center_nm` give it. Compare bands, not `emission_nm`, across formats.

| format | `emission_nm` | detection band |
| --- | --- | --- |
| CZI | the channel's emission wavelength | the detection wavelength ranges |
| ND2 | the peak of the emission spectrum | the emission filter's edges |
| LIF | λ scans: the start of the detection window; otherwise not set | the detection window or band |
| OIR | λ channels: the λ axis wavelength; otherwise the dye's emission | start and end wavelength |
| OME-TIFF, OME-Zarr | the channel's `EmissionWavelength` | the pass band of the channel's emission filters |
| Zeiss LSM | not set | the channel's detection range |

**Acquisition mode.** `channels[].acquisition_mode` is a readable label: the technique, then the contrast method when there is one. Every reader uses the same spelling. Files that store OME enumeration values (CZI, OME-TIFF, OME-Zarr) are converted:

| stored | label |
| --- | --- |
| `LaserScanningConfocalMicroscopy` + `Fluorescence` | Laser Scanning Confocal Fluorescence |
| `WideField` + `Fluorescence` | Widefield Fluorescence |
| `WideField` + `Phase` | Phase Contrast |
| `BrightField` + `Brightfield` | Brightfield |
| `SpinningDiskConfocal` | Spinning Disk Confocal |
| `MultiPhotonMicroscopy` | Multiphoton |
| `TIRF`, `TotalInternalReflection` | TIRF |
| `FluorescenceLifetime` | Fluorescence Lifetime |
| `PALM`, `STORM`, `STED`, `SPIM` | as written |

Other vendor wording is kept, such as `Brightfield (RGB)` from VSI.

**Immersion.** `objective.immersion` is `Oil`, `Water`, `Air`, `Glycerol`, `Multi` or `Other`, whatever the case in the file. `dry` becomes `Air`. Other media, such as `Silicone`, are kept as written.

## Per-frame records

`info --view full` lists per-plane acquisition data under `images[].extra.frames`: the first 100 per image, or all with `--max-frames -1`. `frame_records_total` gives the count. ND2 and CZI files have them. All fields are optional except the indices:

| field | meaning |
| --- | --- |
| `t`, `z` | T and Z index within the image (default 0) |
| `c` | C index; absent when one record covers every channel of the frame |
| `time_ms` or `delta_t_s` | time since the image's `acquired_at` |
| `acquired_at` | absolute time of the frame, ISO-8601 UTC |
| `exposure_ms` | exposure of the frame |
| `exposure_ms_per_channel` | exposures indexed by C |
| `stage_x_um`, `stage_y_um`, `stage_z_um` | stage position when the frame was taken |

Other keys belong to the format, such as ND2's `pfs_status` and `camera_temperature_c`.

## Shared `extra` keys

`extra` holds each format's own fields. These keys mean the same thing in every format, and exports and `info --view explain` read them:

| key | contents | exported as |
| --- | --- | --- |
| `channel_settings` | per channel: detector, binning, gain, exposure, filters | OME `Detector`, `DetectorSettings` |
| `stage_position_um` | `{x, y, z}` | OME `StageLabel` |
| `refractive_index` | number | OME `ObjectiveSettings` |
| `experimenter` | a user name, or name parts, e-mail and institution | OME `Experimenter` |
| `experiment` | a description, or an object with `type` and `description` | OME `Experiment` |
| `rois` | regions of interest with shape, colour and geometry | OME `ROI` |
| `bits_significant` | bits the camera actually filled | – |
| `frames` | per-frame records (above) | OME `Plane` |

## Provenance

`info --view full` includes a provenance map that says how the meaning of each normalized field was learned. Keys are paths into the output, such as `images[].physical_size.x`, with `[]` meaning every element. Each entry names a source, from most to least authoritative:

- `spec`: a published specification or open standard, such as OME-XML or FCS 3.1.
- `vendor-impl`: the vendor's own published implementation or documentation.
- `prior-art`: the public documentation of a permissively licensed community reader.
- `inferred`: worked out by comparing files in the test corpus.

[Reading the JSON output](../getting-started/reading-json.md) explains how to use these when deciding how far to trust a value. Each format's notes in [Formats](../formats/index.md) list the source of every field.

## The experiment block

The `experiment` block says what a file's contents mean as an experiment: which sample, on which instrument, with which method, when, by whom, and what was measured, in scientific words. It has the same shape for a confocal stack, an LC-MS run, a flow-cytometry tube and a plate read, so you can ask the same question of any file. It is left out when the file records none of this.

```bash
openreadout info run.raw --json | jq .data.experiment
```

```json
{
  "sample": {"id": "QC1", "sequence_position": "D:35", "source_field": "spectra[0].extra.sample_name"},
  "instrument": {"vendor": "Thermo Fisher Scientific", "model": "LTQ Orbitrap Discovery", "serial": "20698",
                 "software_version": "2.4 SP1", "kind": {"id": "OBI:0000049", "label": "mass spectrometer"}},
  "method": {
    "name": "UPLC-ESI-profiling-19min_profile_NEG",
    "technique": {"id": "CHMO:0000524", "label": "liquid chromatography-mass spectrometry"},
    "assay": {"id": "OBI:0003097", "label": "liquid chromatography mass spectrometry assay"},
    "parameters": {
      "method_length": {"value": 19, "unit": "min", "ucum": "min"},
      "injection_volume": {"value": 10, "unit": "µL", "ucum": "uL"},
      "polarity": {"value": ["negative"]}, "ms_levels": {"value": [1]}, "ion_source": {"value": "ESI"}
    }
  },
  "acquisition": {"started_at": "2009-05-07T20:03:10.046Z", "operator": "LTQ OT Discovery", "duration_s": 1139.976},
  "measurements": [
    {"kind": "spectra", "indices": [0],
     "what": "LC-MS, negative mode, ESI, 2,048 scans, MS1, retention time 0.0119–19 min, m/z 50–1000",
     "technique": {"id": "CHMO:0000524", "label": "liquid chromatography-mass spectrometry"},
     "terms": [{"id": "MS:1000129", "label": "negative scan"}, {"id": "MS:1000073", "label": "electrospray ionization"},
               {"id": "MS:1000579", "label": "MS1 spectrum"}, {"id": "MS:1000484", "label": "orbitrap"}],
     "parameters": {"scans": {"value": 2048}, "mz_range": {"value": [50, 1000], "unit": "m/z", "ucum": "{m/z}"}}}
  ],
  "provenance": {
    "sample.id": {"source": "inferred", "from": "spectra[0].extra.sample_name"},
    "instrument.model": {"source": "prior-art", "from": "spectra[0].instrument.model"},
    "method.technique": {"source": "inferred", "from": "spectra[0]"},
    "measurements[0]": {"source": "inferred", "from": "spectra[0]"}
  }
}
```

The parts:

- `sample`: what was measured. Fields are `id`, `name`, `well`, `barcode` and `sequence_position` (vial or autosampler position). `source_field` names the field the id came from.
- `instrument`: `vendor`, `model`, `serial`, `software`, `software_version`, and `kind`, an OBI device term.
- `method`: the method, protocol or experiment name as saved by the software; the technique (a CHMO, FBbi or OBI term); the assay (an OBI term); and `parameters`.
- `acquisition`: `started_at`, `ended_at`, `operator`, `duration_s` and `comment`. `duration_s` is the length of the recorded data. `comment` is the free text saved with the acquisition, such as an ABF file comment or FCS `$COM`. `saved_at` appears instead of `started_at` when the file records only when it was saved, as SoftMax Pro text exports do.
- `measurements[]`: one entry per kind of data block (`image`, `table`, `trace`, `spectra`), with `indices` pointing at the blocks. `what` describes the measurement in words, such as `absorbance at 450 nm, 96 wells`.
- `notes`: things to know, such as a CZI whose scenes cover several wells.
- `provenance`: where each value came from.

**Parameters** use our own snake_case names: `nucleus`, `pulse_program`, `method_length`, `injection_volume`, `polarity`, `ms_levels`, `ion_source`, `wavelength`, `objective_magnification`, `pixel_size`, `time_interval`, `sample_rate`, `gradient`, `solvents`, `oven_program`, `column` and others. Each is `{value, unit, ucum}`; a parameter with a unit carries its UCUM code. Structured settings keep their units in their keys:

- `gradient` is the LC pump program as steps `{time_min, flow_ul_min, percent: {A: …, B: …}}`.
- `solvents` names the solvent channels: `{"A": "Eau + 0.1% HCOOH", "B": "ACN + 0.1% HCOOH"}`.
- `oven_program` is the GC oven program as steps `{temperature_c, hold_min, rate_c_per_min}`.

**Provenance.** Every value has an entry keyed by its path in `experiment`, such as `sample.id` or `method.parameters.nucleus`. An entry covers everything under its path. `from` is the field the value was taken from. `source` is the reader's own source for that field, or `inferred` for OpenReadout's own mapping, such as the choice of sample-id field, the `what` text, terms and durations. Values a reader takes from a vendor file name that file, such as `acqus ##OWNER`.

### Where the sample id comes from

`sample.id` is the first of these the file records:

- **Mass spectrometry and chromatography**: the sequence's sample id, then its sample name, then the mzML `sampleList`. The vial or autosampler position becomes `sequence_position`.
- **Flow cytometry**: `$SMNO`, then the vendor keywords `TUBE NAME`, `SAMPLE ID` or `SampleID`, then `$WELLID`, then `$SRC`. `$WELLID` also gives `well`, and `$CELLS` gives `name`.
- **Plate readers**: the plate barcode or id, else the plate name when it is not a default such as `Plate 1`.
- **NMR**: Bruker's `USERA1` to `USERA5` fields, then the first line of the title; JCAMP-DX `##TITLE`; JEOL's `sample_id`.
- **Microscopy** (CZI, ND2 and LIF only): the plate well of the scenes when all images share one, else the image name when every image has the same one and it looks like a sample id.

Software defaults are not used as sample ids: `Position 1`, `Series011`, `TileScan_002_Merging`, `Plate 1`, `Specimen_001`, file names, and placeholders such as `NA`, `-1` and `<user>`. A LIF project name describes the experiment, not one sample, so it becomes `method.name`.

### Terms and units

Terms come from a curated table of ontology ids and labels; no ontology files are shipped.

| where | vocabulary | examples |
| --- | --- | --- |
| `instrument.kind` | OBI devices | `OBI:0400169` microscope, `OBI:0400044` flow cytometer, `OBI:0000049` mass spectrometer, `OBI:0001058` microplate reader |
| `method.technique` | CHMO (chemistry), FBbi (imaging), OBI | `CHMO:0000524` LC-MS, `CHMO:0000593` 1H NMR, `CHMO:0000061` flow cytometry, `FBbi:00000246` fluorescence microscopy |
| `method.assay` | OBI assays | `OBI:0003097` LC-MS assay, `OBI:0000916` flow cytometry assay, `OBI:0002119` microscopy assay |
| `measurements[].terms` | PSI-MS, CHMO, FBbi | `MS:1000129` negative scan, `MS:1000073` ESI, `MS:1000579` MS1 spectrum, `FBbi:00000249` time lapse |

Units are written for people and carry their [UCUM](https://ucum.org) code: `µm` → `um`, `Å` → `Ao`, `µs` → `us`, `°C` → `Cel`, `µL` → `uL`, `ppm` → `[ppm]`, `m/z` → `{m/z}`, `cm⁻¹` → `/cm`, `mM` → `mmol/L`, `rpm` → `{rpm}`. Plate-reader units are UCUM annotations: `OD` → `{OD}`, `RFU` → `{RFU}`, `RLU` → `{RLU}`.

| ontology | version checked | licence | source |
| --- | --- | --- | --- |
| PSI-MS controlled vocabulary (`MS:`) | 4.2.2 | CC BY 3.0 | <https://github.com/HUPO-PSI/psi-ms-CV> |
| Chemical Methods Ontology (`CHMO:`) | 2026-05-28 | CC BY 4.0 | <https://github.com/rsc-ontologies/rsc-cmo> |
| Biological Imaging Methods Ontology (`FBbi:`) | 2026-06-25 | CC BY 4.0 | <https://github.com/CRBS/Biological_Imaging_Methods_Ontology> |
| Ontology for Biomedical Investigations (`OBI:`) | 2026-07-27 | CC BY 4.0 | <https://github.com/obi-ontology/obi> |
| Unified Code for Units of Measure (UCUM) | 2.2 | UCUM License (no charge, royalty-free) | <https://ucum.org/license> |

We checked each term id and label against the EBI Ontology Lookup Service for the release listed. Attribution is in the repository's `NOTICE` file.

### Asking in words

`info --view explain` opens with an *Experiment* paragraph: sample, technique, method and settings, operator and data length.

`info FILE --ask "QUESTION"` matches the question's words to topics and answers each from the model, naming the fields each answer came from:

```bash
openreadout info run.raw --ask "what was the gradient?"
```

| topic | words that select it |
| --- | --- |
| `sample` | sample, specimen, well, barcode, vial, tube, position |
| `channels` | channel, dye, stain, DAPI, GFP, wavelength, excitation, emission, filter, detector |
| `method` | gradient, oven, solvent, column, method, protocol, pulse program, settings, parameter |
| `run_length` | how long, duration, run length, run time |
| `polarity` | polarity, positive, negative, ion mode |
| `ms_levels` | MS level, MS/MS, MS2, tandem, fragment, precursor |
| `comment` | comment, note, remark, annotation |
| `instrument`, `operator`, `acquired`, `technique` | instrument or model; who or operator; when or date; technique or modality |

A question that matches no topic gets the technique, sample, method and run length.

MS1 and MS/MS scans are interleaved in a run, so "the first MS/MS scan" is not simply the first scan after the MS1 scans. `openreadout spectrum FILE --ms-level 2 --nth 1` returns it directly.

## OME-XML export

`export --format ome-tiff` writes OME-XML, as does the `OME/METADATA.ome.xml` of a multi-image OME-Zarr store. It validates against the OME 2016-06 schema. Lengths avoid non-ASCII characters so that Bio-Formats can read them: `PhysicalSizeX/Y/Z` carry no unit attribute (the schema default is µm), and stage and plane positions are written in nm.

| OME element | from |
| --- | --- |
| `Experiment` | `extra.experiment` |
| `Experimenter` | `extra.experimenter` |
| `Instrument/Microscope` | `instrument.manufacturer`, `instrument.model` |
| `Instrument/Laser` | a channel's `excitation_nm` when its acquisition mode names a laser technique (confocal, multiphoton, TIRF, STED) |
| `Instrument/GenericExcitationSource` | other channels' `excitation_nm` |
| `Instrument/Detector` | `instrument.detector`, then each other detector in `extra.channel_settings` |
| `Instrument/Objective` | `objective` |
| `Instrument/Filter` | each distinct detection band |
| `Image/@Name` | `name`, when the source has one |
| `Image/AcquisitionDate` | `acquired_at`, to the millisecond |
| `Image/ObjectiveSettings` | `objective.immersion`, `extra.refractive_index` |
| `Image/StageLabel` | `extra.stage_position_um`, else the first frame's stage position |
| `Pixels/@PhysicalSize*`, `@TimeIncrement` | `physical_size`, `time_increment_s`, scaled by the step of an even `--select` |
| `Channel` | name, fluor, wavelengths, colour, acquisition mode and contrast method |
| `Channel/DetectorSettings` | gain and binning from `extra.channel_settings` |
| `Plane` | per-frame records; `DeltaT` falls back to `time_increment_s × t` |
| `ROI` | `extra.rois`; coordinates are copied as stored and are not verified |
| `MapAnnotation` (namespace `openreadout.dev/normalized`) | what OME cannot hold exactly: the full-precision `acquired_at`, the acquisition software and version, an immersion outside the OME list, an acquisition-mode label OME cannot express |

`EmissionWavelength` is written only when the file records an emission wavelength; a band is not turned into one. Images with identical instruments, objectives, light sources, detectors and bands share one `Instrument`. `--embed-vendor` also stores the vendor metadata tree.

OpenReadout's own OME-TIFF and OME-Zarr readers read the `MapAnnotation` back, so a round trip keeps those values. Other OME readers see ordinary key-value annotations.

An export and read-back does not reproduce:

- `dimension_order`, which is always `XYCZT` in the export.
- The source's tiling (`mosaic`, tile sizes). The export holds the stitched image in its own tiles.
- `time_increment_s` where the source had only per-frame times. The OME-TIFF reader infers the mean step from the planes, so the read-back has one.
- Everything in `extra` except what the table above maps.

`compare` compares a source with its export and reports these differences; see [check](../reference/commands/check.md).

## Conformance

The corpus test `crates/openreadout-corpus-tests/tests/metadata.rs` checks every test file against the rules on this page. For the `experiment` block it checks that every value has a provenance entry, every term is in the curated table, every unit has its UCUM code, timestamps are ISO-8601, `duration_s` is positive and every measurement has a description. Maintainers run it as described in [docs/maintaining.md](https://github.com/openreadout/openreadout/blob/main/docs/maintaining.md).
