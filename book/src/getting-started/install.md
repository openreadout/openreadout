# Install

OpenReadout is one statically linked program, `openreadout`, with no runtime dependencies. Every channel on this page installs the same binary, built from the same source.

OpenReadout has not been released yet. The install script, Homebrew, crates.io, npm, Docker and Python channels start working with the first release. Until then, build from source with cargo (see [cargo](#cargo)) or Nix.

## Install script (macOS, Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh
```

The script downloads the release archive for your machine and puts the binary in `~/.local/bin`. Two environment variables change that:

- `OPENREADOUT_VERSION=v0.1.0` installs a specific version instead of the latest.
- `OPENREADOUT_INSTALL_DIR=/usr/local/bin` installs into another directory.

On Windows, the PowerShell script installs into `%LOCALAPPDATA%\Programs\openreadout` and adds it to your `PATH`:

```powershell
irm https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.ps1 | iex
```

## Homebrew (macOS, Linux)

```bash
brew install openreadout/tap/openreadout
```

The formula installs the prebuilt binary. Each release also attaches the formula itself, so you can install from it before the tap exists:

```bash
curl -fsSLO https://github.com/openreadout/openreadout/releases/latest/download/openreadout.rb
brew install --formula ./openreadout.rb
```

## cargo

```bash
cargo install openreadout --locked       # build from source; needs Rust 1.91 or newer
cargo binstall openreadout               # download the release binary instead of compiling
```

Before the crates are on crates.io, build straight from the repository:

```bash
cargo install --locked --git https://github.com/openreadout/openreadout openreadout
```

`--no-default-features` builds a smaller binary without the MCP server and without Parquet and Arrow export.

## npm

```bash
npx openreadout info run42.czi
npm install -g openreadout
```

The npm package has no dependencies. When it is installed, it downloads the release archive for your platform, checks it against the release's `SHA256SUMS`, and extracts the binary. Three environment variables control this:

- `OPENREADOUT_BINARY` uses a binary you already have.
- `OPENREADOUT_DOWNLOAD_BASE` downloads from a mirror that holds the release assets and `SHA256SUMS`.
- `OPENREADOUT_SKIP_DOWNLOAD` waits until the first run to download.

## Docker

```bash
docker run --rm -v "$PWD:/data" ghcr.io/openreadout/openreadout info /data/run42.czi
```

The image holds the static binary and the license files, and nothing else (no shell). The entrypoint is `openreadout` and the working directory is `/data`, so paths relative to the mounted directory work. Run as yourself so that exported files belong to you:

```bash
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/data" ghcr.io/openreadout/openreadout \
    export slide3.lif -o slide3.ome.tiff
```

Images are published for `linux/amd64` and `linux/arm64` with the tags `X.Y.Z`, `X.Y` and `latest`. To build the image yourself, run `docker build -t openreadout .` in a checkout. To use the container as an MCP server, see [AI agents](../guides/agents.md).

## Python

```bash
pip install openreadout
```

This installs the Python package, with wheels for Linux, macOS and Windows. It reads the same files from Python, but it does not put the `openreadout` command on your `PATH`. See [Python](../guides/python.md).

## Nix

```bash
nix run github:openreadout/openreadout -- info run42.czi
nix profile install github:openreadout/openreadout
```

The flake builds from source, so it works on any commit. `nix develop` opens a shell with the Rust tools used to work on OpenReadout.

## Other channels

- **Scoop (Windows):** `scoop install https://github.com/openreadout/openreadout/releases/latest/download/openreadout.json`.
- **winget (Windows):** the package `OpenReadout.OpenReadout` has not been submitted yet. The manifests are in [`packaging/winget`](https://github.com/openreadout/openreadout/tree/main/packaging/winget).
- **R:** the R package needs a Rust toolchain. See [R](../guides/r.md).
- **Nextflow, Galaxy and Snakemake:** these use the binary on your `PATH`. See [Pipelines](../guides/pipelines.md).
- **Release archives:** download `openreadout-<target>.tar.gz` (or `.zip` on Windows) from the [releases page](https://github.com/openreadout/openreadout/releases) and put the binary on your `PATH`.
- **From source:** clone the repository and run `cargo build --release -p openreadout`. The binary is `target/release/openreadout`.

Release archives are built for these targets:

- `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` (static, so they run on any Linux distribution)
- `aarch64-apple-darwin` and `x86_64-apple-darwin`
- `x86_64-pc-windows-msvc`

Each archive holds the binary, the README, the licenses and notices, man pages and shell completions.

## Verify a download

Each release has a `SHA256SUMS` file covering every asset, a CycloneDX SBOM per target (`openreadout-<target>.cdx.json`), and GitHub build-provenance attestations:

```bash
sha256sum -c SHA256SUMS --ignore-missing          # on macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
gh attestation verify openreadout-aarch64-apple-darwin.tar.gz --repo openreadout/openreadout
```

The macOS binaries are not notarized and the Windows binaries are not signed. If Gatekeeper blocks a binary you downloaded by hand, run `xattr -d com.apple.quarantine openreadout`. The install script, Homebrew, npm and cargo do not set the quarantine flag.

## Check the installation

```bash
openreadout --version
openreadout self formats     # supported formats and what each does not handle yet
openreadout self doctor      # self-test on built-in synthetic files
```

Next, try [your first file](first-file.md). To use OpenReadout from an AI agent, see [AI agents](../guides/agents.md).

## Uninstall

Remove the binary, or use the tool you installed it with:

- install script: delete `~/.local/bin/openreadout` (Windows: `%LOCALAPPDATA%\Programs\openreadout`)
- `brew uninstall openreadout`
- `cargo uninstall openreadout`
- `npm uninstall -g openreadout`
- `scoop uninstall openreadout`
- `nix profile remove openreadout`

If you ran `openreadout self skill --install`, also delete the `openreadout` folder it created under `~/.claude/skills` or `~/.agents/skills`.
