#!/usr/bin/env bash
# triage_corruption.sh: run `openreadout check` on every file under a directory and sort
# the results by exit code, e.g. after copying a day's acquisitions off a microscope PC.
#
# Usage:   examples/shell/triage_corruption.sh DIR
# Needs:   openreadout (or set OPENREADOUT=/path/to/openreadout), jq, find.
# Output:  one line per recognized file: OK, CORRUPT (with the error findings),
#          UNSUPPORTED (known format, feature not decoded yet, with the hint), IO or ERROR;
#          then a summary. Non-instrument files (exit 3) are counted, not listed.
# Exit:    0 if nothing is corrupt or failed to read, 1 otherwise, so it can gate a pipeline.
#
# Exit codes are the contract (https://openreadout.github.io/openreadout/reference/commands/index.html#exit-codes): 0 intact, 3 not an instrument file,
# 4 corrupt or truncated, 5 I/O, 6 unsupported feature. Branch on them, not on messages.
set -euo pipefail

BIN="${OPENREADOUT:-openreadout}"
dir="${1:?usage: triage_corruption.sh DIR}"; dir="${dir%/}"
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 2; }

ok=0 corrupt=0 unsupported=0 io=0 other=0 skipped=0
while IFS= read -r -d '' file; do
  rc=0
  json=$("$BIN" check "$file" --json 2>/dev/null) || rc=$?
  case "$rc" in
    0) ok=$((ok + 1)); echo "OK           $file" ;;
    3) skipped=$((skipped + 1)) ;;
    4)
      corrupt=$((corrupt + 1)); echo "CORRUPT      $file"
      # check reports findings in data; a file too broken to open reports an error envelope.
      jq -r 'if .ok then (.data.findings[] | select(.severity == "error") | "               \(.code): \(.message)")
             else "               \(.error.message)" end' <<<"$json" ;;
    6)
      unsupported=$((unsupported + 1)); echo "UNSUPPORTED  $file"
      jq -r '"               \(.error.message)" + (if .error.hint then "\n               hint: \(.error.hint)" else "" end)' <<<"$json" ;;
    5) io=$((io + 1)); echo "IO           $file: $(jq -r .error.message <<<"$json")" ;;
    *) other=$((other + 1)); echo "ERROR($rc)    $file" ;;
  esac
done < <(find "$dir/" -type f -print0 | sort -z)   # the trailing / also follows a symlinked DIR

echo "---"
echo "ok=$ok corrupt=$corrupt unsupported=$unsupported io=$io error=$other not-instrument=$skipped"
[ $((corrupt + io + other)) -eq 0 ]
