# Varian / Agilent NMR provenance

## 2026-09-22 — initial implementation

Inputs: `nmrglue-varian-1d`, `nmrglue-varian-2d`, `nmrglue-varian-2d-tppi`,
`nmrglue-varian-3d` in the existing BSD-3-Clause nmrglue test archive. Hex dumps inspected
before implementation: 32-byte big-endian headers (six 32-bit dimensions/strides, two 16-bit
codes, a 32-bit block-header count), then 28-byte block headers and interleaved float32
samples. File sizes equal 32 + block count × block stride. `procpar` text inspected.

Permissive prior art: https://github.com/jjhelmus/nmrglue at
`5e2f095705bb90c6dc2f6a916abdfda82bac8e04`, BSD-3-Clause (LICENSE.txt retrieved and
verified before reading `nmrglue/fileio/varian.py`). Used to interpret header flags,
procpar value counts, numeric types and block headers.
Raw sample values are returned without applying block scaling or phase corrections, matching
nmrglue. Sweeps retain disk order (`as_2d=True` oracle); no indirect phase-cycle reordering.
Processed files use the same header container when present; unverified variants must fail
explicitly. Bounds/overflow tests and generated type variants supplement public float32 files.

## 2026-09-23 — implementation and validation (Richard Zimring with Claude as assistant)

The 2026-09-22 entry above records what was inspected before the reader existed; this entry
records how it was written and checked.

**Prior art consulted** (permissive): nmrglue 0.12 as installed in `oracle/.venv`
(`nmrglue-0.12.dist-info/licenses/LICENSE.txt`: BSD-3-Clause, read before the source),
`nmrglue/fileio/varian.py`: `read`/`read_fid`/`read_fid_ntraces`, `get_nblocks`/`get_block`
(block = block headers + traces), `get_fileheader` (32-byte big-endian header: six 32-bit, two
16-bit, one 32-bit field), `get_blockheader`/`get_hyperheader` (28 bytes each), `fileheader2dic`
and `blockheader2dic` (meaning of the status bits), `find_dtype` (float32 if bit 0x8, else int32
if 0x4, else int16), `uninterleave_data`, `find_shape` (how `np`, `ni`, `phase` and `array`
shape nD data; used only to describe sweeps, not to reorder them), `read_procpar`/`get_parameter`
(procpar record syntax). Names in our code and docs are our own.

**Corpus files used:** `nmrglue-agilent-1d/-2d/-2d-tppi/-3d/-4d` (BSD-3-Clause nmrglue test
archive), `nmrpy-test1-fid`, `nmrpy-test2-fid` (NMRPy test data, BSD-3-Clause, LICENSE.txt at the
pinned commit read), `nmrxiv-s325-1h` (CC-BY-4.0), `nmrxiv-s501-carbon`, `nmrxiv-s501-ghsqcad`
(CC-BY-SA-4.0). Licences of nmrXiv studies from `api/v1/schemas/bioschemas/<S…>` and the project
list. Also inspected, not added: nmrXiv S361 (VnmrJ 4.0, same console as S325), Zenodo 5683588
(CC0, a VnmrJ 4.2 PRESS spectrum with files flattened to `fid-N`/`procpar-N`, no directories).

**Inferred from the files (hex dumps and a Python survey of every block header):**
- every corpus `fid` is exactly 32 + blocks × block size; block numbers run 1…n; block scale is 0
  everywhere; `arraydim` = blocks × traces per block in every `procpar` → `check` warnings
  `block_index`, `array_mismatch`, info `block_scale` flag deviations;
- status 0xc9 (float32) on VNMRS consoles, 0x49 on a VnmrJ 4.2 INOVA, 0x45 (int32) on an older
  INOVA whose procpar has `parversion` 5.1 and no `parver`; no int16 file was found, so int16 is
  tested only on synthetic files;
- `time_run`/`time_complete` are `YYYYMMDDThhmmss` without a zone (spectrometer local time);
  `time_submitted_local` carries a suffix (`X07PMWedJun`) that is ignored;
- `tn`/`dn` name nuclei element-first (`H1`, `C13`); we report `1H`, `13C` (inferred rewrite);
- processed data (`datdir/phasefile`) appear in no public directory found; nmrglue does not read
  them either, so they are listed by `info --view structure`, not decoded (a known gap).

**Validation:** `oracle/gen.py` branch `varian` (nmrglue `varian.read(as_2d=True)`, or
`read_fid` for the directory without procpar): every corpus directory matches — sample and sweep
counts, sample rate (`sw`), nucleus, frequency, scans, `np`, pulse program, solvent, and the
xxh3-128 of every hashed sweep (up to 1000 per file) bit for bit: 10 files, 6,320 sweep ×
channel blocks.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the procpar version, sample type, dimensions and the block scale factors (available, not applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-11-14 — sweep order, indirect-dimension parameters, experiment workspaces, `phasefile`

**Corpus files:** `bmrb18352-exp1949` (a VnmrJ experiment workspace: `acqfil/fid`, `curpar`, `datdir/phasefile`, `datdir/data`; 3D, `array` `phase2,phase`), `bmrb27037-chsqc-600v` (2D, `f1coef`), and the earlier development directories (`nmrglue-agilent-2d`, `-2d-tppi`, `-3d`, `nmrxiv-s501-ghsqcad`).
**Prior art consulted:** nmrglue 0.12 (BSD-3-Clause), `fileio/varian.py` read as documentation: `find_shape` (the direct dimension is `np`/2; each `niN` gives `niN` × the number of `phaseN` values traces; a non-phase arrayed parameter adds a dimension inside the increments; the last name in `array` is the innermost loop), `find_torder` with `fileiobase.index2trace_reg`/`index2trace_opp` (with `array` `phase2,phase` the phase of F1 changes fastest, then the phase of F2, then the F1 increment, then the F2 increment; `phase,phase2` swaps the two phase loops), and `get_fileheader`/`get_nblocks` (the same 32-byte header and blocks are used by other VnmrJ data files).
**What was inferred from what.**
- In all six multidimensional directories the product of the `niN` (those above 1) and the lengths of the arrayed parameters equals the number of traces in `fid`, so the sweep order is reported as nested loops, slowest first: `niN` … `ni`, then the arrayed parameters in the order `array` names them. nmrglue's two torders and its "innermost array" rule are this order; it is reported, not applied (sweeps stay in disk order).
- `f1coef`/`f2coef` are 8 numbers in `bmrb27037-chsqc-600v` (`1 0 -1 0 0 -1 0 -1`) and empty elsewhere; they are reported as numbers, not used.
- A workspace (`curpar`, `acqfil/`, `datdir/`) holds the FID in `acqfil`; `datdir/phasefile` has the 32-byte header with status 0x490b (data, spectrum, float32, no complex bit) and two block headers per block: 512 traces of 512 real float32 values, a processed 2D plane (`procdim` 2). It is decoded with the same block layout, values as stored. `datdir/data` (status 0x4b3b, complex and hypercomplex bits, 2048 values per trace) is listed, not decoded: the order of its four hypercomplex parts is not documented by any source used here.

## 2026-10-11 — scans in the data versus scans requested

**Corpus files:** every development input (13 directories). **Prior art consulted:** none new.
**What was inferred:** `ct` equals the first block header's `completed_scans` in all 13. It is below `nt` only in `nmrxiv-s501-carbon` (3000 requested, 1776 completed, a stopped acquisition). The FID holds the sum of `ct` transients, so `scans` now reports `ct` when it is positive and `requested_scans` reports `nt`.
