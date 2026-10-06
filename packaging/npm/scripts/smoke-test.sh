#!/usr/bin/env bash
# Publish packed npm tarballs to a throwaway local registry and install them the way users do.
#
#   scripts/smoke-test.sh TARBALL_DIR [npm|pnpm|all]
#
# TARBALL_DIR holds openreadout-<version>.tgz and the platform package tarballs from `npm pack`
# (openreadout-cli-<os>-<cpu>-<version>.tgz). The script starts Verdaccio on 127.0.0.1 with no
# uplink, so nothing comes from or goes to npmjs.com, publishes the tarballs to it, then checks:
#
# - `npx -y openreadout@<version>` in an empty directory (the MCP client configuration);
# - npm and pnpm (through corepack) installs into a project: exactly one platform package, the
#   one matching this machine, `openreadout --version` and an MCP initialize handshake;
# - the error message when optional dependencies are left out.
#
# Needs Node.js 18+ and network access to fetch Verdaccio and pnpm. KEEP=1 keeps the scratch
# directory.
set -euo pipefail

dir="$(cd "${1:?usage: smoke-test.sh TARBALL_DIR [npm|pnpm|all]}" && pwd)"
which="${2:-all}"
here="$(cd "$(dirname "$0")/.." && pwd)"
version="$(node -p "require('$here/package.json').version")"
[ -f "$dir/openreadout-$version.tgz" ] || { echo "error: $dir/openreadout-$version.tgz not found" >&2; exit 1; }
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0

work="$(mktemp -d)"
registry_pid=
cleanup() {
  if [ -n "$registry_pid" ]; then
    kill "$registry_pid" 2>/dev/null || true
    wait "$registry_pid" 2>/dev/null || true
  fi
  if [ -n "${KEEP:-}" ]; then echo "scratch directory kept: $work"; else rm -rf "$work"; fi
}
trap cleanup EXIT

# A local registry with no uplink: publishing needs a user, reading does not.
port="$(node -e "const s=require('net').createServer().listen(0,'127.0.0.1',()=>{console.log(s.address().port);s.close()})")"
registry="http://127.0.0.1:$port/"
cat > "$work/verdaccio.yaml" <<EOF
storage: $work/storage
max_body_size: 200mb  # a platform package is about 20 MB
auth:
  htpasswd:
    file: $work/htpasswd
packages:
  '**':
    access: \$all
    publish: \$authenticated
log: { type: stdout, format: pretty, level: warn }
EOF
npx -y verdaccio@6.10.5 --config "$work/verdaccio.yaml" --listen "127.0.0.1:$port" > "$work/verdaccio.log" 2>&1 &
registry_pid=$!
for _ in $(seq 1 120); do
  curl -fsS "${registry}-/ping" >/dev/null 2>&1 && break
  sleep 1
done
curl -fsS "${registry}-/ping" >/dev/null || { cat "$work/verdaccio.log" >&2; echo "error: the local registry did not start" >&2; exit 1; }
token="$(curl -fsS -X PUT -H 'content-type: application/json' \
  -d '{"name":"smoke","password":"smoke-test-only"}' "${registry}-/user/org.couchdb.user:smoke" |
  node -pe 'JSON.parse(require("fs").readFileSync(0, "utf8")).token')"

# Every npm and pnpm command below reads this file instead of the user's ~/.npmrc.
npmrc="$work/npmrc"
printf 'registry=%s\n//127.0.0.1:%s/:_authToken=%s\n' "$registry" "$port" "$token" > "$npmrc"
export npm_config_userconfig="$npmrc" npm_config_registry="$registry" npm_config_cache="$work/npm-cache"
export npm_config_audit=false npm_config_fund=false npm_config_update_notifier=false

for f in "$dir"/openreadout-cli-*-"$version".tgz "$dir/openreadout-$version.tgz"; do
  npm publish "$f" --provenance=false --loglevel=error
done
echo "published to $registry:"
npm view openreadout@"$version" optionalDependencies

check() { # $1 = label, $2 = project directory, the rest runs openreadout
  local label="$1" proj="$2"; shift 2
  local out reply
  out="$(cd "$proj" && "$@" --version)"
  echo "$label: $out"
  [ "$out" = "openreadout $version" ] || { echo "error: expected 'openreadout $version'" >&2; exit 1; }
  reply="$(cd "$proj" && printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' | "$@" mcp | head -n 1)"
  case "$reply" in *'"serverInfo"'*) echo "$label: MCP initialize ok" ;; *) echo "error: no MCP initialize reply: $reply" >&2; exit 1 ;; esac
}

one_platform_package() { # $1 = node_modules/@openreadout directory
  local n
  n="$(ls "$1" | wc -l | tr -d ' ')"
  echo "installed platform packages: $(ls "$1" | tr '\n' ' ')"
  [ "$n" = 1 ] || { echo "error: expected exactly one platform package" >&2; exit 1; }
}

if [ "$which" = npm ] || [ "$which" = all ]; then
  mkdir -p "$work/npx"
  check "npx -y" "$work/npx" npx -y "openreadout@$version"

  p="$work/npm"; mkdir -p "$p"
  (cd "$p" && npm init -y >/dev/null && npm install --loglevel=error "openreadout@$version")
  one_platform_package "$p/node_modules/@openreadout"
  check npm "$p" npx --no-install openreadout

  m="$work/npm-omit-optional"; mkdir -p "$m"
  (cd "$m" && npm init -y >/dev/null && npm install --loglevel=error --omit=optional "openreadout@$version")
  if err="$(cd "$m" && npx --no-install openreadout --version 2>&1)"; then
    echo "error: expected a failure without the platform package" >&2; exit 1
  fi
  case "$err" in
    *"npm install @openreadout/cli-"*"@$version"*) echo "npm --omit=optional: fails with a message naming the platform package" ;;
    *) echo "error: unexpected message: $err" >&2; exit 1 ;;
  esac
fi

if [ "$which" = pnpm ] || [ "$which" = all ]; then
  p="$work/pnpm"; mkdir -p "$p"
  echo '{"name":"pnpm-smoke","version":"0.0.0","private":true}' > "$p/package.json"
  (cd "$p" && corepack pnpm@10 add --reporter=append-only "openreadout@$version" >/dev/null)
  one_platform_package "$p/node_modules/.pnpm/node_modules/@openreadout"
  check pnpm "$p" corepack pnpm@10 exec openreadout
fi

echo "smoke test passed"
