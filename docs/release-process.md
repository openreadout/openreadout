# Release process

How to cut an OpenReadout release, what the release workflow produces, and how each publishing channel is switched on. Every publish step is gated: with no repository variables set, a tag produces a GitHub release and nothing else leaves the repository.

## 1. Prepare

1. `main` is green (CI: `check`, `corpus-smoke`, `python-tests`, `cross-build`, `publish-dry-run`, `third-party-notices`, `release-check`).
2. Optional full dry run: *Actions > Release > Run workflow* on `main`. `workflow_dispatch` builds every artifact (archives, `.mcpb` bundles, wheels, sdists, SBOMs, Homebrew/Scoop/winget manifests, the Docker image) and uploads them to the run as `release-dry-run`; it creates no release and publishes nothing.
3. Bump the version everywhere it is written:
   ```bash
   scripts/bump-version.sh 0.2.0
   ```
   This edits `Cargo.toml` (workspace version and the internal dependency requirements), `Cargo.lock`, `server.json`, `mcpb/manifest.json`, `.claude-plugin/plugin.json` and `marketplace.json`, `.codex-plugin/plugin.json`, `gemini-extension.json`, `python/bioio-openreadout/pyproject.toml` and `python/napari-openreadout/pyproject.toml` (their version and their `openreadout>=0.2.0,<0.3` requirement), `r/openreadout/DESCRIPTION`, the version pins of `integrations/` (bioconda, Galaxy, Nextflow, Snakemake), `packaging/npm/package.json`, `packaging/wasm/package.json`, the crate versions in `THIRD-PARTY-NOTICES.md`, and moves the `[Unreleased]` changelog entries under `## [0.2.0] - <date>`. It finishes with `cargo xtask version-check`. The Python extension and the Nix flake read the version from `Cargo.toml`.
4. Edit `CHANGELOG.md` into release notes, then commit: `git commit -s -am "chore: release v0.2.0"` and merge to `main` through a pull request.

## 2. Tag

```bash
git tag -s v0.2.0 -m "v0.2.0"      # or -a if you do not sign tags
git push origin v0.2.0
```

The tag must equal the `Cargo.toml` version with a `v` prefix and `CHANGELOG.md` must have its heading: the `version-check` job (`cargo xtask version-check --tag v0.2.0`) fails the release otherwise.

## 3. What the Release workflow produces

`.github/workflows/release.yml`, on a `v*` tag:

| job | output |
| --- | --- |
| `version-check` | fails fast if any version string disagrees |
| `build` (5 targets) | `openreadout-<target>.tar.gz` / `.zip` (binary at the archive root + README, SKILL.md, licenses, NOTICE, THIRD-PARTY-NOTICES.md, `man/*.1` and `completions/` from `cargo xtask man`), `openreadout-mcp-<target>.mcpb` |
| `sbom` | `openreadout-<target>.cdx.json` (CycloneDX 1.5, cargo-cyclonedx, dependencies of the binary for that target) |
| `wheels`, `sdist` | Python abi3 wheels for 7 platforms, `openreadout` sdist, `bioio-openreadout` and `napari-openreadout` wheels + sdists (`twine check --strict`) |
| `homebrew` | `openreadout.rb` (Homebrew formula) and `openreadout.json` (Scoop manifest) rendered from the archives' SHA-256; winget manifests as a run artifact only |
| `release` | `SHA256SUMS` over every asset, `server.json` with the `.mcpb` hashes filled in, and the GitHub release with generated notes |
| `docker` | builds the image, runs `--version`, `self formats --json`, a volume-mount check and a label check; pushes only if enabled |
| `pypi`, `publish-crates`, `publish-npm` | publish only if enabled (below) |

Every archive, bundle, wheel, sdist and SBOM has a GitHub build-provenance attestation (`gh attestation verify <file> --repo openreadout/openreadout`).

Targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`.

## 4. Publishing gates

Repository variables live under *Settings > Secrets and variables > Actions > Variables*; secrets under *Secrets*. Each publish job also runs in a GitHub environment (`pypi`, `pypi-bioio`, `pypi-napari`, `crates-io`, `npm`), where required reviewers can be added for a manual approval step.

| channel | variable | secret / setup | job |
| --- | --- | --- | --- |
| PyPI (`openreadout`, `bioio-openreadout`, `napari-openreadout`) | `PUBLISH_PYPI=true` | none: register a trusted publisher on PyPI per project (owner `openreadout`, repository `openreadout`, workflow `release.yml`; environment `pypi` for `openreadout`, `pypi-bioio` for `bioio-openreadout`, `pypi-napari` for `napari-openreadout`: PyPI allows one pending publisher per environment) | `pypi` |
| crates.io (every publishable crate; `cargo xtask publish-order` lists them) | `PUBLISH_CRATES=true` | `CARGO_REGISTRY_TOKEN`, an environment secret of `crates-io` (crates.io API token with `publish-new` and `publish-update`) | `publish-crates` |
| npm (`openreadout`) | `PUBLISH_NPM=true` | none: trusted publishing (OIDC). Publish the first version by hand (`npm publish --access public` in `packaging/npm`), then on npmjs.com configure the trusted publisher (organization `openreadout`, repository `openreadout`, workflow `release.yml`, environment `npm`) | `publish-npm` |
| ghcr.io (`ghcr.io/openreadout/openreadout`) | `PUBLISH_DOCKER=true` | none (uses `GITHUB_TOKEN` with `packages: write`); after the first push, make the package public under the repository's *Packages* | `docker` |
| Homebrew tap | — | create `openreadout/homebrew-tap`, copy the release's `openreadout.rb` to `Formula/openreadout.rb` | manual (automatable with a PAT) |
| Scoop bucket | — | optional `openreadout/scoop-bucket` with the release's `openreadout.json` under `bucket/`; `checkver`/`autoupdate` keep it current | manual |
| winget | — | see `packaging/winget/README.md` | manual PR to microsoft/winget-pkgs |
| MCP Registry | — | `mcp-publisher publish` with the release's `server.json` (the release's copy has the real `.mcpb` hashes for all five targets; the checked-in file carries zero placeholders). Publish after the npm package exists: the registry verifies that `packaging/npm/package.json` has `mcpName` equal to the server name (`cargo xtask version-check` checks this too). An `oci` entry for the Docker image can be added once it is public; a `cargo` entry needs a visible `mcp-name: io.github.openreadout/openreadout` line in the `openreadout` crate's README | manual |

Notes per channel:

- **crates.io** publishes every crate that is not `publish = false` in dependency order: `cargo xtask publish-order` computes the list from the manifests (every internal dependency, dev- and build-dependencies included, comes before its dependents; ties alphabetical), and both the `publish-crates` job and CI's `publish-dry-run` read it, so a new crate is never left out. `scripts/publish-crates.sh` does the publishing: it waits 30 s after each crate (cargo itself also waits until the new version is in the index) and skips versions already on crates.io, so a failed job can simply be re-run.

  **Rate limit.** crates.io limits how fast new crates can be created: a burst of 5, then one every 10 minutes (new versions of existing crates: a burst of 30, then one per minute); beyond that the server answers 429 with a "try again after" time (defaults from the crates.io source, `src/rate_limiter.rs`). The first release creates every crate, so it takes roughly `(crates - 5) x 10` minutes, more than one 6-hour job. The script waits until the stated time and retries the same crate, for at most 5 h 20 min of waiting in total (`MAX_WAIT_S`), then fails with a message; **re-run the failed `publish-crates` job** (once or twice for the first release) and it continues where it stopped. Later releases only publish new versions and fit in the burst. After the first release, crates.io trusted publishing can replace the token (configure it per crate, then use `rust-lang/crates-io-auth-action`). CI's `publish-dry-run` packages and verifies every publishable crate on pull requests. `openreadout-py`, `openreadout-r`, `openreadout-wasm`, `openreadout-bench` and `openreadout-corpus-tests` are `publish = false`.
- **API stability of the crates**: the format crates document only their reader types and format-id constants; their parser modules are `#[doc(hidden)]` (public for tests and fuzz targets, not a supported API). Enums and option structs that will grow are `#[non_exhaustive]`. Sibling crates are released in lockstep with `^0.x` requirements, so a patch release must not break a hidden item another crate of the workspace uses (today: `openreadout-zarr` uses `openreadout-tiff`'s OME-XML parser, and every reader crate uses `openreadout-codecs`).
- **npm** runs after the GitHub release exists, because installing the package downloads the release binary; the job installs the packed tarball and runs `openreadout --version` before publishing. The job publishes with npm trusted publishing: `id-token: write`, npm >= 11.5.1 (installed by the job), no `NODE_AUTH_TOKEN`; `--provenance` requires the repository to be public. A package that does not exist yet cannot have a trusted publisher, which is why the first version is published by hand; `repository.url` in `packaging/npm/package.json` must match the repository exactly.
- **Docker** always builds and tests the image; `PUBLISH_DOCKER` only controls the push (tags `X.Y.Z`, `X.Y`, `latest`, `sha-…`, platforms `linux/amd64` and `linux/arm64`) and the registry attestation.
- **PyPI** is unchanged from before: trusted publishing, no stored token.

## 5. After the release

- Install from each enabled channel on macOS, Linux and Windows (`book/src/getting-started/install.md`); open an [issue](https://github.com/openreadout/openreadout/issues) for any channel that fails.
- From the second crates.io release on, check the public API against the previous one before tagging: `cargo install cargo-semver-checks --locked`, then `cargo semver-checks --workspace --exclude openreadout-py --exclude openreadout-corpus-tests --exclude xtask` (the baseline is the version on crates.io, so this cannot run before the first publish). A reported break needs a minor bump while the version is `0.x`. Hidden (`#[doc(hidden)]`) items are not checked. Once it runs clean, add it to CI's `publish-dry-run` job.
- Update the Homebrew tap / Scoop bucket if they are maintained by hand.
- If something is wrong with the assets, delete the release and the tag, fix, and tag again; crates.io and npm versions cannot be reused, so bump the patch version instead once those gates are on.

## Agent marketplaces and directories

These listings read the repository or the release. Most need setting up once, and then pick up each release by themselves. Each plugin and extension runs the `openreadout` binary from the user's `PATH`, so its listing should say to install the program first. Link [`PRIVACY.md`](../PRIVACY.md) wherever a form asks for a privacy policy.

- **Claude Code marketplace**: `.claude-plugin/marketplace.json` and `.claude-plugin/plugin.json`. Users run `/plugin marketplace add openreadout/openreadout`. CI's `release-check` job validates the plugin and installs it.
- **Anthropic's plugin directory**: submit at [claude.ai/directory/manage](https://claude.ai/directory/manage) (*Plugin bundle*), after the [pre-submission checklist](https://claude.com/docs/plugins/pre-submission-checklist). This repository is too large for it as it stands. The directory stops validating a repository over 50 MiB as GitHub archives it, and holds a plugin with over 512 files or a file over 256 KiB for review. Submit a small repository that holds only the plugin files instead (`.claude-plugin/`, `.mcp.json`, `skills/`, `assets/`, a README and the licenses), and raise its version with each release.
- **Codex**: `.agents/plugins/marketplace.json` lists the plugin in `.codex-plugin/plugin.json`. Users run `codex plugin marketplace add openreadout/openreadout`, then `codex plugin add openreadout@openreadout`. Nothing to publish.
- **Gemini CLI**: `gemini-extension.json` at the root. The extension gallery lists repositories with the `gemini-cli-extension` GitHub topic, once a day. Users run `gemini extensions install https://github.com/openreadout/openreadout`.
- **MCP Registry**: after the npm package exists (see the table above).
- **Smithery**: `smithery mcp publish openreadout-mcp-<target>.mcpb -n openreadout/openreadout`, or upload a bundle at [smithery.ai/new](https://smithery.ai/new).
- **Glama**: lists public MCP servers from GitHub. To claim the listing for the organization, add a `glama.json` at the root with `{"maintainers": ["<GitHub user>"]}`, then use *Claim* on the server page.
- **cursor.directory**: submit at [cursor.directory/plugins/new](https://cursor.directory/plugins/new) with the `openreadout mcp` command.

## Signing and notarization (deferred)

Release binaries are unsigned apart from the ad-hoc signature the macOS linker adds (enough for Apple Silicon to run them). What proper signing needs, none of which exists yet:

- **macOS**: an Apple Developer Program membership (paid, yearly) for a *Developer ID Application* certificate, then `codesign --options runtime --timestamp` on each Mach-O binary and `xcrun notarytool submit` of a zip or disk image. A bare binary cannot be stapled, so users are covered by the online notarization check; the certificate and an App Store Connect API key would live in repository secrets and the `build` job would sign on the macOS runners.
- **Windows**: an Authenticode code-signing certificate (an OV/EV certificate from a CA, or Azure Trusted Signing), applied with `signtool sign /fd sha256 /tr <timestamp URL>` in the `build` job before zipping.

Until then `book/src/getting-started/install.md` explains the Gatekeeper workaround; Homebrew, the install script, npm and cargo do not set the quarantine flag.

## Local checks

```bash
cargo xtask version-check                        # all version strings agree
cargo publish --dry-run --locked $(cargo xtask publish-order --flags)   # what publish-crates would upload
# (use a fresh CARGO_TARGET_DIR if an earlier dry run packaged the same version: cargo treats the
#  packaged crates as immutable registry crates and would reuse their stale builds)
cargo xtask homebrew-formula --sums SHA256SUMS --out /tmp/openreadout.rb   # also scoop-manifest, winget-manifest
cargo xtask mcpb pack --binary target/release/openreadout --target aarch64-apple-darwin --out /tmp
OPENREADOUT_INSTALL_FROM=openreadout-aarch64-apple-darwin.tar.gz OPENREADOUT_INSTALL_DIR=/tmp/bin sh scripts/install.sh
(cd packaging/npm && npm test && npm pack --dry-run)
maturin build --out dist && maturin sdist --out dist                        # Python wheel (profile release-py) + sdist
python -m build python/bioio-openreadout --outdir dist && python -m build python/napari-openreadout --outdir dist
twine check --strict dist/*
(cd book && npm ci && npm run build) && python3 book/check_links.py                         # docs site builds, no broken links
docker build -t openreadout . && docker run --rm openreadout self formats --json
cargo fetch --locked && cargo about generate --frozen --fail -m crates/openreadout-cli/Cargo.toml about.hbs -o THIRD-PARTY-NOTICES.md
```
