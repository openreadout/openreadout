#!/bin/sh
# One oracle venv that can regenerate every committed evals fact file (evals/analysis.py and the
# fact scripts it imports): the default dependencies plus the `chrom` and `qpcr` groups, plus
# allotropy installed without its dependency pin on rainbow-api 1.0.10 (the `plate` group
# conflicts with `chrom`, which needs rainbow-api >= 1.5.2; allotropy's plate parsers used here
# do not import rainbow). Run from the repository root: sh oracle/setup_full_env.sh
set -e
cd "$(dirname "$0")"
uv sync --group chrom --group qpcr
VIRTUAL_ENV="$PWD/.venv" uv pip install --no-deps "allotropy==0.1.146"
VIRTUAL_ENV="$PWD/.venv" uv pip install babel chardet openpyxl defusedxml python-calamine
.venv/bin/python -c "import allotropy.parser_factory, rainbow, rdmlpython, openslide, pyopenms; print('oracle env: all fact readers import')"
