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
    put("whole_jcamp", "mini.jdx", MINI_JCAMP)
    # Plexon corpus files are share-alike (LOCAL_CUTS); committed seeds are magic-only headers.
    put("whole_plexon", "magic.plx", b"PLEX" + bytes(252))
    put("whole_pl2", "magic.pl2", b"\xfe" + bytes(9) + b"PLEXON" + bytes(240))
    put("whole_heka", "mini.dat", mini_heka())
    put("whole_openephys", "mini.continuous", b"header.format = 'Open Ephys Data Format';\nheader.sampleRate = 30000;\nheader.bitVolts = 0.195;\n".ljust(1024, b" ") + struct.pack("<qHH", 0, 1024, 0) + bytes(2048) + bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 255]))
    put("whole_spike2", "magic.smr", struct.pack("<h", 6) + b"(C) CED 87" + bytes(500))
    new_format_seeds()
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
