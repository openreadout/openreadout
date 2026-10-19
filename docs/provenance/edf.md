# Provenance log — EDF, EDF+, BDF and BDF+ (`.edf`, `.bdf`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — first reader (Richard Zimring with Claude as assistant)

**Specifications read** (public documents, no registration):
- EDF: Kemp et al. 1992, as published at https://www.edfplus.info/specs/edf.html: the 256-byte
  fixed header, 256 bytes per signal in field-major order, data records of each signal's samples
  in turn as 2-byte little-endian two's complement integers, and the linear map between the
  digital and physical minimum and maximum.
- EDF+: Kemp and Olivan 2003, https://www.edfplus.info/specs/edfplus.html: `EDF+C` / `EDF+D` at
  the start of the reserved field, the `EDF Annotations` signal holding time-stamped annotation
  lists (onset byte 21 duration byte 20 texts separated by byte 20, ending with byte 0), the
  time-keeping TAL that opens every data record, the patient and recording subfields
  (`Startdate dd-MMM-yyyy`, admin code, technician, equipment) and the 1985 clipping year for
  `dd.mm.yy` dates.
- BDF: BioSemi's public page https://www.biosemi.com/faq/file_format.htm: byte 0 is 255 followed
  by `BIOSEMI`, the reserved field reads `24BIT`, samples take 3 bytes.
- BDF+: https://www.teuniz.net/edfbrowser/bdfplus%20format%20description.html: `BDF+C` / `BDF+D`,
  the `BDF Annotations` label, 24-bit little-endian samples.

**Prior art run as oracles, not read:** pyedflib (BSD-2-Clause, https://github.com/holgern/pyedflib)
for the digital samples and annotations, MNE-Python (BSD-3-Clause, https://github.com/mne-tools/mne-python)
for a second reading of the header and of the physical values (`oracle/gen.py`, `edf()`). Neither
project's source was opened while writing the reader.

**Design choices, each from the specifications:**
- Physical value = digital × scale + offset with scale = (physical max − physical min) /
  (digital max − digital min) and offset = physical min − digital min × scale, the linear map of
  the EDF specification.
- Signals with the same number of samples per data record share a sample rate; each such group is
  one trace, so nothing is resampled. The annotation signals are not traces; their TALs are the
  `annotations` table.
- An `EDF+D` / `BDF+D` file's data records are grouped into sweeps of contiguous records, using
  the time-keeping TAL of each record.
- A `-1` record count (allowed only while recording) is replaced by the number of whole records
  the file holds, with a finding.

**Corpus files used** (28 development files from 17 sources): `gin-edf-*` (NeuralEnsemble
ephy_testing_data on G-Node GIN, commit bf392aa, CC-BY-SA-4.0); OpenNeuro ds003810, ds004448,
ds004752, ds004840, ds005648, ds005857 and ds008851 (CC0); PhysioNet eegmmidb S001R01 and the
Sleep-EDF Expanded SC4001EC hypnogram (ODC-By-1.0); Zenodo records 3885755 and 4688758 (CC0); the
test files of MNE-Python (BSD-3-Clause), pyedflib (BSD-3-Clause) and edfio (Apache-2.0); the
Nihon Kohden export in mne-testing-data (CC0); figshare 28485251 (CC-BY-4.0, an EDF+D written
by a Nihon Kohden EEG-1100C).

**What was inferred from the files and the reference readers' output:**
- pyedflib and MNE give annotation onsets after the first data record's start, not after the
  header's start time: `gin-edf-test-subsecond` starts 0.3945312 s after its header time, and its
  TAL `+2.3457031` is reported at 1.9511719 s. The reader counts every time from the first
  record's start too, and adds that start to `recorded_at`. `zenodo-4688758-s65` (EDF+D written
  by BESA) has its first record at +1 s and contiguous records, so it is one sweep, and so is
  `figshare-28485251-1` (EDF+D, 602 contiguous records from 0 s). No development file has gaps
  between records, so the `discontinuous records` layout stays unvalidated.
- pyedflib keeps TAL times in 100 ns steps. The reader parses them exactly to the nanosecond and
  subtracts before converting to floating point, so the two agree bit for bit on every file here.
- A TAL without a duration is reported by pyedflib as −1 and by MNE as 0; the reader gives 0.
- `gin-edf-event-markers-bdf` holds annotations after the last data record. pyedflib and the
  reader keep all 541, and MNE drops the 183 past the end of the data.
- `openneuro-ds004448-sub-001-ses-03-task-smrbmi` has a signal (Cz) whose physical minimum and
  maximum are equal. Its values are the physical minimum, as in MNE, and `check` reports it.
- `mnepython-test-edf-stim-channel` writes `reserved` in the reserved field, so it is plain EDF.
