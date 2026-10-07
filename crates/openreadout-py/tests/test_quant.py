"""File.analyze("chromatogram") and File.analyze("peaks"): the same options as the MCP tool."""

import numpy as np
import pytest
from openreadout import File, UsageError


def test_xic_and_peaks(corpus):
    with File(corpus("mtbls20-caffeine-pos.raw")) as f:
        out = f.analyze("chromatogram", tic=True, mz=[195.0877], ppm=10, max_points=20000)
        _tic, xic = out["chromatograms"]
        assert isinstance(xic["rt_min"], np.ndarray) and len(xic["rt_min"]) == 47
        assert xic["kind"] == "xic" and xic["centroid_scans"] == 47
        assert xic["apex_rt_min"] == pytest.approx(0.6129, abs=1e-3)
        res = f.analyze("peaks", mz=[195.0877], rt=0.6, window=0.3)
        peak = res["chromatograms"][0]["picked"]["peak"]
        assert peak["rt_min"] == pytest.approx(0.62, abs=0.02)
        compounds = [{"name": "caffeine", "mz": 195.0877, "rt": 0.6}]
        rows = f.analyze("peaks", compounds=compounds)["compounds"]
        assert rows[0]["found"] and rows[0]["area"] == pytest.approx(peak["area"])


def test_detector_trace_and_errors(corpus):
    with File(corpus("chromhandler-001F0101.D")) as f:
        res = f.analyze("peaks", traces=[0], area_seconds=True, min_snr=10)
        c = res["chromatograms"][0]
        assert c["area_unit"] == "pA·s" and 97 < c["main_peak_area_percent"] < 99
        with pytest.raises(UsageError):
            f.analyze("peaks", baseline="bogus")


def _mzxml_without_rt(path):
    """Two MS1 scans: scan 1 at 60 s, scan 2 with no ``retentionTime`` attribute."""
    import base64
    import struct

    def peaks(pairs):
        vals = [v for p in pairs for v in p]
        return base64.b64encode(struct.pack(f">{len(vals)}f", *vals)).decode()

    scans = [
        (1, ' retentionTime="PT60S"', [(100.0, 2.0), (200.0, 3.0)]),
        (2, "", [(100.0, 5.0)]),
    ]
    body = "".join(
        f'<scan num="{n}" msLevel="1" peaksCount="{len(p)}" polarity="+" centroided="1"{rt}>'
        f'<peaks precision="32" byteOrder="network" pairOrder="m/z-int">{peaks(p)}</peaks></scan>'
        for n, rt, p in scans
    )
    path.write_text(
        '<?xml version="1.0" encoding="ISO-8859-1"?>\n'
        '<mzXML xmlns="http://sashimi.sourceforge.net/schema_revision/mzXML_3.2">'
        f'<msRun scanCount="2">{body}</msRun></mzXML>\n'
    )
    return path


def test_absent_retention_time_is_none_and_nan(tmp_path):
    """A scan whose file states no retention time: ``None`` in documents, NaN in float columns."""
    import openreadout

    path = _mzxml_without_rt(tmp_path / "no-rt.mzXML")
    with File(path) as f:
        scans = f.scans()["scans"]
        assert [s["scan_number"] for s in scans] == [1, 2]
        assert scans[0]["rt_s"] == pytest.approx(60.0) and scans[1]["rt_s"] is None
        assert f.read_spectrum(index=1)["rt_s"] is None
        assert f.read_spectrum(scan=1)["rt_s"] == pytest.approx(60.0)
        pa = pytest.importorskip("pyarrow")
        tab = f.to_arrow(spectra=True)
        assert tab.schema.field("rt_s").type == pa.float64()
        assert tab.column("rt_s").to_pylist() == [60.0, 60.0, None]
        per_scan = f.to_arrow(spectra=True, per_scan=True)
        assert per_scan.column("rt_s").to_pylist() == [60.0, None]
        pytest.importorskip("pandas")
        df = f.to_pandas(spectra=True)
        assert df["rt_s"].dtype == np.float64 and df["rt_s"].isna().tolist() == [False, False, True]
        # chromatograms leave the scan out (never placed at 0) and say so
        tic = f.analyze("chromatogram", tic=True)["chromatograms"][0]
        assert tic["rt_min"].tolist() == [1.0]
        assert any("no retention time" in n for n in tic["notes"])
    res = openreadout.batch("scans", path)
    rt = res.table["rt_min"]
    assert rt.dtype == np.float64 and rt.iloc[0] == pytest.approx(1.0) and np.isnan(rt.iloc[1])
