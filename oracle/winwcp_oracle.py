#!/usr/bin/env python
"""WinWCP ground truth for files whose recording time Neo cannot parse: `gen.py`'s `winwcp()`
oracle (Neo's WinWcpRawIO, BSD-3) with one difference. Neo 0.14.5 reads the file header's `RTIME`
with the pattern `%d/%m/%Y %H:%M:%S` and raises when a file stores something else (an empty
string, or a time without a date such as `17:24:56.377`), before it reads any record. Here that
one parse returns no date instead of raising. Neo's recording time is not part of the oracle
(`winwcp()` records signals, names, units and the sample rate), so the output has the same shape
and meaning as for any other WinWCP file.

Usage (the shared oracle venv):
    oracle/.venv/bin/python oracle/winwcp_oracle.py --id ID corpus/files/ID.wcp [--id ID2 PATH2 ...]

Writes `corpus/oracle/<ID>.json`, as `gen.py` does.
"""
from __future__ import annotations

import datetime as _dt
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))


class _LenientDatetime(_dt.datetime):
    @classmethod
    def strptime(cls, s, fmt):  # type: ignore[override]
        try:
            return _dt.datetime.strptime(s, fmt)
        except ValueError:
            return None


class _DatetimeModule:
    datetime = _LenientDatetime


def main() -> None:
    from neo.rawio import winwcprawio
    winwcprawio.datetime = _DatetimeModule  # only the RTIME parse uses it
    import gen
    gen.main()


if __name__ == "__main__":
    main()
