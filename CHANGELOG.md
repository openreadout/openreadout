# Changelog

All notable changes to this project are documented here. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- An interactive viewer in the chat for clients with MCP Apps (Claude Desktop, claude.ai, ChatGPT, the Codex app, VS Code): image planes, sweeps, NMR spectra, chromatograms with their spectra, plates and flow-cytometry plots, and a file viewer for vendor files opened in the ChatGPT and Codex desktop apps.
- A Codex plugin and marketplace (`codex plugin marketplace add openreadout/agent-plugins`) and a Gemini CLI extension (`gemini extensions install https://github.com/openreadout/agent-plugins`).
- The release workflow can sign and notarize the macOS binaries with a Developer ID, and sign the Windows binary with Azure Artifact Signing. `scripts/macos-sign.sh` does the macOS part and also runs on a Mac.
- `.zenodo.json`, so Zenodo can archive each release with a DOI.
- `cargo binstall openreadout` on Windows on Arm installs the x64 build.
- qPCR results exports: the Results tables of Applied Biosystems software (StepOne, 7500, QuantStudio, ViiA 7; `.xls`, `.xlsx`, text, with their amplification and melt curves) and Bio-Rad CFX `Quantification Cq Results` (`.csv`, `.xlsx`) are read as `qpcr-results-export`, so `analyze qpcr` works on them.

### Changed

- The Claude Code plugin, the Codex plugin and the Gemini CLI extension install from their own repository, [openreadout/agent-plugins](https://github.com/openreadout/agent-plugins), which the release workflow updates. Add the Claude Code marketplace again with `/plugin marketplace add openreadout/agent-plugins`.
- A privacy policy, `PRIVACY.md`, linked from the README, the docs site and the `.mcpb` manifest.
- The npm package no longer downloads the binary in a postinstall script. The binary comes in a platform package (`@openreadout/cli-darwin-arm64`, `-darwin-x64`, `-linux-arm64`, `-linux-x64`, `-win32-x64`) that the package manager installs as an optional dependency, so it works with pnpm 10, behind proxies and from registry mirrors.
- The install scripts check the download against the release's `SHA256SUMS`. `install.ps1` reads `OPENREADOUT_VERSION` like `install.sh`, works on Windows PowerShell 5.1 with older TLS defaults, and puts the program on the `PATH` of the current terminal as well. `install.sh` prints the line to add to your shell's startup file when `~/.local/bin` is not on your `PATH`.

### Fixed

- Tecan i-control exports with several reads per well returned no values, and German i-control exports were not recognised. Each well's value is now i-control's `Mean`, and a workbook with one export per sheet gives one plate read per sheet.
- The Claude Code plugin failed to load because its marketplace entry and `plugin.json` both declared the skill.
- The Homebrew formula and winget manifests attached to a release no longer start with the template's header comment.
- Docker build records (`*.dockerbuild`) no longer end up among the release assets.
- `CITATION.cff` now validates: the dual license is a list of SPDX identifiers.
- The website's home page and *Connect an assistant* no longer say that there is no release yet.

## [0.1.0] - 2026-10-02

The first public release.

### Added

- Readers for raw files from microscopes, screening systems, flow cytometers, electrophysiology rigs, NMR and optical spectrometers, mass spectrometers, chromatographs, plate readers, qPCR cyclers and other bench instruments. `openreadout self formats` lists them with their validation level.
- Sixteen commands: `info`, `check`, `preview`, `stats`, `trace`, `table`, `spectra`, `analyze`, `export`, `batch`, `link`, `index`, `search`, `watch`, `self` and `mcp`.
- JSON output on every command, with published JSON Schemas and documented exit codes.
- Verified export to OME-TIFF, OME-Zarr, mzML, CSV, Parquet, Arrow, NWB, JCAMP-DX, Allotrope ASM and RDML.
- Analyses of chromatograms, peaks, plate assays, qPCR, NMR spectra, patch-clamp recordings, extracellular spikes and flow-cytometry gates.
- An MCP server, an agent skill and a Claude Code plugin.
- A Python package with bioio and napari plugins, an R package, a WebAssembly build, and Nextflow, Galaxy and Snakemake integrations.
- A documentation website: task recipes with real output, a searchable format list taken from `self formats`, a JSON reference rendered from the published schemas, and a browser demo.
