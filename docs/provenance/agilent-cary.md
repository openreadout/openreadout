# Provenance: Agilent (Varian) Cary UV-Vis files (`agilent-cary`)

## 2026-09-26 — first reader, from corpus files and the depositors' CSV exports (Claude as assistant)

**Corpus files used (development only; no held-out source):**

- Zenodo 21041480 "UV-Vis documentation examples for pySpecdata" (Franck lab, CC0): two `.BSW`
  (Cary 60, batch Scan 5.0), one `.DSW` (Cary 60, Scan 5.0, eight spectra) and one `.BSK`
  (Cary 60, Scanning Kinetics 5.0, nine spectra and a baseline); no exports. (Zenodo 15644941
  is an earlier version of the same record with the same four files.)
- Zenodo 14894113, 10873570, 10953327, 14893472, 14893496 (P. Błaszkiewicz and co-authors, Poznań,
  CC-BY-4.0): `.DSW` files of a Cary 4000 (Scan 3.00(182)), 178 distinct, 175 of them with the
  CSV that the Cary software exported from the same file (`<name>.csv`: a title row, a
  `Wavelength (nm),Abs,` header, then one `x,y` row per point). Record 10953339 holds copies of
  10873570's files (same checksums) and was not used separately. A sample of these is in the
  manifest (`cary-*` ids); the prototype was run on all of them.
- DataverseNL 10.34894/BQNQJX (two `.BSW` with CSV) could not be downloaded: the repository
  answers scripted requests with a browser challenge, which was not circumvented.

**Inferred (from bytes, differential comparison between files, and the CSV exports):**

1. The file starts with a length-prefixed text (`0x11`, `Varian UV-VIS-NIR`) in a 0x3E-byte
   header; at 0x3E a sequence of *stores* begins, each a u32-length-prefixed class name
   (`TContinuumStore`, `TGraphStore`, `TReportStore`, `TDatabaseStore`, `TBaselineInfoStore`,
   `TBaselineStore`) followed by a u32 **store size counted from the store's first byte** (the
   next store starts at `start + size`; on every file the last store ends 4 bytes before the
   end of the file). Found by differencing consecutive store offsets in the eight-spectrum
   `.DSW`.
2. A `TContinuumStore` is one spectrum. After the size: u32, u32 (149 in every spectrum store: a
   store version), f32 **x minimum**, f32 **x maximum**, f32 **y minimum**, f32 **y maximum**,
   u32 **point count**, u32; the header is 1,028 bytes long; then **point count × (f32 x, f32 y)**
   pairs. Found by searching the file for the CSV's first values as float32 (the x/y pairs are
   interleaved); checked on all 208 spectrum stores: the stored extremes equal the minimum and
   maximum of the pairs, and on all 175 CSV pairs every x agrees to 5·10⁻⁴ nm and every y to the
   CSV's ten significant digits (the CSV writes the float32 values).
3. After the pairs come u32-length-prefixed Latin-1 texts: the spectrum's **name** (the CSV's
   title row), `Collection Time: 12/18/2024 10:12:52 PM` (month/day/year, 12-hour clock: every
   file whose day exceeds 12 puts the month first; the Polish depositor's files too, so it is
   not a locale setting), `Operator Name : …`, the application and version (`Scan Software
   Version: 3.00(182)`, `Scan Version 5.0.0.999`, `Scanning Kinetics Version 5.0.0.999`),
   `Parameter List : `, one text per parameter (Scan 3.00: fixed 302-character fields, the name
   in the first 34 columns; 5.0: `␣␣name␣␣value[␣␣value]`; empty texts separate groups), then
   `Method Log :` with `Method Name : …`, `Date/Time stamp: …`, the logged method changes and
   `End Method Modifications`, then status texts `<SBW (nm)> , 2.000`, `<Current Wavelength> ,
   300.00` and, in kinetics files, `[Time] , 2.000`. `[Time]` is the method's **scheduled** time of the spectrum in minutes, not
   when it was collected: the kinetics file's parameters are `Cycle Time(min) 2.0 30.0` and
   `End Time(min) 6.0 180.0`, and its nine spectra say 0, 2, 4, 6, 36, 66, 96, 126, 156, while
   their `Collection Time`s are 11:49:14, 11:50:08, 11:52:08, 11:54:08, 11:56:08, 12:26:08, …
   (elapsed 0, 0.9, 2.9, 4.9, 6.9, 36.9, … min). Both are returned, under different names. The
   bytes after the last text to the end of the store are not decoded.
4. Axis and quantity from the parameters: `X Mode` `Nanometers` → wavelength in nm; `Y Mode`
   (`Ordinate mode` in kinetics files) `Abs` → absorbance. The Scanning Kinetics file names no
   X mode; its x values run from 700 to 200, as the method log's `X Start nm` 700.0 and
   `X Stop nm` 200.0 say, so nm is taken (recorded in `assurance.assumed`). Other modes are
   returned under their own name with a finding (no file shows them).
5. `TBaselineStore` has the spectrum store's header (version 149) and a name (`Baseline 100%T`):
   read as a baseline spectrum the same way (its extremes also equal its values'). The other
   stores are listed by `info --view structure`, not decoded.

**Validation:** `corpus/oracle/series/cary-*.json` (oracle/series_oracle.py on each file's CSV
export: every point's x and y, and the facts the CSV repeats); the Cary 60 files have no export
and are checked for self-consistency only (stored extremes, parameter Start/Stop against the x
range, the store chain ending at the file's end).
