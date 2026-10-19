# Provenance log — Shimadzu UVProbe `.spc`

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-12 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted:** none for the layout. The compound-file container is read with `openreadout_core::cfb`, written from Microsoft's public [MS-CFB] specification. olefile 0.47 (BSD-2-Clause) was run as a black box to list the streams and, in the oracle, to read them.

**Corpus files used:** `figshare29858651-uvprobe-substrate`, `figshare29858651-uvprobe-baseline-air`, `figshare29858651-uvprobe-usl18-52-1` (figshare 29858651, CC BY 4.0, UV-3600 Plus, UVProbe 2.43), `figshare31645174-uvprobe-usl19-69` (figshare 31645174, CC BY 4.0, same instrument, another study), `figshare30231052-uvprobe-dpvc` (figshare 30231052, CC BY 4.0, UV-3600 Plus, UVProbe 2.71 on a Chinese-language PC). Each file of the first two records has the depositor's UVProbe text export beside it (`"<file> - RawData"`, then wavelength and value columns with three decimals). No held-out file.

**What was inferred from what.**
- The streams (listed by the container): `DataStorage1/DataSetGroup/DataSet1/DataSpectrumStorage/Data/` holds `Data Header.1` (two u32: point count and a value code), `X Data.1` and `Y Data.1` (float64 arrays of that many points). In all 8 files examined the arrays have the declared length, the x values run from high to low wavelength in 1 nm steps, and they are the export's wavelengths in reverse order; the y values equal the export's values to within half the last printed digit (6 files, 601–661 points each).
- `DataHeader/Header Info` starts with three float64: the first and last x and the step (850, 190, 1; 2500, 200, 1), which equal the x array's ends and spacing in every file.
- The value code is 2 in the files whose method text (`MethodStorage/PageTexts2`) says `Measuring Mode: Absorbance` and 4 in the files whose method text says `测定方式:反射率` (reflectance) with values of 14–55, a percentage. Other codes are left unnamed.
- `DataSetHeaderInfo`: a one-byte-length text (the data set name, `RawData`), four zero bytes, a float64 OLE Automation date (days since 1899-12-30, local time; the history record `DataSetHistory` "Created new data set" carries the same value), six zero bytes, a text (an organization name), eight zero bytes, then texts: software (`UVProbe`), its version (`2.43`, `2.71`), the instrument model (`UV-3600Plus`; `UV-3600Plu`, cut to ten characters, in the Chinese-language files), the instrument series (`UV-3600Plus Series`), a u32 (1), a second model text and the serial number (`A12015200066`). Texts are GBK on the Chinese-language PC.
- `DataSetGroupHeaderInfo` (u32) is 1 in every file, with one `DataSet1`; a file with more data sets or with `.2` arrays has no development file and is refused per data set (`unsupported_data_set` finding).
- `MethodStorage/PageTexts0..4`: method pages, each a list of one-byte-length label and value texts (`Wavelength Range (nm.):`, `Scan Speed:`, `Slit Width:`); they are kept as text in the vendor tree, not named.

## 2026-10-18 — the text exports as the oracle (Richard Zimring with Claude as assistant)

**Corpus files:** the three `.spc` of figshare 29858651 and five of figshare 31645174
(`USL19-69`, `USL19-69b`, `USL19-69c`, `USL19-69d`, `USL17-17`), each with the UVProbe text
export saved beside it (CC BY 4.0); `figshare30231052-uvprobe-dpvc` (no export).

**Prior art consulted:** none.

**What was inferred:** nothing new about the format. The exports of figshare 31645174 write a
decimal comma (`200,0	6,000`). They list the points from the shortest wavelength up, the
reverse of the stored order, so the oracle matches rows by wavelength. All 601–661 points of each
file agree with the export to within half the last printed digit. UVProbe's own version
(`UVProbe 2.43`, `UVProbe 2.71`, from `DataSetHeaderInfo`) is now observed as the writer version.

## 2026-10-19 — five more depositors (Richard Zimring with Claude as assistant)

**Corpus files:** `zenodo13884418-uvprobe-ni-f-uv`, `zenodo13884418-uvprobe-ni-f-wo3-au-uv`
(Zenodo 13884418, CC BY 4.0, UV-2600, UVProbe 2.50), `figshare33102008-uvprobe-film-usl19-67a`
(figshare 33102008, CC BY 4.0, UV-3600 Plus, UVProbe 2.43; the lab of figshare 29858651),
`gh-sukritranjan-sulfite-kinetics-release-uvprobe-ph7-1` and `-blank` (GitHub
sukritranjan/sulfite-kinetics-release, MIT with a citation request, UV-1900, UVProbe 2.70),
`gh-tillbiskup-uvvispy-uvprobe-sa281-02-280k` (GitHub tillbiskup/uvvispy test data,
BSD-2-Clause, UV-1601PC, UVProbe 2.43), `gh-cheminfo-spc-parser-uvprobe-ole` (GitHub
cheminfo/spc-parser test data, MIT, UV-2600, UVProbe 2.62). Each has the UVProbe text export
beside it.

**Prior art consulted:** none; only the test data of uvvispy and spc-parser was used, not
their code.

**What was inferred:** nothing new about the format; every file reads unchanged and agrees with
its export. Two scans store 0.2 nm steps (3001 and 1501 points); the Zenodo 13884418 exports
list only every fifth point, so their rows are matched by wavelength without a count check. The
sulfite exports print five decimals.
