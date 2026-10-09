# Bioconda pull request

To submit, fork bioconda/bioconda-recipes, copy `meta.yaml` and `build.sh` from this directory to
`recipes/openreadout/`, and open a pull request with the title and body below.

After the merge, the bioconda autobump bot opens update pull requests upstream for new releases.
`scripts/bump-version.sh` keeps the version here in step, and `scripts/recipe-sha256.sh` sets the
`sha256` once the release is published. Only the first submission uses this copy.

## Title

Add openreadout 0.2.0

## Body

Adds [OpenReadout](https://github.com/openreadout/openreadout), a command-line tool that reads
raw files from life-science instruments (microscopes, flow cytometers, mass spectrometers,
chromatography systems, plate readers, qPCR machines, electrophysiology rigs and NMR
spectrometers) without vendor software. It prints their metadata as JSON, exports them to open
formats (OME-TIFF, OME-Zarr, mzML, Parquet) and checks them for damage.

- Built from the v0.2.0 GitHub release tarball with conda's Rust compiler. The repository's
  `rust-toolchain.toml` is removed in `build.sh` so cargo doesn't ask for rustup. The crates need
  Rust 1.91 or newer.
- License is `MIT OR Apache-2.0`. Both license texts, `NOTICE` and the licenses of the bundled
  crates (`THIRDPARTY.yml`, from `cargo-bundle-licenses`) are packaged.
- `run_exports` pins `x.x` because the project is at 0.x, where minor releases may break the CLI.
- Builds for linux-aarch64 and osx-arm64 as well (`additional-platforms`). The project already
  ships release binaries for both.
- Tests run `--version`, `--help`, `self formats --json` (the list of supported formats) and
  `self doctor --json`, which reads built-in synthetic files through the readers and needs no
  test data.

I maintain the upstream project.

## Local verification (2026-10-06, macOS 26 on Apple silicon)

These steps aren't part of the pull request. They record what was checked before submitting.
They ran on 0.1.0. For 0.2.0 (2026-10-08) the `sha256` was recomputed from
`https://github.com/openreadout/openreadout/archive/refs/tags/v0.2.0.tar.gz` and the lint was
run again. The build wasn't repeated.

- The `sha256` matched the 0.1.0 archive, downloaded twice (once by hand, once by conda-build,
  which checks it).
- `bioconda-utils lint recipes config.yml --packages openreadout` with bioconda-utils 5.0.0 passes
  ("All checks OK"). bioconda-utils has no osx-arm64 build, so it ran from an osx-64 environment
  under Rosetta (`CONDA_SUBDIR=osx-64`), in a copy of the bioconda-recipes layout with its
  `config.yml`.
- conda-build 26.9.1 built the recipe natively for osx-arm64 with conda-forge's pinning and
  bioconda's `bioconda_utils-conda_build_config.yaml` (Rust 1.98.1 from conda-forge, clang 21,
  macOS 11.0 target). The build took 14 minutes with 2 cargo jobs. All four test commands passed,
  and `self doctor --json` reported `"ok": true`. The binary links only `libSystem`.
- For the local build only, `build.sh` also exported `CARGO_BUILD_JOBS=2` and `HOME`, because
  this machine has a cargo wrapper in `~/.cargo/config.toml` that cargo finds from the build
  directory. The submitted `build.sh` doesn't have those lines.
- Not tested locally: the linux-64 and linux-aarch64 builds, which bioconda's CI runs.
