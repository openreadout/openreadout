# syntax=docker/dockerfile:1
#
# OpenReadout container image: one statically linked (musl) binary on an empty base.
#
#   docker build -t openreadout .
#   docker run --rm openreadout self formats --json
#   docker run --rm -v "$PWD:/data" openreadout info /data/run42.czi
#   docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/data" openreadout export /data/a.czi -o /data/a.ome.tiff
#   docker run --rm -i -v "$PWD:/data:ro" openreadout mcp          # MCP server on stdio
#
# Multi-platform (linux/amd64, linux/arm64): the build stage always runs on the build machine's
# architecture and cross-compiles for the target, so no emulation is needed.

ARG RUST_VERSION=1

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-bookworm AS build
ARG TARGETARCH
ARG BUILDARCH
WORKDIR /src
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# Pick the musl target and, when cross-compiling, a linker for it. No C code is compiled (every
# codec is pure Rust), so a GNU cross gcc is enough to link the self-contained musl binary.
# Package versions are not pinned: they follow the pinned Debian release of the base image.
# hadolint ignore=DL3008
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) triple="x86_64-unknown-linux-musl";  gcc="x86_64-linux-gnu-gcc";  pkg="gcc-x86-64-linux-gnu" ;; \
      arm64) triple="aarch64-unknown-linux-musl"; gcc="aarch64-linux-gnu-gcc"; pkg="gcc-aarch64-linux-gnu" ;; \
      *) echo "unsupported TARGETARCH $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    if [ "$TARGETARCH" = "$BUILDARCH" ]; then pkg=""; gcc="cc"; fi; \
    apt-get install -y --no-install-recommends musl-tools $pkg; \
    rm -rf /var/lib/apt/lists/*; \
    rustup target add "$triple"; \
    echo "$triple" > /target-triple; \
    echo "$gcc" > /target-linker

# Only what the build needs (see .dockerignore). rust-toolchain.toml is deliberately not copied:
# the image's toolchain is used as is.
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
COPY xtask ./xtask
COPY skills ./skills

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    set -eux; \
    triple="$(cat /target-triple)"; \
    linker_var="CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER"; \
    export "$linker_var=$(cat /target-linker)"; \
    cargo build --release --locked -p openreadout --target "$triple"; \
    mkdir -p /out; \
    cp "target/$triple/release/openreadout" /out/openreadout

FROM scratch
ARG VERSION=0.1.0
ARG REVISION=unknown
LABEL org.opencontainers.image.title="OpenReadout" \
      org.opencontainers.image.description="Read raw lab-instrument files (Zeiss CZI, Nikon ND2, Leica LIF microscopy; FCS flow cytometry) without vendor software: JSON metadata, integrity check, OME-TIFF/OME-Zarr export, MCP server." \
      org.opencontainers.image.url="https://github.com/openreadout/openreadout" \
      org.opencontainers.image.source="https://github.com/openreadout/openreadout" \
      org.opencontainers.image.documentation="https://openreadout.github.io/openreadout/getting-started/install.html#docker" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.authors="The OpenReadout Authors" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      io.modelcontextprotocol.server.name="io.github.openreadout/openreadout"
COPY --from=build /out/openreadout /openreadout
COPY LICENSE-MIT LICENSE-APACHE NOTICE THIRD-PARTY-NOTICES.md /licenses/
WORKDIR /data
ENTRYPOINT ["/openreadout"]
CMD ["--help"]
