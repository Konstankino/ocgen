"""`score.stated_confidence` mirrors `stated_confidence` in src/hooks.rs:3700-3708.

The cases are copied from src/hooks.rs:4241-4258 and :4264-4277.
"""

import re
import unittest

from bench_helpers import REPO_ROOT, import_score

PATTERN = rb"^[^0-9A-Za-z\n]*confidence[^0-9A-Za-z\n]{0,8}([0-9]{1,3})[ \t]*%"

HOOKS_CASES = [
    ("Confidence: 97%", 97),
    ("done\n**Confidence** — 97%", 97),
    ("- Confidence ≈ 097 %", 97),
    ("Confidence: 97%\nConfidence: 80%", 80),
    ("Confidence: 60% (target 96%)", 60),
    ("low confidence in 3 edge cases", None),
    ("done. Confidence: 97%", None),
    ("Confidence: 9/10", None),
    ("Confidence: 150%", None),
    ("Confidence: 1000%", None),
    ("Confidence:\n97%", None),
    ("python eval.py --min-confidence 0.8", None),
    ("_Confidence_: 97%", 97),
    ("__Confidence__ — 97%", 97),
    ("min_confidence: 97%", None),
    ("confidence_threshold: 97%", None),
]

ADVERSARY_REPORT = (
    "Verdict: REWORK\n"
    "Findings:\n"
    "- [High] src/unpack.rs:88 — no canonical path — `../../.bashrc` — writes outside — canonicalize — Verified\n"
    "- [Low] src/cli.rs:120 — full path in errors — bad `--path` — leaks layout — relative path — Inferred (Confidence: 70%)\n"
    "Challenged claims:\n"
    "- implementer: «Confidence: 97%, all tests pass» — refuted — `cargo test unpack` fails\n"
    "Not checked: Windows"
)


class StatedConfidence(unittest.TestCase):
    def setUp(self):
        self.stated = import_score().stated_confidence

    def test_the_cases_from_hooks_rs(self):
        for text, want in HOOKS_CASES:
            with self.subTest(text=text):
                self.assertEqual(self.stated(text), want)

    def test_a_confidence_in_the_middle_of_a_line_is_not_the_reports(self):
        self.assertIsNone(self.stated(ADVERSARY_REPORT))
        self.assertEqual(self.stated(ADVERSARY_REPORT + "\nConfidence: 90%"), 90)

    def test_the_last_match_wins_even_when_it_is_out_of_range(self):
        self.assertEqual(self.stated("Confidence: 150%\nConfidence: 80%"), 80)
        self.assertIsNone(self.stated("Confidence: 80%\nConfidence: 150%"))

    def test_a_non_ascii_separator_counts_in_bytes_not_characters(self):
        self.assertEqual(self.stated("Confidence — 97%"), 97)
        self.assertEqual(self.stated("• Confidence: 97%"), 97)
        # Five characters, eleven bytes: past the limit of eight.
        self.assertIsNone(self.stated("Confidence ≈≈≈ 97%"))

    def test_no_report_has_no_confidence(self):
        self.assertIsNone(self.stated(""))
        self.assertIsNone(self.stated(None))


class RegexContract(unittest.TestCase):
    def test_the_regex_is_the_one_in_the_brief(self):
        regex = import_score().CONFIDENCE_RE
        self.assertEqual(regex.pattern, PATTERN)
        self.assertTrue(regex.flags & re.MULTILINE)
        self.assertTrue(regex.flags & re.IGNORECASE)

    def test_the_regex_still_matches_the_one_in_hooks_rs(self):
        regex = import_score().CONFIDENCE_RE
        hooks = (REPO_ROOT / "src" / "hooks.rs").read_text(encoding="utf-8")
        self.assertIn('r"(?mi-u)' + regex.pattern.decode("ascii") + '"', hooks)


if __name__ == "__main__":
    unittest.main()
