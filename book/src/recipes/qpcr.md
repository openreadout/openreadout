# Check a qPCR run

Use this after a qPCR run to see the Cq of every well, the efficiency and R² of a standard curve, and relative expression by ΔΔCq. It reads RDML files, Applied Biosystems `.eds`, Rotor-Gene `.rex` and LightCycler 480 `.ixo`.

## Run it

```text
$ openreadout analyze qpcr rdml-stepone-std.rdml --standard-curve
rdml-stepone-std.rdml (rdml, rdml): Standard Curve Example
instrument: Applied Biosystems StepOne™ Instrument
24 well x target records
well   sample                 target            task        Cq        Tm
A1     NTC_RNase P            RNase P           ntc         40.000         -
...
A4     pop1_RNase P           RNase P           unknown     28.963         -
A5     pop1_RNase P           RNase P           unknown     28.839         -
...
B2     STD_RNase P_10000.0    RNase P           standard    26.874         -
...
C8     STD_RNase P_625.0      RNase P           standard    31.035         -

Cq: 24 determined, 0 undetermined, 0 no result (0 excluded)

standard curve RNase P: slope -3.4770, intercept 40.768, R² 0.9995, efficiency 93.9 % (15 wells, 5 levels)
note: target RNase P: amplificationEfficiency 93.91181 read as a percentage (1.9391 fold)
```

`rdml-stepone-std.rdml` is an RDML file written by StepOne Software, from the RDML R package's examples (MIT). It is committed at [`fuzz/corpus/core_zip/rdml-stepone-std.rdml`](../../../fuzz/corpus/core_zip/rdml-stepone-std.rdml): one target (RNase P), five standards from 625 to 10000 copies in triplicate, two unknown populations and three NTCs. The output on this page is real, with long tables trimmed.

## What it tells you

- The table has one row per well and target. `Cq` is the value the file stores, as the instrument software called it. `undet.` marks wells the file says did not amplify; they are left out of averages.
- Look at the NTC wells. Here they read `40.000`, which is the run's cycle count: StepOne wrote that number into this RDML file, and OpenReadout reports it as written. OpenReadout reads a Cq at the cycle count as undetermined only in SDS-layout `.eds` files, where the software is known to write it that way, so in RDML check NTCs by eye.
- The standard curve is a least-squares line of Cq against log10(quantity) over the standard wells. A slope of −3.32 means 100 % efficiency; efficiency = (10^(−1/slope) − 1) × 100, here 93.9 %.
- When the file stores the vendor's own fit, its slope, efficiency and R² are given next to ours (`vendor_slope`, `vendor_efficiency_percent` and `vendor_r2` in the JSON). This file stores only the target's efficiency, 93.91181, which matches.

## Variations

### Relative expression (ΔΔCq)

```bash
openreadout analyze qpcr plate.eds --ddcq --reference GAPDH --control untreated
```

`--reference` names the endogenous control and `--control` the calibrator sample. Both default to the ones the file records. Each sample and target gets ΔCq, ΔΔCq and RQ = 2^−ΔΔCq with its range. A file without a reference target gives a usage error that lists its targets:

```text
$ openreadout analyze qpcr rdml-stepone-std.rdml --ddcq
error: usage error: ΔΔCq needs a reference target (endogenous control): pass --reference TARGET; this file's targets: RNase P
hint: Check the arguments with `openreadout help <command>`; `openreadout info FILE --json` shows what the file holds.
```

### Recompute Cq from the curves

`--cq` computes a threshold Cq for every curve and compares it with the stored one:

```text
$ openreadout analyze qpcr rdml-stepone-std.rdml --cq
...
our Cq vs vendor: 24 curves, 24 both with Cq, 0 both undetermined, 0 only vendor, 0 only ours; mean diff -7.369, median |diff| 6.909, max |diff| 28.629, within 0.5 cycles 0.042, r = 0.50760
```

The two agree closely only when the file records the threshold and baseline the software used. This StepOne RDML file records neither, so OpenReadout falls back to its own automatic threshold, and the difference above is a difference of method, not a reading error. Use the stored Cq for such files, and set `--threshold` and `--baseline` to match your software's settings when you need the comparison. See [qPCR formats](../formats/qpcr.md).

### Many runs

```text
$ openreadout batch qpcr qruns/ --set standard_curve=true
2 data sets (2 ok, 0 failed)
path                         format  target   points  levels   slope  intercept      r2  efficiency_percent
...
qruns/rdml-stepone-std.rdml  rdml    RNase P      15       5  -3.477    40.7681  0.9995             93.9102
qruns/small.rdml             rdml    -             -       -       -          -       -                   -
```

`small.rdml` has a single standard well, so it gets no curve. `--set ddcq=true` gives one row per sample and target instead.

### From an assistant

The MCP tool is `openreadout_analyze` with `kind: "qpcr"` and options such as `standard_curve`, `ddcq`, `reference_targets` and `control_sample`.

## More

- [qPCR formats](../formats/qpcr.md): what each format stores and how Cq, ΔΔCq and the standard curve are computed.
- [`analyze` reference](../reference/commands/analyze.md#qpcr): every flag.
- JSON: [`qpcr`](../reference/json/qpcr.md).
