# Recipes

Each recipe is one task, one command and how to read what comes back. The outputs are real, from public sample files in the repository. For the methods behind an analysis, follow the link at the end of each recipe to its guide.

## Files

- [Is this file intact?](check-files.md): `check` a file or a folder after a copy, and get exit code 4 when something is truncated or damaged.
- [Write the methods paragraph](methods-text.md): `info --view explain` turns the acquisition settings recorded in a file into sentences.
- [Convert to an open format](export.md): OME-TIFF, OME-Zarr, mzML, NWB or Parquet, read back and verified before it is kept.
- [Load planes in Python](python.md): open a file, list its images and read a plane into NumPy, or use the bioio and napari plugins.

## Analyses

- [Integrate chromatogram peaks](peaks.md): peak tables with retention times and areas from a chromatogram or an extracted-ion trace.
- [Fit a dose–response curve](plate-assay.md): IC50 or EC50 and Z′ from a plate-reader export and a plate layout.
- [Check a qPCR run](qpcr.md): Cq values, the standard curve's efficiency and R², and ΔΔCq.
- [Spike features for a folder of recordings](ephys.md): per-cell and per-sweep features for many ABF, ATF or NWB files in one table.

## Many files

- [Index and search a lab share](index-share.md): catalogue a share once, then search it, report on its health and find personal data before sharing.

The same tasks are available to an AI agent as MCP tools; [Connect an assistant](../getting-started/assistant.md) sets that up. Every command and flag is in the [command reference](../reference/commands/index.md).
