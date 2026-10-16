# Provenance log — NeXus NXmx diffraction data (`nxmx`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-15 — reader (Richard Zimring with Claude as assistant)

**Published specification read:** the NeXus NXmx application definition and base classes NXdetector, NXbeam, NXtransformations (https://manual.nexusformat.org/classes/applications/NXmx.html; NeXus International Advisory Committee, public); the HDF5 file format specification's Link Message (type 0x0006) for external links (The HDF Group, public).

**Prior art read (permissive):** bitshuffle (MIT, https://github.com/kiyo-masui/bitshuffle) README and `bshuf_h5filter` description: the HDF5 filter 32008, its client data (version, element size, block size, compression 0/2), the LZ4 chunk framing (8-byte big-endian size, 4-byte big-endian block size, per block a 4-byte compressed size) and that elements after the last whole block of 8 are copied unshuffled. dxtbx (BSD-3, `FormatNXmxEigerFilewriter.py`, `FormatNexus.py`) as documentation of where DECTRIS masters keep `detectorSpecific` and that masked pixels hold 2^n − 1.

**Oracle:** h5py 3.16 with hdf5plugin 7.1 (BSD-3 / MIT), run on the corpus files.

**Corpus files used:** Zenodo 3383611 (CC-BY-4.0, Graeme Winter, DLS I04): `Therm_6_1_master.h5`, `Therm_6_1.nxs`, `Therm_6_1_000001.h5` (EIGER 2X 16M, 3 images); Zenodo 4327001 (CC-BY-4.0): `wtB2_1_master.h5` (DECTRIS file-writer master, data files not used).

**Observed:** the link name, length and NUL-terminated file and object path in the master's bytes (`data_000001`, length 27, `Therm_6_1_000001.h5`, `/data`); `/data` of the data file is (3, 4362, 4148) uint16, one image per chunk, filter 32008 with client data (0, 3, 2, 0, 2); 1 253 320 pixels of every image hold 65535; `image_nr_low` 1 and `image_nr_high` 3.

**Inferred:** that the goniometer axis whose values change from image to image is the scan axis; the name patterns of data files and masters.

**Fuzzing (2026-10-15):** `nxmx_links` found a panic on a link value of one byte, and `codec_bitshuffle` one on a declared size that is not a whole number of elements and an allocation of a declared size far beyond what the chunk can hold; all are now skipped or refused, with unit tests.
