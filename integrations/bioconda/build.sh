#!/bin/bash
# Draft bioconda build script (recipes/openreadout/build.sh). Builds the CLI from source with
# conda's Rust compiler; rust-toolchain.toml would ask rustup for a pinned toolchain, so it is
# removed (the crates need rustc >= 1.91).
set -euxo pipefail

rm -f rust-toolchain.toml
cargo-bundle-licenses --format yaml --output THIRDPARTY.yml
cargo install --no-track --locked --verbose --root "${PREFIX}" --path crates/openreadout-cli
