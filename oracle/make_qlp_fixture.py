#!/usr/bin/env python
"""Write a small synthetic QuantaSoft `.qlp` (layout: docs/formats/biorad-ddpcr.md) for tests and
fuzz seeds: one plate setup of two wells (A01 used on both channels, A02 on channel 1), the
supermix of A01, and one read well (A01) of six droplets, one of each class plus a rejected one.

Usage:  python make_qlp_fixture.py OUT.qlp
"""
import struct
import sys


def build() -> bytes:
    out = bytearray(b"IILQ" + struct.pack("<I", 0))
    blobs = []

    def blob(b: bytes) -> int:
        """Append `b` after the header; returns its offset."""
        off = len(out)
        out.extend(b)
        return off

    def ifd(entries, nxt=0) -> int:
        off = len(out)
        out.extend(struct.pack("<H", len(entries)))
        for tag, typ, count, value in entries:
            out.extend(struct.pack("<HHI", tag, typ, count) + value)
        out.extend(struct.pack("<I", nxt))
        return off

    def u32(v):
        return struct.pack("<I", v)

    software = blob(b"QuantaSoft 1.7.4.0917\0")
    channels = blob(b"Ch1,Ch2\0")
    when = blob(b"2026:10:19 10:00:00\0")

    def field(text: str, n: int) -> bytes:
        b = text.encode() + b"\0"
        return b + b"\0" * (n - len(b))

    setup_at = len(out)
    recs = bytearray()
    for k, (well, sample, ch1, ch2) in enumerate([
        ("A01", "S1", ("Unknown", "GeneA"), ("Unknown", "GeneB")),
        ("A02", "NTC", ("NTC", "GeneA"), ("Not Used", "")),
    ]):
        r = u32(setup_at + 1386 * (k + 1)) + b"\x02\x01" + field(well, 4) + field(sample, 256)
        r += field("Absolute Quantification", 32) + field("ABS", 512)
        r += field(ch1[0], 32) + field(ch1[1], 256) + field(ch2[0], 32) + field(ch2[1], 256)
        assert len(r) == 1386
        recs += r
    blob(bytes(recs))
    supermix = blob(b"ddPCR Supermix for Probes\0")
    droplets = [
        (1000, 900.0, 11.0, 1.0, 1100.0, 11.0, 1.0, 0x11),
        (1010, 9000.0, 12.0, 1.0, 1150.0, 12.0, 1.0, 0x22),
        (1020, 9100.0, 12.5, 1.0, 6000.0, 12.5, 1.0, 0x33),
        (1030, 950.0, 11.5, 1.0, 6100.0, 11.5, 1.0, 0x44),
        (1040, 400.0, 30.0, 0.5, 500.0, 30.0, 0.5, 0x00),
        (1050, 920.0, 11.2, 1.0, 1120.0, 11.2, 1.0, 0x11),
    ]
    rec_at = blob(b"".join(struct.pack("<I6f", *d[:7]) for d in droplets))
    cls_at = blob(bytes(d[7] for d in droplets))
    # IFDs, written last to first so each knows the next one's offset
    well_ifd = ifd([
        (65019, 2, 1, b"A01\0"),
        (65021, 7, len(droplets), u32(rec_at)),
        (65057, 7, len(droplets), u32(cls_at)),
        (65078, 11, 1, struct.pack("<f", 0.85)),
    ])
    setup_a02 = ifd([(65019, 2, 1, b"A02\0"), (65077, 2, 1, u32(0))], well_ifd)
    setup_a01 = ifd([(65019, 2, 1, b"A01\0"), (65077, 2, 26, u32(supermix))], setup_a02)
    plate = ifd([(65018, 7, 2, u32(setup_at))], setup_a01)
    first = ifd([
        (305, 2, 22, u32(software)),
        (306, 2, 20, u32(when)),
        (65004, 2, 8, u32(channels)),
    ], plate)
    out[4:8] = u32(first)
    return bytes(out)


if __name__ == "__main__":
    open(sys.argv[1], "wb").write(build())
