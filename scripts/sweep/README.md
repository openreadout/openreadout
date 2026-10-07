# Corpus sweep

`sweep.py` runs the release binary over the development corpus and records, for each file and command, the exit code, peak memory, run time and output. It looks for panics, commands killed for memory or time, errors without a hint, exports that were not verified, and output that differs between two runs of the same command. It needs Python 3.11 or newer and nothing outside the standard library.

```sh
cargo build --release -p openreadout
python3.11 -m unittest discover -s scripts/sweep
python3.11 scripts/sweep/sweep.py --results target/sweep/run.jsonl --only mzml
python3.11 scripts/sweep/summarize.py target/sweep/run.jsonl
```

The corpus is read from `OPENREADOUT_CORPUS_DIR` (default `corpus/files`). The script skips held-out files and files that are not downloaded, and it writes nothing into the corpus. `--only TEXT` keeps the manifest entries whose id contains `TEXT`. `--resume` continues an interrupted run into the same results file (use the same binary and options). Use a new results path for each run.

Each command runs twice with one thread, a 120-second timeout and a 3 GiB memory ceiling. The script samples the process's memory every 50 ms and kills it at the ceiling, and it takes the peak reported by `wait4` after exit. Export outputs, including every file of a directory store, are compared between the two runs by SHA-256.

The commands are `info`, `info --view format`, `check`, `preview`, `stats` on the first plane, and, depending on what `info` lists, `trace`, `table`, `spectrum`, `scans` and `analyze chromatogram` with small output limits. Exports cover OME-TIFF and OME-Zarr (one plane, no pyramid), CSV, Parquet and Arrow (16 rows), NWB and JCAMP-DX (32 samples), mzML and the spectra as Parquet and Arrow (the whole run), ASM for plates and RDML for qPCR files. Inputs larger than 2 GiB skip the exports.

`--oracle-results PATH` marks exit codes 4 and 5 on files whose oracle test passed. Make that file with `CORPUS_RESULTS=PATH cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus corpus_matches_oracle`.

Keep results under `target/`. The findings of a campaign and their fixes go in `docs/benchmark/` (for example `sweep-2026-10.md`).
