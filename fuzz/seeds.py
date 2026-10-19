#!/usr/bin/env python3
"""Regenerate the committed fuzz seed corpora under fuzz/corpus/<target>/.

Usage (from the repository root; the oracle venv provides imagecodecs for codec seeds):

    OPENREADOUT_CORPUS_DIR=corpus/files oracle/.venv/bin/python fuzz/seeds.py [--info-json DIR]

Seeds are either synthesized here (minimal but complete CZI/ND2/LIF files, codec streams of
small generated images) or cut from permissively licensed corpus files (first 64 KiB, or
single structures such as one subblock header or one LV chunk). Provenance of every
corpus-derived seed is listed below with its license (see fuzz/README.md). Keep the total under 3 MB.
"""

from __future__ import annotations

import gzip
import os
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OUT = ROOT / "corpus"
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT.parent / "corpus" / "files"))
HEAD = 64 * 1024

# Corpus files seeds may be cut from (all in corpus/manifest.toml with redistributable licenses).
CZI_FILES = [
    "zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging.czi",  # CC-BY-4.0
    "aics-s-1-t-1-c-1-z-1.czi",  # BSD-3-Clause
    "zenodo10577621-LineScan-Z200.czi",  # CC-BY-4.0
]
ND2_FILES = [
    "aics-ND2-dims-rgb.nd2",  # BSD-3-Clause
    "aics-ND2-dims-c2y32x32.nd2",  # BSD-3-Clause
    "aics-ND2-maxime-BF007.nd2",  # BSD-3-Clause
]
LIF_FILES = [
    "ome-michael-PR2729-frameOrderCombinedScanTypes.lif",  # CC-BY-4.0
    "bsst749-2a-ishi-hf-fshr-dyngo-fsh-5.lif",  # EMBL-EBI terms (redistributable)
]


BUNDLE_SEP = b"\n=====FUZZ-NEXT-FILE=====\n"
SIRIUS = "allotropy-openlab-sirius-rslt/Sirius-2023-09-01 07-52-44-04-00.rslt/"  # whole files (a zip needs its central directory)
WATERS = "mtbls15166-brain-b1-raw-zip/ANR_DeciVerneuil_TRP_Depletion_Kinetics_2_Brain_B1.raw/"  # fuzz/src/lib.rs BUNDLE_SEP

# (target, corpus file, bytes to keep)
COMMITTED_CUTS = [
    ("core_zip", "rdml-stepone-std.rdml", 16 * 1024),  # MIT (whole file: 8.7 KB)
    ("whole_thermo", "mtbls805-msms-869.raw", 160 * 1024),  # EMBL-EBI terms (MetaboLights)
    ("whole_fcs", "fcsparser-cyflow-cube-8.fcs", 64 * 1024),  # MIT
    ("whole_fcs", "flowio-data1.fcs", 32 * 1024),  # BSD-3-Clause
    ("whole_jeol", "nmrxiv-s1243/E sinica-higher concentration-500 MHz.jdf", 64 * 1024),  # CC0-1.0
    ("whole_jeol", "nmrxiv-s200/Limonene_7020ug200uL_CDCl3_HSQC_400MHz_Jeol.jdf", 64 * 1024),  # CC0-1.0
    ("whole_abf", "pyabf-2018-12-09-pclamp11-0001.abf", 64 * 1024),  # MIT
    ("whole_tiff", "aics-s_1_t_1_c_2_z_1_RGB.tiff", 64 * 1024),  # BSD-3-Clause
    ("whole_tiff", "ome-artificial-single-channel.ome.tiff", 80 * 1024),  # CC-BY-4.0
    ("whole_mrc", "mrcfile-emd-3197.map", 64 * 1024),  # CC0-1.0
    ("whole_dm", "zenodo8190744-EELS-STO.dm3", 64 * 1024),  # CC-BY-4.0
    ("whole_dicom", "pydicom/CT_small.dcm", 64 * 1024),  # MIT
    ("whole_dicom", "pydicom/MR_small_RLE.dcm", 64 * 1024),  # MIT
    ("whole_dicom", "pydicom/JPEG2000.dcm", 64 * 1024),  # MIT
    ("whole_dicom", "pydicom-data/emri_small_jpeg_ls_lossless.dcm", 64 * 1024),  # MIT
    ("whole_dicom", "pydicom-data/liver_deflate.dcm", 64 * 1024),  # MIT
    ("whole_dicom", "pydicom/image_dfl.dcm", 64 * 1024),  # MIT
    ("whole_nifti", "nibabel/functional.nii", 64 * 1024),  # MIT
    ("whole_nifti", "nibabel/example_nifti2.nii.gz", 64 * 1024),  # MIT
    ("codec_jpegls", "pydicom/JPEGLSNearLossless_16.dcm", 64 * 1024),  # MIT (whole file; the stream starts inside it)
    ("whole_emd", "zenodo20040988-0050-STEM-15.4nm.emd", 96 * 1024),  # CC-BY-4.0
    ("whole_jcamp", "jcamp-lancashire-pktab1.jdx", 16 * 1024),  # public domain
    ("whole_jcamp", "jcamp-lancashire-compound.jdx", 32 * 1024),  # public domain
    ("whole_mzml", "pyteomics-tiny-pwiz.mzML", 32 * 1024),  # Apache-2.0
    ("whole_mzxml", "pyteomics-test-mzxml.mzXML", 32 * 1024),  # Apache-2.0
    ("whole_oir", "zenodo13680725-map-a01.oir", 64 * 1024),  # CC-BY-4.0
    ("whole_andi", "sciformats-andi_chrom_valid.cdf", 8 * 1024),  # MIT
    ("whole_andi", "cheminfo-agilent-hplc.cdf", 32 * 1024),  # MIT
    ("chrom_netcdf", "sciformats-andi_chrom_valid.cdf", 8 * 1024),  # MIT
    ("chrom_netcdf", "cheminfo-agilent-hplc.cdf", 32 * 1024),  # MIT
    ("whole_shimadzu", "zenodo17868549-gp070190p-hplc.lcd", 64 * 1024),  # CC-BY-4.0
    ("chrom_cfb", "zenodo17868549-gp070190p-hplc.lcd", 64 * 1024),  # CC-BY-4.0
    ("whole_plate", "gen5-abs450-non-numeric.txt", 8 * 1024),  # MIT
    ("whole_plate", "softmax-fl-kinetic-plates.txt", 8 * 1024),  # MIT
    ("whole_plate", "bmg-mars-lum-1536.csv", 8 * 1024),  # MIT
    ("whole_plate_xlsx", "magellan-pro-compact.xlsx", 16 * 1024),  # MIT
    ("whole_plate_xlsx", "skanit-luciferase.xlsx", 16 * 1024),  # MIT
    ("whole_qpcr_export", "qpcrx-cfx-churchlab-vwf-pecam1.csv", 8 * 1024),  # MIT
    ("tims_sqlite", "timsrust-test-dda.d/analysis.tdf", 32 * 1024),  # Apache-2.0
    ("jcamp_asdf", "jcamp-lancashire-dupinc1.jdx", 16 * 1024),  # public domain
    ("jcamp_asdf", "jcamp-lancashire-sqzdec1.jdx", 16 * 1024),  # public domain
    ("whole_zvi", "figshare-zvi/figshare15042921-wt_19h_1.zvi", 64 * 1024),  # CC-BY-4.0
    ("whole_dcimg", "dcimg/zenodo-14287640/Cell09_642_000_000.dcimg", 64 * 1024),  # CC-BY-4.0
    ("whole_dcimg", "dcimg/zenodo-14281237/Cell07_642_000_000.dcimg", 64 * 1024),  # CC-BY-4.0
    ("whole_ims", "ome-imaris/croppedRetinaLz4.ims", 128 * 1024),  # CC-BY-4.0
    ("whole_nwb", "dandi-nwb/dandi000027-sub-RAT123.nwb", 32 * 1024),  # CC-BY-4.0
    ("whole_hdf5", "dandi-nwb/dandi000027-sub-RAT123.nwb", 32 * 1024),  # CC-BY-4.0
    ("whole_opus", "opus-or2-test-spectra.0", 80 * 1024),  # MIT
    ("whole_opus", "opus-bitumen-ageing-zip/Unaged.2", 48 * 1024),  # CC-BY-4.0
    ("whole_omnic", "omnic-toffolo-library-zip/ATR/Paraffin.SPA", 40 * 1024),  # CC-BY-4.0
    ("whole_wdf", "wdf-zenodo8102788-ooid.wdf", 80 * 1024),  # CC0-1.0
    ("whole_pesp", "pesp-specio-spectra.sp", 32 * 1024),  # BSD-3-Clause
    ("whole_pesp", "pesp-zenodo8161216-ts-black-gallus-untreated-01-01.sp", 32 * 1024),  # CC-BY-4.0
    ("whole_spc", "zenodo22745748-pf1801.spc", 32 * 1024),  # CC-BY-4.0
    ("whole_spc", "zenodo2248038-nujol1.spc", 32 * 1024),  # CC-BY-4.0
    ("whole_atf", "pyabf-model-vc-step.atf", 8 * 1024),  # MIT
    ("whole_atf", "pyabf-sine-sweep-magnitude-20.atf", 8 * 1024),  # MIT
    ("whole_oib", "olympus-fv/zenodo4598136-Spleenx20.oib", 64 * 1024),  # CC0-1.0
    ("whole_pcrd", "pcrd-cfx-primerpickr.pcrd", 4 * 1024),  # CC-BY-4.0
    ("whole_unicorn_zip", "unicorn-zip-allotropy-single-uv.zip", 16 * 1024),  # MIT
    ("whole_scn", "scn-zenodo3517804-p27.scn", 256 * 1024),  # CC-BY-4.0
    ("whole_itc", "itc-zenodo21479520-hsl3-ctnip4.itc", 16 * 1024),  # CC-BY-4.0
    ("whole_xrdml", "xrd-zenodo15498085-nn.xrdml", 16 * 1024),  # CC-BY-4.0
    ("whole_bruker_raw", "xrd-zenodo17227355-raw4-tumba.raw", 16 * 1024),  # CC-BY-4.0
    ("whole_bruker_raw", "xrd-zenodo5013537-raw1-knbo3.raw", 16 * 1024),  # CC0-1.0
    ("whole_ras", "xrd-nims-ras.ras", 16 * 1024),  # MIT
    ("whole_mpr", "echem-zenodo15211416-cv-ferri.mpr", 32 * 1024),  # CC-BY-4.0
    ("whole_mpr", "echem-navani-ocv.mpr", 32 * 1024),  # MIT
    ("whole_mpt", "echem-zenodo7245929-peis.mpt", 32 * 1024),  # CC0-1.0
    ("whole_gamry", "echem-gamryparser-cv.dta", 16 * 1024),  # MIT
    ("whole_nda", "echem-newarenda-new-nda-file.nda", 128 * 1024),  # BSD-3-Clause
    ("whole_ndax", "echem-newarenda-unit27-ndax.ndax", 16 * 1024),  # BSD-3-Clause
    ("whole_arbin", "echem-navani-arbin.res", 96 * 1024),  # MIT
    ("whole_ngb", "ngb-pyngb-douglas-fir-sta-baseline-10k-250813-r15.ngb-bs3", 256 * 1024),  # MIT
    ("whole_ta001", "ta-dscq20-data-079.001", 64 * 1024),  # MIT
    ("whole_trios", "trios-zenodo17225583-gelma3-4c-freq.tri", 128 * 1024),  # CC-BY-4.0
    ("whole_biacore", "biacore-allotropy-fig4b-her3-immob.blr", 128 * 1024),  # MIT
    ("whole_bme", "biacore-bme-allotropy-example2.bme", 256 * 1024),  # MIT
    ("whole_jws", "jws-jws2txt-001hg.jws", 64 * 1024),  # MIT
    ("whole_gpr", "gpr-zenodo21015949-ab4.gpr", 16 * 1024),  # CC-BY-4.0
    ("whole_seahorse", "seahorse-zenodo10435506-mst-2.asyr", 256 * 1024),  # CC-BY-4.0
    ("whole_octet", "octet-pykingenie-230309_001.frd", 64 * 1024),  # MIT
    ("whole_zetasizer", "zetasizer-zenodo10944781-dls.dts", 64 * 1024),  # CC-BY-4.0
    ("whole_jws", "jws-jascofiles-uvvis-abs.jws", 16 * 1024),  # MIT
    ("whole_jws", "jws-jascofiles-legacy-raman.jws", 64 * 1024),  # MIT
    ("whole_cary", "cary-z14894113-a2.dsw", 64 * 1024),  # CC-BY-4.0
    ("whole_cary", "cary-pyspecdata-ras-stability4.bsk", 128 * 1024),  # CC0-1.0
    ("whole_itc", "itc-zenodo6608282-d25-50um.itc", 16 * 1024),  # CC0-1.0
    ("whole_uvprobe", "figshare29858651-uvprobe-substrate.spc", 32 * 1024),  # CC-BY-4.0
    ("whole_nanodrop", "nanodrop-8000-dna.txt", 16 * 1024),  # MIT (allotropy test data)
    ("whole_nanodrop", "nanodrop-eight-dsdna.txt", 16 * 1024),  # MIT (allotropy test data)
    ("whole_nanodrop", "nanodrop-one-dsdna.csv", 16 * 1024),  # MIT (allotropy test data)
    ("whole_nanodrop", "nanodrop-one-uvvis-nanodrop-r.tsv", 16 * 1024),  # MIT (nanodRop)
    ("whole_qubit", "qubit4-out-of-range.csv", 16 * 1024),  # MIT (allotropy test data)
    ("whole_qubit", "qubit-flex-sep-header.csv", 16 * 1024),  # MIT (allotropy test data)
]
# (target, [(corpus file, bytes to keep), ...]) joined by BUNDLE_SEP in the target's order
OIF = "olympus-fv/zenodo4421962-oif/Bead12/"
MASSHUNTER = ['MSScan.xsd', 'MSScan.bin', 'MSPeak.bin', 'MSProfile.bin', 'MSMassCal.bin', 'DefaultMassCal.xml', 'MSTS.xml', 'Contents.xml', 'Devices.xml', 'sample_info.xml', 'TCC1.cd', 'TCC1.cg']
MASSHUNTER_CUTS = {'MSScan.bin': 16384, 'MSPeak.bin': 8192, 'MSProfile.bin': 16384, 'MSMassCal.bin': 4096, 'TCC1.cg': 4096}
COMMITTED_BUNDLES = [
    ("whole_masshunter", [("mtbls449-13047CHQ_0001_A1-d/13047CHQ_0001_A1.d/AcqData/" + n, MASSHUNTER_CUTS.get(n, 64 * 1024)) for n in MASSHUNTER]),  # EMBL-EBI terms (MetaboLights)
    ("whole_masshunter", [("mtbls1334-STD_neg_MSMS_1min0205-d/STD_neg_MSMS_1min0205.d/AcqData/" + n, MASSHUNTER_CUTS.get(n, 64 * 1024)) for n in MASSHUNTER]),  # EMBL-EBI terms (MetaboLights)
    ("whole_oif", [(OIF + f, cut) for f, cut in [("50x.oif", 64 * 1024), ("50x.oif.files/s_C001.pty", 8 * 1024), ("50x.oif.files/s_C001.tif", 16 * 1024)]]),  # CC0-1.0
    ("whole_ser", [("zenodo17463176-Fig_b1_1.ser", 32 * 1024), ("zenodo17463176-Fig_b1.emi", 48 * 1024)]),  # CC-BY-4.0
    ("whole_bruker", [("nmrglue-test-data/bruker_1d/acqus", 16 * 1024), ("nmrglue-test-data/bruker_1d/fid", 16 * 1024)]),  # BSD-3-Clause
    ("whole_varian", [("nmrpy-test2.fid/procpar", 32 * 1024), ("nmrpy-test2.fid/fid", 8 * 1024)]),  # BSD-3-Clause
    ("whole_imzml", [("imzml-example-continuous.imzML", 32 * 1024), ("imzml-example-continuous.ibd", 16 * 1024)]),  # Apache-2.0
    ("whole_paravision", [("brukerapi-pv7/20210128_122257_LEGO_PHANTOM_API_TEST_1_1/20/" + n, cut) for n, cut in [("acqp", 64 * 1024), ("method", 64 * 1024), ("pdata/1/visu_pars", 64 * 1024), ("pdata/1/2dseq", 16 * 1024)]]),  # CC-BY-4.0
    ("whole_tims", [("timsrust-test-dda.d/analysis.tdf", 32 * 1024), ("timsrust-test-dda.d/analysis.tdf_bin", 4 * 1024)]),  # Apache-2.0
    ("whole_vsi", [("zenodo6094961-vsi-multifile/data/vsi-ets-test-jpg2k.vsi", 48 * 1024), ("zenodo6094961-vsi-multifile/data/_vsi-ets-test-jpg2k_/stack1/frame_t_0.ets", 32 * 1024)]),  # CC-BY-4.0
    ("whole_chemstation", [("chromhandler-CA10_100uM.D/dad1A.ch", 8 * 1024), ("entab-carotenoid_extract.d/dad1.uv", 16 * 1024), ("entab-carotenoid_extract.d/MSD1.MS", 16 * 1024)]),  # MIT
    ("whole_openlab", [(SIRIUS + f, cut) for f, cut in [("2023-09-01 07-52-57-04-00-01.dx", 8 * 1024), ("2023-09-01 07-52-57-04-00-01.rx", 16 * 1024), ("Sirius-2023-09-01 07-52-44-04-00.acaml", 80 * 1024)]]),  # MIT (allotropy test data)
    ("whole_sciex", [("mtbls6084-sl-st-blank2.wiff", 1280 * 1024), ("mtbls6084-sl-st-blank2.wiff.scan", 4 * 1024)]),  # EMBL-EBI terms (MetaboLights)
    ("whole_hcs_harmony", [("hcs/harmony-zenodo7841360/Images/Index.idx.xml", 4096), ("hcs/harmony-zenodo7841360/Images/r03c07f01p01-ch1sk1fk1fl1.tiff", 16 * 1024)]),  # CC-BY-4.0
    ("whole_hcs_cellvoyager", [("hcs/cellvoyager-jump-1053601756/MeasurementData.mlf", 6 * 1024), ("hcs/cellvoyager-jump-1053601756/MeasurementDetail.mrf", 4096), ("hcs/cellvoyager-jump-1053601756/CellPainting_20x_2bin_6FoV.mes", 8 * 1024), ("hcs/cellvoyager-jump-1053601756/1053601756_A01_T0001F001L01A01Z01C01.tif", 16 * 1024)]),  # CC0-1.0
    ("whole_hcs_imagexpress", [("hcs/imagexpress-idr0081/BSF018292-1A.HTD", 4096), ("hcs/imagexpress-idr0081/BSF018292-1A_A01_w1.TIF", 16 * 1024)]),  # CC-BY-4.0
    ("whole_waters", [(WATERS + f, cut) for f, cut in [("_HEADER.TXT", 4096), ("_extern.inf", 8192), ("_FUNCTNS.INF", 4096), ("_FUNC001.IDX", 4096), ("_FUNC001.DAT", 4096), ("_CHROMS.INF", 1024), ("_CHRO001.DAT", 4096)]]),  # CC0-1.0
]
LOCAL_CUTS = [
    ("whole_unicorn_res", "unicorn-res-pycorn-sample1.res", 64 * 1024),  # GPL-2.0
    ("whole_unicorn_res", "unicorn-res-unicornr-sample.res", 64 * 1024),  # GPL-3.0
    ("whole_neuralynx", "nlx-bml-unfilledsplit.ncs", 64 * 1024),
    ("whole_neuralynx", "nlx-cheetah-v5-4-0-csc5-trunc.ncs", 64 * 1024),
    ("whole_blackrock", "brk-2-1-l101210-001.ns2", 64 * 1024),
    ("whole_blackrock", "brk-ptp-20231027-125608-001.ns2", 64 * 1024),
    ("whole_intan", "intan-rhd-test-1.rhd", 64 * 1024),
    ("whole_intan", "intan-rhs-test-1.rhs", 64 * 1024),
    ("whole_plexon", "plexon-file-plexon-1.plx", 64 * 1024),
    ("whole_pl2", "plexon-nc16fpspkevt-1m.pl2", 64 * 1024),
    ("whole_spike2", "spike2-file-spike2-2.smr", 64 * 1024),
]
LOCAL_BUNDLES = [
    ("whole_spikeglx", [("sglx-np2-with-sync.imec0.ap.meta", 32 * 1024), ("sglx-np2-with-sync.imec0.ap.bin", 16 * 1024)]),
    ("whole_spikeglx", [("sglx-np2-with-sync.nidq.meta", 32 * 1024), ("sglx-np2-with-sync.nidq.bin", 4000)]),
]
def mini_heka() -> bytes:
    """A minimal PatchMaster bundle written from docs/formats/heka-patchmaster.md: a DAT2 header,
    eight int16 samples and a pulsed tree with one group, series, sweep and trace."""
    sizes = [640, 144, 1408, 288, 512]
    samples = struct.pack("<8h", *range(-4, 4))
    dat_at = 256
    pul_at = dat_at + len(samples)
    rec = [bytearray(s) for s in sizes]
    rec[0][520:528] = struct.pack("<d", 5190109783.0)
    rec[1][4:6] = b"E1"
    rec[2][4:6] = b"S1"
    rec[3][48:56] = struct.pack("<d", 5190109790.0)
    tr = rec[4]
    tr[4:10] = b"Imon-1"
    tr[40:48] = struct.pack("<ii", dat_at, 8)
    tr[64:66] = struct.pack("<H", 1)
    tr[68], tr[70] = 3, 0
    tr[72:80] = struct.pack("<d", 1e-12)
    tr[96:97] = b"A"
    tr[104:112] = struct.pack("<d", 2e-5)
    tr[120:121] = b"s"
    tree = b"eerT" + struct.pack("<i5i", 5, *sizes)
    for r, n in zip(rec, [1, 1, 1, 1, 0]):
        tree += bytes(r) + struct.pack("<i", n)
    head = bytearray(256)
    head[0:4] = b"DAT2"
    head[8:28] = b"v2x90.2, 22-Nov-2016"
    head[48:53] = struct.pack("<ib", 2, 1)
    head[64:80] = struct.pack("<ii", dat_at, len(samples)) + b".dat".ljust(8, b"\0")
    head[80:96] = struct.pack("<ii", pul_at, len(tree)) + b".pul".ljust(8, b"\0")
    return bytes(head) + samples + tree


MINI_JCAMP = b"""##TITLE=fuzz seed
##JCAMP-DX=4.24
##DATA TYPE=INFRARED SPECTRUM
##XUNITS=1/CM
##YUNITS=ABSORBANCE
##XFACTOR=1.0
##YFACTOR=0.001
##FIRSTX=400
##LASTX=410
##NPOINTS=11
##FIRSTY=100
##XYDATA=(X++(Y..Y))
400 100 200 300 400 500 600
406 700 800 900 1000 1100
##END=
"""


def put(target: str, name: str, data: bytes) -> None:
    d = OUT / target
    d.mkdir(parents=True, exist_ok=True)
    (d / name).write_bytes(data)


def corpus(name: str) -> bytes | None:
    p = CORPUS / name
    return p.read_bytes() if p.exists() else None


# ---------------------------------------------------------------- synthetic CZI


def czi_segment(sid: bytes, payload: bytes, alloc: int | None = None) -> bytes:
    alloc = len(payload) if alloc is None else alloc
    return sid.ljust(16, b"\0") + struct.pack("<QQ", alloc, len(payload)) + payload.ljust(alloc, b"\0")


def czi_dir_entry(pixel_type: int, pos: int, compression: int, dims: list[tuple[str, int, int, int]]) -> bytes:
    e = b"DV" + struct.pack("<iqii", pixel_type, pos, 0, compression) + b"\0" * 6 + struct.pack("<i", len(dims))
    assert len(e) == 32
    for d, start, size, stored in dims:
        e += d.encode().ljust(4, b"\0") + struct.pack("<iifi", start, size, 0.0, stored)
    return e


def mini_czi(compression: int = 0, payload: bytes | None = None, w: int = 8, h: int = 8) -> bytes:
    xml = (
        '<ImageDocument><Metadata><Information><Image><SizeX>%d</SizeX><SizeY>%d</SizeY>'
        "<PixelType>Gray8</PixelType><Dimensions><Channels><Channel Id=\"Channel:0\" Name=\"c0\">"
        "<Color>#FFFF0000</Color></Channel></Channels></Dimensions></Image></Information>"
        "<Scaling><Items><Distance Id=\"X\"><Value>1e-7</Value></Distance></Items></Scaling>"
        "</Metadata></ImageDocument>" % (w, h)
    ).encode()
    pixels = payload if payload is not None else bytes((x * 7 + y * 3) & 0xFF for y in range(h) for x in range(w))
    dims = [("X", 0, w, w), ("Y", 0, h, h), ("C", 0, 1, 1)]
    file_hdr_len = 32 + 512
    meta = struct.pack("<ii", len(xml), 0).ljust(256, b"\0") + xml
    meta_off = file_hdr_len
    sb_off = meta_off + 32 + len(meta)
    entry = czi_dir_entry(0, sb_off, compression, dims)
    sb = (struct.pack("<iiq", 0, 0, len(pixels)) + entry).ljust(256, b"\0") + pixels
    dir_off = sb_off + 32 + len(sb)
    directory = struct.pack("<i", 1).ljust(128, b"\0") + czi_dir_entry(0, sb_off, compression, dims)
    att_off = dir_off + 32 + len(directory)
    attdir = struct.pack("<i", 0).ljust(256, b"\0")
    guid = b"\x11" * 16
    fh = struct.pack("<ii", 1, 0) + b"\0" * 8 + guid + guid + struct.pack("<iqqiq", 0, dir_off, meta_off, 0, att_off)
    out = czi_segment(b"ZISRAWFILE", fh.ljust(512, b"\0"))
    out += czi_segment(b"ZISRAWMETADATA", meta)
    out += czi_segment(b"ZISRAWSUBBLOCK", sb)
    out += czi_segment(b"ZISRAWDIRECTORY", directory)
    out += czi_segment(b"ZISRAWATTDIR", attdir)
    return out


# ---------------------------------------------------------------- synthetic ND2 (LV metadata)


def lv_name(n: str) -> bytes:
    s = (n + "\0").encode("utf-16-le")
    return bytes([len(s) // 2]) + s


def lv_u32(n: str, v: int) -> bytes:
    return bytes([3]) + lv_name(n) + struct.pack("<I", v)


def lv_i32(n: str, v: int) -> bytes:
    return bytes([2]) + lv_name(n) + struct.pack("<i", v)


def lv_f64(n: str, v: float) -> bytes:
    return bytes([6]) + lv_name(n) + struct.pack("<d", v)


def lv_str(n: str, v: str) -> bytes:
    return bytes([8]) + lv_name(n) + (v + "\0").encode("utf-16-le")


def lv_level(n: str, children: list[bytes]) -> bytes:
    body = b"".join(children)
    return bytes([11]) + lv_name(n) + struct.pack("<IQ", len(children), len(body)) + body + b"\0" * (8 * len(children))


def nd2_chunk(name: str, data: bytes) -> bytes:
    nb = name.encode()
    return struct.pack("<IIQ", 0x0ABECEDA, len(nb), len(data)) + nb + data


def mini_nd2(w: int = 8, h: int = 8, frames: int = 2, compressed: bool = False) -> bytes:
    attrs = lv_level(
        "SLxImageAttributes",
        [
            lv_u32("uiWidth", w),
            lv_u32("uiHeight", h),
            lv_u32("uiWidthBytes", w * 2),
            lv_u32("uiComp", 1),
            lv_u32("uiBpcInMemory", 16),
            lv_u32("uiBpcSignificant", 12),
            lv_u32("uiSequenceCount", frames),
            lv_i32("ePixelType", 1),
        ]
        + ([lv_i32("eCompression", 0)] if compressed else []),
    )
    loop = lv_level(
        "SLxExperiment",
        [
            lv_i32("eType", 1),
            lv_level("uLoopPars", [lv_u32("uiCount", frames), lv_f64("dPeriod", 100.0)]),
        ],
    )
    seq = lv_level(
        "SLxPictureMetadata",
        [
            lv_f64("dTimeMSec", 1.5),
            lv_f64("dCalibration", 0.25),
            lv_str("wsObjectiveName", "Plan Fluor 10x"),
            lv_level(
                "sPicturePlanes",
                [lv_level("sPlaneNew", [lv_level("a0", [lv_str("sDescription", "DAPI"), lv_u32("uiCompCount", 1)])])],
            ),
        ],
    )
    chunks = [
        ("ND2 FILE SIGNATURE CHUNK NAME01!", b"Ver3.0".ljust(64, b"\0")),
        ("ImageAttributesLV!", attrs),
        ("ImageMetadataLV!", loop),
        ("ImageMetadataSeqLV|0!", seq),
    ]
    for i in range(frames):
        px = struct.pack("<d", float(i)) + b"".join(struct.pack("<H", (x * y + i) & 0xFFF) for y in range(h) for x in range(w))
        if compressed:
            px = px[:8] + zlib.compress(px[8:])
        chunks.append((f"ImageDataSeq|{i}!", px))
    out = b""
    entries = []
    for name, data in chunks:
        entries.append((name, len(out), len(data)))
        out += nd2_chunk(name, data)
    cm = b""
    for name, off, ln in entries:
        cm += name.encode() + struct.pack("<QQ", off, ln)
    cm += b"ND2 CHUNK MAP SIGNATURE 0000001!" + struct.pack("<QQ", 0, 0)
    cm_off = len(out)
    out += nd2_chunk("ND2 FILEMAP SIGNATURE NAME 0001!", cm)
    out += b"ND2 CHUNK MAP SIGNATURE 0000001!" + struct.pack("<Q", cm_off)
    return out


# ---------------------------------------------------------------- synthetic LIF


def mini_lif(w: int = 8, h: int = 6, z: int = 2, version: int = 2) -> bytes:
    bps = 1
    xml = (
        '<LMSDataContainerHeader Version="%d"><Element Name="proj"><Data><Experiment/></Data>'
        "<Children><Element Name=\"img\" UniqueID=\"u1\"><Data><Image><ImageDescription><Channels>"
        '<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" BytesInc="0" LUTName="Green" Min="0" Max="255"/>'
        "</Channels><Dimensions>"
        '<DimensionDescription DimID="1" NumberOfElements="%d" Origin="0" Length="1e-6" Unit="m" BytesInc="%d"/>'
        '<DimensionDescription DimID="2" NumberOfElements="%d" Origin="0" Length="1e-6" Unit="m" BytesInc="%d"/>'
        '<DimensionDescription DimID="3" NumberOfElements="%d" Origin="0" Length="2e-6" Unit="m" BytesInc="%d"/>'
        "</Dimensions></ImageDescription><TimeStampList>1D5B1C2A3B4C5D6E</TimeStampList></Image></Data>"
        '<Memory Size="%d" MemoryBlockID="MemBlock_1"/><Children/></Element></Children></Element>'
        "</LMSDataContainerHeader>"
        % (version, w, bps, h, w * bps, z, w * h * bps, w * h * z * bps)
    )
    xb = xml.encode("utf-16-le")
    out = struct.pack("<II", 0x70, len(xb) + 5) + b"\x2a" + struct.pack("<I", len(xb) // 2) + xb
    pixels = bytes((i * 5) & 0xFF for i in range(w * h * z))
    bid = "MemBlock_1".encode("utf-16-le")
    if version >= 2:
        hdr = struct.pack("<II", 0x70, len(bid) + 14) + b"\x2a" + struct.pack("<Q", len(pixels)) + b"\x2a" + struct.pack("<I", len(bid) // 2)
    else:
        hdr = struct.pack("<II", 0x70, len(bid) + 10) + b"\x2a" + struct.pack("<I", len(pixels)) + b"\x2a" + struct.pack("<I", len(bid) // 2)
    return out + hdr + bid + pixels


# ---------------------------------------------------------------- structure extraction


def czi_structures(data: bytes):
    """Yield (kind, bytes) for subblock payload headers, directory bytes and the XML."""
    off = 0
    while off + 32 <= len(data):
        sid = data[off : off + 16].split(b"\0")[0]
        alloc, used = struct.unpack_from("<QQ", data, off + 16)
        if sid == b"ZISRAWSUBBLOCK":
            yield "subblock", data[off + 32 : off + 32 + 256]
            meta_size, att_size, data_size = struct.unpack_from("<iiq", data, off + 32)
            ndims = struct.unpack_from("<i", data, off + 32 + 16 + 28)[0]
            hdr = max(256, 16 + 32 + 20 * ndims)
            comp = struct.unpack_from("<i", data, off + 32 + 16 + 18)[0]
            start = off + 32 + hdr + meta_size
            yield f"payload{comp}", data[start : start + data_size]
        elif sid == b"ZISRAWDIRECTORY":
            yield "directory", data[off + 32 + 128 : off + 32 + used]
        elif sid == b"ZISRAWATTDIR":
            yield "attdir", data[off + 32 + 256 : off + 32 + used]
        elif sid == b"ZISRAWMETADATA":
            xml_size = struct.unpack_from("<i", data, off + 32)[0]
            yield "xml", data[off + 32 + 256 : off + 32 + 256 + xml_size]
        if alloc == 0 or sid == b"":
            break
        off += 32 + alloc


def nd2_chunks(data: bytes):
    cm_off = struct.unpack_from("<Q", data, len(data) - 8)[0]
    magic, nlen, dlen = struct.unpack_from("<IIQ", data, cm_off)
    payload = data[cm_off + 16 + nlen : cm_off + 16 + nlen + dlen]
    yield "map", "chunkmap", payload
    i = 0
    while i < len(payload):
        bang = payload.find(b"!", i)
        if bang < 0 or bang + 17 > len(payload):
            break
        name = payload[i : bang + 1].decode("latin-1")
        coff, clen = struct.unpack_from("<QQ", payload, bang + 1)
        i = bang + 17
        if name.startswith("ND2 CHUNK MAP"):
            break
        m, nl, dl = struct.unpack_from("<IIQ", data, coff)
        yield "chunk", name, data[coff + 16 + nl : coff + 16 + nl + dl]


def lif_xml(data: bytes) -> bytes:
    xml_len = struct.unpack_from("<I", data, 9)[0]
    return data[13 : 13 + 2 * xml_len].decode("utf-16-le").encode("utf-8")


# ---------------------------------------------------------------- main


def safe(name: str) -> str:
    return "".join(c if c.isalnum() or c in "-_." else "_" for c in name)


def _unicorn_desc(flags: int, name: bytes, unit: bytes, storage: int, factor: float, second: float = 0.0) -> bytes:
    return struct.pack("<HH", 78, flags) + name.ljust(40, b"\0") + unit.ljust(16, b"\0") + struct.pack("<Hdd", storage, factor, second)


def mini_unicorn_res() -> bytes:
    """A UNICORN 3-5 `.res` with one curve and one event list (docs/formats/cytiva-unicorn.md)."""
    curve = struct.pack("<3H", 6, 2, 1) + _unicorn_desc(0x8001, b"Acc. Time", b"min", 0x0104, 0.5) \
        + _unicorn_desc(0x8002, b"Acc. Volume", b"ml", 0x0104, 0.01) + _unicorn_desc(0x4001, b"0", b" mAU", 0x0104, 0.001) \
        + b"".join(struct.pack("<ii", 50 * i, 1000 * i) for i in range(8))
    events = struct.pack("<3H", 6, 6, 1) + _unicorn_desc(0x8001, b"", b"", 0x0104, 1.0) \
        + _unicorn_desc(0x8001, b"Time", b"min", 0x0308, 1.0) + _unicorn_desc(0x8002, b"Volume", b"ml", 0x0308, 1.0) \
        + _unicorn_desc(0x4000, b"TubeNo", b"", 0x044c, 1.0) + _unicorn_desc(0x4000, b"Add. Text", b"", 0x044c, 1.0) \
        + _unicorn_desc(0x4000, b"Value", b"", 0x0308, 1.0) + _unicorn_desc(0x4000, b"Flags", b"", 0x0104, 1.0) \
        + struct.pack("<dd", 1.0, 1.0) + b"A1".ljust(76, b"\0") + b"".ljust(76, b"\0") + struct.pack("<di", 0.0, 1)
    blocks = [(b"run:1_UV", bytes([1, 0, 4, 0, 1, 0x14]), 240, curve), (b"run:1_Fractions", bytes([1, 0, 4, 0, 0x44, 4]), 552, events)]
    d = 0x2b0
    f = bytearray(d + 344 * (len(blocks) + 1))
    f[0:4] = b"\x11\x47\x11\x47"
    struct.pack_into("<II", f, 4, 0x18, d)
    f[0x18:0x24] = b"UNICORN 3.10"
    for i, (name, kind, hdr, data) in enumerate(blocks):
        at = len(f)
        f += data
        e = d + 344 * i
        f[e:e + 6] = kind
        f[e + 6:e + 6 + len(name)] = name
        struct.pack_into("<4I", f, e + 302, len(data), len(data), at, hdr)
    struct.pack_into("<I", f, 16, len(f))
    return bytes(f)


def mini_unicorn_zip() -> bytes:
    """A UNICORN 7 result export with one curve (MS-NRBF float arrays in a padded nested zip)."""
    import io
    import zipfile

    head = bytes([0, 1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0, 0, 0, 0])

    def floats(v):
        return head + bytes([15]) + struct.pack("<ii", 1, len(v)) + bytes([11]) + struct.pack(f"<{len(v)}f", *v) + bytes([11])

    def put_member(z, name, data):  # fixed timestamps: the seed is byte-for-byte reproducible
        z.writestr(zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0)), data, zipfile.ZIP_DEFLATED)

    inner = io.BytesIO()
    with zipfile.ZipFile(inner, "w", zipfile.ZIP_DEFLATED) as z:
        put_member(z, "CoordinateData.AmplitudesDataType", b"System.Single[]\r\n")
        put_member(z, "CoordinateData.Amplitudes", floats([1.0, 2.0, 5.0, 2.0]))
        put_member(z, "CoordinateData.VolumesDataType", b"System.Single[]\r\n")
        put_member(z, "CoordinateData.Volumes", floats([0.0, 0.1, 0.2, 0.3]))
    curve = inner.getvalue() + bytes(512)
    chrom = (b'<Chromatogram FormatVersion="9"><Curves><Curve CurveDataType="UV"><Name>UV 1_280</Name>'
             b"<IsoChroneType>Time</IsoChroneType><DistanceBetweenPoints>0.1</DistanceBetweenPoints><DistanceToStartPoint>0.1</DistanceToStartPoint>"
             b"<AmplitudeUnit>mAU</AmplitudeUnit><CurveNumber>1</CurveNumber><CurvePoints><CurvePoint><IsFullResolution>true</IsFullResolution>"
             b"<BinaryCurvePointsFileName>Chrom.1_1_True</BinaryCurvePointsFileName></CurvePoint></CurvePoints></Curve></Curves>"
             b'<EventCurves><EventCurve EventCurveType="Fraction"><Events><Event><EventTime>0.2</EventTime><EventVolume>0.1</EventVolume>'
             b"<EventText>1</EventText></Event></Events></EventCurve></EventCurves></Chromatogram>")
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        put_member(z, "Result.xml", b'<Result UNICORNVersion="7.3.0.473"><Name>seed</Name></Result>')
        put_member(z, "Chrom.1.Xml", chrom)
        put_member(z, "Chrom.1_1_True", curve)
    return out.getvalue()


def new_format_seeds() -> None:
    """Structure-level seeds for the sub-parser targets of the newer readers."""
    # A gel of four lanes with two bands each: width byte (+8), options byte, 8-bit rows
    # (fuzz_targets/measure_lanes.rs).
    import math

    w, h = 56, 64
    gel = bytearray()
    for y in range(h):
        for x in range(w):
            v = 20.0
            for cx in (8, 22, 36, 50):
                if abs(x - cx) <= 4:
                    v += 180 * math.exp(-((y - 16) ** 2) / 8) + 90 * math.exp(-((y - 40) ** 2) / 8)
            gel.append(min(255, int(v)))
    put("measure_lanes", "synthetic-4-lanes.bin", bytes([w - 8, 0]) + bytes(gel))
    # An Incucyte metric export, wells as columns (a decimal comma and an ERROR cell).
    put("whole_plate", "synthetic-incucyte-wells.txt", (
        "Vessel Name: synthetic\nMetric: Phase Object Confluence (Percent)\nCell Type: none\n"
        "Passage: 1\nNotes: \nAnalysis: synthetic seed\n\nDate Time\tElapsed\tA1\tA2\tB1\n"
        "01/02/2026 08:00:00\t0\t10.5\t11.2\t9.8\n01/02/2026 12:00:00\t4\t15.1\t16,0\t12.2\n"
        "01/02/2026 16:00:00\t8\t22.4\t23.9\tERROR\n").encode())
    put("whole_unicorn_res", "mini.res", mini_unicorn_res())
    put("whole_unicorn_zip", "mini-export.zip", mini_unicorn_zip())
    # ASDF: SQZ, DIF and DUP forms of one short table (JCAMP-DX 4.24 compression rules).
    put("jcamp_asdf", "sqz-dif-dup.txt", b"400A00J0S3j0\n405B00J1j1T\n410%\n")
    # timsTOF frame blobs: `u32` byte count and `u32` scan count, then zstd (see bruker-tdf.md).
    tdf_bin = corpus("timsrust-test-dda.d/analysis.tdf_bin")
    if tdf_bin is not None and len(tdf_bin) >= 8:
        total, scans = struct.unpack_from("<II", tdf_bin)
        put("tims_frame", "tdf-frame0.bin", struct.pack("<I", scans) + b"\0" + tdf_bin[8:total])
        put("tims_frame", "empty-frame.bin", struct.pack("<I", scans) + b"\0")
    tsf = corpus("timsrust-test-dia.d/analysis.tdf_bin")
    if tsf is not None and len(tsf) >= 8:
        total, scans = struct.unpack_from("<II", tsf)
        put("tims_frame", "tdf-dia-frame0.bin", struct.pack("<I", scans) + b"\0" + tsf[8:min(total, 16384)])
    # Sciex grid scan: the first enhanced MS scan of msv97113-cm-5-pos-2 (MassIVE MSV000097113, CC0),
    # whose first index record points at the start of the .wiff.scan's data.
    scan = corpus("msv97113-cm-5-pos-2.wiff.scan")
    if scan is not None:
        put("sciex_grid", "ems-scan1.bin", scan[0x2C:0x2C + 2740])
    # Waters drift index + the first scan's .cdt bytes (ProteoWizard test data, Apache-2.0).
    ind = corpus("pwiz-waters/HDMRM_Short_noLM.raw/_func001.ind")
    cdt = corpus("pwiz-waters/HDMRM_Short_noLM.raw/_func001.cdt")
    if ind is not None and cdt is not None:
        first = 36 + 12 + 20 * 200
        put("waters_drift", "hdmrm-scan1.bin", struct.pack("<I", first) + ind[:first] + cdt[:12000])
    # OME-Zarr v2: one 1x1x1x8x8 uint8 array, uncompressed, as a zip store and as a directory
    # (whole_zarr's parts: .zgroup, .zattrs, 0/.zarray, 0/0.0.0.0.0, then v3 files left empty).
    import io
    import json
    import zipfile

    axes = [{"name": a, "type": t} for a, t in zip("tczyx", ["time", "channel", "space", "space", "space"])]
    scale = [{"type": "scale", "scale": [1, 1, 1, 0.5, 0.5]}]
    zattrs = json.dumps({"multiscales": [{"version": "0.4", "axes": axes, "datasets": [{"path": "0", "coordinateTransformations": scale}]}]}).encode()
    zarray = json.dumps({"zarr_format": 2, "shape": [1, 1, 1, 8, 8], "chunks": [1, 1, 1, 8, 8], "dtype": "|u1", "compressor": None, "fill_value": 0, "order": "C", "filters": None, "dimension_separator": "."}).encode()
    zgroup = b'{"zarr_format": 2}'
    chunk = bytes(range(64))
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_STORED) as z:
        for n, d in [(".zgroup", zgroup), (".zattrs", zattrs), ("0/.zarray", zarray), ("0/0.0.0.0.0", chunk)]:
            z.writestr(n, d)
    put("whole_zarr_zip", "mini-v2.ome.zarr.zip", buf.getvalue())
    put("whole_zarr", "mini-v2.bundle", BUNDLE_SEP.join([zgroup, zattrs, zarray, chunk]))

    # Binary arrays: selector byte (see fuzz_targets/mzml_binary.rs), then base64.
    import base64

    values = [100.0 + 0.25 * i for i in range(16)]
    raw64 = struct.pack("<16d", *values)
    raw32 = struct.pack("<16f", *values)
    put("mzml_binary", "f64-plain.txt", bytes([1]) + base64.b64encode(raw64))
    put("mzml_binary", "f32-plain.txt", bytes([0]) + base64.b64encode(raw32))
    put("mzml_binary", "f64-zlib.txt", bytes([1 | 1 << 2]) + base64.b64encode(zlib.compress(raw64)))
    put("mzml_binary", "f64-big-endian.txt", bytes([1 | 0x80]) + base64.b64encode(struct.pack(">16d", *values)))
    try:
        import numpy as np
        import pynumpress  # BSD-3-Clause; used as an encoder only

        arr = np.array(values, dtype=np.float64)
        lin = bytes(pynumpress.encode_linear(arr, pynumpress.optimal_linear_fixed_point(arr)))
        pic = bytes(pynumpress.encode_pic(arr))
        slof = bytes(pynumpress.encode_slof(arr, pynumpress.optimal_slof_fixed_point(arr)))
        put("mzml_binary", "numpress-linear.txt", bytes([1 | 3 << 2]) + base64.b64encode(lin))
        put("mzml_binary", "numpress-pic.txt", bytes([1 | 4 << 2]) + base64.b64encode(pic))
        put("mzml_binary", "numpress-slof.txt", bytes([1 | 5 << 2]) + base64.b64encode(slof))
        put("mzml_binary", "numpress-linear-zlib.txt", bytes([1 | 3 << 2 | 1 << 5]) + base64.b64encode(zlib.compress(lin)))
    except ImportError:
        print("pynumpress not installed: MS-Numpress seeds skipped", file=sys.stderr)


VMS_NAMES = ["s.vms", "s.jpg", "s(1,0).jpg", "s(0,1).jpg", "s(1,1).jpg", "s_map2.jpg", "s.opt", "s_macro.jpg"]


def synthetic_fixture_seeds() -> None:
    """Seeds from the committed synthetic fixtures (our own writers: oracle/make_isyntax_fixtures.py,
    oracle/make_vms_fixtures.py, oracle/make_baf_fixtures.py)."""
    fx = ROOT.parent / "crates" / "openreadout-wsi" / "tests" / "fixtures"
    put("whole_isyntax", "synthetic.isyntax", (fx / "isyntax-synthetic.isyntax").read_bytes())
    d = fx / "vms-synthetic"
    vms = (d / "synthetic.vms").read_text().replace("synthetic", "s").replace("Reference=s", "Reference=synthetic")
    files = ["synthetic.jpg", "synthetic(1,0).jpg", "synthetic(0,1).jpg", "synthetic(1,1).jpg", "synthetic_map2.jpg",
             "synthetic.opt", "synthetic_macro.jpg"]
    put("whole_vms", "synthetic.bundle", BUNDLE_SEP.join([vms.encode()] + [(d / f).read_bytes() for f in files]))
    b = ROOT.parent / "crates" / "openreadout-bruker-tims" / "tests" / "fixtures" / "baf-synthetic.d"
    put("whole_baf", "synthetic.bundle", BUNDLE_SEP.join([(b / "analysis.baf").read_bytes(), (b / "Calibrator.ami").read_bytes()]))


def structural_seeds() -> None:
    """Diffraction and cryo-EM seeds: CBF and SMV images written here (byte-offset coded
    counts with a PILATUS header; an ADSC header), a SerialEM mdoc cut from EMPIAR-10751 (CC0)
    and a bitshuffle/LZ4 chunk made with hdf5plugin when it is installed."""
    def byte_offset(vals):
        out, prev = bytearray(), 0
        for v in vals:
            d = v - prev
            if -127 <= d <= 127:
                out += struct.pack("<b", d)
            elif -32767 <= d <= 32767:
                out += b"\x80" + struct.pack("<h", d)
            else:
                out += b"\x80" + struct.pack("<h", -32768) + struct.pack("<i", d)
            prev = v
        return bytes(out)
    vals = [((i * 37) % 11) - 1 if i % 9 else 900 for i in range(64)]
    coded = byte_offset(vals)
    head = ("###CBF: VERSION 1.5\r\ndata_seed\r\n_array_data.header_convention \"PILATUS_1.2\"\r\n"
            "_array_data.header_contents\r\n;\r\n# Detector: PILATUS 2M, S/N 1\r\n# Pixel_size 172e-6 m x 172e-6 m\r\n"
            "# Wavelength 1.0 A\r\n# Detector_distance 0.2 m\r\n# Beam_xy (4.0, 4.0) pixels\r\n# Count_cutoff 800 counts\r\n;\r\n"
            "_array_data.data\r\n;\r\n--CIF-BINARY-FORMAT-SECTION--\r\nContent-Type: application/octet-stream;\r\n"
            "     conversions=\"x-CBF_BYTE_OFFSET\"\r\n"
            f"X-Binary-Size: {len(coded)}\r\nX-Binary-Element-Type: \"signed 32-bit integer\"\r\n"
            "X-Binary-Number-of-Elements: 64\r\nX-Binary-Size-Fastest-Dimension: 8\r\nX-Binary-Size-Second-Dimension: 8\r\n\r\n").encode()
    put("whole_cbf", "pilatus-8x8.cbf", head + b"\x0c\x1a\x04\xd5" + coded + b"\r\n--CIF-BINARY-FORMAT-SECTION----\r\n;\r\n")
    smv = (b"{\nHEADER_BYTES=  512;\nDIM=2;\nBYTE_ORDER=little_endian;\nTYPE=unsigned_short;\nSIZE1=8;\nSIZE2=8;\n"
           b"PIXEL_SIZE=0.1;\nDISTANCE=100;\nWAVELENGTH=1.0;\nBEAM_CENTER_X=0.4;\nBEAM_CENTER_Y=0.4;\n}\n").ljust(512, b" ")
    put("whole_smv", "adsc-8x8.img", smv + struct.pack("<64H", *range(64)))
    md = corpus("empiar10751-G3_TS_001.mrc.mdoc")  # CC0 (EMPIAR)
    if md:
        put("whole_mdoc", "tilt-series.mdoc", md[:4096])
    put("whole_mdoc", "frameset.mdoc", b"T = SerialEM: Digitized on a Krios\nVoltage = 300\n\n[FrameSet = 0]\nTiltAngle = 0.5\n"
        b"ExposureDose = 2.5\nSubFramePath = W:\\frames\\stack_0000.tif\nDateTime = 15-Jul-19  13:54:01\n")
    put("nxmx_links", "link.bin", bytes(5) + b"data_000001" + struct.pack("<H", 27) + b"\x00Therm_6_1_000001.h5\x00/data\x00")
    try:
        import io
        import h5py
        import hdf5plugin
        import numpy as np
        buf = io.BytesIO()
        with h5py.File(buf, "w") as f:
            d = f.create_dataset("x", data=(np.arange(1000) % 251).astype("<u2"), chunks=(1000,), **hdf5plugin.Bitshuffle())
            chunk = d.id.read_direct_chunk((0,))[1]
        put("codec_bitshuffle", "u16-lz4.bin", struct.pack("<I", 2000)[:3] + bytes([1]) + chunk)
    except ImportError:
        pass


def ephys_new_seeds() -> None:
    """TDT, Axona and MC_DataTool seeds: the committed synthetic TDT block
    (oracle/make_tdt_fixtures.py) and minimal sessions written here (the corpus files of these
    formats are share-alike and seed local-seeds/ only)."""
    fx = ROOT.parent / "crates" / "openreadout-ephys" / "tests" / "fixtures" / "tdt-synthetic"
    put("whole_tdt", "synthetic.bundle",
        BUNDLE_SEP.join([(fx / "synthetic_B1.tsq").read_bytes(), (fx / "synthetic_B1.tev").read_bytes()]))
    setf = ("trial_date Sunday, 4 Oct 2020\r\ntrial_time 11:07:07\r\nsw_version 1.2.2.16\r\nADC_fullscale_mv 1500\r\n"
            + "".join(f"gain_ch_{i} 10000\r\n" for i in range(64))
            + "".join(f"collectMask_{t} {int(t == 1)}\r\n" for t in range(1, 17)) + "rawRate 48000\r\n").encode()
    packets = b"".join(b"ADU1" + struct.pack("<I", p) + bytes(24) + struct.pack("<192h", *range(p, p + 192)) + bytes(16)
                       for p in range(2))
    spikes = (b"trial_date Sunday, 4 Oct 2020\r\nnum_chans 4\r\ntimebase 96000 hz\r\nbytes_per_timestamp 4\r\n"
              b"samples_per_spike 50\r\nbytes_per_sample 1\r\nnum_spikes 1\r\ndata_start"
              + (struct.pack(">i", 960) + bytes(range(50))) * 4 + b"\r\ndata_end\r\n")
    put("whole_axona", "mini.bundle", BUNDLE_SEP.join([setf, packets, spikes]))
    put("whole_mcs_raw", "mini.raw",
        b"MC_DataTool binary conversion\r\nVersion 2.6.10\r\nSample rate = 20000\r\nADC zero = 32768\r\n"
        b"El = 0.2083\xb5V/AD\r\nStreams = El_12;El_13\r\nEOH\r\n" + struct.pack("<8H", *range(32760, 32768)))
    put("whole_edf", "mini-edfplus.edf", _mini_edf(False))
    put("whole_edf", "mini-bdf.bdf", _mini_edf(True))
    vhdr = (b"Brain Vision Data Exchange Header File Version 1.0\r\n; Data created by the Vision Recorder\r\n"
            b"[Common Infos]\r\nDataFile=r.eeg\r\nMarkerFile=r.vmrk\r\nDataFormat=BINARY\r\n"
            b"DataOrientation=MULTIPLEXED\r\nNumberOfChannels=2\r\nSamplingInterval=2000\r\n"
            b"SegmentationType=MARKERBASED\r\nSegmentDataPoints=2\r\n"
            b"[Binary Infos]\r\nBinaryFormat=INT_16\r\n[Channel Infos]\r\nCh1=Fp1,,0.5,\xc2\xb5V\r\nCh2=A\\1B,REF,2\r\n"
            b"[Coordinates]\r\nCh1=1,-90,-72\r\n[Comment]\r\nsettings\r\n")
    vmrk = (b"Brain Vision Data Exchange Marker File, Version 1.0\r\n[Marker Infos]\r\n"
            b"Mk1=New Segment,,1,1,0,20200124040556123456\r\nMk2=Stimulus,S  1,3,1,0\r\n")
    put("whole_brainvision", "mini.bundle", BUNDLE_SEP.join([vhdr, vmrk, struct.pack("<8h", 1, -1, 2, -2, 3, -3, 4, -4)]))
    put("whole_eeglab", "mini.bundle", BUNDLE_SEP.join([_mini_eeglab_set(), struct.pack("<8f", *range(8))]))



def _mini_eeglab_set() -> bytes:
    """A MAT 5.0 file with an EEG struct (2 channels, 4 points, chanlocs, one event) whose data
    names r.fdt; one field compressed, as MATLAB's -v7 writes."""
    def pad(b: bytes) -> bytes:
        return b + bytes(-len(b) % 8)

    def el(ty: int, data: bytes) -> bytes:
        return pad(struct.pack("<II", ty, len(data)) + data)

    def mat(cls: int, dims, name: str, body: bytes) -> bytes:
        b = el(6, struct.pack("<II", cls, 0)) + el(5, struct.pack(f"<{len(dims)}i", *dims)) + el(1, name.encode()) + body
        return el(14, b)

    def num(v: float) -> bytes:
        return mat(6, [1, 1], "", el(9, struct.pack("<d", v)))

    def txt(t: str) -> bytes:
        return mat(4, [1, len(t)], "", el(4, t.encode("utf-16-le")))

    def strukt(name: str, fields, rows) -> bytes:
        body = struct.pack("<IIi", 5, 4, 32) + bytes(4)
        body += el(1, b"".join(f.encode().ljust(32, b"\0") for f in fields))
        body += b"".join(b"".join(r) for r in rows)
        return mat(2, [1, len(rows)], name, body)

    eeg = strukt("EEG", ["nbchan", "pnts", "trials", "srate", "xmin", "data", "chanlocs", "event"], [[
        num(2), num(4), num(1), num(250), num(0), txt("r.fdt"),
        strukt("", ["labels"], [[txt("Fz")], [txt("Cz")]]),
        strukt("", ["type", "latency"], [[txt("stim"), num(2)]]),
    ]])
    head = b"MATLAB 5.0 MAT-file, Platform: GLNXA64, Created on: Mon Oct 19 10:00:00 2026".ljust(116, b" ")
    head += bytes(8) + struct.pack("<H", 0x0100) + b"IM"
    z = zlib.compress(eeg)
    return head + struct.pack("<II", 15, len(z)) + z


def _mini_edf(bdf: bool) -> bytes:
    """Two data records of one 4-sample signal and an annotation signal (EDF+C / BDF+C)."""
    def pad(t: str, n: int) -> bytes:
        return t.encode().ljust(n, b" ")
    ann = "BDF Annotations" if bdf else "EDF Annotations"
    sigs = [("EEG Fpz-Cz", "uV", "-500", "500", "-2048", "2047", 4), (ann, "", "-1", "1", "-32768", "32767", 10)]
    h = (b"\xffBIOSEMI" if bdf else pad("0", 8)) + pad("X X X X", 80) + pad("Startdate 02-MAR-2002 X X X", 80)
    h += pad("02.03.02", 8) + pad("11.25.00", 8) + pad(str(256 * 3), 8) + pad("BDF+C" if bdf else "EDF+C", 44)
    h += pad("2", 8) + pad("1", 8) + pad("2", 4)
    for k, w in enumerate([16, 80, 8, 8, 8, 8, 8, 80, 8, 32]):
        for sg in sigs:
            v = {0: sg[0], 2: sg[1], 3: sg[2], 4: sg[3], 5: sg[4], 6: sg[5], 8: str(sg[6])}.get(k, "")
            h += pad(v, w)
    width = 3 if bdf else 2
    out = h
    for r in range(2):
        out += b"".join(int(v).to_bytes(width, "little", signed=True) for v in (r, -r, 100, -100))
        tal = f"+{r}\x14\x14\x00".encode() + (b"+0.5\x151\x14Spike\x14\x00" if r == 0 else b"")
        out += tal.ljust(10 * width, b"\x00")
    return out


def _bgzf(data: bytes) -> bytes:
    """BGZF: gzip members with the `BC` extra subfield (block size - 1), then the empty EOF block."""
    out = b""
    for i in range(0, max(len(data), 1), 60000):
        chunk = data[i:i + 60000]
        c = zlib.compressobj(6, zlib.DEFLATED, -15)
        z = c.compress(chunk) + c.flush()
        block = (b"\x1f\x8b\x08\x04" + bytes(4) + b"\x00\xff" + struct.pack("<H", 6) + b"BC"
                 + struct.pack("<HH", 2, 25 + len(z) + 1) + z
                 + struct.pack("<II", zlib.crc32(chunk), len(chunk)))
        out += block
    eof = bytes.fromhex("1f8b08040000000000ff0600424302001b0003000000000000000000")
    return out + eof


def mini_bam() -> bytes:
    text = b"@HD\tVN:1.6\tSO:unsorted\n@RG\tID:r\tPL:PACBIO\tSM:s\n"
    body = b"BAM\x01" + struct.pack("<i", len(text)) + text + struct.pack("<i", 1)
    body += struct.pack("<i", 4) + b"ref\x00" + struct.pack("<i", 100)
    for k, seq in enumerate([b"ACGTACGTAC", b"GGGCCCAATT"]):
        name = f"m1_1_1/{k}/ccs".encode() + b"\x00"
        packed = bytes(("=ACMGRSVTWYHKDBN".index(chr(seq[i])) << 4) | ("=ACMGRSVTWYHKDBN".index(chr(seq[i + 1])))
                       for i in range(0, len(seq), 2))
        qual = bytes([30 + k] * len(seq))
        aux = b"npC" + bytes([5]) + b"rqf" + struct.pack("<f", 0.999)
        rec = struct.pack("<iiBBHHHiiii", -1 if k else 0, -1 if k else 9, len(name), 255, 4680, 0, 4 if k else 0,
                          len(seq), -1, -1, 0) + name + packed + qual + aux
        body += struct.pack("<i", len(rec)) + rec
    return _bgzf(body)


def mini_run_bcl() -> list[bytes]:
    runinfo = b"""<?xml version="1.0"?>
<RunInfo Version="2"><Run Id="220101_M00001_0001_000000000-AAAAA" Number="1">
<Flowcell>000000000-AAAAA</Flowcell><Instrument>M00001</Instrument><Date>220101</Date>
<Reads><Read Number="1" NumCycles="2" IsIndexedRead="N" /></Reads>
<FlowcellLayout LaneCount="1" SurfaceCount="1" SwathCount="1" TileCount="1" /></Run></RunInfo>
"""
    params = b"<?xml version=\"1.0\"?><RunParameters><Setup><ApplicationName>MiSeq Control Software</ApplicationName><ApplicationVersion>2.6</ApplicationVersion></Setup><RTAVersion>1.18.54</RTAVersion><ExperimentName>seed</ExperimentName></RunParameters>"
    tile = bytes([2, 10]) + b"".join(struct.pack("<HHHf", 1, 1101, code, v)
                                     for code, v in [(100, 1.2e6), (101, 1.1e6), (102, 4.0), (103, 3.0), (200, 0.001), (201, 0.002), (300, 1.5)])
    q = bytes([4, 206]) + b"".join(struct.pack("<HHH", 1, 1101, c) + struct.pack("<50I", *([0] * 29 + [1, 2] + [0] * 19)) for c in (1, 2))
    err = bytes([3, 30]) + b"".join(struct.pack("<HHHfIIIII", 1, 1101, c, 0.5, 1, 0, 0, 0, 0) for c in (1, 2))
    ext = bytes([2, 38]) + b"".join(struct.pack("<HHH4f4HQ", 1, 1101, c, 2.0, 2.0, 2.0, 2.0, 300, 200, 100, 50, 0) for c in (1, 2))
    cor = bytes([2, 48]) + b"".join(struct.pack("<HHH9H5If", 1, 1101, c, *([100] * 9), 0, 1, 1, 1, 1, 5.0) for c in (1, 2))
    etile = bytes([1, 10]) + struct.pack("<HHHf", 1, 1101, 0, 3.0)
    index = bytes([1]) + struct.pack("<HHH", 1, 1101, 1) + struct.pack("<H", 8) + b"ACGTACGT" + struct.pack("<I", 2) \
        + struct.pack("<H", 1) + b"s" + struct.pack("<H", 1) + b"p"
    filt = struct.pack("<III", 0, 3, 4) + bytes([1, 1, 0, 1])
    bcl1 = struct.pack("<I", 4) + bytes([(30 << 2) | 0, (35 << 2) | 1, 0, (20 << 2) | 3])
    bcl2 = struct.pack("<I", 4) + bytes([(31 << 2) | 2, (36 << 2) | 1, (12 << 2) | 0, 0])
    locs = struct.pack("<IfI", 1, 1.0, 4) + struct.pack("<8f", 10.0, 11.0, 20.5, 21.25, 30.0, 31.0, 1418.95, 2.0)
    return [runinfo, params, tile, q, err, ext, cor, etile, index, filt, bcl1, bcl2, locs]


def mini_run_cbcl() -> list[bytes]:
    runinfo = b"""<?xml version="1.0"?>
<RunInfo Version="5"><Run Id="200101_A00001_0001_BAAAAAAAXX" Number="1"><Flowcell>AAAAAAAXX</Flowcell>
<Instrument>A00001</Instrument><Date>1/1/2020 1:00:00 PM</Date><Reads><Read Number="1" NumCycles="2" IsIndexedRead="N"/></Reads>
<FlowcellLayout LaneCount="1" SurfaceCount="2" SwathCount="1" TileCount="1"><TileSet TileNamingConvention="FourDigit"><Tiles><Tile>1_2101</Tile></Tiles></TileSet></FlowcellLayout>
<ImageChannels><Name>RED</Name><Name>GREEN</Name></ImageChannels></Run></RunInfo>
"""
    tile = bytes([3, 15]) + struct.pack("<f", 1.38) + struct.pack("<HIBff", 1, 2101, ord("t"), 4.0, 3.0) \
        + struct.pack("<HIBIf", 1, 2101, ord("r"), 1, 0.0)
    q = bytes([7, 20, 1, 3, 2, 17, 11, 18, 29, 25, 30, 40, 37]) + b"".join(
        struct.pack("<HIH3I", 1, 2101, c, 1, 1, 2) for c in (1, 2))
    filt = struct.pack("<III", 0, 3, 4) + bytes([1, 0, 1, 1])
    cbcls = []
    for nib in (bytes([0x9D, 0x0E]), bytes([0xE1, 0x09])):
        z = zlib.compressobj(6, zlib.DEFLATED, 31)
        block = z.compress(nib) + z.flush()
        header = struct.pack("<HI", 1, 65) + bytes([2, 2]) + struct.pack("<I", 4) \
            + b"".join(struct.pack("<II", a, b) for a, b in [(0, 0), (1, 11), (2, 25), (3, 37)]) \
            + struct.pack("<I", 1) + struct.pack("<IIII", 2101, 4, 2, len(block)) + bytes([0])
        cbcls.append(header + block)
    locs = struct.pack("<IfI", 1, 1.0, 4) + struct.pack("<8f", 0.0, 0.0, 1.8, 0.0, 3.6, 0.0, 5.4, 0.0)
    return [runinfo, tile, q, filt, cbcls[0], cbcls[1], locs]


def sequencing_seeds() -> None:
    """DNA sequencer seeds: synthetic run folders (BCL and CBCL), FASTQ and BAM written here, the
    VBZ FAST5 of ont_fast5_api's test data (MPL-2.0) and, when the `pod5` package is installed, a
    one-read POD5 file it writes."""
    put("whole_illumina_bcl", "mini.bundle", BUNDLE_SEP.join(mini_run_bcl()))
    put("whole_illumina_cbcl", "mini.bundle", BUNDLE_SEP.join(mini_run_cbcl()))
    run = mini_run_bcl()
    for i, name in [(2, "tile-v2.bin"), (3, "q-v4.bin"), (4, "error-v3.bin"), (5, "extraction-v2.bin"), (6, "corrected-v2.bin"), (8, "index-v1.bin")]:
        put("seq_interop", name, run[i])
    cb = mini_run_cbcl()
    put("seq_interop", "tile-v3.bin", cb[1])
    put("seq_interop", "q-v7.bin", cb[2])
    put("seq_interop", "cbcl-header.bin", cb[4][:65])
    fq = b"@r1 1:N:0:ACGT\nACGTNACGTA\n+\nIIIII#IIII\n@r2\nGGCC\nAATT\n+r2\nIIII\n!!!!\n"
    put("whole_fastq", "mini.fastq", fq)
    put("whole_fastq", "mini.fastq.gz", gzip.compress(fq, mtime=0))
    put("whole_bam", "mini.bam", mini_bam())
    vbz = corpus("seq-fast5-vbz-reads.fast5")  # MPL-2.0 (ont_fast5_api test data)
    if vbz is not None:
        put("whole_fast5", "vbz-reads.fast5", vbz)
    # VBZ rows: the worked example of docs/provenance/nanopore.md (19 samples), zstd-compressed
    raw = bytes.fromhex("c001000002020207ca0807270a620200000000000000000000")
    try:
        import pyzstd
        z = pyzstd.compress(raw)
        put("seq_vbz", "pod5-row-19.bin", struct.pack("<H", 19) + z)
        put("seq_vbz", "fast5-chunk-19.bin", struct.pack("<H", 19) + struct.pack("<I", 38) + z)
    except ImportError:
        put("seq_vbz", "raw-19.bin", struct.pack("<H", 19) + raw)
    try:
        import numpy as np
        import pod5
        import tempfile, uuid, datetime
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "mini.pod5"
            run_info = pod5.RunInfo(
                acquisition_id="a", acquisition_start_time=datetime.datetime(2024, 1, 1), adc_max=2047, adc_min=-2048,
                context_tags={"sample_frequency": "4000"}, experiment_name="e", flow_cell_id="F", flow_cell_product_code="FLO",
                protocol_name="p", protocol_run_id="r", protocol_start_time=datetime.datetime(2024, 1, 1), sample_id="s",
                sample_rate=4000, sequencing_kit="k", sequencer_position="1", sequencer_position_type="minion",
                software="seed", system_name="h", system_type="t", tracking_id={"run_id": "r"})
            with pod5.Writer(p) as w:
                w.add_read(pod5.Read(
                    read_id=uuid.UUID(int=1), end_reason=pod5.EndReason(pod5.EndReasonEnum.SIGNAL_POSITIVE, False),
                    calibration=pod5.Calibration(offset=10.0, scale=0.2), pore=pod5.Pore(channel=1, well=1, pore_type="p"),
                    read_number=1, start_sample=0, median_before=200.0, num_minknow_events=1,
                    tracked_scaling=pod5.pod5_types.ShiftScalePair(0.0, 1.0), predicted_scaling=pod5.pod5_types.ShiftScalePair(0.0, 1.0),
                    num_reads_since_mux_change=0, time_since_mux_change=0.0, run_info=run_info,
                    signal=np.arange(64, dtype=np.int16) * 3))
            put("whole_pod5", "mini.pod5", p.read_bytes())
    except Exception as e:  # noqa: BLE001
        print(f"pod5 seed: {e}", file=sys.stderr)


def abif_shrink(data: bytes, keep: int) -> bytes:
    """An ABIF file rewritten with every array cut to its first `keep` elements (the directory
    and every tag kept, so the seed is a complete small file)."""
    count, off = struct.unpack(">i", data[18:22])[0], struct.unpack(">i", data[26:30])[0]
    items = []
    for k in range(count):
        e = data[off + 28 * k: off + 28 * k + 28]
        name, num, et, es, n, size, doff, _ = struct.unpack(">4sihhiiii", e)
        body = e[20:20 + size] if size <= 4 else data[doff:doff + size]
        if n > keep and size > 4 and es > 0 and size == n * es:
            n, body = keep, body[: keep * es]
        items.append((name, num, et, es, n, body))
    blob, entries = b"", b""
    for name, num, et, es, n, body in items:
        if len(body) <= 4:
            entries += struct.pack(">4sihhii", name, num, et, es, n, len(body)) + body.ljust(4, b"\0") + bytes(4)
        else:
            entries += struct.pack(">4sihhiiii", name, num, et, es, n, len(body), 128 + len(blob), 0)
            blob += body
    head = b"ABIF" + data[4:6] + struct.pack(">4sihhiiii", b"tdir", 1, 1023, 28, len(items), len(entries), 128 + len(blob), 0)
    return head.ljust(128, b"\0") + blob + entries


def mini_bioanalyzer() -> bytes:
    """A Bioanalyzer XML export with one ladder well: three ladder peaks and 40 signal values."""
    import base64

    sig = base64.b64encode(struct.pack("<40f", *[abs(20 - i) * 0.5 for i in range(40)])).decode()
    curve = "".join(f"<LadderPeak><MigrationTime>{t}</MigrationTime><FragmentSize>{s}</FragmentSize></LadderPeak>" for t, s in [(20, 25), (25, 200), (30, 1000)])
    peak = "<PeakMolecular><MigrationTime>25</MigrationTime><AlignedMigrationTime>25</AlignedMigrationTime><FragmentSize>200</FragmentSize><Height>10</Height><Observations>Lower Marker</Observations></PeakMolecular>"
    return (
        "<Chipset><Method/><Chips><Chip><AssayHeader><Class>RNA</Class><Title>RNA Nano</Title></AssayHeader>"
        "<AssayBody><DAAssaySetpoints><DAMAssayInfoMolecular><SizeUnit>nt</SizeUnit><ConcentrationUnit>ng/µl</ConcentrationUnit></DAMAssayInfoMolecular></DAAssaySetpoints></AssayBody>"
        "<ChipInformation><CreationDate>2020-01-01T00:00:00</CreationDate></ChipInformation><Files><File><Samples><Sample>"
        "<WellNumber>1</WellNumber><Name>L</Name><Category>Ladder</Category><HasData>true</HasData><DAResultStructures>"
        f"<DARIntegrator><Channel><PeaksMolecular>{peak}</PeaksMolecular></Channel></DARIntegrator>"
        f"<DARStandardCurve><Channel><LadderPeaks>{curve}</LadderPeaks></Channel></DARStandardCurve>"
        "<DARRIN><Channel><RIN>9.5</RIN></Channel></DARRIN></DAResultStructures><DASignals><DetectorChannels><Channel><SignalData>"
        "<ChannelID>RedFluorescence</ChannelID><XStart>20</XStart><XStep>0.25</XStep><XStartAligned>20</XStartAligned><XStepAligned>0.25</XStepAligned>"
        f"<NumberOfSamples>40</NumberOfSamples><ProcessedSignal>{sig}</ProcessedSignal></SignalData></Channel></DetectorChannels></DASignals>"
        "</Sample></Samples></File></Files></Chip></Chips></Chipset>"
    ).encode()


def mini_tapestation() -> bytes:
    """A TapeStation XML export and its electropherogram CSV (two lanes), as a bundle."""
    def peaks(rows):
        return "".join(f"<Peak><Size>{s}</Size><RunDistance>{d}</RunDistance><Observations>{o}</Observations></Peak>" for s, d, o in rows)
    xml = (
        "<?xml version=\"1.0\"?><File><FileInformation><Assay>D1000</Assay><RunEndDate>18-Sep-2020 6:29 AM</RunEndDate></FileInformation>"
        "<ScreenTapes><ScreenTape><ScreenTapeID>T</ScreenTapeID></ScreenTape></ScreenTapes><Assay><Units><MolecularWeightUnit>bp</MolecularWeightUnit></Units></Assay><Samples>"
        f"<Sample><WellNumber>A1</WellNumber><Comment>Ladder</Comment><Observations>Ladder</Observations><ScreenTapeID>T</ScreenTapeID><Peaks>{peaks([(25, 80, 'Lower Marker'), (100, 50, ''), (1500, 10, 'Upper Marker')])}</Peaks></Sample>"
        f"<Sample><WellNumber>B1</WellNumber><Comment>s</Comment><ScreenTapeID>T</ScreenTapeID><DIN>8.1</DIN><Peaks>{peaks([(25, 82, 'Lower Marker'), (1500, 12, 'Upper Marker')])}</Peaks></Sample>"
        "</Samples></File>"
    ).encode()
    csv = ("A1: Ladder,B1: s\n" + "".join(f"{i % 7},{i % 5}\n" for i in range(60))).encode()
    return xml + BUNDLE_SEP + csv


def mini_cfb(files: dict[str, bytes]) -> bytes:
    """A version-3 compound file (512-byte sectors, mini-stream cutoff 0 so every stream lives in
    regular sectors) holding `files` (paths with `/`; storages are implied). MS-CFB layout."""
    nodes = [{"name": "Root Entry", "kind": 5, "children": [], "data": b""}]
    index = {"": 0}
    for path, data in files.items():
        parts = path.split("/")
        for i in range(len(parts)):
            cur, parent = "/".join(parts[: i + 1]), "/".join(parts[:i])
            if cur not in index:
                leaf = i == len(parts) - 1
                nodes.append({"name": parts[i], "kind": 2 if leaf else 1, "children": [], "data": data if leaf else b""})
                index[cur] = len(nodes) - 1
                nodes[index[parent]]["children"].append(index[cur])
    end, free, nostream = 0xFFFFFFFE, 0xFFFFFFFF, 0xFFFFFFFF
    n_dir = (len(nodes) * 128 + 511) // 512
    fat = [0xFFFFFFFD] + [i + 2 for i in range(n_dir - 1)] + [end]
    starts = []
    body = b""
    for n in nodes:
        k = (len(n["data"]) + 511) // 512 if n["kind"] == 2 else 0
        if k == 0:
            starts.append(end)
            continue
        first = len(fat)
        starts.append(first)
        fat += [first + j + 1 for j in range(k - 1)] + [end]
        body += n["data"].ljust(k * 512, b"\0")
    assert len(fat) <= 128, "mini_cfb holds at most 128 sectors"
    fat += [free] * (128 - len(fat))
    directory = b""
    for i, n in enumerate(nodes):
        sib = nostream
        for m in nodes:
            ch = m["children"]
            if i in ch and ch.index(i) + 1 < len(ch):
                sib = ch[ch.index(i) + 1]
        name = n["name"].encode("utf-16-le")
        e = name.ljust(64, b"\0") + struct.pack("<HBB", len(name) + 2, n["kind"], 1)
        e += struct.pack("<III", nostream, sib, n["children"][0] if n["children"] else nostream)
        e += bytes(16 + 4 + 16) + struct.pack("<III", starts[i], len(n["data"]), 0)
        directory += e
    directory = directory.ljust(n_dir * 512, b"\0")
    h = bytes.fromhex("d0cf11e0a1b11ae1") + bytes(16) + struct.pack("<HHHHH", 0x3E, 3, 0xFFFE, 9, 6) + bytes(6)
    h += struct.pack("<IIIIIIIII", 0, 1, 1, 0, 0, end, 0, end, 0)
    h += struct.pack("<I", 0) + struct.pack("<I", free) * 108
    return h + struct.pack("<128I", *fat) + directory + body


def mini_sgd() -> bytes:
    """A GeneSys file with one 4 x 3 16-bit capture (docs/formats/syngene-sgd.md)."""
    def text(s: str) -> bytes:
        return struct.pack("<I", len(s)) + s.encode()
    pixels = struct.pack("<HIIIIII", 2, 4, 3, 1, 1, 1, 4) + struct.pack("<12H", *range(100, 1300, 100))
    capture = (struct.pack("<H", 15) + bytes(8) + b"\x01" + text("GeneSys Version: 1.6.1.0") + struct.pack("<dH", 9.5, 0)
               + text("No Light") + text("Synoptics 6.0MP") + text("ART-694 #1") + text("G:Box mini 6") + struct.pack("<H", 11)
               + text("No Filter") + bytes(8) + text("ECL"))
    desc = struct.pack("<HIHQ", 5, 1663601689, 0, 0) + text("user")
    multi = struct.pack("<HI", 5, 1) + text("RawData") + struct.pack("<I", 1)
    return mini_cfb({"Detector/GeneSnap/RawData/PixelData": pixels, "Detector/GeneSnap/RawData/Capture": capture,
                     "Detector/GeneSnap/RawData/Description": desc, "Detector/MultiImage/MultiData": multi})


def mini_tiff(pages: list) -> bytes:
    """A little-endian classic TIFF: each page is (width, height, bits, samples, extra tags as
    (tag, type, count, value bytes)), uncompressed, one strip."""
    out = bytearray(b"II*\0" + bytes(4))
    offsets = []
    for w, h, bits, data, tags in pages:
        strip = len(out)
        out += data
        entries = [(256, 4, 1, struct.pack("<I", w)), (257, 4, 1, struct.pack("<I", h)), (258, 3, 1, struct.pack("<H", bits)),
                   (259, 3, 1, struct.pack("<H", 1)), (273, 4, 1, struct.pack("<I", strip)), (277, 3, 1, struct.pack("<H", 1)),
                   (278, 4, 1, struct.pack("<I", h)), (279, 4, 1, struct.pack("<I", len(data)))] + tags
        if not any(t[0] == 262 for t in tags):
            entries.append((262, 3, 1, struct.pack("<H", 1)))
        entries.sort()
        blobs = []
        for tag, typ, cnt, val in entries:
            if len(val) > 4:
                if len(out) % 2:
                    out += b"\0"
                blobs.append(struct.pack("<I", len(out)))
                out += val
            else:
                blobs.append(val.ljust(4, b"\0"))
        if len(out) % 2:
            out += b"\0"
        offsets.append(len(out))
        out += struct.pack("<H", len(entries))
        for (tag, typ, cnt, _), b in zip(entries, blobs):
            out += struct.pack("<HHI", tag, typ, cnt) + b
        out += bytes(4)
    struct.pack_into("<I", out, 4, offsets[0])
    for a, b in zip(offsets, offsets[1:]):
        n = struct.unpack_from("<H", out, a)[0]
        struct.pack_into("<I", out, a + 2 + 12 * n, b)
    return bytes(out)


def imager_seeds() -> None:
    """Gel and blot imager and Incucyte TIFFs (docs/formats/tiff.md § Gel and blot imagers,
    § Incucyte) and a GeneSys .sgd, all synthetic."""
    def ascii_tag(tag: int, s: str) -> tuple:
        b = s.encode("latin-1") + b"\0"
        return (tag, 2, len(b), b)

    def p(s: str) -> bytes:
        return bytes([len(s)]) + s.encode("latin-1")

    px16 = struct.pack("<8H", *range(1000, 1800, 100))
    desc = ("Comment = \r\nMethod = Chemiluminescence\r\nExposure time = 5.0 s\r\nDate time = 2023:10:31 07:47:31\r\n"
            "Temperature = -25\r\nBinning = 8x\r\nSerial number = 1\r\nInstrument = Amersham Imager 680\r\nDark offset = 1000\r\nInvert = No")
    put("whole_tiff", "imager-amersham.tif", mini_tiff([(4, 2, 16, px16, [(262, 3, 1, struct.pack("<H", 0)), ascii_tag(270, desc),
                                                                           ascii_tag(272, "Amersham Imager 680")])]))
    glp = (p("_GLP_BPL_") + bytes([7, 0, 0, 0, 1, 0, 0]) + p("Thursday, 13 February 2020") + p("02:00:38") + p("C:\\Images\\b.Tif")
           + p("Alliance  Q9 Mini 18.02 - SN ") + p("0 min 11 sec 200 msec ") + p("Q45191") + p("C:\\U\\Blue.prt")
           + p("7: Chemiluminescence (-20)") + p("-24.55 \xb0C"))
    glp = glp.ljust((len(glp) + 7) // 8 * 8, b"\0")
    put("whole_tiff", "imager-glp.tif", mini_tiff([(4, 2, 16, px16, [(1808, 5, len(glp) // 8, glp)])]))
    xml = ("<ImageInfo><AcqInfo><ObjectiveMagnification>10</ObjectiveMagnification><FullResPixelSpacing>1.244 \xb5m</FullResPixelSpacing>"
           "<ImageType>HD Phase Image</ImageType><CameraSettings><ExpTime>0.46 msec</ExpTime></CameraSettings>"
           "<TimeStamp>07/22/2020 13:33:44</TimeStamp></AcqInfo><DeviceInfo><HardwareName>IncuCyte S3</HardwareName></DeviceInfo></ImageInfo>")
    sw = ascii_tag(305, "IncuCyte 2019B")
    put("whole_tiff", "incucyte.tif", mini_tiff([(4, 2, 8, bytes(range(8)), [sw]),
                                                  (1, 1, 8, b"\0", [(254, 4, 1, struct.pack("<I", 1)), sw, ascii_tag(60020, xml)])]))
    put("whole_sgd", "mini.sgd", mini_sgd())


def main() -> None:
    try:
        import numpy as np
        import imagecodecs
    except ImportError:
        np = imagecodecs = None
        print("imagecodecs/numpy not available: codec seeds limited to zlib", file=sys.stderr)

    # Whole-file and container seeds: synthetic complete files + corpus heads.
    put("whole_czi", "mini-uncompressed.czi", mini_czi())
    put("czi_segment_walk", "mini-uncompressed.czi", mini_czi())
    put("whole_nd2", "mini.nd2", mini_nd2())
    put("whole_nd2", "mini-zlib.nd2", mini_nd2(compressed=True))
    put("nd2_chunk_map", "mini.nd2", mini_nd2())
    put("whole_lif", "mini-v2.lif", mini_lif())
    put("whole_lif", "mini-v1.lif", mini_lif(version=1))
    imager_seeds()
    put("lif_container", "mini-v2.lif", mini_lif())

    img8 = bytes((x * 7 + y * 3) & 0xFF for y in range(8) for x in range(8))
    if imagecodecs is not None:
        arr8 = np.frombuffer(img8, dtype=np.uint8).reshape(8, 8)
        arr16 = (np.arange(64, dtype=np.uint16).reshape(8, 8) * 97) & 0x0FFF
        z = imagecodecs.zstd_encode(img8)
        put("whole_czi", "mini-zstd0.czi", mini_czi(5, z))
        lo_hi = arr16.tobytes()
        shuffled = lo_hi[0::2] + lo_hi[1::2]
        z1 = bytes([3, 1, 1]) + imagecodecs.zstd_encode(shuffled)
        put("codec_zstd1", "hilo-16bit.bin", struct.pack("<I", len(lo_hi))[:3] + z1)
        put("codec_zstd1", "plain-8bit.bin", struct.pack("<I", 65)[:3] + bytes([1, 0]) + z)
        put("codec_zstd0", "gray8.bin", struct.pack("<I", 64)[:3] + z)
        put("codec_zstd0", "gray16.bin", struct.pack("<I", 128)[:3] + imagecodecs.zstd_encode(lo_hi))
        put("codec_hilo", "gray16.bin", shuffled)
        lzw = imagecodecs.lzw_encode(img8)
        put("codec_lzw", "gray8.bin", struct.pack("<I", 64)[:3] + lzw)
        put("codec_lzw", "repeat.bin", struct.pack("<I", 4096)[:3] + imagecodecs.lzw_encode(b"\0" * 4096))
        put("whole_czi", "mini-lzw.czi", mini_czi(2, lzw))
        for name, arr in [("gray8", arr8), ("gray16", arr16), ("rgb8", np.dstack([arr8, arr8 // 2, 255 - arr8]))]:
            try:
                put("codec_jpegxr", f"{name}.jxr", imagecodecs.jpegxr_encode(arr, level=1.0))
            except Exception as e:  # noqa: BLE001
                print(f"jpegxr {name}: {e}", file=sys.stderr)
        for fn, target, name, arr in [
            ("jpeg8_encode", "codec_jpeg", "gray8.jpg", arr8),
            ("jpeg8_encode", "codec_jpeg", "rgb8.jpg", np.dstack([arr8, arr8 // 2, 255 - arr8])),
            ("ljpeg_encode", "codec_jpeg", "lossless16.jpg", arr16),
            ("jpeg2k_encode", "codec_jpeg2000", "gray16.j2k", arr16),
            ("jpeg2k_encode", "codec_jpeg2000", "gray8.j2k", arr8),
        ]:
            try:
                encoded = getattr(imagecodecs, fn)(arr)
                if target == "codec_jpeg":  # 3-byte decoded-size bound first (fuzz_targets/codec_jpeg.rs)
                    encoded = struct.pack("<I", arr.nbytes)[:3] + encoded
                put(target, name, encoded)
            except Exception as e:  # noqa: BLE001
                print(f"{fn} {name}: {e}", file=sys.stderr)
        # No JPEG XR whole-file seed: codec_jpegxr covers that decoder, and its known upstream
        # memory-safety bugs (SECURITY.md) would otherwise dominate whole_czi findings.
    put("codec_zlib", "gray8.bin", struct.pack("<I", 64)[:3] + zlib.compress(img8))
    put("codec_zlib", "zeros.bin", struct.pack("<I", 4096)[:3] + zlib.compress(b"\0" * 4096, 9))
    put("codec_hilo", "odd.bin", bytes(range(17)))

    for name in CZI_FILES:
        data = corpus(name)
        if data is None:
            continue
        put("czi_segment_walk", safe(name) + ".head", data[:HEAD])
        if len(data) <= 128 * 1024:
            put("whole_czi", safe(name), data)
        seen = set()
        for kind, blob in czi_structures(data):
            if kind in seen or not blob:
                continue
            seen.add(kind)
            if kind in ("subblock", "directory", "attdir"):
                put("czi_subblock_header", f"{safe(name)}.{kind}", blob[: 16 * 1024])
            elif kind == "xml" and len(blob) <= 128 * 1024:
                put("czi_metadata_xml", f"{safe(name)}.xml", blob)
            elif kind == "payload5":
                put("codec_zstd0", f"{safe(name)}.zstd0", blob[:HEAD])
            elif kind == "payload6":
                put("codec_zstd1", f"{safe(name)}.zstd1", blob[:HEAD])
            elif kind == "payload4":
                put("codec_jpegxr", f"{safe(name)}.jxr", blob[:HEAD])
    put("czi_metadata_xml", "mini.xml", mini_czi()[32 + 512 + 32 + 256 :].split(b"</ImageDocument>")[0] + b"</ImageDocument>")

    for name in ND2_FILES:
        data = corpus(name)
        if data is None:
            continue
        if len(data) <= 160 * 1024:
            put("whole_nd2", safe(name), data)
        else:
            put("whole_nd2", safe(name) + ".head", data[:HEAD])
        kept = {"nd2_lv": 0, "nd2_variant_xml": 0}
        for item in nd2_chunks(data):
            if item[0] == "map":
                put("nd2_chunk_map", safe(name) + ".map", item[2])
                continue
            _, cname, blob = item
            if len(blob) > 32 * 1024 or cname.startswith("ImageDataSeq") or not blob:
                continue
            target = "nd2_variant_xml" if blob.lstrip()[:1] == b"<" else "nd2_lv"
            # All Image* metadata chunks, plus a couple of custom-data chunks per file.
            if not cname.startswith("Image"):
                if kept[target] >= 2:
                    continue
                kept[target] += 1
            put(target, f"{safe(name)}.{safe(cname)}", blob)
    put("nd2_variant_xml", "mini.xml", b'<?xml version="1.0"?><variant version="1.0"><no_name runtype="CLxListVariant"><uiWidth runtype="lx_uint32" value="696"/><d runtype="double" value="0.5"/><b runtype="bool" value="true"/><s runtype="CLxStringW" value="hi"/><L runtype="CLxListVariant"><_00 runtype="lx_int32" value="-1"/></L><a runtype="CLxByteArray" value="AAEC"/></no_name></variant>')
    put("nd2_lv", "mini-attrs.lv", lv_level("SLxImageAttributes", [lv_u32("uiWidth", 8), lv_u32("uiHeight", 8), lv_u32("uiComp", 1)]))

    for name in LIF_FILES:
        data = corpus(name)
        if data is None:
            continue
        put("lif_container", safe(name) + ".head", data[:HEAD])
        put("whole_lif", safe(name) + ".head", data[:HEAD])
        xml = lif_xml(data)
        if len(xml) <= 256 * 1024:
            put("lif_xml_model", safe(name) + ".xml", xml)
    put("lif_xml_model", "mini.xml", lif_xml(mini_lif()))

    for i, s in enumerate(["c=0", "z=2-5\nt=0,3,7", "c=0,1\nz=0-0\nt=9", "Z = 1 - 2", "c=4294967295", "t=1-"]):
        put("selection", f"s{i}.txt", s.encode())

    # Formats added after the first fuzz campaign. Committed seeds come only from files with a
    # permissive license (MIT, BSD, CC0, CC-BY, public domain, EMBL-EBI terms); see README.md.
    for target, name, cut in COMMITTED_CUTS:
        data = corpus(name)
        if data is not None:
            put(target, safe(name), data[:cut])
    for target, parts in COMMITTED_BUNDLES:
        blobs = []
        for name, cut in parts:
            data = corpus(name)
            blobs.append(b"" if data is None else data[:cut])
        if any(blobs):
            put(target, safe(parts[0][0]) + ".bundle", BUNDLE_SEP.join(blobs))
    # ABIF: the directory sits at the end, so seeds are whole files with short arrays
    for name, keep in [
        ("abif-biopython-3730.ab1", 600),  # BSD-3-Clause (Biopython)
        ("abif-biopython-fsa.fsa", 600),  # BSD-3-Clause (Biopython)
        ("abif-fraggler-multiplex.fsa", 1500),  # MIT (Fraggler)
    ]:
        data = corpus(name)
        if data is not None:
            put("whole_abif", safe(name), abif_shrink(data, keep))
    # Agilent exports: the corpus files are Agilent's demo runs (hold tier), so seeds are synthetic
    put("whole_bioanalyzer", "mini.xml", mini_bioanalyzer())
    put("whole_tapestation", "mini.bundle", mini_tapestation())
    put("whole_jcamp", "mini.jdx", MINI_JCAMP)
    # Plexon corpus files are share-alike (LOCAL_CUTS); committed seeds are magic-only headers.
    put("whole_plexon", "magic.plx", b"PLEX" + bytes(252))
    put("whole_pl2", "magic.pl2", b"\xfe" + bytes(9) + b"PLEXON" + bytes(240))
    put("whole_heka", "mini.dat", mini_heka())
    put("whole_openephys", "mini.continuous", b"header.format = 'Open Ephys Data Format';\nheader.sampleRate = 30000;\nheader.bitVolts = 0.195;\n".ljust(1024, b" ") + struct.pack("<qHH", 0, 1024, 0) + bytes(2048) + bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 255]))
    put("whole_spike2", "magic.smr", struct.pack("<h", 6) + b"(C) CED 87" + bytes(500))
    new_format_seeds()
    synthetic_fixture_seeds()
    ephys_new_seeds()
    sequencing_seeds()
    structural_seeds()
    put("codec_packbits", "runs.bin", struct.pack("<I", 64)[:3] + bytes([0xFD, 7, 2, 1, 2, 3, 0x81, 9]))
    # LZMA2 of b"hello hello hello hello, lzma2!" (CPython lzma, FORMAT_RAW, preset 6)
    put("codec_lzma2", "hello.bin", struct.pack("<I", 31)[:3] + bytes.fromhex("e0001e00145d00341949ee8de9560ae7799a19124543a9df4c4c4000"))
    # Share-alike (CC-BY-SA-4.0) corpus files are not committed; they seed a local,
    # gitignored directory that run.sh adds when present.
    for target, name, cut in LOCAL_CUTS:
        data = corpus(name)
        if data is not None:
            d = ROOT / "local-seeds" / target
            d.mkdir(parents=True, exist_ok=True)
            (d / safe(name)).write_bytes(data[:cut])
    for target, parts in LOCAL_BUNDLES:
        blobs = [(corpus(n) or b"")[:cut] for n, cut in parts]
        if any(blobs):
            d = ROOT / "local-seeds" / target
            d.mkdir(parents=True, exist_ok=True)
            (d / (safe(parts[0][0]) + ".bundle")).write_bytes(BUNDLE_SEP.join(blobs))

    # OME-XML builder seeds: `openreadout info --json` payloads (the `data` field).
    if "--info-json" in sys.argv:
        import json

        src = Path(sys.argv[sys.argv.index("--info-json") + 1])
        for p in sorted(src.glob("*.json")):
            env = json.loads(p.read_text())
            if env.get("ok"):
                data = env["data"]
                data["path"] = Path(data["path"]).name  # no local directory names in seeds
                put("ome_xml", p.stem + ".json", json.dumps(data).encode())

    total = sum(f.stat().st_size for f in OUT.rglob("*") if f.is_file())
    print(f"seed corpus: {sum(1 for f in OUT.rglob('*') if f.is_file())} files, {total} bytes")


if __name__ == "__main__":
    main()
