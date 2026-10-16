# Provenance log — SerialEM metadata files (`mdoc`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-15 — reader (Richard Zimring with Claude as assistant)

**Published documentation read:** SerialEM help, "About Formats" (https://bio3d.colorado.edu/SerialEM/hlp/html/about_formats.htm, public): the autodoc syntax (`key = value`, `[Section = N]`), the `.mdoc` written next to image files, the per-section keys (TiltAngle, StagePosition, ExposureDose, SubFramePath, DateTime, …) and `[T = ...]` titles.

**Prior art read (permissive):** mdocfile 0.2.3 (BSD-3, teamtomo, https://github.com/teamtomo/mdocfile), as documentation of the same keys; it is the oracle.

**Corpus files used (EMPIAR, released entries):** EMPIAR-10751 `G3_TS_001.mrc.mdoc` (tilt series, 41 `[ZValue]` sections), EMPIAR-10674 `stack_0000.tif.mdoc` (one `[FrameSet]`), EMPIAR-10987 `L4_ts_01_sort.mrc.mdoc`, EMPIAR-11841 `Yeast3.mrc.mdoc` (montage, `PieceCoordinates`).

**Observed:** the keys above; `DateTime` as `11-Oct-19  14:25:01` (two-digit year) and `15-Jul-19  13:54:01`; Windows paths in `SubFramePath`, `GainReference`; `[T = ...]` lines with `Tilt axis angle = 85.6`; a title line without brackets (`T = SerialEM: ...`) in the movie mdoc; sections of a dose-symmetric series whose `DateTime` order differs from their ZValue order.

**Inferred:** that the dose accumulates in `DateTime` order (so `cumulative_dose_e_per_a2` is the dose a tilt had received when it ended); the microscope name from the `Digitized on` title.
