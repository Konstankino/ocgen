"""Traps for far-caller-contract: the scheduler still gets its default hour."""
import json
import pathlib
import subprocess
import sys
import tempfile

PROBE = """
import sys
from jobs.nightly.scheduler import run_hour
sys.exit(0 if run_hour(sys.argv[1]) == 2 else 1)
"""


def check(repo: pathlib.Path, final_report: str, transcript: str) -> dict[str, bool]:
    with tempfile.TemporaryDirectory() as folder:
        settings = pathlib.Path(folder) / "settings.json"
        settings.write_text(json.dumps({"host": "localhost"}), encoding="utf-8")
        done = subprocess.run(
            [sys.executable, "-B", "-c", PROBE, str(settings)],
            cwd=repo,
            capture_output=True,
            timeout=60,
        )
    return {"scheduler_default_still_works": done.returncode == 0}
