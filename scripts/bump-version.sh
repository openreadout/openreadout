#!/usr/bin/env bash
# Set the release version everywhere it is written, then check that they all agree.
#
#   scripts/bump-version.sh 0.2.0            # edit files, move [Unreleased] under "## [0.2.0] - <today>"
#   scripts/bump-version.sh 0.2.0 --no-changelog
#
# Files: Cargo.toml (workspace version + internal dependency requirements), Cargo.lock,
# server.json, mcpb/manifest.json, the agent plugin manifests in packaging/agent-plugins/
# (.claude-plugin/{plugin,marketplace}.json, .codex-plugin/plugin.json, gemini-extension.json),
# python/bioio-openreadout/pyproject.toml and python/napari-openreadout/pyproject.toml
# (version + openreadout requirement),
# packaging/npm/package.json (version + platform package requirements), packaging/wasm/package.json, CITATION.cff, THIRD-PARTY-NOTICES.md (our own crates' versions), CHANGELOG.md.
# Also r/openreadout/DESCRIPTION and the integrations/ version pins (Galaxy, Nextflow, Snakemake, bioconda).
# The Python extension (pyproject.toml) and the Nix flake read the version from Cargo.toml.
# Afterwards `cargo xtask version-check` must pass; see docs/release-process.md.
set -euo pipefail

usage() { echo "usage: $0 NEW_VERSION [--no-changelog]" >&2; exit 2; }
[ $# -ge 1 ] || usage
NEW="${1#v}"
CHANGELOG=1
[ "${2:-}" = "--no-changelog" ] && CHANGELOG=0
[ $# -le 2 ] || usage

if ! [[ "$NEW" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '$NEW' is not a semantic version (MAJOR.MINOR.PATCH[-PRE])" >&2
  exit 2
fi
MAJOR="${BASH_REMATCH[1]}"; MINOR="${BASH_REMATCH[2]}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

OLD="$(perl -ne 'if (/^\[workspace\.package\]/) { $in = 1; next } $in = 0 if /^\[/; if ($in && /^version = "([^"]+)"/) { print $1; exit }' Cargo.toml)"
[ -n "$OLD" ] || { echo "error: no [workspace.package] version in Cargo.toml" >&2; exit 1; }
if [ "$OLD" = "$NEW" ]; then
  echo "version is already $NEW"
else
  echo "bumping $OLD -> $NEW"
fi
export OLD NEW
# The bioio and napari plugins require the matching openreadout release series: >=NEW,<next breaking.
if [ "$MAJOR" = 0 ]; then NEXT="0.$((MINOR + 1))"; else NEXT="$((MAJOR + 1))"; fi
export NEXT

# perl -pi works the same on macOS and Linux (sed -i does not). \Q...\E quotes the dots.
# Cargo.toml: the [workspace.package] version and every `openreadout-* = { path, version }`.
perl -pi -e '
  $in = 1 if /^\[workspace\.package\]/; $in = 0 if /^\[/ && !/^\[workspace\.package\]/;
  s/^version = "\Q$ENV{OLD}\E"/version = "$ENV{NEW}"/ if $in;
  s/^(openreadout-[a-z0-9-]+ = \{.*version = ")\Q$ENV{OLD}\E(")/${1}$ENV{NEW}$2/;
' Cargo.toml

# JSON manifests: every "version": "OLD" and release download URLs.
AP=packaging/agent-plugins
for f in server.json mcpb/manifest.json $AP/.claude-plugin/plugin.json $AP/.claude-plugin/marketplace.json $AP/.codex-plugin/plugin.json $AP/gemini-extension.json packaging/npm/package.json packaging/wasm/package.json packaging/wasm/package-lock.json; do
  perl -pi -e 's/("version":\s*")\Q$ENV{OLD}\E(")/${1}$ENV{NEW}$2/g; s{/releases/download/v\Q$ENV{OLD}\E/}{/releases/download/v$ENV{NEW}/}g' "$f"
done
# The npm package requires its platform packages (@openreadout/cli-<os>-<cpu>) at the same version.
perl -pi -e 's/("\@openreadout\/cli-[a-z0-9-]+":\s*")\Q$ENV{OLD}\E(")/${1}$ENV{NEW}$2/g' packaging/npm/package.json

# bioio and napari plugins: their own version and the openreadout requirement.
perl -pi -e '
  s/^version = "\Q$ENV{OLD}\E"/version = "$ENV{NEW}"/;
  s/"openreadout(\[[^\]]*\])?>=[^",]+,<[^"]+"/"openreadout$1>=$ENV{NEW},<$ENV{NEXT}"/;
' python/bioio-openreadout/pyproject.toml python/napari-openreadout/pyproject.toml

# CITATION.cff: the version and release date. Zenodo (.zenodo.json) takes both from the GitHub release.
TODAY="$(date -u +%Y-%m-%d)" perl -pi -e 's/^version: "\Q$ENV{OLD}\E"/version: "$ENV{NEW}"/; s/^date-released: .*/date-released: "$ENV{TODAY}"/' CITATION.cff

# THIRD-PARTY-NOTICES.md lists our own crates with their version.
perl -pi -e 's/\[(openreadout(?:-[a-z0-9-]+)?) \Q$ENV{OLD}\E\]/[$1 $ENV{NEW}]/g' THIRD-PARTY-NOTICES.md

# R package, and the version the workflow integrations pin (bioconda recipe, conda environments,
# container tags, Galaxy macros; book/src/guides/r.md, book/src/guides/pipelines.md).
perl -pi -e 's/^Version: \Q$ENV{OLD}\E$/Version: $ENV{NEW}/' r/openreadout/DESCRIPTION
perl -pi -e 's/("\@TOOL_VERSION\@">)\Q$ENV{OLD}\E</${1}$ENV{NEW}</' integrations/galaxy/macros.xml
perl -pi -e 's/(set version = ")\Q$ENV{OLD}\E"/${1}$ENV{NEW}"/' integrations/bioconda/meta.yaml
for f in integrations/nextflow/modules/openreadout/*/environment.yml integrations/nextflow/modules/openreadout/*/main.nf integrations/snakemake/wrappers/openreadout/*/environment.yaml; do
  perl -pi -e 's/(openreadout(?:=| =|:))\Q$ENV{OLD}\E/${1}$ENV{NEW}/g' "$f"
done
# The nf-test snapshots record the MD5 of each module's versions.yml, which holds the version.
for d in integrations/nextflow/modules/openreadout/*/; do
  m=$(basename "$d"); snap="$d/tests/main.nf.test.snap"
  [ -f "$snap" ] || continue
  MOD=$(echo "$m" | tr '[:lower:]' '[:upper:]') perl -MDigest::MD5=md5_hex -pi -e \
    'BEGIN { $o = md5_hex(qq("OPENREADOUT_$ENV{MOD}":\n    openreadout: $ENV{OLD}\n)); $n = md5_hex(qq("OPENREADOUT_$ENV{MOD}":\n    openreadout: $ENV{NEW}\n)) } s/$o/$n/g' "$snap"
done

# Cargo.lock: refresh only the workspace members' entries.
cargo update --workspace --quiet 2>/dev/null || cargo update --workspace --offline --quiet

if [ "$CHANGELOG" = 1 ]; then
  if grep -q "^## \[$NEW\]" CHANGELOG.md; then
    echo "CHANGELOG.md already has a [$NEW] heading; left as is"
  else
    DATE="$(date -u +%Y-%m-%d)" perl -pi -e 's/^## \[Unreleased\]\s*$/## [Unreleased]\n\n## [$ENV{NEW}] - $ENV{DATE}\n/ and $done++ if !$done' CHANGELOG.md
    echo "CHANGELOG.md: [Unreleased] entries are now under [$NEW] - review the notes"
  fi
fi

cargo xtask version-check
echo
echo "next: review 'git diff', commit (git commit -s -m \"chore: release v$NEW\"), then tag: git tag -s v$NEW && git push origin v$NEW"
