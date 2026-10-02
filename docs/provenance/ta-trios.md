# Provenance: TA Instruments TRIOS files (`.tri`)

## 2026-09-26: initial reader (`ta-trios`; Richard Zimring with Claude as assistant)

**Corpus files used (development)** — all CC-BY-4.0 Zenodo records, licences read from the record
API on 2026-09-26:

- Zenodo 17225583 (de Graaf, Tampere University; Discovery HR-2 rheometer, 12 mm parallel plate):
  oscillation time sweeps and frequency sweeps (`GelMA…​.tri`) with the depositor's TRIOS Excel
  exports of the same runs (`…​.xls`: a `Details` sheet and one sheet per procedure step with
  Storage modulus, Loss modulus, Tan(delta), Angular frequency, Oscillation torque, Step time,
  Temperature, Raw phase, Oscillation displacement and, for frequency sweeps, Complex viscosity).
  30 pairs were compared while developing; the corpus holds six.
- Zenodo 15584951 (Pereiro et al., UPV/EHU; DSC25): `Eutectogels_CEC_DSC_*.tri` with the
  depositor's TRIOS CSV exports (`;`-separated, decimal comma: time min, temperature °C, Heat Flow
  (Normalized) W/g of the last procedure step).
- Zenodo 14587622 (Uřičář et al.; TGA550): `TGA_plasticized.tri`, `TGA_unplasticized.tri` with
  `data.xlsx`, whose `readme` says the thermogravimetric columns were copied from TRIOS (time min,
  temperature °C, weight mg per procedure step).
- Zenodo 22110629 (Guiu-Sans et al.; DMA850 stress ramps), 19679221 (Dinç; DSC), 17946405
  (Donoughue; Discovery HR30 time sweep): no export; structure and physical plausibility only.
- Zenodo 3976387 (Kameda; Discovery HR-2, 2019): an older file generation (version byte 12) whose
  data are not in the records below; used only to write the refusal.

**Held out:** Zenodo 2393624 (Hannequart et al.; DSC of a NiTi wire with TRIOS CSV exports) — not
opened while developing (`corpus/manifest.toml`, role `heldout`).

**Prior art consulted:** none. No open reader of `.tri` files was found (GitHub repository and
code search for `.tri` TRIOS readers, PyPI; TRIOS's own exports are the only ground truth).

**What we inferred** (from hex dumps of the files above and comparison with their exports):

- Header: bytes 0-1 `00 25`, byte 2 a file-generation byte (`0E` in every file with data; `0C` in
  the 2019 files), bytes 7-9 `08 25 02`, bytes 14-16 `14 20 01`, a u32 header length at 17 and a
  u32 entry count at 21, then that many key/value pairs of .NET-style 7-bit-length UTF-8 strings
  (`instrumenttype`, `instrumentserialnumber`, `instrumentname`, `companyname`, `rundate`,
  `culture`, `ticks`, `holder`, `operator`, `project`, `samplename`, `comments`,
  `procedurename`, `proceduresegments` (`;` or `#` separated), `proceduresignals`, `instrumentmode`,
  `testtype`, `samplesize` (in the run's culture: `4,1` for es-ES)). A PNG thumbnail follows
  (`01 00 01`, u32 length, PNG).
- Objects are `21 06 <u32 length> <16-byte GUID>`. A procedure step is such an object whose
  payload starts with 16 zero bytes and `F2 21 01 04 00 00 00 00 00 00 00 01 00 11 20 02`; it holds
  its properties and its signals. Properties are `20 04 <u32 length> <key GUID> <key GUID> <GUID>`,
  then `06 20 01 <u32 length> <u32 type> <u32 1> <value>`: type 2 a float64, type 4 a 7-bit-length
  string, types 5, 7, 8 small integers. The step's name is the string property whose key GUID is
  `d467abca-26c2-4afa-8ce7-23e95909709d` (`Ramp 10,00 °C/min to 100,00 °C`, `Frequency sweep - 1`:
  the export's sheet name).
- A signal is `21 06 <u32 length> <signal GUID>`, `<u32 1> <u32 1> 00 F2`, `21 01 04000000
  00000000`, `01 00 <u32 n>`, `01 10`, `21 01 <u32 L> <L bytes>` (L = 4: a zero; or L = 4 + 4n:
  n per-point u32 flags), `01 00 <u32 n>`, n little-endian float32 values, 3 zero bytes. The same
  GUID means the same quantity in every file and instrument (Temperature
  `f79c919e-6fa6-4856-92f8-6e5868f792f5` in DSC, TGA, DMA and rheometer files). Values are in SI
  units except temperature (°C): time s, heat flow W, weight kg, torque N·m, angle rad.
  Records with second u32 `0x02800001` hold no values (TRIOS computes those variables); records
  with `0x101` repeat the step's signals with n + 1 points and no values (or flags); records whose
  first u32 is 2 hold 64 values per data point (oscillation waveforms). None of these three is
  returned.
- G's 2026-09-26 finding that a rheometer torque "exists only as an all-zero u32 array" was the
  flag array of the layout above; the float32 values follow it and equal the export's
  Oscillation torque (µN·m) in all 30 exports.
- Names: every stored signal of the rheometer exports matched by value in all 30 pairs
  (Angular frequency `fa189102…`, Step time `7b1a8875…`, Temperature, Raw phase `5051699d…`
  (rad; exported in °), Oscillation torque `360672f5…` (N·m; exported in µN·m), Oscillation
  displacement `bb58e619…` (rad)). DSC and TGA files list `proceduresignals` in the order of their
  signal records; where the list is as long as the records (DSC25: 13 of 13) the names are taken
  by position and cross-checked against the other files; where it is not (TGA550: 9 names, 10
  records) only the positions that agree with the DSC file's GUIDs and the export are named.
  Time, temperature, weight and heat flow agree with the exports: TGA time, temperature and
  weight within the export's rounding (0.01 min, 0.01 °C, 0.001 mg); DSC time and temperature
  within rounding, and heat flow equals −(exported normalized heat flow) × sample size (the file
  holds the display setting `Exo Up`; the stored value has the opposite sign).
- Oscillation moduli are not stored. With the geometry's stress constant K_τ (property
  `e4fea4e5-a210-4f38-b6b2-0a28b2137a9c`, 2/(πR³) for the 12 mm plate), the strain constant
  R/gap (gap: signal `94411d75-361a-4cd4-a298-841a22dd9a83`, m), the instrument inertia
  (`6594d858-7e8d-402c-8554-12377d9f33c0`), the geometry inertia
  (`2dad9ebd-bdae-4839-9bd4-44c39e65f684`) and the sample inertia π ρ gap R⁴ / 6 (ρ: property
  `a58eb6fe-2446-4e34-b59f-47d56d2461d4`, 1000 kg/m³), TRIOS's moduli are
  G″ = K_τ M sin φ / (K_γ θ) and G′ = K_τ (M cos φ + I ω² θ) / (K_γ θ) (M torque, φ raw phase,
  θ displacement, ω angular frequency), tan δ = G″/G′, |η*| = |G*|/ω: in all 30 exports within
  2e-5 relative (time sweeps) and 4e-4 (frequency sweeps, where the inertia term cancels most of
  the torque at 628 rad/s). The fitted total inertia minus the two stored ones is 1.43e-9 kg·m²
  in every file, which is the sample term.
