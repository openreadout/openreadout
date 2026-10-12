"""Write the synthetic Bruker BAF fixture `crates/openreadout-bruker-tims/tests/fixtures/baf-synthetic.d/`
(`analysis.baf` and `Calibrator.ami`) as docs/formats/bruker-baf.md describes (our own writer; no
vendor code). Deterministic: rerun to reproduce it byte for byte.

- the pointer record, writer and compressor records, an acquisition header (date, string table,
  variable definitions, a reference calibration with drift settings), one segment
- spectrum 1: MS1 at 1.5 s, positive, a packed profile of 40 samples and 3 centroids (f64
  positions, f32 intensities and widths), a kind-3 calibration with a residual polynomial
- spectrum 2: MS/MS at 2.25 s of precursor 300.25 (charge 2, CID, 20 eV, isolation resolution 3)
  with 2 centroids
- the summary table and the trailer index
- Calibrator.ami: a few bytes, then a kind-3 payload with a slightly different K

    python oracle/make_baf_fixtures.py
"""
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates" / "openreadout-bruker-tims" / "tests" / "fixtures" / "baf-synthetic.d"
BASE = 0xBFA00000


def rec(tag, body):
    return struct.pack("<II", 8 + len(body), tag) + body


def cstr(s):
    return s.encode("latin-1") + b"\0"


def calibration(k, temps=(30.5, 28.0)):
    """Kind 3: delay 3000 ns, time base 0.5 ns, t0 2000 ns, K, Q 0.01 (m/z about 100), residual
    1e-3 + 1e-6 m in [50, 1500]."""
    p = struct.pack("<HH5d", 3, 1, 3000.0, 0.5, 2000.0, k, 0.01)
    p += struct.pack("<I5d", 0, temps[0], 2000.0, 0.01, 50.0, 1500.0)
    p += struct.pack("<HHI", 1, 2, 0) + struct.pack("<2d", 1e-3, 1e-6)
    p += struct.pack("<II", 0xBFAC, 0xFFFFFFFF) + bytes(16) + struct.pack("<5I", 0xCFA0, 36, 2, 20, 2)
    p += struct.pack("<2d", *temps)
    return p


def calibration_rec(payload):
    return rec(BASE + 6, struct.pack("<I", 16) + payload)


VARS = [  # id, name, type, value texts, unit
    (10, "Collision_Energy_Act", 10, "", "eV"),
    (11, "Quadrupole_IsolationResolution_Act", 10, "", "m/z"),
    (12, "MSMS_FragmentationMode_Act", 5, "0:CID;1:ETD", ""),
    (13, "MSMS_PreCursorChargeState", 5, "", "z"),
    (20, "Mode_ScanBegin", 10, "", "m/z"),
    (21, "Mode_ScanEnd", 10, "", "m/z"),
    (22, "TOF_SwitchTempCompensation", 5, "0:Off;1:On", ""),
    (23, "TOF_UseOnlyOneTemperature", 5, "", ""),
]


def var_defs():
    body = struct.pack("<HH", 12, len(VARS))  # entries start 12 bytes into the record
    for vid, name, typ, text, unit in VARS:
        strs = [b"", cstr(name), cstr("Group"), cstr(name.replace("_", " ")), cstr(text) if text else b"", b"", cstr(unit) if unit else b""]
        offs, blob, pos = [], b"", 20
        for s in strs:
            offs.append(pos if s else 0)
            blob += s
            pos += len(s)
        entry = struct.pack("<HHBB7H", 20 + len(blob), vid, typ, 0xFF, *offs) + blob
        body += entry
    return rec(BASE + 0x3002, body)


def strings():
    items = [r"D:\Data\synthetic.d\analysis.baf", "synthetic TOF", "Operator", "OpenReadout fixture writer", "1.2.3", "synthetic.m", "0.0.0.0", "User", "Synthetic sample"]
    hdr = 12 + 4 * len(items)  # header size field, offsets of strings 2..n, a 0
    blob = b""
    offs = []
    for i, s in enumerate(items):
        if i:
            offs.append(8 + hdr + len(blob))
        blob += cstr(s)
    body = struct.pack("<I", 8 + hdr) + b"".join(struct.pack("<I", o) for o in offs) + struct.pack("<I", 0)
    body = body.ljust(hdr, b"\0") + blob
    return rec(BASE + 0x3003, body)


def reference():
    p = struct.pack("<HH5d", 2, 1, 3000.0, 0.5, 2000.0, 1.0e8, 0.01) + bytes(8)
    p += struct.pack("<HH4d", 2, 2, 30.0, 28.0, 4.0, 2.0)
    return calibration_rec(p)


def var_list(values):
    body = struct.pack("<II", 16, len(values))
    for vid, v, typ in values:
        raw = struct.pack("<d", v) if typ == 10 else struct.pack("<q", int(v))
        body += struct.pack("<H", vid) + raw
    return rec(BASE + 1, body)


def packed(samples):
    """Packed profile: marker, count, the 26-byte table, then the class-coded deltas."""
    widths = [0, 4, 5, 6, 7, 9, 10, 11, 32]
    base = [0, 1]
    for w in widths[1:-1]:
        base.append(base[-1] + (1 << w))
    bits = []

    def put(v, n):
        for i in range(n - 1, -1, -1):
            bits.append((v >> i) & 1)

    cur, i = 0, 0
    while i < len(samples):
        d = samples[i] - cur
        run = 0
        while d == 0 and i + run < len(samples) and samples[i + run] == cur:
            run += 1
        if run >= 3:
            put(0, 9), put(1, 1), put(run, 32)
            i += run
            continue
        if d == 0:
            put(1, 1)
        else:
            mag = abs(d)
            c = next((c for c in range(1, 8) if base[c] <= mag < base[c] + (1 << widths[c])), None)
            if c is None:
                put(0, 8), put(1, 1), put(d & 0xFFFFFFFF, 32)
            else:
                put(0, c), put(1, 1), put(((mag - base[c]) << 1) | (1 if d < 0 else 0), widths[c] + 1)
        cur = samples[i]
        i += 1
    while len(bits) % 8:
        bits.append(0)
    stream = bytes(int("".join(map(str, bits[k:k + 8])), 2) for k in range(0, len(bits), 8)) + bytes(8)
    table = bytes(widths) + bytes(range(1, 10)) + struct.pack(">II", 1, 10)
    return struct.pack("<II", 0xEE77, len(samples)) + struct.pack(">I", len(table)) + table + stream


def lines(sid, pos, inten, width):
    n = len(pos)
    stats = struct.pack("<5f", 0.0, 39.0, min(inten), max(inten), sum(inten))
    head = struct.pack("<5I", 52, sid, sid, 3, n) + struct.pack("<I", 5) + stats
    first = 4 + 32 + 8
    offs = [first, first + 8 * n, first + 12 * n] + [0] * 5
    data = struct.pack("<I", n) + struct.pack("<8I", *offs) + struct.pack("<d", -1.0)
    data += struct.pack(f"<{n}d", *pos) + struct.pack(f"<{n}f", *inten) + struct.pack(f"<{n}f", *width)
    return rec(BASE + 0x1002, head + data)


def profile(sid, samples):
    body = packed(samples)
    head = struct.pack("<III", 44, sid, sid) + bytes([2, 1, 6, 0]) + struct.pack("<II", len(body), 1000) + struct.pack("<3f", -1, -1, -1)
    return rec(BASE + 0x1001, head + body)


def spectrum(sid, rt_ms, flags, scan_type, subs):
    head = struct.pack("<6I", 40, 1, sid, flags, rt_ms, 0) + bytes(scan_type) + struct.pack("<f", -1.0)
    return rec(BASE + 0x2002, head + b"".join(subs))


def build():
    out = bytearray(36)  # the pointer record, filled in at the end
    out += rec(BASE + 0x4001, b"OPENREADOUT-SYNTH".ljust(32, b"\0") + bytes(20))
    out += rec(BASE + 0x4003, struct.pack("<4HI", 1, 2, 3, 4, 0) + bytes(16) + cstr("Variable VLI2 based compressor"))
    acq_at = len(out)
    date = bytes([12, 10]) + struct.pack("<HhH", 2026, 120, 250) + bytes([30, 15, 9, 0])
    acq_body = bytes(40) + date + cstr("synthetic.d") + strings() + var_defs() + reference()
    out += rec(BASE + 0x3001, acq_body)
    seg_at = len(out)
    seg_vars = [(20, 50.0), (21, 1500.0), (22, 1), (23, 0)]
    sv = struct.pack("<II", 20, len(seg_vars)) + bytes([0, 2, 0, 0])
    for vid, v in seg_vars:
        sv += struct.pack("<H", vid) + (struct.pack("<d", v) if vid in (20, 21) else struct.pack("<q", int(v)))
    out += rec(BASE + 0x2001, struct.pack("<3I", 20, 0, 1) + rec(BASE + 0xD, sv))
    offsets = {"seg": [(1, seg_at)], "spec": [], "prof": [], "lines": []}
    temps = [(30.5, 28.0), (31.0, 28.5)]
    profile_samples = [0, 0, 0, 0, 5, 30, 120, 260, 120, 30, 5, 0, 0, 0, 0, 0, 3000, 70000, 2000, 0] + [0] * 20
    specs = [
        (1, 1500, 3, [0, 2, 0, 0], [], [(10, 0.0, 10), (11, 0.0, 10)], [7.0, 17.0, 23.5], [150.0, 260.0, 70000.0], [2.5, 3.0, 4.0]),
        (2, 2250, 2, [0, 2, 2, 0], [300.25], [(10, 20.0, 10), (11, 3.0, 10), (12, 0, 5), (13, 2, 5)], [11.25, 30.0], [42.0, 7.5], [3.5, 2.5]),
    ]
    summary = b""
    for (sid, rt, flags, st, prec, vals, pos, inten, width), t in zip(specs, temps):
        subs = [var_list(vals)]
        if prec:
            subs.append(rec(BASE + 5, struct.pack("<II", 16, len(prec)) + struct.pack(f"<{len(prec)}f", *prec)))
        subs.append(calibration_rec(calibration(1.0e8 * (1 - 1e-6 * (4.0 * (t[0] - 30.0) + 2.0 * (t[1] - 28.0))), t)))
        offsets["spec"].append((sid, len(out)))
        out += spectrum(sid, rt, flags, st, subs)
        if flags == 3:
            offsets["prof"].append((sid, len(out)))
            out += profile(sid, profile_samples)
        offsets["lines"].append((sid, len(out)))
        out += lines(sid, pos, inten, width)
        out += rec(BASE + 0x4004, struct.pack("<5I", 28, 0x2002, sid, 1, 0))
        stats = struct.pack("<5f", 0.0, 39.0, min(inten), max(inten), sum(inten))
        summary += struct.pack("<HHI5I", 56, 0 if not prec else 60, 60, rt, 0, 1, sid, flags) + stats + bytes(st) + struct.pack("<f", -1.0)
    sum_at = len(out)
    out += rec(BASE + 8, struct.pack("<HHII", 20, 0, 1000, len(specs)) + summary)
    tr_at = len(out)
    entries = b"".join(struct.pack("<IQ", i, o) for key in ("seg", "spec", "prof", "lines") for i, o in offsets[key])
    out += rec(BASE + 0xA, struct.pack("<II", 4, 64).ljust(56, b"\0") + entries)
    out[0:36] = rec(BASE + 9, struct.pack("<I3Q", BASE + 5, acq_at, sum_at, tr_at))
    return bytes(out)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "analysis.baf").write_bytes(build())
    (OUT / "Calibrator.ami").write_bytes(b"\xa6\xbf synthetic recalibration \0" + calibration(1.00002e8, (30.0, 28.0)) + bytes(16))
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
