import unittest

from projectpaths import relativize


class Relativize(unittest.TestCase):
    def test_a_file_inside_the_project(self):
        self.assertEqual(relativize("/home/me/proj/src/a.py", "/home/me/proj"), "src/a.py")

    def test_a_file_outside_the_project_comes_back_unchanged(self):
        self.assertEqual(relativize("/elsewhere/a.py", "/home/me/proj"), "/elsewhere/a.py")
