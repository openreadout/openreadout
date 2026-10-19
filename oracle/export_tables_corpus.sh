#!/bin/sh
# Ground truth for every corpus file of the formats export_tables.py reads.
#   OPENREADOUT_CORPUS_DIR=... sh export_tables_corpus.sh [FORMAT]
set -eu
cd "$(dirname "$0")"
DIR="${OPENREADOUT_CORPUS_DIR:-../corpus/files}"
FORMATS="${1:-biorad-ddpcr}"
for fmt in $FORMATS; do
  for id in $(awk -F'"' -v f="$fmt" '/^id = /{id=$2} /^format = /{if ($2 == f) print id}' ../corpus/manifest.toml); do
    f=$(ls "$DIR/$id".* 2>/dev/null | head -n 1)
    if [ -z "$f" ]; then
      echo "missing $id (cargo xtask corpus fetch --tier standard --format $fmt)" >&2
      continue
    fi
    python3 -I export_tables.py --id "$id" --format "$fmt" "$f"
  done
done
