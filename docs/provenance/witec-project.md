# Provenance log — WITec Project `.wip` / WITec Data `.wid`

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- wit_io (WITio) by Joonas T. Holmi, MIT No Attribution (MIT-0), https://gitlab.com/jtholmi/wit_io
  (`+WITio/+doc/README on WIT-tag format.txt`, the author's published description of the file
  format, read in full): the 8-byte magic (`WIT_PRCT`/`WIT_DATA` for versions 0–5,
  `WIT_PR06`/`WIT_DA06` for 6–7), the recursive tag record (name length, name, type code
  0 = tree, 2 = float64, 3 = float32, 4 = int64, 5 = int32, 6 = date as seven uint16, 7 = uint8,
  8 = boolean, 9 = strings; start and end file offsets as uint64), the project and data-file
  trees (`Version`, `SystemInformation`, `Data` with `DataClassName <n>`/`Data <n>` pairs and
  `NumberOfData`), the contents of the graph, image, bitmap and text entries, the
  interpretation unit tables (space, spectral, time, frequency, inverse space, phase, z) and the
  linear, lookup-table, space and spectral transformations.
- witio 0.2.0 (Python port of wit_io), MIT-0, https://github.com/iCalculate/witio (the PyPI
  package, installed in a separate venv, `.venv/` of the worktree): `transform.py` (the three
  spectral calibrations: second-order polynomial in the pixel index, the grating equation with
  pixel `nC` at wavelength `LambdaC`, the free polynomial clamped outside its bin range; the
  affine space transformation), `units.py` (the conversions from nm to µm, 1/cm, Raman shift
  `1e7·(1/λexc − 1/λ)`, eV, meV and the relative energies, h·c = 1239.84193 eV·nm), `data.py`
  (the value type codes of the data arrays: 1 int64, 2 int32, 3 int16, 4 int8, 5 uint32,
  6 uint16, 7 uint8, 8 boolean, 9 float32, 10 float64; the storage order of graph, image and
  bitmap arrays with and without the "inverted" flag; lines whose `LineValid` flag is false are
  blanked), `metadata.py` (the version-7 `Trace` records).
- witio is also run as the oracle (`oracle/witec_oracle.py`, black box), together with the
  vendor software's own text export of one data file (below).

**Corpus files used:** `witio-a-v5-graphene` (wit_io demo `A_v5.wip`, version 5, WITec Control
1.60, MIT-0), `witio-d-stitch-spectra-v7` and `witio-e-unpattern-video-v7` (wit_io demos, version
7), `zenodo4944335-proxy-component-spectra` and `zenodo4944335-35d-crm2-scan1-crr` (a `.wid`
data file; CC0), `zenodo4314529-axt-standard`, `zenodo4314529-axt-water-medium`,
`zenodo4314529-hmec-1-axt-30min-2` (CC-BY-4.0), `zenodo4314495-hcaec-tnf-axt1h` (CC-BY-4.0),
`figshare26765020-2dmsr-2-16-23-continued-ii` (CC-BY-4.0), `zenodo7907659-sa4` (ODbL-1.0).

**What was inferred from the files** (hex dumps and differential checks, recorded before the
parser was written):
- Storage order, confirmed on the files rather than taken from witio: in an image stored with
  the inverted flag false (`A_v5.wip`, four 104 × 144 band-sum images) the values run down a
  column first (index `y + height·x`): read that way the images are smooth (mean absolute
  neighbour difference 0.14–1.0 standard deviations), read row-first they are not (0.93–1.38).
  With the flag true (`HMEC-1_AXT_30min_2.wip`, 46 × 60 maps) the values run along a row first
  (`x + width·y`; 0.50 against 1.12). A graph array stores each spectrum's points contiguously
  and orders the spectra the same way (the 2930 cm⁻¹ band of the background-subtracted map,
  read row-first, correlates at 0.962 with the file's own 2930 cm⁻¹ peak image; 0.36 the other
  way).
- The `Information` text entry of a measurement (RTF) holds `key:<tab>value` lines under
  section headings (General, the spectrograph, the camera, the objective, the image scan, the
  sample location): start date and time, user name, system id, excitation wavelength, grating,
  centre wavelength, integration time, accumulations, objective name and magnification, points
  per line, lines per image, scan width and height in µm. The link from a graph to its text is
  by caption (`<measurement> Information`, `<measurement>--<n>--Information`), inferred from the
  captions of the ten files.
- Version-7 bitmaps store one int32 per pixel, bytes red, green, blue, unused, a row at a time
  from the first stored row (witio's reading). The first stored row is the top of the picture:
  every bitmap's and every map's space transformation (seven files, versions 5, 7 and 8) maps an
  increasing row index to a decreasing stage y (negative y scale), and in the version-5 file,
  whose video image is a BMP (bottom-up rows, so its visual top is known), the same sign holds
  for the BMP's top row. The embedded thumbnails do not correlate with the video images and
  were not used.
- Version-5 bitmaps are a Windows BMP file inside a stream entry (`BM` signature, 24-bit rows
  bottom-up, padded to four bytes).
