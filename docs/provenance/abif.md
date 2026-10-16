# Applied Biosystems ABIF (`.ab1`, `.fsa`, `.hid`) provenance

## 2026-10-15 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`):

- Biopython test data, `Tests/Abi` at commit `fc0c1567` (Biopython License Agreement, files
  redistributable with the notice): `abif-biopython-310` (ABI PRISM 310), `abif-biopython-3100`
  (3100 written by a 3130 with KB basecaller 1.4.1.8), `abif-biopython-3730` (3730xl, KB 1.2),
  `abif-biopython-a6-1-db3` (3730xl, KB 1.4.0), `abif-biopython-empty` (3730xl run with no
  sequence: five `N` calls), `abif-biopython-no-smpl1` (a file rewritten by other software: 19
  tags, no sample name, lower-case calls in `PBAS2`), `abif-biopython-nonascii` (3730xl, a
  sample name byte outside ASCII), `abif-biopython-fsa` (a 3130xl fragment-analysis run, AFLP
  with a ROX size standard), `abif-biopython-fake` (26 bytes, not ABIF: a corrupt-file case).

**Prior art consulted:**

- *Applied Biosystems Genetic Analysis Data File Format* (ABIF File Format Specification and
  Sample File Schema), Applied Biosystems, July 2006. Published by the vendor for programmers
  who want to read `.ab1` and `.fsa` files; no registration or licence terms attached. Read from
  a public copy at <https://kodomo.fbb.msu.ru/~lewis/terms/term3/pr6/ABIF_File_Format.pdf>
  (the vendor's original URL, `www6.appliedbiosystems.com/support/software_community/ABIF_File_Format.pdf`,
  no longer resolves). It describes the container (header, directory entries, element types)
  and lists the tags that the 3100, 3130, 3730 Data Collection software and SeqScape 2.5 and
  Sequencing Analysis 5.2 write.
- Biopython `Bio/SeqIO/AbiIO.py` (dual Biopython License Agreement / BSD-3-Clause), read as
  documentation: which tags carry the called bases, quality values and sample name, and that
  `.fsa` files have no `PBAS` tags.

### Inferred from the specification

- 128-byte header: `ABIF`, a big-endian 16-bit version (101), then one 28-byte directory entry
  that points at the directory (element count at byte 18, offset at byte 26). Every integer is
  big-endian.
- A directory entry: four-character tag name, 32-bit tag number, element type, element size,
  element count, data size, data offset (or the data itself when it is 4 bytes or less, stored
  from the high-order byte), and a reserved handle.
- Element types: 1 byte, 2 char, 3 word (u16), 4 short (i16), 5 long (i32), 7 float, 8 double,
  10 date (i16 year, u8 month, u8 day), 11 time (u8 hour, minute, second, hundredths),
  12 thumbprint, 13 bool, 18 pString (length byte first), 19 cString (NUL-terminated), 1024 and
  above user data (kept as bytes); others are legacy and kept as bytes.
- Tags used: `DATA` 1–4 and 105 raw dye channels, 5–8 run monitors (voltage in decavolts,
  current in mA, laser power in mW, temperature in °C), 9–12 and 205 analysed channels; `FWO_`
  the base each channel holds; `PBAS`, `PCON`, `PLOC` 1 (edited) and 2 (as called) bases,
  quality values and peak positions (scan of the analysed channels); `DyeN`, `DyeW`, `Dye#`,
  `DySN`, `DyeB` (`S` marks the size-standard dye); `StdF` size-standard file; `SMPL`, `SpNm`
  sample names; `TUBE` well; `LANE` capillary; `MCHN`, `MODL`, `HCFG` instrument; `RUND`/`RUNT`
  run and collection start and stop; `RunN`, `RPrN`, `RMdN`, `GTyp` polymer, `EPVt`, `InVt`,
  `InSc`, `Tmpr`, `LsrP`, `LNTD`, `User`, `CTID`, `CMNT`, `SVER`, `SPAC`, `S/N%`, `NOIS`,
  `Satd`/`OfSc` saturated scans, `OvrI`/`OvrV` values above 32767.

### Inferred from the files (scratch dumps of the directories, not committed)

- Every file but `fake` starts `ABIF`, version 101, directory tag `tdir` 1, element type 1023,
  size 28.
- The analysed channels are shorter than the raw ones (16,302 against 16,961 scans in the 3730
  file): analysis starts at a later scan. `PLOC` values index the analysed channels (the last
  peak of the 3730 file is below 16,302).
- `PCON` holds one byte per base; the empty run has five bases with quality 0.
- `no_smpl1` keeps only the 19 tags another program wrote: `DATA1`–`4` equal `DATA9`–`12`, the
  calls in `PBAS2` are lower case and `PCON` is all zero.
- `test.fsa` has four dyes (5-FAM, JOE, NED, ROX), `DyeB` all blank and no `StdF`: the file does
  not say which dye is the size standard. ROX as the fourth dye of dye set F is the standard by
  the convention the dye set names.
- `Rate1` is a 12-byte user type; its meaning is not documented and it is kept as bytes.

### Fragment sizing

The files store raw fragment-analysis traces, not sizes: sizing is done by analysis software and
not written back. The reader sizes fragments itself (`Source::Inferred`) with the Local Southern
method (Elder and Southern, *Anal. Biochem.* 128 (1983) 227–231): each scan between two ladder
peaks is sized from the three-parameter fit `L = c / (m − m0) + L0` through the nearest ladder
peaks, averaging the fit through the two peaks below and one above with the fit through one below
and two above. The ladder fragment lengths are the published product specifications of each size
standard (the vendor's public product pages); they are facts about a reagent, not file contents.
Ladder peaks are matched to the expected lengths by the reader's own alignment (see
`crates/openreadout-ce/src/sizing.rs`).

## 2026-10-15 — 3500 files and the instrument's sizing

**Corpus files used:** Fraggler (Clinical Genomics Umeå, MIT) demo and paper data at commit
`ebbb56d9`: `abif-fraggler-multiplex`, `-error`, `-no-peaks`, `-k1` … `-k7` (3500 Dx, Data
Collection 3 IVD v3.0.1, SizeCaller v1.1.0, dyes 6-FAM, VIC, NED, PET and LIZ, GS600 LIZ), and
`abif-fraggler-peakscanner`, the PeakScanner peak table the Fraggler authors exported for the
seven `-k` runs.

**Prior art consulted:** none beyond the entries above.

### Inferred from the files

- These files have version 300, not 101; the container is laid out exactly as the 2006
  specification describes, so versions 1.xx to 3.xx are read alike.
- `SMap` 1 is the scan index 0 … n−1 and `SMap` 2 (doubles) a size for each scan: −1 before the
  sized range, then strictly rising. `StdF` names the standard and the range it was fitted over
  (`GS600LIZ(60-600)+Normalization`).
- `Peak` 1–25 are parallel arrays, one entry per peak. Matching every row of the PeakScanner
  export to the file's peak with the same dye and data point (127 of 127 rows of `-k1`, all
  equal to the export's two decimals) gave: 1 dye number (5 = LIZ, by `DyeN`), 2 apex scan
  ("Data Point"), 3 and 4 start and end scans, 7 height, 12 size, 13 and 14 start and end sizes,
  17 area in base pairs. Peak 21 holds a ladder length on the LIZ peaks inside the fitted range
  (60 … 600) and 0 elsewhere: the size-standard assignment. The other columns are not named.
- Sizing check: the reader's own Local Southern sizing, run on the same LIZ channel against
  GS600 LIZ (60–600) with its own peak matching, differs from `SMap` 2 by at most 0.07 bp at the
  instrument's peaks of `-k1`, `multiplex` and `error`. The reader therefore uses `SMap` 2 when
  it is present and its own sizing otherwise.
