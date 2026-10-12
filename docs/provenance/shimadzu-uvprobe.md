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
