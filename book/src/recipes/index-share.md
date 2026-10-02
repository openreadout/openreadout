# Index and search a lab share

Use this when a shared drive holds years of instrument data and you want to know what is there: which files were taken with a 63x objective, which are truncated, which have no open-format copy. `index` builds a catalog of every data set from its headers, and `search` queries it.

## Run it

```text
$ openreadout index share -o idx
indexed .../idx: 10 data sets, 11 files (470.1 kB) in 11 item(s) of .../share
crawl: 0.0 s, 1175 items/s, 11 new, 0 changed, 0 unchanged, 0 removed, 1 unrecognised, 0 unreadable; ...

format  data sets     bytes
──────  ─────────  ────────
abf             1   60.4 kB
czi             2  128.9 kB
fcs             2   91.2 kB
lif             1    1.8 kB
mzml            1   25.1 kB
nd2             3  162.8 kB

check: ok 8, truncated 2
personal data: 2 flags in 1 data sets (free_text 2)
```

`share` here is a scratch folder with copies of public test files from the repository (CZI, ND2, LIF, FCS, mzML and ABF), one ND2 cut short and one text file. The output on this page is real, with long lines and paths trimmed.

## What it tells you

- The crawl reads headers only: what `info` reads, the headers-only integrity check, and the first and last 64 KiB of each file for a fingerprint. It doesn't read pixels, events or spectra, so it is quick on a large share.
- `idx` is a directory of Parquet tables (`experiments.parquet`, `files.parquet`, `problems.parquet`) and an `index.json` manifest. There is no server or database: DuckDB, pandas, polars or Excel read the tables directly.
- `check:` counts data sets per integrity status. One truncated file here is the cut ND2, the other a 32 KiB test FCS file whose events need more bytes than it has.
- `personal data:` counts flags but doesn't show the values. Here one FCS file has a free-text comment that should be reviewed before it is shared.
- Run the same command again to update the index. Unchanged files are not opened again (`0 new, 0 changed, 11 unchanged`). An interrupted crawl continues where it stopped.

## Variations

### Search it

```text
$ openreadout search idx "format=czi|nd2" --fields path,format,objective,pixel,z_step
path                              format  objective_magnification  objective                 ...
────────────────────────────────  ──────  ───────────────────────  ────────────────────────  ...
aics-ND2-dims-rgb.nd2             nd2                        10.0  Plan Fluor 10x Ph1 DLL    ...
cut.nd2                           nd2                        10.0  Plan Fluor 10x            ...
mini.czi                          czi                                                        ...
mini.nd2                          nd2                        10.0  Plan Fluor 10x            ...
zenodo10577621-LineScan-Z200.czi  czi                        10.0  EC Plan-Neofluar 10x/0.3  ...
5 of 5 matching data sets (paths under .../share/scope)
```

All terms must match; `|` gives alternatives within a term, `OR` between terms. Comparisons take units and partial dates:

```bash
openreadout search idx "objective=63x channel~GFP acquired<2020"
openreadout search idx "status=truncated|corrupt" --fields path,format,check_codes
openreadout search idx "format=fcs events>10000" --sort -size
```

The fields and aliases (`objective`, `pixel`, `channel`, `sample`, `status` and many more) are listed in the [query language](../guides/lab-shares.md#query-language). `--json` adds `total`, the number of matches, which answers "how many" questions.

### Storage health

```text
$ openreadout search idx --health -o health.md
...
| data sets | 10 (470.1 kB) |
| files seen | 11 |
| unrecognised files | 1 (6 B) |
| truncated, corrupt or unreadable | 2 |
| duplicate groups | 0 (0 B redundant, confirmed by SHA-256) |
| same experiment stored twice | 0 (0 exported twice) |
| at risk (vendor-only, no open export) | 7 (353.9 kB; 0 legacy) |
| data sets with possible personal data | 1 (2 flags) |
...
```

The report is Markdown, with sections on integrity, duplicates, the same experiment stored twice, files at risk and personal data.

### Personal data

`search idx pii=true --fields path,pii_kinds,pii_fields` lists the flagged data sets and the fields that triggered each flag. To share a selection, export it with `--redact`, which replaces every flagged value with a stable salted hash:

```bash
openreadout search idx "family=microscopy status=ok" --export dataset/ --redact --salt-file ~/.openreadout-salt
```

### From an assistant

`openreadout_index` crawls in capped sessions: call it again with the same arguments while it answers `complete: false`. `openreadout_search` takes the same queries and returns `total`, so an assistant can answer "how many CZI files here were taken with a 63x objective?".

## More

- [Lab shares, indexes and live acquisitions](../guides/lab-shares.md): every column, the query language, [storage health](../guides/lab-shares.md#storage-health) and [personal data](../guides/lab-shares.md#personal-data).
- [`index`](../reference/commands/index-cmd.md) and [`search`](../reference/commands/search.md) references.
- JSON: [`search`](../reference/json/search.md), [`search --health`](../reference/json/search-health.md).
