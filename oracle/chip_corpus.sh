#!/bin/bash
# Ground truth of the Agilent Bioanalyzer and TapeStation exports of the corpus (hold tier).
#   PY=python bash oracle/chip_corpus.sh
set -e
cd "$(dirname "$0")"
PY=${PY:-python}
C=${OPENREADOUT_CORPUS_DIR:-../corpus/files}
for id in agilent-bioanalyzer-dna1000 agilent-bioanalyzer-rna-nano agilent-bioanalyzer-hsdna; do
  $PY chip_oracle.py --id $id $C/$id.xml.gz
done
for id in agilent-tapestation-d1000 agilent-tapestation-gdna agilent-tapestation-rna agilent-tapestation-hsd1000; do
  $PY chip_oracle.py --id $id $C/$id.xml.gz $C/${id}_Electropherogram.csv.gz
done
