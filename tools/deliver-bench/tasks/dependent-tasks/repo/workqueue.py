"""The work queue: what to pick up next."""
from taskstore import load_tasks


def queue(path):
    """The tasks to work on, oldest first."""
    return sorted(load_tasks(path), key=lambda task: task["created"])


def next_task(path):
    tasks = queue(path)
    return tasks[0] if tasks else None
