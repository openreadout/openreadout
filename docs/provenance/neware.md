# Provenance: Neware BTS `.nda` / `.ndax`

## 2026-09-26: initial reader (`neware-nda`, `neware-ndax`)

**Corpus files used (development):**

| id | source | variant |
| --- | --- | --- |
| `echem-newarenda-new-nda-file` | NewareNDA tests (BSD-3-Clause), `tests/nda/neware_reader/new_nda_file.nda` | `.nda` version 29 (BTS 7.6, `NEWARE20200325`) |
| `echem-newarenda-sim-nda` | NewareNDA tests, `tests/nda/mediafire/SIM.nda` | `.nda` version 29 (`NEWARE20180910`), SIM steps |
| `echem-newarenda-2-1-6-61-nda` | NewareNDA tests, `tests/nda/mediafire/2-1-6_61[07005012].nda` | `.nda` version 130, BTS 9.0 record layout |
| `echem-newarenda-issue72-nda` | NewareNDA tests, `tests/nda/github/Issue72/TestFile.nda` | `.nda` version 130, BTS 9.1 record layout |
| `echem-zenodo21631502-sintef-nda` | Zenodo 21631502 (SINTEF, CC-BY-4.0) | `.nda` version 130, BTS 9.1 layout with a temperature |
| `echem-newarenda-unit27-ndax` | NewareNDA tests, `tests/nda/github/Issue94/…Unit27_Example_Data_File.ndax` | `.ndax`, `.ndc` version 14 with two auxiliary temperatures |
| `echem-newarenda-issue60-ndax` | NewareNDA tests, `tests/nda/github/Issue60/BTS85-36-6-5-110-20240424.ndax` | `.ndax`, `.ndc` version 14, variable logging interval |
| `echem-navani-ndax` | navani (be-smith/navani, MIT), `Example_data/test.ndax` | `.ndax`, `.ndc` version 5 (whole records) |
| `echem-cellpy-ife-ndax` | cellpy (jepegit/cellpy, MIT), `testdata/data/20260302_IFE_BTS85_2_9_8_1.ndax` | `.ndax`, `.ndc` version 17 with auxiliary voltage and temperatures |

Held out (never opened while developing): the SINTEF Duracell `.ndax` record (Zenodo 20802274,
CC-BY-4.0; a fourth `.ndax` depositor).

**Prior art consulted:**

- NewareNDA 2024.x (<https://github.com/Solid-Energy-Systems/NewareNDA>, BSD-3-Clause, SES AI),
  read as documentation: the `NEWARE` magic and the version byte at offset 14, the 86-byte version-29
  record (index, cycle, step, status, time in ms, voltage in 0.1 mV, current and capacities scaled by
  the current range), the version-130 record layouts (BTS 9.0: 88-byte records whose first six
  bytes repeat; BTS 9.1: `0x55` records with float32 values), the `.ndax` zip members, the `.ndc`
  record layouts per version and file type, the status codes, and the table that maps a current
  range to a scale factor. It is also the black-box oracle for every development file.
- The oracle venv's NewareNDA is not used by the binary.

**What we inferred ourselves (from the bytes, not from NewareNDA):**

- Every `.ndc` file (versions 11-17, and version 5) is a 4096-byte header followed by 4096-byte
  pages. A page starts with a `u16` page kind (the file type + 1) and a `u16` record count, then a
  validity bitmap (one bit per record); its last four bytes are the CRC-32 of the rest of the page.
  NewareNDA filters records by value (`Voltage != 0`, `Index != 0`); we read exactly `count` records
  per page and refuse a page whose CRC does not match. Checked on every page of the development
  `.ndax` files.
- `data_runInfo.ndc` (file type 18) holds a record only when the logging interval changes, at step
  boundaries, or periodically; the records in between are logged at the preceding interval. Checked
  on the development files: the stored step time of each logged record equals the previous logged
  time plus (records between − 1) × the previous interval plus its own interval, on all but one
  record in 28 124 (a pause in `echem-newarenda-issue60-ndax`). We reconstruct `step_time` for the
  records in between with that rule (reported under `assumed`) and return the capacities and
  energies only where they are stored (NaN elsewhere; NewareNDA integrates current over time for
  them, an estimate we do not make).
- The auxiliary files `data_AUX_<channel>_<type>_<n>.ndc` name their channel type: the middle
  number is the `ChlType` of `TestInfo.xml` minus 100 (102 voltage → `_2_`, 103 temperature →
  `_3_`, 1122 → `_1022_`). We match files to their `TestInfo.xml` entries by channel and type.
  NewareNDA matches them by sorted file name, which swaps the voltage and temperature channels of
  `echem-cellpy-ife-ndax` (its `T1` is the 0 V auxiliary voltage; the 25.6 °C cell temperature is
  in its unnamed column). Values: the voltage file holds 0.0, the temperature file 25.6, which
  matches the depositor's BTSDA export of another channel of the same rig (V1 0.0 V, T1 26.6 °C).
- Total test time: BTSDA's export (`nw_regular_export_ife_example.csv` in cellpy, a Neware BTSDA
  "regular export") shows `Total Time` continuing across a step change while `Time` (step time)
  restarts at 0 on the same instant. We compute `time` (the trace abscissa) the same way:
  cumulative step time, adding nothing at a step change.
