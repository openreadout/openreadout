# Bio-Rad droplet digital PCR provenance

## 2026-10-19 — QuantaSoft and QX Manager CSV exports (Richard Zimring with Claude as assistant)

**Written before the reader.** Format `biorad-ddpcr`.

**Corpus files used** (all in `corpus/manifest.toml`): the nine results exports of figshare 24278779, 24295207, 24297802, 24298027, 24298105, 24298141, 24302305, 24302329 and 24302350 (CC BY 4.0, García-Escudero, Avila et al.); `ddpcr-nipd-19-3581` and `ddpcr-nipd-20-0618` (github.com/joe-m-shaw/ddpcr_nipd, MIT); `daattali-ddpcr-quant`, `daattali-ddpcr-quant-a01-amplitude`, `daattali-ddpcr-small`, `daattali-ddpcr-small-a05-amplitude` (github.com/daattali/ddpcr, MIT); `dpcp-10ng-a01-amplitude`, `dpcp-0-0195ng-b04-amplitude` (github.com/alfodefalco/dPCP, MIT); `podcall-vim-4plex-d04-amplitude` (github.com/bioc/PoDCall, GPL-3, data only); `ornelles-ddpcr-demo-a01-amplitude` (github.com/ornelles/ddpcr, GPL-3, data only). No file is held out yet.

**Prior art consulted:** none. Python's `csv` module reads the same files as the oracle (`oracle/export_tables.py`).

**Inferred from the files:**

- Results exports start with a header line whose first cell is `Well`. QuantaSoft 1.7 (García-Escudero, joe-m-shaw) writes `Well,ExptType,Experiment,Sample,TargetType,Target,Status,Concentration,Supermix,CopiesPer20uLWell,...,AcceptedDroplets,...` (63 headers) and then two more empty cells per row. An older QuantaSoft (`daattali-ddpcr-small`) writes `TypeAssay,Assay` in place of `TargetType,Target` and has no `Supermix` or `CopiesPer20uLWell`. QX Manager (`daattali-ddpcr-quant`) writes `Well,Sample description 1,...,Sample description 4,Target,Conc(copies/µL),pg/µL,...,Accepted Droplets,Positives,Negatives,...` (73 headers, UTF-8 `µ`).
- `Concentration` holds a number or the words `No Call` (wells QuantaSoft could not classify). The concentration unit, copies/µL, is written in QX Manager's header and matches `CopiesPer20uLWell` ÷ 20 in QuantaSoft's rows (`140` and `2800`).
- Amplitude exports: QuantaSoft writes `Ch1 Amplitude,Ch2 Amplitude,Cluster` (dPCP, PoDCall, ornelles, daattali) with cluster numbers 0 to 4. QX Manager writes three `Target Value of ...` lines and a blank line, then `Ch1Amplitude,Ch2Amplitude,1,2,` with `0`, `1` or `u` per target. The well is in the file name (`..._A01_Amplitude.csv`).

## 2026-10-19 — QuantaSoft `.qlp` plates

**Written before the reader.** Format `biorad-qlp`.

**Corpus files used:** the nine `.qlp` files of the figshare records above (`figshare24278779-qlp` ... `figshare24302350-qlp`), each published with QuantaSoft's results export of the same plate (the CSV entries above), and `zenodo4957405-specificity-qlp` (Zenodo 4957405, CC-BY-4.0, Deconinck). All were written by QuantaSoft 1.7.4.0917. No file is held out yet.

**Prior art consulted:** none. The QX Manager `.ddpcr` files of Zenodo 19729930 were checked only far enough to see that their 7-Zip header is encrypted (`bsdtar` reports it); they are not read.

**Inferred from the files** (hex dumps, and differential comparison with each plate's export):

- Bytes 0–3 are `IILQ`, bytes 4–7 the offset of the first IFD, and IFDs have TIFF's shape (entry count, 12-byte entries of tag, type, count and value, next-IFD offset). The first IFD holds TIFF tags 305 (`QuantaSoft 1.7.4.0917`), 306, 315 and 316 and private tag 65004 (`Ch1,Ch2`).
- The second IFD's tag 65018 points at 96 records of 1386 bytes. Each starts with the offset of the next record. At fixed places in each record are the well, the sample (`63` for A01 of 24302350, matching the export's `Sample`), `Absolute Quantification` and `ABS` (the export's `ExptType` and `Experiment`), and per channel `Unknown` or `Not Used` and the target (`TubB`). The export lists a channel only when its kind is not `Not Used`.
- 96 further IFDs (one per well) hold the well name in tag 65019 (four inline bytes) and the supermix in tag 65077 (`QX200 ddPCR EvaGreen Supermix`, the export's `Supermix`).
- The IFDs of read wells hold tag 65021 with a count equal to the number of droplets; the space to the next blob is that count times 28 bytes. Read as a u32 and six float32, the second and fifth values range over the amplitudes of the two channels. Tag 65057 has the same count and one byte per droplet: in A01 of 24302350 the byte `0x11` occurs 17,079 times and `0x22` 2,155 times, the export's negatives and positives, and `0x00` 1,603 times (20,837 stored − 19,234 accepted). On the duplex plate 24278779, `0x33` and `0x44` occur as often as the export's `Ch1+Ch2+` and `Ch1-Ch2+`. The mean of the second value over a well's `0x22` and `0x33` droplets equals the export's `MeanAmplitudeofPositives` for channel 1 (15815.22 and `15815`), and the fifth value does the same for channel 2. Tag 65078 is a float32 of 0.85 in every well, the droplet volume in nL: −ln(17079 ÷ 19234) ÷ 0.00085 µL is 139.8 copies/µL, the export's `140`.
- Every well whose counts differ from the export is one the export calls `No Call`. In each of them no droplet has class `0x00` and all droplets have one class, so the classes were not set. The reader leaves the counts of such wells empty.
