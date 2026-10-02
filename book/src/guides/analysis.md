# Choosing an analysis

Reading a file tells you what is in it. The `analyze` commands answer the next question. They use the same readers, take `--json` like every other command, and never change the input file.

| question | command | guide |
| --- | --- | --- |
| When does m/z X elute? What does the TIC look like? | `analyze chromatogram` | [Chromatograms and peaks](quantitation.md) |
| What are the peak areas, heights and purity? | `analyze peaks` | [Chromatograms and peaks](quantitation.md) |
| What is the area of this IR, Raman or UV-Vis band? | `analyze peaks --x-range A:B` | [Spectral bands](quantitation.md#spectral-bands-and-regions) |
| Which MS/MS scans are in this run? | `spectra` | [spectra](../reference/commands/spectra.md) |
| What are the concentrations, IC50s or Z′ of this plate? | `analyze assay` | [Plate-reader assays](plate-analysis.md) |
| What are the Cq values and fold changes? | `analyze qpcr` | [Real-time PCR](../formats/qpcr.md) |
| Where are the NMR peaks and what are their integrals? | `analyze nmr-peaks` | [NMR processing](nmr.md) |
| What are the action potentials, rheobase and input resistance? | `analyze ephys-features` | [Electrophysiology](ephys.md) |
| How many extracellular spikes per channel? | `analyze spikes` | [Electrophysiology](ephys.md) |
| How many events fall in each gate of a FlowJo workspace? | `analyze gate` | [FlowJo workspaces](../formats/flowjo-wsp.md) |
| What is the mean intensity per well of a screening plate? | `stats --per well` | [High-content screening](../formats/hcs.md) |
| What is the answer for every file in a folder, by condition? | `batch` | [Batch tables](batch.md) |

Every flag of every subcommand is in the [`analyze` reference](../reference/commands/analyze.md). The JSON output of each is described under [JSON output](../reference/json/index.md).
