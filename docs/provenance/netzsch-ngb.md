# Provenance: NETZSCH Proteus `.ngb-*` measurement files

## 2026-09-26: initial reader (`netzsch-ngb`)

**Corpus files used (development):** the 11 test files of pyNGB (`tests/test_files/`, MIT: STA
449 F3 `.ngb-ss3` sample runs, `.ngb-bs3` correction runs, STA 449 F3A `.ngb-ds3` "sample +
correction" files, DIL 402 Expedis `.ngb-dla`/`.ngb-cla` dilatometer files) and the DSC `.ngb-sd7`
files of Zenodo 18902844 (Paszkiewicz, Irska, Walkowiak; CC-BY-4.0) with the depositors' Proteus
`ExpDat` exports of the same measurements.

Held out (never opened while developing): Zenodo 15845146 (Krause; DSC `.ngb-sd7` with `ExpDat`
text exports), a third depositor.

**Prior art consulted:** pyNGB 0.6.0 (<https://github.com/GraysonBellamy/pyngb>, MIT, Grayson
Bellamy), read as documentation (`format/container.py`, `format/grammar.py`,
`format/document.py`, `format/channels.py`, `format/maps.py`, `constants.py`) and run as a black
box for ground truth:

- the container: a zip whose `Streams/stream_N.table` members each start with `Netzsch TA file`
  at offset 2 and `_db_format_1` at 28, a section directory at 0x50 (14-byte entries `ff ff`, id,
  offset, size, 2 pad bytes) whose sections are contiguous and end at the end of the member;
- the record grammar: `18 fc ff ff 03 80 01`, a `u16` field id, `00 00 01 00 00 00 0c 00`,
  `17 fc ff ff`, a type byte, then a scalar (`80 01`) or an array (`a0 01`, `u32` element count),
  ended by `01 00 00 00 02 00 01 00 00` (or the two variant endings); table opens are
  reference-typed records ending `02 00 00 80 <type u16> 00 00`;
- the data streams 2 and 3: a channel-header table (type 0x2B22; the low byte of its category
  names the channel) followed by one value table per program segment (type 0x2B23) holding one
  array (field 0x0F40 float64 or 0x0F3D float32); arrays concatenate in stream order; a channel
  header that repeats starts a second run (the correction of a "sample + correction" file);
- the channel ids (0x8C time in minutes, 0x8D sample temperature, 0x8E DSC, 0x90 mass, 0x9C-0x9E
  gas flows, 0x8F length change, 0x4E/0x4F force, 0x30-0x38 stream-3 furnace channels), their
  units (pyNGB's column table), and the stream-1 metadata fields (instrument, operator, lab,
  project, date performed as Unix seconds, sample name, id, material, mass, crucible, furnace,
  carrier, comment).

**What we checked ourselves:** on every development file the container directory, the grammar
(no malformed or truncated span in streams 2 and 3) and the channel lengths; against the
Paszkiewicz exports, which Proteus resamples at every kelvin and converts to mW/mg with its
sensitivity calibration, the sample temperature and time at the exported points and the DSC
signal as export × sensitivity × sample mass (the export's sensitivity column), within the
exports' printed precision.
