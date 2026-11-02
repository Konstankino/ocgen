"""Stores the team's tasks as JSON on disk."""
import json

DEFAULT_PRIORITY = 3


def load_tasks(path):
    """The stored tasks, each a dict with `title`, `created` (an ISO date) and `priority`.

    A task written before `priority` existed gets the default.
    """
    with open(path, encoding="utf-8") as handle:
        tasks = json.load(handle)
    for task in tasks:
        task.setdefault("priority", DEFAULT_PRIORITY)
    return tasks


def save_tasks(path, tasks):
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(tasks, handle, indent=2)


def add_task(path, title, created, priority=DEFAULT_PRIORITY):
    tasks = load_tasks(path)
    tasks.append({"title": title, "created": created, "priority": priority})
    save_tasks(path, tasks)
