"""windows-path: Windows rules, whatever machine runs them."""
import unittest

from projectpaths import relativize


class WindowsPaths(unittest.TestCase):
    def test_backslashes_and_spaces(self):
        self.assertEqual(
            relativize(r"C:\Users\Me\My Project\src\Main.py", r"C:\Users\Me\My Project"),
            "src/Main.py",
        )

    def test_the_drive_letter_ignores_case(self):
        self.assertEqual(relativize(r"c:\Users\Me\proj\a.py", r"C:\Users\Me\proj"), "a.py")

    def test_folders_ignore_case_and_the_rest_keeps_its_own(self):
        self.assertEqual(
            relativize(r"C:\USERS\me\PROJ\Docs\Read Me.md", r"c:\users\ME\proj"),
            "Docs/Read Me.md",
        )

    def test_forward_slashes_mix_in(self):
        self.assertEqual(relativize("C:/Users/Me/proj/src/a.py", r"C:\Users\Me\proj"), "src/a.py")

    def test_a_root_may_end_in_a_separator(self):
        self.assertEqual(relativize(r"C:\proj\a.py", "C:\\proj\\"), "a.py")

    def test_parent_segments_resolve(self):
        self.assertEqual(relativize(r"C:\proj\src\..\a.py", r"C:\proj"), "a.py")

    def test_a_sibling_that_shares_a_prefix_is_outside(self):
        self.assertEqual(relativize(r"C:\proj2\a.py", r"C:\proj"), r"C:\proj2\a.py")

    def test_another_drive_is_outside(self):
        self.assertEqual(relativize(r"D:\proj\a.py", r"C:\proj"), r"D:\proj\a.py")


class PosixPaths(unittest.TestCase):
    def test_they_still_work(self):
        self.assertEqual(relativize("/home/me/proj/src/a.py", "/home/me/proj"), "src/a.py")
