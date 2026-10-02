#!/usr/bin/env bash
# Run fuzz targets one after another for a fixed time each, collecting every crash instead of
# stopping at the first (libFuzzer fork mode with a single worker). Seeds come from
# fuzz/corpus/<target> (committed) and fuzz/local-seeds/<target> (gitignored, written by
# seeds.py from share-alike corpus files); new coverage goes to fuzz/work/<target> and crashes
# to fuzz/artifacts/<target>/.
#
#   fuzz/run.sh [SECONDS_PER_TARGET] [target ...]
#
# One target at a time, one worker, 2 GB RSS cap: fuzzing next to builds on a 16 GB machine
# has exhausted memory before. Needs a nightly toolchain and cargo-fuzz
# (`rustup toolchain install nightly`, `cargo install cargo-fuzz`). CI's `fuzz-smoke` job runs
# the equivalent of `fuzz/run.sh 15 $(python3 affected.py origin/main)` on pull requests and
# `fuzz/run.sh 60` weekly (Linux only).
set -euo pipefail
cd "$(dirname "$0")"
SECS="${1:-60}"
shift $(( $# > 0 ? 1 : 0 ))
cargo +nightly fuzz build -O -a >/dev/null
if [ $# -eq 0 ]; then
  # shellcheck disable=SC2046
  set -- $(cargo +nightly fuzz list)
fi
BIN_DIR="target/$(rustc +nightly -vV | sed -n 's/^host: //p')/release"
mkdir -p work artifacts logs
for t in "$@"; do
  mkdir -p "work/$t" "artifacts/$t" "corpus/$t"
  seeds=("corpus/$t")
  [ -d "local-seeds/$t" ] && seeds+=("local-seeds/$t")
  "$BIN_DIR/$t" "work/$t" "${seeds[@]}" \
    -fork=1 -ignore_crashes=1 -ignore_ooms=1 -ignore_timeouts=1 \
    -max_total_time="$SECS" -rss_limit_mb=2048 -malloc_limit_mb=2048 -timeout=20 \
    -artifact_prefix="artifacts/$t/" >"logs/$t.log" 2>&1 || true
  n=$(find "artifacts/$t" -type f | wc -l | tr -d ' ')
  echo "$t: $(grep -Eo 'cov: [0-9]+' "logs/$t.log" | tail -1) artifacts: $n"
done
