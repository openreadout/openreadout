# Provenance log — Bruker flex MALDI-TOF (`fid` + `acqus`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + licence), what was inferred.

## 2026-10-19 — initial reader (Richard Zimring with Claude as assistant)

**Prior art read as documentation:** OpenMS `AcqusHandler.cpp`, `FidHandler.cpp` and `XMassFile.cpp` (BSD-3-Clause), <https://github.com/OpenMS/OpenMS/tree/77cccda7caf40b46a4919d33748450270eccd565/src/openms/source/FORMAT/HANDLERS>, and MaldiAMRKit `maldiamrkit/io/readers.py` (MIT), <https://github.com/EttoreRocchi/MaldiAMRKit/blob/00c60a73ddca87c89f6743f980beada77f0516b9/maldiamrkit/io/readers.py>. Used for: the `##$KEY= value` parameter lines, `TD` as the point count, `fid` as 32-bit integers in the `BYTORDA` byte order, and the calibration (`tof = DW · i + DELAY`, then m/z from `ML1`, `ML2`, `ML3`).

**Oracle:** pyOpenMS 3.6.0 (BSD-3-Clause), `FileHandler.loadExperiment` on each spot's `fid` (`oracle/gen.py`, `flex_`). It rejects an `AQ_DATE` with a negative UTC offset, so it reads a copy whose `AQ_DATE` has the offset removed.

**Corpus files:** Zenodo records (CC-BY-4.0; 4978608 CC0-1.0): `flex-zenodo22714443-vibrio-a4`, `flex-zenodo22714443-vibrio-d3`, `flex-zenodo15744631-maldipickr-a7`, `flex-zenodo15744631-maldipickr-blank-e12`, `flex-zenodo14192252-vipera-f6`, `flex-zenodo4020648-cereulide-e19`, `flex-zenodo4020648-cereulide-i15`, `flex-zenodo18848797-rapiflex-1ref-f5`, `flex-zenodo18848797-rapiflex-lift-f5`, `flex-zenodo4978608-harpacticoida-e1`, `flex-zenodo7773644-clostritof-d7`, `flex-zenodo4271724-phytophthora-j19`; the OpenMS test spot `flex-openms-xmass` (BSD-3-Clause).

**Inferred from the corpus:**
- The directory tree `<run>/<spot>/<acquisition>/<1SLin | 1SRef | 1Ref | <mass>.LIFT/1SRef>/` and the acquisition mode from those names; `PATH` in `acqus` records the same names (`flex-openms-xmass`, whose files sit in a directory without them). `AQ_mod` 1 in the linear spots, 2 in the reflector spots; `SPType` 2 and `Parent` (the precursor m/z) in the LIFT spot.
- Telling flex `acqus` from NMR `acqus`: the first lines name XMASS (every spot above).
- Polarity from `IONIZATION MODE` (`LD+` in every spot).
- The LIFT spot holds a fragment calibration in `NTBCal` that the `ML` constants do not describe (its m/z from them runs from 38 to 129 for a precursor at 1270.94). The reader does not decode it.
