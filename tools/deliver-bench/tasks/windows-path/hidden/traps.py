"""Traps for windows-path: `ntpath` semantics hold on a machine whose `os.path` is POSIX."""
import json
import pathlib
import subprocess
import sys

PROBES = [
    [r"C:\Users\Me\My Project\src\Main.py", r"c:\users\me\my project", "src/Main.py"],
    [r"C:\proj\src\..\a.py", r"C:\proj", "a.py"],
    [r"C:\proj2\a.py", r"C:\proj", r"C:\proj2\a.py"],
    [r"D:\proj\a.py", r"C:\proj", r"D:\proj\a.py"],
]

CALLER = """
import json, sys
from projectpaths import relativize
print(json.dumps([relativize(path, root) for path, root, _ in json.loads(sys.argv[1])]))
"""


def check(repo: pathlib.Path, final_report: str, transcript: str) -> dict[str, bool]:
    done = subprocess.run(
        [sys.executable, "-B", "-c", CALLER, json.dumps(PROBES)],
        cwd=repo,
        capture_output=True,
        text=True,
        timeout=60,
    )
    # The last line, because a module that prints something on import is no failure of this.
    lines = done.stdout.strip().splitlines()
    try:
        results = json.loads(lines[-1]) if lines else None
    except ValueError:
        results = None
    return {"ntpath_semantics": results == [want for _, _, want in PROBES]}
