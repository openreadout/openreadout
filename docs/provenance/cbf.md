# Provenance log — CBF / imgCIF diffraction images (`cbf`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-15 — reader (Richard Zimring with Claude as assistant)

**Published specification read:** the imgCIF/CBF dictionary and binary-section description published by the IUCr (https://www.iucr.org/resources/cif/dictionaries/cif_img; Bernstein & Hammersley, *International Tables for Crystallography* Vol. G, §2.9 and §3.7), and the CBFlib documentation web pages (http://www.bernstein-plus-sons.com/software/CBF/doc/CBFlib.html, "byte_offset" compression). CBFlib itself is GPL/LGPL: only its documentation pages were read, not its source. From these: the MIME-style binary section (`--CIF-BINARY-FORMAT-SECTION--`, header lines `Content-Type` with `conversions="x-CBF_BYTE_OFFSET"`, `X-Binary-Size`, `X-Binary-Element-Type`, `X-Binary-Element-Byte-Order`, `X-Binary-Number-of-Elements`, `X-Binary-Size-Fastest-Dimension`, `-Second-Dimension`, `-Padding`, `Content-MD5`), the 4-byte start marker `0C 1A 04 D5`, and the byte-offset coding (each value is the previous one plus a signed delta of 1 byte; the byte −128 escapes to a 2-byte delta, −32768 to a 4-byte delta, and the smallest 4-byte value to an 8-byte one, all little-endian).

**Prior art read (permissive):** FabIO 2026.6.0 (MIT, ESRF; `cbfimage.py`, installed in `oracle/.venv`) as documentation of the same layout and of the PILATUS 1.2 header convention (`# Detector:`, `# Pixel_size`, `# Exposure_time`, `# Wavelength`, `# Detector_distance`, `# Beam_xy`, `# Start_angle`, `# Angle_increment`, `# Count_cutoff`, ...). The PILATUS header keys are also listed in Dectris' public detector manuals' description of the miniCBF header. dxtbx (BSD-3, https://github.com/cctbx/dxtbx, `FormatCBFMiniPilatus*.py`) read as documentation for which header values become the beam centre (pixels, fast then slow) and that negative counts on Pilatus images mark module gaps (−1) and bad pixels (−2).

**Corpus files used:** DIALS `centroid_test_data` `centroid_0001.cbf` … (CC-BY-4.0, Graeme Winter 2013; licence stated in the dials-data definition https://github.com/dials/data/blob/main/dials_data/definitions/centroid_test_data.yml): PILATUS 6M-F at Diamond I02, miniCBF, 2463 × 2527 signed 32-bit, byte offset, padding 4095.

**Observed:** the header lines and binary section above; the image starts at the first byte after the marker; `X-Binary-Size` counts the compressed bytes; the element count equals fastest × second dimension; `Content-MD5` is the base64 MD5 of the compressed bytes. Values −1 fill the module gaps.

**Inferred:** that the "Beam_xy" pixel pair is (fast, slow), from dxtbx; that `Count_cutoff` is the overload value (dxtbx uses it as the trusted maximum).

## 2026-10-19 — more depositors (Richard Zimring with Claude as assistant)

**Corpus files added** (Zenodo): `zenodo1216026-eiger-ed-diff12-0011-cbf` (CC-BY-4.0, electron diffraction on an EIGER, written by CBFlib 0.9.5: `###CBF: VERSION 1.0`, header convention `SLS_1.0`, no PILATUS header lines), `zenodo50836-thermolysin-1800-cbf` (CC0-1.0, PILATUS3 2M at ESRF ID30, written by CBFlib 0.8.0) and `zenodo7948815-srugi-15kev3-0181-cbf` (CC-BY-4.0, PILATUS3 6M).

**Oracle:** FabIO (MIT), run on the files. All three agree on pixels and geometry; the reader needed no change.
