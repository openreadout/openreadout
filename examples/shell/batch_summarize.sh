#!/usr/bin/env bash
# batch_summarize.sh: one tab-separated line per instrument file under a directory.
#
# Usage:   examples/shell/batch_summarize.sh DIR [> summary.tsv]
# Needs:   openreadout (or set OPENREADOUT=/path/to/openreadout), jq, find.
# Output:  path, format, images, planes, tables (flow cytometry), total rows, then for
#          image 0: XxY, z, c, t, pixel type, pixel size in µm; and the acquisition time. Files that are not instrument files
#          (exit code 3) are skipped. Files too damaged to open (exit 4), known formats
#          using a feature not decoded yet (exit 6) and other failures go to stderr.
# Exit:    0 unless a file failed for another reason (I/O, usage, internal error).
#          Use triage_corruption.sh to gate on damaged files.
#
# `info` reads headers only, so this stays fast on directories of multi-gigabyte files.
set -euo pipefail

BIN="${OPENREADOUT:-openreadout}"
dir="${1:?usage: batch_summarize.sh DIR}"; dir="${dir%/}"
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 2; }

printf 'path\tformat\timages\tplanes\ttables\trows\txy\tz\tc\tt\tpixel_type\tpixel_um\tacquired_at\n'

seen=0 skipped=0 corrupt=0 unsupported=0 failed=0
while IFS= read -r -d '' file; do
  rc=0
  json=$("$BIN" info "$file" --json 2>/dev/null) || rc=$?
  case "$rc" in
    0) ;;
    3) skipped=$((skipped + 1)); continue ;;   # not an instrument file
    4|6)                                         # too damaged to open / feature not decoded yet
      if [ "$rc" -eq 4 ]; then corrupt=$((corrupt + 1)); else unsupported=$((unsupported + 1)); fi
      echo "$file: $(jq -r '.error.code + ": " + .error.message' <<<"$json")" >&2
      continue ;;
    *)
      failed=$((failed + 1))
      echo "$file: $(jq -r '.error.code + ": " + .error.message' <<<"$json" 2>/dev/null || echo "exit $rc")" >&2
      continue ;;
  esac
  seen=$((seen + 1))
  jq -r '.data as $d | ($d.images[0] // {}) as $i | [
      $d.path, $d.format.id, ($d.images | length), $d.plane_count,
      (($d.tables // []) | length), ([($d.tables // [])[].row_count] | add // 0),
      (if $i.size_x then "\($i.size_x)x\($i.size_y)" else "" end),
      ($i.size_z // ""), ($i.size_c // ""), ($i.size_t // ""), ($i.pixel_type // ""),
      (if $i.physical_size.x then ($i.physical_size.x * 10000 | round / 10000) else "" end),
      ($i.acquired_at // ($d.tables // [])[0].extra.acquisition_start // "")
    ] | @tsv' <<<"$json"
done < <(find "$dir/" -type f -print0 | sort -z)   # the trailing / also follows a symlinked DIR

echo "summarized $seen file(s); $corrupt corrupt, $unsupported unsupported, $skipped not instrument files, $failed failed" >&2
[ "$failed" -eq 0 ]
