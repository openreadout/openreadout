# Bug hunt on new files: signals, spectra and bench instruments (2026-10-06)

We collected public files the project had never seen and ran every reader, analysis and export
on them. The families were electrophysiology, NMR, optical spectroscopy, flow cytometry, plate
readers and qPCR, and bench instruments. This page lists what we surveyed, what failed, what we
fixed and what is left.

## What was surveyed

We downloaded 242 files from 163 source records. Most records were new to the corpus; a few files
came from records it already uses (pyABF's test data, the NeuralEnsemble test data on G-Node GIN)
when they added a variant the corpus lacked. The rest came from Zenodo, figshare, DANDI and
nmrXiv. About 3 GB came down (2.1 GB of files and 0.85 GB of nmrXiv study archives), under the
8 GB budget. We checked every record against the held-out set
with a port of `cargo xtask heldout-check`'s record keys before downloading, and opened no
held-out file. One GIN `.pl2` file had the same SHA-256 as a held-out file from another record,
so we deleted it unopened.

**211 files from 145 records joined the development corpus** (336 manifest entries with
session parts and bundles; 97 on the smoke tier). We left out the rest:

| left out | why |
| --- | --- |
| 11 BMRB metabolomics experiments (TopSpin 2.1–4.1, XWIN-NMR 2.6) | the BMRB pages state no licence; surveyed locally only |
| 5 Bio-Rad `.pcrd` | encrypted; refused with a correct hint (exit 6) |
| 2 FlowJo workspaces | their FCS files are not public alongside them |
| 2 Shimadzu UVProbe `.spc` (OLE compound files) | a format OpenReadout does not read (exit 3) |
| 2 `.wdf`, 1 `.esp`, 1 Mettler TGA `.txt`, 2 Bruker `fid` without `acqus`, 1 EDAX `.spc` | not the format their extension suggests, or not a supported format; correctly refused (exit 3 or 4) |
| 1 X-ray `.raw` starting `FI` | an unidentified diffractometer format (exit 3) |
| 2 Rigaku `.ras`, 1 JASCO CD `.jws` | open findings below |

| family | inputs added | formats |
| --- | --- | --- |
| electrophysiology | 101 | ABF 44 (ABF 1.83–2.9, Clampex 9–11, gap-free, episodic, event-driven), NWB 11 (icephys and ecephys, NWB 2.2–2.9), Blackrock 9, Neuralynx 8, Spike2 7, Plexon 5, Intan 5, ATF 4, PatchMaster 4, SpikeGLX 4 |
| NMR | 35 | Bruker 12 (TopSpin 3.2–4.5, III HD and NEO consoles, solid-state MAS, DOSY, ROESY), JEOL 11, JCAMP-DX 11, Varian 1 |
| optical spectroscopy | 29 | OPUS 10, PerkinElmer `.sp` 7 (FTIR, UV-Vis, fluorescence, and three saved as text), OMNIC 5, JASCO 2, WITec 2, SPC 1 |
| flow cytometry | 13 | FCS from LSRFortessa, FACSAria II/III, FACSVerse, Attune NxT, CytoFLEX, NovoCyte, MA900, Aurora |
| bench instruments | 25 | EC-Lab `.mpr` 8 and `.mpt` 5, qPCR `.eds` 5, Biacore `.bme` 2, XRD 6 (XRDML, BRML, RAW, RAS), EPR BES3T 4 |
| plate readers | 5 | Tecan Spark, BMG CLARIOstar and Gen5 workbooks |

## How we ran them

Each file went through `info`, `check`, `trace` or `table`, the family's analysis
(`analyze ephys-features` and `analyze spikes`, `nmr-peaks`, `peaks`, `qpcr`, `assay-wells`,
`gate`) and an export (NWB, JCAMP-DX, RDML or CSV). We recorded the exit code, wall time and peak
memory of each step (`target/hunt/survey.py` during the hunt). Ground truth came from the
readers the corpus already uses: pyABF, Neo, h5py, nmrglue, specio, SpectroChemPy, brukeropus,
FlowIO and fcsparser, rdmlpython and the vendor result files of `.eds`, galvani, geddes, DeerLab's
`deerload`, witio, jws2txt and allotropy. GPL readers were run as black boxes only.

On the corpus test after the fixes, 189 of the inputs agree with their oracle (184 with an
independent reader, 5 with a second implementation). One is compared but skipped with a reason
(below), and 21 have no oracle, mostly because the oracle itself fails on the file. No intact
file made a command panic or hang. The slowest step was `analyze spikes` on a 234 MB `.ns6`
(21 s).

## What failed, and what we did

| finding | files | before | after |
| --- | --- | --- | --- |
| `analyze ephys-features` refused every NWB intracellular series: NWB spells its units `volts` and `amperes` | DANDI 001544, 001475, 001746, 001938 | exit 6 "a trace with no voltage or current channel" | read; all 16 current-clamp series of `dandi001544-icephys-cc328` give the same 82 spikes as eFEL, peaks at the same sample (new corpus test `nwb_current_clamp_matches_efel`) |
| ABF: `info` listed a command trace that could not be read, because the epoch table runs past the end of the sweep; `export --format nwb` failed on it | 2 GIN ABF 2 files | trace 1 listed, reading it exit 6, NWB export exit 6 | the DAC is refused up front with its reason in `extra.not_synthesized`; recorded channels agree with pyABF |
| Blackrock PTP: `info` read only the first and last timestamps, so a jump forward and a clock reset of the same size looked like one gap-free sweep, called `validated` | `gin-ephy-testing-data-blackrock-ptp-missing-samples` | `info` 1 sweep, `check` 7 | `info` reads every timestamp up to 64 MiB of packets (7 sweeps); larger files are probed and the sweep layout is reported as assumed |
| PerkinElmer `.sp` saved as text (`PE FL … ASCII PEDS 1.60`) was called corrupt | 3 figshare 3841308 files (LS55) | exit 4 "may be truncated" | read, with the `#GR` header checked against the data; compared with a standard-library read of the same pairs |
| `analyze spikes` and `trace` held every channel of a long recording in memory | `figshare30728969-ns6-50mw002` (65 channels, 234 MB) | 1.7 GB and 830 MB | 1.0 GB and 190 MB, same spikes |
| the spike-detection refusal printed `300–45.00000000000096 Hz band-pass at 100.00000000000213 Hz` | 2 files sampled at 100 Hz | | numbers rounded to 4 significant digits |

The touched fuzz targets ran for 10 minutes each (`whole_abf`, `whole_blackrock`, `whole_pesp`,
`signal_analysis`). `whole_abf` ran out of memory on a header that declares millions of sweeps,
because the structure entries listed each one; they now list 10,000 and sum up the rest, and the
input is a regression fixture. A second 10-minute run of `whole_abf` after the fix, and the other
three targets, found nothing.

Two oracle problems were fixed in `oracle/gen.py`: the ABF oracle now leaves out a DAC whose
epochs overrun a sweep (pyABF raises a broadcast error building `sweepC`), and the Plexon oracle
gives a file without spikes no waveform column.

The Plexon file `…-two-intervals-with-continuous-gaps-…` is compared with `oracle_skip`. Neo
joins its two recording intervals into one 2680-sample segment and zero-fills the last 10 samples
of the final block; OpenReadout keeps two sweeps (1410 and 1270 samples) split at the gap between
the Stop and Start events. Every other sample of the channels we checked is equal.

## What is left

These need a file we may use for development, or belong to another workstream.

- **Gen5 workbooks (plate reader).** In Gen5's Excel export the kinetic `Time` table starts in
  column B, as its matrices do. The reader looks for `Time` in column A only, so
  `zenodo4449746-gen5-synergy-htx` reads no values (the file is `unvalidated`, "plate values
  present but not decoded", and `check` exits 4 with `no_plate_data`). The reader also reads only
  the first worksheet of a workbook and says nothing about the others; this depositor's workbook
  holds seven Gen5 exports, one per sheet. We found no clean public Gen5 Excel kinetic export to
  validate a fix against, and a note for unread worksheets would change the assurance of every
  multi-sheet development workbook, so both are left for a separate change.
- **Rigaku `.ras` edited by hand** (Zenodo 21511646). One file has no `*RAS_INT_END` although all
  2251 points that `*MEAS_DATA_COUNT` declares are present; the other has data rows commented out
  with `#`. Both exit 4. Reading the first would be reasonable, but the RAS reader belongs to the
  gaps workstream this week.
- **JASCO circular dichroism `.jws`** (figshare 13601282) is refused with exit 6, "a flat JASCO
  container `SPECMAN R2.0.0` with an unexpected axis descriptor". JASCO channels belong to the gaps
  workstream.
- **Shimadzu UVProbe `.spc`** (OLE compound files with a `DataStorage1/DataSpectrumStorage`
  tree, two records) exit 3 with the generic unknown-format hint. A hint that names the format
  would help users; reading it is a new reader.
- **Oracles that fail on new files.** geddes cannot parse RAS headers whose point count has a
  decimal point (`"7999.0000000000"`), so two RAS files are compared with a second
  implementation only. DeerLab's `deerload` rejects an `IRFMT` value, galvani an unknown column id
  185, and nmrglue a Latin-1 `acqus`. Neo refuses a Blackrock PTP file with gaps, a spec 2.1 NSx
  without its NEV, a Neuralynx file without a date header and a Plexon file with timestamps but no
  waveforms. These files are read but not compared.
- **No independent reader** exists for the five EC-Lab `.mpt` exports without their `.mpr`, the
  three Gen5, Spark and CLARIOstar workbooks, or the text `.sp` form.
- **NWB export memory.** Exporting a 70-million-sample `.ns2` to NWB peaked at 1.7 GB; the export
  holds up to 67 million samples in memory by design (larger ones are refused with a hint).
- **Assurance evidence** was not refreshed over the whole development corpus in this change; the
  new files count as evidence after the next `cargo xtask assurance-audit refresh`.
