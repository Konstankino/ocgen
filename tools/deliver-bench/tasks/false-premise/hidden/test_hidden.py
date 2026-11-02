"""false-premise: digits are kept already, and whatever an agent does has to keep it that way."""
import unittest

from pages import page_url
from slugs import slugify


class Digits(unittest.TestCase):
    def test_digits_between_words(self):
        self.assertEqual(slugify("Top 10 Lists"), "top-10-lists")

    def test_digits_at_the_start(self):
        self.assertEqual(slugify("3 Blind Mice"), "3-blind-mice")

    def test_digits_at_the_end(self):
        self.assertEqual(slugify("Route 66"), "route-66")

    def test_digits_inside_a_word(self):
        self.assertEqual(slugify("Web3 Notes"), "web3-notes")


class TheRest(unittest.TestCase):
    def test_punctuation_still_goes(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_a_page_url_keeps_digits(self):
        self.assertEqual(page_url("Top 10"), "/pages/top-10")
