import unittest

from pages import page_url
from slugs import slugify


class Slugs(unittest.TestCase):
    def test_words_join_with_hyphens(self):
        self.assertEqual(slugify("Hello World"), "hello-world")

    def test_punctuation_goes(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_a_page_url_uses_the_slug(self):
        self.assertEqual(page_url("About Us"), "/pages/about-us")
