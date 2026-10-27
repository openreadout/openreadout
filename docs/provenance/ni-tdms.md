# Provenance log — National Instruments TDMS

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + licence), what was inferred.

## 2026-10-19 — initial reader (Richard Zimring with Claude as assistant)

**Vendor documentation consulted** (public): National Instruments, "TDMS File Format Internal Structure", <https://www.ni.com/en/support/documentation/supplemental/07/tdms-file-format-internal-structure.html> (read 2026-10-19). Used for: the segment lead-in and its table-of-contents flags, byte order, object paths, raw-data indexes (no data, same as before, new index, string byte size), the DAQmx raw-data index with its format-changing scalers and raw buffer widths, properties, the data type codes, timestamps (seconds since 1904 and a 2^-64 fraction), contiguous and interleaved raw data, chunks, incremental metadata and the new object list, and the `.tdms_index` file.

**Prior art:** npTDMS 1.13.0 (LGPL-3.0, <https://github.com/adamreeve/npTDMS>), run as a black-box oracle only (`oracle/tdms_oracle.py`); its source was not read. It also wrote the committed fixture (`oracle/make_tdms_fixtures.py`).

**Corpus files:** Zenodo records (CC-BY-4.0; 5011086 CC0-1.0): `tdms-zenodo3563513-signalexpress`, `tdms-zenodo5011086-rtdc`, `tdms-zenodo10404440-straingauge`, `tdms-zenodo15379761-daqmx-poly`, `tdms-zenodo6200886-pxi`, `tdms-zenodo7758683-cdaq`, `tdms-zenodo17582347-ldv`, `tdms-zenodo14923316-ta`, `tdms-zenodo15420253-geolab`, `tdms-zenodo14505832-geolab`, `tdms-zenodo5792896-febid`, `tdms-zenodo14860707-vna-v4712`, `tdms-zenodo17747449-saw-v4712`; the tdms-rs test corpus (robingkn/tdms-rs, MIT OR Apache-2.0, files written with npTDMS): `tdms-rs-strings`, `-timestamps`, `-integers`, `-floats`, `-booleans`, `-multiple-segments`, `-empty-segment`, `-standard-layout`, `-linear-scaling`, `-append-mode`, `-unicode-paths`, `-edge-cases`.

**Inferred from the corpus:**
- The DAQmx index marker: the bytes `69 12 00 00` are read as the little-endian 32-bit number `0x1269` in place of an index length (`tdms-zenodo10404440-straingauge`).
- DAQmx raw type code 3 is a 16-bit signed integer: each channel takes two bytes of a 6-byte row (three channels) in `tdms-zenodo10404440-straingauge`, and the values, scaled, equal npTDMS's (also `tdms-zenodo15379761-daqmx-poly`).
- The scaling chain: `NI_Scale[N-1]` with `Linear_Input_Source` / `Polynomial_Input_Source` 0, and no `NI_Scale[0]` properties, applies to the raw DAQmx values (both DAQmx files; linear and polynomial).
- The timestamp layout in little-endian files: the 64-bit fraction first, then the seconds (`tdms-rs-timestamps`, `tdms-zenodo17582347-ldv`).
- String raw data stores end offsets (`tdms-rs-strings`, `tdms-rs-edge-cases`).
- The writer names a file property can give (`WriterName` in `tdms-zenodo3563513-signalexpress`).

## 2026-10-27 — channel size overflow (Richard Zimring with Claude as assistant)

The `whole_tdms` fuzz target found that a segment's per-channel byte sizes could add up past `u64`. The sum is checked now, and such a segment is reported as unreadable. Fixture: `crates/openreadout-tdms/tests/fixtures/malformed/file-fuzz-chunk-size-overflow.tdms` (fuzzer output from the `mini.tdms` fixture). Prior art: none.
