# OpenReadout

OpenReadout reads the raw files that lab instruments write, without the vendor's software. It is one program with no runtime to install. It tells you what a file holds, checks that the file is intact, and exports it to open formats such as OME-TIFF, OME-Zarr, mzML or Parquet. The source file is never modified.

```text
$ openreadout info aics-s-1-t-1-c-2-z-1.lif
aics-s-1-t-1-c-2-z-1.lif
  format: Leica LIF (lif) v2  size: 16.0 MiB  images: 1  planes: 2
  assurance: validated - every variant feature of this file was read correctly in development-corpus files confirmed by an independent reader (up to 21 files from 11 sources)
  [0] single_image_forJamie/PEI_laminin_35k  2048x2048 z=1 c=2 t=1  uint16 XYCZT  px=0.3250 µm
      ch0 Gray
      ch1 Green
      objective: HC PL FLUOTAR L    20x/0.40 DRY 20x NA 0.4
      acquired: 2020-03-11T19:48:24.472Z
```

It reads files from microscopes, mass spectrometers, chromatographs, cytometers, electrophysiology rigs, NMR and optical spectrometers, plate readers, qPCR cyclers and more. The [format list](formats/index.md) has every format and its known gaps.

Every command can print JSON with a published schema, so scripts and AI agents can use it as easily as people. The same operations run as an MCP server. There are also Python and R packages, plugins for bioio and napari, and a WebAssembly build that reads files in the browser.

## Where to start

- **New here:** [install it](getting-started/install.md), then [open your first file](getting-started/first-file.md).
- **Scripts and notebooks:** [reading the JSON output](getting-started/reading-json.md), then the [Python](guides/python.md) or [R](guides/r.md) guide.
- **Analysis:** [choosing an analysis](guides/analysis.md) covers peaks, plate assays, qPCR, NMR and electrophysiology. [Batch tables](guides/batch.md) combine many files.
- **AI agents:** [setting up an agent](guides/agents.md) and the [MCP tools](reference/mcp.md).
- **Every command and flag:** the [command reference](reference/commands/index.md).
- **Whether to trust it:** [validation](project/validation.md), the [FAQ](project/faq.md) and the [comparison with other tools](project/comparison.md).

## When a file fails

If OpenReadout cannot read one of your files, or reads it wrongly, run:

```bash
openreadout check --report FILE
```

This writes a diagnostic bundle that you can review before sharing. Attach it to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml).

OpenReadout is licensed MIT OR Apache-2.0 and is not affiliated with any instrument vendor ([trademarks](https://github.com/openreadout/openreadout/blob/main/TRADEMARKS.md)).
