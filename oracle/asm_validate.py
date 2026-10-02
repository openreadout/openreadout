#!/usr/bin/env python
"""Validate Allotrope Simple Model (ASM) plate-reader JSON against the public ASM schema.

Usage: uv run python asm_validate.py FILE.json [...]

The schema is the unmodified self-contained ("embed") plate-reader schema REC/2025/03 from the
public Allotrope repository, downloaded at a pinned commit into oracle/.cache/asm/ (gitignored).
It is never copied into the repository: its licence (CC BY-NC 4.0 / CC BY-ND 4.0) does not allow
distributing modified copies, and we do not need to distribute it at all (see
docs/provenance/plate-readers.md). Validation uses `jsonschema` (Draft 2020-12, with format checks
for date-time).
"""
import json, sys, urllib.request
from pathlib import Path

ASM_REPO = "https://" + "gitlab.com/allotrope-public/asm"
ASM_COMMIT = "1c8c466a3fe4ad803e1a144d48971d9728ab4123"
SCHEMA_PATH = "json-schemas/adm/plate-reader/REC/2025/03/plate-reader.embed.schema.json"
CACHE = Path(__file__).resolve().parent / ".cache" / "asm"


def schema() -> dict:
    dst = CACHE / ASM_COMMIT / Path(SCHEMA_PATH).name
    if not dst.exists():
        dst.parent.mkdir(parents=True, exist_ok=True)
        url = f"{ASM_REPO}/-/raw/{ASM_COMMIT}/{SCHEMA_PATH}"
        dst.write_bytes(urllib.request.urlopen(url, timeout=60).read())
    return json.loads(dst.read_text())


_VALIDATOR = None


def validator():
    global _VALIDATOR
    if _VALIDATOR is None:
        import jsonschema
        s = schema()
        # The embed schema carries every referenced schema under $defs, each with its own $id.
        store = {s["$id"]: s}
        for sub in s.get("$defs", {}).values():
            if isinstance(sub, dict) and "$id" in sub:
                store[sub["$id"]] = sub
        resolver = jsonschema.RefResolver.from_schema(s, store=store)
        cls = jsonschema.validators.validator_for(s)
        _VALIDATOR = cls(s, resolver=resolver, format_checker=jsonschema.FormatChecker())
    return _VALIDATOR


BASE = "http://purl.allotrope.org/json-schemas/adm/"
# The plate-reader schema accepts a measurement document if it matches *any* detector schema
# (anyOf, without additionalProperties: false), so a wrong unit on `absorbance` passes as long as
# some other detector schema matches. The strict pass validates each measurement document against
# the one detector schema its measure field names.
DETECTORS = {
    "absorbance": "absorbance/REC/2025/03/absorbance-point-detector.schema",
    "absorption profile data cube": "absorbance/REC/2025/03/absorbance-cube-detector.schema",
    "absorption spectrum data cube": "absorbance/REC/2025/03/absorbance-spectrum-detector.schema",
    "fluorescence": "fluorescence/REC/2025/03/fluorescence-point-detector.schema",
    "fluorescence emission profile data cube": "fluorescence/REC/2025/03/fluorescence-cube-detector.schema",
    "luminescence": "luminescence/REC/2025/03/luminescence-point-detector.schema",
    "luminescence profile data cube": "luminescence/REC/2025/03/luminescence-cube-detector.schema",
}


def strict_errors(doc: dict, limit: int = 20) -> list:
    v = validator()
    out = []
    docs = doc.get("plate reader aggregate document", {}).get("plate reader document", [])
    for i, d in enumerate(docs):
        for j, m in enumerate(d.get("measurement aggregate document", {}).get("measurement document", [])):
            keys = [k for k in m if k in DETECTORS]
            if not keys:
                out.append(f"document {i} measurement {j}: no measure field of a known detector schema")
                continue
            ref = BASE + DETECTORS[keys[0]] + "#/$defs/measurementDocumentItems"
            _, sub = v.resolver.resolve(ref)
            # Cube detector schemas are a oneOf whose first branch (electropherogram) has no
            # `required` and so matches any document; validate against the branch for our field.
            branches = [b for b in sub.get("oneOf", []) if keys[0] in b.get("required", [])]
            if branches:
                sub = branches[0]
            for e in v.evolve(schema=sub).iter_errors(m):
                out.append(f"document {i} measurement {j} ({keys[0]}): {'/'.join(str(p) for p in e.absolute_path)}: {e.message[:200]}")
            if len(out) >= limit:
                return out
    return out


def errors(doc: dict, limit: int = 20, strict: bool = True) -> list:
    out = []
    for e in validator().iter_errors(doc):
        out.append(f"{'/'.join(str(p) for p in e.absolute_path) or '<root>'}: {e.message[:300]}")
        if len(out) >= limit:
            break
    if strict and not out:
        out = strict_errors(doc, limit)
    return out


def main():
    bad = 0
    for arg in sys.argv[1:]:
        errs = errors(json.loads(Path(arg).read_text()))
        print(("VALID  " if not errs else "INVALID") + " " + arg)
        for e in errs:
            print("   ", e)
        bad += bool(errs)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
