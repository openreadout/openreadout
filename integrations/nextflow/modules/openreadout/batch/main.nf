process OPENREADOUT_BATCH {
    tag "$meta.id"
    label 'process_medium'

    // Placeholder tags: the biocontainers image appears once the bioconda recipe
    // (integrations/bioconda) is merged. Until then set `process.container` to any image with a
    // shell and `openreadout` on PATH (book/src/guides/pipelines.md), or use conda/PATH.
    conda "${moduleDir}/environment.yml"
    container "${ workflow.containerEngine == 'singularity' && !task.ext.singularity_pull_docker_container ?
        'https://depot.galaxyproject.org/singularity/openreadout:0.2.0--0' :
        'biocontainers/openreadout:0.2.0--0' }"

    input:
    tuple val(meta), path(inputs, stageAs: 'inputs/*')
    path sample_sheet   // CSV/TSV/XLSX sample sheet or plate layout, or [] for none
    val measure         // 'stats', 'trace', 'table', 'gate' (runs `analyze gate`) or 'info'

    output:
    tuple val(meta), path("*.table.csv")  , emit: table
    tuple val(meta), path("*.summary.csv"), optional: true, emit: summary
    path "versions.yml"                   , emit: versions

    when:
    task.ext.when == null || task.ext.when

    script:
    // args: options of the measure (e.g. '--where parameter=FITC-A', '--per image');
    // args2: `openreadout summarize` options; when set, the table is also summarized by group
    // (e.g. '--by condition --value median --test welch --control control').
    def args = task.ext.args ?: ''
    def args2 = task.ext.args2 ?: ''
    def prefix = task.ext.prefix ?: "${meta.id}"
    def measures = ['stats', 'trace', 'table', 'gate', 'info']
    if (!(measure in measures)) {
        error "OPENREADOUT_BATCH: measure must be one of ${measures.join(', ')}, not '${measure}'"
    }
    def command = measure == 'gate' ? 'analyze gate' : measure
    def sheet = sample_sheet ? "--sample-sheet ${sample_sheet}" : ''
    def summarize = args2 ? "openreadout summarize ${prefix}.table.csv ${args2} -o ${prefix}.summary.csv" : ''
    """
    openreadout \\
        $command \\
        inputs/ \\
        --tidy \\
        $sheet \\
        $args \\
        -o ${prefix}.table.csv

    $summarize

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        openreadout: \$(openreadout --version | sed 's/^openreadout //')
    END_VERSIONS
    """

    stub:
    def prefix = task.ext.prefix ?: "${meta.id}"
    def args2 = task.ext.args2 ?: ''
    """
    echo 'path,format,error' > ${prefix}.table.csv
    ${args2 ? "touch ${prefix}.summary.csv" : ''}

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        openreadout: \$(openreadout --version | sed 's/^openreadout //')
    END_VERSIONS
    """
}
