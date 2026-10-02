# Provenance: Malvern Zetasizer `.dts` measurement files

## 2026-09-26: initial reader (`malvern-zetasizer-dts`; Richard Zimring with Claude as assistant)

**Corpus files used (development):**

- Zenodo 19044980 (Błaszkiewicz, CC-BY-4.0): `Bizmut_DMSO.dts` (Zetasizer software 7.13, three
  zeta and three size records) with the depositor's Zetasizer export `exported.txt`. **Exposure:**
  this record is the source of a held-out file of another format (an MRC file on main's held-out
  draw); it was used here before that was noticed, so it is not a corpus entry and does not count
  as evidence. The size-result layout below was mapped on it; with no other export of size
  records, size results are withheld (see the format note).
- Zenodo 19193123 (da Silva, CC-BY-4.0): `CNF sulf e fosf com Hist e Arg.dts` (7.12, 36 zeta
  records) with the depositor's table `CNF sulf e fosf com Hist e Arg (ZetaPotential).xlsx`
  (record, sample name, date, temperature, zeta potential, mobility, conductivity).
- Zenodo 6552819 (Xu, Ong, Mao, CC-BY-4.0): `2021_9_29_XF4051_size of PS 30KDa in toluene.dts`
  (7.02, schema 10; size records including three without results).
- Zenodo 13860620 (Nielinger, Alker, Urner, CC-BY-4.0): `DTAB_0.01M_NaCl.dts`,
  `[G1]OGD_D2O.dts`, `water.dts` (7.13).
- Zenodo 12108530 (Jones, CC-BY-4.0): `Fig2_DLS_size_data.dts` (7.12 and 8.01).
- Zenodo 10944781 (Morgenstein, Yudovich, Grupi, CC-BY-4.0): `DLS_DATA.dts` (8.00, averaged
  records).

Held out: one Zenodo depositor (draw 2026-09-26), not opened.

**Prior art consulted:** none exists for the binary `.dts` (searches: GitHub repositories and
code for "zetasizer" and ".dts" — only readers of text exports (cheminfo/parse-zetasizer, MIT;
lbignell/zetasizer_zs) and an unlicensed viewer whose repository holds only a README
(yalmju/dts-extract); Zenodo, figshare). The compound-file container is read with our own
`openreadout_core::cfb` (Microsoft's public [MS-CFB] specification).

**What we inferred (from the files and the depositors' exports):**

- A `.dts` file is a compound file: a `Header` stream (`u16` 1, a 16-byte identifier
  `c0 fe e4 a2 b2 26 d6 11 99 1b 00 90 27 9b 77 0c` in every file, `u16` 1, `u32` next record
  number), a `Deleted` stream and one `REC<n>` stream per record.
- A record begins `u16` schema (10 in 7.02, 13 in 7.12-8.01), `u16` kind (1 size, 2 zeta),
  `u32`, `u16`, `u16`, `u32` record number (equal to `n` and to the export's Record). Strings are
  `u32` byte length, `0x01`, UTF-16LE with a terminating NUL. After the software version string:
  an `f64` OLE Automation date (the export's measurement date and time, local, to the second on
  all 42 exported records), the instrument serial (`MAL…`), a `u32`, an `f32`, then the
  measured temperature as `f32` °C (the export's T to its one decimal on all 42).
- Sample name: the string that follows the material string and its 46-byte block, which begins
  `01 00 00 00 01 00` and ends `02 00 00 00 01 00 00 02 00 00 00 01 00 00` in every file and
  version (dispersants may have several strings, so position alone does not work). Equals the
  export's Sample Name on all 42 records.
- Size results (kind 1): `f32` Z-average (d.nm), `f32` PdI, `u32` n and n `f32` (a correlation
  fit), then three groups — intensity, number, volume — each `u32` k + k peak means (d.nm),
  `u32` k + k peak areas (%), `u32` k + k peak widths (d.nm). Z-average, PdI, intensity peak
  means, areas and %Pd (width / mean) equal the export on the three size records. Located by
  that structure (areas of a distribution sum to 100 %); exactly one match is required. Records
  without results (aborted measurements) have none.
- Zeta results (kind 2): `u32` 5 + 5 `f32` zeta peak areas (%), `u32` 5 + 5 peak means (mV),
  `f64` conductivity (mS/cm), then `f32` values: the applied voltage (≈148-150 V, inferred), the
  zeta potential (mV), its deviation, the mobility (µm·cm/V·s), its deviation and five not
  identified; then peak widths, and mobility peak areas, means and widths (`u32` 5 + 5 `f32`
  each). Zeta potential, mobility and conductivity equal the exports on all 39 zeta records; the
  deviations equal the single peak's width where there is one peak (inferred names). Located by
  that structure; exactly one match is required.
- The number and volume "means" of the export are not stored (they follow from the
  distributions, which are not decoded); nor is the diffusion coefficient (it follows from the
  Z-average through Stokes-Einstein).
