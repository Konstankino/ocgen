import unittest

import report

ROWS = [
    {"region": "North", "units": 3, "revenue": 120.5},
    {"region": "South", "units": 7, "revenue": 200.0},
]


class TextReport(unittest.TestCase):
    def test_one_padded_line_per_row(self):
        north = "North".ljust(10) + "3".ljust(6) + "120.5"
        south = "South".ljust(10) + "7".ljust(6) + "200.00"
        self.assertEqual(report.text_report(ROWS), north + "\n" + south)
