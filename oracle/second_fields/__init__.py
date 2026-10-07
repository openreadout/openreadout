"""Second opinions on normalized fields and key values, for every family.

A reader independent of OpenReadout (and, where one exists, of the primary oracle) is run on
each development input and its values are written as *checks* to
`corpus/oracle/second/fields/<id>.json`. The corpus test
`crates/openreadout-corpus-tests/tests/second_fields.rs` runs OpenReadout, finds our value for
each check and compares; a difference fails until `corpus/oracle/second/adjudications.toml`
says who is right and why.

A family plugs in with a module in this package exposing

    FAMILY = "mass-spectrometry"             # the family name in reports
    FORMATS = {"thermo-raw", ...}            # manifest formats it covers
    def run(rec: Rec, entry: dict, path: Path, ctx: Ctx) -> None

`run` adds checks with `rec.check(...)` (see `Rec.check`) and names the readers it used with
`rec.reader(...)`. It never runs OpenReadout: the second opinion must not see our values.

Usage (from oracle/, development inputs only, never held-out):
    ../oracle/.venv/bin/python -m second_fields [--family F] [--format F] [ID_SUBSTRING ...]
"""
from __future__ import annotations

import json
import math
import os
import sys
import tomllib
import traceback
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "oracle"))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB)
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
OUT = ROOT / "corpus" / "oracle" / "second" / "fields"
MAX_SAMPLES = int(os.environ.get("SECOND_MAX_SAMPLES", "200"))


def spread(n: int, k: int = MAX_SAMPLES) -> list[int]:
    """Up to k indices spread evenly over range(n), first and last included."""
    if n <= 0:
        return []
    if n <= k:
        return list(range(n))
    return sorted({round(i * (n - 1) / (k - 1)) for i in range(k)})


def clean(v):
    """JSON-safe: NumPy scalars to Python, NaN/inf to None, tuples to lists."""
    try:
        import numpy as np
        if isinstance(v, np.generic):
            v = v.item()
        elif isinstance(v, np.ndarray):
            v = v.tolist()
    except ImportError:
        pass
    if isinstance(v, float) and not math.isfinite(v):
        return None
    if isinstance(v, (list, tuple)):
        return [clean(x) for x in v]
    if isinstance(v, dict):
        return {str(k): clean(x) for k, x in v.items()}
    if isinstance(v, bytes):
        return v.decode("utf-8", "replace")
    return v


@dataclass
class Rec:
    id: str
    file: str
    format: str
    family: str
    readers: list[str] = field(default_factory=list)
    checks: list[dict] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)

    def reader(self, name: str) -> int:
        """Register a reader description ("pyopenms 3.5.0 (BSD-3) on the depositor's mzML"),
        returning its index for `check(reader=...)`."""
        if name not in self.readers:
            self.readers.append(name)
        return self.readers.index(name)

    def check(self, field: str, ours: str, theirs, *, cmd=("info",), cmp: str = "exact",
              scope: str = "metadata", rel: float = 0.0, abs: float | None = None,
              zone: bool = False, at: list[int] | None = None, by: str | None = None,
              each: str | None = None, required: bool = False, reader: int = 0) -> None:
        """One comparison. `ours` is a path into the JSON `data` of `openreadout <cmd>` (see
        second_fields.rs); `theirs` the second reader's value. With `by`/`each`, `ours` is an
        array of records and `theirs` a dict keyed by the text of each record's `by` value
        (e.g. a native id). Nothing is recorded when the second reader has no value (None,
        empty text, empty list or dict)."""
        theirs = clean(theirs)
        if theirs is None or theirs == "" or theirs == [] or theirs == {}:
            return
        if isinstance(theirs, str):
            theirs = theirs.strip()
            if not theirs or set(theirs) <= {"x", "X"}:  # empty, or anonymized by the depositor
                return
        c = {"field": field, "scope": scope, "cmd": list(cmd), "ours": ours, "theirs": theirs, "cmp": cmp}
        if rel:
            c["rel"] = rel
        if abs is not None:
            c["abs"] = abs
        if zone:
            c["zone"] = True
        if at is not None:
            c["at"] = list(at)
        if by is not None:
            c["by"] = by
            c["each"] = each or ""

        if required:
            c["required"] = True
        if reader:
            c["reader"] = reader
        self.checks.append(c)

    def note(self, text: str) -> None:
        self.notes.append(text)

    def dump(self) -> dict:
        d = {"id": self.id, "file": self.file, "format": self.format, "family": self.family,
             "readers": self.readers, "checks": self.checks}
        if self.notes:
            d["notes"] = self.notes
        return d


def trace_values(rec: Rec, trace: int, sweep: int, channel: int, values, reader: int, *, rel: float = 1e-9,
                 abs: float | None = None, n_samples: int = 32, stats: bool = True, count: bool = True,
                 label: str | None = None, extra_cmd: tuple = (), field_prefix: str | None = None) -> None:
    """The samples of one channel of one sweep of a trace, against `openreadout trace`: the
    first `n_samples` values and whole-sweep statistics (count, min, max, mean). `values` are the
    second reader's physical values in the same unit as ours; `abs` defaults to 1e-12 of the
    largest magnitude (so exact zeros compare)."""
    import numpy as np
    v = np.asarray(values, dtype=np.float64).reshape(-1)
    if v.size == 0:
        return
    finite = v[np.isfinite(v)]
    scale = float(np.max(np.abs(finite))) if finite.size else 0.0
    a = abs if abs is not None else 1e-12 * scale
    cmd = ("trace", "--trace", str(trace), "--sweep", str(sweep), "--max-samples", str(n_samples), *extra_cmd)
    ch = f"/channels/[index={channel}]"
    name = field_prefix or f"trace {trace}{'' if label is None else f' ({label})'} sweep {sweep} channel {channel}"
    k = min(n_samples, v.size)
    rec.check(f"{name}: first samples", f"{ch}/samples", [float(x) for x in v[:k]], cmd=cmd, cmp="num",
              rel=rel, abs=a, scope="traces", reader=reader)
    if count:
        rec.check(f"{name}: sample count", f"{ch}/stats/count", int(v.size), cmd=cmd, scope="traces", reader=reader)
    if stats and finite.size:
        for key, fn in (("min", np.min), ("max", np.max)):
            rec.check(f"{name}: {key}", f"{ch}/stats/{key}", float(fn(finite)), cmd=cmd, cmp="num", rel=rel,
                      abs=a, scope="traces", reader=reader)
        rec.check(f"{name}: mean", f"{ch}/stats/mean", float(np.mean(finite)), cmd=cmd, cmp="num",
                  rel=max(rel, 1e-9), abs=max(a, 1e-9 * scale), scope="traces", reader=reader)


class Ctx:
    """Manifest access for plugins: exports paired with an input, entries by id."""

    def __init__(self, manifest: list[dict]):
        self.manifest = manifest
        self.by_id = {e["id"]: e for e in manifest}

    def path(self, entry_or_id) -> Path:
        e = self.by_id[entry_or_id] if isinstance(entry_or_id, str) else entry_or_id
        return FILES / e["filename"]

    def entry(self, id_: str) -> dict | None:
        return self.by_id.get(id_)

    def oracle(self, id_: str) -> dict | None:
        """The primary oracle JSON of an id (corpus/oracle/<id>.json), if any."""
        p = ROOT / "corpus" / "oracle" / f"{id_}.json"
        if oracle_json.exists(p):  # <id>.json or <id>.json.gz
            try:
                return oracle_json.load(p)
            except json.JSONDecodeError:
                return None
        return None


def main(argv: list[str]) -> int:
    fam = fmt = None
    only: list[str] = []
    it = iter(argv)
    for a in it:
        if a == "--family":
            fam = next(it)
        elif a == "--format":
            fmt = set(next(it).split(","))
        else:
            only.append(a)
    manifest = tomllib.load(open(ROOT / "corpus" / "manifest.toml", "rb"))["file"]
    ctx = Ctx(manifest)
    OUT.mkdir(parents=True, exist_ok=True)
    mods = []
    for name in ("ms", "flow", "ephys", "nmr", "vib", "chrom", "plate", "qpcr", "bench", "images"):
        if fam and fam != name:
            continue
        try:
            mods.append(__import__(f"second_fields.{name}", fromlist=[name]))
        except ModuleNotFoundError as e:
            if e.name and e.name.startswith("second_fields."):
                continue
            raise
    n_files = n_checks = 0
    for e in manifest:
        if e.get("role") != "input" or e.get("tier") == "heldout" or e.get("role") == "heldout":
            continue
        if fmt and e["format"] not in fmt:
            continue
        if only and not any(o in e["id"] for o in only):
            continue
        mod = next((m for m in mods if e["format"] in m.FORMATS), None)
        if mod is None:
            continue
        p = FILES / e["filename"]
        if not p.exists():
            continue
        rec = Rec(e["id"], e["filename"], e["format"], mod.FAMILY)
        out = OUT / f"{e['id']}.json"
        try:
            mod.run(rec, e, p, ctx)
        except Exception as ex:  # a second reader that cannot read the file: recorded, not fatal
            d = rec.dump() | {"error": f"{type(ex).__name__}: {ex}"}
            out.write_text(json.dumps(d, indent=1) + "\n")
            print(f"{e['id']}: ERROR {type(ex).__name__}: {ex}", flush=True)
            if os.environ.get("SECOND_TRACE"):
                traceback.print_exc()
            continue
        if not rec.checks:
            if out.exists():
                out.unlink()
            print(f"{e["id"]}: no checks {rec.notes[:2]}", flush=True)
            continue
        text = json.dumps(rec.dump(), indent=1, ensure_ascii=False)
        for d in {str(p.parent), str(p.resolve().parent)}:
            text = text.replace(d + "/", "")
        out.write_text(text + "\n")
        n_files += 1
        n_checks += len(rec.checks)
        print(f"{e['id']}: {len(rec.checks)} checks ({', '.join(rec.readers)})", flush=True)
    print(f"wrote {n_files} files, {n_checks} checks")
    return 0
