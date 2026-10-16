#!/usr/bin/env bash
# Test the Python package and the bioio plugin from the wheel in dist/, then lint and type-check
# the Python sources. CI's `python` job runs this after `maturin build --profile dev --out dist`;
# `cargo xtask ci` runs it the same way.
#
# Each interpreter in $PYTHONS (default: python3) gets a fresh virtual environment under
# target/ci-venv/, so nothing is installed into your own Python. Set OPENREADOUT_CORPUS_DIR to run
# the tests that read the smoke corpus.
set -euo pipefail
cd "$(dirname "$0")/../.."

# The newest wheel, in case dist/ holds older builds.
wheel=$(ls -t dist/openreadout-*.whl 2>/dev/null | head -n 1)
if [ -z "$wheel" ]; then
  echo "no wheel in dist/; build one first: maturin build --profile dev --out dist" >&2
  exit 1
fi

venv=""
for py in ${PYTHONS:-python3}; do
  venv="target/ci-venv/$py"
  echo "== $py: $("$py" --version) in $venv"
  rm -rf "$venv"
  "$py" -m venv "$venv"
  "$venv/bin/python" -m pip install --quiet --upgrade pip
  "$venv/bin/python" -m pip install --quiet "${wheel}[test]" xxhash "./python/bioio-openreadout[test]"
  "$venv/bin/python" -m pip list | grep -Ei 'openreadout|bioio|czifile|liffile|nd2|dask|xarray'
  "$venv/bin/python" -m pytest crates/openreadout-py/tests python/bioio-openreadout/tests -ra
done

echo "== lint and type-check (ruff, mypy) with the last interpreter"
"$venv/bin/python" -m pip install --quiet ruff mypy
"$venv/bin/ruff" format --check crates/openreadout-py python
"$venv/bin/ruff" check crates/openreadout-py python
"$venv/bin/mypy"
