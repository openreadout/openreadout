#!/usr/bin/env bash
# Publish every publishable workspace crate to crates.io, dependencies first
# (`cargo xtask publish-order`). Needs CARGO_REGISTRY_TOKEN. Re-runnable: versions that are
# already on crates.io are skipped.
#
# crates.io rate-limits crate creation (a burst of 5, then one new crate per 10 minutes; new
# versions of existing crates: a burst of 30, then one per minute) and answers 429 with
# "Please try again after <date>". On a 429 this script waits until then (or WAIT_FALLBACK_S) and
# retries the same crate, until MAX_WAIT_S of waiting has been spent in total. Then it stops with
# a failure so the job can be re-run (the first release publishes more new crates than fit in
# one 6-hour job).
set -euo pipefail

MAX_WAIT_S=${MAX_WAIT_S:-19200}      # 5 h 20 min of waiting at most, below the 6 h job limit
WAIT_FALLBACK_S=${WAIT_FALLBACK_S:-600}
SETTLE_S=${SETTLE_S:-30}
UA="openreadout-release (github.com/openreadout/openreadout)"

VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="openreadout"))')
CRATES=$(cargo xtask publish-order)
waited=0

# Seconds to wait after a 429, from the "try again after <HTTP date>" text on stdin; 0 if absent.
retry_after() {
  local when
  when=$(sed -n 's/.*[Tt]ry again after \(.* GMT\).*/\1/p' | head -n 1)
  [ -n "$when" ] || { echo 0; return; }
  python3 - "$when" <<'PY'
import sys, time
from email.utils import parsedate_to_datetime
try:
    print(max(0, int(parsedate_to_datetime(sys.argv[1]).timestamp() - time.time())))
except Exception:
    print(0)
PY
}

for crate in $CRATES; do
  code=$(curl -s -o /dev/null -w '%{http_code}' -A "$UA" "https://crates.io/api/v1/crates/$crate/$VERSION" || true)
  if [ "$code" = 200 ]; then
    echo "$crate $VERSION is already on crates.io; skipping"
    continue
  fi
  while true; do
    log=$(mktemp)
    if cargo publish --locked -p "$crate" 2>&1 | tee "$log"; then
      rm -f "$log"
      break
    fi
    if ! grep -q -E '429|Too Many Requests|published too many' "$log"; then
      echo "::error::cargo publish failed for $crate (not a rate limit)"
      rm -f "$log"
      exit 1
    fi
    wait_s=$(retry_after < "$log")
    rm -f "$log"
    # A little over the stated time, and never a busy loop.
    wait_s=$(( wait_s > 0 ? wait_s + 15 : WAIT_FALLBACK_S ))
    if [ $(( waited + wait_s )) -gt "$MAX_WAIT_S" ]; then
      echo "::error::crates.io rate limit: waited $waited s already and $crate needs $wait_s s more; stopping before the job time limit. Re-run this job to continue (published crates are skipped)."
      exit 1
    fi
    echo "crates.io rate limit hit on $crate; waiting $wait_s s (total waited $waited s) and retrying"
    sleep "$wait_s"
    waited=$(( waited + wait_s ))
  done
  # cargo waits for the index to list the new version; give the sparse index CDN a moment more.
  sleep "$SETTLE_S"
done
