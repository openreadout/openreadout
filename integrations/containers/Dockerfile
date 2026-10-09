# syntax=docker/dockerfile:1
#
# OpenReadout for workflow engines (Nextflow, Snakemake, Galaxy, CWL): the static binary of the
# official image on a small base with a shell, bash and ps, which the engines need to run their
# task scripts inside the container. (The official ghcr.io/openreadout/openreadout image is
# FROM scratch: the binary only.) Use the biocontainers image instead once the bioconda recipe
# (integrations/bioconda) is merged.
#
#   docker build -t openreadout-pipeline:0.2.0 --build-arg VERSION=0.2.0 integrations/containers
#   nextflow run ... -with-docker openreadout-pipeline:0.2.0
#   (or process.container = 'openreadout-pipeline:0.2.0' in nextflow.config)

ARG VERSION=latest
FROM ghcr.io/openreadout/openreadout:${VERSION} AS bin

FROM debian:bookworm-slim
ARG VERSION
LABEL org.opencontainers.image.title="OpenReadout (pipelines)" \
      org.opencontainers.image.description="OpenReadout with a shell, for workflow engines" \
      org.opencontainers.image.source="https://github.com/openreadout/openreadout" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.version="${VERSION}"
# procps: `ps`, which Nextflow uses for task metrics. Versions follow the pinned Debian release.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends procps \
    && rm -rf /var/lib/apt/lists/*
COPY --from=bin /openreadout /usr/local/bin/openreadout
COPY --from=bin /licenses/ /usr/share/doc/openreadout/
RUN openreadout --version
