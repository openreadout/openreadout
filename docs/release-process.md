# Release process

How to cut an OpenReadout release, what the release workflow produces, and how each publishing channel is switched on. Every publish step is gated: with no repository variables set, a tag produces a GitHub release and nothing else leaves the repository.

## 1. Prepare

1. `main` is green (CI: `check`, `corpus-smoke`, `python-tests`, `cross-build`, `publish-dry-run`, `third-party-notices`, `release-check`).
2. Optional full dry run: *Actions > Release > Run workflow* on `main`. `workflow_dispatch` builds every artifact (archives, `.mcpb` bundles, wheels, sdists, SBOMs, Homebrew/Scoop/winget manifests, the Docker image) and uploads them to the run as `release-dry-run`; it creates no release and publishes nothing.
3. Bump the version everywhere it is written:
   ```bash
   scripts/bump-version.sh 0.2.0
   ```
   This edits `Cargo.toml` (workspace version and the internal dependency requirements), `Cargo.lock`, `server.json`, `mcpb/manifest.json`, the agent plugin manifests in `packaging/agent-plugins/` (`.claude-plugin/plugin.json` and `marketplace.json`, `.codex-plugin/plugin.json`, `gemini-extension.json`), `python/bioio-openreadout/pyproject.toml` and `python/napari-openreadout/pyproject.toml` (their version and their `openreadout>=0.2.0,<0.3` requirement), `r/openreadout/DESCRIPTION`, the version pins of `integrations/` (bioconda, Galaxy, Nextflow, Snakemake), `packaging/npm/package.json`, `packaging/wasm/package.json`, the crate versions in `THIRD-PARTY-NOTICES.md`, and moves the `[Unreleased]` changelog entries under `## [0.2.0] - <date>`. It finishes with `cargo xtask version-check`. The Python extension and the Nix flake read the version from `Cargo.toml`.
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
| `npm-packages` | the npm tarballs: `openreadout` and one `@openreadout/cli-<os>-<cpu>` per target, installed from a local registry with npm, pnpm and `npx -y` and run, as a run artifact |
| `sbom` | `openreadout-<target>.cdx.json` (CycloneDX 1.5, cargo-cyclonedx, dependencies of the binary for that target) |
| `wheels`, `sdist` | Python abi3 wheels for 7 platforms, `openreadout` sdist, `bioio-openreadout` and `napari-openreadout` wheels + sdists (`twine check --strict`) |
| `homebrew` | `openreadout.rb` (Homebrew formula) and `openreadout.json` (Scoop manifest) rendered from the archives' SHA-256; winget manifests as a run artifact only |
| `release` | `SHA256SUMS` over every asset, `server.json` with the `.mcpb` hashes filled in, and the GitHub release with generated notes |
| `agent-plugins-tree`, `agent-plugins` | the openreadout/agent-plugins repository's contents (`cargo xtask agent-plugins`) as a run artifact; on a tag, pushes them there as a commit `openreadout X.Y.Z` with the tag `vX.Y.Z` (below) |
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
| npm (`openreadout` and the five `@openreadout/cli-*` platform packages) | `PUBLISH_NPM=true` | none: trusted publishing (OIDC). Each package needs a trusted publisher on npmjs.com (organization `openreadout`, repository `openreadout`, workflow `release.yml`, environment `npm`), and a package must exist before it can have one, so publish the first version of each new package by hand (below) | `publish-npm` |
| ghcr.io (`ghcr.io/openreadout/openreadout`) | `PUBLISH_DOCKER=true` | none (uses `GITHUB_TOKEN` with `packages: write`); after the first push, make the package public under the repository's *Packages* | `docker` |
| Homebrew tap | — | create `openreadout/homebrew-tap`, copy the release's `openreadout.rb` to `Formula/openreadout.rb` | manual (automatable with a PAT) |
| Scoop bucket | — | optional `openreadout/scoop-bucket` with the release's `openreadout.json` under `bucket/`; `checkver`/`autoupdate` keep it current | manual |
| winget | — | see `packaging/winget/README.md` | manual PR to microsoft/winget-pkgs |
| MCP Registry (`io.github.openreadout/openreadout`) | `PUBLISH_MCP_REGISTRY=true` | none: GitHub OIDC login. The job publishes the release's `server.json`, which has the real `.mcpb` hashes (the checked-in file has zero placeholders), after npm and crates.io, because the registry checks that `packaging/npm/package.json` has `mcpName` equal to the server name and that the `openreadout` crate's README has an `mcp-name:` line. To publish by hand, `mcp-publisher login github` is not enough for an organization namespace ([registry#1468](https://github.com/modelcontextprotocol/registry/issues/1468)): an org owner logs in with `mcp-publisher login github --token "$(gh auth token)"` (needs `read:org`) | `publish-mcp-registry` |

Notes per channel:

- **crates.io** publishes every crate that is not `publish = false` in dependency order: `cargo xtask publish-order` computes the list from the manifests (every internal dependency, dev- and build-dependencies included, comes before its dependents; ties alphabetical), and both the `publish-crates` job and CI's `publish-dry-run` read it, so a new crate is never left out. `scripts/publish-crates.sh` does the publishing: it waits 30 s after each crate (cargo itself also waits until the new version is in the index) and skips versions already on crates.io, so a failed job can simply be re-run.

  **Rate limit.** crates.io limits how fast new crates can be created: a burst of 5, then one every 10 minutes (new versions of existing crates: a burst of 30, then one per minute); beyond that the server answers 429 with a "try again after" time (defaults from the crates.io source, `src/rate_limiter.rs`). The first release creates every crate, so it takes roughly `(crates - 5) x 10` minutes, more than one 6-hour job. The script waits until the stated time and retries the same crate, for at most 5 h 20 min of waiting in total (`MAX_WAIT_S`), then fails with a message; **re-run the failed `publish-crates` job** (once or twice for the first release) and it continues where it stopped. Later releases only publish new versions and fit in the burst. After the first release, crates.io trusted publishing can replace the token (configure it per crate, then use `rust-lang/crates-io-auth-action`). CI's `publish-dry-run` packages and verifies every publishable crate on pull requests. `openreadout-py`, `openreadout-r`, `openreadout-wasm`, `openreadout-bench` and `openreadout-corpus-tests` are `publish = false`.
- **API stability of the crates**: the format crates document only their reader types and format-id constants; their parser modules are `#[doc(hidden)]` (public for tests and fuzz targets, not a supported API). Enums and option structs that will grow are `#[non_exhaustive]`. Sibling crates are released in lockstep with `^0.x` requirements, so a patch release must not break a hidden item another crate of the workspace uses (today: `openreadout-zarr` uses `openreadout-tiff`'s OME-XML parser, and every reader crate uses `openreadout-codecs`).
- **npm** ships the binary in five platform packages, `@openreadout/cli-darwin-arm64`, `-darwin-x64`, `-linux-arm64`, `-linux-x64` (the static musl builds) and `-win32-x64` (which also installs on Windows on Arm). Each has `os` and `cpu` fields, and `openreadout` lists all five as optional dependencies at its own version, so the package manager installs only the one for the machine. `openreadout`'s command is a small Node.js launcher (`packaging/npm/openreadout.js`) that finds that package and runs its binary. Nothing downloads at install time and there are no install scripts, which matters because pnpm 10 skips dependencies' install scripts by default and corporate proxies and offline mirrors block downloads from GitHub. `packaging/npm/lib/platforms.js` is the list of platform packages; `scripts/platform-packages.js` builds them from the release archives.

  The `npm-packages` job builds and packs all six, then `packaging/npm/scripts/smoke-test.sh` publishes them to a throwaway [Verdaccio](https://verdaccio.org) registry on 127.0.0.1 and installs them with `npx -y openreadout@<version>`, npm and pnpm. It checks that exactly one platform package was installed, `openreadout --version`, an MCP `initialize` handshake, and the error message when optional dependencies are left out. The GitHub release waits for this job. `publish-npm` then uploads the tarballs, platform packages first, with trusted publishing (`id-token: write`, npm >= 11.5.1, no `NODE_AUTH_TOKEN`) and `--provenance`. It skips versions that are already on npm, so you can re-run it. `repository.url` in each package must match the repository exactly, which the tests check.

  **First publish of the platform packages (once).** npm only lets you add a trusted publisher to a package that exists. Publish 0.1.0 of the five platform packages by hand from the v0.1.0 release binaries. This is harmless: `openreadout` 0.1.0 does not depend on them.
  ```bash
  gh release download v0.1.0 --repo openreadout/openreadout --dir /tmp/rel \
    --pattern 'openreadout-*.tar.gz' --pattern 'openreadout-*.zip' --pattern SHA256SUMS
  cd packaging/npm   # on the commit that adds the platform packages, with version 0.1.0
  node scripts/platform-packages.js --archives /tmp/rel --sums /tmp/rel/SHA256SUMS --out /tmp/npm-platform
  npm login          # an owner of the openreadout organization on npmjs.com
  for d in /tmp/npm-platform/*/; do (cd "$d" && npm publish --access public); done
  ```
  Then, for each of the five packages on npmjs.com, open *Settings > Trusted publishing*, choose GitHub Actions and enter organization `openreadout`, repository `openreadout`, workflow `release.yml` and environment `npm`. While you are there, set *Publishing access* to require two-factor authentication and disallow tokens, as for `openreadout`.
- **Docker** always builds and tests the image; `PUBLISH_DOCKER` only controls the push (tags `X.Y.Z`, `X.Y`, `latest`, `sha-…`, platforms `linux/amd64` and `linux/arm64`) and the registry attestation.
- **PyPI** is unchanged from before: trusted publishing, no stored token.

## 5. After the release

- Install from each enabled channel on macOS, Linux and Windows (`book/src/getting-started/install.md`); open an [issue](https://github.com/openreadout/openreadout/issues) for any channel that fails.
- From the second crates.io release on, check the public API against the previous one before tagging: `cargo install cargo-semver-checks --locked`, then `cargo semver-checks --workspace --exclude openreadout-py --exclude openreadout-corpus-tests --exclude xtask` (the baseline is the version on crates.io, so this cannot run before the first publish). A reported break needs a minor bump while the version is `0.x`. Hidden (`#[doc(hidden)]`) items are not checked. Once it runs clean, add it to CI's `publish-dry-run` job.
- Update the Homebrew tap / Scoop bucket if they are maintained by hand.
- If something is wrong with the assets, delete the release and the tag, fix, and tag again; crates.io and npm versions cannot be reused, so bump the patch version instead once those gates are on.

## Agent marketplaces and directories

These listings read the repository or the release. Most need setting up once, and then pick up each release by themselves. Each plugin and extension runs the `openreadout` binary from the user's `PATH`, so its listing should say to install the program first. Link [`PRIVACY.md`](../PRIVACY.md) wherever a form asks for a privacy policy.

The Claude Code and Codex plugins and the Gemini CLI extension install from [openreadout/agent-plugins](https://github.com/openreadout/agent-plugins), not from this repository. This repository archives to over 60 MB, and a Codex or Gemini install would clone all of it. The manifests live in `packaging/agent-plugins/`, laid out as at the root of that repository. `cargo xtask agent-plugins --out DIR` copies them, adds `skills/openreadout/`, `assets/icon.svg`, `PRIVACY.md`, the licenses and `NOTICE`, and fails if a manifest's version differs from `Cargo.toml`. The tree is about 130 KB.

The release workflow's `agent-plugins` job runs on a tag, after the GitHub release. It replaces the contents of openreadout/agent-plugins with the tree, commits it as `openreadout X.Y.Z` (signed off by `github-actions[bot]`), tags it `vX.Y.Z` and pushes `main` and the tag. If nothing changed it tags the existing commit. It pushes over SSH with `AGENT_PLUGINS_DEPLOY_KEY`, a secret of the `agent-plugins` environment holding the private half of an ed25519 deploy key with write access on openreadout/agent-plugins. The job checks GitHub's host key against the keys it pins, so update them if GitHub rotates its keys. A manual run (`workflow_dispatch`) keeps the tree as the `agent-plugins` artifact and pushes nothing. Don't edit openreadout/agent-plugins by hand, because the next release overwrites it.

- **Claude Code marketplace**: `.claude-plugin/marketplace.json` and `.claude-plugin/plugin.json`. Users run `/plugin marketplace add openreadout/agent-plugins`, then `/plugin install openreadout@openreadout`. CI's `release-check` job builds the tree, validates the plugin and installs it.
- **Anthropic's plugin directory**: submit openreadout/agent-plugins at [claude.ai/directory/manage](https://claude.ai/directory/manage) (*Plugin bundle*), after the [pre-submission checklist](https://claude.com/docs/plugins/pre-submission-checklist). The directory stops validating a repository over 50 MiB as GitHub archives it, and holds a plugin with over 512 files or a file over 256 KiB for review. The xtask tests check the file count and sizes.
- **Codex**: `.agents/plugins/marketplace.json` lists the plugin in `.codex-plugin/plugin.json`, which shares the plugin's `.mcp.json` with Claude Code. Users run `codex plugin marketplace add openreadout/agent-plugins`, then `codex plugin add openreadout@openreadout`.
- **Gemini CLI**: `gemini-extension.json`. The extension gallery lists repositories with the `gemini-cli-extension` GitHub topic once a day, so openreadout/agent-plugins carries that topic. Users run `gemini extensions install https://github.com/openreadout/agent-plugins`.
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
# npm packages from release archives, installed from a local registry with npm, pnpm and npx -y:
(cd packaging/npm && node scripts/platform-packages.js --archives DIR --out /tmp/npm-platform \
  && for d in /tmp/npm-platform/*/; do npm pack "$d" --pack-destination /tmp/npm-tgz; done \
  && npm pack --pack-destination /tmp/npm-tgz && scripts/smoke-test.sh /tmp/npm-tgz)
maturin build --out dist && maturin sdist --out dist                        # Python wheel (profile release-py) + sdist
python -m build python/bioio-openreadout --outdir dist && python -m build python/napari-openreadout --outdir dist
twine check --strict dist/*
(cd book && npm ci && npm run build) && python3 book/check_links.py                         # docs site builds, no broken links
docker build -t openreadout . && docker run --rm openreadout self formats --json
cargo fetch --locked && cargo about generate --frozen --fail -m crates/openreadout-cli/Cargo.toml about.hbs -o THIRD-PARTY-NOTICES.md
```
