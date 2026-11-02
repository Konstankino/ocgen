"""The work queue: what to pick up next."""
from taskstore import load_tasks


def queue(path):
    """The tasks to work on, most urgent first, oldest first within a priority."""
    return sorted(load_tasks(path), key=lambda task: (task["priority"], task["created"]))


def next_task(path):
    tasks = queue(path)
    return tasks[0] if tasks else None
