#!/bin/bash
# Ground truth of the Applied Biosystems ABIF files of the corpus: every command that writes
# corpus/oracle/abif/*.json (and the held-out ones in corpus/oracle/heldout).
#   PY=../../oracle/.venv/bin/python bash oracle/abif_corpus.sh   (Biopython installed)
set -e
cd "$(dirname "$0")"
PY=${PY:-python}
C=${OPENREADOUT_CORPUS_DIR:-../corpus/files}
for id in abif-biopython-310 abif-biopython-3100 abif-biopython-3730 abif-biopython-a6-1-db3 \
          abif-biopython-empty abif-biopython-no-smpl1 abif-biopython-nonascii; do
  $PY abif_oracle.py --id $id $C/$id.ab1
done
for id in abif-biopython-fsa abif-fraggler-multiplex abif-fraggler-error abif-fraggler-no-peaks; do
  $PY abif_oracle.py --id $id $C/$id.fsa
done
wells=(A B C D E F G)
for k in 1 2 3 4 5 6 7; do
  $PY abif_oracle.py --id abif-fraggler-k$k $C/abif-fraggler-k$k.fsa \
    --peakscanner $C/abif-fraggler-peakscanner.csv --name "Multi_K${k}__230907PRT1-4test_${wells[$((k-1))]}04_Dx.fsa"
done
# held-out (measured, never developed against)
$PY abif_oracle.py --out ../corpus/oracle/heldout --id ho-pooledpeaks-23-2a \
  "$C/heldout/ho-pooledpeaks-23-2a/23.2a_I_A01_2012-07-18.fsa"
$PY abif_oracle.py --out ../corpus/oracle/heldout --id ho-pooledpeaks-shaem-3a \
  "$C/heldout/ho-pooledpeaks-shaem-3a/Multiplex_set_I_Shaem.3a_2_Sample_20221028_215633.fsa"
