# Install

OpenReadout is one program, `openreadout`, with no runtime dependencies. All the options on this page give you the same binary, built from the same source.

## Install script

On macOS and Linux this puts the binary in `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh
```

Set `OPENREADOUT_VERSION=v0.1.0` for a specific version, or `OPENREADOUT_INSTALL_DIR` for another directory. On Windows (PowerShell) this installs into `%LOCALAPPDATA%\Programs\openreadout` and adds it to your `PATH`:

```powershell
irm https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.ps1 | iex
```

## Other ways

- **Homebrew:** `brew install openreadout/tap/openreadout`. Each release also attaches the formula, `openreadout.rb`, which you can install with `brew install --formula ./openreadout.rb`.
- **Scoop (Windows):** `scoop install https://github.com/openreadout/openreadout/releases/latest/download/openreadout.json`.
- **npm:** `npx openreadout info run42.czi` or `npm install -g openreadout`. The package downloads the release binary for your platform and checks it against the release's `SHA256SUMS`. `OPENREADOUT_BINARY` points it at a binary you already have.
- **cargo:** `cargo binstall openreadout` downloads the release binary; `cargo install openreadout --locked` builds it (Rust 1.91 or newer, from [rustup](https://rustup.rs)) and puts it in `~/.cargo/bin`. Add `--no-default-features` for a smaller binary without the MCP server and without Parquet and Arrow export.
- **Docker:** `docker run --rm -v "$PWD:/data" ghcr.io/openreadout/openreadout info /data/run42.czi`. The image holds the binary and the license files only (no shell), for `linux/amd64` and `linux/arm64`. Add `--user "$(id -u):$(id -g)"` so exported files belong to you.
- **Python:** `pip install openreadout` reads the same files from Python, with wheels for Linux, macOS and Windows. It does not put the `openreadout` command on your `PATH`. See [Python](../guides/python.md).
- **R:** `R CMD INSTALL r/openreadout` from a checkout, which needs a Rust toolchain. See [R](../guides/r.md).
- **Nix:** `nix run github:openreadout/openreadout -- info run42.czi`, or `nix profile install github:openreadout/openreadout`.
- **Release archives:** `openreadout-<target>.tar.gz` (`.zip` on Windows) from the [releases page](https://github.com/openreadout/openreadout/releases), for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` (static, any Linux distribution), `aarch64-apple-darwin`, `x86_64-apple-darwin` and `x86_64-pc-windows-msvc`. Each archive holds the binary, the README, the licenses and notices, man pages and shell completions.
- **From source:** `cargo install --locked --git https://github.com/openreadout/openreadout openreadout` builds the latest commit.

Not available yet: winget (manifests in [`packaging/winget`](../../../packaging/winget)) and bioconda (recipe drafted in [`integrations/bioconda`](../../../integrations/bioconda), not submitted). Nextflow, Galaxy and Snakemake use the binary on your `PATH`; see [Pipelines](../guides/pipelines.md).

## Verify a download

Each release has a `SHA256SUMS` file, a CycloneDX SBOM per target (`openreadout-<target>.cdx.json`) and GitHub build-provenance attestations:

```bash
sha256sum -c SHA256SUMS --ignore-missing          # on macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
gh attestation verify openreadout-aarch64-apple-darwin.tar.gz --repo openreadout/openreadout
```

The macOS binaries are not notarized and the Windows binaries are not signed. If Gatekeeper blocks a binary you downloaded by hand, run `xattr -d com.apple.quarantine openreadout`. The install script, Homebrew, npm and cargo do not set the quarantine flag.

## Check it works

```text
$ openreadout --version
openreadout 0.1.0
```

`openreadout self doctor` reports the version, build, features and formats, then runs a self-test on built-in synthetic files (long lines are trimmed):

```text
$ openreadout self doctor
openreadout 0.1.0 (x86_64-unknown-linux-gnu, release)
  features: mcp
  mcp server: available (15 tools)
  threads: 4
  formats (96): czi, lif, nd2, thermo-raw, fcs, imzml, mzml, mzxml, mzmlb, bruker-tdf, ...
self-test:
  ok   detect tiff                  tiff (Definite)
  ok   tiff info and planes         16x8 uint16, 2 planes bit-exact
  ok   tiff check                   7 checks performed
  ...
  ok   batch table                  info over 4 files: 4 rows, no errors
all checks passed
```

If the last line is `all checks passed`, the installation works. `openreadout self formats` lists every supported format and what each does not handle yet.

## Uninstall

Delete the binary, or use the tool you installed it with: `cargo uninstall openreadout`, `brew uninstall openreadout`, `npm uninstall -g openreadout`, `scoop uninstall openreadout` or `nix profile remove openreadout`. The install script puts the binary in `~/.local/bin/openreadout` (Windows: `%LOCALAPPDATA%\Programs\openreadout`). If you ran `openreadout self skill --install`, also delete the `openreadout` folder it created under `~/.claude/skills` or `~/.agents/skills`.

## Next: your first file

[Your first file](first-file.md) shows what is in a file, checks it, converts it and draws a picture of it.
