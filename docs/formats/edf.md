# European Data Format: EDF, EDF+, BDF and BDF+ (`.edf`, `.bdf`)

EDF is the open format for EEG and polysomnography recordings (Kemp et al. 1992). EDF+ (Kemp and
Olivan 2003) adds annotations, discontinuous recordings and structured patient and recording
fields. BDF is BioSemi's 24-bit version of EDF, and BDF+ the 24-bit version of EDF+. One reader
handles all four. Sleep labs, clinical EEG systems and most research amplifiers can write EDF, and
BIDS accepts EDF and BDF for EEG.

Derived from the published specifications (`docs/provenance/edf.md`): the EDF and EDF+ pages at
edfplus.info, BioSemi's BDF page and the BDF+ description. pyedflib and MNE-Python (both BSD) are
run as the reference readers; their source was not read.

Crate: `openreadout-ephys`, format id `edf` (`EDF_FORMAT_ID`), extensions `edf` and `bdf`, reader
`EdfReader`, dataset `EdfDataset` (`open`, `open_input`, `edf`). Detection (`looks_like_edf`): the
version field is `0` followed by spaces (EDF) or byte 255 followed by `BIOSEMI` (BDF), and the
header length at byte 184 equals 256 × (signal count + 1) → definite. Other files with these
extensions are extension-only matches.

## Layout (`parse_edf` → `EdfFile`)

- **Fixed header**, 256 bytes of ASCII (`EDF_HEADER_BLOCK`): version (8; `version_field`, `bdf`),
  local patient identification (80; `patient`), local recording identification (80;
  `recording`), start date `dd.mm.yy` (8; `start_date`), start time `hh.mm.ss` (8; `start_time`),
  header bytes (8; `header_bytes`), reserved (44; `reserved`: `EDF+C`, `EDF+D`, `BDF+C`, `BDF+D`,
  BioSemi's `24BIT`, or blank), number of data records (8; `declared_records`, −1 while
  recording), duration of a data record in seconds (8; `record_duration_s`), number of signals
  (4; at most `MAX_EDF_SIGNALS`).
- **Signal headers** (`EdfSignal`), 256 bytes per signal, stored field by field for all signals:
  label (16; `label`), transducer type (80; `transducer`), physical dimension (8;
  `physical_dimension`), physical minimum and maximum (8 each; `physical_min`, `physical_max`),
  digital minimum and maximum (8 each; `digital_min`, `digital_max`), prefiltering (80;
  `prefiltering`), samples per data record (8; `samples_per_record`), reserved (32; `reserved`).
  A signal labelled `EDF Annotations` or `BDF Annotations` is an annotation signal
  (`annotations`).
- **Data records** follow the header at 256 × (signals + 1) (`data_offset`, `record_offset`). Each
  holds every signal's samples in header order (`offset_in_record`; `record_bytes` per record):
  2-byte little-endian two's complement integers in EDF, 3-byte in BDF (`sample_bytes`).
  `records` is the number of whole records in the file, at most the declared count.
- **Values**: physical = digital × `scale` + `offset`, with `scale` = (physical max − physical min)
  / (digital max − digital min) and `offset` = physical min − digital min × `scale`. A signal whose
  digital range is empty (`calibrated` false) keeps its digital values (scale 1, offset 0).
- **TALs** (time-stamped annotation lists, `parse_tal`) fill the annotation signals: `+onset`, an
  optional byte 21 and duration, byte 20, then texts each ended by byte 20, and a closing 0 byte.
  In EDF+ and BDF+ the first TAL of every data record is a time-keeping TAL whose first text is
  empty; its onset is the record's start after the header's start time. The first record's is
  the subsecond start (`start_offset_s`), and every time the reader reports counts from it, as
  pyedflib and MNE do: `record_starts` and `record_start` give each record's start after the
  first one's. Every other text is an annotation (`EdfAnnotation`: `onset_s` after the first
  record's start, `duration_s`, 0 when the TAL gives none, `text`). TAL times are parsed exactly
  to the nanosecond (`seconds_ns`) and subtracted before they become floating point. At most
  `MAX_EDF_ANNOTATION_BYTES` of annotation signal are read and `MAX_EDF_ANNOTATIONS` annotations
  kept.
- `variant` names the file: `EDF`, `EDF+C`, `EDF+D`, `BDF`, `BDF+C`, `BDF+D` (`plus` for the last
  four kinds with a `+`). `recorded_at` is the start date and time plus `start_offset_s`; the year comes from the EDF+
  `Startdate dd-MMM-yyyy` subfield when the recording field has one (`recording_subfields`), else
  from `yy` with 1985 as the clipping year.

## Mapping

- **Traces**: ordinary signals that share `samples_per_record` (so a sample rate) form one trace
  (`groups`), in the order their first signal appears; a file with one rate has one trace named
  `signals`, otherwise each is `signals at <rate> Hz`. `sample_rate_hz` = samples per record /
  record duration (`rate_hz`). Channels keep the signal labels and units; dtype `int16` (EDF) or
  `int24` (BDF). Channel `extra`: `signal` (header index), `physical_min`, `physical_max`,
  `digital_min`, `digital_max`, `transducer`, `prefiltering`.
- **Sweeps**: one, except in `EDF+D` / `BDF+D` files, where runs of data records that follow each
  other within `EDF_CONTIGUOUS_TOLERANCE_S` are sweeps (`sweeps`); `extra.sweep_starts_s` and
  `extra.sweep_sample_counts` then give each run's start and length. `start_s` is 0: the
  recording starts at the first sample.
- Trace `extra`: `samples_per_record`, `record_duration_s`, `data_records`; on trace 0
  `recorded_at` and the EDF+ recording subfields `admin_code`, `technician` and `equipment`
  (`X`, meaning unknown, is left out).
- **Table** `annotations` (when the file has any): `onset_s`, `duration_s` (seconds after the
  first sample) and `text`, a code into the column's `extra.categories`.
- `format_version` is the variant. The patient field is only in the vendor metadata.
- Reads are capped at `MAX_EDF_READ` samples per channel and `MAX_EDF_READ_BYTES` of records.

## `check` finding codes

`truncated`, `bad_range`, `bad_digital_range`, `bad_physical_range` (errors); `header_bytes`,
`records_unknown`, `extra_bytes`, `annotations_capped`, `missing_time_keeping`, `bad_tal`,
`record_time_gap`, `digital_range_exceeds_sample_size`, `no_annotation_signal` (warnings).

## Oracle mapping

`oracle/gen.py` `edf()`: pyedflib reads the digital samples of every signal and the annotations;
MNE-Python reads the header (sample counts per record, the physical and digital extremes, units).
Physical values are the pyedflib digital samples × MNE's scale + offset, by the formula above, in
float64; pyedflib's and MNE's own physical values are checked against them to 1e-9.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/edf/` must appear here)

| identifier | meaning |
| --- | --- |
| `EdfReader`, `EDF_FORMAT_ID`, `EdfDataset`, `open`, `open_input`, `edf` | entry points |
| `EdfFile`, `parse_edf`, `bdf`, `version_field`, `patient`, `recording`, `start_date`, `start_time`, `header_bytes`, `reserved`, `declared_records`, `records`, `record_duration_s`, `signals`, `record_bytes`, `file_len`, `record_starts`, `annotations`, `findings` | the file |
| `variant`, `plus`, `sample_bytes`, `data_offset`, `record_offset`, `rate_hz`, `groups`, `sweeps`, `record_start`, `recorded_at`, `recording_subfields` | derived from the header |
| `EdfSignal`, `label`, `transducer`, `physical_dimension`, `physical_min`, `physical_max`, `digital_min`, `digital_max`, `prefiltering`, `samples_per_record`, `offset_in_record`, `scale`, `offset`, `calibrated` | a signal |
| `EdfAnnotation`, `onset_s`, `duration_s`, `text`, `parse_tal`, `seconds_ns`, `start_offset_s` | annotations and times |
| `EDF_HEADER_BLOCK`, `MAX_EDF_SIGNALS`, `MAX_EDF_ANNOTATION_BYTES`, `MAX_EDF_ANNOTATIONS`, `MAX_EDF_READ`, `MAX_EDF_READ_BYTES`, `EDF_CONTIGUOUS_TOLERANCE_S`, `looks_like_edf` | constants, limits, detection |
