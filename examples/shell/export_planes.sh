#!/usr/bin/env bash
# export_planes.sh: export one channel's middle z-slice at the first time point of every
# image in the given files, as small OME-TIFFs, e.g. to build a quick-look folder or a
# training set without converting whole stacks.
#
# Usage:   examples/shell/export_planes.sh OUTDIR FILE... [env CHANNEL=0 TO=ome-tiff]
# Needs:   openreadout (or set OPENREADOUT=/path/to/openreadout), jq.
# Output:  OUTDIR/<file stem>_img<i>_c<c>_z<z>.ome.tiff (or .ome.zarr with TO=ome-zarr),
#          one line per export with the plane count and the read-back verification result.
# Exit:    0 if every export was written and verified, 1 otherwise.
#
# Selection syntax is the same as `openreadout export --select`: c=0, z=2-5, t=0,3,7.
# The source files are only read; each export is verified by reading it back.
set -euo pipefail

BIN="${OPENREADOUT:-openreadout}"
CHANNEL="${CHANNEL:-0}"
TO="${TO:-ome-tiff}"
out="${1:?usage: export_planes.sh OUTDIR FILE...}"; shift
[ "$#" -gt 0 ] || { echo "usage: export_planes.sh OUTDIR FILE..." >&2; exit 2; }
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 2; }
case "$TO" in ome-tiff) ext=ome.tiff ;; ome-zarr) ext=ome.zarr ;; *) echo "TO must be ome-tiff or ome-zarr" >&2; exit 2 ;; esac
mkdir -p "$out"

failed=0
for file in "$@"; do
  rc=0
  info=$("$BIN" info "$file" --json 2>/dev/null) || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "skip $file (exit $rc)" >&2
    [ "$rc" -eq 3 ] || failed=$((failed + 1))
    continue
  fi
  stem=$(basename "$file"); stem="${stem%.*}"
  # One line per image: index, size_z, size_c.
  while read -r img size_z size_c; do
    if [ "$CHANNEL" -ge "$size_c" ]; then
      echo "skip $file image $img: has $size_c channel(s), CHANNEL=$CHANNEL" >&2
      continue
    fi
    z=$((size_z / 2))
    dest="$out/${stem}_img${img}_c${CHANNEL}_z${z}.$ext"
    rc=0
    report=$("$BIN" export "$file" --format "$TO" -o "$dest" --overwrite \
      --image "$img" --select "c=$CHANNEL" --select "z=$z" --select t=0 --json 2>/dev/null) || rc=$?
    if [ "$rc" -eq 0 ] && [ "$(jq -r .data.verified <<<"$report")" = true ]; then
      echo "$dest: $(jq -r '"\(.data.planes_written) plane(s), \(.data.bytes_written) bytes, verified"' <<<"$report")"
    else
      failed=$((failed + 1))
      echo "FAILED $file image $img: $(jq -r '.error.message // "not verified"' <<<"$report" 2>/dev/null || echo "exit $rc")" >&2
    fi
  done < <(jq -r '.data.images[] | "\(.index) \(.size_z) \(.size_c)"' <<<"$info")
done
[ "$failed" -eq 0 ]
