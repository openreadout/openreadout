process OPENREADOUT_INFO {
    tag "$meta.id"
    label 'process_single'

    // Placeholder tags: the biocontainers image appears once the bioconda recipe
    // (integrations/bioconda) is merged. Until then set `process.container` to any image with a
    // shell and `openreadout` on PATH (book/src/guides/pipelines.md), or use conda/PATH.
    conda "${moduleDir}/environment.yml"
    container "${ workflow.containerEngine == 'singularity' && !task.ext.singularity_pull_docker_container ?
        'https://depot.galaxyproject.org/singularity/openreadout:0.1.0--0' :
        'biocontainers/openreadout:0.1.0--0' }"

    input:
    tuple val(meta), path(input)

    output:
    tuple val(meta), path("*.info.json"), emit: json
    path "versions.yml"                 , emit: versions

    when:
    task.ext.when == null || task.ext.when

    script:
    def args = task.ext.args ?: ''
    def prefix = task.ext.prefix ?: "${meta.id}"
    """
    openreadout \\
        info \\
        $args \\
        --json \\
        $input \\
        > ${prefix}.info.json

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        openreadout: \$(openreadout --version | sed 's/^openreadout //')
    END_VERSIONS
    """

    stub:
    def prefix = task.ext.prefix ?: "${meta.id}"
    """
    echo '{"ok": true, "data": {}}' > ${prefix}.info.json

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        openreadout: \$(openreadout --version | sed 's/^openreadout //')
    END_VERSIONS
    """
}
