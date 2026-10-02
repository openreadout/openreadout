#!/usr/bin/env python
"""Ground truth for Arbin MITS Pro `.res` result files (`arbin-res`) from access_parser
(Claroty, Apache-2.0, https://github.com/claroty/access_parser; `pip install access-parser==0.0.6`
in a separate venv, not the shared oracle environment), run as a black box on the Jet database.

Recorded, in `oracle/series_oracle.py`'s format (`crates/openreadout-corpus-tests/tests/
series_oracle/mod.rs` compares): every numeric column of `Channel_Normal_Table`, rows ordered by
test and data point and sampled at up to 256 points (matched to our channels by Arbin's column
label; the abscissa is `Test_Time`), each auxiliary input of `Auxiliary_Table` joined on the data
point (our `aux_voltage_<n>`/`aux_temperature_<n>` channels, n = Auxiliary_Index + 1),
`Channel_Statistic_Table` as table cells, and the test facts of `Global_Table` (test name,
operator, schedule, serial number, software version, start time).

With `--cellpy-golden raw.parquet` the sampled data points are also checked against cellpy's own
golden output for the same file (tests/data/goldens/loader_arbin_res of cellpy, MIT); the script
stops if the two readers disagree.

Usage:  python arbin_oracle.py --id ID FILE [--cellpy-golden PARQUET] [--out DIR]
"""
from __future__ import annotations

import datetime as dt
import json
import math
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "series"
ROWS = 256
EPOCH = dt.datetime(1899, 12, 30)


def pick(n: int) -> list[int]:
    return sorted({round(i * (n - 1) / (ROWS - 1)) for i in range(ROWS)}) if n > ROWS else list(range(n))


def ole_iso(days: float) -> str:
    t = EPOCH + dt.timedelta(milliseconds=round(days * 86_400_000))
    s = t.isoformat(timespec="milliseconds")
    return s[:-4] if s.endswith(".000") else s


def fnum(v) -> float | None:
    if v is None or v == "" or isinstance(v, (bytes, str)):
        return None
    x = float(v)
    return None if math.isnan(x) else x


def main(argv: list[str]) -> int:
    out_dir, ident, golden, files = OUT, None, None, []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        elif a == "--cellpy-golden":
            golden = Path(next(it))
        else:
            files.append(Path(a))
    if len(files) != 1 or not ident:
        print(__doc__, file=sys.stderr)
        return 2
    from access_parser import AccessParser  # type: ignore

    path = files[0]
    db = AccessParser(str(path))
    src = f"access_parser {path.name}"
    normal = db.parse_table("Channel_Normal_Table")
    n = len(normal["Data_Point"])
    order = sorted(range(n), key=lambda i: (normal["Test_ID"][i], normal["Data_Point"][i]))
    if len(set(normal["Test_ID"])) != 1:
        print("more than one test: not supported by this script", file=sys.stderr)
        return 1
    time = [float(normal["Test_Time"][i]) for i in order]
    idx = pick(n)
    traces = []
    for col, vals in normal.items():
        if col == "Test_ID":
            continue
        v = [fnum(vals[i]) for i in order]
        if all(x is None for x in v):
            continue
        samples = [[i, time[i], v[i]] for i in idx if v[i] is not None]
        traces.append({"trace": 0, "sweep": 0, "label": col, "n": n, "samples": samples,
                       "x_tol": 1e-9, "y_tol_rel": 1e-12, "y_tol_abs": 0.0, "source": src})
    # auxiliary inputs joined on the data point
    if "Auxiliary_Table" in db.catalog:
        aux = db.parse_table("Auxiliary_Table")
        if aux and isinstance(aux.get("Data_Point"), list) and aux["Data_Point"]:
            units = {}
            if "Aux_Global_Data_Table" in db.catalog:
                ag = db.parse_table("Aux_Global_Data_Table")
                if isinstance(ag.get("Data_Type"), list):
                    for t, k, u in zip(ag["Data_Type"], ag["Auxiliary_Index"], ag["Unit"]):
                        units[(t, k)] = u
            pos = {normal["Data_Point"][i]: k for k, i in enumerate(order)}
            chans: dict[tuple[int, int], dict[int, float]] = {}
            for r in range(len(aux["Data_Point"])):
                key = (aux["Data_Type"][r], aux["Auxiliary_Index"][r])
                k = pos.get(aux["Data_Point"][r])
                if k is not None:
                    chans.setdefault(key, {})[k] = float(aux["X"][r])
            for (ty, ix), vals in sorted(chans.items()):
                unit = (units.get((ty, ix)) or "").strip()
                if ty == 0 or unit == "V":
                    name = f"aux_voltage_{ix + 1}"
                elif ty == 1 or unit in ("C", "°C"):
                    name = f"aux_temperature_{ix + 1}"
                else:
                    continue
                samples = [[i, time[i], vals[i]] for i in idx if i in vals]
                traces.append({"trace": 0, "sweep": 0, "channel": name, "n": n, "samples": samples,
                               "x_tol": 1e-9, "y_tol_rel": 1e-12, "y_tol_abs": 0.0,
                               "source": f"{src} Auxiliary_Table (Data_Type {ty}, Auxiliary_Index {ix})"})
    tables = []
    stat = db.parse_table("Channel_Statistic_Table") if "Channel_Statistic_Table" in db.catalog else None
    if stat and isinstance(stat.get("Data_Point"), list) and stat["Data_Point"]:
        m = len(stat["Data_Point"])
        so = sorted(range(m), key=lambda i: (stat["Test_ID"][i], stat["Data_Point"][i]))
        cells = []
        names = {"Test_ID": "test", "Data_Point": "record", "Vmax_On_Cycle": "vmax_on_cycle",
                 "Charge_Time": "charge_time", "Discharge_Time": "discharge_time"}
        for r, i in enumerate(so):
            for col, ours in names.items():
                if col in stat:
                    x = fnum(stat[col][i])
                    if x is not None:
                        cells.append([r, ours, x])
        tables.append({"table": 0, "rows": m, "cells": cells, "tol_rel": 1e-12, "tol_abs": 0.0,
                       "source": f"{src} Channel_Statistic_Table"})
    g = db.parse_table("Global_Table")
    facts = []

    def text(col):
        v = g.get(col)
        return v[0].strip() if isinstance(v, list) and v and isinstance(v[0], str) and v[0].strip() else None

    for path_, col in (("sample.name", "Test_Name"), ("acquisition.operator", "Creator"),
                       ("method.name", "Schedule_File_Name"), ("instrument.serial", "Serial_Number"),
                       ("instrument.software_version", "Software_Version"),
                       ("acquisition.comment", "Comments")):
        v = text(col)
        if v:
            facts.append({"path": path_, "value": v, "source": f"{src} Global_Table {col}"})
    sd = g.get("Start_DateTime")
    if isinstance(sd, list) and sd:
        facts.append({"path": "acquisition.started_at", "value": ole_iso(float(sd[0])),
                      "source": f"{src} Global_Table Start_DateTime"})
    if golden is not None:
        import pandas as pd  # type: ignore

        gd = pd.read_parquet(golden)
        cols = {"Test_Time": "test_time", "Step_Time": "step_time", "Current": "current", "Voltage": "voltage",
                "Charge_Capacity": "charge_capacity", "Discharge_Capacity": "discharge_capacity",
                "Charge_Energy": "charge_energy", "Discharge_Energy": "discharge_energy",
                "Cycle_Index": "cycle_index", "Step_Index": "step_index", "Data_Point": "data_point",
                "dV/dt": "dv_dt", "Internal_Resistance": "internal_resistance"}
        if len(gd) != n:
            raise SystemExit(f"cellpy golden has {len(gd)} rows, access_parser {n}")
        for ours, theirs in cols.items():
            a = [float(normal[ours][i]) for i in order]
            b = gd[theirs].astype(float).tolist()
            worst = max(abs(x - y) / max(1.0, abs(y)) for x, y in zip(a, b))
            if worst > 1e-9:
                raise SystemExit(f"{ours}: access_parser and cellpy differ by {worst}")
        print(f"cellpy golden agrees with access_parser on {len(cols)} columns of {n} rows", file=sys.stderr)
    out = {"id": ident, "format": "arbin-res", "independent": True,
           "reader": "access_parser 0.0.6 (Apache-2.0) AccessParser.parse_table" + (
               "; data points also checked against cellpy's golden (MIT)" if golden else ""),
           "traces": traces, "tables": tables, "facts": facts}
    out_dir.mkdir(parents=True, exist_ok=True)
    dst = out_dir / f"{ident}.json"
    dst.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(dst, len(traces), "channels", len(facts), "facts")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
