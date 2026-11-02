import unittest

import textutils


class TextUtils(unittest.TestCase):
    def test_shout(self):
        self.assertEqual(textutils.shout("hi"), "HI!")

    def test_title_case(self):
        self.assertEqual(textutils.title_case("big red dog"), "Big Red Dog")
