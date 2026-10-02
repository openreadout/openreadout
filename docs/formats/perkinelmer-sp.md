# PerkinElmer `.sp`

PerkinElmer Spectrum software (Spectrum One, Frontier, Spectrum 3, also with a Spotlight microscope) saves a single IR or UV/Vis spectrum as a `.sp` file. OpenReadout returns it as one trace with one sweep, with the instrument, detector, source, resolution and scan settings in the trace's `extra`.

Derived from specio's `.sp` reader (BSD-3-Clause, read as prior art and run as a reference reader) and hex dumps of five public files. Provenance: `docs/provenance/perkinelmer-sp.md`. Format id `perkinelmer-sp`, family `spectroscopy`, crate `openreadout-spectro` (`PeSpReader`, `PESP_FORMAT_ID`).

**Detection:** the file starts with `PEPE`, followed by a 40-byte description (`2D constant interval DataSet file`).

## Layout

From byte 44: blocks of a u16 id, an int32 body length and the body. A body is either nested blocks or one **typed member**: a u16 type tag, then the value — 0x7523 text (u16 length), 0x751b float64, 0x751c float64 followed by a validity flag (0x7524, u16), 0x751d two float64, 0x752b and 0x751a uint32, 0x752c uint16 followed by a validity flag, 0x7515 uint16, 0x7516 a float64 array (u32 byte length). The data set is block 120; its members:

| id | member | our use |
| --- | --- | --- |
| 35698 | two float64 | x of the first and last point |
| 35699 | two float64 | y minimum and maximum |
| 35700 | float64 | data interval (checked against the range) |
| 35701 | uint32 | number of points |
| 35703 | text | x units (`cm-1` → `wavenumber`, `nm` → `wavelength`) |
| 35704 | text | y units (`A` → `absorbance` in AU, `%T` → `transmittance` in %, `%R` → `reflectance` in %, `KM` → `kubelka_munk`; others `intensity` with the text as unit) |
| 35707 | text | spectrum kind (`Spectrum`) |
| 35713 | text | data set name |
| 35709 | text | path the file was saved to |
| 35708 | float64 array | the values |
| 35711 | nested | history records (block 121: user 35698, operation 35699, date 35700, arguments 35701, description 35702, the previous record under 35703 — each processing step adds a level, so the chain may nest dozens of blocks deep; blocks are read to depth 256) and the instrument record (block 123: model 35837, serial 35838, firmware 35839; settings 35840–35882), under the oldest history record |

The instrument settings are named from their values across the corpus (our reading; specio reads the same texts by position): 35840 scans, 35841 detector, 35842 source, 35843 beamsplitter, 35844 resolution (cm⁻¹, flagged float64), 35845 apodization (`Strong`, `Filler`), 35846 spectrum type, 35847 beam type, 35849 phase correction, 35854 accessory, 35882 laser wavenumber (15798 cm⁻¹, HeNe).

## Data model

One trace, one sweep, one channel (float64): x evenly from the first to the last value over the points.

### `traces[].extra` (our vocabulary)

| key | from |
| --- | --- |
| `axis`, `data_type` (`INFRARED SPECTRUM`, `UV/VIS SPECTRUM` for nm), `y_quantity` | 35698, 35701, 35703, 35704 |
| `title`, `spectrum_kind` | 35713, 35707 |
| `instrument`, `instrument_serial`, `instrument_firmware` | 35837–35839 |
| `detector`, `source`, `beamsplitter`, `apodization`, `spectrum_type`, `beam_type`, `phase_correction`, `accessory` | settings (above) |
| `scans`, `resolution_cm1`, `laser_wavenumber_cm1` | 35840, 35844, 35882 |
| `data_interval` | 35700 |
| `acquired_at`, `operator` | the history record whose operation is `Created as New Dataset` (else the oldest): its date (`Thu Mar 09 09:19:21 2006`, with `(GMT+1:00)` when recorded) and user |
| `x_units_text`, `y_units_text` | 35703, 35704 |

**Experiment.** `sample.id` = the data set name, instrument (PerkinElmer, model, serial), `method.parameters`: `resolution`, `scans`, `laser_wavenumber`, `detector`, `source`, `beamsplitter`, `apodization`, `accessory`; `acquisition.started_at`, `operator`, `comment` (the history record's description, e.g. the microscope position).

**Vendor tree.** `info --view full` holds the description, the saved path, every history record and the whole block tree with decoded members by id.

## Validation

**specio 0.1.0** (BSD-3) on the 5 corpus files: every value bit for bit and the x-axis ends (specio 0.1 needs `collections.Iterable` restored on Python ≥ 3.10, and its instrument-text step fails on Latin-1 bytes; the oracle makes only that metadata step lenient). No vendor export of these files is public.

## Known gaps

One spectrum per file; PerkinElmer Spotlight images (`.fsm`, a `PEPE` file whose description names a 4-D data set) are read by the `perkinelmer-fsm` reader (`docs/formats/perkinelmer-fsm.md`) and not claimed here. UV-Vis `.sp` files (Lambda instruments, x in nm) follow the same layout by inference only: no public file was available. Settings other than those named above are in the vendor tree by member id.

## Vocabulary (public API of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `PeSpReader` | the reader (`FormatReader`): detection by the first bytes, `open`/`open_input` |
| `PESP_FORMAT_ID` | the format id, `perkinelmer-sp` |
| `SpectroDataset` | an opened file (the `Dataset` the four spectroscopy readers share): traces, tables, map images and attachments read lazily |

Everything else — trace names, channel names, `extra` keys and their values — is listed in the tables above in our own words.
