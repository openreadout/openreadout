# Pipelines: Nextflow, Galaxy, Snakemake

The repository has ready-made steps that run the `openreadout` program in three workflow systems:

- **Nextflow:** modules `OPENREADOUT_INFO`, `OPENREADOUT_EXPORT` and `OPENREADOUT_BATCH`, following nf-core conventions.
- **Galaxy:** tools for metadata, export and batch tables.
- **Snakemake:** wrappers `openreadout/export` and `openreadout/batch`.

They are in [`integrations/`](https://github.com/openreadout/openreadout/tree/main/integrations), with shared test files in `integrations/test-data/`. For R or Python code, use the [R](r.md) or [Python](python.md) package instead.

## Getting the program into a job

Every step needs `openreadout` on the job's `PATH`, or a conda environment or container that provides it.

- **conda:** each step declares `bioconda::openreadout`. The bioconda recipe is drafted in [`integrations/bioconda`](https://github.com/openreadout/openreadout/tree/main/integrations/bioconda) but not yet submitted. Until it is, [install the program](../getting-started/install.md) and run without conda.
- **containers:** the nf-core modules point at the `biocontainers/openreadout` image that bioconda will build from the recipe. The project's own image, `ghcr.io/openreadout/openreadout`, contains only the program and no shell, so workflow engines cannot run scripts in it. Build a small image with a shell from [`integrations/containers/Dockerfile`](https://github.com/openreadout/openreadout/blob/main/integrations/containers/Dockerfile) and point the steps at it.

## Nextflow

The modules use nf-core's layout: `tuple val(meta), path(...)` inputs, `task.ext.args` for extra options, a `versions.yml` per task, a `stub:` block, `meta.yml`, `environment.yml` and nf-test tests. Copy `integrations/nextflow/modules/openreadout/` into your pipeline's `modules/local/`.

```groovy
include { OPENREADOUT_INFO   } from './modules/local/openreadout/info/main'
include { OPENREADOUT_EXPORT } from './modules/local/openreadout/export/main'
include { OPENREADOUT_BATCH  } from './modules/local/openreadout/batch/main'

workflow {
    images = Channel.fromPath(params.images).map { f -> [ [ id: f.baseName ], f ] }
    OPENREADOUT_INFO(images)                               // *.info.json
    OPENREADOUT_EXPORT(images, 'ome-tiff')                 // *.ome.tiff and an *.export.json report

    fcs = Channel.fromPath(params.fcs).collect().map { fs -> [ [ id: 'run1' ], fs ] }
    OPENREADOUT_BATCH(fcs, file(params.sample_sheet), 'table')   // run1.table.csv
}
```

```groovy
// conf/modules.config
process {
    withName: 'OPENREADOUT_EXPORT' { ext.args = '--compression lzw --pyramid mean' }
    withName: 'OPENREADOUT_BATCH' {
        ext.args  = '--where parameter=FITC-A'                                         // options of the measure
        ext.args2 = '--by condition --value median --test welch --control control'     // also writes *.summary.csv
    }
}
```

The modules:

- `OPENREADOUT_INFO` takes `[meta, file]` and emits `json` (`*.info.json`, the `info --json` output) and `versions`.
- `OPENREADOUT_EXPORT` takes `[meta, file]` and a format (`ome-tiff`, `ome-zarr`, `mzml`, `csv` or `parquet`). It emits the file it produced on the channel of the same name (`ome_tiff`, `ome_zarr`, `mzml`, `csv` or `parquet`), `report` (`*.export.json`) and `versions`.
- `OPENREADOUT_BATCH` takes `[meta, [files]]`, a sample sheet (or `[]`) and a measure (`stats`, `trace`, `table`, `gate` or `info`). It emits `table` (`*.table.csv`), `summary` (`*.summary.csv`, when `ext.args2` is set) and `versions`.

To run the tests, put `openreadout` on your `PATH` and run `nf-test test` in `integrations/nextflow`.

## Galaxy

There are three tools:

- **OpenReadout metadata** runs `info`, `info --view full` or `check`, and outputs JSON.
- **OpenReadout export** writes OME-TIFF, OME-Zarr (as a zip archive), mzML, CSV or Parquet, with the export report as a second dataset.
- **OpenReadout batch table** turns many datasets into one CSV table, joined to a sample sheet (CSV, tabular or XLSX), with an optional group summary and a Welch or Mann-Whitney test.

Datasets are linked under their original names, so a sample sheet can refer to them by file name. Data sets stored as directories, such as Bruker `.d` or Agilent `.D`, are not supported in Galaxy.

To try the tools in a local Galaxy:

```bash
pip install planemo
cd integrations/galaxy
planemo lint --fail_level warn .
planemo test --no_dependency_resolution .        # needs openreadout on PATH
planemo serve .
```

To install them on a Galaxy server before they are in the Tool Shed, copy `integrations/galaxy/` into the server's `tools/` directory and add the three XML files to the tool configuration. They use the datatypes `json`, `ome.tiff`, `zip`, `mzml`, `csv` and `parquet`, all present in Galaxy 23.0 and newer.

## Snakemake

The wrappers use the snakemake-wrappers layout. Until they are in the snakemake-wrappers repository, point `wrapper:` at a checkout:

```python
OR = "file:///path/to/openreadout/integrations/snakemake/wrappers/openreadout"

rule to_ome_tiff:
    input: "raw/{sample}.czi"
    output: output="ome/{sample}.ome.tiff", report="ome/{sample}.export.json"
    params: extra="--compression lzw"
    log: "logs/{sample}.export.log"
    wrapper: f"{OR}/export"

rule fcs_table:
    input: files=expand("fcs/{tube}.fcs", tube=TUBES), sample_sheet="samples.csv"
    output: table="results/fcs.table.csv", summary="results/fcs.summary.csv"
    params: measure="table", extra="--where parameter=FITC-A",
            summarize="--by condition --value median --test welch --control control"
    log: "logs/fcs_table.log"
    wrapper: f"{OR}/batch"
```

The export format comes from `params.format`, or else from the output's extension (`.ome.tiff`, `.ome.zarr`, `.mzML`, `.csv`, `.parquet`, `.arrow`, `.nwb`, `.jdx`, `.rdml` or `.asm.json`). For an OME-Zarr output, declare it as `directory(...)`.

Each wrapper has a test workflow in its `test/` directory. Run it with `snakemake --cores 1`; its last rule checks the contents of the outputs.

The options passed through `ext.args`, `params.extra` and `summarize` are those of [`openreadout batch`](../reference/commands/batch.md) and [`openreadout export`](../reference/commands/export.md).
