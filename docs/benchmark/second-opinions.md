# Second opinions on normalized fields: every family

Measured 2026-09-26 on the development inputs of every family (no held-out file): each field a
second reader or the vendor's own export also reports is set against ours — units, scaling,
channel names, sample, operator and instrument strings, timestamps with their time zones, counts
and, where the second reader decodes them, values. The primary oracles (`corpus/oracle/`) check
decoding; these checks cover the fields they do not, above all the two fields assurance tracks:
**acquisition time** (`experiment.acquisition.started_at`) and **instrument model**
(`experiment.instrument.model`).

How it works (`crates/openreadout-corpus-tests/tests/second_fields.rs`):

- A plugin per family (`oracle/second_fields/<family>.py`) runs the second reader and writes one
  record per file, `corpus/oracle/second/fields/<id>.json`: a list of checks, each a field name,
  the value the second reader gives, the path of the same value in our `openreadout <cmd> <file>
  --json` output, and a comparator (`exact`, `num` with tolerances, `text`, `loose` letters and
  digits only, `contains`, `set`, and for times `time` (the same instant when both carry a zone,
  else the same clock; `zone` also requires the same offset), `wallclock`, `time_or_utc` (a
  reader that writes the local clock or UTC without saying which: the same wall clock or ours in
  UTC) and `time_mod_zone` (a reader that writes a converter-local clock: equal up to a whole
  number of quarter hours)). A new reader plugs in by adding a module with `FAMILY`, `FORMATS` and
  `run(rec, entry, path, ctx)`.
- The test runs the CLI once per file and command and compares. A difference fails the test
  unless `corpus/oracle/second/adjudications.toml` adjudicates it (`right = "openreadout"`,
  `"second"` or `"neither"`, with the evidence); a value only the second reader has is a gap
  (listed, not failed).
- With `SECOND_RESULTS` the test writes one line per file for `cargo xtask assurance-audit
  refresh`: the outputs compared and the tracked fields the second reader **agreed** on
  (`fields`); an adjudicated difference lets the file pass but confirms no field. A difference
  adjudicated against us, or left open, fails the file.

Second readers never read our output. The GPL ones (Bio-Formats, chromConverter, galvani) and
SpectroChemPy run as black boxes.

Rerun:

```bash
cd oracle && OPENREADOUT_CORPUS_DIR=../corpus/files CHROMCONVERTER_LIB=<R library> \
  .venv/bin/python -m second_fields [--family ms|flow|ephys|nmr|vib|chrom|plate|qpcr|bench|images]
SECOND_REPORT=/tmp/second.jsonl SECOND_RESULTS=/tmp/second-results.jsonl \
  cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test second_fields
python oracle/second_report.py /tmp/second.jsonl     # the tables below
```

## Agreement by family

"Ours right" / "theirs right" / "neither" count adjudicated differences; "time confirmed" and
"model confirmed" count files whose acquisition time or instrument model a second reader agreed
with (the evidence `--strict` needs to stop withholding those fields; the counts include files
whose value comes from a documented field, which assurance does not track). In the assurance
evidence (`corpus/assurance/evidence.json`, all oracles together) the acquisition time is now
confirmed on 561 of the 1043 development files that hold an undocumented one (85 before these
checks) and the instrument model on 173 of 646 (14 before).

<!-- BEGIN GENERATED -->

All families: 775 files, 6344 checks, 6114 agree, 123 adjudicated differences (123 ours right, 0 theirs right, 0 neither), 0 unadjudicated, 107 gaps; acquisition time confirmed on 594 files, instrument model on 217.

### Mass spectrometry

Second readers: Python sqlite3, olefile 0.47, pyopenms 3.5.0, the .d's AcqData XML, the .raw's _HEADER.TXT, the file header, read with lxml/regular expressions, the vendor-library conversion.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `agilent-masshunter` | 16 | 144 | 138 | 6 | 0 | 0 | 0 | 0 | 16 | 6 |
| `bruker-tdf` | 7 | 97 | 97 | 0 | 0 | 0 | 0 | 0 | 7 | 7 |
| `mzml` | 12 | 150 | 150 | 0 | 0 | 0 | 0 | 0 | 8 | 11 |
| `mzmlb` | 4 | 47 | 47 | 0 | 0 | 0 | 0 | 0 | 1 | 4 |
| `mzxml` | 2 | 24 | 24 | 0 | 0 | 0 | 0 | 0 | 0 | 2 |
| `sciex-wiff` | 6 | 41 | 32 | 9 | 0 | 0 | 0 | 0 | 6 | 2 |
| `thermo-raw` | 28 | 349 | 332 | 17 | 0 | 0 | 0 | 0 | 24 | 15 |
| `waters-raw` | 18 | 178 | 169 | 5 | 0 | 0 | 0 | 4 | 16 | 16 |

### Chromatography

Second readers: chromConverter 0.9.0, scipy 1.18.1 netcdf_file: the file's global attributes.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `andi-chrom` | 24 | 47 | 47 | 0 | 0 | 0 | 0 | 0 | 24 | 0 |
| `chemstation` | 14 | 459 | 422 | 36 | 0 | 0 | 0 | 1 | 10 | 5 |
| `empower-arw` | 6 | 72 | 72 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `shimadzu` | 3 | 38 | 11 | 23 | 0 | 0 | 0 | 4 | 0 | 0 |

### Flow cytometry

Second readers: FlowKit 1.3.2, fcsparser 0.2.4.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `fcs` | 38 | 852 | 845 | 0 | 0 | 0 | 0 | 7 | 35 | 35 |

### Electrophysiology

Second readers: Neo 0.14.5, Neo 0.14.5 AxonRawIO, Python csv module on the ATF text, nept 0.1.0, pyABF 2.3.8, pynwb 4.2.0, the .meta text and NumPy int16 samples.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `abf` | 36 | 1000 | 996 | 4 | 0 | 0 | 0 | 0 | 34 | 0 |
| `atf` | 7 | 127 | 127 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `blackrock` | 10 | 10 | 7 | 2 | 0 | 0 | 0 | 1 | 7 | 0 |
| `neuralynx` | 20 | 83 | 79 | 4 | 0 | 0 | 0 | 0 | 20 | 0 |
| `nwb` | 11 | 302 | 300 | 0 | 0 | 0 | 0 | 2 | 11 | 0 |
| `plexon` | 4 | 4 | 4 | 0 | 0 | 0 | 0 | 0 | 4 | 0 |
| `spikeglx` | 13 | 134 | 132 | 0 | 0 | 0 | 0 | 2 | 12 | 0 |

### NMR

Second readers: jcamp 1.3.2, nmrglue 0.12, the .jdf context section.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `bruker-nmr` | 11 | 254 | 233 | 0 | 0 | 0 | 0 | 21 | 11 | 0 |
| `jcamp-dx` | 18 | 173 | 170 | 1 | 0 | 0 | 0 | 2 | 3 | 13 |
| `jeol-jdf` | 14 | 230 | 229 | 1 | 0 | 0 | 0 | 0 | 12 | 0 |
| `varian-nmr` | 11 | 135 | 135 | 0 | 0 | 0 | 0 | 0 | 11 | 0 |

### Vibrational spectroscopy

Second readers: OMNIC's CSV export of the same spectrum, SpectroChemPy 1.0.0, brukeropusreader 1.3, spc_io 0.2.1.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `bruker-opus` | 16 | 385 | 382 | 2 | 0 | 0 | 0 | 1 | 13 | 0 |
| `galactic-spc` | 6 | 6 | 6 | 0 | 0 | 0 | 0 | 0 | 6 | 0 |
| `thermo-omnic` | 21 | 85 | 85 | 0 | 0 | 0 | 0 | 0 | 21 | 0 |

### Plate readers

Second readers: allotropy 0.1.146, an independent plate-grid scanner, the SoftMax Pro text export's footer.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `plate` | 30 | 54 | 50 | 4 | 0 | 0 | 0 | 0 | 21 | 0 |

### Real-time PCR

Second readers: qslib 0.15.3, the RDML XML read with Python's ElementTree.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `applied-biosystems-eds` | 6 | 15 | 15 | 0 | 0 | 0 | 0 | 0 | 6 | 0 |
| `rdml` | 4 | 12 | 11 | 1 | 0 | 0 | 0 | 0 | 2 | 3 |

### Bench instruments (EPR, XRD, electrochemistry, thermal, FPLC, SPR, ITC, microarrays)

Second readers: DeerLab deerload, NewareNDA 2026.6.11, allotropy 0.1.146, galvani 0.5.0, gamry-parser 0.4.6, pyNGB 0.6.0, the .brml XML members, read with regular expressions, the .gpr ATF header records, the .itc file's first `%` line, read as text, the .ras text header, the .rasx MesurementConditions XML, the .xrdml XML, read with ElementTree, the EC-Lab export of the same run, the file's text header, read with regular expressions.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `biologic-mpr` | 19 | 35 | 19 | 0 | 0 | 0 | 0 | 16 | 19 | 0 |
| `biologic-mpt` | 16 | 32 | 32 | 0 | 0 | 0 | 0 | 0 | 16 | 16 |
| `bruker-bes3t` | 14 | 14 | 12 | 0 | 0 | 0 | 0 | 2 | 12 | 0 |
| `bruker-brml` | 4 | 7 | 7 | 0 | 0 | 0 | 0 | 0 | 4 | 3 |
| `cytiva-biacore-blr` | 9 | 18 | 18 | 0 | 0 | 0 | 0 | 0 | 9 | 9 |
| `cytiva-unicorn-zip` | 11 | 22 | 21 | 0 | 0 | 0 | 0 | 1 | 10 | 11 |
| `gamry-dta` | 5 | 9 | 9 | 0 | 0 | 0 | 0 | 0 | 5 | 0 |
| `genepix-gpr` | 5 | 10 | 10 | 0 | 0 | 0 | 0 | 0 | 5 | 5 |
| `microcal-itc` | 10 | 10 | 10 | 0 | 0 | 0 | 0 | 0 | 0 | 10 |
| `netzsch-ngb` | 18 | 36 | 36 | 0 | 0 | 0 | 0 | 0 | 18 | 18 |
| `neware-nda` | 5 | 5 | 5 | 0 | 0 | 0 | 0 | 0 | 5 | 0 |
| `neware-ndax` | 4 | 4 | 4 | 0 | 0 | 0 | 0 | 0 | 4 | 0 |
| `panalytical-xrdml` | 7 | 14 | 14 | 0 | 0 | 0 | 0 | 0 | 7 | 7 |
| `rigaku-ras` | 4 | 5 | 5 | 0 | 0 | 0 | 0 | 0 | 4 | 1 |
| `rigaku-rasx` | 2 | 4 | 4 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |

### Image formats (metadata)

Second readers: Bio-Formats 8.5.0, h5py 3.16.0, nd2 0.11.3, oiffile 2026.2.8.

| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `biorad-scn` | 13 | 38 | 38 | 0 | 0 | 0 | 0 | 0 | 12 | 0 |
| `dcimg` | 15 | 30 | 30 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `dm` | 5 | 10 | 8 | 2 | 0 | 0 | 0 | 0 | 0 | 0 |
| `imagexpress` | 1 | 3 | 2 | 0 | 0 | 0 | 0 | 1 | 0 | 0 |
| `ims` | 6 | 18 | 18 | 0 | 0 | 0 | 0 | 0 | 6 | 0 |
| `lif` | 17 | 57 | 51 | 6 | 0 | 0 | 0 | 0 | 16 | 7 |
| `mrc` | 8 | 16 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `nd2` | 28 | 71 | 71 | 0 | 0 | 0 | 0 | 0 | 15 | 0 |
| `oib` | 7 | 28 | 28 | 0 | 0 | 0 | 0 | 0 | 7 | 7 |
| `oif` | 2 | 8 | 8 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| `oir` | 15 | 45 | 45 | 0 | 0 | 0 | 0 | 0 | 15 | 0 |
| `opera-harmony` | 8 | 24 | 24 | 0 | 0 | 0 | 0 | 0 | 8 | 0 |
| `tiff` | 59 | 154 | 149 | 0 | 0 | 0 | 0 | 5 | 31 | 0 |
| `vsi` | 27 | 70 | 33 | 0 | 0 | 0 | 0 | 37 | 11 | 0 |
| `zvi` | 15 | 40 | 40 | 0 | 0 | 0 | 0 | 0 | 10 | 0 |

<!-- END GENERATED -->

## Where ours was wrong (fixed)

| format | field | what the second opinion showed | fix |
| --- | --- | --- | --- |
| `sciex-wiff` | acquisition time | 2–9 h off on every file: the stored u32 is the acquisition computer's local clock, not UTC (olefile's storage creation times, UTC, are minutes before it) | zoned from the compound file's UTC storage times, rounded to a quarter hour (`docs/provenance/sciex-wiff.md`) |
| `thermo-raw` | instrument model | Orbitrap Exploris files reported the site label the instrument name carries | the model string when the name carries a label (`docs/provenance/thermo-raw.md`) |
| `nwb` | acquisition time, operator, comment, sample | pynwb reads session start, experimenter, description and subject id; ours left them out of the experiment | filled from the NWB fields (`Source::Spec`; `docs/provenance/hdf5.md`) |
| `chemstation` | instrument model | chromConverter and ours both said `GCI` on every version-179/181 GC file from three laboratories, and on an LC VWD file; `Result.xml` names the instruments (`Agilent 6890 GC`, `Agilent 7890A`) | `GCI` is not taken as a model; `Result.xml` modules give model and serial (`docs/provenance/chemstation.md`) |
| `chemstation` | detector | renamed files (`entab-test_fid.ch`) gave `ENTAB`, `CHEMPLEXITY` | letters of the file name's last part |
| `mzml` | retention time (MS-1) | a `<scan>` without `scan start time` read as 0 s (pyopenms: unset) | `rt_s` is null when the file states none; chromatograms leave such scans out and say so, the mzML export writes no time (`docs/provenance/mzml.md`) |
| `jeol-jdf` | sample id (NMR-1) | the id was the 16-byte `sample_id` parameter (`20230816 Zheng R`) | the context section's `sample_id => "…"` line, the whole id, which the context read as text confirms (`docs/provenance/jeol-jdf.md`) |
| `plate` | acquisition time (PLATE-1) | SoftMax Pro text exports' `Date Last Saved` was reported as the read's start | reported as `acquisition.saved_at` (a new field); the exports state no read time (`docs/provenance/plate-readers.md`) |
| `waters-raw` | sample description | only on the spectra run; an MRM-only run keeps it on its SRM table | the experiment's `sample.name` (`docs/provenance/waters-raw.md`) |
| `bruker-tdf` | polarity, sample name | `Frames.Polarity` was not summarized for the run; a sample name containing `default` was taken for a software default | `extra.polarities`; only names that start with *default* are defaults (`docs/provenance/bruker-tdf.md`) |

## Where the other reader is wrong

Each is an entry of `corpus/oracle/second/adjudications.toml` with its evidence; the largest
groups:

- **Reference conversions that leave data out** (ProteoWizard, depositors' mzML): SIM scans
  dropped, one polarity or only MS1 exported, truncated reference files, excerpts.
- **Instrument names spelled from an ontology**: ProteoWizard writes the PSI-MS term (`Q Exactive
  HF`, `4000 QTRAP`) where the file holds `Q Exactive HF Orbitrap` or `4000 Q TRAP`.
- **Software versions**: ProteoWizard writes MassLynx `4.1` for every file; `_extern.inf` says
  `4.2 SCN983`.
- **Scaling and layout**: chromConverter 0.9 returns Shimadzu integers unscaled and interleaves
  two-channel `.lcd` tables, divides version-179 ChemStation values by a constant, adds a point to
  some `.ch` files, and rounds DAD values to seven digits (rainbow-api and the vendor's ASCII
  exports agree with ours).
- **Header parsing**: nmrglue drops spaces from JEOL parameter strings; the `jcamp` package reads
  a CR-only header as one title; brukeropusreader names reflectance blocks `AB` (brukeropus, a
  third reader, agrees with ours).
- **Image arrangement**: Bio-Formats lists Leica tiles as separate series and orders DM4 images
  differently.
- **Kinetic sweeps**: Neo reads an ABF 1 file's episode count from the wrong field (pyABF agrees
  with ours).

## Open findings

MS-1, NMR-1 and PLATE-1 and the Waters and TDF gaps are fixed (table above, 2026-09-26).

- **Gaps** (the second reader has a field ours does not report): ND2 dates of legacy files without
  a stored start, Blackrock NSx 2.1 files whose only time is in the `.nev`, and MassLynx
  `acquisition software version` on four ProteoWizard test directories without `_extern.inf`
  (ProteoWizard writes its constant `4.1`; the files state no version).
