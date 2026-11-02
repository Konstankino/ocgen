import unittest

from calendar_utils import is_leap_year


class LeapYears(unittest.TestCase):
    def test_every_fourth_year(self):
        self.assertTrue(is_leap_year(2024))

    def test_a_century_is_not_a_leap_year(self):
        self.assertFalse(is_leap_year(1900))
