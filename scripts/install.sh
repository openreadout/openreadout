#!/bin/sh
# OpenReadout installer: downloads the release binary for this machine into ~/.local/bin (or $OPENREADOUT_INSTALL_DIR).
# Usage: curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.sh | sh
# OPENREADOUT_VERSION=v0.1.0 installs that release instead of the latest one. The archive is
# checked against the release's SHA256SUMS before anything is installed.
# Test hook: OPENREADOUT_INSTALL_FROM=path/to/openreadout-<target>.tar.gz installs that local
# release archive instead of downloading one, without a checksum (used for release dry runs; see
# docs/release-process.md).
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
asset="openreadout-$t.tar.gz"
case "$VERSION" in latest) ;; v*) ;; *) VERSION="v$VERSION" ;; esac
if [ "$VERSION" = latest ]; then base="https://github.com/$REPO/releases/latest/download"; else base="https://github.com/$REPO/releases/download/$VERSION"; fi
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
if [ -n "$FROM" ]; then
  echo "installing from local archive $FROM"
  cp "$FROM" "$tmp/$asset"
else
  echo "downloading $base/$asset"
  curl -fsSL "$base/$asset" -o "$tmp/$asset"
  curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
  if command -v sha256sum >/dev/null 2>&1; then sum=$(sha256sum "$tmp/$asset"); else sum=$(shasum -a 256 "$tmp/$asset"); fi
  actual=${sum%% *}
  expected=$(awk -v a="$asset" '$2 == a || $2 == "*" a { print $1 }' "$tmp/SHA256SUMS")
  if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for $asset (expected ${expected:-none}, got $actual); nothing was installed" >&2
    exit 1
  fi
  echo "sha256 ok"
fi
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$DIR"
install -m 755 "$tmp/openreadout" "$DIR/openreadout"
echo "installed $DIR/openreadout ($("$DIR/openreadout" --version))"
case ":$PATH:" in
  *":$DIR:"*) ;;
  *)
    case "${SHELL:-}" in
      */zsh) rc="$HOME/.zshrc" ;;
      */bash) if [ "$os" = Darwin ]; then rc="$HOME/.bash_profile"; else rc="$HOME/.bashrc"; fi ;;
      *) rc="" ;;
    esac
    echo
    echo "$DIR is not on your PATH yet. To add it, run:"
    if [ -n "$rc" ]; then
      echo "  echo 'export PATH=\"$DIR:\$PATH\"' >> $rc && export PATH=\"$DIR:\$PATH\""
    else
      echo "  export PATH=\"$DIR:\$PATH\"    (and add the same line to your shell's startup file)"
    fi
    echo "or run it as $DIR/openreadout"
    echo
    ;;
esac
echo "agent skill: openreadout self skill --install all    |  MCP: openreadout mcp --install claude-desktop (or claude, cursor, codex, vscode, gemini)"
