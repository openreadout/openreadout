#!/bin/bash
set -euxo pipefail

# Build with conda's Rust compiler. The repository's rust-toolchain.toml asks rustup for a
# toolchain, which isn't available here, so remove it.
rm -f rust-toolchain.toml

cargo-bundle-licenses --format yaml --output THIRDPARTY.yml
cargo install --no-track --locked --verbose --root "${PREFIX}" --path crates/openreadout-cli
