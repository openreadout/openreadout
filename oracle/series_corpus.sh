#!/bin/bash
# Ground truth of the sampled-column formats (EPR, X-ray diffraction, electrochemistry) of the
# development corpus: every command that writes corpus/oracle/series/*.json.
#   PY=../../oracle/.venv/bin/python bash oracle/series_corpus.sh   (numpy, geddes, galvani installed)
set -e
cd "$(dirname "$0")"
PY=${PY:-python}
C=${OPENREADOUT_CORPUS_DIR:-../corpus/files}
g() { $PY series_oracle.py --id $1 --format $2 $C/$1.$3 --geddes "${@:4}"; }
# ---- epr
# EPR ground truth (development files)
for id in epr-easyspin-e580-cwx epr-cwepr-bdpa-2dfieldpower epr-killian-tot-kinetics epr-zenodo15590546-deer epr-zenodo10463869-asympol epr-zenodo14034164-ref epr-zenodo14034164-img epr-zenodo18969515-eseem epr-deernet-deer-252cl epr-deernet-white-1b epr-mars-ky3-96k epr-watersplitting-js463; do
  $PY series_oracle.py --id $id --format bruker-bes3t $C/$id.DSC --deerload
done
$PY series_oracle.py --id epr-pyepri-fusillo --format bruker-bes3t $C/epr-pyepri-fusillo.DSC --deerload \
  --export $C/epr-pyepri-fusillo.export.txt --ycol 0 --y-tol-rel 5e-9
$PY series_oracle.py --id epr-killian-tot-accu --format bruker-bes3t $C/epr-killian-tot-accu.DSC --deerload \
  --export $C/epr-killian-tot-accu.export.txt --skip-until Intensity --xcol 1 --ycol 2 --x-tol 1e-6 --y-tol-rel 1e-12
$PY series_oracle.py --id epr-zenodo45520-sige --format bruker-esp $C/epr-zenodo45520-sige.par \
  --export $C/epr-zenodo45520-sige.export.asc --skip-until Intensity --xcol 0 --ycol 1 --x-tol 3e-4 --y-tol-abs 1e-6 \
  --fact method.parameters.microwave_frequency=9.478 --fact method.parameters.modulation_amplitude=1
$PY series_oracle.py --id epr-zenodo5925657-angle --format bruker-esp $C/epr-zenodo5925657-angle.par \
  --export $C/epr-zenodo5925657-angle.export.csv --xcol 0 --ycol 1 --delim , --x-tol 1e-4 --y-tol-abs 1e-5 --sweep 0 \
  --export $C/epr-zenodo5925657-angle.export.csv --xcol 0 --ycol 19 --delim , --x-tol 1e-4 --y-tol-abs 1e-5 --sweep 18 \
  --export $C/epr-zenodo5925657-angle.export.csv --xcol 0 --ycol 37 --delim , --x-tol 1e-4 --y-tol-abs 1e-5 --sweep 36
# ---- xrd
# X-ray diffraction ground truth (development files)
$PY series_oracle.py --id xrd-zenodo15557974-car24014 --format panalytical-xrdml $C/xrd-zenodo15557974-car24014.xrdml \
  --export $C/xrd-zenodo15557974-car24014.export.csv --skip-until "[Scan points]" --xcol 0 --ycol 2 --x-tol 5e-9 \
  --fact method.parameters.wavelength_kalpha1=1.78901 --fact method.parameters.wavelength_kalpha2=1.7929 \
  --fact method.parameters.anode=Co --fact method.parameters.tube_voltage=45 --fact method.parameters.tube_current=40 \
  --fact method.parameters.counting_time=48.195 --fact sample.id=2024-1613
$PY series_oracle.py --id xrd-zenodo5484779-rutile-nb5 --format panalytical-xrdml $C/xrd-zenodo5484779-rutile-nb5.xrdml \
  --export $C/xrd-zenodo5484779-rutile-nb5.export.csv --skip-until "[Scan points]" --xcol 0 --ycol 1 --x-tol 5e-9 \
  --fact method.parameters.wavelength_kalpha1=1.78901 --fact method.parameters.anode=Co \
  --fact method.parameters.tube_voltage=45 --fact method.parameters.tube_current=40 --fact method.parameters.counting_time=259.715
$PY series_oracle.py --id xrd-zenodo15498085-nn --format panalytical-xrdml $C/xrd-zenodo15498085-nn.xrdml \
  --export $C/xrd-zenodo15498085-nn.export.xy --xcol 0 --ycol 1 --x-tol 5e-9
$PY series_oracle.py --id xrd-zenodo20826400-kpc-low --format panalytical-xrdml $C/xrd-zenodo20826400-kpc-low.xrdml \
  --export $C/xrd-zenodo20826400-kpc-low.export.xy --xcol 0 --ycol 1 --x-tol 5e-9
$PY series_oracle.py --id xrd-yadg-210520step1 --format panalytical-xrdml $C/xrd-yadg-210520step1.xrdml \
  --export $C/xrd-yadg-210520step1.export.xy --xcol 0 --ycol 1 --x-tol 5e-9 \
  --export $C/xrd-yadg-210520step1.export.csv --skip-until "[Scan points]" --xcol 0 --ycol 1 --x-tol 5e-9 \
  --fact method.parameters.wavelength_kalpha1=1.540598 --fact method.parameters.anode=Cu --fact method.parameters.counting_time=50.165
# ---- xrd2
# X-ray diffraction ground truth: Bruker RAW/BRML, Rigaku RAS/RASX (development files)
$PY series_oracle.py --id xrd-zenodo17012484-raw1-mudstone --format bruker-raw $C/xrd-zenodo17012484-raw1-mudstone.raw --geddes \
  --export $C/xrd-zenodo17012484-raw1-mudstone.export.uxd --skip-until _2THETACOUNTS --xcol 0 --ycol 1 --x-tol 1e-6 \
  --fact sample.id=0.7B200_5D --fact acquisition.operator=Administrator --fact method.parameters.anode=Cu \
  --fact method.parameters.wavelength_kalpha1=1.5406 --fact method.parameters.wavelength_kalpha2=1.54439 \
  --fact method.parameters.tube_voltage=40 --fact method.parameters.tube_current=40 \
  --fact method.parameters.step_time=1.19998:1e-6 --fact method.parameters.goniometer_radius=300 \
  --fact acquisition.started_at=2024-07-02T08:24:41
$PY series_oracle.py --id xrd-xylib-raw1-bt86 --format bruker-raw $C/xrd-xylib-raw1-bt86.raw --geddes \
  --export $C/xrd-xylib-raw1-bt86.export.uxd --skip-until _2THETACOUNTS --xcol 0 --ycol 1 --x-tol 5e-3 \
  --fact sample.id=BT86 --fact method.parameters.anode=Cu --fact method.parameters.wavelength_kalpha1=1.5406 \
  --fact method.parameters.tube_voltage=45 --fact method.parameters.step_time=0.5 \
  --fact acquisition.started_at=2010-03-04T20:59:14
for id in xrd-zenodo14852801-raw1-strings xrd-zenodo5013537-raw1-knbo3 xrd-zenodo10891027-raw4-eptachori \
          xrd-zenodo1291900-raw4-mo xrd-geddes-raw4-v5converter xrd-fairmat-raw4-scrambled xrd-zenodo14176267-raw-sandstone xrd-zenodo14278562-raw-carmine; do
  g $id bruker-raw raw
done
$PY series_oracle.py --id xrd-zenodo17227355-raw4-tumba --format bruker-raw $C/xrd-zenodo17227355-raw4-tumba.raw --geddes \
  --export $C/xrd-zenodo17227355-raw4-tumba.export.xy --xcol 0 --ycol 1 --x-tol 6e-5
# RAW4 and BRML of the same measurements: the BRML file's tube settings and wavelengths
for id in xrd-zenodo10891027-brml-eptachori xrd-zenodo6362954-brml-opa xrd-matterviz-brml-ybco xrd-fairmat-brml-2thomega; do
  g $id bruker-brml brml
done
$PY series_oracle.py --id xrd-nims-ras --format rigaku-ras $C/xrd-nims-ras.ras --geddes \
  --export $C/xrd-nims-ras.export.csv --xcol 0 --ycol 1 --delim , --x-tol 1e-9
g xrd-zenodo11161891-ras-cowo4 rigaku-ras ras
$PY series_oracle.py --id xrd-zenodo17159456-ras-dyal2 --format rigaku-ras $C/xrd-zenodo17159456-ras-dyal2.ras \
  --export $C/xrd-zenodo17159456-ras-dyal2.export.asc --parse values --skip-until "*COUNT" --stop-at "*END" \
  --fact method.parameters.anode=Cu --fact method.parameters.tube_voltage=40 --fact method.parameters.tube_current=30 \
  --fact method.parameters.wavelength_kalpha1=1.54059:2e-6
g xrd-fairmat-rasx-powder rigaku-rasx rasx
# ---- echem
# electrochemistry ground truth (development files)
for s in cp ca-issue-149 coc-issue-185 cv cva-issue-202 gcpl-issue-149 gcpl-pr-182-1 geis-issue-149 lsv mb-issue-95 mb-issue-223 ocv peis peis-issue-225-ewe-ece zir vsp-ocv-with bcd-issue-241; do
  id=echem-yadg-$s
  $PY series_oracle.py --id $id --format biologic-mpr $C/$id.mpr --mpt $C/$id.mpt --galvani --start
  $PY series_oracle.py --id $id-mpt --format biologic-mpt $C/$id.mpt --galvani $C/$id.mpr
done
$PY series_oracle.py --id echem-galvani-v1150-ca --format biologic-mpr $C/echem-galvani-v1150-ca.mpr --mpt $C/echem-galvani-v1150-ca.mpt --galvani --start
$PY series_oracle.py --id echem-galvani-v1150-ca-mpt --format biologic-mpt $C/echem-galvani-v1150-ca.mpt --galvani $C/echem-galvani-v1150-ca.mpr
$PY series_oracle.py --id echem-zenodo12205268-ocv-ref --format biologic-mpr $C/echem-zenodo12205268-ocv-ref.mpr --mpt $C/echem-zenodo12205268-ocv-ref.txt --galvani --start
$PY series_oracle.py --id echem-zenodo12205268-ocv-ref-txt --format biologic-mpt $C/echem-zenodo12205268-ocv-ref.txt --galvani $C/echem-zenodo12205268-ocv-ref.mpr
$PY series_oracle.py --id echem-zenodo15211416-cv-ferri --format biologic-mpr $C/echem-zenodo15211416-cv-ferri.mpr --galvani --start
$PY series_oracle.py --id echem-navani-ocv --format biologic-mpr $C/echem-navani-ocv.mpr --galvani --start
$PY series_oracle.py --id echem-zenodo7245929-peis-mpt --format biologic-mpt $C/echem-zenodo7245929-peis.mpt --mpt $C/echem-zenodo7245929-peis.mpt --second-implementation
# data module version 0 (figshare 1228760)
for n in 1 4; do
  id=echem-figshare1228760-bio-logic$n
  $PY series_oracle.py --id $id --format biologic-mpr $C/$id.mpr --mpt $C/$id.mpt --galvani --start
  $PY series_oracle.py --id $id-mpt --format biologic-mpt $C/$id.mpt --galvani $C/$id.mpr
done
# time/s written as dates and times, no .mpr deposited (figshare 30080953)
for s in cp ca; do
  id=echem-figshare30080953-$s-mpt
  $PY series_oracle.py --id $id --format biologic-mpt $C/echem-figshare30080953-$s.mpt --mpt $C/echem-figshare30080953-$s.mpt --second-implementation
done
# ---- echem_extra
# galvani cannot read these .mpr files (column ids above 255 in data module 11): the .mpt inputs
# are checked by a second, standard-library reading of the table
for s in coc-issue-185 mb-issue-223 peis-issue-225-ewe-ece; do
  id=echem-yadg-$s
  $PY series_oracle.py --id $id-mpt --format biologic-mpt $C/$id.mpt --mpt $C/$id.mpt --second-implementation
done
# ---- gamry
for id in echem-zenodo7245929-rde-cv-dimethyl echem-zenodo7245929-rde-orr-dimethyl echem-zenodo7245929-rde-eis-dimethyl echem-impedancepy-eis; do
  $PY series_oracle.py --id $id --format gamry-dta $C/$id.DTA --gamry
done
$PY series_oracle.py --id echem-gamryparser-cv --format gamry-dta $C/echem-gamryparser-cv.dta --gamry
# ---- neware (NewareNDA installed)
for id in echem-newarenda-new-nda-file echem-newarenda-sim-nda; do
  $PY series_oracle.py --id $id --format neware-nda $C/$id.nda --newarenda --start
done
for id in echem-newarenda-issue72-nda echem-zenodo21631502-sintef-nda; do
  $PY series_oracle.py --id $id --format neware-nda $C/$id.nda --newarenda --time-total --start
done
$PY series_oracle.py --id echem-newarenda-2-1-6-61-nda --format neware-nda $C/echem-newarenda-2-1-6-61-nda.nda --newarenda --no-cycle --start
for id in echem-newarenda-unit27-ndax echem-newarenda-issue60-ndax echem-navani-ndax echem-cellpy-ife-ndax; do
  $PY series_oracle.py --id $id --format neware-ndax $C/$id.ndax --newarenda --start
done
# ---- netzsch (pyngb installed)
$PY series_oracle.py --id ngb-pyngb-01-messung-probe-g-300-grad-ar --format netzsch-ngb $C/ngb-pyngb-01-messung-probe-g-300-grad-ar.ngb-ds3 --pyngb
$PY series_oracle.py --id ngb-pyngb-02-messung-probe-e-300-grad-ar --format netzsch-ngb $C/ngb-pyngb-02-messung-probe-e-300-grad-ar.ngb-ds3 --pyngb
$PY series_oracle.py --id ngb-pyngb-df-filed-sta-21o2-10k-220222-r1 --format netzsch-ngb $C/ngb-pyngb-df-filed-sta-21o2-10k-220222-r1.ngb-ss3 --pyngb
$PY series_oracle.py --id ngb-pyngb-douglas-fir-sta-10k-250730-r13 --format netzsch-ngb $C/ngb-pyngb-douglas-fir-sta-10k-250730-r13.ngb-ss3 --pyngb
$PY series_oracle.py --id ngb-pyngb-douglas-fir-sta-baseline-10k-250730-r13 --format netzsch-ngb $C/ngb-pyngb-douglas-fir-sta-baseline-10k-250730-r13.ngb-bs3 --pyngb
$PY series_oracle.py --id ngb-pyngb-douglas-fir-sta-baseline-10k-250813-r15 --format netzsch-ngb $C/ngb-pyngb-douglas-fir-sta-baseline-10k-250813-r15.ngb-bs3 --pyngb
$PY series_oracle.py --id ngb-pyngb-ro-filed-sta-n2-10k-250129-r29 --format netzsch-ngb $C/ngb-pyngb-ro-filed-sta-n2-10k-250129-r29.ngb-ss3 --pyngb
$PY series_oracle.py --id ngb-pyngb-ro-para-n2-10k-260910-correction-r1 --format netzsch-ngb $C/ngb-pyngb-ro-para-n2-10k-260910-correction-r1.ngb-cla --pyngb
$PY series_oracle.py --id ngb-pyngb-ro-para-n2-10k-260910-r1 --format netzsch-ngb $C/ngb-pyngb-ro-para-n2-10k-260910-r1.ngb-dla --pyngb
$PY series_oracle.py --id ngb-pyngb-ro-perp-n2-10k-260917-r1 --format netzsch-ngb $C/ngb-pyngb-ro-perp-n2-10k-260917-r1.ngb-dla --pyngb
$PY series_oracle.py --id ngb-pyngb-red-oak-sta-10k-250731-r7 --format netzsch-ngb $C/ngb-pyngb-red-oak-sta-10k-250731-r7.ngb-ss3 --pyngb
$PY series_oracle.py --id ngb-zenodo18902844-pla-fw --format netzsch-ngb $C/ngb-zenodo18902844-pla-fw.ngb-sd7 --pyngb --expdat $C/ngb-zenodo18902844-pla-fw-export-1.xlsx --expdat $C/ngb-zenodo18902844-pla-fw-export-2-grzanie.xlsx
$PY series_oracle.py --id ngb-zenodo18902844-pla-200um --format netzsch-ngb $C/ngb-zenodo18902844-pla-200um.ngb-sd7 --pyngb --expdat $C/ngb-zenodo18902844-pla-200um-export-1.xlsx --expdat $C/ngb-zenodo18902844-pla-200um-export-2-grzanie.xlsx --expdat $C/ngb-zenodo18902844-pla-200um-export-chlodzenie.xlsx
$PY series_oracle.py --id ngb-zenodo18902844-pla-600um --format netzsch-ngb $C/ngb-zenodo18902844-pla-600um.ngb-sd7 --pyngb --expdat $C/ngb-zenodo18902844-pla-600um-export-1.xlsx --expdat $C/ngb-zenodo18902844-pla-600um-export-2-grzanie.xlsx
$PY series_oracle.py --id ngb-zenodo18902844-pef-fw --format netzsch-ngb $C/ngb-zenodo18902844-pef-fw.ngb-sd7 --pyngb --expdat $C/ngb-zenodo18902844-pef-fw-export-1.xlsx --expdat $C/ngb-zenodo18902844-pef-fw-export-2-grzanie.xlsx --expdat $C/ngb-zenodo18902844-pef-fw-export-chlodzenie.xlsx
$PY series_oracle.py --id ngb-zenodo18902844-pef-200um --format netzsch-ngb $C/ngb-zenodo18902844-pef-200um.ngb-sd7 --pyngb --expdat $C/ngb-zenodo18902844-pef-200um-export-1.xlsx --expdat $C/ngb-zenodo18902844-pef-200um-export-2-grzanie.xlsx --expdat $C/ngb-zenodo18902844-pef-200um-export-chlodzenie.xlsx
$PY series_oracle.py --id ngb-zenodo18902844-pef-600um --format netzsch-ngb $C/ngb-zenodo18902844-pef-600um.ngb-sd7 --pyngb --expdat $C/ngb-zenodo18902844-pef-600um-export-1.xlsx --expdat $C/ngb-zenodo18902844-pef-600um-export-2-grzanie.xlsx --expdat $C/ngb-zenodo18902844-pef-600um-export-chlodzenie.xlsx
$PY series_oracle.py --id ngb-zenodo18902844-pla-pef-pla --format netzsch-ngb $C/ngb-zenodo18902844-pla-pef-pla.ngb-sd7 --pyngb --expdat $C/ngb-zenodo18902844-pla-pef-pla-export-1-grzanie.xlsx --expdat $C/ngb-zenodo18902844-pla-pef-pla-export-2-grzanie.xlsx --expdat $C/ngb-zenodo18902844-pla-pef-pla-export-chlodzenie.xlsx
# ---- TA Instruments (Universal Analysis exports)
$PY series_oracle.py --id ta-zenodo17293641-pcl-standard --format ta-universal-analysis $C/ta-zenodo17293641-pcl-standard.001 --ta-export $C/ta-zenodo17293641-pcl-standard.export.txt
$PY series_oracle.py --id ta-zenodo17293641-pcl-mdsc-r --format ta-universal-analysis $C/ta-zenodo17293641-pcl-mdsc-r.001 --ta-export $C/ta-zenodo17293641-pcl-mdsc-r.export.txt --ta-interp
$PY series_oracle.py --id ta-dscq20-data-079 --format ta-universal-analysis $C/ta-dscq20-data-079.001 --ta-export $C/ta-dscq20-data-079.export.tsv
$PY series_oracle.py --id ta-dscq20-data-082 --format ta-universal-analysis $C/ta-dscq20-data-082.001 --ta-export $C/ta-dscq20-data-082.export.tsv
$PY series_oracle.py --id ta-dscq20-data-083 --format ta-universal-analysis $C/ta-dscq20-data-083.001 --ta-export $C/ta-dscq20-data-083.export.tsv
$PY series_oracle.py --id ta-dscq20-data-085 --format ta-universal-analysis $C/ta-dscq20-data-085.001 --ta-export $C/ta-dscq20-data-085.export.tsv
# ---- agilent-cary: the Cary software's CSV export of each .DSW (Błaszkiewicz, Zenodo)
$PY series_oracle.py --id cary-z14894113-3-1 --format agilent-cary $C/cary-z14894113-3-1.dsw \
  --export $C/cary-z14894113-3-1.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14894113-a2 --format agilent-cary $C/cary-z14894113-a2.dsw \
  --export $C/cary-z14894113-a2.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14894113-3 --format agilent-cary $C/cary-z14894113-3.dsw \
  --export $C/cary-z14894113-3.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14894113-au1 --format agilent-cary $C/cary-z14894113-au1.dsw \
  --export $C/cary-z14894113-au1.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10873570-au --format agilent-cary $C/cary-z10873570-au.dsw \
  --export $C/cary-z10873570-au.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10873570-aunps-peg-pbs-20-iii --format agilent-cary $C/cary-z10873570-aunps-peg-pbs-20-iii.dsw \
  --export $C/cary-z10873570-aunps-peg-pbs-20-iii.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10873570-aunrs-po-pegylacji-bez-wirowania --format agilent-cary $C/cary-z10873570-aunrs-po-pegylacji-bez-wirowania.dsw \
  --export $C/cary-z10873570-aunrs-po-pegylacji-bez-wirowania.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10873570-antybiotyk --format agilent-cary $C/cary-z10873570-antybiotyk.dsw \
  --export $C/cary-z10873570-antybiotyk.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10953327-aunrs-peg3 --format agilent-cary $C/cary-z10953327-aunrs-peg3.dsw \
  --export $C/cary-z10953327-aunrs-peg3.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10953327-aunrs2k --format agilent-cary $C/cary-z10953327-aunrs2k.dsw \
  --export $C/cary-z10953327-aunrs2k.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10953327-aunrs-peg --format agilent-cary $C/cary-z10953327-aunrs-peg.dsw \
  --export $C/cary-z10953327-aunrs-peg.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z10953327-aunrs3 --format agilent-cary $C/cary-z10953327-aunrs3.dsw \
  --export $C/cary-z10953327-aunrs3.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893472-dpbf1 --format agilent-cary $C/cary-z14893472-dpbf1.dsw \
  --export $C/cary-z14893472-dpbf1.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893472-miesz1-1080-ii --format agilent-cary $C/cary-z14893472-miesz1-1080-ii.dsw \
  --export $C/cary-z14893472-miesz1-1080-ii.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893472-miesz1-480 --format agilent-cary $C/cary-z14893472-miesz1-480.dsw \
  --export $C/cary-z14893472-miesz1-480.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893472-miesz1-960-bablowanie --format agilent-cary $C/cary-z14893472-miesz1-960-bablowanie.dsw \
  --export $C/cary-z14893472-miesz1-960-bablowanie.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893496-rb1 --format agilent-cary $C/cary-z14893496-rb1.dsw \
  --export $C/cary-z14893496-rb1.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893496-aunrs76 --format agilent-cary $C/cary-z14893496-aunrs76.dsw \
  --export $C/cary-z14893496-aunrs76.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893496-rb-02-nano200 --format agilent-cary $C/cary-z14893496-rb-02-nano200.dsw \
  --export $C/cary-z14893496-rb-02-nano200.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
$PY series_oracle.py --id cary-z14893496-rb-dichloro --format agilent-cary $C/cary-z14893496-rb-dichloro.dsw \
  --export $C/cary-z14893496-rb-dichloro.export.csv --skip-until Wavelength --xcol 0 --ycol 1 --delim , --channel absorbance --x-tol 5e-4 --y-tol-rel 1e-7 --y-tol-abs 1e-9
