import os
import tempfile
import unittest

from taskstore import add_task, load_tasks, save_tasks
from workqueue import next_task, queue


class Tasks(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.path = os.path.join(folder.name, "tasks.json")
        save_tasks(self.path, [])

    def test_a_task_is_stored(self):
        add_task(self.path, "write docs", "2026-01-01")
        self.assertEqual(load_tasks(self.path)[0]["title"], "write docs")

    def test_the_queue_starts_with_the_oldest(self):
        add_task(self.path, "newer", "2026-01-02")
        add_task(self.path, "older", "2026-01-01")
        self.assertEqual([task["title"] for task in queue(self.path)], ["older", "newer"])

    def test_an_empty_queue_has_no_next_task(self):
        self.assertIsNone(next_task(self.path))
