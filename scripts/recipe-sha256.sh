#!/usr/bin/env bash
# Set the sha256 lines of the bioconda and conda-forge recipes to the published archives of a
# release. Run it after the release is on GitHub and PyPI:
#
#   scripts/recipe-sha256.sh            # the version in Cargo.toml
#   scripts/recipe-sha256.sh 0.2.0
#
# It downloads the GitHub tag archive (bioconda builds from it) and the three PyPI sdists
# (conda-forge builds from them), and fails if any of them is missing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-$(perl -ne 'if (/^\[workspace\.package\]/) { $in = 1; next } $in = 0 if /^\[/; if ($in && /^version = "([^"]+)"/) { print $1; exit }' Cargo.toml)}"
VERSION="${VERSION#v}"

if command -v sha256sum >/dev/null; then SHA256=(sha256sum); else SHA256=(shasum -a 256); fi
sha() {
  curl -fsSL --retry 3 "$1" | "${SHA256[@]}" | cut -d' ' -f1
}

set_sha() { # file, url
  local sum
  sum="$(sha "$2")"
  grep -q "version.*\"$VERSION\"" "$1" || { echo "error: $1 is not at version $VERSION" >&2; exit 1; }
  SUM="$sum" perl -pi -e 's/^(\s*sha256:\s*)[0-9a-f]{64}$/$1$ENV{SUM}/' "$1"
  echo "$1: $sum"
}

set_sha integrations/bioconda/meta.yaml \
  "https://github.com/openreadout/openreadout/archive/refs/tags/v$VERSION.tar.gz"
PYPI=https://pypi.org/packages/source
set_sha integrations/conda-forge/python-openreadout/recipe.yaml \
  "$PYPI/o/openreadout/openreadout-$VERSION.tar.gz"
set_sha integrations/conda-forge/bioio-openreadout/recipe.yaml \
  "$PYPI/b/bioio-openreadout/bioio_openreadout-$VERSION.tar.gz"
set_sha integrations/conda-forge/napari-openreadout/recipe.yaml \
  "$PYPI/n/napari-openreadout/napari_openreadout-$VERSION.tar.gz"
