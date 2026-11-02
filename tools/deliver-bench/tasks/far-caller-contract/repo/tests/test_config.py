import json
import os
import tempfile
import unittest

from config import load_config
from server import bind_address


class Config(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.path = os.path.join(folder.name, "settings.json")
        with open(self.path, "w", encoding="utf-8") as handle:
            json.dump({"host": "localhost", "port": 8080}, handle)

    def test_it_reads_a_value(self):
        self.assertEqual(load_config(self.path, "host"), "localhost")

    def test_the_server_binds_to_what_the_file_says(self):
        self.assertEqual(bind_address(self.path), ("localhost", 8080))
