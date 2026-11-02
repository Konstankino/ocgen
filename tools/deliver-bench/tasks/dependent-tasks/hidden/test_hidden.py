"""dependent-tasks: the field, files from before it, and the sort that needs both. Old files are checked
through `queue`, so it doesn't matter where the migration happens."""
import json
import os
import tempfile
import unittest

from taskstore import add_task
from workqueue import next_task, queue

OLD_TASKS = [
    {"title": "old-newer", "created": "2026-01-02"},
    {"title": "old-older", "created": "2026-01-01"},
]


class Case(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.path = os.path.join(folder.name, "tasks.json")

    def write(self, tasks):
        with open(self.path, "w", encoding="utf-8") as handle:
            json.dump(tasks, handle)

    def on_disk(self):
        with open(self.path, encoding="utf-8") as handle:
            return json.load(handle)

    def titles(self):
        return [task["title"] for task in queue(self.path)]


class Priority(Case):
    def test_add_task_stores_the_priority(self):
        self.write([])
        add_task(self.path, "urgent", "2026-01-01", priority=1)
        self.assertEqual(self.on_disk()[0]["priority"], 1)

    def test_add_task_defaults_to_three(self):
        self.write([])
        add_task(self.path, "normal", "2026-01-01")
        self.assertEqual(self.on_disk()[0]["priority"], 3)


class OldFiles(Case):
    def test_old_tasks_count_as_priority_three(self):
        self.write(
            OLD_TASKS
            + [
                {"title": "higher", "created": "2026-03-01", "priority": 2},
                {"title": "lower", "created": "2025-12-01", "priority": 4},
            ]
        )
        self.assertEqual(self.titles(), ["higher", "old-older", "old-newer", "lower"])

    def test_old_tasks_keep_their_fields(self):
        self.write(OLD_TASKS)
        oldest = queue(self.path)[0]
        self.assertEqual(oldest["title"], "old-older")
        self.assertEqual(oldest["created"], "2026-01-01")

    def test_adding_to_an_old_file_keeps_its_tasks(self):
        self.write(OLD_TASKS)
        add_task(self.path, "new", "2026-02-01", priority=1)
        self.assertEqual(self.titles(), ["new", "old-older", "old-newer"])


class Queue(Case):
    def test_the_most_urgent_comes_first(self):
        self.write(
            [
                {"title": "low", "created": "2026-01-01", "priority": 5},
                {"title": "high", "created": "2026-01-03", "priority": 1},
            ]
        )
        self.assertEqual(self.titles(), ["high", "low"])

    def test_a_tie_goes_to_the_oldest(self):
        self.write(
            [
                {"title": "newer", "created": "2026-01-03", "priority": 2},
                {"title": "older", "created": "2026-01-01", "priority": 2},
            ]
        )
        self.assertEqual(self.titles(), ["older", "newer"])

    def test_an_old_file_still_queues(self):
        self.write(OLD_TASKS)
        self.assertEqual(self.titles(), ["old-older", "old-newer"])

    def test_the_next_task_is_the_most_urgent(self):
        self.write(
            [
                {"title": "low", "created": "2026-01-01", "priority": 5},
                {"title": "high", "created": "2026-01-03", "priority": 1},
            ]
        )
        self.assertEqual(next_task(self.path)["title"], "high")
