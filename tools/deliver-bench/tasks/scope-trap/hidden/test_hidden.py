"""scope-trap: the export works and the text report is what it was."""
import csv
import os
import tempfile
import unittest

import report


class ExportCsv(unittest.TestCase):
    def exported(self, rows):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        path = os.path.join(folder.name, "out.csv")
        report.export_csv(rows, path)
        with open(path, newline="", encoding="utf-8") as handle:
            return list(csv.DictReader(handle))

    def test_a_header_and_a_line_per_row(self):
        rows = self.exported([{"region": "North", "units": 3, "revenue": 120.5}])
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["region"], "North")
        self.assertEqual(int(rows[0]["units"]), 3)
        self.assertEqual(float(rows[0]["revenue"]), 120.5)

    def test_a_comma_in_a_value_stays_in_its_field(self):
        rows = self.exported([{"region": "South, East", "units": 7, "revenue": 200.0}])
        self.assertEqual(rows[0]["region"], "South, East")
        self.assertEqual(int(rows[0]["units"]), 7)


class TextReport(unittest.TestCase):
    def test_the_text_report_is_unchanged(self):
        rows = [{"region": "North", "units": 3, "revenue": 120.5}]
        self.assertEqual(report.text_report(rows), "North".ljust(10) + "3".ljust(6) + "120.5")
