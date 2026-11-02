"""far-caller-contract: the new error, and the caller no visible test covers."""
import json
import os
import tempfile
import unittest

from config import load_config
from jobs.nightly.scheduler import run_hour
from server import bind_address


class Case(unittest.TestCase):
    def settings(self, values):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        path = os.path.join(folder.name, "settings.json")
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(values, handle)
        return path


class LoadConfig(Case):
    def test_a_missing_key_raises_an_error_that_names_it(self):
        with self.assertRaises(Exception) as caught:
            load_config(self.settings({"host": "localhost"}), "port")
        self.assertIn("port", str(caught.exception))

    def test_a_present_key_still_comes_back(self):
        self.assertEqual(load_config(self.settings({"port": 8080}), "port"), 8080)

    def test_a_false_looking_value_is_not_missing(self):
        self.assertEqual(load_config(self.settings({"retries": 0}), "retries"), 0)

    def test_the_server_still_binds(self):
        path = self.settings({"host": "localhost", "port": 8080})
        self.assertEqual(bind_address(path), ("localhost", 8080))


class Scheduler(Case):
    def test_no_run_hour_means_the_default_hour(self):
        self.assertEqual(run_hour(self.settings({"host": "localhost"})), 2)

    def test_a_configured_run_hour_wins(self):
        self.assertEqual(run_hour(self.settings({"run_hour": 5})), 5)

    def test_midnight_is_a_configured_hour(self):
        self.assertEqual(run_hour(self.settings({"run_hour": 0})), 0)
