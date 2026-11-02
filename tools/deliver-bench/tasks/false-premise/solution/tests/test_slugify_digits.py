import unittest

from slugs import slugify


class Digits(unittest.TestCase):
    def test_digits_are_kept(self):
        self.assertEqual(slugify("Top 10 Lists"), "top-10-lists")
