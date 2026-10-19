#!/usr/bin/env python
"""The committed TDMS fixture (`crates/openreadout-tdms/tests/fixtures/mini.tdms`), written with
npTDMS (LGPL-3.0, used as a black-box writer): two segments; group `Sensors` with two float64
channels sampled every 0.5 s (a unit, a linear scale on the second); group `Log` with an int16
channel, a timestamp channel and a string channel; file properties naming a writer.

Usage:  python make_tdms_fixtures.py
"""
from __future__ import annotations

from pathlib import Path

import numpy as np

OUT = Path(__file__).resolve().parent.parent / "crates" / "openreadout-tdms" / "tests" / "fixtures"


def main() -> None:
    from nptdms import ChannelObject, GroupObject, RootObject, TdmsWriter

    OUT.mkdir(parents=True, exist_ok=True)
    root = RootObject(properties={"WriterName": "openreadout fixture", "title": "mini"})
    sensors = GroupObject("Sensors")
    log = GroupObject("Log", properties={"operator": "nobody"})
    scaled = {
        "unit_string": "mV",
        "wf_increment": 0.5,
        "NI_Scaling_Status": "unscaled",
        "NI_Number_Of_Scales": np.int32(2),
        "NI_Scale[1]_Scale_Type": "Linear",
        "NI_Scale[1]_Linear_Slope": 2.0,
        "NI_Scale[1]_Linear_Y_Intercept": 1.0,
        "NI_Scale[1]_Linear_Input_Source": np.int32(0),
    }
    with TdmsWriter(OUT / "mini.tdms") as w:
        for part in range(2):
            a = np.arange(4, dtype=np.float64) + 4 * part
            w.write_segment([
                root, sensors, log,
                ChannelObject("Sensors", "temperature", a * 0.25, properties={"unit_string": "degC", "wf_increment": 0.5}),
                ChannelObject("Sensors", "voltage", a, properties=scaled),
                ChannelObject("Log", "counts", (a * 10 - 5).astype(np.int16)),
                ChannelObject("Log", "time", np.array(["2024-01-02T03:04:05.5", "2024-01-02T03:04:06", "2024-01-02T03:04:06.5",
                                                        "2024-01-02T03:04:07"], dtype="datetime64[us]") + np.timedelta64(2 * part, "s")),
                ChannelObject("Log", "notes", [f"note {i + 4 * part}" for i in range(4)]),
            ])
    print("wrote", OUT / "mini.tdms")


if __name__ == "__main__":
    main()
