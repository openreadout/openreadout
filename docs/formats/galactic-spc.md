# Galactic / Thermo GRAMS SPC

SPC is the spectrum format of Galactic GRAMS (later Thermo), and many other programs write it, among them OMNIC, Agilent MicroLab, Digilab, Spectragryph, Renishaw WiRE and AIST-NT. OpenReadout returns one trace per file in which every subfile is a sweep (one spectrum), with the x axis, the quantities and units, and the header and log text. Big-endian files are refused. Derived from SpectroChemPy's `read_spc` (CeCILL-B) and spc-io (MIT), read as documentation, and checked on public files from several depositors with SpectroChemPy as the reference reader (spc-io where SpectroChemPy cannot decode a text field). Provenance: `docs/provenance/galactic-spc.md`.

Crate: `openreadout-spectro`, format id `galactic-spc` (`SPC_FORMAT_ID`), extension `spc`, reader
`SpcReader`; the dataset is the shared `SpectroDataset`.

## Detection

The header has no signature, only the version byte at offset 1 (0x4B new, 0x4D old, 0x4C big-endian). Only files with the `.spc` extension are claimed, and then only when the header is consistent: not a zip (`PK`, which also has 0x4B at byte 1), experiment code ≤ 20, a point count (or the XY-XY flag together with the multifile and x-values flags), at least one subfile, finite first/last x and known x/y type codes. Big-endian 0x4C `.spc` files are claimed and refused on open.

## Mapping

One trace per file; every subfile is a sweep (a spectrum). The x axis is `extra.axis` (evenly
spaced from the header's first/last x) or, when the file stores an x array (flag 0x80), a first
channel of x values. Multifiles add a table `subfiles` (`subfile_index`, `z`, `scans`, and `w` when
the file has W planes); `z` is the subfile time for ordered files, else first time + index × z
increment.

Trace `extra`: `technique` and `technique_code`, `x_type_code`, `y_type_code`, `z_type_code`,
`z_quantity`, `z_unit` (multifiles), `axis_labels` (flag 0x20), `value_encoding` (`float32`,
`fixed32`, `fixed16`, `fixed32-word-swapped`), `exponent` (when one exponent scales every
spectrum), `resolution`, `source_instrument`, `comment` (also the trace name), `method`,
`peak_point`, `modification_flags`, `recorded_at` (packed date, when plausible), `w_planes`,
`w_increment`, `z_increment`, `log` (the log block's `key=value` lines).

x codes → our quantity/unit: 1 wavenumber 1/cm, 2 wavelength µm, 3 wavelength nm, 4/5/11/12/23/24/
25/30 time, 6/7/8/26 frequency, 9 mass_to_charge, 10 chemical_shift ppm, 13 raman_shift 1/cm, 14
energy eV, 16 diode, 17 channel, 18 angle, 19–21 temperature, 22 points, 27–29 wavelength cm/m/mm.
y codes: 1 interferogram, 2 absorbance, 3 kubelka_munk, 4 counts, 5/9 voltage, 10 log_1_r,
11 percent, 12 intensity, 13 relative_intensity, 128 transmittance, 129 reflectance, 130
single_beam, 131 emission, others as SpectroChemPy names them.

## Layout (new format, byte 1 = 0x4B, little-endian)

| offset | type | meaning |
| --- | --- | --- |
| 0 | u8 | flags: 0x01 16-bit values, 0x04 multifile, 0x08 random z, 0x10 ordered z, 0x20 axis labels, 0x40 per-subfile x arrays, 0x80 x array |
| 2, 3 | u8, i8 | technique code, exponent (−128 = float32 values) |
| 4 | u32 | points |
| 8, 16 | f64, f64 | first and last x |
| 24 | u32 | subfiles |
| 28, 29, 30 | u8 × 3 | x, y, z type codes |
| 32 | u32 | packed date: minute 6 bits, hour 5, day 5, month 4, year 12 |
| 36, 45 | char[9] × 2 | resolution, source instrument |
| 54 | u16 | peak point |
| 88 | char[130] | comment |
| 218 | char[30] | axis labels (NUL separated) |
| 248, 252 | u32, u32 | log offset, modification flags |
| 264 | char[48] | method |
| 312, 316, 320 | f32, u32, f32 | z increment, W planes, W increment |

Then the x array (points × f32, flag 0x80), then per subfile a 32-byte header (flags, i8 exponent,
u16 index, f32 time, f32 next time, f32 noise, u32 points, u32 scans, f32 W level) and the values.
Values: float32, or fixed point: value = int32 × 2^(exponent − 32) (int16 × 2^(exponent − 16) with
flag 0x01). A single-subfile file is scaled by the main exponent (its subfile exponent is
ignored); each subfile of a multifile by its own exponent. The log block at the log offset: u32
disk size, memory size, text offset, binary and disk sizes, then the text (`key=value` lines, CR LF).

**Old format (0x4D):** 256-byte header: i16 exponent at 2, f32 points at 4, f32 first and last x at
8 and 12, x and y codes at 16 and 17, year (low 12 bits) at 18, month, day, hour, minute at 20–23,
resolution at 24, comment at 64; the first subfile header is inside the header at 224; values
start at 256 as int32 with their two 16-bit words most-significant first.

**Refused** (exit 6): 0x4C (big-endian) files, per-subfile x arrays (0x40), old-format multifiles,
16-bit or x-array old-format files, multifiles mixing float and fixed subfiles. Files whose
byte 1 is not 0x4B/0x4C/0x4D (Bruker EPR, Becker & Hickl, EDAX `.spc`) are not detected.

## `check` finding codes

`truncated` (error), `log_out_of_bounds`, `log_overlaps_data`, `subfile_count` (warnings).

## Vocabulary (public identifiers of the SPC module of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `SpcReader`, `SPC_FORMAT_ID` | the reader and its format id |
