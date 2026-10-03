# FAQ

## Why not use Bio-Formats?

Bio-Formats is the reference for microscopy formats. It reads far more of them than OpenReadout, and it is what Fiji, QuPath and OMERO use. OpenReadout uses it to decide when two other readers disagree. If Bio-Formats works for you, keep using it.

OpenReadout is meant for cases where Bio-Formats fits less well:

- **No runtime.** It is one static binary with no JVM. It installs in one step on a CI runner, in a minimal container, on a locked-down lab PC or in an agent's sandbox.
- **Output for programs.** Every command can print JSON with a published schema, exit codes have fixed meanings, and errors carry a hint. The same operations are available as an MCP server.
- **Fast metadata.** `info` reads headers only, so it answers in milliseconds even on multi-gigabyte files (see [Performance](performance.md)).
- **Permissive license.** OpenReadout is MIT OR Apache-2.0, so it can be embedded in closed-source tools. The Bio-Formats format readers are GPL.
- **Beyond microscopy.** The same commands cover mass spectrometry, flow cytometry, chromatography, electrophysiology, NMR, spectroscopy, plate readers, qPCR and more ([Formats](../formats/index.md)).

See also the [comparison with other tools](comparison.md).

## Is it validated?

We compare each reader with independent readers on a corpus of public files and write down the reason for each disagreement ([Validation](validation.md)). CI runs the smallest tier of that corpus on every push.

This is not validation in the regulatory sense (IQ/OQ/PQ under GxP). See [regulated use](#can-i-use-it-in-a-regulated-gxp-21-cfr-part-11-lab) below.

## Can it write vendor formats?

No. The raw file is the primary record, so OpenReadout doesn't write to it. It writes open formats, such as OME-TIFF, OME-Zarr, mzML and Parquet, to new files and reads each one back to verify it. The [roadmap](roadmap.md) mentions writers for experiment set-up files, such as acquisition worklists, not for raw data.

## You run GPL readers. Does that make OpenReadout GPL?

No. GPL and LGPL readers (Bio-Formats, libCZI and pylibCZIrw, bioio-czi, bioio-lif, readlif) run only in the separate `oracle/` Python environment, as black boxes that produce reference values. They aren't linked into, shipped with or imported by OpenReadout or its Python packages, and we don't read their source while writing a parser ([clean-room policy](clean-room.md), rule 3). All we keep from them is plane hashes and geometry.

## How were the formats worked out, and is that legal?

From files we are allowed to use, by reading hex dumps and comparing files that differ in one setting, and from the published documentation of permissively licensed readers. We don't use vendor SDKs, headers, DLLs or non-public specifications. Each format page links to a provenance log that records how we got there. Reverse engineering file formats for interoperability is lawful in the US and protected in the EU. The [clean-room policy](clean-room.md) lists the rules the project follows. This is not legal advice.

## How large a file can it handle?

`info`, `info --view structure` and `check --headers-only` read headers and directories only, so their time and memory do not grow with the pixel data. `export` and `check --planes` work one plane at a time, so peak memory is a few planes, not the file. For large exports, select what you need:

```bash
openreadout export big.czi --to ome-zarr --image 0 --select c=0 --select t=0-9 -o subset.ome.zarr
```

OME-Zarr output is chunked and can include a pyramid. In Python, `File.to_dask()` decodes a plane only when a computation needs it. Measured numbers are on the [performance](performance.md) page.

## Can I use it in a regulated (GxP, 21 CFR Part 11) lab?

Not for submission data without your own qualification. The project provides some pieces of one:

- release binaries built from tagged commits, with build attestations;
- the corpus comparison, which you can rerun on your own machine;
- `verified: true` in each export result, and the tool name and version recorded in each exported file;
- no in-place writes, no network access and no telemetry.

You still need your own qualification, support arrangements and change control.

## Does it connect to the network?

No. The binary contains no network code (`cargo deny` checks this in CI), no telemetry and no update check. If you see it open a network connection, report it as a security bug (see [`SECURITY.md`](https://github.com/openreadout/openreadout/blob/main/SECURITY.md)). The [privacy policy](https://github.com/openreadout/openreadout/blob/main/PRIVACY.md) says what OpenReadout does with your data.

## My file does not open. What now?

1. Run `openreadout info --view format FILE`. Exit code 3 means the format is not supported. Exit code 6 means the format is known but the file uses a feature that is not decoded yet; the `hint` says which.
2. `openreadout self formats` lists the known gaps of every reader.
3. Run `openreadout check --report FILE`. It writes a diagnostic bundle that you can review before sharing. Attach it to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml) with the acquisition software and its version. If the file can be shared under an open license, it can join the test corpus, which is the fastest way to get it fixed.

## Why the focus on AI agents?

Agents are good at deciding what to do with a file but bad at parsing vendor formats, so OpenReadout does the parsing for them. Stable JSON, exit codes and a single binary also make it easy to use in shell scripts and pipelines.
