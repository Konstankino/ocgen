import unittest

from timeutil import parse_duration


class ParseDuration(unittest.TestCase):
    def test_a_single_unit(self):
        self.assertEqual(parse_duration("45s"), 45)

    def test_anything_else_is_an_error(self):
        with self.assertRaises(ValueError):
            parse_duration("soon")
