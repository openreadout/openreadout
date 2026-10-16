#!/bin/sh
# Ground truth for every NanoDrop and Qubit corpus file (benchtop_oracle.py).
#   OPENREADOUT_CORPUS_DIR=... sh benchtop_corpus.sh
set -eu
cd "$(dirname "$0")"
DIR="${OPENREADOUT_CORPUS_DIR:-../corpus/files}"
for id in $(awk -F'"' '/^id = /{id=$2} /^format = "(nanodrop|qubit)"/{print id}' ../corpus/manifest.toml); do
  f=$(ls "$DIR/$id".* 2>/dev/null | head -n 1)
  if [ -z "$f" ]; then
    echo "missing $id (cargo xtask corpus fetch --tier standard --format nanodrop|qubit)" >&2
    continue
  fi
  uv run --only-group plate --with pandas python benchtop_oracle.py --id "$id" "$f"
done
