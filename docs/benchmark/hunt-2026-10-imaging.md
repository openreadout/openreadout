# Imaging bug hunt, October 2026

We ran OpenReadout on imaging files from public records the corpus had not used, to find bugs
on files it had never seen. This page lists what we surveyed, what failed, what we fixed and
what is left. The files are development files now (`corpus/manifest.toml`, between the
"Imaging bug hunt, 2026-10" markers), each with an independent oracle in `corpus/oracle/`.

## What we surveyed

143 files from 137 source records (135 Zenodo records and two OME sample directories), 2.9 GB in all, chosen for instrument models, software
versions, labs and years the corpus lacked. Every record was checked against the held-out set
(`cargo xtask heldout-check` passes; no held-out file was opened) and every licence is CC-BY-4.0,
CC-BY-SA-4.0 or CC0-1.0, read from the record. Human tissue was avoided where another sample
would do.

| family | files | notes |
| --- | --- | --- |
| CZI | 28 | ZEN blue 2.x to 3.x and ZEN black; LSM 710, LSM 880, LSM 900 Airyscan 2, Elyra SIM, Axio Imager, Axio Observer, an unnamed "Other Microscope"; MIPs, sums, SIM reconstructions, FRAP time series; bug-report files ("Import issue", "Example czi ZEN black file") |
| ND2 | 14 | NIS-Elements 4.30 to 6.10; A1, AX, Ti2 widefield, Nimbus, colour DS-Fi3; three Bio-Formats bug reports (skewed frames, "colour problems", a file Fiji 8.1.1 could not open); one "ND2" that is a TIFF |
| LIF | 11 | LAS AF and LAS X 3 to 4, SP5, SP8, STELLARIS (one with no channel names), a coherent-Raman excitation sweep |
| TIFF family | 30 | LSM (2009 to 2025, spectral and phasor files), SVS, NDPI (three NDP.scan 2 fluorescence files), an Aperio FL `.scn`, OME-TIFF from five writers, ImageJ and tifffile stacks, MetaMorph and Micro-Manager exports, SEM and micro-CT TIFFs |
| IMS | 6 | Imaris 9 to 10, one file from a Bio-Formats bug report, one with a z-reduced pyramid |
| VSI | 1 | an Olympus DSX-2000 EFI scan written by PRECiV (texture map and height map) |
| OIR | 1 | an FV3000 stack whose last plane is incomplete |
| OME-Zarr | 1 | NGFF 0.4 HCS plate from the scMultipleX test record |
| DM3 / DM4 | 25 | images, line spectra, EELS spectrum images, an FFT (complex), montage tiles, a Bio-Formats bug report |
| MRC | 12 | cryoSPARC, RELION and ChimeraX maps, tilt series, segmentations (int8), an AFM density map, a SerialEM micrograph (int16) |
| SER / EMI, Velox EMD | 6 | Ceta and STEM HAADF files from 2019 to 2026 |
| not instrument files | 9 | files that share an extension with a supported format: a JSON model and a text file named `.emd`, a Java object file named `.ser`, a MARC library catalogue named `.mrc`, a Bayesian network `.bif`, an XPS `.vms`, an atom-probe `.SCN`, a Stockholm `.stk` alignment, and an NDPI set file (`.ndpis`) |

The 9 non-instrument files are not in the manifest; they were run to see how detection fails.

For each file we ran `info`, `check`, `preview`, `stats` on one plane, and an OME-TIFF and an
OME-Zarr export of the first image, recording exit codes, time and peak memory
(`/usr/bin/time -l`). Then `oracle/gen.py` wrote ground truth with the usual independent readers
(czifile, nd2, liffile, oirfile, tifffile, mrcfile, dm3_lib, ncempy, h5py, zarr-python;
Bio-Formats as a black box for VSI) and the corpus test compared them.
`oracle/metadata_compare.py` set every image's metadata against Bio-Formats as well.

## Results before the fixes

- **No panics, no hangs.** `info` took at most 0.84 s (median 0.02 s) and 30 MB. `check` took at
  most 0.36 s.
- **Pixels.** 127 of the 134 inputs agreed with their oracle on every compared plane at the
  first run. Four lossy-JPEG files (three NDPI and one Aperio FL `.scn`) differed from
  libjpeg-turbo by at most 3 grey levels; the manifest now marks them `lossy`, as it does the
  other JPEG slides. One was bug H1 below, and two are truncated files (see the end of the next
  section).
- **Exits.** Apart from the non-instrument files, non-zero exits came from the two truncated
  files, H2 and H3 below, the complex-valued DM4 FFT (exit 6 on export, a known gap with a hint)
  and `stats` on two whole-slide level-0 planes above 4 GiB (exit 6 with a hint to use
  `--level` or `--region`). Every error carried a hint.
- **Metadata.** No file disagreed with its primary oracle. Against Bio-Formats, the differences
  were conventions documented in the format notes (RGB as samples, not channels; MRC ISPG 0 as T;
  generic TIFF stacks as Z; LIF channel names from the LUT) or Bio-Formats errors (ND2 channel
  names and exposure, where the `nd2` package agrees with us).

## Bugs found and fixed

| # | severity | what happened | file | fix |
| --- | --- | --- | --- | --- |
| H1 | S2 | VSI raw tiles with three samples per pixel were read with red and blue exchanged. No development file had raw RGB tiles. The file's own TIFF preview (RGB by the TIFF specification) and Bio-Formats both show the stored order is blue, green, red. The file was `unvalidated` (writer PRECiV never seen). | `zenodo19893921-dsx-efi-vsi` | raw three-sample tiles are returned as red, green, blue (`docs/provenance/vsi.md`, 2026-10-06) |
| H2 | S2 | An Imaris file whose level 1 has 109 z planes against 219 at level 0 could not be exported (exit 2: plane out of range at level 1). `preview` failed the same way when it chose that level (for example with `--max-size 64`, or on any large z-reduced file at the default size). | `zenodo4433202-ovule-732` | export copies only source levels with the image's z count; `preview` does not pick other levels |
| H3 | S3 | OME-TIFF export refused int8, int16 and int32 planes (exit 6), so int16 MRC micrographs and int8 segmentations could only go to OME-Zarr. | `zenodo15871573-carbon-ctf`, `zenodo10814409-chlamy-seg` | written with TIFF `SampleFormat` 2 and read back; test `crates/openreadout-ometiff/tests/signed_samples.rs` |
| H4 | S4 | A file matched only by its extension that then failed to open was reported as corrupt with the hint "may be truncated by an interrupted acquisition or copy" (a JSON file named `.emd`, a Java object file named `.ser`), or as an MRC with an unknown mode (a MARC catalogue named `.mrc`). | the non-instrument files above | the error now says only the extension matched, and the hint says the file may be another kind of file (`Registry::open`; unit test in `openreadout-core`) |

The exit codes are unchanged by H4: an agent still gets 4 or 6, but no longer a wrong explanation.

Two files are in the corpus as `role = "corrupt"`, and `check` flags both correctly: an AFM
density map 16 bytes short of its header (`zenodo14172249-lafm-gltph`), and an FV3000 stack whose
last plane holds 81,920 of 2,097,152 bytes (`zenodo18303030-mclid-p21`). OpenReadout refuses
the incomplete plane with a hint and reads the rest with `--select`.

## After the fixes

All 134 inputs agree with their oracles (`corpus_matches_oracle` with `CORPUS_ONLY` set to the
hunt ids, which now takes a comma-separated list). The evidence refresh adds 132 confirmed
development files: CZI 93 → 121 files and 25 → 53 depositors, ND2 28 → 42, LIF 22 → 33,
TIFF 118 → 148, DM 89 → 114, MRC 13 → 23, IMS 10 → 16, EMD 32 → 37. SER rises from medium to
high confidence (a fifth depositor). Of the 134 inputs, 76 were `validated` before the fixes,
47 `partially_validated` and 11 `unvalidated`; the new files validate the variants they
brought (ZEN blue 3.1 and 3.2, NIS-Elements 4.30, 4.60, 5.11, 5.21 and 6.10, NDP.scan 2, the
DSX writer and others).

## What is left, and why

- **LIF channel names** came from `LUTName` (`Red`, `Gray`). Fixed after the hunt: a channel is
  now named after the dye LAS X records for it, per channel (LAS X 4) or on its detector's band
  when the bands map to the channels one for one; the colour stays in `color`
  (`docs/provenance/lif.md`, 2026-10-07). On three files Bio-Formats' dye names are shifted
  against the channels' colours and lasers.
- **MRC files with ISPG 0** are read as image stacks (T), following the MRC2014 specification
  and mrcfile, even when they are clearly density maps written by older tools (cryoSPARC,
  RELION, ChimeraX, NVERSION 0). An agent asking for the z voxel size of such a map gets none.
  A rule that recognises them would diverge from mrcfile and needs a decision.
- **`stats` on a whole-slide level 0** held the plane in memory: 2.9 GB peak for a
  53,760 × 17,664 RGB NDPI, while `export` streams the same file in tiles. Fixed after the hunt:
  `stats` reads large planes of tiled levels in strips, with the same results
  (`docs/architecture-memory.md`).
- **NDPI sets** (`.ndpis`, one NDPI per fluorescence channel) exited 3. Since 2026-10-07 a set
  is read as one image with a channel per listed file (`ome-ndpi-manuel-test3`,
  `docs/provenance/tiff.md`).
- **OME-Zarr 0.5** had no development file. Since 2026-10-07 two of IDR's NGFF 0.5 sample stores
  are in the corpus (`idr0062A-6001240-labels-ngff05`, `idr0066-chicken-embryo-mip-ngff05`,
  `docs/provenance/ome-zarr.md`); both agree with zarr-python.
- **Not surveyed for lack of small public files from new records:** DCIMG (the new OME record
  holds 1.6 GB files), EER, MetaMorph `.nd`, QPTIFF, BIF, and the formats another workstream owns
  (OIB/OIF, MIRAX, ImageXpress, CellVoyager).
- The deeper pass (every plane, export of every image, read-back with OpenReadout and an
  independent reader) ran later: `deep-pass-2026-10-imaging.md`. z-MIP previews were not run.
