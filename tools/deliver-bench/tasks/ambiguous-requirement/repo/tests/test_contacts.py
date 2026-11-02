import json
import os
import tempfile
import unittest

import contacts

PEOPLE = [
    {"name": "Bo", "email": "bo@example.com", "updated_at": "2026-01-01"},
    {"name": "Al", "email": "al@example.com", "updated_at": "2026-02-01"},
]


class Contacts(unittest.TestCase):
    def test_display_names_are_sorted(self):
        self.assertEqual(contacts.display_names(PEOPLE), ["Al", "Bo"])

    def test_it_loads_a_file(self):
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "contacts.json")
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(PEOPLE, handle)
            self.assertEqual(contacts.load_contacts(path), PEOPLE)
