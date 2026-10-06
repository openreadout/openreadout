# Commands

`openreadout` has nineteen commands. Each one has its own page:

- [`info`](info.md): what a file holds. Reads headers only.
- [`check`](check.md): validate a file's integrity.
- [`planes`](planes.md): hash every plane of a file.
- [`compare`](compare.md): compare a file with a second file, such as its export.
- [`report`](report-cmd.md): write a diagnostic bundle for a file OpenReadout cannot read.
- [`export`](export.md): convert to an open format (OME-TIFF, OME-Zarr, CSV, Parquet, Arrow, mzML, NWB, JCAMP-DX, Allotrope ASM, RDML).
- [`preview`](preview.md): render a PNG or JPEG of an image plane, a trace, a mass spectrum or a plate.
- [`stats`](stats.md): pixel statistics per plane, channel, image or plate well.
- [`trace`](trace.md): samples and statistics of one sweep of a 1-D signal.
- [`table`](table.md): rows of a table, such as FCS events or plate-reader values.
- [`spectra`](spectra.md): mass-spectrometry scan headers, or one spectrum.
- [`analyze`](analyze.md): peaks, chromatograms, NMR peaks, patch-clamp features, spikes, qPCR, flow gating and plate-reader assays, one subcommand each.
- [`batch`](batch.md): any measure over many files as one tidy table.
- [`link`](link.md): group files that measured the same sample.
- [`index`](index-cmd.md): catalog every data set under a directory.
- [`search`](search.md): query an index, report its storage health, or export a selection.
- [`watch`](watch.md): follow directories where an instrument is writing.
- [`self`](self.md): this installation (formats, self-test, JSON Schemas, agent skill, completions, man pages).
- [`mcp`](mcp.md): run as an MCP server, or configure a client.

`openreadout <command> --help` is the authoritative list of flags for the version you have installed. These pages describe each flag in one line and add what the help text leaves out.

The [MCP tools](../mcp.md) are the same operations under the same names: `openreadout_` plus the command, or the `analyze` subcommand, with `-` as `_` (`analyze nmr-peaks` is `openreadout_nmr_peaks`). Their arguments are the long flags with `-` as `_` (`--rt-range` is `rt_range`). A repeatable flag is singular on the command line and a plural list in MCP (`--channel 0 --channel 2` is `channels: [0, 2]`), and a flag that turns off something on by default is `--no-X` on the command line and `X: false` in MCP.

Every command prints human-readable text by default. With `--json` it prints the JSON wrapper described in [Reading the JSON output](../../getting-started/reading-json.md). The fields of each command's `data` are listed in the [JSON reference](../json/index.md). Commands don't write to the files they read.

## Global flags

These work with every command, before or after the command name (`openreadout --threads 4 export …`).

- `--threads N`: worker threads for plane decoding in `export`, `planes` and `stats`, and for the files `index` reads. Default: the number of CPUs. The output is the same for every value.
- `--color auto|always|never`: colour in human-readable output. `auto` colours terminals only and honours `NO_COLOR` and `CLICOLOR_FORCE`. JSON is never coloured.
- `--progress`: show progress on stderr even when stderr is not a terminal.
- `--no-progress`: never show progress.
- `-q`, `--quiet`: no human-readable output on success, no progress and no batch summary. Errors still go to stderr, and JSON output is unchanged.
- `--live-window SECONDS`: an incomplete file changed less than this long ago is reported as still being written (`acquisition.state: in_progress`) rather than interrupted. Default 300, or `OPENREADOUT_LIVE_WINDOW`; `0` turns it off. See [Live acquisition](../../guides/lab-shares.md).
- `--only POINTERS`: keep only these values of `data`, as JSON pointers (`/images/0/physical_size`; `*` maps over array elements, as in `/images/*/name`). Comma-separated or repeated. Implies JSON. Pointers that match nothing are listed under `missing`.
- `--compact`: print JSON on one line instead of indented.
- `--strict`: refuse (exit 6, with a hint) to return values that this file's assurance does not validate. Also `OPENREADOUT_STRICT=1`. See [Assurance](../assurance.md).

## Several inputs

`info`, `check`, `planes`, `stats`, `export`, `trace`, `table`, `analyze peaks`, `analyze chromatogram` and `analyze gate` accept several files, directories or glob patterns. A directory that a reader recognises as one data set (a ChemStation `.D`, a Bruker `.d`, a Waters `.raw`) is one input. These flags control the run:

- `-r`, `--recursive`: walk sub-directories of directory arguments.
- `--jsonl`: one compact JSON wrapper per input and line, each with its `path`.
- `--continue-on-error`: keep going after an input fails. This is the default.
- `--fail-fast`: stop at the first input that fails.
- `--skip-unknown`: leave out files that are not instrument files instead of reporting them.

```bash
openreadout check -r --jsonl /data/run42 > report.jsonl
openreadout export -r --skip-unknown raw/ -o ome/
```

`export -r data/ -o out/` keeps each input's relative path under `out/`. To turn many files into one table, see [`batch`](batch.md).

## Exit codes

Exit codes are part of the public interface. Scripts and agents can branch on them.

| code | meaning | JSON `error.code` |
| --- | --- | --- |
| 0 | success | – |
| 1 | error | `error`, `internal_panic` |
| 2 | usage: bad arguments, bad selection, index out of range | `usage` |
| 3 | unknown format | `unknown_format` |
| 4 | corrupt, truncated or empty file | `corrupt_file` |
| 5 | I/O: missing file, permission denied, read error | `io` |
| 6 | unsupported feature of a known format | `unsupported_feature` |

Every error carries a `hint` that says what to do next. Human output prints `error: …` and `hint: …` on stderr. With `--json` the error wrapper goes to stdout.

Special cases:

- With several inputs, the exit code is the highest code of any input. Each input's own code is in its JSON wrapper or in the summary table.
- A batch table (`batch`, or `--tidy`, `--sample-sheet`, `--by`, `--csv` on another command) exits 0 once the table is built: a data set that failed is a row with an `error` column. `--fail-fast` exits with the first failure's code instead.
- `compare` exits 0 when the two files hold the same data and 1 when they differ.
- `self doctor` exits 1 when a self-test check fails.
- A file that an instrument is still writing (OME-TIFF, OME-Zarr, ND2, CZI) is not corrupt. It exits 0 with `acquisition.state: in_progress`.
- `internal_panic` is a bug. Please report it. No stack trace is printed unless `RUST_BACKTRACE` is set.

## Standard input

`-` reads the file from standard input for `info`, `check`, `planes` and `stats`:

```bash
cat sample.czi | openreadout info -
```

The input is copied to a temporary file so that formats that need random access work. The copy is capped at 4 GiB; `OPENREADOUT_STDIN_MAX_BYTES` changes the cap. Formats stored as directories or as several files cannot be read this way.

## Shell completions and man pages

```bash
openreadout self completions bash > ~/.local/share/bash-completion/completions/openreadout
openreadout self completions zsh  > "${fpath[1]}/_openreadout"
openreadout self completions fish > ~/.config/fish/completions/openreadout.fish
openreadout self man --out ~/.local/share/man/man1
```

See [`self`](self.md) for the other shells.
