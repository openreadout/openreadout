# Assurance and strict mode

A reader can pass all its tests and still return plausible but wrong numbers for a variant it hasn't seen before: a new format version, another writer, an unusual codec. So `info` and `check` tell you whether a file is a variant its reader has been validated on, meaning that development files of the same variant were read correctly and confirmed by an independent reader. With `--strict`, OpenReadout refuses to return values that haven't been validated.

## The `assurance` block

`info` (every view) and `check` print one line:

```text
  assurance: partially validated - decoded along validated paths, but writer version Bio-Formats 8: not in the development corpus (descriptive; decoding does not depend on it)
```

With `--json`, the full block is under `data.assurance`. Here it is for a TIFF file whose pixels use a codec no development file has:

```bash
openreadout info lerc_uint16.tif --json | jq .data.assurance
```

```json
{
  "level": "unvalidated",
  "summary": "UNVALIDATED for pixels: codec lerc: never seen in a development-corpus file, so how it is decoded is unvalidated; values there may be wrong (--strict refuses them)",
  "fingerprint": "tiff|codec=lerc|dialect=plain|format_version=6.0|layout=strips|sample_layout=uint16",
  "variant": [
    {"kind": "format_version", "value": "6.0", "scope": ["metadata", "pixels"], "status": "validated", "corpus_files": 92, "sources": 19},
    {"kind": "sample_layout", "value": "uint16", "scope": ["pixels"], "status": "validated", "corpus_files": 28, "sources": 11},
    {"kind": "dialect", "value": "plain", "scope": ["metadata", "pixels"], "status": "validated", "corpus_files": 45, "sources": 6},
    {"kind": "codec", "value": "lerc", "scope": ["pixels"], "status": "unseen", "corpus_files": 0, "sources": 0},
    {"kind": "layout", "value": "strips", "scope": ["pixels"], "status": "validated", "corpus_files": 88, "sources": 19}
  ],
  "reasons": ["[pixels] codec lerc: never seen in a development-corpus file, so how it is decoded is unvalidated"],
  "inferred_fields": {"count": 1, "of": 8, "examples": ["images[].size_z"]},
  "strict_refuses": ["pixels"],
  "reader_confidence": "high"
}
```

The file is `crates/openreadout-tiff/tests/fixtures/codecs/lerc_uint16.tif` in the repository. The counts in `variant` change as the test corpus grows.

### Level

- `validated`: every feature of the file's variant was read correctly on development files that an independent reader confirmed, and no part of the file was skipped, assumed or left uncalibrated.
- `partially_validated`: values are decoded along validated paths, but something is less certain. For example, a descriptive feature such as the writer version hasn't been seen before, a structure was skipped, or a value was assumed.
- `unvalidated`: some output depends on something that hasn't been validated, so its values may be wrong.

`summary` says the same in one line, for people and agents.

### Fingerprint and variant

The `fingerprint` is the format id followed by `kind=value` for every feature, sorted. Files with the same fingerprint are the same variant, so you can group a share's files by it; the [index](../guides/lab-shares.md) stores it as the `variant` column.

Each entry of `variant` is one feature:

- `kind` and `value`: for example `codec` and `lerc`.
- `scope`: the outputs whose decoding depends on the feature. An empty scope means the feature only describes the file.
- `status`: `validated`, `seen` (only on files no independent reader confirmed) or `unseen`.
- `corpus_files` and `sources`: how many development files, from how many independent depositors, confirm it.

The scopes are:

- `metadata`: the normalized summary (`info`)
- `pixels`: image planes (`stats`, `preview`, image export)
- `spectra`: mass spectra and scan headers, and chromatograms built from them
- `traces`: sampled signals, such as electrophysiology sweeps, detector chromatograms and IR, Raman or NMR spectra
- `tables`: FCS events, plate reads, qPCR results and other tables

### Other fields

- `reasons`: why the level is not `validated`, one line each. A line about one scope starts with `[scope]`.
- `undecoded`: structures the reader found but did not decode. With a `scope`, values in those outputs may be incomplete.
- `assumed`: values the reader assumed instead of reading, such as a default unit or a day/month order.
- `inferred`: values derived by a rule instead of read from a field that states them, with the rule and whether it was validated.
- `calibrations`: vendor calibrations the file carries, each `applied`, `not_applied` or `available` (for example FCS compensation, which `table --compensate` applies).
- `inferred_fields`: how many normalized fields have a meaning that was worked out from files rather than taken from a specification. See [Provenance](../guides/metadata.md#provenance).
- `strict_refuses`: the outputs `--strict` refuses for this file.
- `strict_withholds`: single fields `--strict` replaces with null (below).
- `reader_confidence`: the reader's overall level (`high`, `medium` or `low`), for context. It is computed from the test evidence by a fixed [rubric](https://github.com/openreadout/openreadout/blob/main/docs/assurance.md#the-confidence-rubric), not set by hand. The [evidence page](../project/evidence.md) lists it per format with the files behind it.

### What to do with it

Check `assurance.level` before you rely on a number.

- On `validated`, the values follow paths confirmed on independent files.
- On `partially_validated`, they are probably right. Read `reasons` and say what is uncertain.
- On `unvalidated`, the outputs in `strict_refuses` may be wrong. Treat them as unverified, or compare them with another reader.

## Strict mode

`--strict` refuses to return values that this file's assurance does not validate. Turn it on with the global flag `--strict`, the environment variable `OPENREADOUT_STRICT=1`, or the MCP argument `strict: true`. The environment variable covers everything the process does, including `batch`, `index` and the MCP server.

```bash
openreadout stats lerc_uint16.tif --strict
```

```text
error: strict: unsupported feature: returning pixels from a tiff file outside the validated variants (codec lerc: never seen in a development-corpus file, so how it is decoded is unvalidated)
hint: --strict (MCP strict: true) refuses pixels that no independent reader has confirmed for this kind of file. Rerun without --strict to read them anyway and treat them as unverified (`info --json` → assurance says why), or check them against another reader.
```

The exit code is 6, the code for an unsupported feature (see [Exit codes](commands/index.md#exit-codes)). With `--json`, the error has `code: "unsupported_feature"` and the same message and hint.

Each request needs one scope, and is refused when that scope is in `strict_refuses`:

| request | scope |
| --- | --- |
| `info`, including `--view full` and `--view explain` | `metadata` |
| planes and regions: `planes`, `stats`, `preview`, image `export` | `pixels` |
| `spectra`, and chromatograms from spectra | `spectra` |
| `trace`, and analyses on traces | `traces` |
| `table`, and analyses on tables | `tables` |

Strict mode doesn't refuse these, because they report on the file rather than on its measured values: `check`, `info --view structure`, `info --view format`, provenance and attachments. `check` still prints the assurance block.

Strict mode refuses only `unvalidated` outputs. A `partially_validated` file is read, because its values are decoded along validated paths.

### Withheld fields

A single value can be wrong even when every output is decoded along validated paths: a read mode guessed from a label, or a measurement time taken from the wrong field. `--strict` handles these one value at a time. `info` returns everything else, replaces each field in `strict_withholds` with null, and says so in `notes`. Asking for a withheld field directly with `--only` exits 6 with the reason, instead of printing null.

A field is withheld when its value was assumed, when it was derived by a rule that no independent reader has confirmed, or when no independent reader has compared that field for this format.

## Getting a variant validated

If a file you need is refused or not `validated`, you can help. `openreadout report FILE` writes a diagnostic bundle to a local file: the fingerprint, each decoding step with its error, and the file's structure. It holds no pixel, spectral or trace values, and no free text or paths unless you add `--include-text`. `--dry-run` prints it without writing anything. OpenReadout doesn't send it anywhere. Attach the bundle to an issue, ideally with an export of the same file from the vendor's software. See [Contributing](../project/contributing.md).

For maintainers, [docs/assurance.md](https://github.com/openreadout/openreadout/blob/main/docs/assurance.md) explains how the validated sets are derived from the test corpus.
