# Install

OpenReadout is one program, `openreadout`, with no runtime dependencies. All the options on this page give you the same binary, built from the same source.

## Install script

On macOS and Linux this puts the binary in `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh
```

The script checks the download against the release's `SHA256SUMS`. If `~/.local/bin` is not on your `PATH` yet (it isn't by default on macOS), the script prints the line to add to your shell's startup file. Until then, run the program as `~/.local/bin/openreadout`.

On Windows (PowerShell) this installs into `%LOCALAPPDATA%\Programs\openreadout` and adds it to your `PATH`:

```powershell
irm https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.ps1 | iex
```

Both scripts install the latest release. Set `OPENREADOUT_VERSION=v0.1.0` for a specific version, or `OPENREADOUT_INSTALL_DIR` for another directory (in PowerShell, `$env:OPENREADOUT_VERSION = "v0.1.0"` before the command).

## Other ways

- **Homebrew:** `brew install openreadout/tap/openreadout`. Each release also attaches the formula, `openreadout.rb`, which you can install with `brew install --formula ./openreadout.rb`.
- **Scoop (Windows):** `scoop bucket add openreadout https://github.com/openreadout/scoop-bucket`, then `scoop install openreadout/openreadout`. `scoop update` picks up new releases.
- **npm:** `npx openreadout info run42.czi` or `npm install -g openreadout`. The binary comes in a platform package such as `@openreadout/cli-linux-x64`, which npm, pnpm, Yarn and Bun install for your machine as an optional dependency. Nothing is downloaded from GitHub and no install script runs, so it works behind proxies and from registry mirrors. If optional dependencies are turned off (`--omit=optional`), `openreadout` names the package to add. `OPENREADOUT_BINARY` points it at a binary you already have.
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

### If macOS blocks the program

Only a binary downloaded with a web browser is checked by Gatekeeper. The install script, Homebrew, npm, pip and cargo don't mark their downloads, so their binaries are not affected.

The v0.1.0 binaries are not notarized. If you downloaded an archive in a browser and macOS says it can't verify that `openreadout` is free of malware, remove the mark and run it again:

```bash
xattr -d com.apple.quarantine openreadout
```

Later releases are signed with a Developer ID and notarized by Apple. macOS checks the notarization online the first time you run a downloaded copy, so that first run needs an internet connection. Offline, the same `xattr` command lets it run. `codesign -dv openreadout` shows who signed a binary.

The Windows binaries of v0.1.0 are not signed either. A command-line program started from a terminal usually runs without a SmartScreen prompt.

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
  mcp server: available (32 tools)
  threads: 4
  formats (97): czi, lif, nd2, thermo-raw, fcs, imzml, mzml, mzxml, mzmlb, bruker-tdf, ...
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

Delete the binary, or use the tool you installed it with: `cargo uninstall openreadout`, `brew uninstall openreadout`, `npm uninstall -g openreadout`, `scoop uninstall openreadout` or `nix profile remove openreadout`. The install script puts the binary in `~/.local/bin/openreadout`. On Windows it uses the folder `%LOCALAPPDATA%\Programs\openreadout`; delete it and remove it from your user `PATH` (*Settings > System > About > Advanced system settings > Environment Variables*). If you ran `openreadout self skill --install`, also delete the `openreadout` folder it created under `~/.claude/skills` or `~/.agents/skills`.

## Next: your first file

[Your first file](first-file.md) shows what is in a file, checks it, converts it and draws a picture of it.
