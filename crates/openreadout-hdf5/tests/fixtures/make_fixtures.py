#!/usr/bin/env python
"""Write the SYNTHETIC HDF5 fixtures of openreadout-hdf5 with h5py (BSD-3).

    cd oracle && uv run python ../crates/openreadout-hdf5/tests/fixtures/make_fixtures.py
    ORACLE_OUT=../crates/openreadout-hdf5/tests/fixtures/oracle uv run python gen.py \
        ../crates/openreadout-hdf5/tests/fixtures/*.ims ../crates/openreadout-hdf5/tests/fixtures/*.nwb

None of these files comes from an instrument or from Imaris / pynwb:
- synthetic-imaris.ims follows the Imaris layout observed in the OME sample files
  (docs/formats/ims.md): attributes as arrays of one-character strings, volumes padded to whole
  chunks, gzip + shuffle chunks, 2 time points, 2 channels, 2 resolution levels;
- synthetic-timeseries.nwb follows the NWB 2 schema for an NWBFile with three TimeSeries under
  acquisition/: starting_time + rate with 2-D int16 data (conversion, offset), uniform
  timestamps, irregular timestamps; plus an ElectricalSeries without a time base;
- synthetic-ecephys.nwb follows the NWB 2 / HDMF-common schemas for extracellular data: an
  electrodes DynamicTable (numeric, boolean, text and object-reference columns), an
  ElectricalSeries with an electrodes region and channel_conversion, an LFP ElectricalSeries in a
  processing module, a units table (ragged spike_times and electrodes, a 2-D waveform_mean, a
  text column), a 3-D SpikeEventSeries and a trials TimeIntervals table;
- synthetic-generic.h5 is a plain HDF5 file with nested groups and attributes.
"""
from pathlib import Path

import h5py
import numpy as np

HERE = Path(__file__).resolve().parent


def chars(s: str):
    """Imaris-style attribute: an array of one-character strings."""
    return np.array([c.encode() for c in s], dtype="S1")


def imaris(path: Path):
    X, Y, Z, C, T = 37, 21, 5, 2, 2
    with h5py.File(path, "w") as f:
        for k, v in {"DataSetDirectoryName": "DataSet", "DataSetInfoDirectoryName": "DataSetInfo",
                     "ImarisDataSet": "ImarisDataSet", "ImarisVersion": "5.5.0",
                     "ThumbnailDirectoryName": "Thumbnail"}.items():
            f.attrs[k] = chars(v)
        f.attrs["NumberOfDataSets"] = np.array([1], dtype=np.uint32)
        info = f.create_group("DataSetInfo")
        img = info.create_group("Image")
        for k, v in {"X": str(X), "Y": str(Y), "Z": str(Z), "Unit": "um", "ExtMin0": "0", "ExtMax0": str(X * 0.5),
                     "ExtMin1": "10", "ExtMax1": str(10 + Y * 0.5), "ExtMin2": "-1", "ExtMax2": str(-1 + Z * 2.0),
                     "Name": "synthetic", "RecordingDate": "2026-01-02 03:04:05.678", "LensPower": "40",
                     "NumericalAperture": "1.3"}.items():
            img.attrs[k] = chars(v)
        for c, (name, col, ex, em) in enumerate([("DAPI", "0.000 0.000 1.000", "405", "461"), ("GFP", "0.000 1.000 0.000", "488", "509")]):
            g = info.create_group(f"Channel {c}")
            for k, v in {"Name": name, "Color": col, "LSMExcitationWavelength": ex + " nm", "LSMEmissionWavelength": em + " nm"}.items():
                g.attrs[k] = chars(v)
        ti = info.create_group("TimeInfo")
        ti.attrs["DatasetTimePoints"] = chars(str(T))
        ti.attrs["TimePoint1"] = chars("2026-01-02 03:04:05.678")
        ti.attrs["TimePoint2"] = chars("2026-01-02 03:04:08.178")
        levels = [(X, Y, Z, (4, 16, 16), (8, 32, 48)), ((X + 1) // 2, (Y + 1) // 2, Z, (4, 16, 16), (8, 16, 32))]
        for l, (x, y, z, chunk, stored) in enumerate(levels):
            for t in range(T):
                for c in range(C):
                    g = f.create_group(f"DataSet/ResolutionLevel {l}/TimePoint {t}/Channel {c}")
                    for k, v in {"ImageSizeX": str(x), "ImageSizeY": str(y), "ImageSizeZ": str(z)}.items():
                        g.attrs[k] = chars(v)
                    data = np.zeros(stored, dtype=np.uint16)
                    zz, yy, xx = np.indices((z, y, x))
                    data[:z, :y, :x] = (xx * 3 + yy * 101 + zz * 1000 + c * 7 + t * 13 + l * 17) % 65536
                    g.create_dataset("Data", data=data, chunks=chunk, compression="gzip", shuffle=True)
        f.create_dataset("Thumbnail/Data", data=np.zeros((8, 32), dtype=np.uint8))


def nwb(path: Path):
    with h5py.File(path, "w") as f:
        f.attrs["neurodata_type"] = "NWBFile"
        f.attrs["namespace"] = "core"
        f.attrs["nwb_version"] = "2.7.0"
        f["session_description"] = "synthetic session for openreadout tests"
        f["identifier"] = "synthetic-0001"
        f["session_start_time"] = "2026-01-02T03:04:05.000000+01:00"
        f["timestamps_reference_time"] = "2026-01-02T03:04:05.000000+01:00"
        f.create_dataset("file_create_date", data=[b"2026-01-02T04:00:00+01:00"])
        g = f.create_group("general")
        g["lab"] = "Test Lab"
        g["institution"] = "Nowhere"
        acq = f.create_group("acquisition")
        # regular: starting_time + rate, 2 columns of int16 with conversion and offset
        ts = acq.create_group("voltage")
        ts.attrs["neurodata_type"] = "TimeSeries"
        ts.attrs["namespace"] = "core"
        ts.attrs["description"] = "two channels"
        n = 1000
        data = (np.arange(n * 2).reshape(n, 2) * 7 % 2000 - 1000).astype(np.int16)
        d = ts.create_dataset("data", data=data, chunks=(250, 2), compression="gzip")
        d.attrs["unit"] = "volts"
        d.attrs["conversion"] = 0.000195
        d.attrs["offset"] = -0.01
        d.attrs["resolution"] = -1.0
        st = ts.create_dataset("starting_time", data=1.5)
        st.attrs["rate"] = 2000.0
        st.attrs["unit"] = "seconds"
        # uniform timestamps
        u = acq.create_group("temperature")
        u.attrs["neurodata_type"] = "TimeSeries"
        d = u.create_dataset("data", data=np.linspace(20, 25, 50, dtype=np.float32))
        d.attrs["unit"] = "Celsius"
        d.attrs["conversion"] = 1.0
        u.create_dataset("timestamps", data=2.0 + np.arange(50) * 0.25)
        # irregular timestamps, nested under a container
        be = acq.create_group("events")
        be.attrs["neurodata_type"] = "BehavioralEvents"
        ir = be.create_group("licks")
        ir.attrs["neurodata_type"] = "TimeSeries"
        d = ir.create_dataset("data", data=np.ones(6, dtype=np.uint8))
        d.attrs["unit"] = "n/a"
        ir.create_dataset("timestamps", data=np.array([0.1, 0.4, 0.45, 1.2, 3.0, 3.01]))
        # listed only
        es = acq.create_group("ephys")
        es.attrs["neurodata_type"] = "ElectricalSeries"
        es.create_dataset("data", data=np.zeros((10, 4), dtype=np.int16))


def _table(g, desc, colnames, **cols):
    g.attrs["neurodata_type"] = g.attrs.get("neurodata_type", "DynamicTable")
    g.attrs["namespace"] = "hdmf-common"
    g.attrs["description"] = desc
    g.attrs["colnames"] = np.array(colnames, dtype=object)
    for name, data in cols.items():
        g.create_dataset(name, data=data)


def ecephys(path: Path):
    with h5py.File(path, "w") as f:
        f.attrs["neurodata_type"] = "NWBFile"
        f.attrs["namespace"] = "core"
        f.attrs["nwb_version"] = "2.7.0"
        f["session_description"] = "synthetic extracellular session for openreadout tests"
        f["identifier"] = "synthetic-ecephys-0001"
        f["session_start_time"] = "2026-01-02T03:04:05.000000+01:00"
        f["timestamps_reference_time"] = "2026-01-02T03:04:05.000000+01:00"
        f.create_dataset("file_create_date", data=[b"2026-01-02T04:00:00+01:00"])
        ex = f.create_group("general/extracellular_ephys")
        grp = ex.create_group("shank0")
        grp.attrs["neurodata_type"] = "ElectrodeGroup"
        el = ex.create_group("electrodes")
        el.attrs["neurodata_type"] = "DynamicTable"
        str_t = h5py.string_dtype()
        _table(el, "metadata about extracellular electrodes", ["x", "location", "good", "group", "group_name"],
               id=np.arange(4, dtype=np.int64) + 10,
               x=np.array([0.0, 20.0, 40.0, 60.0]),
               location=np.array(["CA1", "CA1", "CA3", "CA1"], dtype=str_t),
               good=np.array([True, False, True, True]),
               group_name=np.array(["shank0"] * 4, dtype=str_t))
        el.create_dataset("group", data=[grp.ref] * 4, dtype=h5py.ref_dtype)
        acq = f.create_group("acquisition")
        es = acq.create_group("raw")
        es.attrs["neurodata_type"] = "ElectricalSeries"
        es.attrs["namespace"] = "core"
        n = 300
        data = ((np.arange(n * 3).reshape(n, 3) * 37) % 4001 - 2000).astype(np.int16)
        d = es.create_dataset("data", data=data)
        d.attrs["unit"] = "volts"
        d.attrs["conversion"] = 1e-6
        d.attrs["offset"] = 0.0
        es.create_dataset("electrodes", data=np.array([3, 0, 2], dtype=np.int64))
        es.create_dataset("channel_conversion", data=np.array([1.0, 0.5, 2.0], dtype=np.float32))
        st = es.create_dataset("starting_time", data=0.25)
        st.attrs["rate"] = 30000.0
        pm = f.create_group("processing/ecephys")
        pm.attrs["neurodata_type"] = "ProcessingModule"
        lfp = pm.create_group("LFP")
        lfp.attrs["neurodata_type"] = "LFP"
        ls = lfp.create_group("LFP")
        ls.attrs["neurodata_type"] = "ElectricalSeries"
        d = ls.create_dataset("data", data=np.linspace(-1, 1, 40 * 2, dtype=np.float32).reshape(40, 2))
        d.attrs["unit"] = "volts"
        d.attrs["conversion"] = 0.001
        ls.create_dataset("electrodes", data=np.array([0, 1], dtype=np.int64))
        st = ls.create_dataset("starting_time", data=0.0)
        st.attrs["rate"] = 1000.0
        un = f.create_group("units")
        un.attrs["neurodata_type"] = "Units"
        _table(un, "sorted units", ["spike_times", "electrodes", "waveform_mean", "quality"],
               id=np.array([7, 8, 9], dtype=np.int64),
               spike_times=np.array([0.1, 0.5, 0.9, 1.2, 0.3, 2.5]),
               spike_times_index=np.array([3, 4, 6], dtype=np.uint32),
               electrodes=np.array([0, 1, 3, 2], dtype=np.int64),
               electrodes_index=np.array([2, 3, 4], dtype=np.uint32),
               waveform_mean=np.arange(3 * 5, dtype=np.float64).reshape(3, 5) * 1e-6,
               quality=np.array(["good", "noise", "good"], dtype=str_t))
        un.attrs["neurodata_type"] = "Units"
        sev = f.create_group("processing/ecephys/spikes")
        sev.attrs["neurodata_type"] = "SpikeEventSeries"
        d = sev.create_dataset("data", data=(np.arange(4 * 2 * 3).reshape(4, 2, 3) - 10).astype(np.int16))
        d.attrs["unit"] = "volts"
        d.attrs["conversion"] = 2e-6
        sev.create_dataset("timestamps", data=np.array([0.01, 0.02, 0.5, 0.75]))
        sev.create_dataset("electrodes", data=np.array([1, 2], dtype=np.int64))
        tr = f.create_group("intervals/trials")
        tr.attrs["neurodata_type"] = "TimeIntervals"
        _table(tr, "trials", ["start_time", "stop_time", "outcome"],
               id=np.arange(2, dtype=np.int64),
               start_time=np.array([0.0, 1.0]), stop_time=np.array([0.9, 1.9]),
               outcome=np.array(["hit", "miss"], dtype=str_t))


def generic(path: Path):
    with h5py.File(path, "w") as f:
        f.attrs["title"] = "plain HDF5"
        f.attrs["values"] = np.array([1.5, 2.5])
        g = f.create_group("results/run1")
        g.attrs["operator"] = "nobody"
        g.create_dataset("matrix", data=np.arange(12, dtype=np.float32).reshape(3, 4))
        f.create_dataset("labels", data=[b"a", b"bb"])


if __name__ == "__main__":
    imaris(HERE / "synthetic-imaris.ims")
    nwb(HERE / "synthetic-timeseries.nwb")
    ecephys(HERE / "synthetic-ecephys.nwb")
    generic(HERE / "synthetic-generic.h5")
    print("wrote", *(p.name for p in sorted(HERE.glob("synthetic-*"))))
