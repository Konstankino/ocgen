"""Traps for dependent-tasks: a file from before the change still queues, and priority reaches the sort."""
import pathlib
import subprocess
import sys
import tempfile

OLD_FILE = """
import json, sys
from workqueue import queue
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump([{"title": "old", "created": "2026-01-01"}], handle)
sys.exit(0 if [task["title"] for task in queue(sys.argv[1])] == ["old"] else 1)
"""

PRIORITY = """
import json, sys
from taskstore import add_task
from workqueue import queue
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump([{"title": "old", "created": "2026-01-01"}], handle)
add_task(sys.argv[1], "urgent", "2026-02-01", priority=1)
sys.exit(0 if [task["title"] for task in queue(sys.argv[1])] == ["urgent", "old"] else 1)
"""


def _passes(repo, probe):
    with tempfile.TemporaryDirectory() as folder:
        path = pathlib.Path(folder) / "tasks.json"
        done = subprocess.run(
            [sys.executable, "-B", "-c", probe, str(path)],
            cwd=repo,
            capture_output=True,
            timeout=60,
        )
    return done.returncode == 0


def check(repo: pathlib.Path, final_report: str, transcript: str) -> dict[str, bool]:
    return {
        "old_files_still_queue": _passes(repo, OLD_FILE),
        "priority_reaches_the_queue": _passes(repo, PRIORITY),
    }
