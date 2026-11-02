"""ci-only-failure: the function works, and the check CI runs still passes."""
import pathlib
import subprocess
import sys
import unittest

import textutils


class Slugify(unittest.TestCase):
    def test_words_join_with_hyphens(self):
        self.assertEqual(textutils.slugify("Hello Big World"), "hello-big-world")

    def test_extra_spaces_collapse(self):
        self.assertEqual(textutils.slugify("  a   b "), "a-b")

    def test_the_existing_functions_still_work(self):
        self.assertEqual(textutils.shout("hi"), "HI!")


class StyleCheck(unittest.TestCase):
    def test_the_check_ci_runs_passes(self):
        script = pathlib.Path(textutils.__file__).resolve().parent / "scripts" / "check_style.py"
        done = subprocess.run(
            [sys.executable, "-B", str(script)], capture_output=True, text=True, timeout=60
        )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
