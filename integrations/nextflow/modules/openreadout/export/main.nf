process OPENREADOUT_EXPORT {
    tag "$meta.id"
    label 'process_low'

    // Placeholder tags: the biocontainers image appears once the bioconda recipe
    // (integrations/bioconda) is merged. Until then set `process.container` to any image with a
    // shell and `openreadout` on PATH (book/src/guides/pipelines.md), or use conda/PATH.
    conda "${moduleDir}/environment.yml"
    container "${ workflow.containerEngine == 'singularity' && !task.ext.singularity_pull_docker_container ?
        'https://depot.galaxyproject.org/singularity/openreadout:0.1.0--0' :
        'biocontainers/openreadout:0.1.0--0' }"

    input:
    tuple val(meta), path(input)
    val format   // 'ome-tiff', 'ome-zarr', 'mzml', 'csv' or 'parquet'

    output:
    tuple val(meta), path("*.ome.tiff")   , optional: true, emit: ome_tiff
    tuple val(meta), path("*.ome.zarr")   , optional: true, emit: ome_zarr
    tuple val(meta), path("*.mzML")       , optional: true, emit: mzml
    tuple val(meta), path("*.csv")        , optional: true, emit: csv
    tuple val(meta), path("*.parquet")    , optional: true, emit: parquet
    tuple val(meta), path("*.export.json"), emit: report
    path "versions.yml"                   , emit: versions

    when:
    task.ext.when == null || task.ext.when

    script:
    def args = task.ext.args ?: ''
    def prefix = task.ext.prefix ?: "${meta.id}"
    def extensions = [ 'ome-tiff': 'ome.tiff', 'ome-zarr': 'ome.zarr', 'mzml': 'mzML', 'csv': 'csv', 'parquet': 'parquet' ]
    def ext = extensions[format]
    if (!ext) {
        error "OPENREADOUT_EXPORT: format must be one of ${extensions.keySet().join(', ')}, not '${format}'"
    }
    // The output is written under a temporary name, read back and verified, then renamed; the
    // report (`verified`, sizes, planes) is kept next to it.
    """
    openreadout \\
        export \\
        $args \\
        --format $format \\
        -o ${prefix}.${ext} \\
        --json \\
        $input \\
        > ${prefix}.export.json

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        openreadout: \$(openreadout --version | sed 's/^openreadout //')
    END_VERSIONS
    """

    stub:
    def prefix = task.ext.prefix ?: "${meta.id}"
    def ext = [ 'ome-tiff': 'ome.tiff', 'ome-zarr': 'ome.zarr', 'mzml': 'mzML', 'csv': 'csv', 'parquet': 'parquet' ][format] ?: 'out'
    def make = format == 'ome-zarr' ? "mkdir ${prefix}.${ext}" : "touch ${prefix}.${ext}"
    """
    $make
    echo '{"ok": true, "data": {}}' > ${prefix}.export.json

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        openreadout: \$(openreadout --version | sed 's/^openreadout //')
    END_VERSIONS
    """
}
