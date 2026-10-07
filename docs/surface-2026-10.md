# CLI and MCP surface, October 2026

Status: implemented on `feat/surface`. The owner approved the whole proposal, including the items first held for review.

- One MCP tool per analysis, with `plate_options` on the six assay tools. The plate-reader subcommands are `analyze assay-wells`, `assay-curve`, `dose-response`, `kinetics`, `growth` and `assay-qc`, and `analyze` stays a CLI group.
- `check` is split into `check`, `planes`, `compare` and `report`, `spectra` into `scans` and `spectrum`, `export --attachment` into `extract`, `batch summarize` into `summarize`, and `search --health` and `--export` into `health` and `export-dataset`. `index --health` is gone. `openreadout_health` is new, and `openreadout_formats` is gone (the `openreadout://formats` resource stays).
- The argument renames and naming conventions below, CSV in `openreadout_export`, and `schema_version` 2. The `--samples` and `--layout` spellings of `--sample-sheet` are gone (rule 4).
- `batch` keeps `measure` and `options`.
- The Python and R bindings follow: `scans()` and `summarize()` in Python, `openreadout_scans()`, `openreadout_spectrum()` and `openreadout_summarize()` in R, and `scans()` and `spectrum()` in the WebAssembly package.
- Measured after the change: 24 top-level commands and 32 tools, with 80 kB of input schemas (58 kB before). The largest is `openreadout_peaks` at 6.8 kB. The six assay tools are 3.4 to 5.4 kB each, against 26 kB for `openreadout_analyze`.

The rest of this note is the proposal as reviewed.

This note maps every command, MCP tool and argument of v0.1.0 to the surface proposed for the next release, and says why. There are no users yet, so names change without aliases or deprecation shims.

## Where we start

The pre-release consolidation (commit `55da6fa5`, 1 October) already cut the CLI from 41 top-level commands to 16 and the MCP server from 34 tools to 15. It did that by folding operations into modes: `info --view`, `check --against`/`--report`/`--planes`, `spectra` (scan list or one spectrum), `export --attachment`, `batch summarize`, `search --health`/`--export`, and one `openreadout_analyze` tool whose `options` are the union of eight analyses. [The table at the end](#appendix-the-v010-consolidation) lists those folds.

Measured on v0.1.0 (`tools/list` of `openreadout mcp`, bytes of JSON):

| tool | args | input schema | output schema |
| --- | --- | --- | --- |
| `openreadout_analyze` | 4 (+ `options`: 6–33 per kind) | 26 138 | 101 397 |
| `openreadout_batch` | 22 | 4 926 | 14 458 |
| `openreadout_preview` | 23 | 3 803 | 13 274 |
| `openreadout_export` | 18 | 3 994 | none |
| `openreadout_stats` | 11 | 3 263 | 16 688 |
| `openreadout_table` | 14 | 2 765 | 5 669 |
| `openreadout_spectra` | 18 | 2 599 | 11 645 |
| `openreadout_info` | 8 | 2 095 | 49 695 |
| `openreadout_check` | 11 | 2 032 | 36 304 |
| `openreadout_trace` | 10 | 1 755 | 6 154 |
| `openreadout_search` | 5 | 1 320 | 1 034 |
| `openreadout_index` | 10 | 1 295 | 8 344 |
| `openreadout_watch` | 6 | 1 048 | 6 376 |
| `openreadout_link` | 6 | 746 | 6 042 |
| `openreadout_formats` | 0 | 36 | 1 879 |
| total (15 tools) | | 57 815 | 278 961 |

`openreadout_analyze` alone is 45 % of all input schema bytes, and its valid options depend on `kind`.

## Evidence from the evals

The transcripts in `evals/results/transcripts/` (2026-09-24 and 25, 281 questions, Haiku 4.5, plus visual reruns with Sonnet 5 and Opus 5.5) predate the consolidation: they used the 34-tool surface under its old name. What they show:

- Agents picked a tool per question without trouble. Tool calls that failed were rare: 2 of 180 `info`, 2 of 69 `trace` (an empty `x_range`), 1 of 8 `spectrum` (index -1), and 3 of 12 `export`.
- All 3 failed `export` calls asked for `format: "csv"` on an FCS file. CSV export exists on the command line but not in MCP.
- The most used arguments were `trace`: `max_samples`, `trace`, `sweep`, `channels`, `x_range`; `scans`: `limit`, `ms_level`, `offset`; `stats`: `select`, `image`, `mip`, `region`, `level`. Rarely used options were rarely touched.
- The only questions that did worse with OpenReadout than without were two plate-reader questions (`plate-gen5-kinetic-reads`, `ana-plate-tecan-doubling-time`).

No eval has run on the v0.1.0 surface yet. The owner's Sonnet 5.5 run after this change will be the first.

## Rules

From the owner and the coordinator, applied to every merge and split below:

1. Merge two operations only when they share most of their arguments and return the same kind of output.
2. No command or tool whose valid arguments depend on a mode or kind flag.
3. Rarely used meta commands may share one parent on the CLI. Pure meta operations don't belong in MCP.
4. Removing pure duplicates is good. Renaming for consistency between CLI and MCP is good.
5. No merged tool may have a larger or more conditional schema than the biggest tool it replaces, unless rarely used options move into a clearly named nested object.

And three naming conventions, so that one vocabulary describes both surfaces:

- An MCP tool is `openreadout_` plus the CLI command, with `-` as `_` (`nmr-peaks` → `openreadout_nmr_peaks`). A command under a CLI group keeps its last word (`self formats` would be `openreadout_formats`).
- An MCP argument is the CLI long flag with `-` as `_` (`--rt-range` → `rt_range`). Two systematic exceptions: a list is a repeatable CLI flag in the singular and an MCP array in the plural (`--channel 0 --channel 2` → `channels: [0, 2]`); a boolean that is on by default is `--no-X` on the CLI and `X: false` in MCP (`--no-pii` → `pii: false`).
- A CLI positional argument has the MCP name of its value: `file` for one input, `paths` for several, `index_dir` for an index.

## Commands and tools

### Proposed CLI

| v0.1.0 | proposed | change and rule |
| --- | --- | --- |
| `info` (`--view summary\|full\|structure\|explain\|format`, `--ask`) | `info` | kept. The views answer one question, what is in this file, at different depths, from the same `file` argument (rule 1; the coordinator's own example). |
| `check` | `check` | integrity only. |
| `check --planes` | `planes` | split out: it returns plane hashes (and writes planes with `--dump-dir`), not an integrity report, and takes `--image/--select/--region/--level`, which integrity does not (rules 1, 2). |
| `check --against B` | `compare A B` | split out: a diff of two files, with its own exit code (1 = different) and flags (`--tolerance`, `--ignore`, `--no-pixels`, ...) (rules 1, 2). |
| `check --report` | `report` | split out: writes a diagnostic bundle with its own flags (`-o`, `--include-text`, `--hex`, ...) (rules 1, 2). |
| `preview` | `preview` | kept. |
| `stats` (`--per channel\|image\|plane\|well\|field`) | `stats` | kept. Every grain measures the same pixel statistics from the same plane arguments; only `--well` is specific to plates. `--no-planes` goes (see arguments). |
| `trace` | `trace` | kept. |
| `table` | `table` | kept. |
| `spectra` (scan list, or one spectrum with `--scan/--index/--nth`) | `scans` | split: the scan list (filters, paging, `--count`, `--csv`) (rules 1, 2). |
| | `spectrum` | split: one spectrum's arrays (`--scan`, `--spectrum`, `--nth`, `--centroid`, `--max-points`) (rules 1, 2). |
| `analyze peaks` | `analyze peaks` | kept under the `analyze` group (see below). |
| `analyze chromatogram` | `analyze chromatogram` | kept. |
| `analyze nmr-peaks` | `analyze nmr-peaks` | kept. |
| `analyze ephys-features` | `analyze ephys-features` | kept. |
| `analyze spikes` | `analyze spikes` | kept. |
| `analyze qpcr` | `analyze qpcr` | kept. |
| `analyze gate` | `analyze gate` | kept. |
| `analyze assay wells` | `analyze assay-wells` | one level less; the name says it is a plate-reader assay. |
| `analyze assay curve` | `analyze assay-curve` | same. |
| `analyze assay dose-response` | `analyze dose-response` | same. |
| `analyze assay kinetics` | `analyze kinetics` | same. |
| `analyze assay growth` | `analyze growth` | same. |
| `analyze assay qc` | `analyze assay-qc` | same. |
| `export` | `export` | conversion only. |
| `export --attachment NAME` | `extract FILE NAME` | split out: writes one stored blob as is; none of the conversion flags apply (rules 1, 2). |
| `batch MEASURE` | `batch MEASURE` | kept (see the note on `batch`). |
| `batch summarize TABLE` | `summarize TABLE` | split out: regroups a saved table and takes no measure, inputs or measure options (rule 2). |
| `link` | `link` | kept. |
| `index` | `index` | kept. `--health` goes: it duplicates `health` (rule 4). |
| `search` | `search` | queries only. |
| `search --health` | `health INDEX_DIR` | split out: a report on the whole index, with its own flags (`--no-hash`, `--max-list`, `-o`), no query (rules 1, 2). |
| `search --export OUT` | `export-dataset INDEX_DIR QUERY OUT` | split out: writes an ML-ready data set, with its own flags (`--tables`, `--redact`, `--license`, `--no-images`) (rules 1, 2). |
| `watch` | `watch` | kept. |
| `mcp` | `mcp` | kept. |
| `self formats\|doctor\|schema\|skill\|completions\|man` | same | kept: meta commands under one parent (rule 3). |

That is 24 top-level commands (`info check planes compare report preview stats trace table scans spectrum analyze export extract batch summarize link index search health export-dataset watch mcp self`), with 13 analyses under `analyze` and 6 meta commands under `self`.

**Why `analyze` stays a CLI group.** Each analysis is its own subcommand with its own flags, so no valid flag depends on a mode (rule 2 holds); the group keeps 13 entries out of the top-level help. The MCP side has one tool per analysis. The alternative, all 13 at the top level (37 commands), is listed under rejected options.

**Why `batch` keeps `MEASURE` and `options`.** `batch` maps one operation over many files. `options` is not a new union of arguments: it is exactly the argument object of the tool `measure` names (`batch peaks --set mz=[195.0877]` = the arguments of `openreadout_peaks`). The schema declares it as a plain object that points at that tool. This is the one place a value names another tool's arguments, and I think it is the honest shape for a map operation. If you disagree, the alternative is a `paths` argument plus the sample-sheet and `by` arguments on every tool that can batch, which would grow nine schemas by about 2 kB each.

### Proposed MCP tools

| v0.1.0 | proposed | change and rule |
| --- | --- | --- |
| `openreadout_info` | `openreadout_info` | kept. |
| `openreadout_check` | `openreadout_check` | integrity only. |
| `openreadout_check` with `against` | `openreadout_compare` | split (rules 1, 2). |
| `openreadout_check` with `report` | `openreadout_report` | split (rules 1, 2). |
| (none; `check --planes` is CLI-only) | (none) | plane hashes stay CLI-only: `openreadout_compare` covers the agent's question (is this copy the same data?). |
| `openreadout_preview` | `openreadout_preview` | kept. |
| `openreadout_stats` | `openreadout_stats` | kept. |
| `openreadout_trace` | `openreadout_trace` | kept. |
| `openreadout_table` | `openreadout_table` | kept. |
| `openreadout_spectra` | `openreadout_scans` | split (rules 1, 2). |
| | `openreadout_spectrum` | split (rules 1, 2). |
| `openreadout_analyze` kind=peaks | `openreadout_peaks` | split: one tool per analysis, its options become its arguments (rule 2). |
| kind=chromatogram | `openreadout_chromatogram` | same. |
| kind=nmr-peaks | `openreadout_nmr_peaks` | same. |
| kind=ephys-features | `openreadout_ephys_features` | same. |
| kind=spikes | `openreadout_spikes` | same. |
| kind=qpcr | `openreadout_qpcr` | same. |
| kind=gate | `openreadout_gate` | same. |
| kind=assay, analysis=wells | `openreadout_assay_wells` | same, and `analysis` is a second mode flag (rule 2). |
| kind=assay, analysis=curve | `openreadout_assay_curve` | same. |
| kind=assay, analysis=dose-response | `openreadout_dose_response` | same. |
| kind=assay, analysis=kinetics | `openreadout_kinetics` | same. |
| kind=assay, analysis=growth | `openreadout_growth` | same. |
| kind=assay, analysis=qc | `openreadout_assay_qc` | same. |
| `openreadout_export` | `openreadout_export` | conversion only; adds `csv` (see arguments). |
| `openreadout_export` with `attachment` | `openreadout_extract` | split (rules 1, 2). |
| `openreadout_batch` | `openreadout_batch` | kept. |
| `openreadout_batch` measure=summarize | `openreadout_summarize` | split (rule 2). |
| `openreadout_link` | `openreadout_link` | kept. |
| `openreadout_index` | `openreadout_index` | kept. |
| `openreadout_search` | `openreadout_search` | kept. |
| (CLI-only `search --health`) | `openreadout_health` | new in MCP: "which files in this share are truncated, duplicated or at risk" is an agent question, and the report is read-only. |
| (CLI-only `search --export`) | (none) | stays CLI-only: a long, resumable bulk write. |
| `openreadout_watch` | `openreadout_watch` | kept. |
| `openreadout_formats` | (none) | dropped from tools (rule 3). The `openreadout://formats` resource keeps the same JSON, and `openreadout_info` names the format of any file it opens. |

That is 32 tools, above the 20–25 the coordinator expected. The six assay tools account for five of the extra. Merging them back into one `openreadout_assay` would give 27, but breaks rule 2 (see rejected options).

Per tool, the proposed input schemas (estimated from today's option schemas) are each at most the size of the tool they come from: the largest is `openreadout_peaks` (33 arguments, about 5 kB, against 26 kB for `openreadout_analyze`). The six assay tools share 15 plate-layout arguments. I propose to keep the four everyone uses (`layout`, `blank_wells`, `positive_wells`, `negative_wells`) at the top level and put the other eleven (`table`, `read`, `wavelength_nm`, `embedded_layout`, `layout_text`, `empty_wells`, `roles`, `blank_subtraction`, `outliers`, `outlier_threshold`, `exclude_outliers`) in a nested object named `plate_options` (rule 5). The CLI keeps them as flat flags with the same names.

### Rejected options

- **One `openreadout_analyze` with `kind` (today's shape).** Fails rule 2: the valid options depend on `kind`, and the schema is the union of eight analyses (26 kB, 45 % of all input schemas).
- **One `openreadout_assay` with `analysis` (26 tools in total).** Fails rule 2: `standards`, `fit_on`, `lloq`, `uloq` apply only to curves, `growth_threshold` only to growth, `normalize` not to kinetics.
- **Merging `assay-wells` and `assay-qc`.** They take exactly the same arguments, but `assay-wells` returns per-well values and replicate groups, `assay-qc` returns plate-level quality metrics: rule 1 fails on the output.
- **Merging `trace`, `spectrum` and `chromatogram` (all 1-D signals).** They don't share arguments: `trace` reads a stored sweep (`trace`, `sweep`, `channels`, `first_sample`, `x_range`), `spectrum` one MS scan (`scan`, `spectrum`, `nth`, `centroid`), `chromatogram` computes signals from many scans (`mz`, `ppm`, `transitions`, `rt_range`, ...). Rule 1 fails.
- **Merging `trace` into `chromatogram` for detector traces.** `chromatogram` already reads stored detector traces (`traces`), but returns them thinned with apex and integral; `trace` returns raw samples and statistics. Different output, so both stay.
- **Merging `stats --per well` back into a separate `well-stats`.** It returns rows per well instead of per channel, but it is the same measurement from the same arguments; the split would duplicate eight arguments for one (`wells`).
- **Splitting `info` views into `info`, `dump`, `explain`, `ls`, `detect` again.** All answer "what is in this file" from `file` alone; only `ask`, `vendor`, `max_frames` and `thumbnail` are view-specific, and each says which view it applies to.
- **All 13 analyses at the top level of the CLI.** 37 top-level commands; the `analyze` group costs nothing in rule 2 terms because each subcommand has its own flags.
- **Grouping `index`, `search`, `health`, `export-dataset` under one CLI parent.** Possible under rule 3 only if they were meta commands; they are the main way to work with a file share, and `openreadout index DIR -o IDX` reads well as is.
- **Renaming `spectra`/`scans` to say "mass" in the name.** `info` already says `spectra[]` holds MS runs and `traces[]` holds 1-D spectra (IR, Raman, UV-Vis, NMR). `scans` is mass-spectrometry vocabulary, and both tool descriptions will start with "Mass spectrometry".
- **Keeping `openreadout_formats` as a tool.** It is meta (rule 3), and the resource has the same content.

## Arguments

Arguments not listed here keep their names on both surfaces. "CLI only" arguments are deliberate: they are listed in each command's book page and are left out of MCP to keep schemas small (chunk sizes, compression, man pages and the like).

### Renamed or moved

| command / tool | v0.1.0 CLI | v0.1.0 MCP | proposed (CLI / MCP) | why |
| --- | --- | --- | --- | --- |
| `info` | `--no-vendor` (vendor tree on by default) | `vendor` (off by default) | `--vendor` / `vendor`, off by default | the skill and `explain` both tell agents to add `--no-vendor`; the tree can be megabytes. |
| `info` | `--all-frames` (default first 100) | `max_frames` (100; 0 none; -1 all) | `--max-frames N` / `max_frames` | one argument with the same values. |
| `stats` | `--no-planes` (planes on by default) | `per: plane` adds them | `--per plane` / `per: plane`; no planes by default | `explain` suggests `--no-planes` every time; one way to ask. |
| `trace` | `--first` | `first_sample` | `--first-sample` / `first_sample` | same name; `table` already has `--first-row`. |
| `table` | `--parameter` | `transform_parameters` | `--parameter` / `parameters` | the batch `table` measure already calls it `parameters`. |
| `preview` | `--plain`, `--grid` | `axes`, `grid` | `--axes rulers\|grid\|none` / `axes` (default `rulers`) | two booleans that conflict become one choice; one argument fewer. |
| `scans` (was `spectra`) | `--rt START-END` | `rt_range` | `--rt-range` / `rt_range` | `chromatogram` uses `rt_range`; in `peaks`, `rt` is one retention time. |
| `scans` | `--precursor` | `precursor_mz` | `--precursor` / `precursor` | as `chromatogram`. |
| `scans` | `--tol DA` | (none) | `--precursor-tol` / `precursor_tol` | as `chromatogram`; restored in MCP. |
| `scans` | `--ppm` | `ppm` | `--precursor-ppm` / `precursor_ppm` | in `chromatogram`, `ppm` is the XIC tolerance. |
| `scans` | `--filter TEXT` | `scan_filter` | `--scan-filter` / `scan_filter` | as `chromatogram`; in `table`, `--filter` is a row condition. |
| `scans` | `--count` | `limit: 0` | `--count` / `count` | as `table`; `limit: 0` stays valid. |
| `spectrum` (was `spectra`) | `--index I` | `index` | `--spectrum I` / `spectrum` | as `preview`, which already says `--spectrum`. |
| `export` | `--to FORMAT` | `format` | `--format` / `format` | same name; `preview` also says `--format`. |
| `export` | `--to csv` | (none) | `csv` in MCP too | the three failed export calls in the evals. Default format is the same on both: `mzml` for mass spectra, `csv` for tables and traces, `ome-tiff` for images (MCP defaulted to `ome-tiff` for tables). |
| `export` | `--labels` | (none) | `--labels` / `labels` | goes with CSV. |
| `extract` (was `export --attachment`) | `--attachment NAME` | `attachment` | positional `NAME` / `attachment` | the name is the whole request. |
| `compare` (was `check --against`) | `check A --against B` | `file`, `against` | `compare A B` / `file`, `against` | |
| `report` (was `check --report`) | `--report` | `report: true` | the command / the tool | |
| `planes` (was `check --planes`) | `--planes` | | the command | |
| `batch` | positional `INPUT...` | `inputs` | positional `PATH...` / `paths` | as `link`. |
| `batch` | `MEASURE spectra` | `measure: spectra` | `scans` | follows the command. |
| `summarize` (was `batch summarize`) | `batch summarize TABLE` | `measure: summarize`, `inputs: [TABLE]` | `summarize TABLE` / `table` | |
| `health` (was `search --health`) | `search IDX --health` | (none) | `health INDEX_DIR` / `index_dir` | |
| `export-dataset` (was `search --export`) | `search IDX Q --export OUT` | (none) | `export-dataset INDEX_DIR QUERY OUT` | |
| `index` | `--health` | (none) | removed | duplicate of `health`. |
| `watch` | `--stall-after` | `stall_after_s` | `--stall-after` / `stall_after` | same name; the description says seconds. |
| `nmr-peaks` | `--range A:B` | `range_ppm` | `--range-ppm` / `range_ppm` | |
| `ephys-features` | `--peak-threshold MV` | `peak_threshold_mv` | `--peak-threshold-mv` / `peak_threshold_mv` | |
| `spikes` | `--band LOW:HIGH` | `band_hz` | `--band-hz` / `band_hz` | |
| `qpcr` | `--cq` | `compute_cq` | `--compute-cq` / `compute_cq` | |
| `qpcr` | `--baseline START-END` | `baseline_start`, `baseline_end` | `--baseline-start`, `--baseline-end` / same | |
| `qpcr` | `--reference TARGET` | `reference_targets` | `--reference-target` / `reference_targets` | |
| `qpcr` | `--control SAMPLE` | `control_sample` | `--control-sample` / `control_sample` | `--control` is also the summary test's control group. |
| `qpcr` | `--undetermined-as CQ` | `undetermined_cq` | `--undetermined-cq` / `undetermined_cq` | |
| assay tools | `--blank WELLS` | `blank_wells` | `--blank-wells` / `blank_wells` | |
| assay tools | `--blank-subtraction MODE` | `blank` | `--blank-subtraction` / `blank_subtraction` | `blank` meant wells on the CLI and a mode in MCP. |
| assay tools | `--positive`, `--negative`, `--empty` | `positive_wells`, `negative_wells`, `empty_wells` | `--positive-wells`, `--negative-wells`, `--empty-wells` / same | |
| assay tools | `--wavelength NM` | `wavelength_nm` | `--wavelength-nm` / `wavelength_nm` | |
| `growth` | `--threshold OD` | `growth_threshold` | `--growth-threshold` / `growth_threshold` | |
| `assay-curve`, `dose-response` | `--preview PNG` | `plot: true` | `--plot PNG` / `plot` | as `peaks --plot`. |
| assay tools | | `analysis` | removed | the tool is the analysis. |
| analysis tools | | `kind`, `options` | removed | the tool is the analysis; options become arguments. |

### Kept as they are, and why

- `index` keeps `-o/--output INDEX_DIR` on the CLI and `index_dir` in MCP: the CLI value name is `INDEX_DIR`, as for `search`, `health`, `from_index`.
- `index` keeps `--no-pii` / `pii` (the default-on convention).
- `compare` keeps `--no-pixels` / `no_pixels`, `link` keeps `--no-recursive` / `no_recursive`, and `health` has `--no-hash` / `no_hash`. These flags default to off, so the name is the same on both surfaces.
- Peaks keeps `--targets FILE` on the CLI and `compounds` (inline list) in MCP. They are different inputs (a file, or the list itself).
- `strict` stays on every tool that reads values, as an override of the server default.

### CLI only, unchanged

`info`: `--no-provenance`, `--redact`, `--salt-file`, `--sidecar`, and the table flags (`--tidy`, `--sample-sheet`, `--by`, ...) that `stats`, `trace`, `table`, `gate` also take. `report`: `--dry-run`, `--hex`, `--full-check`, `--no-hash`, `--no-first-read`, `--overwrite`. `compare`: `--include-extra`, `--no-metadata`, `--level`. `planes`: everything. `preview`: `-o`, `--quality`, `--overwrite`, NMR `--process*`. `export`: `--compression`, `--chunk-size`, `--levels`, `--pyramid`, `--embed-vendor`, `--skip-incomplete`, `--no-plate`, `--per-image`, NMR `--process*`. `scans`: `--csv`. `spectrum`: `--exclude-flagged`. `peaks`: `--min-points`, `--skim-ratio`, `--noise`, `--baseline-window`, `--targets`, `-o`, `--plot`, `--width`. `nmr-peaks`: `--gb`, `--no-group-delay`. `spikes`: `--exclude-ms`, `--order`. `link`: `--max-shared`. `index`: `--follow-symlinks`. `watch`: `--interval`, `--once`, `--timeout`, `--qc-rules`, `--print-qc-rules`, `--max-tracked`. `export-dataset`, `self`, `mcp`: everything.

## Server instructions

Today's instructions are already short (one paragraph: zero-based indices, raw values, assurance, which tools write, errors carry a hint). With one tool per operation, the tool names carry the routing, so they stay as they are, plus one sentence on the naming conventions above so an agent can move between the CLI and MCP.

## What else changes

The skill (`skills/openreadout/` and its embedded copy), the book (command pages, the MCP page, guides and recipes), the README command examples, `docs/`, `packaging/agent-plugins/`, `mcpb/manifest.json`, the Galaxy, Nextflow, Snakemake and bioconda test commands, the Python and R bindings where they name commands or options (`openreadout.analyze(..., kind)` and `openreadout_analyze()` keep their kind argument: a library function is not an agent tool, and the R and Python docs already list options per kind), the JSON Schemas (`schema_version` bump where output keys change: the `info --view full` default without `vendor`, `stats` without `planes`), the golden snapshots, `oracle/mcp_smoke.py`, and the evals harness.

The two export implementations (CLI `commands/export.rs`, MCP `tools/export.rs`) stay separate, but the CSV writer moves from the CLI crate into `openreadout-ops` so that both use it.

## Appendix: the v0.1.0 consolidation

For the record, the names before 1 October 2026 and where they went in v0.1.0. The proposal above restores several of these as separate commands and tools.

| before v0.1.0 (CLI) | v0.1.0 | proposed |
| --- | --- | --- |
| `info`, `dump`, `explain`, `ls`, `detect` | `info --view summary\|full\|explain\|structure\|format` | `info --view ...` |
| `check` | `check` | `check` |
| `planes` | `check --planes` | `planes` |
| `compare` | `check --against` | `compare` |
| `report` | `check --report` | `report` |
| `preview` | `preview` | `preview` |
| `stats`, `stats --by-well` | `stats --per ...` | `stats --per ...` |
| `trace` | `trace` | `trace` |
| `table` | `table` | `table` |
| `scans` | `spectra` | `scans` |
| `spectrum` | `spectra --scan/--index/--nth` | `spectrum` |
| `peaks`, `chromatogram`, `nmr-peaks`, `ephys-features`, `spikes`, `qpcr`, `gate` | `analyze <kind>` | `analyze <kind>` |
| `assay <analysis>` | `analyze assay <analysis>` | `analyze assay-wells\|assay-curve\|dose-response\|kinetics\|growth\|assay-qc` |
| `export` | `export` | `export` |
| `extract` | `export --attachment` | `extract` |
| `batch` | `batch` | `batch` |
| `summarize` | `batch summarize` | `summarize` |
| `link` | `link` | `link` |
| `index` | `index` | `index` |
| `search` | `search` | `search` |
| `health` | `search --health` | `health` |
| `export-dataset` | `search --export` | `export-dataset` |
| `watch` | `watch` | `watch` |
| `formats`, `doctor`, `schema`, `skill`, `completions`, `man` | `self <name>` | `self <name>` |
| `mcp` | `mcp` | `mcp` |

| before v0.1.0 (MCP) | v0.1.0 | proposed |
| --- | --- | --- |
| `info`, `dump`, `explain`, `ls`, `detect` | `openreadout_info` with `view` | `openreadout_info` with `view` |
| `check` | `openreadout_check` | `openreadout_check` |
| `compare` | `openreadout_check` with `against` | `openreadout_compare` |
| `report` | `openreadout_check` with `report` | `openreadout_report` |
| `planes` | dropped | (none) |
| `describe` | dropped (tools declare output schemas) | (none) |
| `preview` | `openreadout_preview` | `openreadout_preview` |
| `stats`, `well_stats` | `openreadout_stats` with `per` | `openreadout_stats` with `per` |
| `trace` | `openreadout_trace` | `openreadout_trace` |
| `table` | `openreadout_table` | `openreadout_table` |
| `scans`, `spectrum` | `openreadout_spectra` | `openreadout_scans`, `openreadout_spectrum` |
| `peaks`, `chromatogram`, `nmr_peaks`, `ephys_features`, `spikes`, `qpcr`, `gate` | `openreadout_analyze` with `kind` | one tool each |
| `assay` | `openreadout_analyze` kind=assay | `openreadout_assay_wells`, `_assay_curve`, `_dose_response`, `_kinetics`, `_growth`, `_assay_qc` |
| `export`, `extract` | `openreadout_export` | `openreadout_export`, `openreadout_extract` |
| `batch`, `summarize` | `openreadout_batch` | `openreadout_batch`, `openreadout_summarize` |
| `link` | `openreadout_link` | `openreadout_link` |
| `index` | `openreadout_index` | `openreadout_index` |
| `search` | `openreadout_search` | `openreadout_search` |
| (none) | (none) | `openreadout_health` |
| `watch` | `openreadout_watch` | `openreadout_watch` |
| `formats` | `openreadout_formats` | resource `openreadout://formats` only |
