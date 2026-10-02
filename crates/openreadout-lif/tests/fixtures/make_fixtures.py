#!/usr/bin/env python3
"""Write the SYNTHETIC Leica-family test files in this directory (stdlib only, deterministic).

None of these files came from a microscope or from LAS X. They are built from the structure that
liffile (BSD-3-Clause, https://github.com/cgohlke/liffile) documents, because no licensed LOF /
XLIF / XLEF / XLCF / XLLF corpus file exists yet, and because no licensed LIF in the corpus has
rotation (DimID 6) or XT/T-slice (DimID 7/8) axes. See docs/provenance/lif.md (2026-09-22).
Ground truth for the files liffile can read is oracle/*.json, written by
`cd oracle && ORACLE_OUT=../crates/openreadout-lif/tests/fixtures/oracle uv run python gen.py <files>`.

    python3 make_fixtures.py        # rewrites every fixture
"""
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent


def text_block(text: str) -> bytes:
    u = text.encode("utf-16-le")
    n = len(u) // 2
    return struct.pack("<IIBI", 0x70, 2 * n + 5, 0x2A, n) + u


def mem_block(block_id: str, payload: bytes) -> bytes:
    u = block_id.encode("utf-16-le")
    n = len(u) // 2
    return struct.pack("<IIBQBI", 0x70, 2 * n + 14, 0x2A, len(payload), 0x2A, n) + u + payload


def pattern(nbytes: int, seed: int) -> bytes:
    return bytes(((i * 37 + seed * 101 + (i >> 8) * 13) & 0xFF) for i in range(nbytes))


def channel(bits=8, inc=0, lut="Gray", dtype=0, tag=0):
    return (f'<ChannelDescription DataType="{dtype}" ChannelTag="{tag}" Resolution="{bits}" NameOfMeasuredQuantity="" '
            f'Min="0" Max="255" Unit="" LUTName="{lut}" IsLUTInverted="0" BytesInc="{inc}" BitInc="0"/>')


def dim(dim_id, n, inc, origin=0.0, length=None, unit="m"):
    if length is None:
        length = (n - 1) * 1e-6
    return (f'<DimensionDescription DimID="{dim_id}" NumberOfElements="{n}" Origin="{origin!r}" '
            f'Length="{length!r}" Unit="{unit}" BitInc="0" BytesInc="{inc}"/>')


def image_element(name, block, size, channels, dims, extra=""):
    return (f'<Element Name="{name}" Visibility="1" CopyOption="1" UniqueID="00000000-0000-0000-0000-{zlib.crc32(name.encode()):012d}">'
            f'<Data><Image TextDescription=""><ImageDescription><Channels>{"".join(channels)}</Channels>'
            f'<Dimensions>{"".join(dims)}</Dimensions></ImageDescription>{extra}</Image></Data>'
            f'<Memory Size="{size}" MemoryBlockID="{block}"/><Children/></Element>')


def write(path: Path, data: bytes):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    print("wrote", path.relative_to(HERE), len(data), "bytes")


HALF_VALUES = [0x0000, 0x3C00, 0xC000, 0x7BFF, 0x0001, 0x03FF, 0x7C00, 0x7E00, 0x7C01, 0x8000, 0x3555, 0xFBFF]


def synthetic_lif():
    elems, blocks = [], [mem_block("MemBlock_0", b"")]
    # 1. lambda x 2 channels, 12-bit in 16-bit samples: X 8 (inc 2), Y 4 (16), lambda 3 (64); channels 192 apart
    size = 8 * 4 * 3 * 2 * 2
    elems.append(image_element("Lambda2ch", "MemBlock_1", size,
        [channel(12, 0, "Green"), channel(12, 192, "Red")],
        [dim(1, 8, 2), dim(2, 4, 16), dim(5, 3, 64, origin=5.0e-7, length=4.0e-8)]))
    blocks.append(mem_block("MemBlock_1", pattern(size, 1)))
    # 2. rotation series: X 6, Y 5, Z 2 (inc 30), rotation 3 (inc 60)
    size = 6 * 5 * 2 * 3
    elems.append(image_element("Rotation", "MemBlock_2", size, [channel()],
        [dim(1, 6, 1), dim(2, 5, 6), dim(3, 2, 30, length=2e-6), dim(6, 3, 60, length=90.0, unit="degree")]))
    blocks.append(mem_block("MemBlock_2", pattern(size, 2)))
    # 3. XT slices and T slices: X 4, Y 4, T 2 (inc 16), XT slices 2 (inc 32), T slices 3 (inc 64)
    size = 4 * 4 * 2 * 2 * 3
    elems.append(image_element("Slices", "MemBlock_3", size, [channel()],
        [dim(1, 4, 1), dim(2, 4, 4), dim(4, 2, 16, length=1.5, unit="s"), dim(7, 2, 32, length=1.0, unit=""), dim(8, 3, 64, length=2.0, unit="")]))
    blocks.append(mem_block("MemBlock_3", pattern(size, 3)))
    # 4. tile scan, FlipX only: 3 tiles of 6x4, 1 um pixels, stage steps of 4 um (2 px overlap), Z 2
    size = 6 * 4 * 2 * 3
    tiles = "".join(f'<Tile FieldX="{i}" FieldY="0" PosX="{x!r}" PosY="{y!r}" PosZ="0"/>'
                    for i, (x, y) in enumerate([(0.01, 0.02), (0.01 + 4e-6, 0.02 + 1e-6), (0.01 + 8e-6, 0.02)]))
    tsi = f'<Attachment Name="TileScanInfo" Application="LAS AF" FlipX="1" FlipY="0" SwapXY="0">{tiles}</Attachment>'
    elems.append(image_element("TileScan", "MemBlock_4", size, [channel()],
        [dim(1, 6, 1, length=5e-6), dim(2, 4, 6, length=3e-6), dim(3, 2, 24, length=1e-6), dim(10, 3, 48, length=2.0, unit="")], tsi))
    blocks.append(mem_block("MemBlock_4", pattern(size, 4)))
    # 5. half floats (Resolution 16, DataType 1), 4 x 3
    vals = HALF_VALUES
    elems.append(image_element("Half", "MemBlock_5", 24, [channel(16, 0, "Gray", dtype=1)], [dim(1, 4, 2), dim(2, 3, 8)]))
    blocks.append(mem_block("MemBlock_5", struct.pack("<12H", *vals)))
    # 6. FALCON FLIM element (raw photon data, not decodable)
    flim = ('<Element Name="FLIM Compressed" Visibility="1" CopyOption="1"><Data><SingleMoleculeDetection IsImage="true" IsAnalysisResult="false">'
            '<Dataset><RawData><Format>LMSCOMPRESSED</Format><VoxelSizeX>2e-07</VoxelSizeX><VoxelSizeY>2e-07</VoxelSizeY><VoxelSizeZ>0</VoxelSizeZ>'
            '<Dimensions><Dimension><DimensionIdentifier>X</DimensionIdentifier><Size>16</Size></Dimension>'
            '<Dimension><DimensionIdentifier>Y</DimensionIdentifier><Size>8</Size></Dimension>'
            '<Dimension><DimensionIdentifier>T</DimensionIdentifier><Size>1</Size></Dimension></Dimensions>'
            '<ClockPeriod>9.696969697e-11</ClockPeriod><LaserPulseFrequency>19505000</LaserPulseFrequency><PixelTime>1.725e-06</PixelTime>'
            '<BiDirectional>false</BiDirectional><SinusCorrection>0</SinusCorrection></RawData></Dataset></SingleMoleculeDetection></Data>'
            '<Memory Size="64" MemoryBlockID="MemBlock_6"/><Children/></Element>')
    elems.append(flim)
    blocks.append(mem_block("MemBlock_6", pattern(64, 6)))
    xml = ('<LMSDataContainerHeader Version="2"><Element Name="synthetic-dims.lif" Visibility="0" CopyOption="1" UniqueID="00000000-0000-0000-0000-000000000000">'
           '<Data><Experiment IsSavedFlag="1" Path="synthetic-dims.lif"><TimeStamp HighInteger="30000000" LowInteger="0"/></Experiment></Data>'
           '<Memory Size="0" MemoryBlockID="MemBlock_0"/><Children>' + "".join(elems) + "</Children></Element></LMSDataContainerHeader>")
    write(HERE / "synthetic-dims.lif", text_block(xml) + b"".join(blocks))


def lof_bytes(xml: str, payload: bytes) -> bytes:
    return (text_block("LMS_Object_File") + struct.pack("<BIBI", 0x2A, 2, 0x2A, 1)
            + struct.pack("<BQ", 0x2A, len(payload)) + payload + text_block(xml))


def lof_image_xml(name, block, size, channels, dims):
    return '<LMSDataContainerHeader Version="2">' + image_element(name, block, size, channels, dims) + "</LMSDataContainerHeader>"


def xlif(name, block, size, channels, dims, frames):
    fr = "".join(f'<Frame File="{f}" Offset="{o}" Size="{s}" UUID="00000000-0000-0000-0000-00000000000{i}"/>' for i, (f, o, s) in enumerate(frames))
    return ('<?xml version="1.0" encoding="utf-8"?>\n<LMSDataContainerHeader Version="2">'
            f'<Element Name="{name}" Visibility="1" CopyOption="1" UniqueID="00000000-0000-0000-0000-0000000000a1">'
            f'<Data><Image TextDescription=""><ImageDescription><Channels>{"".join(channels)}</Channels>'
            f'<Dimensions>{"".join(dims)}</Dimensions></ImageDescription></Image></Data>'
            f'<Memory Size="{size}" MemoryBlockID="{block}">{fr}</Memory><Children/></Element></LMSDataContainerHeader>')


def refs_xml(name, data_tag, refs):
    r = "".join(f'<Reference File="{f}" UUID="00000000-0000-0000-0000-0000000000b{i}"/>' for i, f in enumerate(refs))
    return ('<?xml version="1.0" encoding="utf-8"?>\n<LMSDataContainerHeader Version="2">'
            f'<Element Name="{name}" Visibility="1" CopyOption="1" UniqueID="00000000-0000-0000-0000-0000000000c1">'
            f'<Data><{data_tag}/></Data><Children>{r}</Children></Element></LMSDataContainerHeader>')


def lof_and_xlef():
    # standalone LOF: 5 x 3, two 8-bit channels 15 bytes apart
    ch2 = [channel(8, 0, "Green"), channel(8, 15, "Red")]
    d53 = [dim(1, 5, 1), dim(2, 3, 5)]
    write(HERE / "lof" / "Single.lof", lof_bytes(lof_image_xml("Single", "MemBlock_7", 30, ch2, d53), pattern(30, 7)))
    # older LOF: bare <Data> fragment instead of LMSDataContainerHeader
    old = ('<Data><Image TextDescription=""><ImageDescription><Channels>' + channel(16, 0) + '</Channels><Dimensions>'
           + dim(1, 4, 2) + dim(2, 2, 8) + '</Dimensions></ImageDescription></Image></Data><Memory Size="16" MemoryBlockID="MemBlock_1"/>')
    write(HERE / "lof" / "Legacy.lof", lof_bytes(old, pattern(16, 8)))

    # XLEF experiment folder: Experiment.xlef -> Series001.xlif (one LOF frame)
    #                                        -> Collection\Collection.xlcf -> Series002.xlif (two LOF frames, Z 0 and Z 1)
    root = HERE / "xlef"
    s1 = lof_image_xml("Series001", "MemBlock_11", 30, ch2, d53)
    write(root / "Series001.lof", lof_bytes(s1, pattern(30, 11)))
    write(root / "Series001.xlif", xlif("Series001", "MemBlock_11", 30, ch2, d53, [("Series001.lof", 0, 30)]).encode())
    z_plane = [channel(16, 0, "Gray")]
    dz = [dim(1, 4, 2), dim(2, 3, 8), dim(3, 2, 24, length=5e-7)]
    for z, part in enumerate("ab"):
        xml = lof_image_xml(f"Series002_{part}", f"MemBlock_2{z}", 24, z_plane, [dim(1, 4, 2), dim(2, 3, 8)])
        write(root / "Collection" / f"Series002_{part}.lof", lof_bytes(xml, pattern(24, 20 + z)))
    write(root / "Collection" / "Series002.xlif",
          xlif("Series002", "MemBlock_22", 48, z_plane, dz, [("Series002_a.lof", 0, 24), ("Series002_b.lof", 24, 24)])
          .replace('encoding="utf-8"', 'encoding="utf-16"').encode("utf-16"))
    write(root / "Collection" / "Collection.xlcf", refs_xml("Collection", "Collection", ["Series002.xlif"]).encode())
    write(root / "Experiment.xlef", refs_xml("Experiment", "Experiment", ["Series001.xlif", "Collection\\Collection.xlcf"]).encode())

    # broken experiment: a missing reference and a TIFF frame (read with clean errors, never a panic)
    broken = HERE / "xlef-broken"
    write(broken / "Series009.xlif", xlif("Series009", "MemBlock_90", 30, ch2, d53, [("Series009.bmp", 0, 30)]).encode())
    write(broken / "Broken.xlef", refs_xml("Broken", "Experiment", ["Series009.xlif", "Missing.xlif"]).encode())
    # folder view (XLLF): handled like a collection (inferred; liffile does not read XLLF)
    write(root / "Folder.xllf", refs_xml("Folder", "Folder", ["Series001.xlif"]).encode())


if __name__ == "__main__":
    synthetic_lif()
    lof_and_xlef()
