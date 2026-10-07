"""Tests for the sweep's measurement and finding classification, without corpus data."""

import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import sweep


class Measurement(unittest.TestCase):
    def test_exit_status_and_peak_rss(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = sweep.execute([sys.executable, "-c", "raise SystemExit(4)"], Path(tmp), 10)
        self.assertEqual(r["exit"], 4)
        self.assertGreater(r["rss_bytes"], 0)

    def test_timeout_kills_process_group(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = sweep.execute([sys.executable, "-c", "import time; time.sleep(20)"], Path(tmp), 0.2)
        self.assertEqual(r["terminated"], "timeout")
        self.assertLess(r["seconds"], 5)

    def test_memory_monitor(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(sweep, "LIMIT", 1024):
            r = sweep.execute([sys.executable, "-c", "import time; time.sleep(20)"], Path(tmp), 10)
        self.assertEqual(r["terminated"], "memory")

    def test_findings(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            args = SimpleNamespace(
                binary=Path("/unused"),
                timeout=10,
                passed={"example"},
                results=folder / "results.jsonl",
            )
            entry = dict(id="example", filename="file", format="synthetic")
            a = {
                "exit": 4,
                "seconds": 1,
                "rss_bytes": 12,
                "terminated": None,
                "stdout": "",
                "stderr": "",
                "json": {"ok": False, "error": {"message": "bad"}},
            }
            with patch.object(sweep, "execute", side_effect=[dict(a), dict(a)]):
                sweep.pair(args, entry, "info", ["info"], folder)
            record = json.loads(args.results.read_text())
            self.assertEqual(record["findings"], ["missing_hint", "oracle_pass_exit_4_5"])


if __name__ == "__main__":
    unittest.main()
