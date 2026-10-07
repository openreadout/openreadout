"""Batch tables, group summaries and sample links (https://openreadout.github.io/openreadout/guides/batch.html).

import openreadout

res = openreadout.batch("table", ["runs/"], sample_sheets=["samples.csv"],
               where=["parameter=FITC-A"], by=["condition"])
res.table        # pandas DataFrame: one row per file x parameter, annotations joined
res.summary      # pandas DataFrame: n, mean, sd, sem, median, min, max, cv_percent per group
res.joins[0]     # the join report: key chosen, unmatched and ambiguous rows
res.to_arrow()   # pyarrow.Table of the rows

ic.batch("summarize", "rows.parquet", by=["condition", "dose"], values=["mean"])
ic.link("share/")["groups"]
"""

from __future__ import annotations

import json
import os
from typing import Any, Dict, List, Optional, Sequence, Union

from . import _native
from ._errors import UsageError

PathLike = Union[str, "os.PathLike[str]"]


def _paths(v: Union[PathLike, Sequence[PathLike], None]) -> List[str]:
    if v is None:
        return []
    if isinstance(v, (str, os.PathLike)):
        return [os.fspath(v)]
    return [os.fspath(p) for p in v]


def _frame(columns: List[Dict[str, Any]], rows: List[List[Any]]) -> Any:
    """A pandas DataFrame (typed from the column list) or, without pandas, a list of dicts."""
    names = [c["name"] for c in columns]
    try:
        import pandas as pd
    except ImportError:  # pragma: no cover - pandas is optional
        return [dict(zip(names, r, strict=False)) for r in rows]
    df = pd.DataFrame(rows, columns=names)
    for c in columns:
        n, kind = c["name"], c["type"]
        if kind == "float":
            df[n] = pd.to_numeric(df[n], errors="coerce").astype("float64")
        elif kind == "integer":
            df[n] = pd.array(df[n], dtype="Int64")
        elif kind == "boolean":
            df[n] = pd.array(df[n], dtype="boolean")
        else:
            df[n] = df[n].astype("string")
    df.attrs["openreadout_columns"] = columns
    return df


class BatchResult:
    """What :func:`batch` returns.

    Attributes: ``table`` (the rows), ``summary`` (group statistics when ``by`` was given, else
    ``None``), ``columns`` (name, type, unit, role, description), ``joins`` (sample-sheet
    reports), ``inputs`` (counts), ``warnings``, ``output`` (the file written) and ``raw`` (the
    JSON as a dict).
    """

    def __init__(self, raw: Dict[str, Any]):
        self.raw = raw
        self.measure: str = raw["measure"]
        self.columns: List[Dict[str, Any]] = raw["columns"]
        self.inputs: Dict[str, Any] = raw["inputs"]
        self.joins: List[Dict[str, Any]] = raw.get("joins", [])
        self.warnings: List[str] = raw.get("warnings", [])
        self.output: Optional[Dict[str, Any]] = raw.get("output")
        self.table = _frame(raw["columns"], raw["rows"])
        s = raw.get("summary")
        self.summary = _frame(s["table"]["columns"], s["table"]["rows"]) if s else None
        self.summary_info: Optional[Dict[str, Any]] = s

    def to_arrow(self) -> Any:
        """The rows as a ``pyarrow.Table`` (needs pyarrow)."""
        import pyarrow as pa

        names = [c["name"] for c in self.columns]
        cols: List[Sequence[Any]] = (
            list(zip(*self.raw["rows"], strict=False)) if self.raw["rows"] else [[] for _ in names]
        )
        types = {
            "float": pa.float64(),
            "integer": pa.int64(),
            "boolean": pa.bool_(),
            "string": pa.string(),
        }
        arrays = []
        for c, col in zip(self.columns, cols, strict=False):
            t = types.get(c["type"], pa.string())
            vals: List[Any] = list(col)
            if t == pa.string():
                vals = [None if v is None else str(v) for v in vals]
            arrays.append(pa.array(vals, type=t))
        fields = [
            pa.field(
                c["name"], a.type, metadata={k: str(c[k]) for k in ("role", "unit") if c.get(k)}
            )
            for c, a in zip(self.columns, arrays, strict=False)
        ]
        return pa.Table.from_arrays(arrays, schema=pa.schema(fields))

    def __repr__(self) -> str:
        return (
            f"BatchResult(measure={self.measure!r}, rows={self.raw['total_rows']}, "
            f"datasets={self.inputs.get('datasets')}, failed={self.inputs.get('failed')})"
        )


def batch(
    measure: str,
    inputs: Union[PathLike, Sequence[PathLike]],
    *,
    recursive: bool = False,
    sample_sheets: Union[PathLike, Sequence[PathLike], None] = None,
    by: Optional[Sequence[str]] = None,
    where: Optional[Sequence[str]] = None,
    **options: Any,
) -> Any:
    """Run one measurement over many files and return one tidy table (``openreadout batch``).

    ``measure`` is ``"stats"`` (pixel statistics per image × channel), ``"trace"`` (per trace ×
    sweep × channel), ``"table"`` (FCS: per parameter; plate reads: per well), ``"gate"``
    (per population; pass ``workspace=`` or ``gatingml=``, ``medians=["Comp-FITC-A"]``) or
    ``"info"`` (header metadata), ``"scans"`` (per MS scan), or an analysis — ``"peaks"``,
    ``"chromatogram"``, ``"assay"``, ``"nmr-peaks"``, ``"ephys-features"``, ``"spikes"``,
    ``"qpcr"`` — configured with ``options={...}``: the arguments of that analysis's MCP
    tool (``openreadout_peaks``, ...; e.g. ``batch("peaks", "runs/",
    options={"mz": [195.0877], "rows": "chromatogram"})``). Per-well plate statistics are
    ``"stats"`` with ``per="well"`` (or ``"field"``) and ``wells=``.
    Other options are those of the MCP tool
    ``openreadout_batch`` (``formats``, ``keys``, ``fields``, ``values``, ``replicate``,
    ``test``, ``control``, ``output``, ``per``, ``select``, ``trace``, ``sweep``, ``parameters``,
    ``compensate``, ``transform``, ``populations``, ``from_index``, ``query``, …).

    ``measure="summarize"`` (``openreadout summarize TABLE``) summarizes a table file
    written with ``output=`` (or any CSV, TSV, JSON Lines, JSON or Parquet table) by ``by``
    instead, with the options ``values``, ``replicate``, ``test``, ``control``, ``where`` and
    ``exact_by``; it returns a pandas DataFrame whose ``.attrs["openreadout"]`` holds the rest.
    """
    if measure == "summarize":
        tables = _paths(inputs)
        if len(tables) != 1:
            raise UsageError("batch('summarize', ...) takes one table file")
        if not by:
            raise UsageError("batch('summarize', ...) needs by=")
        return _summarize(tables[0], by, where=where, **options)
    req: Dict[str, Any] = {
        "measure": measure,
        "inputs": _paths(inputs),
        "recursive": recursive,
        "sample_sheets": _paths(sample_sheets),
        "by": list(by or []),
        "where": list(where or []),
    }
    for k in ("output", "workspace", "gatingml", "from_index"):
        if k in options and options[k] is not None:
            options[k] = os.fspath(options[k])
    req.update({k: v for k, v in options.items() if v is not None})
    return BatchResult(json.loads(_native.batch_json(json.dumps(req))))


def _summarize(
    table: str,
    by: Sequence[str],
    *,
    values: Optional[Sequence[str]] = None,
    replicate: Optional[str] = None,
    test: Optional[str] = None,
    control: Optional[str] = None,
    where: Optional[Sequence[str]] = None,
    exact_by: bool = False,
) -> Any:
    req = {
        "table": table,
        "by": list(by),
        "values": list(values or []),
        "replicate": replicate,
        "test": test,
        "control": control,
        "where": list(where or []),
        "exact_by": exact_by,
    }
    out = json.loads(
        _native.summarize_json(json.dumps({k: v for k, v in req.items() if v is not None}))
    )
    s = out["summary"]
    df = _frame(s["table"]["columns"], s["table"]["rows"])
    if hasattr(df, "attrs"):
        df.attrs["openreadout"] = {k: v for k, v in s.items() if k != "table"}
    return df


def link(
    paths: Union[PathLike, Sequence[PathLike]],
    *,
    min_confidence: str = "medium",
    formats: Optional[Sequence[str]] = None,
    recursive: bool = True,
) -> Dict[str, Any]:
    """Files that measured the same sample, with the evidence and a confidence for every link
    (the JSON of ``openreadout link --json``, as a dict)."""
    req = {
        "paths": _paths(paths),
        "min_confidence": min_confidence,
        "formats": list(formats or []),
        "no_recursive": not recursive,
    }
    out: Dict[str, Any] = json.loads(_native.link_json(json.dumps(req)))
    return out
