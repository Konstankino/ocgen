import contextlib
import io
import json
import os
import tempfile
import unittest

import report


class Report(unittest.TestCase):
    def test_it_orders_the_events_and_counts_them(self):
        text = report.build_report([{"name": "b", "ts": 2}, {"name": "a", "ts": 1}])
        self.assertEqual(text, "a\nb\n2 events")

    def test_main_prints_the_report_for_a_log(self):
        with tempfile.TemporaryDirectory() as folder:
            log = os.path.join(folder, "events.jsonl")
            with open(log, "w", encoding="utf-8") as handle:
                handle.write(json.dumps({"name": "a", "ts": 1}) + "\n")
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(report.main([log]), 0)
        self.assertIn("1 events", out.getvalue())
