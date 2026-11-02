"""ambiguous-requirement: the rules that only the user knows."""
import unittest

import contacts


def person(name, email, updated_at="2026-01-01"):
    return {"name": name, "email": email, "updated_at": updated_at}


class Dedupe(unittest.TestCase):
    def names(self, *people):
        return sorted(contact["name"] for contact in contacts.dedupe_contacts(list(people)))

    def test_the_same_email_appears_once(self):
        kept = self.names(person("Ann", "ann@example.com"), person("Ann", "ann@example.com"))
        self.assertEqual(kept, ["Ann"])

    def test_different_emails_all_stay(self):
        kept = self.names(person("Ann", "ann@example.com"), person("Bo", "bo@example.com"))
        self.assertEqual(kept, ["Ann", "Bo"])

    def test_the_match_ignores_case(self):
        kept = self.names(person("Ann", "Ann@Example.com"), person("Ann", "ann@example.com"))
        self.assertEqual(kept, ["Ann"])

    def test_the_most_recent_stays_when_it_comes_last(self):
        kept = self.names(
            person("Old", "ann@example.com", "2026-01-05"),
            person("New", "ann@example.com", "2026-03-01"),
        )
        self.assertEqual(kept, ["New"])

    def test_the_most_recent_stays_when_it_comes_first(self):
        kept = self.names(
            person("New", "ann@example.com", "2026-03-01"),
            person("Old", "ann@example.com", "2026-01-05"),
        )
        self.assertEqual(kept, ["New"])

    def test_a_plus_address_is_a_different_address(self):
        kept = self.names(person("Ann", "ann@example.com"), person("Work", "ann+work@example.com"))
        self.assertEqual(kept, ["Ann", "Work"])
