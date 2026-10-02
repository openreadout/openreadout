# table

`table` returns rows of a table: FCS events, plate-reader values, spike, event or peak tables. For FCS files it can compensate and transform the values and add population membership from a FlowJo workspace or Gating-ML file.

```text
openreadout table [OPTIONS] [FILE]...
```

`info` lists a file's tables under `tables[]`.

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--table N`: table index (FCS data set, plate read). Default 0 (in a batch table, every table).
- `--first-row N`: zero-based first row. Default 0.
- `--max-rows N`: rows to return, at most 1000000. Default 100.
- `--filter CONDITION`: only rows meeting `COLUMN OP NUMBER`, with OP one of `>`, `>=`, `<`, `<=`, `==`, `!=`. Repeatable or joined with `&&`. The whole table is scanned.
- `--count`: count the matching rows only; return no rows.

### Flow cytometry

- `--compensate [auto|fcs|gating]`: compensate first. Without a value, `auto`: the gating file's matrix when one is given, else the file's own spillover matrix.
- `--transform SPEC`: transform after compensation: `logicle`, `arcsinh`, `hyperlog`, `log`, `linear`, `biex`, `flowjo-log`, `arcsinh-cofactor`, with parameters as `NAME:K=V,…` (for example `logicle:T=262144,W=0.5,M=4.5,A=0`), or `workspace` for the FlowJo sample's own transforms.
- `--parameter NAME`: parameters to transform. Default: every fluorescence parameter. Repeatable or comma-separated.
- `--workspace WSP`: FlowJo workspace (`.wsp`).
- `--gatingml XML`: Gating-ML 2.0 document.
- `--sample NAME`: workspace sample, by name or id.
- `--population PATH`: append a 0/1 column `gate:<PATH>` with this population's membership. Repeatable.

Without these flags, `table` returns raw stored values.

Several files, a directory or a glob give one summary table: per FCS file and parameter (events, median, mean, sd, min, max), or per plate well and read. `table` takes the flags in [Several inputs](index.md#several-inputs) and the [batch table flags](batch.md#batch-table-flags).

## Examples

```console
$ openreadout table doctor.fcs --max-rows 3
FSC-A	SSC-A	Time
0.5	1.5	2.5
10.5	11.5	12.5
20.5	21.5	22.5
# rows 0..3 of 10 (table 0)
```

Count events without exporting them:

```console
$ openreadout table doctor.fcs --filter "FSC-A > 50" --count
# 5 of 10 rows (50.0000%) meet FSC-A > 50
```

Compensated, logicle-transformed events with membership of one gate:

```bash
openreadout table sample.fcs --compensate --transform logicle \
  --workspace analysis.wsp --population /Lymphocytes/Singlets --json
```

To count every population of a gating tree, use [`analyze gate`](analyze.md#gate).

## JSON

[`table`](../json/table.md), [batch table](../json/batch-table.md).

Run `openreadout table --help` for the full help of your installed version.
