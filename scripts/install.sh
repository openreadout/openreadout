#!/bin/sh
# OpenReadout installer: downloads the release binary for this machine into ~/.local/bin (or $OPENREADOUT_INSTALL_DIR).
# Usage: curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh
# Test hook: OPENREADOUT_INSTALL_FROM=path/to/openreadout-<target>.tar.gz installs that local
# release archive instead of downloading one (used for release dry runs; see docs/release-process.md).
set -eu
REPO="openreadout/openreadout"
VERSION="${OPENREADOUT_VERSION:-latest}"
DIR="${OPENREADOUT_INSTALL_DIR:-$HOME/.local/bin}"
FROM="${OPENREADOUT_INSTALL_FROM:-}"
os=$(uname -s); arch=$(uname -m)
case "$os" in
  Darwin) case "$arch" in arm64) t=aarch64-apple-darwin ;; x86_64) t=x86_64-apple-darwin ;; *) echo "unsupported arch $arch"; exit 1 ;; esac ;;
  Linux)  case "$arch" in aarch64|arm64) t=aarch64-unknown-linux-musl ;; x86_64) t=x86_64-unknown-linux-musl ;; *) echo "unsupported arch $arch"; exit 1 ;; esac ;;
  *) echo "unsupported OS $os (use install.ps1 on Windows)"; exit 1 ;;
esac
if [ "$VERSION" = latest ]; then url="https://github.com/$REPO/releases/latest/download/openreadout-$t.tar.gz"; else url="https://github.com/$REPO/releases/download/$VERSION/openreadout-$t.tar.gz"; fi
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
if [ -n "$FROM" ]; then
  echo "installing from local archive $FROM"
  cp "$FROM" "$tmp/pkg.tar.gz"
else
  echo "downloading $url"
  curl -fsSL "$url" -o "$tmp/pkg.tar.gz"
fi
tar -xzf "$tmp/pkg.tar.gz" -C "$tmp"
mkdir -p "$DIR"
install -m 755 "$tmp/openreadout" "$DIR/openreadout"
echo "installed $DIR/openreadout ($("$DIR/openreadout" --version))"
case ":$PATH:" in *":$DIR:"*) ;; *) echo "add $DIR to your PATH, e.g.: export PATH=\"$DIR:\$PATH\"" ;; esac
echo "agent skill: openreadout self skill --install all    |  MCP: openreadout mcp --config claude"
