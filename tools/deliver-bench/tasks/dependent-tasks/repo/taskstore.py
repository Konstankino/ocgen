"""Stores the team's tasks as JSON on disk."""
import json


def load_tasks(path):
    """The stored tasks, each a dict with `title` and `created` (an ISO date)."""
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def save_tasks(path, tasks):
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(tasks, handle, indent=2)


def add_task(path, title, created):
    tasks = load_tasks(path)
    tasks.append({"title": title, "created": created})
    save_tasks(path, tasks)
