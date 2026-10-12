"""Write the synthetic TDT block `crates/openreadout-ephys/tests/fixtures/tdt-synthetic/` as
docs/formats/tdt-tank.md describes (our own writer; no vendor code). Rerun to reproduce it byte
for byte.

- `synthetic_B1.tsq`: 40-byte index records (record 0 with the file length, a start mark, the
  stream chunks, snippets and strobes, a stop mark)
- `synthetic_B1.tev`: the chunks and snippet waveforms back to back
- stream store `Wave`: 2 int16 channels, 16 samples per chunk, 4 chunks, 1000 Hz
- stream store `Flt1`: 1 float32 channel, 8 samples per chunk, 3 chunks, 500 Hz
- snippet store `eSpk`: 3 snippets of 8 float32 samples on channels 1 and 2, sort codes 0 and 1
- strobe store `Trg/` with two onsets (values 3 and 5), the first closed by a `Trg\\` record

    python oracle/make_tdt_fixtures.py
"""
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates" / "openreadout-ephys" / "tests" / "fixtures" / "tdt-synthetic"
T0 = 1_700_000_000.0


def rec(size, kind, name, channel, sort, time, payload, fmt, rate):
    """One index record; `payload` is the offset (int) or a strobe value (float)."""
    tail = struct.pack("<q", payload) if isinstance(payload, int) else struct.pack("<d", payload)
    return struct.pack("<ii4sHHd", size, kind, name, channel, sort, time) + tail + struct.pack("<if", fmt, rate)


def main():
    tev = bytearray()
    recs = [rec(20, 0x8801, b"\x01\0\0\0", 0, 0, T0, 40, 0, 0.0)]
    for k in range(4):
        for ch in (1, 2):
            at = len(tev)
            tev += struct.pack("<16h", *[(ch * 100 + k * 16 + i) * (-1) ** i for i in range(16)])
            recs.append(rec(10 + 8, 0x8101, b"Wave", ch, 0, T0 + 0.001 + k * 0.016, at, 2, 1000.0))
        if k < 3:
            at = len(tev)
            tev += struct.pack("<8f", *[k + i / 8 for i in range(8)])
            recs.append(rec(10 + 8, 0x8101, b"Flt1", 1, 0, T0 + 0.002 + k * 0.016, at, 0, 500.0))
            at = len(tev)
            tev += struct.pack("<8f", *[(-1) ** k * (i - 4) / 4 for i in range(8)])
            recs.append(rec(10 + 8, 0x8201, b"eSpk", 1 + k % 2, k % 2, T0 + 0.01 + k * 0.02, at, 0, 24414.0625))
    recs.append(rec(10, 0x101, b"Trg/", 0, 0, T0 + 0.005, 3.0, 4, 0.0))
    recs.append(rec(10, 0x102, b"Trg\\", *struct.unpack("<HH", b"Trg/"), T0 + 0.025, 3.0, 4, 0.0))
    recs.append(rec(10, 0x101, b"Trg/", 0, 0, T0 + 0.04, 5.0, 4, 0.0))
    recs.append(rec(20, 0x8801, b"\x02\0\0\0", 0, 0, T0 + 0.07, len(tev), 0, 0.0))
    tsq = rec((len(recs) + 1) * 40, 0, b"\0\0\0\0", 0, 0, 0.0, 0, 0, 0.0) + b"".join(recs)
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "synthetic_B1.tsq").write_bytes(tsq)
    (OUT / "synthetic_B1.tev").write_bytes(bytes(tev))
    print(f"wrote {OUT}: {len(tsq)} + {len(tev)} bytes")


if __name__ == "__main__":
    main()
